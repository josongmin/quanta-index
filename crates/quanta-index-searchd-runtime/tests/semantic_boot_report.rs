//! LDB-E2E-01 observability: the assembled runtime exposes a populated,
//!
//! payload-free `SemanticBootReport` (migration outcome + seeded sealed-
//! generation count + cold-boot seed timing). This proves the boot path
//! surfaces direct-open vs migration, not just that the helpers compute it.

#![forbid(unsafe_code)]
#![expect(
    clippy::expect_used,
    reason = "integration-test helpers outside `#[test]` fns assert fixture setup with `expect`; the workspace already permits this inside test fns and a helper that cannot set up its fixture has no caller to propagate to"
)]

use std::path::{Path, PathBuf};

use quanta_index_contract::{
    BatchIngestMode, CapabilityStatusV1, EmbeddingDistanceMetric, EmbeddingId,
    EmbeddingModelContract, EmbeddingNormalization, EmbeddingRecord, ManifestGeneration,
    OwnerDocKind, RepoId, RepoRelativePath, RevisionId, SearchPlaneTrackKind, SearchScopeKey,
    SearchScopeSurface, SemanticCorpusKindV1, SemanticIngestBatch, SemanticReplaceScope,
    SourceRoleV1, lex::LanguageCode,
};
use quanta_index_core::GenerationStorageKeyV1;
use quanta_index_core::domains::generation::GenerationQuarantineReasonV1;
use quanta_index_searchd::app::SearchdConfig;
use quanta_index_searchd::app::semantic_boot::SemanticMigrationOutcome;
use quanta_index_searchd_runtime::build_runtime;
use quanta_index_semantic::{SemanticAdapter, build_resident_batch_v1};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn socket_paths() -> (PathBuf, PathBuf, PathBuf) {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let dir = std::env::temp_dir();
    (
        dir.join(format!("qi-bootrep-q-{pid}-{nanos}.sock")),
        dir.join(format!("qi-bootrep-c-{pid}-{nanos}.sock")),
        dir.join(format!("qi-bootrep-i-{pid}-{nanos}.sock")),
    )
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "test asserts the assembled boot report via assert macros"
)]
fn fresh_runtime_exposes_empty_semantic_boot_report() -> TestResult {
    let temp = quanta_index_searchd_harness::private_tempdir()?;
    let (query_socket, control_socket, ingest_socket) = socket_paths();
    let mut config = SearchdConfig::from_state_root(temp.path().to_path_buf())
        .try_with_search_corpus_history_retention_limits(
            8,
            16 * 1024 * 1024,
            128,
            256 * 1024 * 1024,
        )
        .expect("valid test retention policy");
    config = SearchdConfig::with_socket_overrides(config, query_socket, control_socket);
    config = SearchdConfig::with_ingest_socket_override(config, ingest_socket);

    let runtime = build_runtime(config)?;

    // No legacy journal and no durable generations on a fresh state root, so the
    // boot path reports a direct (no-migration) open with zero seeded generations.
    assert_eq!(runtime.semantic_boot.migration, SemanticMigrationOutcome::NoLegacyJournal);
    assert_eq!(runtime.semantic_boot.seed.sealed_generations, 0);
    Ok(())
}

fn fixture_batch(generation: ManifestGeneration) -> Result<SemanticIngestBatch, String> {
    let language =
        LanguageCode::new("rust").map_err(|err| format!("fixture language invalid: {err}"))?;
    Ok(SemanticIngestBatch {
        repo_id: RepoId::new("repo-bootrep").expect("static fixture ID satisfies canonical policy"),
        revision_id: RevisionId::new("rev-bootrep")
            .expect("static fixture ID satisfies canonical policy"),
        generation,
        base_generation: None,
        manifest_digest: format!("manifest:{}", generation.get()),
        batch_digest: format!("batch:{}", generation.get()),
        mode: BatchIngestMode::ReplaceGeneration,
        model_contract: EmbeddingModelContract {
            model_id: "text-embed".to_string().into_boxed_str(),
            model_version: Some("1".to_string().into_boxed_str()),
            dimension: 3,
            normalization: EmbeddingNormalization::L2Unit,
            distance_metric: EmbeddingDistanceMetric::Cosine,
            policy_digest: "policy:bootrep".to_string().into_boxed_str(),
            view_policy_digest: None,
        },
        required_corpora: vec![SemanticCorpusKindV1::SymbolCard],
        corpus_policy_digest: Some("policy:bootrep".to_string()),
        clear_surfaces: Vec::new(),
        replace_scopes: vec![SemanticReplaceScope {
            scope: SearchScopeKey {
                doc_surface: SearchScopeSurface::Chunk,
                repo_relative_path: RepoRelativePath::new("x.rs"),
            },
            scope_digest: "scope:x.rs".to_string(),
            embeddings: vec![EmbeddingRecord {
                embedding_id: EmbeddingId::new("emb-1"),
                record_id: "record-1".to_string().into_boxed_str(),
                owner_kind: OwnerDocKind::Chunk,
                owner_id: "owner-1".to_string().into_boxed_str(),
                corpus_kind: SemanticCorpusKindV1::SymbolCard,
                parent_owner_id: None,
                source_doc_id: "doc-1".to_string().into_boxed_str(),
                repo_relative_path: RepoRelativePath::new("x.rs"),
                language,
                package: None,
                symbol_kind: None,
                visibility: None,
                source_role: SourceRoleV1::CardText,
                generated: false,
                capability_status: CapabilityStatusV1::Full,
                authority_digest: "auth:1".to_string().into_boxed_str(),
                render_policy_digest: "render:1".to_string().into_boxed_str(),
                card_schema_version: 1,
                start_byte: 0,
                end_byte: 8,
                start_line: 1,
                end_line: 2,
                snippet: "fn x() {}".to_string().into_boxed_str(),
                embedding_input_digest: "in:1".to_string().into_boxed_str(),
                vector_digest: "vec:1".to_string().into_boxed_str(),
                view_kind: "raw_chunk".to_string().into_boxed_str(),
                vector: vec![1.0_f32, 0.0, 0.0],
            }],
            cluster_memberships: Vec::new(),
        }],
        tombstone_scopes: Vec::new(),
        seal: true,
    })
}

fn build_config(state_root: &Path) -> SearchdConfig {
    let (query_socket, control_socket, ingest_socket) = socket_paths();
    let mut config = SearchdConfig::from_state_root(state_root.to_path_buf())
        .try_with_search_corpus_history_retention_limits(
            8,
            16 * 1024 * 1024,
            128,
            256 * 1024 * 1024,
        )
        .expect("valid test retention policy");
    config = SearchdConfig::with_socket_overrides(config, query_socket, control_socket);
    SearchdConfig::with_ingest_socket_override(config, ingest_socket)
}

/// A corrupted generation nothing activates and nothing retains is an
/// orphan, not fatal and not seeded.
///
/// Boot proves the active set only (QI-BB-026), and this state root has
/// none. The generation was built straight through the adapter, so the
/// durable search-corpus authority never recorded it: boot lists it as an
/// orphan (QI-BB-003) without looking at its content, seeds nothing, and a
/// pin to it answers `UNKNOWN_GENERATION`. The door that opens a retained
/// corrupt generation refuses it; `e2e_boot_quarantine` proves that end to
/// end.
#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "test asserts the assembled boot report on an inactive corrupted generation via assert macros"
)]
fn runtime_boot_inventories_a_corrupted_inactive_semantic_generation() -> TestResult {
    let temp = quanta_index_searchd_harness::private_tempdir()?;
    let state_root = temp.path().to_path_buf();
    let semantic_root = state_root.join("indexes").join("semantic");
    let adapter = SemanticAdapter::with_state_root(semantic_root.clone())?;
    let generation = ManifestGeneration::new(9);
    build_resident_batch_v1(&adapter, &fixture_batch(generation)?)?;

    let manifest_path = GenerationStorageKeyV1::for_repo_revision(
        &RepoId::new("repo-bootrep").expect("static fixture ID satisfies canonical policy"),
        &RevisionId::new("rev-bootrep").expect("static fixture ID satisfies canonical policy"),
    )
    .generation_dir(&semantic_root, generation)
    .join("semantic-manifest.cbor");
    let manifest_bytes = std::fs::read(&manifest_path)?;
    let mut value: ciborium::value::Value = ciborium::from_reader(&manifest_bytes[..])
        .map_err(|err| format!("decode manifest cbor: {err}"))?;
    let mut bumped = false;
    if let ciborium::value::Value::Map(entries) = &mut value {
        for (key, val) in entries.iter_mut() {
            if key.as_text() == Some("row_count") {
                *val = ciborium::value::Value::Integer(ciborium::value::Integer::from(99_u64));
                bumped = true;
                break;
            }
        }
    }
    if !bumped {
        return Err("expected `row_count` field in manifest CBOR".into());
    }
    let mut tampered = Vec::new();
    ciborium::into_writer(&value, &mut tampered)
        .map_err(|err| format!("encode manifest cbor: {err}"))?;
    std::fs::write(&manifest_path, &tampered)?;

    let runtime = build_runtime(build_config(&state_root))?;
    assert_eq!(runtime.semantic_boot.seed.sealed_generations, 0);
    assert_eq!(runtime.semantic_boot.seed.quarantined_generations, 0);
    assert_eq!(runtime.boot_inventory.semantic.sealed_generations, 0);
    assert!(runtime.boot_inventory.semantic.quarantined.is_empty());
    // The runtime names directories under the canonical state root.
    let generation_dir = std::fs::canonicalize(
        manifest_path
            .parent()
            .ok_or("the manifest lives in its generation directory")?,
    )?;
    let orphaned: Vec<(SearchPlaneTrackKind, &Path, GenerationQuarantineReasonV1)> = runtime
        .boot_inventory
        .semantic
        .orphaned
        .iter()
        .map(|entry| (entry.track, entry.path.as_path(), entry.reason))
        .collect();
    assert_eq!(
        orphaned,
        vec![(
            SearchPlaneTrackKind::Semantic,
            generation_dir.as_path(),
            GenerationQuarantineReasonV1::Orphaned
        )]
    );
    assert_eq!(
        runtime.boot_inventory.active_pairs_validated, 0,
        "nothing is active, so boot proves nothing"
    );
    Ok(())
}
