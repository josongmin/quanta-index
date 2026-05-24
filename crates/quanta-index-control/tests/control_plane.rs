#![expect(
    clippy::too_long_first_doc_paragraph,
    reason = "test helper comments favor a single explanatory block over forced reflow"
)]
#![expect(
    clippy::items_after_statements,
    reason = "tests intentionally put fixture-defining `use` lines next to the assertions they support"
)]

pub mod support;

use quanta_index_contract::{
    ManifestDigest, PublishedSearchBundlePrepareRequest, PublishedSearchGenerationActivateRequest,
    RepoId, RevisionId, SearchBundleMutationOp,
};
use quanta_index_control::ControlPlane;
use quanta_index_core::{
    CoreError, GenerationPinPort, PublishedSearchActivationStatePort,
    PublishedSearchBundleDeltaApplyPort, PublishedSearchBundleInspectPort,
    PublishedSearchBundlePreparePort, PublishedSearchGenerationActivatePort,
    PublishedSearchGenerationCatalogPort, PublishedSearchGenerationReadinessPort,
    SearchPlaneMetadataStorePort,
};
use tempfile::tempdir;

use self::support::{
    delta_request_for, sample_delta_request, sample_generation, sample_manifest,
    sample_manifest_all_optionals, sample_manifest_required_only, sample_outbox,
};

#[test]
fn prepared_bundle_is_visible_in_readiness_count() {
    let temp = match tempdir() {
        Ok(temp) => temp,
        Err(error) => {
            assert!(false, "tempdir failed: {error}");
            return;
        }
    };
    let db_path = temp.path().join("control-plane.sqlite3");
    let mut store = match ControlPlane::open(&db_path) {
        Ok(store) => store,
        Err(error) => {
            assert!(false, "open store failed: {error}");
            return;
        }
    };

    let prepare_result = store.prepare_bundle(PublishedSearchBundlePrepareRequest {
        outbox: sample_outbox(),
    });
    assert!(prepare_result.is_ok(), "prepare failed: {prepare_result:?}");

    let readiness_result = store.read_readiness(&RepoId::new("repo"), &RevisionId::new("rev"));
    let readiness = match readiness_result {
        Ok(readiness) => readiness,
        Err(error) => {
            assert!(false, "read readiness failed: {error}");
            return;
        }
    };

    assert_eq!(readiness.prepared_bundle_count, 1);
    assert!(readiness.active_generation.is_none());
}

#[test]
fn activated_generation_is_exposed_through_public_ports() {
    let temp = match tempdir() {
        Ok(temp) => temp,
        Err(error) => {
            assert!(false, "tempdir failed: {error}");
            return;
        }
    };
    let db_path = temp.path().join("control-plane.sqlite3");
    let mut store = match ControlPlane::open(&db_path) {
        Ok(store) => store,
        Err(error) => {
            assert!(false, "open store failed: {error}");
            return;
        }
    };
    let generation = sample_generation();

    let activation_result = store.activate_generation(PublishedSearchGenerationActivateRequest {
        generation: generation.clone(),
        lexical_ready: true,
        semantic_ready: true,
        active_at_ms: 42,
    });
    assert!(
        activation_result.is_ok(),
        "activate failed: {activation_result:?}"
    );

    let manifest = sample_manifest();
    let record_result = store.record_generation_manifest(manifest);
    assert!(record_result.is_ok(), "record failed: {record_result:?}");

    let readiness_result = store.read_readiness(&RepoId::new("repo"), &RevisionId::new("rev"));
    let readiness = match readiness_result {
        Ok(readiness) => readiness,
        Err(error) => {
            assert!(false, "read readiness failed: {error}");
            return;
        }
    };
    let inspected_result = store.inspect_bundle(&generation);
    let inspected = match inspected_result {
        Ok(inspected) => inspected,
        Err(error) => {
            assert!(false, "inspect failed: {error}");
            return;
        }
    };

    assert_eq!(readiness.active_generation, Some(generation.clone()));
    assert_eq!(inspected.manifest.repo_id, generation.repo_id);
    // sample_manifest has lexical_chunk_rows + symbol_rows (required) +
    // metadata_rows + graph_rows + embedding_records (optional) = 5 refs.
    assert_eq!(inspected.artifacts.len(), 5);
}

/// Open a temp control plane and activate the sample generation.
///
/// Returns `Err(message)` instead of panicking so callers can
/// `assert!(false, msg)` from inside the `#[test]` body and respect the
/// workspace `panic` ban.
fn open_with_active_generation(
    db_path: &std::path::Path,
) -> Result<(ControlPlane, PublishedSearchGenerationActivateRequest), String> {
    let mut store =
        ControlPlane::open(db_path).map_err(|error| format!("open store failed: {error}"))?;
    let generation = sample_generation();
    let activation = PublishedSearchGenerationActivateRequest {
        generation,
        lexical_ready: true,
        semantic_ready: true,
        active_at_ms: 42,
    };
    let _activation_response = store
        .activate_generation(activation.clone())
        .map_err(|error| format!("activate failed: {error}"))?;
    Ok((store, activation))
}

/// Open a temp control plane, activate generation A, then RECORD-ONLY (not
/// activate) generation B so delta-apply tests can target B per the SSOT
/// "delta against non-active generation" rule (delta governance P0).
fn open_with_recorded_non_active_generation(
    db_path: &std::path::Path,
) -> Result<(ControlPlane, quanta_index_contract::PublishedGenerationSet), String> {
    use quanta_index_contract::{
        BundleArtifactRef, BundleEncoding, GenerationId, ManifestGeneration,
        PublishedGenerationSet, PublishedSearchBundleManifest, RepoId, RevisionId,
    };
    let mut store =
        ControlPlane::open(db_path).map_err(|error| format!("open store failed: {error}"))?;
    // Activate G7 (sample) so the catalog has an active row.
    let active = sample_generation();
    let _activated = store
        .activate_generation(PublishedSearchGenerationActivateRequest {
            generation: active,
            lexical_ready: true,
            semantic_ready: true,
            active_at_ms: 42,
        })
        .map_err(|error| format!("activate g7 failed: {error}"))?;
    // Prepare a strictly-newer non-active generation G8 with a recorded
    // manifest. Delta-apply targets this one.
    let next = PublishedGenerationSet {
        repo_id: RepoId::new("repo"),
        revision_id: RevisionId::new("rev"),
        manifest_generation: ManifestGeneration::new(8),
        lexical_generation: GenerationId::new(20),
        symbol_generation: GenerationId::new(21),
        structural_generation: None,
        history_generation: None,
        semantic_generation: Some(GenerationId::new(22)),
        metadata_generation: Some(GenerationId::new(23)),
    };
    // Write catalog row for G8 via mark_active then... no, mark_active makes
    // it active. We need catalog-only. Use record_generation_manifest which
    // calls into INSERT OR REPLACE on the manifest table; we also need a
    // catalog row. Simplest: call record_generation_manifest with a manifest
    // for G8 (table only), then manually upsert a catalog row via a tiny
    // internal helper exposed for tests.
    let manifest = PublishedSearchBundleManifest {
        repo_id: next.repo_id.clone(),
        revision_id: next.revision_id.clone(),
        manifest_generation: next.manifest_generation,
        bundle_schema_version: 1,
        lexical_chunk_rows: BundleArtifactRef {
            relative_path: "bundle/chunk-g8.json".into(),
            encoding: BundleEncoding::Json,
            byte_length: 2,
            content_digest: quanta_index_contract::ManifestDigest::new("digest"),
        },
        symbol_rows: BundleArtifactRef {
            relative_path: "bundle/symbol-g8.json".into(),
            encoding: BundleEncoding::Json,
            byte_length: 2,
            content_digest: quanta_index_contract::ManifestDigest::new("digest"),
        },
        metadata_rows: None,
        graph_rows: None,
        embedding_input_views: None,
        embedding_records: None,
        mutation_delta: None,
    };
    store
        .record_generation_manifest(manifest)
        .map_err(|error| format!("record manifest g8 failed: {error}"))?;
    // Insert a catalog row for G8 via a write-through path. We don't have a
    // dedicated "record catalog row" port, so use the test-only direct write
    // through a private hook. Cleanest: extend control with a public
    // record-only catalog API. For Phase 1 tests we go through the test
    // harness file to keep production surface clean.
    use quanta_index_control::test_support::record_catalog_row_for_test;
    record_catalog_row_for_test(&store, &next)
        .map_err(|error| format!("record catalog g8 failed: {error}"))?;
    Ok((store, next))
}

#[test]
fn delta_apply_against_unknown_generation_is_rejected() {
    let temp = match tempdir() {
        Ok(temp) => temp,
        Err(error) => {
            assert!(false, "tempdir failed: {error}");
            return;
        }
    };
    let db_path = temp.path().join("control-plane.sqlite3");
    let mut store = match ControlPlane::open(&db_path) {
        Ok(store) => store,
        Err(error) => {
            assert!(false, "open failed: {error}");
            return;
        }
    };
    // No prior activation -> generation_catalog has no row for our delta target.
    let result = store.apply_bundle_delta(sample_delta_request());
    assert!(
        matches!(result, Err(CoreError::InvalidContract(_))),
        "expected InvalidContract, got {result:?}"
    );
}

macro_rules! ok_or_fail {
    ($expr:expr, $msg:expr) => {
        match $expr {
            Ok(v) => v,
            Err(error) => {
                assert!(false, "{}: {error}", $msg);
                return;
            }
        }
    };
}

#[test]
fn delta_apply_is_idempotent_against_known_generation() {
    let temp = ok_or_fail!(tempdir(), "tempdir failed");
    let db_path = temp.path().join("control-plane.sqlite3");
    let (mut store, target) =
        ok_or_fail!(open_with_recorded_non_active_generation(&db_path), "setup");

    let first = ok_or_fail!(
        store.apply_bundle_delta(delta_request_for(&target)),
        "first apply failed"
    );
    assert!(first.applied, "first apply should succeed");
    assert!(first.reason.is_none());

    let second = ok_or_fail!(
        store.apply_bundle_delta(delta_request_for(&target)),
        "second apply failed"
    );
    assert!(!second.applied, "second apply should be idempotent no-op");
    assert!(
        second
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("already applied")),
        "expected idempotent reason, got {:?}",
        second.reason
    );
}

#[test]
fn delta_apply_with_no_operations_reports_no_op() {
    let temp = ok_or_fail!(tempdir(), "tempdir failed");
    let db_path = temp.path().join("control-plane.sqlite3");
    let (mut store, target) =
        ok_or_fail!(open_with_recorded_non_active_generation(&db_path), "setup");

    let mut request = delta_request_for(&target);
    request.delta.operations.clear();
    let response = ok_or_fail!(store.apply_bundle_delta(request), "apply failed");
    assert!(!response.applied);
    assert!(
        response
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("no operations")),
        "unexpected reason: {:?}",
        response.reason
    );
}

#[test]
fn delta_apply_partial_mix_counts_as_applied() {
    let temp = ok_or_fail!(tempdir(), "tempdir failed");
    let db_path = temp.path().join("control-plane.sqlite3");
    let (mut store, target) =
        ok_or_fail!(open_with_recorded_non_active_generation(&db_path), "setup");

    // First apply: just one op
    let mut first_request = delta_request_for(&target);
    first_request.delta.operations = vec![SearchBundleMutationOp::UpsertChunk {
        chunk_identity: "chunk-A".into(),
        text_digest: "digest-A".into(),
    }];
    let first = ok_or_fail!(store.apply_bundle_delta(first_request), "first apply");
    assert!(first.applied);

    // Second apply: same op A (idempotent) + new op B (new)
    let mut mixed = delta_request_for(&target);
    mixed.delta.operations = vec![
        SearchBundleMutationOp::UpsertChunk {
            chunk_identity: "chunk-A".into(),
            text_digest: "digest-A".into(),
        },
        SearchBundleMutationOp::UpsertChunk {
            chunk_identity: "chunk-B".into(),
            text_digest: "digest-B".into(),
        },
    ];
    let mixed_result = ok_or_fail!(store.apply_bundle_delta(mixed), "mixed apply");
    assert!(
        mixed_result.applied,
        "mixed delta with at least one new op should report applied"
    );
    assert!(mixed_result.reason.is_none());
}

#[test]
fn delta_apply_rejects_mismatched_repo_id() {
    let temp = ok_or_fail!(tempdir(), "tempdir failed");
    let db_path = temp.path().join("control-plane.sqlite3");
    let (mut store, target) =
        ok_or_fail!(open_with_recorded_non_active_generation(&db_path), "setup");

    let mut bad = delta_request_for(&target);
    bad.delta.repo_id = RepoId::new("other-repo");
    let result = store.apply_bundle_delta(bad);
    assert!(
        matches!(result, Err(CoreError::InvalidContract(_))),
        "expected InvalidContract, got {result:?}"
    );
}

#[test]
fn delta_apply_against_active_generation_is_forbidden() {
    // P0 governance per SSOT: delta applies are forbidden against the
    // currently-active generation; preparation always targets a NEW generation.
    let temp = ok_or_fail!(tempdir(), "tempdir failed");
    let db_path = temp.path().join("control-plane.sqlite3");
    let (mut store, activation) = ok_or_fail!(open_with_active_generation(&db_path), "setup");
    // Record manifest for active generation so the "manifest recorded" check
    // would pass — this isolates the test to the "is active?" rejection path.
    let _recorded: Result<(), quanta_index_core::CoreError> =
        store.record_generation_manifest(sample_manifest());
    let result = store.apply_bundle_delta(delta_request_for(&activation.generation));
    assert!(
        matches!(result, Err(CoreError::InvalidContract(_))),
        "expected InvalidContract (delta against active), got {result:?}"
    );
}

#[test]
fn delta_apply_requires_recorded_manifest() {
    // P0 governance: delta-apply requires that a canonical manifest already
    // exists for the target generation; otherwise the delta operations have
    // no schema to resolve against.
    let temp = ok_or_fail!(tempdir(), "tempdir failed");
    let db_path = temp.path().join("control-plane.sqlite3");
    let mut store = ok_or_fail!(ControlPlane::open(&db_path), "open");
    // Activate so a generation exists in catalog, but DON'T record manifest
    // for the next generation we'll target.
    let _activated: Result<_, quanta_index_core::CoreError> =
        store.activate_generation(PublishedSearchGenerationActivateRequest {
            generation: sample_generation(),
            lexical_ready: true,
            semantic_ready: true,
            active_at_ms: 42,
        });
    // Construct a delta against a fresh non-active generation that has a
    // catalog row but no manifest.
    use quanta_index_contract::{GenerationId, ManifestGeneration};
    use quanta_index_control::test_support::record_catalog_row_for_test;
    let mut next = sample_generation();
    next.manifest_generation = ManifestGeneration::new(8);
    next.lexical_generation = GenerationId::new(20);
    next.symbol_generation = GenerationId::new(21);
    next.semantic_generation = Some(GenerationId::new(22));
    let _catalog: Result<(), quanta_index_core::CoreError> =
        record_catalog_row_for_test(&store, &next);
    let result = store.apply_bundle_delta(delta_request_for(&next));
    assert!(
        matches!(result, Err(CoreError::InvalidContract(_))),
        "expected InvalidContract (manifest missing), got {result:?}"
    );
}

#[test]
fn mark_active_generation_sets_active_pointer() {
    let temp = ok_or_fail!(tempdir(), "tempdir failed");
    let db_path = temp.path().join("control-plane.sqlite3");
    let mut store = ok_or_fail!(ControlPlane::open(&db_path), "open failed");

    let generation = sample_generation();
    let result = store.mark_active_generation(&generation, 12345);
    assert!(result.is_ok(), "mark failed: {result:?}");

    let readiness = ok_or_fail!(
        store.read_readiness(&generation.repo_id, &generation.revision_id),
        "read readiness"
    );
    assert_eq!(readiness.active_generation, Some(generation.clone()));
    assert!(readiness.lexical_ready);
    assert!(readiness.semantic_ready);
}

#[test]
fn mark_active_generation_replaces_existing_active_row() {
    let temp = ok_or_fail!(tempdir(), "tempdir failed");
    let db_path = temp.path().join("control-plane.sqlite3");
    let mut store = ok_or_fail!(ControlPlane::open(&db_path), "open failed");

    let generation = sample_generation();
    ok_or_fail!(store.mark_active_generation(&generation, 100), "first mark");
    ok_or_fail!(
        store.mark_active_generation(&generation, 200),
        "second mark"
    );

    let readiness = ok_or_fail!(
        store.read_readiness(&generation.repo_id, &generation.revision_id),
        "read readiness"
    );
    // Active generation pointer unchanged; readiness still both true.
    assert_eq!(readiness.active_generation, Some(generation));
    assert!(readiness.lexical_ready);
    assert!(readiness.semantic_ready);
}

#[test]
fn activate_generation_rejects_stale_manifest_generation() {
    // P0 E-SP2: activating a lower manifest_generation against a (repo, rev)
    // whose currently-active manifest_generation is higher must be rejected
    // as stale, even if readiness flags are both true.
    let temp = ok_or_fail!(tempdir(), "tempdir failed");
    let db_path = temp.path().join("control-plane.sqlite3");
    let (mut store, activation) = ok_or_fail!(open_with_active_generation(&db_path), "setup");
    // Build a stale generation (manifest_generation = active - 1).
    use quanta_index_contract::ManifestGeneration;
    let mut stale = activation.generation;
    stale.manifest_generation = ManifestGeneration::new(stale.manifest_generation.get() - 1);
    let result = store.activate_generation(PublishedSearchGenerationActivateRequest {
        generation: stale,
        lexical_ready: true,
        semantic_ready: true,
        active_at_ms: 100,
    });
    assert!(
        matches!(result, Err(CoreError::InvalidContract(_))),
        "expected InvalidContract (stale activation), got {result:?}"
    );
}

#[test]
fn activate_generation_rejects_regressing_component_generation() {
    // P0 E-SP2: even when manifest_generation increases, a component
    // generation regression (e.g. lexical_generation lower than active) is
    // rejected to preserve component monotonicity invariants.
    let temp = ok_or_fail!(tempdir(), "tempdir failed");
    let db_path = temp.path().join("control-plane.sqlite3");
    let (mut store, activation) = ok_or_fail!(open_with_active_generation(&db_path), "setup");
    use quanta_index_contract::{GenerationId, ManifestGeneration};
    let mut regressing = activation.generation.clone();
    regressing.manifest_generation =
        ManifestGeneration::new(activation.generation.manifest_generation.get() + 1);
    regressing.lexical_generation =
        GenerationId::new(activation.generation.lexical_generation.get() - 1);
    let result = store.activate_generation(PublishedSearchGenerationActivateRequest {
        generation: regressing,
        lexical_ready: true,
        semantic_ready: true,
        active_at_ms: 100,
    });
    assert!(
        matches!(result, Err(CoreError::InvalidContract(_))),
        "expected InvalidContract (lexical regression), got {result:?}"
    );
}

#[test]
fn mark_active_generation_also_rejects_stale_target() {
    // P0: the orchestrator-side path through mark_active_generation must
    // honor the same stale guard so out-of-order build outcomes can't roll
    // the pointer backwards.
    let temp = ok_or_fail!(tempdir(), "tempdir failed");
    let db_path = temp.path().join("control-plane.sqlite3");
    let (mut store, activation) = ok_or_fail!(open_with_active_generation(&db_path), "setup");
    use quanta_index_contract::ManifestGeneration;
    let mut stale = activation.generation;
    stale.manifest_generation = ManifestGeneration::new(stale.manifest_generation.get() - 1);
    let result = store.mark_active_generation(&stale, 100);
    assert!(
        matches!(result, Err(CoreError::InvalidContract(_))),
        "expected InvalidContract (stale mark), got {result:?}"
    );
}

#[test]
fn inspect_bundle_artifacts_include_mutation_delta_when_present() {
    // P1: mutation_delta is itself a BundleArtifactRef in the frozen
    // contract, so the inspect artifacts union must surface it alongside
    // the chunk/symbol/etc. refs.
    let temp = ok_or_fail!(tempdir(), "tempdir");
    let db_path = temp.path().join("control-plane.sqlite3");
    let mut store = ok_or_fail!(ControlPlane::open(&db_path), "open");
    let generation = sample_generation();
    let _activated = store.activate_generation(PublishedSearchGenerationActivateRequest {
        generation: generation.clone(),
        lexical_ready: true,
        semantic_ready: true,
        active_at_ms: 1,
    });

    let mut manifest = sample_manifest_all_optionals();
    let mutation_delta_ref = quanta_index_contract::BundleArtifactRef {
        relative_path: "bundle/mutation_delta.arrow".into(),
        encoding: quanta_index_contract::BundleEncoding::ArrowIpc,
        byte_length: 64,
        content_digest: ManifestDigest::new("sha256:mutation"),
    };
    manifest.mutation_delta = Some(mutation_delta_ref.clone());
    let _recorded = store.record_generation_manifest(manifest);

    let inspected = ok_or_fail!(store.inspect_bundle(&generation), "inspect");
    assert!(
        inspected.artifacts.contains(&mutation_delta_ref),
        "artifacts must include mutation_delta when present; got {:?}",
        inspected.artifacts
    );
    // 2 required + 4 optionals + 1 mutation_delta = 7
    assert_eq!(inspected.artifacts.len(), 7);
}

#[test]
fn delta_apply_rejects_mismatched_revision_id() {
    let temp = ok_or_fail!(tempdir(), "tempdir failed");
    let db_path = temp.path().join("control-plane.sqlite3");
    let (mut store, target) =
        ok_or_fail!(open_with_recorded_non_active_generation(&db_path), "setup");

    let mut bad = delta_request_for(&target);
    bad.delta.revision_id = RevisionId::new("other-rev");
    let result = store.apply_bundle_delta(bad);
    assert!(
        matches!(result, Err(CoreError::InvalidContract(_))),
        "expected InvalidContract, got {result:?}"
    );
}

#[test]
fn record_generation_manifest_then_inspect_roundtrips_manifest() {
    let temp = ok_or_fail!(tempdir(), "tempdir failed");
    let db_path = temp.path().join("control-plane.sqlite3");
    let mut store = ok_or_fail!(ControlPlane::open(&db_path), "open failed");

    let manifest = sample_manifest();
    ok_or_fail!(
        store.record_generation_manifest(manifest.clone()),
        "record failed"
    );

    let generation = sample_generation();
    let inspected = ok_or_fail!(store.inspect_bundle(&generation), "inspect failed");

    assert_eq!(inspected.manifest, manifest);
    assert_eq!(inspected.mode, "serve_only");
    assert_eq!(inspected.state, "active");
}

#[test]
fn record_generation_manifest_is_idempotent_for_identical_payload() {
    let temp = ok_or_fail!(tempdir(), "tempdir failed");
    let db_path = temp.path().join("control-plane.sqlite3");
    let mut store = ok_or_fail!(ControlPlane::open(&db_path), "open failed");

    let manifest = sample_manifest();
    ok_or_fail!(
        store.record_generation_manifest(manifest.clone()),
        "first record failed"
    );
    ok_or_fail!(
        store.record_generation_manifest(manifest.clone()),
        "second record (identical) failed"
    );

    let generation = sample_generation();
    let inspected = ok_or_fail!(store.inspect_bundle(&generation), "inspect failed");
    assert_eq!(inspected.manifest, manifest);
}

#[test]
fn record_generation_manifest_replaces_existing_for_same_key() {
    let temp = ok_or_fail!(tempdir(), "tempdir failed");
    let db_path = temp.path().join("control-plane.sqlite3");
    let mut store = ok_or_fail!(ControlPlane::open(&db_path), "open failed");

    let first = sample_manifest();
    ok_or_fail!(
        store.record_generation_manifest(first),
        "first record failed"
    );

    let mut second = sample_manifest();
    second.bundle_schema_version = 2;
    second.lexical_chunk_rows.byte_length = 9999;
    second.lexical_chunk_rows.content_digest = ManifestDigest::new("sha256:replaced-chunk");
    ok_or_fail!(
        store.record_generation_manifest(second.clone()),
        "replacement record failed"
    );

    let generation = sample_generation();
    let inspected = ok_or_fail!(store.inspect_bundle(&generation), "inspect failed");
    assert_eq!(inspected.manifest, second);
}

#[test]
fn inspect_bundle_for_unknown_generation_returns_not_found() {
    let temp = ok_or_fail!(tempdir(), "tempdir failed");
    let db_path = temp.path().join("control-plane.sqlite3");
    let store = ok_or_fail!(ControlPlane::open(&db_path), "open failed");

    let generation = sample_generation();
    let result = store.inspect_bundle(&generation);
    assert!(
        matches!(result, Err(CoreError::NotFound(_))),
        "expected NotFound, got {result:?}"
    );
}

#[test]
fn record_generation_manifest_rejects_zero_byte_length_artifact_ref() {
    let temp = ok_or_fail!(tempdir(), "tempdir failed");
    let db_path = temp.path().join("control-plane.sqlite3");
    let mut store = ok_or_fail!(ControlPlane::open(&db_path), "open failed");

    let mut bad = sample_manifest();
    bad.lexical_chunk_rows.byte_length = 0;
    let result = store.record_generation_manifest(bad);
    assert!(
        matches!(result, Err(CoreError::InvalidContract(_))),
        "expected InvalidContract, got {result:?}"
    );
}

#[test]
fn record_generation_manifest_rejects_empty_content_digest() {
    let temp = ok_or_fail!(tempdir(), "tempdir failed");
    let db_path = temp.path().join("control-plane.sqlite3");
    let mut store = ok_or_fail!(ControlPlane::open(&db_path), "open failed");

    let mut bad = sample_manifest();
    bad.symbol_rows.content_digest = ManifestDigest::new("");
    let result = store.record_generation_manifest(bad);
    assert!(
        matches!(result, Err(CoreError::InvalidContract(_))),
        "expected InvalidContract, got {result:?}"
    );
}

#[test]
fn open_metadata_store_returns_ok_after_record() {
    let temp = ok_or_fail!(tempdir(), "tempdir failed");
    let db_path = temp.path().join("control-plane.sqlite3");
    let mut store = ok_or_fail!(ControlPlane::open(&db_path), "open failed");

    ok_or_fail!(
        store.record_generation_manifest(sample_manifest()),
        "record failed"
    );

    let generation = sample_generation();
    let result = store.open_metadata_store(&generation);
    assert!(result.is_ok(), "expected Ok, got {result:?}");
}

#[test]
fn open_metadata_store_returns_not_ready_when_no_record() {
    let temp = ok_or_fail!(tempdir(), "tempdir failed");
    let db_path = temp.path().join("control-plane.sqlite3");
    let store = ok_or_fail!(ControlPlane::open(&db_path), "open failed");

    let generation = sample_generation();
    let result = store.open_metadata_store(&generation);
    assert!(
        matches!(result, Err(CoreError::NotReady(_))),
        "expected NotReady, got {result:?}"
    );
}

#[test]
fn inspect_bundle_artifacts_match_union_of_present_refs() {
    let temp = ok_or_fail!(tempdir(), "tempdir failed");
    let db_path = temp.path().join("control-plane.sqlite3");
    let mut store = ok_or_fail!(ControlPlane::open(&db_path), "open failed");

    // Required-only manifest: 2 present refs (lexical_chunk_rows + symbol_rows)
    let required_only = sample_manifest_required_only();
    ok_or_fail!(
        store.record_generation_manifest(required_only.clone()),
        "record required-only failed"
    );

    let generation = sample_generation();
    let inspected_required = ok_or_fail!(store.inspect_bundle(&generation), "inspect failed");
    assert_eq!(inspected_required.artifacts.len(), 2);
    assert!(
        inspected_required
            .artifacts
            .contains(&required_only.lexical_chunk_rows)
    );
    assert!(
        inspected_required
            .artifacts
            .contains(&required_only.symbol_rows)
    );

    // All-optionals manifest: 2 required + 4 optionals = 6 present refs.
    let all_optionals = sample_manifest_all_optionals();
    ok_or_fail!(
        store.record_generation_manifest(all_optionals.clone()),
        "record all-optionals failed"
    );

    let inspected_full = ok_or_fail!(store.inspect_bundle(&generation), "inspect failed");
    assert_eq!(inspected_full.artifacts.len(), 6);
    assert!(
        inspected_full
            .artifacts
            .contains(&all_optionals.lexical_chunk_rows)
    );
    assert!(
        inspected_full
            .artifacts
            .contains(&all_optionals.symbol_rows)
    );
    let Some(metadata_rows_ref) = all_optionals.metadata_rows.as_ref() else {
        assert!(
            false,
            "expected metadata_rows ref in all-optionals manifest"
        );
        return;
    };
    let Some(graph_rows_ref) = all_optionals.graph_rows.as_ref() else {
        assert!(false, "expected graph_rows ref in all-optionals manifest");
        return;
    };
    let Some(embedding_input_views_ref) = all_optionals.embedding_input_views.as_ref() else {
        assert!(
            false,
            "expected embedding_input_views ref in all-optionals manifest"
        );
        return;
    };
    let Some(embedding_records_ref) = all_optionals.embedding_records.as_ref() else {
        assert!(
            false,
            "expected embedding_records ref in all-optionals manifest"
        );
        return;
    };
    assert!(inspected_full.artifacts.contains(metadata_rows_ref));
    assert!(inspected_full.artifacts.contains(graph_rows_ref));
    assert!(inspected_full.artifacts.contains(embedding_input_views_ref));
    assert!(inspected_full.artifacts.contains(embedding_records_ref));
}

#[test]
fn pin_generation_against_unknown_revision_is_none() {
    let temp = ok_or_fail!(tempdir(), "tempdir failed");
    let db_path = temp.path().join("control-plane.sqlite3");
    let store = ok_or_fail!(ControlPlane::open(&db_path), "open failed");

    let pin = ok_or_fail!(
        store.pin_generation(&RepoId::new("unknown"), &RevisionId::new("never")),
        "pin failed"
    );
    let snapshot = ok_or_fail!(pin.pinned_generation(), "snapshot read");
    assert!(snapshot.is_none(), "expected no pin, got {snapshot:?}");
}

#[test]
fn pin_generation_after_activation_returns_active_snapshot() {
    let temp = ok_or_fail!(tempdir(), "tempdir failed");
    let db_path = temp.path().join("control-plane.sqlite3");
    let (store, activation) = ok_or_fail!(open_with_active_generation(&db_path), "setup failed");
    let pin = ok_or_fail!(
        store.pin_generation(
            &activation.generation.repo_id,
            &activation.generation.revision_id,
        ),
        "pin failed"
    );
    let snapshot = ok_or_fail!(pin.pinned_generation(), "snapshot read");
    assert_eq!(snapshot, Some(activation.generation));
}

#[test]
fn pin_snapshot_survives_subsequent_activation_for_same_repo_revision() {
    // U-SP4 scaffold: a pin captured at time T sees the generation that was
    // active at T, even if a later writer activates a different generation
    // for the same (repo, rev). The orchestrator/query layer relies on this
    // immutable-snapshot invariant to avoid mid-flight generation switches.
    let temp = ok_or_fail!(tempdir(), "tempdir failed");
    let db_path = temp.path().join("control-plane.sqlite3");
    let (mut store, activation) =
        ok_or_fail!(open_with_active_generation(&db_path), "setup failed");

    let pin_g1 = ok_or_fail!(
        store.pin_generation(
            &activation.generation.repo_id,
            &activation.generation.revision_id,
        ),
        "pin g1 failed"
    );

    // Activate G2 — a different manifest_generation under the same (repo, rev).
    let mut g2 = activation.generation.clone();
    g2.manifest_generation = quanta_index_contract::ManifestGeneration::new(
        activation.generation.manifest_generation.get() + 1,
    );
    ok_or_fail!(store.mark_active_generation(&g2, 999), "mark_active g2");

    // Existing pin still reports G1.
    let snap_g1 = ok_or_fail!(pin_g1.pinned_generation(), "snap g1");
    assert_eq!(
        snap_g1.as_ref().map(|gens| gens.manifest_generation),
        Some(activation.generation.manifest_generation),
        "in-flight pin must observe pre-activation snapshot"
    );

    // A fresh pin sees the new active generation.
    let pin_g2 = ok_or_fail!(store.pin_generation(&g2.repo_id, &g2.revision_id), "pin g2");
    let snap_g2 = ok_or_fail!(pin_g2.pinned_generation(), "snap g2");
    assert_eq!(
        snap_g2.as_ref().map(|gens| gens.manifest_generation),
        Some(g2.manifest_generation),
        "post-activation pin must observe the new generation"
    );
}

#[test]
fn bound_generation_pin_from_snapshot_returns_constructed_value() {
    use quanta_index_control::BoundGenerationPin;
    let generation = sample_generation();
    let pin = BoundGenerationPin::from_snapshot(Some(generation.clone()));
    let snapshot = ok_or_fail!(pin.pinned_generation(), "snapshot");
    assert_eq!(snapshot, Some(generation));

    let empty = BoundGenerationPin::from_snapshot(None);
    let snapshot = ok_or_fail!(empty.pinned_generation(), "empty snapshot");
    assert!(snapshot.is_none());
}
