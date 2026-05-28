//! LDB-E2E-01 observability: the assembled runtime exposes a populated,
//! payload-free `SemanticBootReport` (migration outcome + seeded sealed-
//! generation count + cold-boot seed timing). This proves the boot path
//! surfaces direct-open vs migration, not just that the helpers compute it.

use std::path::{Path, PathBuf};

use quanta_index_contract::{
    BatchIngestMode, EmbeddingDistanceMetric, EmbeddingId, EmbeddingModelContract,
    EmbeddingNormalization, EmbeddingRecord, ManifestGeneration, OwnerDocKind, RepoId,
    RepoRelativePath, RevisionId, SearchScopeKey, SearchScopeSurface, SemanticIngestBatch,
    SemanticReplaceScope, lex::LanguageCode,
};
use quanta_index_search_plane::LegacySemanticJournalStore;
use quanta_index_searchd::app::SearchdConfig;
use quanta_index_searchd::app::semantic_boot::SemanticMigrationOutcome;
use quanta_index_searchd_runtime::build_runtime;

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
    let temp = tempfile::tempdir()?;
    let (query_socket, control_socket, ingest_socket) = socket_paths();
    let mut config = SearchdConfig::from_state_root(temp.path().to_path_buf());
    config = SearchdConfig::with_socket_overrides(config, query_socket, control_socket);
    config = SearchdConfig::with_ingest_socket_override(config, ingest_socket);

    let runtime = build_runtime(config)?;

    // No legacy journal and no durable generations on a fresh state root, so the
    // boot path reports a direct (no-migration) open with zero seeded generations.
    assert_eq!(
        runtime.semantic_boot.migration,
        SemanticMigrationOutcome::NoLegacyJournal
    );
    assert_eq!(runtime.semantic_boot.seed.sealed_generations, 0);
    Ok(())
}

fn fixture_batch(generation: ManifestGeneration) -> Result<SemanticIngestBatch, String> {
    let language =
        LanguageCode::new("rust").map_err(|err| format!("fixture language invalid: {err}"))?;
    Ok(SemanticIngestBatch {
        repo_id: RepoId::new("repo-bootrep"),
        revision_id: RevisionId::new("rev-bootrep"),
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
        replace_scopes: vec![SemanticReplaceScope {
            scope: SearchScopeKey {
                doc_surface: SearchScopeSurface::Chunk,
                repo_relative_path: RepoRelativePath::new("x.rs"),
            },
            scope_digest: "scope:x.rs".to_string(),
            embeddings: vec![EmbeddingRecord {
                embedding_id: EmbeddingId::new("emb-1"),
                owner_kind: OwnerDocKind::Chunk,
                owner_id: "owner-1".to_string().into_boxed_str(),
                source_doc_id: "doc-1".to_string().into_boxed_str(),
                repo_relative_path: RepoRelativePath::new("x.rs"),
                language,
                symbol_kind: None,
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
        }],
        tombstone_scopes: Vec::new(),
        seal: true,
    })
}

fn build_config(state_root: &Path) -> SearchdConfig {
    let (query_socket, control_socket, ingest_socket) = socket_paths();
    let mut config = SearchdConfig::from_state_root(state_root.to_path_buf());
    config = SearchdConfig::with_socket_overrides(config, query_socket, control_socket);
    SearchdConfig::with_ingest_socket_override(config, ingest_socket)
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "test asserts the migrated boot report end-to-end via assert macros"
)]
fn migrated_runtime_exposes_populated_semantic_boot_report() -> TestResult {
    let temp = tempfile::tempdir()?;
    let state_root = temp.path().to_path_buf();

    // Stage a legacy journal under `state_root/semantic/` *before* the runtime
    // boots; the assembled runtime must run the one-shot migration, write the
    // MIGRATED marker, seed readiness from the new durable generation, and
    // report all three via SemanticBootReport.
    let batch = fixture_batch(ManifestGeneration::new(1))?;
    LegacySemanticJournalStore::write_legacy_journal(state_root.join("semantic"), &[batch])?;

    let runtime = build_runtime(build_config(&state_root))?;

    assert_eq!(
        runtime.semantic_boot.migration,
        SemanticMigrationOutcome::Migrated { imported: 1 }
    );
    assert_eq!(runtime.semantic_boot.seed.sealed_generations, 1);
    Ok(())
}
