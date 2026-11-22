//! Formal scale quality artifact from the existing Native producer and SDK.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Context as _, Result, ensure};
use quanta_index_contract::index::{ObjectRef, OwnerKey, ViewRef, ViewSelection};
use quanta_index_contract::native_protocol::NativeTextCoverage;
use quanta_index_contract::{QueryConstraintSetV1, TextQueryRequest, TextQuerySyntax};
use quanta_index_sdk::native_uds::NativeUdsClient;
use serde_json::json;

use super::{
    BenchArtifactV1, BenchMode, GitHeadV1, HostV1, LatencySummary, NativeIndexMeasurement, budget,
    config_digest, index_native_artifact, owner, params_for, text_spec,
};
use crate::source_fixture::{SCALE_QUERY_TOKEN, ScopedFile};

const WARM_QUERY_SAMPLES: usize = 32;
const SCALE_TOP_K: u32 = 10;
const REQUIRED_PHASES: &[&str] = &[
    "workspace_create",
    "profile_configure",
    "full_begin_input",
    "full_stage_sources",
    "full_event_ack",
    "first_search",
    "warm_search",
    "delta_stage_source",
    "delta_event_ack",
    "delta_search",
    "noop_event_ack",
    "noop_stable_search",
    "delete_event_ack",
    "deleted_search",
    "old_pin_search",
    "drain",
    "reopen",
    "reopen_read_head",
    "reopen_deleted_search",
    "reopen_search",
    "final_drain",
];

pub(super) struct WarmSamples {
    pub(super) samples_ms: Vec<f64>,
    pub(super) result_count: u64,
    pub(super) matched: u64,
}

pub(super) fn measure_warm_queries(
    client: &NativeUdsClient,
    view: ViewRef,
    corpus: &[ScopedFile],
    refs: &[ObjectRef],
) -> Result<WarmSamples> {
    ensure!(
        !corpus.is_empty() && corpus.len() == refs.len(),
        "Native scale quality source/ACK width mismatch"
    );
    let mut expected = BTreeMap::<OwnerKey, (ObjectRef, String)>::new();
    for (index, (file, reference)) in corpus.iter().zip(refs).enumerate() {
        ensure!(
            file.content.contains(SCALE_QUERY_TOKEN),
            "Native scale quality source lacks the planted query"
        );
        let previous = expected.insert(
            owner(index)?,
            (
                *reference,
                format!("{}/{}", file.source_repo_id, file.repo_relative_path),
            ),
        );
        ensure!(
            previous.is_none(),
            "Native scale quality duplicate source owner"
        );
    }
    let expected_matched = u64::try_from(expected.len())?;
    let expected_hits = usize::min(expected.len(), usize::try_from(SCALE_TOP_K)?);
    let mut samples = Vec::with_capacity(WARM_QUERY_SAMPLES);
    for _ in 0..WARM_QUERY_SAMPLES {
        let started = std::time::Instant::now();
        let page = client.search_text(
            ViewSelection::Exact(view),
            text_spec().instance,
            TextQueryRequest {
                syntax: TextQuerySyntax::Native,
                query_text: SCALE_QUERY_TOKEN.into(),
                constraints: QueryConstraintSetV1::unconstrained(),
                generation: None,
                generation_selector: None,
                top_k: SCALE_TOP_K,
                cursor: None,
            },
            &budget(),
        )?;
        let latency_ms = started.elapsed().as_secs_f64() * 1_000.0;
        ensure!(
            page.view == view
                && page.instance == text_spec().instance
                && page.coverage == NativeTextCoverage::CoreRequired
                && page.exhaustive
                && page.matched == expected_matched
                && page.hits.len() == expected_hits,
            "Native scale warm query returned a wrong view, coverage or source count"
        );
        let mut seen = BTreeSet::new();
        for hit in &page.hits {
            let (source, path) = expected
                .get(&hit.owner)
                .context("Native scale warm query returned a foreign source owner")?;
            ensure!(
                hit.source == *source && hit.path == *path && seen.insert(hit.owner),
                "Native scale warm query returned a foreign or duplicate source identity"
            );
        }
        samples.push(latency_ms);
    }
    Ok(WarmSamples {
        samples_ms: samples,
        result_count: u64::try_from(expected_hits)?,
        matched: expected_matched,
    })
}

pub(super) fn validate_quality(measurement: &NativeIndexMeasurement) -> Result<LatencySummary> {
    ensure!(
        measurement.warm_samples_ms.len() == WARM_QUERY_SAMPLES
            && measurement
                .warm_samples_ms
                .iter()
                .all(|ms| ms.is_finite() && *ms >= 0.0),
        "Native scale quality has invalid warm SDK samples"
    );
    let warm = LatencySummary::from_samples_ms(&measurement.warm_samples_ms)
        .context("Native scale warm samples absent")?;
    ensure!(
        measurement.first_query_matched == 1
            && measurement.first_query_ms.is_finite()
            && measurement.first_query_ms >= 0.0,
        "Native scale quality first query oracle absent"
    );
    ensure!(
        measurement.file_count == usize::try_from(params_for(measurement.tier).total_files())?
            && measurement.source_bytes > 0
            && !measurement.corpus_digest.is_empty(),
        "Native scale quality source binding invalid"
    );
    ensure!(
        measurement.warm_matched == Some(u64::try_from(measurement.file_count)?)
            && measurement.warm_result_count
                == Some(u64::try_from(usize::min(
                    measurement.file_count,
                    usize::try_from(SCALE_TOP_K)?,
                ))?),
        "Native scale quality result count differs from planted source gold"
    );
    ensure!(
        measurement.mixed_load.is_none() && measurement.churn_cycles == 0,
        "Native scale quality requires the fixed standard lifecycle"
    );
    for name in REQUIRED_PHASES {
        ensure!(
            measurement.phases.get(name).is_some_and(|phase| {
                phase.completed && phase.wall_ms.is_finite() && phase.wall_ms >= 0.0
            }),
            "Native scale quality phase {name} missing or incomplete"
        );
    }
    Ok(warm)
}

fn quality_artifact(
    measurement: &NativeIndexMeasurement,
    git_head: GitHeadV1,
    host: HostV1,
) -> Result<BenchArtifactV1> {
    let warm = validate_quality(measurement)?;
    let mut artifact = index_native_artifact(measurement, git_head, host)?;
    let native_config_digest = artifact.provenance.config_digest.clone();
    artifact.dimension = "scale".into();
    artifact.mode = BenchMode::Cold;
    artifact.provenance.config_digest = config_digest(
        "scale",
        &[
            ("native_index_config_digest", native_config_digest),
            ("warm_query_samples", WARM_QUERY_SAMPLES.to_string()),
            ("warm_query", SCALE_QUERY_TOKEN.into()),
            ("warm_top_k", SCALE_TOP_K.to_string()),
            ("warm_expected_matches", measurement.file_count.to_string()),
            ("quality_contract", "native-scale-source-oracle-v1".into()),
        ],
    );
    let row = artifact
        .rows
        .first_mut()
        .context("Native scale quality row absent")?;
    row.scenario_id = format!("scale.{}.warm_query", measurement.tier.as_str());
    row.latency = Some(warm);
    row.result_count = measurement.warm_result_count;
    let detail = artifact
        .detail
        .as_object_mut()
        .context("Native scale detail absent")?;
    let _old = detail.insert("passed".into(), json!(true));
    let _old = detail.insert(
        "warm_query".into(),
        json!({
            "query":SCALE_QUERY_TOKEN,
            "top_k":SCALE_TOP_K,
            "matched":measurement.warm_matched,
            "result_count":measurement.warm_result_count,
            "latency":warm,
            "samples_ms":&measurement.warm_samples_ms,
        }),
    );
    let _old = detail.insert("quality_scope".into(), json!(
        "32 actual verified SDK global planted-token top-10 queries against the first published full view; same Native daemon, source-bound owner/reference/path/view/full-match gold; full/delta/noop/delete/old-pin/reopen/drain completed"
    ));
    Ok(artifact)
}

/// Write a schema-2 `scale` artifact only after the Native lifecycle and all
/// measured warm SDK queries pass independent source and phase checks.
pub fn write_index_native_quality_artifacts(
    measurement: &NativeIndexMeasurement,
    dir: &Path,
    git_head: GitHeadV1,
    host: HostV1,
) -> Result<()> {
    let artifact = quality_artifact(measurement, git_head, host)
        .map_err(|error| measurement.with_failure_capture_paths(error))?;
    let result = (|| -> Result<()> {
        crate::artifact::write_json_pretty(
            &dir.join("tier_manifest.json"),
            &crate::scale_provenance::tier_manifest_json(),
        )?;
        artifact.write_to(&dir.join("summary.json"))?;
        Ok(())
    })();
    result.map_err(|error| measurement.with_failure_capture_paths(error))
}

#[cfg(test)]
mod tests {
    use super::super::NativePhase;
    use super::*;
    use std::collections::BTreeMap;

    fn fixture() -> NativeIndexMeasurement {
        let phases = REQUIRED_PHASES
            .iter()
            .map(|name| {
                (
                    *name,
                    NativePhase {
                        wall_ms: 1.0,
                        root_bytes_before: Some(1),
                        root_bytes_after: Some(1),
                        observer_ms: 0.0,
                        completed: true,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        NativeIndexMeasurement {
            tier: crate::source_fixture::ScaleTier::Small,
            seed: 42,
            corpus_digest: "fixed-source".into(),
            daemon_sha256: "00".repeat(32),
            native_config_sha256: "11".repeat(32),
            foreground_workload: json!({"fixed":true}),
            publication_outcomes: json!({"fixed":true}),
            file_count: 16,
            source_bytes: 1024,
            phases,
            first_query_ms: 1.0,
            first_query_matched: 1,
            warm_samples_ms: vec![1.0; WARM_QUERY_SAMPLES],
            warm_result_count: Some(10),
            warm_matched: Some(16),
            total_ms: 100.0,
            resource_observation: None,
            churn_cycles: 0,
            churn: None,
            child_trace_verified: false,
            child_object_io: None,
            mixed_load: None,
            child_traces: None,
        }
    }

    fn provenance() -> (GitHeadV1, HostV1) {
        (
            GitHeadV1::parse("0123456789abcdef0123456789abcdef01234567").expect("fixed head"),
            HostV1 {
                os: "linux".into(),
                arch: "aarch64".into(),
                cpu_count: 2,
                mem_bytes: 6_442_450_944,
                hostname_hash: "fixed-test-host".into(),
            },
        )
    }

    #[test]
    fn quality_assembles_scale_dimension_from_verified_native_measurement() {
        let measured = fixture();
        let (head, host) = provenance();
        let artifact = quality_artifact(&measured, head, host).expect("quality artifact");
        let value = artifact.to_json().expect("serialize quality artifact");
        assert_eq!(value["dimension"], "scale");
        assert_eq!(value["detail"]["passed"], true);
        assert_eq!(value["rows"][0]["scenario_id"], "scale.small.warm_query");
        assert_eq!(value["rows"][0]["latency"]["samples"], 32);
        assert_eq!(value["rows"][0]["result_count"], 10);
        assert_eq!(value["detail"]["warm_query"]["matched"], 16);
        assert_eq!(value["detail"]["warm_query"]["query"], SCALE_QUERY_TOKEN);
        assert_eq!(
            value["detail"]["warm_query"]["samples_ms"]
                .as_array()
                .map(Vec::len),
            Some(32)
        );
    }

    #[test]
    fn quality_requires_real_sample_count_and_all_lifecycle_phases() {
        let mut measured = fixture();
        assert_eq!(
            validate_quality(&measured).expect("fixed quality").samples,
            32
        );
        measured.warm_samples_ms = vec![1.0];
        assert!(validate_quality(&measured).is_err());
        measured.warm_samples_ms = vec![1.0; WARM_QUERY_SAMPLES];
        let _removed = measured.phases.remove("old_pin_search");
        assert!(validate_quality(&measured).is_err());
    }

    #[test]
    fn quality_rejects_wrong_corpus_and_never_promotes_raw_one_sample() {
        let mut measured = fixture();
        measured.file_count = 15;
        assert!(validate_quality(&measured).is_err());
        measured.file_count = 16;
        measured.warm_samples_ms.clear();
        assert!(validate_quality(&measured).is_err());
        measured.warm_samples_ms = vec![1.0; WARM_QUERY_SAMPLES];
        measured.warm_result_count = Some(1);
        assert!(validate_quality(&measured).is_err());
    }
}
