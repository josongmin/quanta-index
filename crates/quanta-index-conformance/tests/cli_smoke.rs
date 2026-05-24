//! Smoke test for the `conformance` bin.
//!
//! Spawns the freshly built binary via `std::process::Command`, points
//! it at a temp TOML corpus + temp output path, and asserts:
//!
//! 1. exit code 0 when the corpus contains only `pending` / `blocked`
//!    rows (so `summary.failed == 0` against the mock executor)
//! 2. the `--output` file was created and starts with the expected
//!    `<?xml` `JUnit` preamble
//!
//! Why only gated rows? The v1 wiring uses `MockExecutor::new()` which
//! returns `NOT_IMPLEMENTED` for any active row — that path produces
//! `UnexpectedError`, which keeps `summary.failed == 0` but pushes
//! `summary.unexpected_error > 0`. The spec-sheet §11 contract is keyed
//! specifically on `summary.failed`, so a gated-only corpus is the
//! cleanest exit-0 fixture. A separate test exercises the exit-1 path.

#![forbid(unsafe_code)]

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use tempfile::TempDir;

/// Path to the built `conformance` bin as injected by Cargo.
const BIN_PATH: &str = env!("CARGO_BIN_EXE_conformance");

/// Create a temp dir or abort the test loudly.
///
/// `tempfile::TempDir::new` returns `io::Result<TempDir>`; we
/// destructure and abort via the workspace `assert!(false, …);
/// std::process::abort()` convention so we don't reach for
/// `unwrap` / `expect`.
fn make_tempdir() -> TempDir {
    match TempDir::new() {
        Ok(t) => t,
        Err(e) => {
            assert!(false, "tempdir create failed: {e}");
            std::process::abort();
        }
    }
}

fn write_file(path: &PathBuf, contents: &str) {
    if let Err(e) = fs::write(path, contents) {
        let p = path.display();
        assert!(false, "write {p} failed: {e}");
    }
}

fn read_file(path: &PathBuf) -> String {
    match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            let p = path.display();
            assert!(false, "read {p} failed: {e}");
            String::new()
        }
    }
}

/// All-gated corpus: every row short-circuits on `Pending` / `Blocked`,
/// no executor traffic, `summary.failed == 0`.
const GATED_CORPUS: &str = r#"
[[row]]
id = "G1"
query = "fooBar"
gate = "pending"
gating_ticket = "LEX-01"
[row.expected]
kind = "single"

[[row]]
id = "G2"
query = "bazQux"
gate = "blocked"
gating_ticket = "RT-02"
[row.expected]
kind = "single"
"#;

/// Active row pointed at the mock executor with no canned response.
///
/// `MockExecutor::new()` returns `NOT_IMPLEMENTED` for any active row;
/// to force a true `Verdict::Fail` we declare `expected = error` with
/// `PARSE_ERROR` and let the executor's `NOT_IMPLEMENTED` mismatch the
/// declared code. This is the only deterministic way to drive
/// `summary.failed > 0` against the v1 mock wiring — every other path
/// would either pass or fall into `UnexpectedError` (which doesn't bump
/// `summary.failed`).
const FAILING_CORPUS: &str = r#"
[[row]]
id = "F1"
query = "no_response"
gate = "active"
[row.expected]
kind = "error"
code = "PARSE_ERROR"
"#;

#[test]
fn bin_exits_zero_on_gated_corpus() {
    let dir = make_tempdir();
    let corpus_path = dir.path().join("corpus.toml");
    let output_path = dir.path().join("report.xml");
    write_file(&corpus_path, GATED_CORPUS);

    let status = match Command::new(BIN_PATH)
        .arg("--corpus")
        .arg(&corpus_path)
        .arg("--output")
        .arg(&output_path)
        .status()
    {
        Ok(s) => s,
        Err(e) => {
            assert!(false, "spawn failed: {e}");
            return;
        }
    };

    assert!(status.success(), "expected exit 0, got status {status:?}",);
    assert_eq!(status.code(), Some(0));

    let rendered = read_file(&output_path);
    assert!(
        rendered.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"),
        "report missing xml preamble: {rendered}",
    );
    assert!(rendered.contains("tests=\"2\""));
    assert!(rendered.contains("skipped=\"2\""));
    assert!(rendered.contains("failures=\"0\""));
}

#[test]
fn bin_exits_one_on_failing_corpus() {
    let dir = make_tempdir();
    let corpus_path = dir.path().join("corpus.toml");
    let output_path = dir.path().join("report.xml");
    write_file(&corpus_path, FAILING_CORPUS);

    let status = match Command::new(BIN_PATH)
        .arg("--corpus")
        .arg(&corpus_path)
        .arg("--output")
        .arg(&output_path)
        .status()
    {
        Ok(s) => s,
        Err(e) => {
            assert!(false, "spawn failed: {e}");
            return;
        }
    };

    assert_eq!(status.code(), Some(1));

    // The report must still have been written before the non-zero
    // exit — junit consumers rely on the artifact existing.
    let rendered = read_file(&output_path);
    assert!(rendered.contains("<testcase"));
    assert!(rendered.contains("F1"));
    assert!(rendered.contains("failures=\"1\""));
}

#[test]
fn bin_exits_one_on_missing_corpus_flag() {
    let dir = make_tempdir();
    let output_path = dir.path().join("report.xml");

    let status = match Command::new(BIN_PATH)
        .arg("--output")
        .arg(&output_path)
        .status()
    {
        Ok(s) => s,
        Err(e) => {
            assert!(false, "spawn failed: {e}");
            return;
        }
    };

    assert_eq!(status.code(), Some(1));
}
