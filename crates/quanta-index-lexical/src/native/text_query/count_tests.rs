use super::*;

#[test]
fn bounded_count_caps_rows_without_changing_caller_ceiling() -> Result<(), IndexError> {
    for (top_k, count, expected) in [
        (8, None, 8),
        (8, Some(LqCountBound::Bounded(1)), 1),
        (8, Some(LqCountBound::Bounded(8)), 8),
        (8, Some(LqCountBound::Bounded(10)), 8),
        (1, Some(LqCountBound::Bounded(10)), 1),
    ] {
        let actual = result_capacity(top_k, count)?;
        if actual != expected {
            return Err(IndexError::BackendViolation(format!(
                "bounded count row cap: top_k={top_k}, count={count:?}, got={actual}, expected={expected}"
            )));
        }
    }
    Ok(())
}

#[test]
fn unsupported_unbounded_and_zero_count_refuse_before_posting_work() {
    for count in [Some(LqCountBound::All), Some(LqCountBound::Bounded(0))] {
        assert!(matches!(
            result_capacity(8, count),
            Err(IndexError::InvalidInput(_))
        ));
    }
}
