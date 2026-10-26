//! Canonical Native content-query construction shared by transport consumers.
//!
//! This owns grammar construction only. Execution capabilities, work admission
//! and source visibility remain with their existing engine owners.

use quanta_index_lq_norm::{
    LqExpr, LqParseError, LqParseErrorCode, LqSpan, limits,
    normalizer::normalize,
    parser::parse,
    tokenizer::{LqToken, LqTokenKind, tokenize},
};

use super::LqQuery;

/// Canonical maximum raw content-query length, checked before tokenization.
pub const MAX_NATIVE_CONTENT_INPUT_BYTES: usize = limits::MAX_INPUT_BYTES;

/// Logical backing for lexing, parsing and normalization of a bounded query.
///
/// Callers requiring custody acquire this grant before construction and retain
/// it until the owned query is destroyed or adopts the engine's plan grant.
/// This is an allocation policy, not a measured process RSS ceiling.
#[must_use]
pub fn native_content_query_backing_bytes(input_bytes: usize) -> Option<u64> {
    if input_bytes > MAX_NATIVE_CONTENT_INPUT_BYTES {
        return None;
    }
    // Every non-EOF token consumes at least one source byte. Include Vec
    // growth, parser copies, normalized AST and depth-bounded sort keys.
    let slots = input_bytes.checked_add(1)?;
    let token = std::mem::size_of::<LqToken>();
    let expression = std::mem::size_of::<LqExpr>();
    let string = std::mem::size_of::<String>();
    let per_token = token
        .checked_mul(2)?
        .checked_add(expression.checked_mul(8)?)?
        .checked_add(string.checked_mul(4)?)?
        .checked_add(128)?;
    let depth = match usize::try_from(limits::MAX_AST_DEPTH) {
        Ok(value) => value,
        Err(_overflow) => return None,
    };
    let string_copies = depth.checked_add(16)?;
    let bytes = slots
        .checked_mul(per_token)?
        .checked_add(input_bytes.checked_mul(string_copies)?)?;
    size_bytes(bytes)
}

/// Construct Native content syntax using the existing canonical LQ grammar.
///
/// Structural engine tokens are rejected before their parser plans are born.
/// Regex preparation requires separate admission by custodial callers.
/// Engine preflight still owns supported
/// filters, options, count limits and execution admission.
pub fn parse_native_content_query(input: &str) -> Result<LqQuery, LqParseError> {
    tokenize_native_content_query(input)?.parse()
}

/// Canonical tokens retained between lexer admission and regex preparation.
pub struct NativeContentQueryTokens<'a> {
    input: &'a str,
    tokens: Vec<LqToken>,
    regex_count: u64,
}

impl NativeContentQueryTokens<'_> {
    /// Conservative logical HIR backing; acquire before calling `parse`.
    /// Engine execution and its retained automata require separate admission.
    #[must_use]
    pub fn preparation_backing_bytes(&self) -> Option<u64> {
        self.regex_count
            .checked_mul(100_000 * 128)?
            .checked_add(if self.regex_count == 0 {
                0
            } else {
                size_bytes(self.input.len())?.checked_mul(256)?
            })
    }

    pub fn parse(self) -> Result<LqQuery, LqParseError> {
        normalize(parse(&self.tokens, self.input)?)
    }
}

/// Tokenize once, refusing unsupported engines before their parser guards.
pub fn tokenize_native_content_query(
    input: &str,
) -> Result<NativeContentQueryTokens<'_>, LqParseError> {
    if input.len() > MAX_NATIVE_CONTENT_INPUT_BYTES {
        return Err(LqParseError::new(
            LqParseErrorCode::LimitExceededBytes,
            LqSpan::synthetic(0),
            "Native content query exceeds canonical input bound",
        ));
    }
    let tokens = tokenize(input)?;
    if let Some(token) = tokens.iter().find(|token| {
        matches!(
            &token.kind,
            LqTokenKind::RawString(_)
                | LqTokenKind::StructuralBlock(_)
                | LqTokenKind::Predicate { .. }
        )
    }) {
        return Err(LqParseError::new(
            LqParseErrorCode::UnsupportedCombo,
            token.span,
            "native BM25 requires Content keywords or phrases",
        ));
    }
    let regex_count = tokens
        .iter()
        .filter(|token| matches!(&token.kind, LqTokenKind::Regex(_)))
        .count();
    if regex_count > 4 {
        return Err(LqParseError::new(
            LqParseErrorCode::UnsupportedCombo,
            LqSpan::synthetic(0),
            "native content regex leaf limit is four",
        ));
    }
    Ok(NativeContentQueryTokens {
        input,
        tokens,
        regex_count: u64::try_from(regex_count).map_err(|_error| {
            LqParseError::new(
                LqParseErrorCode::LimitExceededBytes,
                LqSpan::synthetic(0),
                "native regex count overflow",
            )
        })?,
    })
}

#[cfg(test)]
#[path = "native_content_tests.rs"]
mod tests;

#[expect(
    clippy::disallowed_methods,
    reason = "This Option-returning admission helper deliberately maps conversion overflow to None"
)]
fn size_bytes(bytes: usize) -> Option<u64> {
    u64::try_from(bytes).ok()
}
