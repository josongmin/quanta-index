#![forbid(unsafe_code)]

#[path = "e2e_process_readiness.rs"]
mod e2e_process_readiness;
#[expect(
    dead_code,
    reason = "the shared bounded-wait helper also serves broader runtime suites"
)]
#[path = "common/fail_closed_wait.rs"]
mod fail_closed_wait;
