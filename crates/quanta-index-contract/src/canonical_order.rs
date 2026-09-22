//! Shared canonical-order check for sealed, duplicate-free contract sequences.
//!
//! Several wire surfaces (cluster membership members, batch read items, scoped
//! membership replacements, ...) carry sequences that the contract requires to
//! be strictly ascending. Each of them previously open-coded the same
//! adjacent-pair scan with slice indexing, which is both a mirrored
//! implementation and a panic-capable decode path on malformed input. One
//! helper owns the invariant instead.
//!
//! The helper is public because the invariant is a contract obligation that
//! producer-facing and search-plane crates both have to enforce; a second copy
//! in an adapter is the thing this module exists to prevent.

use core::cmp::Ordering;

/// The first way a sequence departs from strictly ascending order.
///
/// Callers map this onto their own typed wire error so each surface keeps its
/// own vocabulary while sharing one implementation of the check.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CanonicalOrderBreakV1 {
    /// Two adjacent entries compared equal.
    Duplicate,
    /// An entry sorted strictly after its successor.
    OutOfOrder,
}

/// Reports the first canonical-order break in `values`, comparing by `key`.
///
/// Returns `None` when `values` is strictly ascending under `key`, which also
/// proves it is duplicate-free. Adjacent pairs are walked with iterators rather
/// than index arithmetic so no input length can panic the decoder.
pub fn first_canonical_order_break_v1<T, K, F>(
    values: &[T],
    key: F,
) -> Option<CanonicalOrderBreakV1>
where
    K: Ord + ?Sized,
    F: Fn(&T) -> &K,
{
    for (left, right) in values.iter().zip(values.iter().skip(1)) {
        match key(left).cmp(key(right)) {
            Ordering::Equal => return Some(CanonicalOrderBreakV1::Duplicate),
            Ordering::Greater => return Some(CanonicalOrderBreakV1::OutOfOrder),
            Ordering::Less => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{CanonicalOrderBreakV1, first_canonical_order_break_v1};

    fn identity(value: &u8) -> &u8 {
        value
    }

    #[test]
    fn empty_and_single_sequences_are_canonical_v1() {
        let empty: [u8; 0] = [];
        assert_eq!(first_canonical_order_break_v1(&empty, identity), None);
        assert_eq!(first_canonical_order_break_v1(&[7_u8], identity), None);
    }

    #[test]
    fn ascending_sequence_is_canonical_v1() {
        assert_eq!(
            first_canonical_order_break_v1(&[1_u8, 2, 3, 250], identity),
            None
        );
    }

    #[test]
    fn adjacent_equal_entries_report_duplicate_v1() {
        assert_eq!(
            first_canonical_order_break_v1(&[1_u8, 2, 2, 3], identity),
            Some(CanonicalOrderBreakV1::Duplicate)
        );
    }

    #[test]
    fn descending_step_reports_out_of_order_v1() {
        assert_eq!(
            first_canonical_order_break_v1(&[1_u8, 3, 2], identity),
            Some(CanonicalOrderBreakV1::OutOfOrder)
        );
    }

    #[test]
    fn duplicate_is_reported_before_a_later_order_break_v1() {
        assert_eq!(
            first_canonical_order_break_v1(&[1_u8, 1, 9, 4], identity),
            Some(CanonicalOrderBreakV1::Duplicate)
        );
    }

    #[test]
    fn projection_key_drives_the_comparison_v1() {
        let rows: [(u8, &str); 3] = [(1, "z"), (1, "y"), (2, "x")];
        assert_eq!(
            first_canonical_order_break_v1(&rows, |row| &row.0),
            Some(CanonicalOrderBreakV1::Duplicate)
        );
        assert_eq!(
            first_canonical_order_break_v1(&rows, |row| row.1),
            Some(CanonicalOrderBreakV1::OutOfOrder)
        );
    }
}
