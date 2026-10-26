//! Fixed two-row sorter fixtures test the contract boundary, not a sort engine.
use super::*;
use std::{cell::Cell, cmp::Ordering};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[derive(Clone, Copy)]
enum Action {
    OrderPair,
    Keep,
    Duplicate,
    OutOfRange,
}
struct PairDriver {
    action: Action,
    refuse: bool,
    comparisons: Cell<usize>,
}
impl PairDriver {
    fn sort_indices_v3(
        &self,
        values: &mut [usize],
        compare: &mut dyn FnMut(
            usize,
            usize,
        )
            -> Result<Ordering, SourceCoverageProgressError<&'static str>>,
    ) -> Result<(), SourceCoverageProgressError<&'static str>> {
        let [left, right] = values else {
            return Ok(());
        };
        match self.action {
            Action::OrderPair => {
                if compare(*left, *right)? == Ordering::Greater {
                    std::mem::swap(left, right);
                }
            }
            Action::Keep => {}
            Action::Duplicate => *right = *left,
            Action::OutOfRange => *left = usize::MAX,
        }
        Ok(())
    }
    fn admit_comparison_v3(&self, _left: &str, _right: &str) -> Result<(), &'static str> {
        let next = self
            .comparisons
            .get()
            .checked_add(1)
            .ok_or("comparison counter overflow")?;
        self.comparisons.set(next);
        if self.refuse {
            Err("comparison refused")
        } else {
            Ok(())
        }
    }
}
fn driver(action: Action, refuse: bool) -> PairDriver {
    PairDriver {
        action,
        refuse,
        comparisons: Cell::new(0),
    }
}
fn digest(
    records: &[ChunkRecord],
    order: &mut [usize],
    sort: &PairDriver,
) -> Result<[u8; 32], SourceCoverageProgressError<&'static str>> {
    source_file_unit_set_sha256_with_borrowed_scratch_and_sort_v3(
        records,
        &[],
        order,
        &mut [],
        &mut || Ok(()),
        &mut |values, compare| sort.sort_indices_v3(values, compare),
        &mut |left, right| sort.admit_comparison_v3(left, right),
    )
}
#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "fixed contract gold and typed refusal assertions are independent test oracles"
)]
fn fallible_sort_preserves_independently_ordered_wire_and_duplicate_predicate() -> TestResult {
    let first = tests::chunk("a", "alpha")?;
    let second = tests::chunk("b", "beta")?;
    let sorter = driver(Action::OrderPair, false);
    let actual = digest(&[second.clone(), first.clone()], &mut [0; 2], &sorter);
    let expected = hash_canonical_cbor_with_progress(
        b"quanta-index:source-file-unit-set:v1\0",
        &(vec![&first, &second], Vec::<&SymbolRecord>::new()),
        &mut || Ok::<_, std::convert::Infallible>(()),
    );
    match (actual, expected) {
        (Ok(actual), Ok(expected)) => assert_eq!(actual, expected),
        (actual, expected) => {
            return Err(format!("wire comparison refused: {actual:?} {expected:?}").into());
        }
    }
    assert!(sorter.comparisons.get() > 0);
    let duplicate = tests::chunk("a", "beta")?;
    assert!(matches!(
        digest(&[first, duplicate], &mut [0; 2], &sorter),
        Err(SourceCoverageProgressError::Invalid(
            SourceCoverageError::DuplicateCandidateIdWithoutPayload
        ))
    ));
    Ok(())
}
#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "fixed contract gold and typed refusal assertions are independent test oracles"
)]
fn wrong_sorter_order_duplicate_and_foreign_index_cannot_produce_digest() -> TestResult {
    let unordered = [tests::chunk("b", "beta")?, tests::chunk("a", "alpha")?];
    for action in [Action::Keep, Action::Duplicate, Action::OutOfRange] {
        assert!(matches!(
            digest(&unordered, &mut [0; 2], &driver(action, false)),
            Err(SourceCoverageProgressError::Invalid(
                SourceCoverageError::InvalidSource(_)
            ))
        ));
    }
    Ok(())
}
#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "fixed contract gold and typed refusal assertions are independent test oracles"
)]
fn comparison_and_certification_refusal_return_original_typed_error() -> TestResult {
    let first = tests::chunk("a", "alpha")?;
    let second = tests::chunk("b", "beta")?;
    let sort_refusal = driver(Action::OrderPair, true);
    assert!(matches!(
        digest(&[second.clone(), first.clone()], &mut [0; 2], &sort_refusal),
        Err(SourceCoverageProgressError::Progress("comparison refused"))
    ));
    let certification_refusal = driver(Action::Keep, true);
    assert!(matches!(
        digest(&[first, second], &mut [0; 2], &certification_refusal),
        Err(SourceCoverageProgressError::Progress("comparison refused"))
    ));
    Ok(())
}
