//! Mapping the planners' and engines' failures to typed errors, and the shard types the authorities hold.

#![expect(
    clippy::redundant_pub_crate,
    reason = "the module is private to the crate; `pub(crate)` is the visibility its items need across the crate's modules, and the workspace's `unreachable_pub = deny` forbids the bare `pub`"
)]

use crate::normalize;
use crate::normalize::{CaseMode, TextQueryError};
use crate::phrase::PhrasePlannerError;
use quanta_index_contract::SearchPlaneErrorCodeV2 as Code;
use quanta_index_core::CoreError;
use quanta_index_lq_positions::{PositionsError, PositionsErrorCode};
use quanta_index_lq_regex::{RegexErrorCode, RegexExecutor};
use quanta_index_lq_trigram::{TrigramError, TrigramErrorCode};

pub(crate) fn map_trigram_error(context: &str, err: &TrigramError) -> CoreError {
    match err.code {
        TrigramErrorCode::PlanLimitExceeded => CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::LexTrigramPlanLimitExceeded,
            message: format!("lexical: {context}: {err}"),
        },
        TrigramErrorCode::RegexPrefilterUnusable => CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::LexTrigramPrefilterUnusable,
            message: format!("lexical: {context}: {err}"),
        },
        TrigramErrorCode::IndexDeserialize | TrigramErrorCode::IndexCorrupted => {
            CoreError::Storage(format!("lexical: {context}: {err}"))
        }
        TrigramErrorCode::InvalidGeneration => {
            CoreError::InvalidContract(format!("lexical: {context}: {err}"))
        }
    }
}

/// The typed refusal for a keyword or phrase literal the token surfaces
/// cannot express; the code is the normalizer's, shared by every route.
pub(crate) fn map_text_query_error(err: &TextQueryError) -> CoreError {
    CoreError::Typed {
        code: match err {
            TextQueryError::NoTokens => Code::LexTextQueryNoTokens,
            TextQueryError::TokenTooLong { .. } => Code::LexTextQueryTokenTooLong,
        },
        message: format!("lexical: {err}"),
    }
}

/// Preserve the historical `LEX_REGEX_` namespace without synthesizing a
/// wire code from an arbitrary string. Every lower-domain variant is owned.
pub(crate) const fn regex_wire_code(code: RegexErrorCode) -> Code {
    match code {
        RegexErrorCode::ParseFail => Code::LexRegexParseFail,
        RegexErrorCode::ForbiddenSyntax => Code::LexRegexForbiddenSyntax,
        RegexErrorCode::PlanLimitExceeded => Code::LexRegexPlanLimitExceeded,
        RegexErrorCode::RegexPrefilterUnusable => Code::LexRegexRegexPrefilterUnusable,
        RegexErrorCode::QueryTimeout => Code::LexRegexQueryTimeout,
        RegexErrorCode::Interrupted => Code::LexRegexInterrupted,
        RegexErrorCode::ExecutionInternal => Code::LexRegexExecutionInternal,
    }
}

/// Tantivy scope regexes use a different grammar from LQ content regexes,
/// but must hit the same input-byte gate before Tantivy starts compiling.
pub(crate) fn admit_scope_regex_pattern_size(pattern: &str) -> Result<(), CoreError> {
    RegexExecutor::validate_pattern_size(pattern).map_err(|err| CoreError::Typed {
        code: regex_wire_code(err.code),
        message: format!("lexical: regex filter input refused: {err}"),
    })
}

/// Tokenize a keyword or phrase literal for lowering, refusing typed when
/// it has no token or a run past the term cap.
pub(crate) fn text_query_tokens(
    text: &str,
    case: CaseMode,
) -> Result<Vec<normalize::Token>, CoreError> {
    normalize::query_tokens(text, case).map_err(|err| map_text_query_error(&err))
}

/// Lower a phrase-planner error to the typed keyword codes or a contract fault.
///
/// The planner's literal refusals share the keyword codes, since both leaves
/// lower through the same tokenizer; its other errors are contract faults of
/// the plan itself.
pub(crate) fn map_phrase_plan_error(err: PhrasePlannerError) -> CoreError {
    match err {
        PhrasePlannerError::EmptyPhrase => map_text_query_error(&TextQueryError::NoTokens),
        PhrasePlannerError::TokenTooLong { bytes, max } => {
            map_text_query_error(&TextQueryError::TokenTooLong { bytes, max })
        }
        other @ (PhrasePlannerError::TooFewTokens { .. }
        | PhrasePlannerError::UnsupportedSlop { .. }
        | PhrasePlannerError::UnsupportedField { .. }) => {
            CoreError::InvalidContract(format!("lexical: phrase plan: {other}"))
        }
    }
}

/// Fold one regex literal alternative for the `case:no` trigram prefilter.
///
/// An alternative is a byte prefix of some match. The extractor may have
/// cut it inside a multi-byte character, so only the longest well-formed
/// UTF-8 prefix is kept — a shorter prefix of a match is still a prefix —
/// and it is folded with the same per-char fold that built the folded copy.
/// An alternative with no well-formed prefix folds to nothing, which the
/// prefilter refuses as unusable rather than filtering anything away.
pub(crate) fn fold_literal_prefix(literal: &[u8]) -> Vec<u8> {
    let complete = literal
        .utf8_chunks()
        .next()
        .map_or("", |chunk| chunk.valid());
    normalize::fold(complete).into_bytes()
}

pub(crate) fn map_positions_error(context: &str, err: &PositionsError) -> CoreError {
    match err.code {
        PositionsErrorCode::PlanLimitExceeded => CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::LexPhrasePlanLimitExceeded,
            message: format!("lexical: {context}: {err}"),
        },
        PositionsErrorCode::StateGenerationRegression
        | PositionsErrorCode::NormalizerVersionMismatch
        | PositionsErrorCode::IndexDeserialize
        | PositionsErrorCode::IndexCorrupted => {
            CoreError::Storage(format!("lexical: {context}: {err}"))
        }
        PositionsErrorCode::InvalidTerm | PositionsErrorCode::WindowOutOfRange => {
            CoreError::InvalidContract(format!("lexical: {context}: {err}"))
        }
    }
}
