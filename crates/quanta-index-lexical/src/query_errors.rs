//! Mapping the planners' and engines' failures to typed errors, and the shard types the authorities hold.

#![expect(
    clippy::redundant_pub_crate,
    reason = "the module is private to the crate; `pub(crate)` is the visibility its items need across the crate's modules, and the workspace's `unreachable_pub = deny` forbids the bare `pub`"
)]

use crate::normalize;
use crate::normalize::{CaseMode, TextQueryError};
use crate::phrase::PhrasePlannerError;
use quanta_index_core::CoreError;
use quanta_index_lq_positions::{PositionsError, PositionsErrorCode};
use quanta_index_lq_trigram::{TrigramError, TrigramErrorCode};

pub(crate) fn map_trigram_error(context: &str, err: &TrigramError) -> CoreError {
    match err.code {
        TrigramErrorCode::PlanLimitExceeded => CoreError::Typed {
            code: "LEX_TRIGRAM_PLAN_LIMIT_EXCEEDED".to_string(),
            message: format!("lexical: {context}: {err}"),
        },
        TrigramErrorCode::RegexPrefilterUnusable => CoreError::Typed {
            code: "LEX_TRIGRAM_PREFILTER_UNUSABLE".to_string(),
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
        code: err.code().to_string(),
        message: format!("lexical: {err}"),
    }
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
            code: "LEX_PHRASE_PLAN_LIMIT_EXCEEDED".to_string(),
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
