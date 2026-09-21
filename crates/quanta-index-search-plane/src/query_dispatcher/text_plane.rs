//! Executable text-plane policy shared by the history and runtime-metadata
//! routes: filter/leaf admissibility and in-memory boolean text matching.
//!
//! The in-memory predicate is the same contract the lexical index and the
//! history text index execute, through the one text normalizer
//! (QI-BB-011): a keyword or phrase leaf is a whole-token sequence match
//! after NFC and the query's case mode; a raw string is an NFC substring
//! under the same mode; a filter pattern (`author:`, `file:`, `diff.*:`,
//! …) is an NFC substring under the same mode. The case mode is the DSL's
//! one default (`LqOptions::case_mode`): folded unless `case:yes`. The
//! history route's `recency` order runs this predicate over every row,
//! and its `relevance` order runs the same predicate over every row the
//! text index enumerates, so the two orders admit the same rows
//! (QI-BB-023 보완 #3).

use quanta_index_contract::{LqExpr, LqFilter, LqLeaf, LqOptions, LqQuery, LqType};
use quanta_index_core::CoreError;
use quanta_index_lq_text_normalizer::{
    CaseMode, TextQueryError, Token, contains_phrase, contains_substring, nfc, query_tokens,
    tokenize,
};

use crate::query_dispatcher::timeref::{
    parse_runtime_changed_scope_ms, parse_runtime_stale_scope_ms,
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct ExecutableTextPlaneValidationState {
    saw_runtime_authority_filter: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ExecutableTextPlanePolicy {
    History,
    RuntimeMetadata,
}

impl ExecutableTextPlanePolicy {
    const fn plane_name(self) -> &'static str {
        match self {
            Self::History => "history",
            Self::RuntimeMetadata => "runtime metadata",
        }
    }

    fn validate_filter(
        self,
        filter: &LqFilter,
        state: &mut ExecutableTextPlaneValidationState,
    ) -> Result<(), CoreError> {
        match self {
            Self::History => match filter {
                LqFilter::Type { kind } => match kind {
                    LqType::Commit | LqType::Diff => Ok(()),
                    LqType::File | LqType::Path | LqType::Symbol | LqType::Repo => {
                        Err(CoreError::NotImplemented(format!(
                            "history: type filter `{}` is not executable on the current adapter set",
                            kind.as_str()
                        )))
                    }
                },
                LqFilter::File { .. }
                | LqFilter::Rev { .. }
                | LqFilter::Author { .. }
                | LqFilter::Committer { .. }
                | LqFilter::Message { .. }
                | LqFilter::Before { .. }
                | LqFilter::After { .. }
                | LqFilter::Since { .. }
                | LqFilter::Until { .. }
                | LqFilter::DiffAdded { .. }
                | LqFilter::DiffRemoved { .. }
                | LqFilter::DiffTouched { .. }
                | LqFilter::Content { .. } => Ok(()),
                LqFilter::Repo { .. }
                | LqFilter::Lang { .. }
                | LqFilter::Select { .. }
                | LqFilter::Dirty { .. }
                | LqFilter::Changed { .. }
                | LqFilter::Stale { .. }
                | LqFilter::Snapshot { .. }
                | LqFilter::MetaOwner { .. }
                | LqFilter::MetaService { .. }
                | LqFilter::MetaLayer { .. }
                | LqFilter::MetaSurface { .. }
                | LqFilter::Affected { .. }
                | LqFilter::InvalidatedBy { .. }
                | LqFilter::Fork { .. }
                | LqFilter::Archived { .. }
                | LqFilter::Visibility { .. }
                | LqFilter::Context { .. } => Err(CoreError::NotImplemented(
                    "history: one or more filters are not executable on the current adapter set"
                        .to_string(),
                )),
            },
            Self::RuntimeMetadata => match filter {
                LqFilter::Changed { scope } => {
                    state.saw_runtime_authority_filter = true;
                    let _: u64 = parse_runtime_changed_scope_ms(scope)?;
                    Ok(())
                }
                LqFilter::Stale { scope } => {
                    state.saw_runtime_authority_filter = true;
                    let _: u64 = parse_runtime_stale_scope_ms(scope)?;
                    Ok(())
                }
                // `Dirty` carries a yes/no/only mode but, like the snapshot /
                // meta / edge authority filters, only needs to record that a
                // runtime-authority filter was seen at planning time.
                LqFilter::Dirty { .. }
                | LqFilter::Snapshot { .. }
                | LqFilter::MetaOwner { .. }
                | LqFilter::MetaService { .. }
                | LqFilter::MetaLayer { .. }
                | LqFilter::MetaSurface { .. }
                | LqFilter::Affected { .. }
                | LqFilter::InvalidatedBy { .. } => {
                    state.saw_runtime_authority_filter = true;
                    Ok(())
                }
                LqFilter::File { .. } | LqFilter::Lang { .. } | LqFilter::Content { .. } => {
                    Ok(())
                }
                LqFilter::Repo { .. }
                | LqFilter::Rev { .. }
                | LqFilter::Author { .. }
                | LqFilter::Committer { .. }
                | LqFilter::Message { .. }
                | LqFilter::Before { .. }
                | LqFilter::After { .. }
                | LqFilter::Since { .. }
                | LqFilter::Until { .. }
                | LqFilter::DiffAdded { .. }
                | LqFilter::DiffRemoved { .. }
                | LqFilter::DiffTouched { .. }
                | LqFilter::Type { .. }
                | LqFilter::Select { .. }
                | LqFilter::Fork { .. }
                | LqFilter::Archived { .. }
                | LqFilter::Visibility { .. }
                | LqFilter::Context { .. } => Err(CoreError::NotImplemented(
                    "runtime metadata: one or more filters are not executable on the current adapter set"
                        .to_string(),
                )),
            },
        }
    }

    fn finalize(self, state: ExecutableTextPlaneValidationState) -> Result<(), CoreError> {
        match self {
            Self::History => Ok(()),
            Self::RuntimeMetadata => {
                if !state.saw_runtime_authority_filter {
                    return Err(CoreError::InvalidContract(
                        "runtime metadata: at least one runtime authority filter is required (dirty/changed/stale/snapshot/meta.*/affected/invalidated_by)"
                            .to_string(),
                    ));
                }
                Ok(())
            }
        }
    }
}

pub(super) fn validate_executable_text_query(
    query: &LqQuery,
    policy: ExecutableTextPlanePolicy,
) -> Result<(), CoreError> {
    if query.options.timeout_ms.is_some() {
        return Err(CoreError::InvalidContract(format!(
            "{}: timeout option is not executable on the current adapter set",
            policy.plane_name()
        )));
    }
    let mut state = ExecutableTextPlaneValidationState::default();
    for filter in &query.filters {
        policy.validate_filter(filter, &mut state)?;
    }
    policy.finalize(state)?;
    validate_executable_text_surface(&query.expr, &query.options, policy)?;
    for filter in &query.filters {
        if let LqFilter::Content { leaf } = filter {
            validate_leaf_surface(leaf, &query.options, policy)?;
        }
    }
    Ok(())
}

fn validate_executable_text_surface(
    expr: &LqExpr,
    options: &LqOptions,
    policy: ExecutableTextPlanePolicy,
) -> Result<(), CoreError> {
    match expr {
        LqExpr::Empty => Ok(()),
        LqExpr::Leaf(leaf) => validate_leaf_surface(leaf, options, policy),
        LqExpr::Not(inner) => validate_executable_text_surface(inner, options, policy),
        LqExpr::All(children) | LqExpr::Any(children) => {
            for child in children {
                validate_executable_text_surface(child, options, policy)?;
            }
            Ok(())
        }
    }
}

/// Refuse a leaf the plane cannot execute, at plan time.
///
/// A keyword or phrase literal that lowers to no token (or to a run past
/// the term cap) is refused typed here — the same code the lexical route
/// answers — rather than matching nothing row by row, so a query over an
/// empty snapshot is refused exactly like one over a full snapshot.
fn validate_leaf_surface(
    leaf: &LqLeaf,
    options: &LqOptions,
    policy: ExecutableTextPlanePolicy,
) -> Result<(), CoreError> {
    let plane = policy.plane_name();
    match leaf {
        LqLeaf::Keyword(literal) | LqLeaf::Phrase(literal) => {
            let _tokens: Vec<Token> = literal_tokens(plane, literal, options.case_mode())?;
            Ok(())
        }
        LqLeaf::RawString(_) => Ok(()),
        LqLeaf::Regex(_) => Err(CoreError::NotImplemented(format!(
            "{plane}: regex leaves are not executable on the current adapter set"
        ))),
        LqLeaf::StructuralBlock(_) => Err(CoreError::NotImplemented(format!(
            "{plane}: structural leaves are not executable on this route"
        ))),
        LqLeaf::Predicate { .. } => Err(CoreError::NotImplemented(format!(
            "{plane}: predicate leaves are not executable on this route"
        ))),
    }
}

/// The tokens of a keyword or phrase literal under `case`, or the typed
/// refusal every route shares for a literal the token surfaces cannot
/// express.
fn literal_tokens(plane: &str, literal: &str, case: CaseMode) -> Result<Vec<Token>, CoreError> {
    query_tokens(literal, case).map_err(|err| CoreError::Typed {
        code: match err {
            TextQueryError::NoTokens => {
                quanta_index_contract::SearchPlaneErrorCodeV2::LexTextQueryNoTokens
            }
            TextQueryError::TokenTooLong { .. } => {
                quanta_index_contract::SearchPlaneErrorCodeV2::LexTextQueryTokenTooLong
            }
        },
        message: format!("{plane}: {err}"),
    })
}

pub(super) fn expr_matches<F>(expr: &LqExpr, leaf_matches: &mut F) -> Result<bool, CoreError>
where
    F: FnMut(&LqLeaf) -> Result<bool, CoreError>,
{
    match expr {
        LqExpr::Empty => Ok(true),
        LqExpr::Leaf(leaf) => leaf_matches(leaf),
        LqExpr::Not(inner) => Ok(!expr_matches(inner, leaf_matches)?),
        LqExpr::All(children) => {
            for child in children {
                if !expr_matches(child, leaf_matches)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        LqExpr::Any(children) => {
            for child in children {
                if expr_matches(child, leaf_matches)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
    }
}

/// Whether `text` matches one leaf: a keyword or phrase as a whole-token
/// sequence, a raw string as a substring, both after NFC under the
/// query's case mode (see the module doc).
pub(super) fn leaf_matches_text(
    plane: &str,
    leaf: &LqLeaf,
    text: &str,
    options: &LqOptions,
) -> Result<bool, CoreError> {
    let case = options.case_mode();
    match leaf {
        LqLeaf::Keyword(literal) | LqLeaf::Phrase(literal) => {
            let wanted = literal_tokens(plane, literal, case)?;
            let present: Vec<Token> = tokenize(text, case).indexable().cloned().collect();
            Ok(contains_phrase(&present, &wanted))
        }
        LqLeaf::RawString(literal) => Ok(matches_text(literal, text, options)),
        LqLeaf::Regex(_) => Err(CoreError::NotImplemented(format!(
            "{plane}: regex leaves are not executable on the current adapter set"
        ))),
        LqLeaf::StructuralBlock(_) => Err(CoreError::NotImplemented(format!(
            "{plane}: structural leaves are not executable on this route"
        ))),
        LqLeaf::Predicate { .. } => Err(CoreError::NotImplemented(format!(
            "{plane}: predicate leaves are not executable on this route"
        ))),
    }
}

/// Whether `haystack` contains `needle` as an NFC substring under the
/// query's case mode: the raw-string and filter-pattern contract.
pub(super) fn matches_text(needle: &str, haystack: &str, options: &LqOptions) -> bool {
    contains_substring(nfc(haystack).as_ref(), needle, options.case_mode())
}

#[cfg(test)]
mod tests {
    use quanta_index_contract::{LqCase, LqLeaf, LqOptions};
    use quanta_index_core::CoreError;

    use super::{leaf_matches_text, matches_text};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn options(case: Option<LqCase>) -> LqOptions {
        LqOptions {
            case,
            ..LqOptions::defaults()
        }
    }

    /// The predicate is the lexical index's contract: whole tokens after
    /// NFC and the per-character fold, folded by default.
    #[test]
    fn keywords_match_whole_folded_tokens_and_raw_strings_substrings() -> TestResult {
        let keyword = |text: &str| LqLeaf::Keyword(text.to_string());
        let phrase = |text: &str| LqLeaf::Phrase(text.to_string());
        let raw = |text: &str| LqLeaf::RawString(text.to_string());
        let folded = options(None);
        let cases: [(&LqLeaf, &str, &LqOptions, bool); 12] = [
            (&keyword("fix"), "Fix typo", &folded, true),
            (&keyword("fix"), "prefix", &folded, false),
            (
                &keyword("fix"),
                "fix bug",
                &options(Some(LqCase::Insensitive)),
                true,
            ),
            (
                &keyword("fix"),
                "Fix typo",
                &options(Some(LqCase::Sensitive)),
                false,
            ),
            (&keyword("ΟΔΟΣ"), "οδοσ σου", &folded, true),
            (&keyword("οδος"), "ΟΔΟΣ", &folded, false),
            (&keyword("café"), "CAFE\u{301} au lait", &folded, true),
            (&phrase("foo bar"), "x foo.bar y", &folded, true),
            (&phrase("foo bar"), "foo x bar", &folded, false),
            (&raw("x.y"), "fix the 'x.y' literal", &folded, true),
            (&raw("X.Y"), "x.y", &options(Some(LqCase::Sensitive)), false),
            (&raw("fix"), "prefix", &folded, true),
        ];
        for (leaf, text, options, expected) in cases {
            let observed = leaf_matches_text("test", leaf, text, options)?;
            if observed != expected {
                return Err(format!(
                    "{leaf:?} over {text:?} with case {:?}: got {observed}, expected {expected}",
                    options.case
                )
                .into());
            }
        }
        Ok(())
    }

    #[test]
    fn a_literal_without_a_token_is_refused_typed() -> TestResult {
        match leaf_matches_text(
            "test",
            &LqLeaf::Keyword("👍".to_string()),
            "👍",
            &options(None),
        ) {
            Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::LexTextQueryNoTokens,
                ..
            }) => Ok(()),
            other => Err(format!("expected the shared typed refusal, got {other:?}").into()),
        }
    }

    #[test]
    fn filter_patterns_are_nfc_substrings_under_the_case_mode() {
        assert!(matches_text("alice", "Alice Liddell", &options(None)));
        assert!(!matches_text(
            "alice",
            "Alice Liddell",
            &options(Some(LqCase::Sensitive))
        ));
        assert!(matches_text("café", "CAFE\u{301}", &options(None)));
        assert!(matches_text("src/", "src/lib.rs", &options(None)));
    }
}
