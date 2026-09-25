//! Query input policy planning (RBR-02).
//!
//! The bench adapter converts each blind query-pack query into the concrete
//! per-lane requests under one explicit [`QueryInputPolicy`]:
//!
//! * `native` — the raw query is the lexical DSL request unchanged, so the
//!   public `a b` AND semantics are preserved verbatim.
//! * `literal` — the whole raw string is emitted as one safely escaped
//!   lq-norm phrase literal, so no raw text is ever reinterpreted as DSL
//!   operators, filters, or predicates.
//! * `natural_language` — the raw query stays the semantic text, while the
//!   lexical lane receives a deterministic token-OR plan built from the
//!   query alone (fixed tokenization, dedup, limits, escaping). Empty or
//!   over-limit plans are typed refusals; there is no match-all fallback.
//!
//! Every plan carries the four identity digests of the canonical profile
//! contract (`docs/plans/sep-26-retrieval-remediation/tickets/PROFILE-CONTRACT.md`
//! §3): the original query, the policy config, the effective lexical
//! request, and the semantic text. Planning happens once per task through
//! [`plan_query`] and the resulting plan is shared by the cold, warmup, and
//! measurement phases.

use crate::sha256_hex;

/// Fixed natural-language plan profile identifier (part of the policy
/// config identity).
pub const NL_PLAN_PROFILE: &str = "nl-token-or-v1";

/// Whether the one-time planning cost is included in measured query
/// latency. Planning happens once per task before the cold probe, so the
/// measured windows never contain it.
pub const PLANNING_COST_IN_LATENCY: bool = false;

/// Explicit caller-selected treatment of a raw query-pack query.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueryInputPolicy {
    /// Pass the raw query through as the native lexical DSL request.
    Native,
    /// Escape the whole raw query into a single phrase literal.
    Literal,
    /// Keep the raw query for the semantic lane and derive a deterministic
    /// token-OR lexical plan from it.
    NaturalLanguage,
}

impl QueryInputPolicy {
    /// Parse the CLI/protocol spelling. Unknown values are typed refusals,
    /// never silently mapped to a default.
    ///
    /// # Errors
    ///
    /// Returns [`QueryPlanError::UnsupportedPolicy`] for any other string.
    pub fn parse(raw: &str) -> Result<Self, QueryPlanError> {
        match raw {
            "native" => Ok(Self::Native),
            "literal" => Ok(Self::Literal),
            "natural_language" => Ok(Self::NaturalLanguage),
            other => Err(QueryPlanError::UnsupportedPolicy(other.to_string())),
        }
    }

    /// Canonical policy name (stable wire spelling).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Literal => "literal",
            Self::NaturalLanguage => "natural_language",
        }
    }
}

/// Fixed profile configuration of the natural-language token plan. Every
/// field participates in the policy-config identity: changing a value
/// changes `policy_config_sha256` and invalidates frozen evidence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NlPlanConfig {
    /// Maximum number of distinct tokens in one plan.
    pub max_tokens: usize,
    /// Maximum `char` length of a single token; longer raw tokens refuse.
    pub max_token_chars: usize,
    /// Minimum `char` length; shorter tokens are dropped before dedup.
    pub min_token_chars: usize,
}

impl Default for NlPlanConfig {
    fn default() -> Self {
        Self {
            max_tokens: 32,
            max_token_chars: 96,
            min_token_chars: 1,
        }
    }
}

impl NlPlanConfig {
    /// Canonical (sorted-key, fixed-order) JSON fragment of the plan
    /// config fields used by the policy-config identity.
    #[must_use]
    pub fn canonical_fields(&self) -> String {
        format!(
            "\"max_token_chars\":{},\"max_tokens\":{},\"min_token_chars\":{},\
             \"profile\":\"{NL_PLAN_PROFILE}\",\
             \"tokenization\":\"unicode-alnum-joined-punct\"",
            self.max_token_chars, self.max_tokens, self.min_token_chars
        )
    }
}

/// Typed refusals of query planning. No variant ever falls back to a
/// broader or match-all request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QueryPlanError {
    /// The policy string is not one of the three canonical policies.
    UnsupportedPolicy(String),
    /// The natural-language plan produced no tokens after tokenization.
    EmptyTokenPlan,
    /// The distinct-token count exceeded the configured maximum.
    TokenLimitExceeded {
        /// Observed distinct token count.
        tokens: usize,
        /// Configured maximum.
        max_tokens: usize,
    },
    /// A single token exceeded the configured per-token character maximum.
    TokenTooLong {
        /// Observed token character length.
        chars: usize,
        /// Configured maximum.
        max_token_chars: usize,
    },
}

impl std::fmt::Display for QueryPlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedPolicy(raw) => {
                write!(f, "unsupported query input policy: {raw}")
            }
            Self::EmptyTokenPlan => {
                write!(f, "natural-language plan produced no tokens")
            }
            Self::TokenLimitExceeded { tokens, max_tokens } => {
                write!(f, "natural-language plan has {tokens} tokens (max {max_tokens})")
            }
            Self::TokenTooLong {
                chars,
                max_token_chars,
            } => write!(f, "token of {chars} chars exceeds max {max_token_chars}"),
        }
    }
}

impl std::error::Error for QueryPlanError {}

/// One planned query: the concrete per-lane requests plus the four
/// contract identity digests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueryPlan {
    /// Policy that produced this plan.
    pub policy: QueryInputPolicy,
    /// Caller-provided raw query, preserved verbatim.
    pub original: String,
    /// Exact lexical-lane request (DSL text, literal phrase, or token-OR
    /// plan depending on the policy).
    pub lexical_request: String,
    /// Exact semantic-lane text (the raw query under every policy).
    pub semantic_text: String,
    /// SHA-256 of the raw query bytes.
    pub original_query_sha256: String,
    /// SHA-256 of the canonical policy-config JSON.
    pub policy_config_sha256: String,
    /// SHA-256 of the effective lexical request bytes.
    pub effective_lexical_request_sha256: String,
    /// SHA-256 of the semantic text bytes.
    pub semantic_text_sha256: String,
    /// Whether planning cost is inside measured query latency (fixed by
    /// [`PLANNING_COST_IN_LATENCY`]).
    pub planning_cost_in_latency: bool,
}

/// Canonical policy-config JSON for a policy and plan configuration. The
/// spelling of every policy is part of the frozen profile identity.
#[must_use]
pub fn policy_config_canonical(policy: QueryInputPolicy, config: &NlPlanConfig) -> String {
    match policy {
        QueryInputPolicy::Native => "{\"policy\":\"native\"}".to_string(),
        QueryInputPolicy::Literal => {
            "{\"escaping\":\"lq-norm-phrase-v1\",\"policy\":\"literal\"}".to_string()
        }
        QueryInputPolicy::NaturalLanguage => format!(
            "{{\"escaping\":\"lq-norm-phrase-v1\",\"policy\":\"natural_language\",{}}}",
            config.canonical_fields()
        ),
    }
}

/// Escape one raw string into a single lq-norm double-quoted phrase
/// literal. The lq-norm phrase lexer decodes exactly `\\`, `\"`, `\n`,
/// `\r`, `\t` and rejects every other escape, so escaping precisely those
/// five characters round-trips every other `char` (including Unicode)
/// verbatim.
#[must_use]
pub fn literalize(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len() + 2);
    out.push('"');
    for ch in raw.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// Deterministic natural-language tokenization: a token is a maximal run
/// of Unicode alphanumeric characters joined by `-`, `_`, `.`, or `/`.
/// Whitespace and every other punctuation/symbol character separates
/// tokens. Case is preserved.
#[must_use]
pub fn tokenize_nl(raw: &str) -> Vec<String> {
    fn joins(ch: char) -> bool {
        ch == '-' || ch == '_' || ch == '.' || ch == '/'
    }
    let mut tokens: Vec<String> = Vec::new();
    let mut current = String::new();
    for ch in raw.chars() {
        if ch.is_alphanumeric() || joins(ch) {
            current.push(ch);
        } else if !current.is_empty() {
            tokens.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// Plan one raw query under an explicit policy.
///
/// # Errors
///
/// Returns a [`QueryPlanError`] typed refusal for an unsupported policy or
/// for an empty / over-limit natural-language plan. It never substitutes a
/// broader request.
pub fn plan_query(
    policy: QueryInputPolicy,
    raw: &str,
    config: &NlPlanConfig,
) -> Result<QueryPlan, QueryPlanError> {
    let lexical_request = match policy {
        QueryInputPolicy::Native => raw.to_string(),
        QueryInputPolicy::Literal => literalize(raw),
        QueryInputPolicy::NaturalLanguage => {
            let mut distinct: Vec<String> = Vec::new();
            for token in tokenize_nl(raw) {
                let chars = token.chars().count();
                if chars > config.max_token_chars {
                    return Err(QueryPlanError::TokenTooLong {
                        chars,
                        max_token_chars: config.max_token_chars,
                    });
                }
                if chars < config.min_token_chars {
                    continue;
                }
                if !distinct.iter().any(|seen| seen == &token) {
                    distinct.push(token);
                }
            }
            if distinct.is_empty() {
                return Err(QueryPlanError::EmptyTokenPlan);
            }
            if distinct.len() > config.max_tokens {
                return Err(QueryPlanError::TokenLimitExceeded {
                    tokens: distinct.len(),
                    max_tokens: config.max_tokens,
                });
            }
            distinct
                .iter()
                .map(|token| literalize(token))
                .collect::<Vec<String>>()
                .join(" OR ")
        }
    };
    let effective_lexical_request_sha256 = sha256_hex(lexical_request.as_bytes());
    Ok(QueryPlan {
        policy,
        original: raw.to_string(),
        lexical_request,
        semantic_text: raw.to_string(),
        original_query_sha256: sha256_hex(raw.as_bytes()),
        policy_config_sha256: sha256_hex(policy_config_canonical(policy, config).as_bytes()),
        effective_lexical_request_sha256,
        semantic_text_sha256: sha256_hex(raw.as_bytes()),
        planning_cost_in_latency: PLANNING_COST_IN_LATENCY,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_lq_norm::tokenizer::{LqTokenKind, tokenize};

    fn phrase_round_trip(raw: &str) {
        let literal = literalize(raw);
        let tokens = tokenize(&literal).expect("literalize output must tokenize");
        let kinds: Vec<LqTokenKind> = tokens.iter().map(|token| token.kind.clone()).collect();
        assert_eq!(
            kinds,
            vec![
                LqTokenKind::Phrase(raw.to_string()),
                LqTokenKind::Eof
            ],
            "literal of {raw:?} must be exactly one phrase"
        );
    }

    #[test]
    fn literal_policy_round_trips_adversarial_strings() {
        for raw in [
            "plain",
            "with space and\ttab",
            "quote \" inside",
            "backslash \\ inside",
            "both \" and \\ mixed",
            "colon:filter and operators AND OR NOT ( ) - repo:has.file(x)",
            "qualified::identifier snake_case kebab-case path/to/file.rs",
            "unicode 한글 검색 τεστ emoji 🚀",
            "newline\nand\rcarriage\ttab",
            "''single quotes stay literal''",
            "",
        ] {
            phrase_round_trip(raw);
        }
    }

    #[test]
    fn native_policy_preserves_and_semantics_verbatim() {
        let plan = plan_query(
            QueryInputPolicy::Native,
            "parse_and_expression depth",
            &NlPlanConfig::default(),
        )
        .expect("native planning is total");
        assert_eq!(plan.lexical_request, "parse_and_expression depth");
        assert_eq!(plan.semantic_text, "parse_and_expression depth");
        // The token-OR plan must NOT be active under native policy.
        assert!(!plan.lexical_request.contains(" OR "));
    }

    #[test]
    fn literal_policy_emits_single_phrase_request() {
        let plan = plan_query(
            QueryInputPolicy::Literal,
            "find AND fix: the `parse` bug",
            &NlPlanConfig::default(),
        )
        .expect("literal planning is total");
        assert_eq!(plan.lexical_request, "\"find AND fix: the `parse` bug\"");
        phrase_round_trip("find AND fix: the `parse` bug");
    }

    #[test]
    fn natural_language_plan_is_deterministic_token_or() {
        let config = NlPlanConfig::default();
        let raw = "Where does parse_and_expression handle ( AND ) tokens? See parse_and_expression!";
        let plan = plan_query(QueryInputPolicy::NaturalLanguage, raw, &config)
            .expect("nonempty query plans");
        assert_eq!(
            plan.lexical_request,
            "\"Where\" OR \"does\" OR \"parse_and_expression\" OR \"handle\" OR \"AND\" OR \"tokens\" OR \"See\""
        );
        // Semantic text stays the raw query.
        assert_eq!(plan.semantic_text, raw);
        // Planning is deterministic.
        let again = plan_query(QueryInputPolicy::NaturalLanguage, raw, &config)
            .expect("nonempty query plans");
        assert_eq!(plan, again);
    }

    #[test]
    fn natural_language_plan_dedups_preserving_first_occurrence_and_case() {
        let plan = plan_query(
            QueryInputPolicy::NaturalLanguage,
            "Find find FIND the THE function",
            &NlPlanConfig::default(),
        )
        .expect("dedup plan");
        assert_eq!(
            plan.lexical_request,
            "\"Find\" OR \"find\" OR \"FIND\" OR \"the\" OR \"THE\" OR \"function\""
        );
    }

    #[test]
    fn natural_language_plan_keeps_joined_punctuation_tokens() {
        let plan = plan_query(
            QueryInputPolicy::NaturalLanguage,
            "open crates/quanta-index-lexical/src/lib.rs and src/query.rs",
            &NlPlanConfig::default(),
        )
        .expect("path-like tokens plan");
        assert_eq!(
            plan.lexical_request,
            "\"open\" OR \"crates/quanta-index-lexical/src/lib.rs\" OR \"and\" OR \"src/query.rs\""
        );
    }

    #[test]
    fn natural_language_plan_refuses_empty_token_set() {
        let err = plan_query(QueryInputPolicy::NaturalLanguage, "?? !!", &NlPlanConfig::default())
            .expect_err("punctuation-only query has no tokens");
        assert_eq!(err, QueryPlanError::EmptyTokenPlan);
    }

    #[test]
    fn natural_language_plan_refuses_over_limit_plans() {
        let config = NlPlanConfig {
            max_tokens: 2,
            ..NlPlanConfig::default()
        };
        let err = plan_query(QueryInputPolicy::NaturalLanguage, "a b c", &config)
            .expect_err("three distinct tokens exceed the limit");
        assert_eq!(
            err,
            QueryPlanError::TokenLimitExceeded {
                tokens: 3,
                max_tokens: 2
            }
        );
    }

    #[test]
    fn natural_language_plan_refuses_oversized_single_token() {
        let config = NlPlanConfig {
            max_token_chars: 8,
            ..NlPlanConfig::default()
        };
        let err = plan_query(QueryInputPolicy::NaturalLanguage, "abcdefghi", &config)
            .expect_err("single long token refuses");
        assert_eq!(
            err,
            QueryPlanError::TokenTooLong {
                chars: 9,
                max_token_chars: 8
            }
        );
    }

    #[test]
    fn natural_language_plan_applies_min_token_filter_before_dedup() {
        let config = NlPlanConfig {
            min_token_chars: 3,
            ..NlPlanConfig::default()
        };
        let plan = plan_query(QueryInputPolicy::NaturalLanguage, "a bb ccc a", &config)
            .expect("filtered plan remains nonempty");
        assert_eq!(plan.lexical_request, "\"ccc\"");
    }

    #[test]
    fn unknown_policy_is_a_typed_refusal() {
        assert_eq!(
            QueryInputPolicy::parse("english").unwrap_err(),
            QueryPlanError::UnsupportedPolicy("english".to_string())
        );
        for raw in ["native", "literal", "natural_language"] {
            assert!(QueryInputPolicy::parse(raw).is_ok());
        }
    }

    #[test]
    fn identity_digests_bind_original_policy_and_effective_requests() {
        let config = NlPlanConfig::default();
        let native = plan_query(QueryInputPolicy::Native, "fix the bug", &config)
            .expect("native plan");
        let literal = plan_query(QueryInputPolicy::Literal, "fix the bug", &config)
            .expect("literal plan");
        let nl = plan_query(QueryInputPolicy::NaturalLanguage, "fix the bug", &config)
            .expect("nl plan");

        // Same original text: original and semantic digests match.
        assert_eq!(native.original_query_sha256, literal.original_query_sha256);
        assert_eq!(native.semantic_text_sha256, nl.semantic_text_sha256);

        // Different policies produce different effective lexical requests
        // and different policy-config digests.
        assert_ne!(
            native.effective_lexical_request_sha256,
            literal.effective_lexical_request_sha256
        );
        assert_ne!(
            literal.effective_lexical_request_sha256,
            nl.effective_lexical_request_sha256
        );
        assert_ne!(native.policy_config_sha256, literal.policy_config_sha256);
        assert_ne!(literal.policy_config_sha256, nl.policy_config_sha256);

        // Digests are exactly the SHA-256 of the corresponding bytes.
        assert_eq!(
            nl.effective_lexical_request_sha256,
            sha256_hex(nl.lexical_request.as_bytes())
        );
        assert_eq!(
            nl.policy_config_sha256,
            sha256_hex(policy_config_canonical(QueryInputPolicy::NaturalLanguage, &config).as_bytes())
        );
        assert!(!nl.planning_cost_in_latency);
    }

    #[test]
    fn policy_config_identity_changes_with_config_fields() {
        let base = NlPlanConfig::default();
        let tuned = NlPlanConfig {
            max_tokens: 16,
            ..NlPlanConfig::default()
        };
        assert_ne!(
            policy_config_canonical(QueryInputPolicy::NaturalLanguage, &base),
            policy_config_canonical(QueryInputPolicy::NaturalLanguage, &tuned)
        );
        // Canonical spellings are stable strings.
        assert_eq!(
            policy_config_canonical(QueryInputPolicy::Native, &base),
            "{\"policy\":\"native\"}"
        );
    }
}
