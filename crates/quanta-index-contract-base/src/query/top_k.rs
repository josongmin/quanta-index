//! One `top_k` contract for every query route.
//!
//! Before this module, the public result cap was validated in three places
//! with three answers: the semantic policy accepted `1..=10_000`, the hybrid
//! policy accepted the same range under a different error code, and the
//! dispatcher's continuation probe refused `10_000` outright because it needed
//! `top_k + 1` to fit under the same ceiling. Lexical, symbol, history and
//! runtime routes validated nothing, so `top_k = 0` produced an empty page on
//! some routes and one row on others (QI-BB-025).
//!
//! The public range and the internal fetch ceiling are now distinct numbers
//! owned here. The continuation probe fetches one row past the public cap so
//! the result window can report `has_more`, and that extra row is the
//! internal ceiling's business, not the caller's.

use core::fmt;

/// Smallest `top_k` a caller may request.
pub const PUBLIC_TOP_K_MIN: u32 = 1;
/// Largest `top_k` a caller may request, inclusive.
pub const PUBLIC_TOP_K_MAX: u32 = 10_000;
/// Largest number of rows the search plane will fetch internally for one
/// query: the public cap plus the single continuation row.
pub const INTERNAL_FETCH_CEILING: u32 = PUBLIC_TOP_K_MAX + 1;

/// Wire error code for a `top_k` outside the inclusive range from
/// [`PUBLIC_TOP_K_MIN`] to [`PUBLIC_TOP_K_MAX`].
///
/// Every route reports the same code for the same defect. It is declared here
/// so the SDK, the dispatcher and the domain policies cannot drift on it.
pub const TOP_K_OUT_OF_RANGE_CODE: &str = "QUERY_TOP_K_OUT_OF_RANGE";

/// A `top_k` the contract refuses.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TopKOutOfRangeV1 {
    pub requested: u32,
}

impl TopKOutOfRangeV1 {
    /// The stable wire code every route reports for this refusal.
    #[must_use]
    pub const fn code(self) -> &'static str {
        TOP_K_OUT_OF_RANGE_CODE
    }
}

impl fmt::Display for TopKOutOfRangeV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "top_k must be within {PUBLIC_TOP_K_MIN}..={PUBLIC_TOP_K_MAX}, got {}",
            self.requested
        )
    }
}

impl std::error::Error for TopKOutOfRangeV1 {}

/// Accept a caller's `top_k` or refuse it with the shared typed error.
pub const fn validate_public_top_k(top_k: u32) -> Result<u32, TopKOutOfRangeV1> {
    if top_k < PUBLIC_TOP_K_MIN || top_k > PUBLIC_TOP_K_MAX {
        return Err(TopKOutOfRangeV1 { requested: top_k });
    }
    Ok(top_k)
}

/// Wire/contract code for an internal fetch size outside the inclusive
/// range from `1` to [`INTERNAL_FETCH_CEILING`].
///
/// This is not a caller error. The dispatcher computes the fetch size from a
/// validated `top_k`; an adapter that receives a value past the ceiling has
/// been handed something the contract forbids, which is a defect in the
/// search plane, and it must refuse rather than fetch it.
pub const INTERNAL_FETCH_OUT_OF_RANGE_CODE: &str = "QUERY_INTERNAL_FETCH_OUT_OF_RANGE";

/// An internal fetch size an adapter refuses.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InternalFetchOutOfRangeV1 {
    pub requested: u32,
}

impl InternalFetchOutOfRangeV1 {
    #[must_use]
    pub const fn code(self) -> &'static str {
        INTERNAL_FETCH_OUT_OF_RANGE_CODE
    }
}

impl fmt::Display for InternalFetchOutOfRangeV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "internal fetch size must be within 1..={INTERNAL_FETCH_CEILING}, got {}",
            self.requested
        )
    }
}

impl std::error::Error for InternalFetchOutOfRangeV1 {}

/// Accept an adapter-boundary fetch size or refuse it.
///
/// Adapters sit below the public `top_k` gate: they receive the continuation
/// fetch size (public cap plus one) or a hybrid over-fetch, never the caller's
/// number. Validating them against the *public* cap refused the public
/// maximum one layer down from where it had just been accepted (QI-BB-025).
pub const fn validate_internal_fetch_size(fetch: u32) -> Result<u32, InternalFetchOutOfRangeV1> {
    if fetch < PUBLIC_TOP_K_MIN || fetch > INTERNAL_FETCH_CEILING {
        return Err(InternalFetchOutOfRangeV1 { requested: fetch });
    }
    Ok(fetch)
}

/// The number of rows to fetch so the result window can observe one
/// continuation row past an accepted `top_k`.
///
/// Infallible by construction: a validated `top_k` is at most
/// [`PUBLIC_TOP_K_MAX`], so the sum is at most [`INTERNAL_FETCH_CEILING`] and
/// cannot overflow. Callers must validate first; this function does not
/// re-check, so an unvalidated value would silently pass the cap.
#[must_use]
pub const fn continuation_fetch_size(validated_top_k: u32) -> u32 {
    validated_top_k.saturating_add(1)
}

#[cfg(test)]
mod tests {
    use super::{
        INTERNAL_FETCH_CEILING, PUBLIC_TOP_K_MAX, PUBLIC_TOP_K_MIN, TOP_K_OUT_OF_RANGE_CODE,
        TopKOutOfRangeV1, continuation_fetch_size, validate_public_top_k,
    };

    #[test]
    fn public_range_is_inclusive_on_both_ends() {
        assert_eq!(validate_public_top_k(PUBLIC_TOP_K_MIN), Ok(1));
        assert_eq!(validate_public_top_k(PUBLIC_TOP_K_MAX), Ok(10_000));
        assert_eq!(validate_public_top_k(9_999), Ok(9_999));
    }

    #[test]
    fn zero_and_above_max_are_refused_with_one_code() {
        for requested in [0, PUBLIC_TOP_K_MAX + 1, u32::MAX] {
            let refused = validate_public_top_k(requested);
            assert_eq!(refused, Err(TopKOutOfRangeV1 { requested }));
            assert_eq!(
                refused.map_err(TopKOutOfRangeV1::code),
                Err(TOP_K_OUT_OF_RANGE_CODE)
            );
        }
    }

    #[test]
    fn continuation_fetch_stays_under_the_internal_ceiling() {
        assert_eq!(
            continuation_fetch_size(PUBLIC_TOP_K_MAX),
            INTERNAL_FETCH_CEILING
        );
        assert_eq!(continuation_fetch_size(1), 2);
        assert!(INTERNAL_FETCH_CEILING > PUBLIC_TOP_K_MAX);
    }

    #[test]
    fn internal_fetch_accepts_the_continuation_row_and_refuses_beyond() {
        assert_eq!(
            super::validate_internal_fetch_size(INTERNAL_FETCH_CEILING),
            Ok(INTERNAL_FETCH_CEILING)
        );
        assert_eq!(
            super::validate_internal_fetch_size(continuation_fetch_size(PUBLIC_TOP_K_MAX)),
            Ok(INTERNAL_FETCH_CEILING)
        );
        for refused in [0, INTERNAL_FETCH_CEILING + 1, u32::MAX] {
            assert_eq!(
                super::validate_internal_fetch_size(refused)
                    .map_err(super::InternalFetchOutOfRangeV1::code),
                Err(super::INTERNAL_FETCH_OUT_OF_RANGE_CODE),
                "fetch={refused}"
            );
        }
    }

    #[test]
    fn display_names_the_range_and_the_value() {
        let text = TopKOutOfRangeV1 { requested: 0 }.to_string();
        assert!(text.contains("1..=10000"), "{text}");
        assert!(text.contains("got 0"), "{text}");
    }
}
