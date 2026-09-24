#![forbid(unsafe_code)]

#[path = "e2e_process_readiness.rs"]
mod e2e_process_readiness;
#[path = "common/fail_closed_wait.rs"]
mod fail_closed_wait;
#[path = "common/searchd_binary_process.rs"]
mod searchd_binary_process;
