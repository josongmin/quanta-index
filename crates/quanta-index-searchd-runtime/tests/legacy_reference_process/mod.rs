//! Maintained public Legacy SDK/reference oracles, without a current engine.

mod events;
mod idempotency;
#[path = "../native_product_process/legacy_reference_readiness.rs"]
mod no_active;
mod physical;
mod restart;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn require_equal<T: std::fmt::Debug + PartialEq>(
    actual: &T,
    expected: &T,
    label: &str,
) -> TestResult {
    if actual == expected {
        Ok(())
    } else {
        Err(format!("{label}: expected {expected:?}, got {actual:?}").into())
    }
}
