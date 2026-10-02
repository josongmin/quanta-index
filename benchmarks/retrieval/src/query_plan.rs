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
//! * `exact_symbol_name` — one bare ASCII identifier becomes a case-sensitive
//!   exact local-name predicate for the symbol route. Other text refuses.
//! * `literal_file` — one raw query becomes a safely escaped lexical phrase
//!   projected to distinct files before top-k truncation. A phrase is a
//!   match-only (constant-score) restriction, so files come back in path
//!   order: the top k is an observed path-ordered prefix, not a ranking.
//! * `keyword_file` — one bare ASCII identifier becomes a case-sensitive,
//!   scored keyword over content and path tokens, projected to distinct files.
//! * `substring_file` — one raw fragment (at least three bytes) becomes a
//!   case-sensitive raw-substring restriction (trigram candidates, byte
//!   verification), projected to distinct files in path order.
//! * `code_search_file` — the raw text is submitted through the public
//!   `code_search` syntax. File projection and scored ordering are part of
//!   that product contract, not an injected Native LQ operator.
//! * `code_search_typo_file` — one bare ASCII identifier of 3..=64 bytes is submitted as
//!   `typo:<identifier>` through the public `code_search` syntax. The raw
//!   query and effective request keep distinct identities.
//!
//! [`ordering_contract`] names how each file-projection policy orders its
//! files; the record binds it next to the result unit.
//!
//! Every plan carries the four identity digests of the canonical profile
//! contract (`docs/adr/SEP-26-001-retrieval-query-publication-and-result-proof.md`
//! §3): the original query, the policy config, the effective lexical
//! request, and the semantic text. Planning happens once per task through
//! [`plan_query`] and the resulting plan is shared by the cold, warmup, and
//! measurement phases.

use crate::sha256_hex;
use quanta_index_contract::{
    MAX_CODE_SEARCH_TERM_BYTES, MAX_CODE_SEARCH_TERMS, MAX_CODE_SEARCH_TYPO_BYTES,
    MIN_CODE_SEARCH_TYPO_BYTES, valid_code_search_typo_identifier,
};
use quanta_index_lq_norm::ast::{
    LqCase, LqExpr, LqFilter, LqLeaf, LqNormalizedQuery, LqOptions, LqSelect, LqType,
};
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
const MAX_EXACT_SYMBOL_NAME_BYTES: usize = 4096;
/// Keyword file search: one identifier within the lexical index term cap.
pub const MAX_KEYWORD_FILE_BYTES: usize = 256;
/// Raw substring file search: the trigram index needs at least three bytes.
pub const MIN_SUBSTRING_FILE_BYTES: usize = 3;
/// Upper bound on a raw substring fragment.
pub const MAX_SUBSTRING_FILE_BYTES: usize = 256;
/// Files ordered by descending engine score, ties by repository/path/span.
pub const ORDERING_SCORE_DESC: &str = "score_desc_path_tiebreak";
/// Match-only (constant-score) restriction: files come back in path order.
pub const ORDERING_PATH_ORDER: &str = "path_order_constant_score";
const CODE_SEARCH_SYNTAX: &str = "code_search";
const MAX_CODE_SEARCH_INPUT_BYTES: usize = 16 * 1024;

/// How a file-projection policy orders its distinct files. `None` for
/// policies whose results are not file projections.
#[must_use]
pub const fn ordering_contract(policy: QueryInputPolicy) -> Option<&'static str> {
    match policy {
        QueryInputPolicy::KeywordFile
        | QueryInputPolicy::CodeSearchFile
        | QueryInputPolicy::CodeSearchTypoFile => Some(ORDERING_SCORE_DESC),
        QueryInputPolicy::LiteralFile | QueryInputPolicy::SubstringFile => {
            Some(ORDERING_PATH_ORDER)
        }
        QueryInputPolicy::Native
        | QueryInputPolicy::Literal
        | QueryInputPolicy::NaturalLanguage
        | QueryInputPolicy::ExactSymbolName => None,
    }
}

#[must_use]
pub const fn execution_profile_id(policy: QueryInputPolicy) -> &'static str {
    match policy {
        QueryInputPolicy::Native => "quanta-native-v1",
        QueryInputPolicy::Literal => "quanta-literal-v1",
        QueryInputPolicy::LiteralFile => "quanta-literal-file-v1",
        QueryInputPolicy::KeywordFile => "quanta-keyword-file-v1",
        QueryInputPolicy::SubstringFile => "quanta-substring-file-v1",
        QueryInputPolicy::CodeSearchFile => "quanta-code-search-file-v1",
        QueryInputPolicy::CodeSearchTypoFile => "quanta-code-search-typo-file-v1",
        QueryInputPolicy::NaturalLanguage => "quanta-natural-language-ucd17-v2",
        QueryInputPolicy::ExactSymbolName => "quanta-exact-symbol-name-v1",
    }
}

/// Explicit caller-selected treatment of a raw query-pack query.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueryInputPolicy {
    /// Pass the raw query through as the native lexical DSL request.
    Native,
    /// Escape the whole raw query into a single phrase literal.
    Literal,
    /// Escape the raw query as a phrase and request file projection.
    LiteralFile,
    /// One bare identifier as a scored keyword with file projection.
    KeywordFile,
    /// One raw fragment as a raw-substring restriction with file projection.
    SubstringFile,
    /// Public product code-search syntax, with scored distinct-file results.
    CodeSearchFile,
    /// Explicit code-search typo mode, scored as distinct files.
    CodeSearchTypoFile,
    /// Keep the raw query for the semantic lane and derive a deterministic
    /// token-OR lexical plan from it.
    NaturalLanguage,
    /// Query an exact, case-sensitive local symbol name on the symbol route.
    ExactSymbolName,
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
            "literal_file" => Ok(Self::LiteralFile),
            "keyword_file" => Ok(Self::KeywordFile),
            "substring_file" => Ok(Self::SubstringFile),
            "code_search_file" => Ok(Self::CodeSearchFile),
            "code_search_typo_file" => Ok(Self::CodeSearchTypoFile),
            "natural_language" => Ok(Self::NaturalLanguage),
            "exact_symbol_name" => Ok(Self::ExactSymbolName),
            other => Err(QueryPlanError::UnsupportedPolicy(other.to_string())),
        }
    }

    /// Canonical policy name (stable wire spelling).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Literal => "literal",
            Self::LiteralFile => "literal_file",
            Self::KeywordFile => "keyword_file",
            Self::SubstringFile => "substring_file",
            Self::CodeSearchFile => "code_search_file",
            Self::CodeSearchTypoFile => "code_search_typo_file",
            Self::NaturalLanguage => "natural_language",
            Self::ExactSymbolName => "exact_symbol_name",
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
    /// The policy string is not one of the canonical policies.
    UnsupportedPolicy(String),
    /// Exact symbol lookup requires one bounded bare ASCII identifier.
    InvalidSymbolName,
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
    /// Native DSL selected a rank projection without an explicit bench profile.
    NativeProjectionRequiresPolicy { projection: String },
    /// Keyword file search requires one bare ASCII identifier that parses
    /// back as exactly one keyword leaf.
    InvalidKeyword,
    /// Substring file search requires a fragment of at least the minimum
    /// bytes, without a single quote or control character, that parses
    /// back as exactly one raw-string leaf.
    InvalidSubstring { reason: &'static str },
    /// Code-search input is empty or exceeds the public query byte cap.
    InvalidCodeSearch,
    /// The typo profile requires a bare ASCII identifier of 3..=64 bytes.
    InvalidCodeSearchTypo,
}

impl QueryPlanError {
    /// Stable refusal code used by the CLI artifact and runner protocol.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::UnsupportedPolicy(_) => "RBR_QUERY_POLICY_UNSUPPORTED",
            Self::InvalidSymbolName => "RBR_QUERY_SYMBOL_NAME_INVALID",
            Self::EmptyTokenPlan => "RBR_QUERY_NO_INDEXABLE_TOKENS",
            Self::TokenLimitExceeded { .. } => "RBR_QUERY_TOKEN_LIMIT_EXCEEDED",
            Self::TokenCharacterLimitExceeded { .. } => "RBR_QUERY_TOKEN_CHAR_LIMIT_EXCEEDED",
            Self::IndexTokenTooLong { .. } => "RBR_QUERY_TOKEN_TOO_LONG",
            Self::InvalidLexicalRequest { .. } => "RBR_QUERY_LEXICAL_INVALID",
            Self::NativeProjectionRequiresPolicy { .. } => "RBR_QUERY_PROJECTION_REQUIRES_POLICY",
            Self::InvalidKeyword => "RBR_QUERY_KEYWORD_INVALID",
            Self::InvalidSubstring { .. } => "RBR_QUERY_SUBSTRING_INVALID",
            Self::InvalidCodeSearch => "RBR_QUERY_CODE_SEARCH_INVALID",
            Self::InvalidCodeSearchTypo => "RBR_QUERY_CODE_SEARCH_TYPO_INVALID",
        }
    }
}

impl std::fmt::Display for QueryPlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedPolicy(raw) => {
                write!(f, "unsupported query input policy: {raw}")
            }
            Self::InvalidSymbolName => write!(
                f,
                "exact-symbol policy requires one bare ASCII symbol name of at most 4096 bytes"
            ),
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
            Self::NativeProjectionRequiresPolicy { projection } => write!(
                f,
                "native projection {projection} requires an explicit benchmark rank profile"
            ),
            Self::InvalidKeyword => write!(
                f,
                "keyword-file policy requires one bare ASCII identifier of at most 256 bytes \
                 that parses as a single keyword"
            ),
            Self::InvalidSubstring { reason } => {
                write!(f, "substring-file policy refused the fragment: {reason}")
            }
            Self::InvalidCodeSearch => write!(f, "code-search-file policy refused the query"),
            Self::InvalidCodeSearchTypo => write!(
                f,
                "code-search-typo-file policy requires one bare ASCII identifier of 3..=64 bytes"
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
        QueryInputPolicy::ExactSymbolName => {
            "{\"case\":\"sensitive\",\"field\":\"symbol.local_name.exact\",\"policy\":\"exact_symbol_name\"}".to_string()
        }
        QueryInputPolicy::Literal => {
            "{\"escaping\":\"lq-norm-phrase-v1\",\"policy\":\"literal\"}".to_string()
        }
        QueryInputPolicy::LiteralFile => {
            "{\"escaping\":\"lq-norm-phrase-v1\",\"policy\":\"literal_file\",\"projection\":\"file\"}".to_string()
        }
        QueryInputPolicy::KeywordFile => format!(
            "{{\"case\":\"sensitive\",\"match\":\"bare_keyword\",\"max_bytes\":{MAX_KEYWORD_FILE_BYTES},\
             \"ordering\":\"{ORDERING_SCORE_DESC}\",\"policy\":\"keyword_file\",\
             \"projection\":\"file\",\"scope\":\"content_and_path\"}}"
        ),
        QueryInputPolicy::SubstringFile => format!(
            "{{\"case\":\"sensitive\",\"match\":\"raw_substring\",\"max_bytes\":{MAX_SUBSTRING_FILE_BYTES},\
             \"min_bytes\":{MIN_SUBSTRING_FILE_BYTES},\"ordering\":\"{ORDERING_PATH_ORDER}\",\
             \"policy\":\"substring_file\",\"projection\":\"file\",\"scope\":\"content\"}}"
        ),
        QueryInputPolicy::CodeSearchFile => format!(
            "{{\"case\":\"folded\",\"match\":\"code_search_v1\",\"ordering\":\"{ORDERING_SCORE_DESC}\",\
             \"policy\":\"code_search_file\",\"projection\":\"file\",\"scope\":\"content_and_path\",\
             \"syntax\":\"{CODE_SEARCH_SYNTAX}\"}}"
        ),
        QueryInputPolicy::CodeSearchTypoFile => format!(
            "{{\"case\":\"folded\",\"match\":\"identifier_typo_v1\",\"max_bytes\":{MAX_CODE_SEARCH_TYPO_BYTES},\
             \"min_bytes\":{MIN_CODE_SEARCH_TYPO_BYTES},\"ordering\":\"{ORDERING_SCORE_DESC}\",\
             \"policy\":\"code_search_typo_file\",\"projection\":\"file\",\"scope\":\"identifier\",\
             \"syntax\":\"{CODE_SEARCH_SYNTAX}\"}}"
        ),
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
        QueryInputPolicy::Native
        | QueryInputPolicy::Literal
        | QueryInputPolicy::LiteralFile
        | QueryInputPolicy::KeywordFile
        | QueryInputPolicy::SubstringFile
        | QueryInputPolicy::CodeSearchFile
        | QueryInputPolicy::CodeSearchTypoFile
        | QueryInputPolicy::ExactSymbolName => format!(
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
        QueryInputPolicy::Native
        | QueryInputPolicy::Literal
        | QueryInputPolicy::LiteralFile
        | QueryInputPolicy::KeywordFile
        | QueryInputPolicy::SubstringFile
        | QueryInputPolicy::CodeSearchFile
        | QueryInputPolicy::CodeSearchTypoFile
        | QueryInputPolicy::ExactSymbolName => serde_json::json!({}),
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

/// Bind both request syntax and text. Native profiles retain their historical
/// text-only digest; a code-search record cannot be relabeled as Native.
#[must_use]
pub fn effective_request_sha256(policy: QueryInputPolicy, lexical_request: &str) -> String {
    if matches!(
        policy,
        QueryInputPolicy::CodeSearchFile | QueryInputPolicy::CodeSearchTypoFile
    ) {
        let wire = serde_json::json!({
            "query_text": lexical_request,
            "syntax": CODE_SEARCH_SYNTAX,
        });
        sha256_hex(wire.to_string().as_bytes())
    } else {
        sha256_hex(lexical_request.as_bytes())
    }
}

fn validate_code_search_benchmark_input(raw: &str) -> Result<(), QueryPlanError> {
    if raw.is_empty() || raw.len() > MAX_CODE_SEARCH_INPUT_BYTES {
        return Err(QueryPlanError::InvalidCodeSearch);
    }
    let terms: Vec<&str> = raw.split_ascii_whitespace().collect();
    if terms.is_empty() || terms.len() > MAX_CODE_SEARCH_TERMS {
        return Err(QueryPlanError::InvalidCodeSearch);
    }
    if terms.iter().any(|term| {
        term.is_empty()
            || term.len() > MAX_CODE_SEARCH_TERM_BYTES
            || !term
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    }) {
        return Err(QueryPlanError::InvalidCodeSearch);
    }
    Ok(())
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
        QueryInputPolicy::ExactSymbolName => {
            let mut bytes = raw.bytes();
            if raw.len() > MAX_EXACT_SYMBOL_NAME_BYTES
                || !bytes
                    .next()
                    .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
                || !bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            {
                return Err(QueryPlanError::InvalidSymbolName);
            }
            format!("symbol.local_name.exact({raw}) case:yes")
        }
        QueryInputPolicy::Literal => {
            validate_indexable_text(raw)?;
            literalize(raw)
        }
        QueryInputPolicy::LiteralFile => {
            validate_indexable_text(raw)?;
            format!("select:file {}", literalize(raw))
        }
        QueryInputPolicy::KeywordFile => {
            let mut bytes = raw.bytes();
            if raw.len() > MAX_KEYWORD_FILE_BYTES
                || !bytes
                    .next()
                    .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
                || !bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            {
                return Err(QueryPlanError::InvalidKeyword);
            }
            format!("select:file case:yes {raw}")
        }
        QueryInputPolicy::SubstringFile => {
            if raw.len() < MIN_SUBSTRING_FILE_BYTES {
                return Err(QueryPlanError::InvalidSubstring {
                    reason: "fragment is shorter than three bytes",
                });
            }
            if raw.len() > MAX_SUBSTRING_FILE_BYTES {
                return Err(QueryPlanError::InvalidSubstring {
                    reason: "fragment is longer than 256 bytes",
                });
            }
            if raw.contains('\'') {
                return Err(QueryPlanError::InvalidSubstring {
                    reason: "a single quote cannot be carried by a raw string",
                });
            }
            if raw.chars().any(char::is_control) {
                return Err(QueryPlanError::InvalidSubstring {
                    reason: "control characters are not searchable fragment text",
                });
            }
            format!("select:file case:yes '{raw}'")
        }
        QueryInputPolicy::CodeSearchFile => {
            validate_code_search_benchmark_input(raw)?;
            raw.to_string()
        }
        QueryInputPolicy::CodeSearchTypoFile => {
            if !valid_code_search_typo_identifier(raw) {
                return Err(QueryPlanError::InvalidCodeSearchTypo);
            }
            format!("typo:{raw}")
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
    // CodeSearch is a distinct public syntax. Parsing it as Native LQ would
    // misread literal operator words and never prove the product request.
    if !matches!(
        policy,
        QueryInputPolicy::CodeSearchFile | QueryInputPolicy::CodeSearchTypoFile
    ) {
        let parsed = validate_lexical_request(&lexical_request)?;
        // The request must parse back to exactly the one leaf the policy built:
        // no operator, filter or option can be smuggled in through the raw text.
        match policy {
            QueryInputPolicy::KeywordFile
                if !matches!(&parsed.expr, LqExpr::Leaf(LqLeaf::Keyword(text)) if text == raw)
                    || !is_case_sensitive_file_projection(&parsed) =>
            {
                return Err(QueryPlanError::InvalidKeyword);
            }
            QueryInputPolicy::SubstringFile
                if !matches!(&parsed.expr, LqExpr::Leaf(LqLeaf::RawString(text)) if text == raw)
                    || !is_case_sensitive_file_projection(&parsed) =>
            {
                return Err(QueryPlanError::InvalidSubstring {
                    reason: "fragment does not parse back as one raw string",
                });
            }
            QueryInputPolicy::Native => {
                for filter in &parsed.filters {
                    if let LqFilter::Select { dim } = filter
                        && !matches!(dim, LqSelect::Content | LqSelect::ContentMatch)
                    {
                        return Err(QueryPlanError::NativeProjectionRequiresPolicy {
                            projection: format!("select:{}", dim.as_str()),
                        });
                    }
                    if let LqFilter::Type { kind } = filter
                        && matches!(kind, LqType::Repo | LqType::Path)
                    {
                        return Err(QueryPlanError::NativeProjectionRequiresPolicy {
                            projection: format!("type:{}", kind.as_str()),
                        });
                    }
                }
            }
            QueryInputPolicy::Literal
            | QueryInputPolicy::LiteralFile
            | QueryInputPolicy::KeywordFile
            | QueryInputPolicy::SubstringFile
            | QueryInputPolicy::CodeSearchFile
            | QueryInputPolicy::CodeSearchTypoFile
            | QueryInputPolicy::NaturalLanguage
            | QueryInputPolicy::ExactSymbolName => {}
        }
    }
    let effective_lexical_request_sha256 = effective_request_sha256(policy, &lexical_request);
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

/// Exactly `select:file` plus `case:yes`, and nothing else around the leaf.
fn is_case_sensitive_file_projection(parsed: &LqNormalizedQuery) -> bool {
    matches!(
        parsed.filters.as_slice(),
        [LqFilter::Select {
            dim: LqSelect::File
        }]
    ) && parsed.directives.is_empty()
        && parsed.options
            == LqOptions {
                case: Some(LqCase::Sensitive),
                ..LqOptions::defaults()
            }
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

fn validate_lexical_request(request: &str) -> Result<LqNormalizedQuery, QueryPlanError> {
    let tokens = tokenize_lq(request).map_err(|error| QueryPlanError::InvalidLexicalRequest {
        parser_code: error.code.as_code_str().to_string(),
        detail: error.to_string(),
    })?;
    let parsed =
        parse_lq(&tokens, request).map_err(|error| QueryPlanError::InvalidLexicalRequest {
            parser_code: error.code.as_code_str().to_string(),
            detail: error.to_string(),
        })?;
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_lq_norm::ast::{LqExpr, LqFilter, LqLeaf, LqSelect};
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
    fn native_projection_requires_an_explicit_rank_profile() {
        let config = NlPlanConfig::default();
        for projection in ["file", "path", "file.owners", "repo", "symbol"] {
            let raw = format!("select:{projection} needle");
            let error = plan_query(QueryInputPolicy::Native, &raw, &config)
                .expect_err("native non-content projection has no rank authority");
            assert_eq!(error.code(), "RBR_QUERY_PROJECTION_REQUIRES_POLICY");
        }
        for projection in ["type:path needle", "type:repo needle"] {
            let error = plan_query(QueryInputPolicy::Native, projection, &config)
                .expect_err("type projection groups result rows");
            assert_eq!(error.code(), "RBR_QUERY_PROJECTION_REQUIRES_POLICY");
        }
        let quoted = plan_query(QueryInputPolicy::Native, "\"select:file\"", &config)
            .expect("quoted projection text is content");
        assert_eq!(quoted.lexical_request, "\"select:file\"");
        let bare = plan_query(QueryInputPolicy::Native, "Next", &config)
            .expect("legacy bare query remains valid");
        assert_eq!(bare.lexical_request, "Next");
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
    fn literal_file_policy_projects_a_bare_query_and_binds_the_request() {
        let config = NlPlanConfig::default();
        let plan = plan_query(QueryInputPolicy::LiteralFile, "writeContentType", &config)
            .expect("bare file query plans");
        assert_eq!(plan.lexical_request, "select:file \"writeContentType\"");
        let tokens = tokenize_lq(&plan.lexical_request).expect("effective DSL tokens");
        let parsed = parse_lq(&tokens, &plan.lexical_request).expect("effective DSL parses");
        assert_eq!(
            parsed.filters,
            vec![LqFilter::Select {
                dim: LqSelect::File
            }]
        );
        assert_eq!(
            parsed.expr,
            LqExpr::Leaf(LqLeaf::Phrase("writeContentType".to_string()))
        );
        assert_eq!(plan.original, "writeContentType");
        assert_eq!(plan.semantic_text, "writeContentType");
        assert_eq!(
            plan.effective_lexical_request_sha256,
            sha256_hex(b"select:file \"writeContentType\"")
        );
        assert_eq!(
            policy_config_canonical(QueryInputPolicy::LiteralFile, &config),
            "{\"escaping\":\"lq-norm-phrase-v1\",\"policy\":\"literal_file\",\"projection\":\"file\"}"
        );
        assert_eq!(
            execution_profile_id(QueryInputPolicy::LiteralFile),
            "quanta-literal-file-v1"
        );
        assert_eq!(
            QueryInputPolicy::parse("literal_file"),
            Ok(QueryInputPolicy::LiteralFile)
        );
        assert_eq!(
            plan_query(QueryInputPolicy::LiteralFile, "---", &config).unwrap_err(),
            QueryPlanError::EmptyTokenPlan
        );
        assert_eq!(
            ordering_contract(QueryInputPolicy::LiteralFile),
            Some(ORDERING_PATH_ORDER)
        );
    }

    #[test]
    fn keyword_file_policy_is_a_scored_case_sensitive_single_keyword() {
        let config = NlPlanConfig::default();
        let plan = plan_query(QueryInputPolicy::KeywordFile, "writeContentType", &config)
            .expect("identifier plans");
        assert_eq!(
            plan.lexical_request,
            "select:file case:yes writeContentType"
        );
        assert_eq!(
            plan.effective_lexical_request_sha256,
            "1421ce83f43daf81860f7f329193f69d42335935efae90108231edfa2a6dc170"
        );
        assert_eq!(
            policy_config_canonical(QueryInputPolicy::KeywordFile, &config),
            "{\"case\":\"sensitive\",\"match\":\"bare_keyword\",\"max_bytes\":256,\
             \"ordering\":\"score_desc_path_tiebreak\",\"policy\":\"keyword_file\",\
             \"projection\":\"file\",\"scope\":\"content_and_path\"}"
        );
        assert_eq!(
            plan.policy_config_sha256,
            "595ce53233c77d2b3e31493f5c5baf82bf973a050e28a248e1bddf4279233724"
        );
        assert_eq!(
            execution_profile_sha256(QueryInputPolicy::KeywordFile, &config),
            "bc30cff5252dbfd1f0eb09d2825da6a77b3c4bb9a7431c9f60e661aaf8325c13"
        );
        assert_eq!(
            ordering_contract(QueryInputPolicy::KeywordFile),
            Some(ORDERING_SCORE_DESC)
        );
        assert_eq!(
            QueryInputPolicy::parse("keyword_file"),
            Ok(QueryInputPolicy::KeywordFile)
        );
        assert_eq!(QueryInputPolicy::KeywordFile.as_str(), "keyword_file");
        let tokens = tokenize_lq(&plan.lexical_request).expect("tokens");
        let parsed = parse_lq(&tokens, &plan.lexical_request).expect("parses");
        assert_eq!(
            parsed.expr,
            LqExpr::Leaf(LqLeaf::Keyword("writeContentType".to_string()))
        );
        assert_eq!(parsed.options.case_mode(), CaseMode::Sensitive);
        // Anything but one bare identifier refuses: no operator, filter,
        // option or phrase can be smuggled through the raw text.
        for raw in [
            "",
            "1abc",
            "a b",
            "a-b",
            "a.b",
            "a:b",
            "select:repo",
            "\"x\"",
            "'x'",
            "x)",
            "Café",
            "a\tb",
            "AND",
            "OR",
            "NOT",
        ] {
            assert!(
                plan_query(QueryInputPolicy::KeywordFile, raw, &config).is_err(),
                "keyword_file accepted {raw:?}"
            );
        }
        let long = "a".repeat(MAX_KEYWORD_FILE_BYTES + 1);
        assert_eq!(
            plan_query(QueryInputPolicy::KeywordFile, &long, &config).unwrap_err(),
            QueryPlanError::InvalidKeyword
        );
        assert!(
            plan_query(
                QueryInputPolicy::KeywordFile,
                &"a".repeat(MAX_KEYWORD_FILE_BYTES),
                &config
            )
            .is_ok()
        );
    }

    #[test]
    fn substring_file_policy_is_a_case_sensitive_raw_string_projection() {
        let config = NlPlanConfig::default();
        let plan = plan_query(QueryInputPolicy::SubstringFile, "ContentTy", &config)
            .expect("fragment plans");
        assert_eq!(plan.lexical_request, "select:file case:yes 'ContentTy'");
        assert_eq!(
            plan.effective_lexical_request_sha256,
            "4493640ee71af3d81d82d313e9a2a8d7afacb3b0612159c0c697c088bcb0c20e"
        );
        assert_eq!(
            policy_config_canonical(QueryInputPolicy::SubstringFile, &config),
            "{\"case\":\"sensitive\",\"match\":\"raw_substring\",\"max_bytes\":256,\
             \"min_bytes\":3,\"ordering\":\"path_order_constant_score\",\
             \"policy\":\"substring_file\",\"projection\":\"file\",\"scope\":\"content\"}"
        );
        assert_eq!(
            plan.policy_config_sha256,
            "d4b7b04281539571608ca48787e034cc9f035c1264bc4ac8b82729a7b2103ea5"
        );
        assert_eq!(
            execution_profile_sha256(QueryInputPolicy::SubstringFile, &config),
            "0bdb593c7b7cb321882500dc1401ae84d893841cb6f1b3ddf4f34e3f0d47ffa5"
        );
        assert_eq!(
            ordering_contract(QueryInputPolicy::SubstringFile),
            Some(ORDERING_PATH_ORDER)
        );
        // DSL-looking text stays one raw string: it is searched as bytes.
        for raw in [
            "abc",
            "a b",
            "x) OR (y",
            "select:repo",
            "\"q\"",
            "a\\b",
            "Café",
            "64Sl",
        ] {
            let plan = match plan_query(QueryInputPolicy::SubstringFile, raw, &config) {
                Ok(plan) => plan,
                Err(error) => panic!("{raw:?} refused: {error}"),
            };
            let tokens = tokenize_lq(&plan.lexical_request).expect("tokens");
            let parsed = parse_lq(&tokens, &plan.lexical_request).expect("parses");
            assert_eq!(
                parsed.expr,
                LqExpr::Leaf(LqLeaf::RawString(raw.to_string()))
            );
            assert_eq!(
                parsed.filters,
                vec![LqFilter::Select {
                    dim: LqSelect::File
                }]
            );
        }
        for (raw, code) in [
            ("ab", "RBR_QUERY_SUBSTRING_INVALID"),
            ("", "RBR_QUERY_SUBSTRING_INVALID"),
            ("a'b", "RBR_QUERY_SUBSTRING_INVALID"),
            ("ab\n", "RBR_QUERY_SUBSTRING_INVALID"),
            ("a\u{0}b", "RBR_QUERY_SUBSTRING_INVALID"),
        ] {
            assert_eq!(
                plan_query(QueryInputPolicy::SubstringFile, raw, &config)
                    .unwrap_err()
                    .code(),
                code,
                "{raw:?}"
            );
        }
        // Three bytes, not three characters: one two-byte scalar is too short.
        assert!(plan_query(QueryInputPolicy::SubstringFile, "é", &config).is_err());
        assert!(
            plan_query(
                QueryInputPolicy::SubstringFile,
                &"x".repeat(MAX_SUBSTRING_FILE_BYTES + 1),
                &config
            )
            .is_err()
        );
    }

    #[test]
    fn code_search_file_binds_public_syntax_and_keeps_short_atoms() {
        let config = NlPlanConfig::default();
        let plan = plan_query(
            QueryInputPolicy::CodeSearchFile,
            "writeContentType",
            &config,
        )
        .expect("bare identifier is valid");
        assert_eq!(plan.lexical_request, "writeContentType");
        assert_eq!(
            plan.effective_lexical_request_sha256,
            "828e78026cd79b527cc0956b3be52fdf0ebd8b110071f5c309963af3bf719480"
        );
        assert_ne!(
            plan.effective_lexical_request_sha256,
            sha256_hex(plan.lexical_request.as_bytes())
        );
        assert_eq!(ordering_contract(plan.policy), Some(ORDERING_SCORE_DESC));
        assert_eq!(
            plan_query(QueryInputPolicy::CodeSearchFile, "Go To", &config)
                .expect("short atoms are admitted by the adapter")
                .lexical_request,
            "Go To"
        );
        assert!(plan_query(QueryInputPolicy::CodeSearchFile, "a", &config).is_ok());
        assert!(plan_query(QueryInputPolicy::CodeSearchFile, "64Sl", &config).is_ok());
        assert!(plan_query(QueryInputPolicy::CodeSearchFile, "foo\tbar\nqux", &config).is_ok());
        let max_terms = ["a"; 32].join(" ");
        let too_many_terms = ["a"; 33].join(" ");
        let max_term = "a".repeat(256);
        let too_long_term = "a".repeat(257);
        assert!(plan_query(QueryInputPolicy::CodeSearchFile, &max_terms, &config).is_ok());
        assert!(plan_query(QueryInputPolicy::CodeSearchFile, &max_term, &config).is_ok());
        for raw in [
            "",
            "select:file",
            "a-b",
            "Café",
            "foo\u{1c}bar",
            "foo\u{1f}bar",
            &too_many_terms,
            &too_long_term,
        ] {
            assert!(matches!(
                plan_query(QueryInputPolicy::CodeSearchFile, raw, &config),
                Err(QueryPlanError::InvalidCodeSearch)
            ));
        }
    }

    #[test]
    fn code_search_typo_file_binds_distinct_product_request() {
        let config = NlPlanConfig::default();
        let raw = "load_jsom";
        let plan = plan_query(QueryInputPolicy::CodeSearchTypoFile, raw, &config)
            .expect("bare typo identifier is valid");
        assert_eq!(plan.original, raw);
        assert_eq!(plan.lexical_request, "typo:load_jsom");
        assert_eq!(plan.semantic_text, raw);
        assert_eq!(
            plan.policy_config_sha256,
            "652b78bd66c84f7019f496b80660b14ab8c60b7a24ff13545f603980e1315cae"
        );
        assert_eq!(
            execution_profile_sha256(plan.policy, &config),
            "2d6247a92e7e6693d3059ab191a5b0ef2fcb93a484c1099fa1c8bed9e3dc1066"
        );
        assert_eq!(
            plan.effective_lexical_request_sha256,
            "4a9305acacf83f7e71ac1af4e65d17ddcddd04499743146f34eb29d1c309965c"
        );
        assert_eq!(ordering_contract(plan.policy), Some(ORDERING_SCORE_DESC));
        assert_eq!(
            execution_profile_id(plan.policy),
            "quanta-code-search-typo-file-v1"
        );
        assert_ne!(
            plan.effective_lexical_request_sha256,
            plan_query(QueryInputPolicy::CodeSearchFile, raw, &config)
                .expect("existing code-search profile")
                .effective_lexical_request_sha256
        );
        assert_eq!(
            execution_profile_sha256(QueryInputPolicy::CodeSearchFile, &config),
            "e39867e466be2ab7f4c5cb779f1fad338a280f5d6669a97c8ed8552486d5ff61"
        );
        for invalid in [
            "",
            "ab",
            "1number",
            "load-jsom",
            "typo:load_jsom",
            "load_jsom extra",
            "Cafés",
        ] {
            assert_eq!(
                plan_query(QueryInputPolicy::CodeSearchTypoFile, invalid, &config)
                    .unwrap_err()
                    .code(),
                "RBR_QUERY_CODE_SEARCH_TYPO_INVALID"
            );
        }
        assert!(
            plan_query(
                QueryInputPolicy::CodeSearchTypoFile,
                &"a".repeat(64),
                &config
            )
            .is_ok()
        );
        assert!(
            plan_query(
                QueryInputPolicy::CodeSearchTypoFile,
                &"a".repeat(65),
                &config
            )
            .is_err()
        );
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
        for raw in ["native", "literal", "natural_language", "exact_symbol_name"] {
            assert!(QueryInputPolicy::parse(raw).is_ok());
        }
    }

    #[test]
    fn exact_symbol_name_policy_keeps_the_raw_identity_and_refuses_dsl_input() {
        let config = NlPlanConfig::default();
        let plan = plan_query(
            QueryInputPolicy::ExactSymbolName,
            "writeContentType",
            &config,
        )
        .expect("bare symbol name plans");
        assert_eq!(
            plan.lexical_request,
            "symbol.local_name.exact(writeContentType) case:yes"
        );
        assert_eq!(plan.original, "writeContentType");
        assert_eq!(plan.semantic_text, "writeContentType");
        assert_eq!(
            plan.effective_lexical_request_sha256,
            sha256_hex(plan.lexical_request.as_bytes())
        );
        assert_eq!(
            plan.policy_config_sha256,
            sha256_hex(
                policy_config_canonical(QueryInputPolicy::ExactSymbolName, &config).as_bytes()
            )
        );
        for invalid in [
            "",
            "two words",
            "select:file Next",
            "WriteContentType)",
            "é",
        ] {
            assert_eq!(
                plan_query(QueryInputPolicy::ExactSymbolName, invalid, &config).unwrap_err(),
                QueryPlanError::InvalidSymbolName
            );
        }
        assert_eq!(
            plan_query(
                QueryInputPolicy::ExactSymbolName,
                &"a".repeat(4097),
                &config
            )
            .unwrap_err(),
            QueryPlanError::InvalidSymbolName
        );
        for name in ["OR", "AND", "case", "select", "_"] {
            let planned = plan_query(QueryInputPolicy::ExactSymbolName, name, &config)
                .expect("bare identifier must remain valid inside a predicate argument");
            assert_eq!(
                planned.lexical_request,
                format!("symbol.local_name.exact({name}) case:yes")
            );
        }
        assert!(
            plan_query(
                QueryInputPolicy::ExactSymbolName,
                &"a".repeat(4096),
                &config
            )
            .is_ok()
        );
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
