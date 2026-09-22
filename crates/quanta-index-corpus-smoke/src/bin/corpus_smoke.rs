//! `corpus-smoke` CLI shim — spec sheet §11.
//!
//! Usage:
//!
//! ```text
//! cargo run -p quanta-index-corpus-smoke --bin corpus-smoke -- \
//!     --corpus PATH --output PATH
//! ```
//!
//! Exit codes:
//! - `0` when the loaded corpus runs and `report.summary.failed == 0`
//! - `1` on any other terminal condition (load failure, IO failure,
//!   render failure, or `report.summary.failed > 0`)
//!
//! The wiring uses [`MockNormalizer`] + [`MockExecutor`]; the real
//! normalizer arrives via PRE-NORM (separately), at which point this
//! bin gets a constructor swap, not a structural rewrite.
//!
//! Surface guarantees:
//! - hand-rolled argv parser, no `clap` dep
//! - no `panic!` / `unwrap` / `expect` / `todo!` / `unimplemented!`
//! - no `process::exit`; main returns [`ExitCode`]
//! - one-line summary to stderr via `writeln!(io::stderr().lock(), …)`
//!   so the `print_stderr` clippy lint stays clean

#![forbid(unsafe_code)]

use std::env;
use std::ffi::OsString;
use std::fmt;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use quanta_index_corpus_smoke::{
    Report, load_corpus,
    mocks::{MockExecutor, MockNormalizer},
    render_junit, run_corpus,
};

/// Exit code returned when the corpus loads, executes, and reports zero
/// failures. Mirrors the spec-sheet §11 contract.
const EXIT_OK: u8 = 0;
/// Exit code returned for any terminal failure path — argv parse
/// error, corpus load error, output write error, or
/// `report.summary.failed > 0`.
const EXIT_FAIL: u8 = 1;

fn main() -> ExitCode {
    let args: Vec<OsString> = env::args_os().collect();
    match run(&args) {
        Ok(code) => ExitCode::from(code),
        Err(err) => {
            // Best-effort diagnostic. If stderr itself is broken there
            // is nothing useful to do beyond surfacing the non-zero
            // exit code.
            let _stderr_result: io::Result<()> =
                writeln!(io::stderr().lock(), "corpus-smoke: {err}");
            ExitCode::from(EXIT_FAIL)
        }
    }
}

/// Parsed CLI arguments.
#[derive(Clone, Debug, Eq, PartialEq)]
struct CliArgs {
    /// Path to the input TOML corpus.
    corpus: PathBuf,
    /// Path to write the `JUnit` XML report to.
    output: PathBuf,
}

/// Closed-set CLI failure mode.
///
/// Distinct from [`quanta_index_corpus_smoke::CorpusLoadError`] /
/// [`quanta_index_corpus_smoke::ConformanceError`] because those describe
/// the *content* of the corpus run; `CliError` describes the *driver*
/// surface (argv shape, IO around the report file).
#[derive(Debug)]
enum CliError {
    /// `--corpus` or `--output` missing.
    MissingRequiredFlag { flag: &'static str },
    /// A flag was supplied without its value.
    MissingValue { flag: String },
    /// Unknown argv token.
    UnknownArgument { arg: String },
    /// A flag-value was not valid UTF-8 when treated as a string. We
    /// keep `OsString` around so the path round-trips losslessly into
    /// `PathBuf`, but the diagnostic still wants a renderable name.
    NonUtf8Value { flag: &'static str },
    /// `--corpus` / `--output` was supplied more than once.
    DuplicateFlag { flag: &'static str },
    /// Corpus loader returned a typed error.
    Corpus { message: String },
    /// IO failure while creating or writing the output file.
    Io { path: String, message: String },
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingRequiredFlag { flag } => {
                write!(f, "missing required flag `{flag}`")
            }
            Self::MissingValue { flag } => {
                write!(f, "flag `{flag}` requires a value")
            }
            Self::UnknownArgument { arg } => {
                write!(f, "unknown argument `{arg}`")
            }
            Self::NonUtf8Value { flag } => {
                write!(f, "flag `{flag}` value is not valid UTF-8")
            }
            Self::DuplicateFlag { flag } => {
                write!(f, "flag `{flag}` supplied more than once")
            }
            Self::Corpus { message } => write!(f, "corpus load failed: {message}"),
            Self::Io { path, message } => write!(f, "io error on `{path}`: {message}"),
        }
    }
}

impl core::error::Error for CliError {}

/// Top-level driver split out from `main`.
///
/// Keeps the body testable and gives access to `?`. Returns the exit
/// code as `u8` and reserves `Err(_)` for terminal driver failures
/// (which map to [`EXIT_FAIL`]).
fn run(args: &[OsString]) -> Result<u8, CliError> {
    let cli = parse_args(args)?;

    let corpus = load_corpus(&cli.corpus).map_err(|e| CliError::Corpus {
        message: e.to_string(),
    })?;

    let normalizer = MockNormalizer::default();
    let executor = MockExecutor::new();
    let report = run_corpus(&corpus, &normalizer, &executor);

    write_report(&cli.output, &report)?;
    write_summary(&report)?;

    if report.summary.failed == 0 {
        Ok(EXIT_OK)
    } else {
        Ok(EXIT_FAIL)
    }
}

/// Hand-rolled argv parser.
///
/// Accepts exactly:
/// - `--corpus PATH` (required, once)
/// - `--output PATH` (required, once)
///
/// `--flag=VALUE` form is *not* accepted; the spec uses
/// space-separated values. Any other token is rejected.
fn parse_args(args: &[OsString]) -> Result<CliArgs, CliError> {
    let mut corpus: Option<PathBuf> = None;
    let mut output: Option<PathBuf> = None;

    // Skip argv[0] (program name). `args` may be empty when invoked
    // through unusual harnesses, so guard the slice access.
    let tail = args.get(1..).unwrap_or(&[]);
    let mut iter = tail.iter();
    while let Some(raw) = iter.next() {
        let Some(token) = raw.to_str() else {
            return Err(CliError::UnknownArgument {
                arg: raw.to_string_lossy().into_owned(),
            });
        };
        match token {
            "--corpus" => {
                let value = take_value(&mut iter, "--corpus")?;
                if corpus.is_some() {
                    return Err(CliError::DuplicateFlag { flag: "--corpus" });
                }
                corpus = Some(value);
            }
            "--output" => {
                let value = take_value(&mut iter, "--output")?;
                if output.is_some() {
                    return Err(CliError::DuplicateFlag { flag: "--output" });
                }
                output = Some(value);
            }
            other => {
                return Err(CliError::UnknownArgument {
                    arg: other.to_owned(),
                });
            }
        }
    }

    let corpus = corpus.ok_or(CliError::MissingRequiredFlag { flag: "--corpus" })?;
    let output = output.ok_or(CliError::MissingRequiredFlag { flag: "--output" })?;
    Ok(CliArgs { corpus, output })
}

/// Pull the next argv token as a `PathBuf`.
///
/// Maps absent / non-UTF-8 cases onto typed [`CliError`] variants.
/// Paths must be UTF-8 here because we surface them in error strings;
/// treating the value as `OsString` first and only stringifying on the
/// error path preserves the original bytes for the actual filesystem
/// call.
fn take_value(
    iter: &mut core::slice::Iter<'_, OsString>,
    flag: &'static str,
) -> Result<PathBuf, CliError> {
    let raw = iter.next().ok_or_else(|| CliError::MissingValue {
        flag: flag.to_owned(),
    })?;
    if raw.to_str().is_none() {
        return Err(CliError::NonUtf8Value { flag });
    }
    Ok(PathBuf::from(raw))
}

fn write_report(path: &PathBuf, report: &Report) -> Result<(), CliError> {
    let file = File::create(path).map_err(|e| CliError::Io {
        path: path.display().to_string(),
        message: e.to_string(),
    })?;
    let mut writer = BufWriter::new(file);
    render_junit(report, &mut writer).map_err(|e| CliError::Io {
        path: path.display().to_string(),
        message: e.to_string(),
    })?;
    writer.flush().map_err(|e| CliError::Io {
        path: path.display().to_string(),
        message: e.to_string(),
    })?;
    Ok(())
}

/// Emit a single deterministic summary line to stderr.
///
/// Shape is keyed `KEY=VALUE` so downstream grep / CI parsers stay
/// stable across releases. Locked once so the line is written
/// atomically.
fn write_summary(report: &Report) -> Result<(), CliError> {
    let stderr = io::stderr();
    let mut handle = stderr.lock();
    let s = &report.summary;
    writeln!(
        handle,
        "corpus-smoke: total={} passed={} failed={} pending={} expected_error={} unexpected_error={} elapsed_us={}",
        s.total,
        s.passed,
        s.failed,
        s.pending,
        s.expected_error,
        s.unexpected_error,
        s.total_elapsed_us,
    )
    .map_err(|e| CliError::Io {
        path: "<stderr>".to_owned(),
        message: e.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<OsString> {
        let mut v: Vec<OsString> = Vec::with_capacity(values.len().saturating_add(1));
        v.push(OsString::from("corpus-smoke"));
        for s in values {
            v.push(OsString::from(*s));
        }
        v
    }

    #[test]
    fn parse_args_accepts_both_flags() {
        let argv = args(&["--corpus", "/tmp/c.toml", "--output", "/tmp/o.xml"]);
        let Ok(parsed) = parse_args(&argv) else {
            assert!(false, "expected Ok");
            return;
        };
        assert_eq!(parsed.corpus, PathBuf::from("/tmp/c.toml"));
        assert_eq!(parsed.output, PathBuf::from("/tmp/o.xml"));
    }

    #[test]
    fn parse_args_rejects_missing_corpus() {
        let argv = args(&["--output", "/tmp/o.xml"]);
        let Err(err) = parse_args(&argv) else {
            assert!(false, "expected Err");
            return;
        };
        assert!(matches!(
            err,
            CliError::MissingRequiredFlag { flag: "--corpus" }
        ));
    }

    #[test]
    fn parse_args_rejects_missing_output() {
        let argv = args(&["--corpus", "/tmp/c.toml"]);
        let Err(err) = parse_args(&argv) else {
            assert!(false, "expected Err");
            return;
        };
        assert!(matches!(
            err,
            CliError::MissingRequiredFlag { flag: "--output" }
        ));
    }

    #[test]
    fn parse_args_rejects_missing_value() {
        let argv = args(&["--corpus"]);
        let Err(err) = parse_args(&argv) else {
            assert!(false, "expected Err");
            return;
        };
        assert!(matches!(err, CliError::MissingValue { flag } if flag == "--corpus"));
    }

    #[test]
    fn parse_args_rejects_unknown_flag() {
        let argv = args(&["--corpus", "c", "--output", "o", "--mystery"]);
        let Err(err) = parse_args(&argv) else {
            assert!(false, "expected Err");
            return;
        };
        assert!(matches!(err, CliError::UnknownArgument { arg } if arg == "--mystery"));
    }

    #[test]
    fn parse_args_rejects_duplicate_corpus() {
        let argv = args(&["--corpus", "a", "--corpus", "b", "--output", "o"]);
        let Err(err) = parse_args(&argv) else {
            assert!(false, "expected Err");
            return;
        };
        assert!(matches!(err, CliError::DuplicateFlag { flag: "--corpus" }));
    }

    #[test]
    fn parse_args_rejects_eq_form() {
        // Spec form is space-separated; `--corpus=PATH` is intentionally
        // rejected to keep the surface narrow.
        let argv = args(&["--corpus=foo", "--output", "o"]);
        let Err(err) = parse_args(&argv) else {
            assert!(false, "expected Err");
            return;
        };
        assert!(matches!(err, CliError::UnknownArgument { arg } if arg == "--corpus=foo"));
    }
}
