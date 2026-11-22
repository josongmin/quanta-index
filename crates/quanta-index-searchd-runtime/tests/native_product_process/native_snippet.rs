//! Product daemon snippet quality through the Native SDK and cold reopen.

use super::*;

#[test]
fn product_native_snippet_report_passes_four_fixed_queries_after_cold_reopen() -> TestResult {
    let binary = Path::new(env!("CARGO_BIN_EXE_quanta-index-searchd"));
    let report = quanta_index_searchd_harness::snippet::run_native_snippet_report(binary)?;
    let expected = [
        ("snip.phrase.brown_fox", "src/snippet/phrase.rs"),
        ("snip.regex.target_symbol", "src/snippet/regex.rs"),
        ("snip.multi.repeated_marker", "src/snippet/multi.rs"),
        ("snip.longline.compute_target", "src/snippet/longline.rs"),
    ];
    if report.scores.len() != expected.len() || report.native_provenance.is_none() {
        return Err(format!(
            "Native snippet product report omitted fixed rows or provenance: {report:#?}"
        )
        .into());
    }
    for (score, (id, path)) in report.scores.iter().zip(expected) {
        if score.id != id || score.matched_path.as_deref() != Some(path) || !score.passed() {
            return Err(format!(
                "Native snippet product row {id} failed fixed path/quality gold: {score:#?}"
            )
            .into());
        }
    }
    if !report.passed {
        return Err(format!("Native snippet product gate failed: {report:#?}").into());
    }
    Ok(())
}
