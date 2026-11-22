#![forbid(unsafe_code)]

// Exact historical crash oracles against a pinned, debug-capable old-Q binary.
#[path = "e2e_crash_matrix.rs"]
mod e2e_crash_matrix;
#[path = "e2e_hard_scan_checkpoint_restart.rs"]
mod e2e_hard_scan_checkpoint_restart;
#[path = "common/searchd_binary_process.rs"]
mod searchd_binary_process;
