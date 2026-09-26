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
//! contract (`docs/adr/SEP-26-001-retrieval-query-publication-and-result-proof.md`
//! §3): the original query, the policy config, the effective lexical
//! request, and the semantic text. Planning happens once per task through
//! [`plan_query`] and the resulting plan is shared by the cold, warmup, and
//! measurement phases.

use crate::sha256_hex;
use quanta_index_lq_norm::{parser::parse as parse_lq, tokenizer::tokenize as tokenize_lq};
use quanta_index_lq_text_normalizer::{
    CaseMode, TEXT_NORMALIZER_VERSION, TextQueryError, is_token_char, nfc, query_tokens,
};

/// Fixed natural-language plan profile identifier (part of the policy
/// config identity).
pub const NL_PLAN_PROFILE: &str = "nl-token-or-v2";

/// Whether the one-time planning cost is included in measured query
/// latency. Planning happens once per task before the cold probe, so the
/// measured windows never contain it.
pub const PLANNING_COST_IN_LATENCY: bool = false;

#[must_use]
pub const fn execution_profile_id(policy: QueryInputPolicy) -> &'static str {
    match policy {
        QueryInputPolicy::Native => "quanta-native-v1",
        QueryInputPolicy::Literal => "quanta-literal-v1",
        QueryInputPolicy::NaturalLanguage => "quanta-natural-language-ucd17-v2",
    }
}

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
             \"text_normalizer_version\":\"{TEXT_NORMALIZER_VERSION}\",\
             \"tokenization\":\"lexical-ssot-nfc-with-path-joiners\"",
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
    /// A single planned token exceeded the profile's character maximum.
    TokenCharacterLimitExceeded {
        /// Observed token character length.
        chars: usize,
        /// Configured maximum.
        max_token_chars: usize,
    },
    /// A lexical token exceeds the index term byte cap and cannot match.
    IndexTokenTooLong {
        /// Observed token byte length after NFC normalization.
        bytes: usize,
        /// Canonical lexical index byte limit.
        max_bytes: usize,
    },
    /// The effective lexical request failed the canonical tokenizer/parser.
    InvalidLexicalRequest {
        /// Canonical lq-norm error code.
        parser_code: String,
        /// Parser diagnostic detail.
        detail: String,
    },
}

impl QueryPlanError {
    /// Stable refusal code used by the CLI artifact and runner protocol.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::UnsupportedPolicy(_) => "RBR_QUERY_POLICY_UNSUPPORTED",
            Self::EmptyTokenPlan => "RBR_QUERY_NO_INDEXABLE_TOKENS",
            Self::TokenLimitExceeded { .. } => "RBR_QUERY_TOKEN_LIMIT_EXCEEDED",
            Self::TokenCharacterLimitExceeded { .. } => "RBR_QUERY_TOKEN_CHAR_LIMIT_EXCEEDED",
            Self::IndexTokenTooLong { .. } => "RBR_QUERY_TOKEN_TOO_LONG",
            Self::InvalidLexicalRequest { .. } => "RBR_QUERY_LEXICAL_INVALID",
        }
    }
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
                write!(
                    f,
                    "natural-language plan has {tokens} tokens (max {max_tokens})"
                )
            }
            Self::TokenCharacterLimitExceeded {
                chars,
                max_token_chars,
            } => write!(f, "token of {chars} chars exceeds max {max_token_chars}"),
            Self::IndexTokenTooLong { bytes, max_bytes } => {
                write!(
                    f,
                    "token of {bytes} bytes exceeds lexical term max {max_bytes}"
                )
            }
            Self::InvalidLexicalRequest {
                parser_code,
                detail,
            } => write!(
                f,
                "effective lexical request is invalid ({parser_code}): {detail}"
            ),
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

/// Canonical capture-scoped execution profile. The exact bytes are shared
/// with the independent replay oracle and hashed into every capture.
#[must_use]
pub fn execution_profile_canonical(policy: QueryInputPolicy, config: &NlPlanConfig) -> String {
    let profile_id = execution_profile_id(policy);
    match policy {
        QueryInputPolicy::NaturalLanguage => format!(
            "{{\"config\":{{\"max_token_chars\":{},\"max_tokens\":{},\"min_token_chars\":{}}},\
             \"planning_cost_in_latency\":false,\"policy\":\"{}\",\"profile_id\":\"{}\"}}",
            config.max_token_chars,
            config.max_tokens,
            config.min_token_chars,
            policy.as_str(),
            profile_id,
        ),
        QueryInputPolicy::Native | QueryInputPolicy::Literal => format!(
            "{{\"config\":{{}},\"planning_cost_in_latency\":false,\"policy\":\"{}\",\
             \"profile_id\":\"{}\"}}",
            policy.as_str(),
            profile_id,
        ),
    }
}

#[must_use]
pub fn execution_profile_value(
    policy: QueryInputPolicy,
    config: &NlPlanConfig,
) -> serde_json::Value {
    let config_value = match policy {
        QueryInputPolicy::NaturalLanguage => serde_json::json!({
            "max_token_chars": config.max_token_chars,
            "max_tokens": config.max_tokens,
            "min_token_chars": config.min_token_chars,
        }),
        QueryInputPolicy::Native | QueryInputPolicy::Literal => serde_json::json!({}),
    };
    serde_json::json!({
        "profile_id": execution_profile_id(policy),
        "policy": policy.as_str(),
        "config": config_value,
        "planning_cost_in_latency": PLANNING_COST_IN_LATENCY,
    })
}

#[must_use]
pub fn execution_profile_sha256(policy: QueryInputPolicy, config: &NlPlanConfig) -> String {
    sha256_hex(execution_profile_canonical(policy, config).as_bytes())
}

/// Escape one raw string into a single lq-norm double-quoted phrase literal.
///
/// The lq-norm phrase lexer decodes exactly `\\`, `\"`, `\n`, `\r`, `\t`
/// and rejects every other escape. Escaping precisely those five characters
/// round-trips every other `char` (including Unicode) verbatim.
#[must_use]
pub fn literalize(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len().saturating_add(2));
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

/// Deterministic natural-language tokenization.
///
/// A token is a maximal run of Unicode alphanumeric characters joined by
/// `-`, `_`, `.`, or `/`. Whitespace and every other punctuation/symbol
/// character separates tokens. Case is preserved.
#[must_use]
pub fn tokenize_nl(raw: &str) -> Vec<String> {
    fn joins(ch: char) -> bool {
        ch == '-' || ch == '_' || ch == '.' || ch == '/'
    }
    let mut tokens: Vec<String> = Vec::new();
    let mut current = String::new();
    let normalized = nfc(raw);
    for ch in normalized.chars() {
        if is_token_char(ch) || joins(ch) {
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
        QueryInputPolicy::Literal => {
            validate_indexable_text(raw)?;
            literalize(raw)
        }
        QueryInputPolicy::NaturalLanguage => {
            let mut distinct: Vec<String> = Vec::new();
            for token in tokenize_nl(raw) {
                let chars = token.chars().count();
                if chars > config.max_token_chars {
                    return Err(QueryPlanError::TokenCharacterLimitExceeded {
                        chars,
                        max_token_chars: config.max_token_chars,
                    });
                }
                if chars < config.min_token_chars {
                    continue;
                }
                match query_tokens(&token, CaseMode::Folded) {
                    Ok(_) => {}
                    Err(TextQueryError::NoTokens) => continue,
                    Err(TextQueryError::TokenTooLong { bytes, max }) => {
                        return Err(QueryPlanError::IndexTokenTooLong {
                            bytes,
                            max_bytes: max,
                        });
                    }
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
    validate_lexical_request(&lexical_request)?;
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

fn validate_indexable_text(raw: &str) -> Result<(), QueryPlanError> {
    match query_tokens(raw, CaseMode::Folded) {
        Ok(_) => Ok(()),
        Err(TextQueryError::NoTokens) => Err(QueryPlanError::EmptyTokenPlan),
        Err(TextQueryError::TokenTooLong { bytes, max }) => {
            Err(QueryPlanError::IndexTokenTooLong {
                bytes,
                max_bytes: max,
            })
        }
    }
}

fn validate_lexical_request(request: &str) -> Result<(), QueryPlanError> {
    let tokens = tokenize_lq(request).map_err(|error| QueryPlanError::InvalidLexicalRequest {
        parser_code: error.code.as_code_str().to_string(),
        detail: error.to_string(),
    })?;
    let _parsed =
        parse_lq(&tokens, request).map_err(|error| QueryPlanError::InvalidLexicalRequest {
            parser_code: error.code.as_code_str().to_string(),
            detail: error.to_string(),
        })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_lq_norm::tokenizer::{LqTokenKind, tokenize};
    use sha2::{Digest, Sha256};

    fn phrase_round_trip(raw: &str) {
        let literal = literalize(raw);
        let tokens = tokenize(&literal).expect("literalize output must tokenize");
        let kinds: Vec<LqTokenKind> = tokens.iter().map(|token| token.kind.clone()).collect();
        assert_eq!(
            kinds,
            vec![LqTokenKind::Phrase(raw.to_string()), LqTokenKind::Eof],
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
        let raw =
            "Where does parse_and_expression handle ( AND ) tokens? See parse_and_expression!";
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
        let err = plan_query(
            QueryInputPolicy::NaturalLanguage,
            "?? !!",
            &NlPlanConfig::default(),
        )
        .expect_err("punctuation-only query has no tokens");
        assert_eq!(err, QueryPlanError::EmptyTokenPlan);
    }

    #[test]
    fn natural_language_plan_drops_joiner_only_runs() {
        let plan = plan_query(
            QueryInputPolicy::NaturalLanguage,
            "foo --- bar",
            &NlPlanConfig::default(),
        )
        .expect("indexable terms remain");
        assert_eq!(plan.lexical_request, "\"foo\" OR \"bar\"");

        let err = plan_query(
            QueryInputPolicy::NaturalLanguage,
            "--- ... ///",
            &NlPlanConfig::default(),
        )
        .expect_err("joiner-only query has no indexable token");
        assert_eq!(err, QueryPlanError::EmptyTokenPlan);
    }

    #[test]
    fn natural_language_plan_normalizes_nfc_before_identity_and_emission() {
        let composed = plan_query(
            QueryInputPolicy::NaturalLanguage,
            "caf\u{e9}",
            &NlPlanConfig::default(),
        )
        .expect("composed input plans");
        let decomposed = plan_query(
            QueryInputPolicy::NaturalLanguage,
            "cafe\u{301}",
            &NlPlanConfig::default(),
        )
        .expect("decomposed input plans");
        assert_eq!(composed.lexical_request, "\"caf\u{e9}\"");
        assert_eq!(decomposed.lexical_request, composed.lexical_request);
        assert_eq!(
            decomposed.effective_lexical_request_sha256,
            composed.effective_lexical_request_sha256
        );
        assert_ne!(
            decomposed.original_query_sha256,
            composed.original_query_sha256
        );
    }

    #[test]
    fn natural_language_plan_enforces_index_term_byte_cap() {
        let accepted = "\u{ac00}".repeat(85);
        let _accepted = plan_query(
            QueryInputPolicy::NaturalLanguage,
            &accepted,
            &NlPlanConfig::default(),
        )
        .expect("255-byte Hangul token is indexable");

        let refused = "\u{ac00}".repeat(86);
        let err = plan_query(
            QueryInputPolicy::NaturalLanguage,
            &refused,
            &NlPlanConfig::default(),
        )
        .expect_err("258-byte Hangul token is not indexable");
        assert_eq!(err.code(), "RBR_QUERY_TOKEN_TOO_LONG");
    }

    #[test]
    fn literal_policy_refuses_unindexable_text() {
        let err = plan_query(QueryInputPolicy::Literal, "---", &NlPlanConfig::default())
            .expect_err("phrase without index terms must refuse before execution");
        assert_eq!(err, QueryPlanError::EmptyTokenPlan);
    }

    #[test]
    fn native_policy_enforces_exact_parser_input_byte_boundary() {
        let accepted = format!("\"{}\"", "a".repeat(16_382));
        assert_eq!(accepted.len(), 16 * 1024);
        let _accepted_plan = plan_query(
            QueryInputPolicy::Native,
            &accepted,
            &NlPlanConfig::default(),
        )
        .expect("exact 16 KiB phrase request is accepted");

        let refused = format!("\"{}\"", "a".repeat(16_383));
        let error = plan_query(QueryInputPolicy::Native, &refused, &NlPlanConfig::default())
            .expect_err("request above 16 KiB is refused");
        assert_eq!(error.code(), "RBR_QUERY_LEXICAL_INVALID");
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
            QueryPlanError::TokenCharacterLimitExceeded {
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
        let native =
            plan_query(QueryInputPolicy::Native, "fix the bug", &config).expect("native plan");
        let literal =
            plan_query(QueryInputPolicy::Literal, "fix the bug", &config).expect("literal plan");
        let nl =
            plan_query(QueryInputPolicy::NaturalLanguage, "fix the bug", &config).expect("nl plan");

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
            sha256_hex(
                policy_config_canonical(QueryInputPolicy::NaturalLanguage, &config).as_bytes()
            )
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

    #[test]
    fn execution_profile_has_stable_capture_scoped_identity() {
        let config = NlPlanConfig::default();
        let profile = execution_profile_value(QueryInputPolicy::NaturalLanguage, &config);
        assert_eq!(
            profile.get("profile_id"),
            Some(&serde_json::json!("quanta-natural-language-ucd17-v2"))
        );
        assert_eq!(
            profile.pointer("/config/max_tokens"),
            Some(&serde_json::json!(32))
        );
        assert_eq!(
            sha256_hex(
                crate::canonical::canonical_json(&profile)
                    .expect("canonical profile")
                    .as_bytes()
            ),
            execution_profile_sha256(QueryInputPolicy::NaturalLanguage, &config)
        );
    }

    #[test]
    fn unicode_scalar_token_property_matches_pinned_ucd17_oracle() {
        let mut digest = Sha256::new();
        let mut count = 0_u32;
        for scalar in 0_u32..=0x10_FFFF {
            let included = char::from_u32(scalar).is_some_and(is_token_char);
            digest.update([u8::from(included)]);
            count += u32::from(included);
        }
        assert_eq!(count, 150_270);
        assert_eq!(
            format!("{:x}", digest.finalize()),
            "e711b4f9486890857e77db0d581642531caa7b41d4350a5911b84ea4ea862b24"
        );
        for (input, expected) in [
            ("cafe\u{301}", "caf\u{e9}"),
            ("A\u{30a}", "\u{c5}"),
            ("\u{1100}\u{1161}", "\u{ac00}"),
            ("\u{212b}", "\u{c5}"),
        ] {
            assert_eq!(nfc(input), expected);
        }
    }
}
