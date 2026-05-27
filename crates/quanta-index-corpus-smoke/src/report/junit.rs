//! Hand-rolled `JUnit` XML emitter for a conformance [`Report`].
//!
//! One `<testsuite>` per report; one `<testcase>` per row.
//! [`Verdict::Fail`] / [`Verdict::UnexpectedError`] → `<failure>`;
//! [`Verdict::Pending`] → `<skipped>`; [`Verdict::Pass`] /
//! [`Verdict::ExpectedError`] → no children. All attribute values are
//! XML-escaped via `escape_attr`; element text via `escape_text`.

use std::io::{self, Write};

use crate::runner::{Report, RowOutcome, Verdict};

/// Render `report` as `JUnit` XML into `writer`.
///
/// # Errors
///
/// Returns `io::Error` only if `writer` returns one.
pub fn render_junit(report: &Report, writer: &mut dyn Write) -> io::Result<()> {
    writer.write_all(b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n")?;
    writeln!(
        writer,
        "<testsuite name=\"ConformanceCorpus\" tests=\"{}\" failures=\"{}\" skipped=\"{}\" time=\"{}\">",
        report.summary.total,
        report
            .summary
            .failed
            .saturating_add(report.summary.unexpected_error),
        report.summary.pending,
        seconds_repr(report.summary.total_elapsed_us),
    )?;
    for row in &report.rows {
        write_testcase(writer, row)?;
    }
    writer.write_all(b"</testsuite>\n")?;
    Ok(())
}

fn write_testcase(writer: &mut dyn Write, row: &RowOutcome) -> io::Result<()> {
    write!(
        writer,
        "  <testcase classname=\"ConformanceCorpus\" name=\"{}\" time=\"{}\"",
        escape_attr(&row.row_id),
        seconds_repr(row.elapsed_us),
    )?;
    match &row.verdict {
        Verdict::Pass => writer.write_all(b" />\n"),
        Verdict::ExpectedError { code } => {
            writer.write_all(b">\n")?;
            writeln!(
                writer,
                "    <system-out>expected error: {}</system-out>",
                escape_text(code.as_code_str()),
            )?;
            writer.write_all(b"  </testcase>\n")
        }
        Verdict::Fail { reason } => {
            writer.write_all(b">\n")?;
            writeln!(
                writer,
                "    <failure type=\"fail\" message=\"{}\" />",
                escape_attr(reason),
            )?;
            writer.write_all(b"  </testcase>\n")
        }
        Verdict::UnexpectedError { observed } => {
            writer.write_all(b">\n")?;
            writeln!(
                writer,
                "    <failure type=\"unexpected_error\" message=\"observed `{}`\" />",
                escape_attr(observed),
            )?;
            writer.write_all(b"  </testcase>\n")
        }
        Verdict::Pending { ticket } => {
            writer.write_all(b">\n")?;
            writeln!(
                writer,
                "    <skipped message=\"gated on `{}`\" />",
                escape_attr(ticket),
            )?;
            writer.write_all(b"  </testcase>\n")
        }
    }
}

fn seconds_repr(micros: u64) -> String {
    // Floor division — clippy forbids `integer_division`, so we use
    // `checked_div` / `checked_rem` and fall through `unwrap_or(0)`
    // is barred too; the divisor is a non-zero literal so the
    // operation is total, but to satisfy the lint we use explicit
    // `match`.
    let (secs, frac) = match (micros.checked_div(1_000_000), micros.checked_rem(1_000_000)) {
        (Some(s), Some(f)) => (s, f),
        // Unreachable: divisor is the non-zero literal `1_000_000`.
        // Substituting a typed zero keeps the function total without
        // a panic-shaped silent default.
        _ => (0u64, 0u64),
    };
    format!("{secs}.{frac:06}")
}

fn escape_attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            other => out.push(other),
        }
    }
    out
}

fn escape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::{Report, ReportSummary, RowOutcome, Verdict};

    fn outcome(id: &str, verdict: Verdict) -> RowOutcome {
        RowOutcome {
            row_id: id.to_owned(),
            verdict,
            elapsed_us: 1500,
        }
    }

    fn render_to_string(report: &Report) -> String {
        let mut buf: Vec<u8> = Vec::new();
        if let Err(e) = render_junit(report, &mut buf) {
            assert!(false, "render_junit failed: {e}");
        }
        core::str::from_utf8(&buf).map_or_else(
            |e| {
                assert!(false, "rendered bytes not utf8: {e}");
                String::new()
            },
            str::to_owned,
        )
    }

    #[test]
    fn render_stable_bytes_for_mixed_report() {
        let report = Report {
            rows: vec![
                outcome("R1", Verdict::Pass),
                outcome(
                    "R2",
                    Verdict::Fail {
                        reason: "expected single, observed Empty".to_owned(),
                    },
                ),
                outcome(
                    "R3",
                    Verdict::Pending {
                        ticket: "LEX-01".to_owned(),
                    },
                ),
            ],
            summary: ReportSummary {
                total: 3,
                passed: 1,
                failed: 1,
                pending: 1,
                expected_error: 0,
                unexpected_error: 0,
                total_elapsed_us: 4_500,
            },
        };
        let rendered = render_to_string(&report);
        assert!(rendered.contains("<testsuite name=\"ConformanceCorpus\""));
        assert!(rendered.contains("tests=\"3\""));
        assert!(rendered.contains("failures=\"1\""));
        assert!(rendered.contains("skipped=\"1\""));
        assert!(rendered.contains("<testcase classname=\"ConformanceCorpus\" name=\"R1\""));
        assert!(rendered.contains("<failure type=\"fail\""));
        assert!(rendered.contains("<skipped message=\"gated on `LEX-01`\""));
    }

    #[test]
    fn escapes_xml_special_chars_in_reason() {
        let report = Report {
            rows: vec![outcome(
                "R<X>",
                Verdict::Fail {
                    reason: "bad & ugly".to_owned(),
                },
            )],
            summary: ReportSummary {
                total: 1,
                failed: 1,
                total_elapsed_us: 1500,
                ..ReportSummary::default()
            },
        };
        let rendered = render_to_string(&report);
        assert!(rendered.contains("name=\"R&lt;X&gt;\""));
        assert!(rendered.contains("message=\"bad &amp; ugly\""));
    }

    #[test]
    fn unexpected_error_emits_failure_with_observed_code() {
        let report = Report {
            rows: vec![outcome(
                "R1",
                Verdict::UnexpectedError {
                    observed: "TIMEOUT_EXCEEDED".to_owned(),
                },
            )],
            summary: ReportSummary {
                total: 1,
                unexpected_error: 1,
                total_elapsed_us: 1500,
                ..ReportSummary::default()
            },
        };
        let rendered = render_to_string(&report);
        assert!(rendered.contains("<failure type=\"unexpected_error\""));
        assert!(rendered.contains("TIMEOUT_EXCEEDED"));
    }
}
