//! Ambiguity / repairability rail (J7Q-06).
//!
//! Snapshots the typed repair payloads the search-plane wire boundary emits for
//! every bridge error code, and enforces the *repairability invariants* the
//! ticket fixes — without re-deriving the mapping (that is
//! [`quanta_index_search_plane::repair_for_code`], already unit-tested at the
//! producer). The rail proves structural properties the producer alone does not:
//!
//! - every repairable code carries non-empty supported alternatives + a docs
//!   anchor (a hint that points nowhere is not a repair);
//! - the internal invariant-break code carries NO repair (no misleading hint);
//! - the repair classes are not collapsed into one generic bucket — ambiguous,
//!   unsupported, and malformed stay distinct families;
//! - each emitted payload round-trips through the wire codec (fail-closed-safe).
//!
//! Fail-closed posture is unchanged: repair is advisory metadata layered on a
//! still-failing `code`/`message`; nothing here rewrites a query.

use std::path::Path;

use anyhow::Result as AnyResult;
use quanta_index_contract::lex::LexicalErrorCode;
use quanta_index_contract::{
    QueryErrorRepair, RepairClass, SearchPlaneErrorCodeV2, SearchPlaneIpcError,
};
use quanta_index_search_plane::repair_for_code;
use serde_json::{Value, json};

use crate::artifact::{
    BenchArtifactV1, BenchMode, BenchProvenanceV1, BenchRowV1, BenchSyntax, GitHeadV1, HostV1,
    PhaseDurationsV1, ResourceUsageV1, ResultShape, RouteFamily, config_digest, corpus_digest,
    saturating_u64,
};

/// The bridge error codes the rail audits, paired with whether each is expected
/// to be caller-repairable.
///
/// This list is the rail's authority for *which* codes must be covered; the
/// repair *content* comes from the live producer.
const AUDITED_CODES: &[(LexicalErrorCode, bool)] = &[
    (LexicalErrorCode::BridgeAmbiguousFilter, true),
    (LexicalErrorCode::BridgeUnsupportedFilter, true),
    (LexicalErrorCode::BridgeUnsupportedDirective, true),
    (LexicalErrorCode::BridgeVersionPin, true),
    // Translator invariant break: not user-repairable, must carry no hint.
    (LexicalErrorCode::BridgeTranslateFail, false),
];

/// One audited code's outcome.
#[derive(Clone, Debug)]
pub struct CodeAudit {
    pub code: &'static str,
    pub expected_repairable: bool,
    pub repair: Option<QueryErrorRepair>,
    pub failures: Vec<String>,
}

impl CodeAudit {
    #[must_use]
    pub fn passed(&self) -> bool {
        self.failures.is_empty()
    }
}

/// The full ambiguity report.
#[derive(Clone, Debug)]
pub struct AmbiguityReport {
    pub audits: Vec<CodeAudit>,
    pub passed: bool,
}

fn audit_one(code: LexicalErrorCode, expected_repairable: bool) -> CodeAudit {
    let code_str = code.as_code_str();
    let typed_code = SearchPlaneErrorCodeV2::Lexical(code);
    let repair = repair_for_code(typed_code);
    let mut failures = Vec::new();

    match (&repair, expected_repairable) {
        (Some(repair), true) => {
            if repair.supported_alternatives.is_empty() {
                failures.push(format!(
                    "{code_str}: repairable but supported_alternatives is empty (hint with no alternatives)"
                ));
            }
            if repair.docs_anchor.is_none() {
                failures.push(format!("{code_str}: repairable but missing docs_anchor"));
            }
            if !payload_roundtrips(typed_code, Some(repair.clone())) {
                failures.push(format!("{code_str}: wire payload failed serde round-trip"));
            }
        }
        (None, false) => {
            // Correct: internal invariant break carries no repair.
        }
        (Some(_), false) => failures.push(format!(
            "{code_str}: expected NO repair (internal) but a hint was produced"
        )),
        (None, true) => failures.push(format!(
            "{code_str}: expected a repair hint but none was produced"
        )),
    }

    CodeAudit {
        code: code_str,
        expected_repairable,
        repair,
        failures,
    }
}

fn payload_roundtrips(code: SearchPlaneErrorCodeV2, repair: Option<QueryErrorRepair>) -> bool {
    let payload = SearchPlaneIpcError {
        code,
        message: "rail snapshot".to_string(),
        repair,
    };
    // Exercises the same manual Serialize/Deserialize impls the CBOR wire uses;
    // a JSON round-trip is an equivalent codec-agnostic validity check.
    let Ok(text) = serde_json::to_string(&payload) else {
        return false;
    };
    serde_json::from_str::<SearchPlaneIpcError>(&text).is_ok_and(|decoded| decoded == payload)
}

/// Cross-code invariants: the classes must stay distinct families.
fn class_distinctness_failures(audits: &[CodeAudit]) -> Vec<String> {
    let classes: Vec<RepairClass> = audits
        .iter()
        .filter_map(|a| a.repair.as_ref().map(|r| r.class))
        .collect();
    let mut failures = Vec::new();
    let has = |c: RepairClass| classes.contains(&c);
    if !has(RepairClass::Ambiguous) {
        failures.push("no error maps to the Ambiguous class".to_string());
    }
    if !has(RepairClass::Unsupported) {
        failures.push("no error maps to the Unsupported class".to_string());
    }
    let distinct: std::collections::BTreeSet<&'static str> =
        classes.iter().map(|c| c.as_code_str()).collect();
    if distinct.len() < 2 {
        failures.push(format!(
            "repair classes collapsed into {} bucket(s); expected >= 2 distinct families",
            distinct.len()
        ));
    }
    failures
}

/// Run the full ambiguity rail.
#[must_use]
pub fn run_ambiguity_report() -> AmbiguityReport {
    let mut audits: Vec<CodeAudit> = AUDITED_CODES
        .iter()
        .map(|(code, repairable)| audit_one(*code, *repairable))
        .collect();

    // Fold cross-code distinctness failures onto the first audit so they surface
    // in the per-code report without inventing a synthetic row.
    let cross = class_distinctness_failures(&audits);
    if let Some(first) = audits.first_mut() {
        first.failures.extend(cross);
    }

    let passed = audits.iter().all(CodeAudit::passed);
    AmbiguityReport { audits, passed }
}

fn repair_json(repair: &QueryErrorRepair) -> Value {
    json!({
        "class": repair.class.as_code_str(),
        "supported_alternatives": repair.supported_alternatives,
        "docs_anchor": repair.docs_anchor,
    })
}

fn audit_json(audit: &CodeAudit) -> Value {
    json!({
        "code": audit.code,
        "expected_repairable": audit.expected_repairable,
        "repair": audit.repair.as_ref().map(repair_json),
        "failures": audit.failures,
        "passed": audit.passed(),
    })
}

fn artifact_detail(report: &AmbiguityReport) -> Value {
    let payloads: Vec<Value> = report.audits.iter().map(audit_json).collect();
    json!({
        "passed": report.passed,
        "audited_codes": report.audits.len(),
        "audits": payloads,
        "fail_closed_note": "repair is advisory metadata on a still-failing code/message; no query is rewritten",
    })
}

/// The schema-2 ambiguity verdict envelope, bound to the exact error-code
/// payloads audited by this run.
pub fn artifact(
    report: &AmbiguityReport,
    git_head: GitHeadV1,
    host: HostV1,
) -> AnyResult<BenchArtifactV1> {
    let corpus = report
        .audits
        .iter()
        .map(|audit| (audit.code.to_string(), audit_json(audit).to_string()))
        .collect::<Vec<_>>();
    Ok(BenchArtifactV1 {
        dimension: "ambiguity".to_string(),
        mode: BenchMode::Warm,
        concurrency: 1,
        provenance: BenchProvenanceV1 {
            git_head,
            corpus_digest: corpus_digest("ambiguity", &corpus),
            config_digest: config_digest(
                "ambiguity",
                &[("audited_codes", report.audits.len().to_string())],
            ),
            model_revision: None,
        },
        host,
        resources: ResourceUsageV1::observe_self()?,
        phases: PhaseDurationsV1::default(),
        disk_amplification: None,
        rows: report
            .audits
            .iter()
            .map(|audit| BenchRowV1 {
                scenario_id: format!("ambiguity.{}", audit.code),
                route_family: RouteFamily::Lexical,
                syntax: BenchSyntax::Native,
                result_shape: ResultShape::TypedError,
                latency: None,
                qps: None,
                error_count: saturating_u64(audit.failures.len()),
                timeout_count: 0,
                result_count: None,
                typed_error_code: Some(audit.code.to_string()),
                engine_touched: vec!["search_plane_error_contract".to_string()],
                early_stop_reason: None,
            })
            .collect(),
        detail: artifact_detail(report),
    })
}

/// Write the authority envelope and its supplemental error-payload record.
pub fn write_artifacts(
    report: &AmbiguityReport,
    dir: &Path,
    git_head: GitHeadV1,
    host: HostV1,
) -> AnyResult<()> {
    let payloads: Vec<Value> = report.audits.iter().map(audit_json).collect();
    artifact(report, git_head.clone(), host)?.write_to(&dir.join("summary.json"))?;
    crate::artifact::write_json_pretty(
        &dir.join("error_payloads.json"),
        &json!({
            "schema_version": 2,
            "dimension": "ambiguity",
            "git_head": git_head.as_str(),
            "payloads": payloads,
        }),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_producer_satisfies_all_invariants() {
        let report = run_ambiguity_report();
        assert!(
            report.passed,
            "ambiguity invariants failed: {:?}",
            report
                .audits
                .iter()
                .flat_map(|a| a.failures.clone())
                .collect::<Vec<_>>()
        );
        let artifact = artifact(
            &report,
            GitHeadV1::parse(&"a".repeat(40)).expect("head"),
            HostV1::observe().expect("host"),
        )
        .expect("authority artifact")
        .to_json()
        .expect("json");
        assert_eq!(artifact["schema_version"], 2);
        assert_eq!(artifact["detail"]["passed"], true);
        assert_eq!(
            artifact["rows"].as_array().map(Vec::len),
            Some(report.audits.len())
        );
    }

    #[test]
    fn translate_fail_has_no_repair() {
        let report = run_ambiguity_report();
        let tf = report
            .audits
            .iter()
            .find(|a| a.code == LexicalErrorCode::BridgeTranslateFail.as_code_str())
            .expect("translate_fail audited");
        assert!(tf.repair.is_none(), "internal code must carry no hint");
    }

    #[test]
    fn ambiguous_and_unsupported_are_distinct_families() {
        let report = run_ambiguity_report();
        let class_of = |code: LexicalErrorCode| {
            report
                .audits
                .iter()
                .find(|a| a.code == code.as_code_str())
                .and_then(|a| a.repair.as_ref())
                .map(|r| r.class)
        };
        let amb = class_of(LexicalErrorCode::BridgeAmbiguousFilter);
        let uns = class_of(LexicalErrorCode::BridgeUnsupportedFilter);
        assert_eq!(amb, Some(RepairClass::Ambiguous));
        assert_eq!(uns, Some(RepairClass::Unsupported));
        assert_ne!(amb, uns, "distinct error families must not share a class");
    }

    #[test]
    fn distinctness_invariant_bites_on_collapsed_classes() {
        // Prove the gate can go RED: a report where every repair shares one
        // class must trip the distinctness invariant.
        let collapsed = vec![
            CodeAudit {
                code: "A",
                expected_repairable: true,
                repair: Some(QueryErrorRepair {
                    class: RepairClass::Unsupported,
                    supported_alternatives: vec!["repo:".to_string()],
                    docs_anchor: Some("docs".to_string()),
                }),
                failures: vec![],
            },
            CodeAudit {
                code: "B",
                expected_repairable: true,
                repair: Some(QueryErrorRepair {
                    class: RepairClass::Unsupported,
                    supported_alternatives: vec!["file:".to_string()],
                    docs_anchor: Some("docs".to_string()),
                }),
                failures: vec![],
            },
        ];
        let failures = class_distinctness_failures(&collapsed);
        assert!(
            failures.iter().any(|f| f.contains("collapsed")),
            "expected collapse to be flagged, got {failures:?}"
        );
        assert!(
            failures.iter().any(|f| f.contains("Ambiguous")),
            "missing Ambiguous class should be flagged"
        );
    }
}
