//! The one product code-search grammar shared by every executing surface.
//!
//! The grammar lowers to canonical LQ with the product `CodeSearch` pattern
//! type. Native Text executes the same canonical query after
//! [`lower_code_search_native_query`] maps its scoped leaves onto Native
//! Content predicates and path filters. Execution capabilities, work
//! admission and source visibility remain with their engine owners.

use core::fmt::{self, Write as _};

use sha2::{Digest as _, Sha256};

use quanta_index_lq_norm::{
    LQ_VERSION_TAG, LqCase, LqExpr, LqFilter, LqLeaf, LqParseError, LqParseErrorCode,
    LqPatternType, LqPredicateArg, LqSelect, LqSpan, limits::MAX_INPUT_BYTES,
    normalizer::normalize,
};

use super::{LqOptions, LqQuery, MAX_NATIVE_PATH_REGEX_LEAVES, native_content_query_backing_bytes};
use crate::{
    CODE_SEARCH_IDENTIFIER_TYPO_PREDICATE, CODE_SEARCH_SYMBOL_COMPONENTS_PREDICATE,
    MAX_CODE_SEARCH_TERM_BYTES, MAX_CODE_SEARCH_TERMS, valid_code_search_component_query,
    valid_code_search_typo_identifier,
};

const CONTENT_SCOPE: &str = "code_search.content";
const PATH_SCOPE: &str = "code_search.path";
const CONTENT_REGEX_SCOPE: &str = "code_search.content_regex";
const PATH_REGEX_SCOPE: &str = "code_search.path_regex";
const NATIVE_SOURCE_SUBSTRING: &str = "file.contains";
const NATIVE_PATH_SUBSTRING: &str = "path.contains";
const NATIVE_PATH_REGEX: &str = "path.regex";

/// Native Text admits at most this many `file.contains` leaves per query.
pub const MAX_NATIVE_FILE_CONTAINS_LEAVES: usize = 4;
/// Native Text admits at most this many `path.contains` leaves per query.
pub const MAX_NATIVE_PATH_CONTAINS_LEAVES: usize = 4;
/// Native Text admits at most this many `file:` path filters per query.
pub const MAX_NATIVE_PATH_FILTERS: usize = 4;
/// Native Text admits at most this many Content regex leaves per query.
pub const MAX_NATIVE_CONTENT_REGEX_LEAVES: usize = 4;

/// A Native Text leaf kind with a per-query bound.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CodeSearchNativeLeaf {
    /// A Source-body substring, `file.contains`.
    FileContains,
    /// A Source-path substring, `path.contains`.
    PathContains,
    /// A Source-body regex, a Content regex leaf.
    ContentRegex,
    /// A Source-path regex, `path.regex`.
    PathRegex,
}

impl CodeSearchNativeLeaf {
    const fn name(self) -> &'static str {
        match self {
            Self::FileContains => "file.contains leaves",
            Self::PathContains => "path.contains leaves",
            Self::ContentRegex => "content regex leaves",
            Self::PathRegex => "path.regex leaves",
        }
    }
}

/// Typed refusal of a product code-search query.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CodeSearchQueryError {
    /// The closed product grammar or its Native lowering refused the input.
    Grammar(String),
    /// The canonical LQ normalizer refused the lowered query.
    Lq(LqParseError),
    /// The Native lowering needs more leaves of one kind than Native Text
    /// admits in one query.
    NativeFanout {
        leaf: CodeSearchNativeLeaf,
        limit: usize,
    },
}

impl fmt::Display for CodeSearchQueryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Grammar(message) => write!(formatter, "code_search: {message}"),
            Self::Lq(error) => fmt::Display::fmt(error, formatter),
            Self::NativeFanout { leaf, limit } => write!(
                formatter,
                "code_search: Native Text admits at most {limit} {} per query",
                leaf.name()
            ),
        }
    }
}

impl core::error::Error for CodeSearchQueryError {}

impl CodeSearchQueryError {
    /// The public lexical code of this refusal; see [`native_query_refusal_code`].
    #[must_use]
    pub fn lexical_code(&self) -> crate::lex::LexicalErrorCode {
        match self {
            Self::Grammar(_) => crate::lex::LexicalErrorCode::UnsupportedCombo,
            Self::Lq(error) => native_query_refusal_code(error),
            Self::NativeFanout { .. } => crate::lex::LexicalErrorCode::LimitExceededFanout,
        }
    }
}

/// The public lexical code of a refused Native content or code-search query.
///
/// A query shape the Native Text plan cannot execute is `UnsupportedCombo`;
/// every other grammar failure, including input limits, stays `ParseFail`.
#[must_use]
pub fn native_query_refusal_code(error: &LqParseError) -> crate::lex::LexicalErrorCode {
    if error.code == LqParseErrorCode::UnsupportedCombo {
        crate::lex::LexicalErrorCode::UnsupportedCombo
    } else {
        crate::lex::LexicalErrorCode::ParseFail
    }
}

fn invalid(message: impl Into<String>) -> CodeSearchQueryError {
    CodeSearchQueryError::Grammar(message.into())
}

/// Logical backing for parsing and Native-lowering one bounded query.
///
/// It covers the canonical query and its Native rewrite, each funded like one
/// Native content parse. This is an allocation policy, not a measured process
/// RSS ceiling.
#[must_use]
pub fn code_search_query_backing_bytes(input_bytes: usize) -> Option<u64> {
    native_content_query_backing_bytes(input_bytes)?.checked_mul(2)
}

/// Stable public identity of one code-search file result.
///
/// The SHA-256 of a fixed domain and the length-framed repository and path,
/// rendered as `file:<hex>`. Every executing surface reports the same id for
/// the same repository/path, independent of its ranking or storage.
pub fn code_search_file_candidate_id(
    repository: &str,
    path: &str,
) -> Result<String, CodeSearchQueryError> {
    let mut hash = Sha256::new();
    hash.update(b"quanta-index:code-search-file:v1\0");
    for value in [repository, path] {
        let length = u64::try_from(value.len())
            .map_err(|error| invalid(format!("file identity length overflows: {error}")))?;
        hash.update(length.to_le_bytes());
        hash.update(value.as_bytes());
    }
    let mut id = String::from("file:");
    for byte in hash.finalize() {
        write!(id, "{byte:02x}")
            .map_err(|error| invalid(format!("file identity formatting: {error}")))?;
    }
    Ok(id)
}

/// Lower product code search into the canonical LQ query.
///
/// The grammar is deliberately small and closed: unqualified literal terms
/// `ANDed` within one file, quoted contiguous literals, `content:`/`path:`
/// scopes, `regex:/.../` (also scoped), `case:yes|no`, and the explicit
/// single-term `typo:` and `components:` modes. Native LQ remains a separate,
/// explicit syntax.
pub fn parse_code_search_query(query_text: &str) -> Result<LqQuery, CodeSearchQueryError> {
    if query_text.len() > MAX_INPUT_BYTES {
        return Err(invalid("query exceeds the 16 KiB input limit"));
    }
    let words = code_search_words(query_text)?;
    let mut terms = Vec::new();
    let mut case = None;
    for word in words {
        if !word.quoted
            && let Some(value) = word.text.strip_prefix("case:")
        {
            if case.is_some() {
                return Err(invalid("case: may appear only once"));
            }
            case = Some(match value {
                "yes" => LqCase::Sensitive,
                "no" => LqCase::Insensitive,
                _ => return Err(invalid("case: accepts only yes or no")),
            });
            continue;
        }
        if !word.quoted
            && let Some(identifier) = word.text.strip_prefix("typo:")
        {
            if terms.is_empty() && valid_code_search_typo_identifier(identifier) {
                terms.push(LqExpr::Leaf(LqLeaf::Predicate {
                    name: CODE_SEARCH_IDENTIFIER_TYPO_PREDICATE.to_string(),
                    args: vec![LqPredicateArg::RawString(identifier.to_string())],
                }));
                continue;
            }
            return Err(invalid(
                "typo: requires one ASCII identifier of 3..=64 bytes and cannot mix with other terms",
            ));
        }
        if word.scope_prefix_before_quote
            && let Some(value) = word.text.strip_prefix("components:")
        {
            if !terms.is_empty() || !valid_code_search_component_query(value) {
                return Err(invalid(
                    "components: requires 2..=32 lower-case ASCII words and cannot mix with other terms",
                ));
            }
            terms.push(LqExpr::Leaf(LqLeaf::Predicate {
                name: CODE_SEARCH_SYMBOL_COMPONENTS_PREDICATE.to_string(),
                args: vec![LqPredicateArg::RawString(value.to_string())],
            }));
            continue;
        }
        if terms.iter().any(|term| matches!(term, LqExpr::Leaf(LqLeaf::Predicate { name, .. }) if name == CODE_SEARCH_IDENTIFIER_TYPO_PREDICATE || name == CODE_SEARCH_SYMBOL_COMPONENTS_PREDICATE)) {
            return Err(invalid("typed search mode cannot mix with other terms"));
        }
        let scoped = !word.quoted || word.scope_prefix_before_quote;
        let (scope, literal) = if scoped && let Some(value) = word.text.strip_prefix("content:") {
            (Some(CONTENT_SCOPE), value)
        } else if scoped && let Some(value) = word.text.strip_prefix("path:") {
            (Some(PATH_SCOPE), value)
        } else {
            (None, word.text.as_str())
        };
        let literal = if word.regex_delimited {
            literal
                .strip_prefix("regex:")
                .ok_or_else(|| invalid("regex: requires /.../"))?
        } else {
            literal
        };
        if literal.is_empty() {
            return Err(invalid("search term must not be empty"));
        }
        if literal.len() > MAX_CODE_SEARCH_TERM_BYTES {
            return Err(invalid("search term exceeds the 256-byte limit"));
        }
        if !word.regex_delimited
            && scope.is_none()
            && !word.quoted
            && code_search_reserved_word(literal)
        {
            let message = if literal.starts_with("symbol:") {
                "symbol: definition queries use the dedicated symbol route"
            } else if literal.starts_with("regex:") {
                "regex: requires a slash-delimited /.../ pattern"
            } else {
                "this operator or filter is not part of code_search; use native syntax"
            };
            return Err(invalid(message));
        }
        if !word.regex_delimited && literal.starts_with("regex:") && scope.is_some() && !word.quoted
        {
            return Err(invalid("regex: requires /.../"));
        }
        let leaf = match (scope, word.regex_delimited) {
            (None, true) => LqLeaf::Regex(literal.to_string()),
            (Some(name), true) => LqLeaf::Predicate {
                name: format!("{name}_regex"),
                args: vec![LqPredicateArg::RawString(literal.to_string())],
            },
            (Some(name), false) => LqLeaf::Predicate {
                name: name.to_string(),
                args: vec![LqPredicateArg::RawString(literal.to_string())],
            },
            (None, false) => LqLeaf::RawString(literal.to_string()),
        };
        terms.push(LqExpr::Leaf(leaf));
        if terms.len() > MAX_CODE_SEARCH_TERMS {
            return Err(invalid("query exceeds the 32-term limit"));
        }
    }
    if terms.is_empty() {
        return Err(invalid("code_search requires at least one search term"));
    }
    let mut options = LqOptions::defaults();
    options.pattern_type = LqPatternType::CodeSearch;
    options.case = case;
    let source_end = u32::try_from(query_text.len())
        .map_err(|error| invalid(format!("query length exceeds source span range: {error}")))?;
    normalize(LqQuery {
        lq_version: LQ_VERSION_TAG,
        expr: LqExpr::All(terms),
        filters: vec![LqFilter::Select {
            dim: LqSelect::File,
        }],
        directives: Vec::new(),
        options,
        source_span: LqSpan::new(0, source_end),
    })
    .map_err(CodeSearchQueryError::Lq)
}

/// Lower product code search to the LQ shape the Native Text plan executes.
///
/// The result unit is one distinct repository/path group (`select:path`).
/// Every term is one conjunct, under the query's case policy:
///
/// - an unscoped literal matches the Source body or the Source path:
///   `file.contains('t') OR path.contains('t')`;
/// - a `content:` literal is a Source-body substring, `file.contains('t')`;
/// - a `path:` literal is a Source-path substring, `path.contains('t')`;
/// - an unscoped regex matches the Source body or the Source path, with one
///   pattern in both leaves: `/re/ OR path.regex('re')`;
/// - a `content:` regex is a Content regex, `/re/`;
/// - a `path:` regex is a Source-path regex, `path.regex('re')`.
///
/// No pattern gains an inline flag: the query case policy governs every
/// leaf. Native Text admits at most [`MAX_NATIVE_FILE_CONTAINS_LEAVES`] body
/// substrings, [`MAX_NATIVE_PATH_CONTAINS_LEAVES`] path substrings,
/// [`MAX_NATIVE_CONTENT_REGEX_LEAVES`] Content regexes and
/// [`MAX_NATIVE_PATH_REGEX_LEAVES`] path regexes; a query that needs more is
/// refused with [`CodeSearchQueryError::NativeFanout`]. The `typo:` and
/// `components:` modes have no Native plan and refuse.
pub fn lower_code_search_native_query(query_text: &str) -> Result<LqQuery, CodeSearchQueryError> {
    let canonical = parse_code_search_query(query_text)?;
    let parts = match canonical.expr {
        LqExpr::All(parts) => parts,
        leaf @ LqExpr::Leaf(_) => vec![leaf],
        LqExpr::Empty | LqExpr::Not(_) | LqExpr::Any(_) => {
            return Err(invalid(
                "canonical code search must be a conjunction of terms",
            ));
        }
    };
    let mut fanout = NativeFanout::default();
    let mut terms = Vec::new();
    // A path regex may imply the query-scoped `case:no`, as a Content regex
    // does through the normalizer.
    let mut case = canonical.options.case;
    for part in parts {
        let LqExpr::Leaf(leaf) = part else {
            return Err(invalid(
                "canonical code search must be a conjunction of terms",
            ));
        };
        match leaf {
            LqLeaf::RawString(literal) => {
                fanout.add(CodeSearchNativeLeaf::FileContains)?;
                fanout.add(CodeSearchNativeLeaf::PathContains)?;
                terms.push(LqExpr::Any(vec![
                    raw_predicate(NATIVE_SOURCE_SUBSTRING, literal.clone()),
                    raw_predicate(NATIVE_PATH_SUBSTRING, literal),
                ]));
            }
            LqLeaf::Regex(source) => {
                fanout.add(CodeSearchNativeLeaf::ContentRegex)?;
                fanout.add(CodeSearchNativeLeaf::PathRegex)?;
                terms.push(LqExpr::Any(vec![
                    LqExpr::Leaf(LqLeaf::Regex(source.clone())),
                    raw_predicate(NATIVE_PATH_REGEX, source),
                ]));
            }
            LqLeaf::Predicate { name, args } => {
                let value = single_raw_argument(args)?;
                match name.as_str() {
                    CONTENT_SCOPE => {
                        fanout.add(CodeSearchNativeLeaf::FileContains)?;
                        terms.push(raw_predicate(NATIVE_SOURCE_SUBSTRING, value));
                    }
                    CONTENT_REGEX_SCOPE => {
                        fanout.add(CodeSearchNativeLeaf::ContentRegex)?;
                        terms.push(LqExpr::Leaf(LqLeaf::Regex(value)));
                    }
                    PATH_SCOPE => {
                        fanout.add(CodeSearchNativeLeaf::PathContains)?;
                        terms.push(raw_predicate(NATIVE_PATH_SUBSTRING, value));
                    }
                    PATH_REGEX_SCOPE => {
                        fanout.add(CodeSearchNativeLeaf::PathRegex)?;
                        let (pattern, implied) = canonical_path_regex(value, case)?;
                        case = implied;
                        terms.push(raw_predicate(NATIVE_PATH_REGEX, pattern));
                    }
                    CODE_SEARCH_IDENTIFIER_TYPO_PREDICATE => {
                        return Err(invalid("typo: has no Native Text plan"));
                    }
                    CODE_SEARCH_SYMBOL_COMPONENTS_PREDICATE => {
                        return Err(invalid("components: has no Native Text plan"));
                    }
                    _ => return Err(invalid("unknown canonical code_search scope")),
                }
            }
            LqLeaf::Keyword(_) | LqLeaf::Phrase(_) | LqLeaf::StructuralBlock(_) => {
                return Err(invalid("canonical code_search leaf is unsupported"));
            }
        }
    }
    let expr = if terms.len() > 1 {
        LqExpr::All(terms)
    } else {
        terms
            .pop()
            .ok_or_else(|| invalid("code_search requires at least one search term"))?
    };
    let mut options = LqOptions::defaults();
    options.case = case;
    normalize(LqQuery {
        lq_version: LQ_VERSION_TAG,
        expr,
        filters: vec![LqFilter::Select {
            dim: LqSelect::Path,
        }],
        directives: Vec::new(),
        options,
        source_span: canonical.source_span,
    })
    .map_err(CodeSearchQueryError::Lq)
}

/// Leaves of each bounded Native kind spent so far by one lowering.
#[derive(Default)]
struct NativeFanout {
    file_contains: usize,
    path_contains: usize,
    content_regexes: usize,
    path_regexes: usize,
}

impl NativeFanout {
    fn add(&mut self, leaf: CodeSearchNativeLeaf) -> Result<(), CodeSearchQueryError> {
        let (count, limit) = match leaf {
            CodeSearchNativeLeaf::FileContains => {
                (&mut self.file_contains, MAX_NATIVE_FILE_CONTAINS_LEAVES)
            }
            CodeSearchNativeLeaf::PathContains => {
                (&mut self.path_contains, MAX_NATIVE_PATH_CONTAINS_LEAVES)
            }
            CodeSearchNativeLeaf::ContentRegex => {
                (&mut self.content_regexes, MAX_NATIVE_CONTENT_REGEX_LEAVES)
            }
            CodeSearchNativeLeaf::PathRegex => {
                (&mut self.path_regexes, MAX_NATIVE_PATH_REGEX_LEAVES)
            }
        };
        if *count >= limit {
            return Err(CodeSearchQueryError::NativeFanout { leaf, limit });
        }
        *count = count
            .checked_add(1)
            .ok_or(CodeSearchQueryError::NativeFanout { leaf, limit })?;
        Ok(())
    }
}

/// Canonicalize a path regex by the Content regex rules.
///
/// The one canonical normalizer decides: a leading `(?i)` becomes the
/// query's `case:no` and contradicts `case:yes`, and any other inline flag
/// is refused. Returns the canonical pattern and the query case policy it
/// leaves.
fn canonical_path_regex(
    pattern: String,
    case: Option<LqCase>,
) -> Result<(String, Option<LqCase>), CodeSearchQueryError> {
    let mut options = LqOptions::defaults();
    options.case = case;
    let probe = normalize(LqQuery {
        lq_version: LQ_VERSION_TAG,
        expr: LqExpr::Leaf(LqLeaf::Regex(pattern)),
        filters: Vec::new(),
        directives: Vec::new(),
        options,
        source_span: LqSpan::new(0, 0),
    })
    .map_err(CodeSearchQueryError::Lq)?;
    match probe.expr {
        LqExpr::Leaf(LqLeaf::Regex(pattern)) => Ok((pattern, probe.options.case)),
        LqExpr::Empty | LqExpr::Leaf(_) | LqExpr::Not(_) | LqExpr::All(_) | LqExpr::Any(_) => {
            Err(invalid("a path regex lost its canonical shape"))
        }
    }
}

fn raw_predicate(name: &str, literal: String) -> LqExpr {
    LqExpr::Leaf(LqLeaf::Predicate {
        name: name.to_string(),
        args: vec![LqPredicateArg::RawString(literal)],
    })
}

fn single_raw_argument(args: Vec<LqPredicateArg>) -> Result<String, CodeSearchQueryError> {
    let mut args = args.into_iter();
    match (args.next(), args.next()) {
        (Some(LqPredicateArg::RawString(value)), None) => Ok(value),
        _ => Err(invalid(
            "canonical code_search scope requires one raw literal",
        )),
    }
}

struct CodeSearchWord {
    text: String,
    quoted: bool,
    /// Whether `content:` or `path:` appeared before the opening quote.
    scope_prefix_before_quote: bool,
    regex_delimited: bool,
}

fn code_search_words(query_text: &str) -> Result<Vec<CodeSearchWord>, CodeSearchQueryError> {
    let mut words = Vec::new();
    let mut text = String::new();
    let mut quoted = false;
    let mut scope_prefix_before_quote = false;
    let mut in_quotes = false;
    let mut escaped = false;
    let mut just_closed_quote = false;
    let mut in_regex = false;
    let mut regex_delimited = false;
    for ch in query_text.chars() {
        if in_regex {
            if ch.is_control() {
                return Err(invalid("control character in regex"));
            }
            if escaped {
                text.push(ch);
                escaped = false;
            } else if ch == '\\' {
                text.push(ch);
                escaped = true;
            } else if ch == '/' {
                in_regex = false;
                just_closed_quote = true;
            } else {
                text.push(ch);
            }
        } else if in_quotes {
            if ch.is_control() {
                return Err(invalid("control character in quoted literal"));
            }
            if escaped {
                if !matches!(ch, '"' | '\\') {
                    return Err(invalid(r#"quoted literals support only \" and \\ escapes"#));
                }
                text.push(ch);
                escaped = false;
            } else {
                match ch {
                    '\\' => escaped = true,
                    '"' => {
                        in_quotes = false;
                        just_closed_quote = true;
                    }
                    _ => text.push(ch),
                }
            }
        } else if ch.is_whitespace() {
            if !text.is_empty() || quoted {
                words.push(CodeSearchWord {
                    text: std::mem::take(&mut text),
                    quoted,
                    scope_prefix_before_quote,
                    regex_delimited,
                });
                quoted = false;
                scope_prefix_before_quote = false;
                regex_delimited = false;
                just_closed_quote = false;
            }
        } else if just_closed_quote {
            return Err(invalid("quoted literal must end at a term boundary"));
        } else if ch == '/' && matches!(text.as_str(), "regex:" | "content:regex:" | "path:regex:")
        {
            in_regex = true;
            regex_delimited = true;
        } else if ch == '"' {
            if !text.is_empty() && !matches!(text.as_str(), "content:" | "path:" | "components:") {
                return Err(invalid("quotes must start a term or scoped value"));
            }
            in_quotes = true;
            quoted = true;
            scope_prefix_before_quote = !text.is_empty();
        } else if ch == '\\' {
            return Err(invalid("escape bare backslashes with a quoted literal"));
        } else if ch.is_control() {
            return Err(invalid("control character in search term"));
        } else {
            text.push(ch);
        }
    }
    if in_quotes || in_regex || escaped {
        return Err(invalid("unterminated quoted literal or regex"));
    }
    if !text.is_empty() || quoted {
        words.push(CodeSearchWord {
            text,
            quoted,
            scope_prefix_before_quote,
            regex_delimited,
        });
    }
    Ok(words)
}

fn code_search_reserved_word(literal: &str) -> bool {
    // Explicitly refuse syntax that would otherwise look like an inert
    // literal while the user intended a filter or different search lane.
    [
        "select:",
        "type:",
        "repo:",
        "lang:",
        "file:",
        "rev:",
        "regex:",
        "symbol:",
        "components:",
        "patterntype:",
    ]
    .iter()
    .any(|prefix| literal.starts_with(prefix))
}

#[cfg(test)]
#[path = "code_search_tests.rs"]
mod tests;
