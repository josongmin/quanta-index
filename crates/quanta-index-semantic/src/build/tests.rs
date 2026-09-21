use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

use arrow_array::Array;
use futures::TryStreamExt as _;
use lancedb::query::ExecutableQuery as _;
use tempfile::tempdir;

use quanta_index_contract::{
    BatchIngestMode, CapabilityStatusV1, ClusterMembershipBatchReadRequestV1,
    ClusterMembershipReadFailureV1, ClusterMembershipReadOutcomeV1, ClusterMembershipReadRequestV1,
    ClusterMembershipReplaceV1, EmbeddingDistanceMetric, EmbeddingId, EmbeddingModelContract,
    EmbeddingNormalization, EmbeddingRecord, GenerationPin, ManifestGeneration, OwnerDocKind,
    RepoId, RepoRelativePath, RevisionId, SearchScopeKey, SearchScopeSurface, SemanticCorpusKindV1,
    SemanticIngestBatch, SemanticReplaceScope, SemanticSourceScopeKeyV1, SemanticTombstoneScope,
    SourceRoleV1, SymbolId, lex::LanguageCode,
};
use quanta_index_core::{
    CoreError, RequestBudgetV1, ResidentScopeSource, SEMANTIC_STREAM_WINDOW_VECTOR_BYTES,
    SemanticIndexOpenPort, SemanticIngestHeaderV1, SemanticScopeSource as _,
    SemanticStreamWindowPolicy, build_resident_semantic_batch_v1,
};

use super::{
    BACKUP_DIR_NAME, POST_DATASET_PRE_CONTRACT_PROMOTION, PRE_DATASET_PROMOTION,
    PROMOTION_CRASH_BOUNDARY_ENV, PROMOTION_CRASH_EXIT_CODE, STAGING_DIR_NAME,
    StreamScopeAuthorityV1, build_stream, column_as, ensure_generation_contract, failpoint,
    open_connection, persist_generation_contract, recover_dataset_artifacts,
    stage_generation_contract,
};
use crate::budget::{DenseLaneBudgetV1, DenseLaneTalliesV1};
use crate::durable_write::{set_atomic_write_fail_before_rename_action, write_atomic};
use crate::generation_contract::GenerationContract;
use crate::integrity::SealTalliesV1;
use crate::layout::{
    self, CLUSTER_MEMBERSHIP_TABLE_NAME, COLUMN_AUTHORITY_DIGEST, COLUMN_CAPABILITY_STATUS,
    COLUMN_CARD_SCHEMA_VERSION, COLUMN_CORPUS_KIND, COLUMN_GENERATED, COLUMN_OWNER_ID,
    COLUMN_PACKAGE, COLUMN_PARENT_OWNER_ID, COLUMN_RECORD_ID, COLUMN_RENDER_POLICY_DIGEST,
    COLUMN_SOURCE_DOC_ID, COLUMN_SOURCE_ROLE, COLUMN_VISIBILITY, TABLE_NAME,
};
use crate::manifest::SemanticManifest;
use crate::search::open_generation;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const PROMOTION_CRASH_ROOT_ENV: &str = "QUANTA_INDEX_SEMANTIC_PROMOTION_CRASH_ROOT";

/// A budget that never interrupts and tallies nobody reads, for the
/// searches these build tests run to check what a seal serves.
fn unbounded_watch() -> DenseLaneBudgetV1<'static> {
    static BUDGET: std::sync::OnceLock<RequestBudgetV1> = std::sync::OnceLock::new();
    static TALLIES: DenseLaneTalliesV1 = DenseLaneTalliesV1::new();
    DenseLaneBudgetV1 {
        budget: BUDGET.get_or_init(RequestBudgetV1::unbounded),
        tallies: &TALLIES,
    }
}

fn repo_id() -> RepoId {
    RepoId::new("repo-build").expect("static fixture ID satisfies canonical policy")
}

fn revision_id() -> RevisionId {
    RevisionId::new("rev-build").expect("static fixture ID satisfies canonical policy")
}

fn model_contract() -> EmbeddingModelContract {
    EmbeddingModelContract {
        model_id: "test-model".to_string().into_boxed_str(),
        model_version: Some("1".to_string().into_boxed_str()),
        dimension: 3,
        normalization: EmbeddingNormalization::L2Unit,
        distance_metric: EmbeddingDistanceMetric::Cosine,
        policy_digest: "policy:test".to_string().into_boxed_str(),
        view_policy_digest: None,
    }
}

fn scope(path: &str) -> SearchScopeKey {
    SearchScopeKey {
        doc_surface: SearchScopeSurface::Chunk,
        repo_relative_path: RepoRelativePath::new(path),
    }
}

fn semantic_scope(
    corpus_kind: SemanticCorpusKindV1,
    owner_kind: OwnerDocKind,
    owner_id: &str,
) -> SemanticSourceScopeKeyV1 {
    SemanticSourceScopeKeyV1 {
        corpus_kind,
        owner_kind,
        owner_id: owner_id.to_string(),
    }
}

fn embedding(
    id: &str,
    path: &str,
    owner_kind: OwnerDocKind,
    owner_id: &str,
    corpus_kind: SemanticCorpusKindV1,
    vector: Vec<f32>,
) -> Result<EmbeddingRecord, String> {
    let source_role = match corpus_kind {
        SemanticCorpusKindV1::RawCodeFallback => SourceRoleV1::RawFallbackText,
        SemanticCorpusKindV1::DocumentSummary => SourceRoleV1::SummaryText,
        SemanticCorpusKindV1::DocumentLeaf | SemanticCorpusKindV1::DocumentSection => {
            SourceRoleV1::DocumentText
        }
        SemanticCorpusKindV1::SymbolCard
        | SemanticCorpusKindV1::ModuleCard
        | SemanticCorpusKindV1::ClusterCard
        | SemanticCorpusKindV1::TestBehavior
        | SemanticCorpusKindV1::RepositorySummary => SourceRoleV1::CardText,
    };
    let capability_status = if corpus_kind == SemanticCorpusKindV1::RawCodeFallback {
        CapabilityStatusV1::Degraded
    } else {
        CapabilityStatusV1::Full
    };
    Ok(EmbeddingRecord {
        embedding_id: EmbeddingId::new(id),
        record_id: format!("record-{id}").into_boxed_str(),
        owner_kind,
        owner_id: owner_id.to_string().into_boxed_str(),
        corpus_kind,
        parent_owner_id: (corpus_kind == SemanticCorpusKindV1::RawCodeFallback)
            .then(|| owner_id.to_string().into_boxed_str()),
        source_doc_id: format!("doc-{id}").into_boxed_str(),
        repo_relative_path: RepoRelativePath::new(path),
        language: LanguageCode::new("rust").map_err(std::string::ToString::to_string)?,
        package: Some("crate".to_string().into_boxed_str()),
        symbol_kind: None,
        visibility: Some("pub".to_string().into_boxed_str()),
        source_role,
        generated: false,
        capability_status,
        authority_digest: format!("auth:{id}").into_boxed_str(),
        render_policy_digest: format!("render:{id}").into_boxed_str(),
        card_schema_version: u32::from(corpus_kind != SemanticCorpusKindV1::RawCodeFallback),
        start_byte: 0,
        end_byte: 8,
        start_line: 1,
        end_line: 1,
        snippet: format!("fn {id}() {{}}").into_boxed_str(),
        embedding_input_digest: format!("input:{id}").into_boxed_str(),
        vector_digest: format!("vector:{id}").into_boxed_str(),
        view_kind: "raw_chunk".to_string().into_boxed_str(),
        vector,
    })
}

fn batch(
    generation: ManifestGeneration,
    path: &str,
    id: &str,
    vector: Vec<f32>,
    seal: bool,
) -> Result<SemanticIngestBatch, String> {
    Ok(SemanticIngestBatch {
        repo_id: repo_id(),
        revision_id: revision_id(),
        generation,
        base_generation: None,
        manifest_digest: format!("manifest:{}", generation.get()),
        batch_digest: format!("batch:{}:{path}", generation.get()),
        mode: BatchIngestMode::ReplaceGeneration,
        model_contract: model_contract(),
        required_corpora: Vec::new(),
        corpus_policy_digest: None,
        clear_surfaces: Vec::new(),
        replace_scopes: vec![SemanticReplaceScope {
            scope: scope(path),
            scope_digest: format!("scope:{path}"),
            embeddings: vec![embedding(
                id,
                path,
                OwnerDocKind::Chunk,
                &format!("owner-{id}"),
                SemanticCorpusKindV1::RawCodeFallback,
                vector,
            )?],
            cluster_memberships: Vec::new(),
        }],
        tombstone_scopes: Vec::new(),
        seal,
    })
}

fn promotion_batch(
    generation: ManifestGeneration,
    id: &str,
    owner_kind: OwnerDocKind,
    owner_id: &str,
    corpus_kind: SemanticCorpusKindV1,
    required_corpora: Vec<SemanticCorpusKindV1>,
    vector: Vec<f32>,
) -> Result<SemanticIngestBatch, String> {
    let mut batch = batch(generation, "src/promotion.rs", id, vector, false)?;
    let record = &mut batch.replace_scopes[0].embeddings[0];
    record.owner_kind = owner_kind;
    record.owner_id = owner_id.to_string().into_boxed_str();
    record.corpus_kind = corpus_kind;
    record.source_role = match corpus_kind {
        SemanticCorpusKindV1::RawCodeFallback => SourceRoleV1::RawFallbackText,
        SemanticCorpusKindV1::DocumentSummary => SourceRoleV1::SummaryText,
        SemanticCorpusKindV1::DocumentLeaf | SemanticCorpusKindV1::DocumentSection => {
            SourceRoleV1::DocumentText
        }
        SemanticCorpusKindV1::SymbolCard
        | SemanticCorpusKindV1::ModuleCard
        | SemanticCorpusKindV1::ClusterCard
        | SemanticCorpusKindV1::TestBehavior
        | SemanticCorpusKindV1::RepositorySummary => SourceRoleV1::CardText,
    };
    record.capability_status = if corpus_kind == SemanticCorpusKindV1::RawCodeFallback {
        CapabilityStatusV1::Degraded
    } else {
        CapabilityStatusV1::Full
    };
    record.parent_owner_id = None;
    record.card_schema_version = u32::from(corpus_kind != SemanticCorpusKindV1::RawCodeFallback);
    batch.required_corpora = required_corpora;
    batch.corpus_policy_digest = Some("semantic-source.v1".to_string());
    Ok(batch)
}

fn seal_existing_generation_batch(mut batch: SemanticIngestBatch) -> SemanticIngestBatch {
    batch.batch_digest = format!("{}:seal", batch.batch_digest);
    batch.required_corpora.clear();
    batch.replace_scopes.clear();
    batch.tombstone_scopes.clear();
    batch.seal = true;
    batch
}

fn run_promotion_crash_child(root: &Path, boundary: &str) -> TestResult {
    if boundary != PRE_DATASET_PROMOTION && boundary != POST_DATASET_PRE_CONTRACT_PROMOTION {
        return Err(format!("unknown promotion crash boundary {boundary}").into());
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let generation = ManifestGeneration::new(90);
    let module_batch = promotion_batch(
        generation,
        "module-after-crash",
        OwnerDocKind::Module,
        "module:after-crash",
        SemanticCorpusKindV1::ModuleCard,
        vec![SemanticCorpusKindV1::ModuleCard],
        vec![0.0, 1.0, 0.0],
    )?;
    let result = build(&runtime, root, &module_batch);
    match result {
        Ok(()) => Err("promotion child returned without abrupt exit".into()),
        Err(err) => {
            Err(format!("promotion child returned error instead of abrupt exit: {err}").into())
        }
    }
}

fn child_root_from_env() -> Option<PathBuf> {
    env::var_os(PROMOTION_CRASH_ROOT_ENV).map(PathBuf::from)
}

/// Build an already-resident batch through the streamed entry under the
/// production window policy.
fn build(
    runtime: &tokio::runtime::Runtime,
    root: &Path,
    batch: &SemanticIngestBatch,
) -> Result<(), CoreError> {
    let header = SemanticIngestHeaderV1::of_batch(batch);
    let mut source =
        ResidentScopeSource::new(&batch.replace_scopes, SemanticStreamWindowPolicy::DEFAULT)?;
    let _tally = build_stream(
        runtime,
        root,
        SemanticStreamWindowPolicy::DEFAULT,
        &header,
        &mut source,
        &SealTalliesV1::default(),
    )?;
    Ok(())
}

/// Build through the adapter's port, discarding the tally.
fn build_with(
    adapter: &crate::SemanticAdapter,
    batch: &SemanticIngestBatch,
) -> Result<(), CoreError> {
    let _tally =
        build_resident_semantic_batch_v1(adapter, batch, SemanticStreamWindowPolicy::DEFAULT)?;
    Ok(())
}

/// Admit every replace scope of `batch` through the streaming authority.
fn admit_scope_authority(batch: &SemanticIngestBatch) -> Result<(), CoreError> {
    let header = SemanticIngestHeaderV1::of_batch(batch);
    let mut authority = StreamScopeAuthorityV1::new(&header)?;
    for scope in &batch.replace_scopes {
        authority.admit_replace_scope(scope)?;
    }
    Ok(())
}

fn assert_recovered_promotion_state(
    root: &Path,
    boundary: &str,
    base_batch: SemanticIngestBatch,
) -> TestResult {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let generation = base_batch.generation;
    let generation_dir = layout::generation_dir(root, &repo_id(), &revision_id(), generation);
    let contract_path = layout::build_contract_path(&generation_dir);
    let staged_contract_path = contract_path.with_extension("next");

    let before_recovery = GenerationContract::decode(&std::fs::read(&contract_path)?)?;
    assert_eq!(before_recovery.required_corpora, vec!["SymbolCard"]);
    assert!(staged_contract_path.exists(), "crash must leave staged contract");
    match boundary {
        PRE_DATASET_PROMOTION => {
            assert!(
                generation_dir.join(STAGING_DIR_NAME).exists(),
                "pre-dataset crash must leave unpromoted staging dataset"
            );
            assert!(
                !generation_dir.join(BACKUP_DIR_NAME).exists(),
                "pre-dataset crash must not create backup dataset"
            );
        }
        POST_DATASET_PRE_CONTRACT_PROMOTION => {
            assert!(
                !generation_dir.join(STAGING_DIR_NAME).exists(),
                "post-dataset crash must have consumed staging dataset"
            );
            assert!(
                generation_dir.join(BACKUP_DIR_NAME).exists(),
                "post-dataset crash must preserve prior dataset as backup"
            );
        }
        other => return Err(format!("unknown promotion crash boundary {other}").into()),
    }

    let seal_batch = seal_existing_generation_batch(base_batch);
    build(&runtime, root, &seal_batch)?;

    let recovered = GenerationContract::decode(&std::fs::read(&contract_path)?)?;
    let expected_corpora = match boundary {
        PRE_DATASET_PROMOTION => vec!["SymbolCard"],
        POST_DATASET_PRE_CONTRACT_PROMOTION => vec!["ModuleCard", "SymbolCard"],
        other => return Err(format!("unknown promotion crash boundary {other}").into()),
    };
    assert_eq!(recovered.required_corpora, expected_corpora);
    assert!(!staged_contract_path.exists());
    assert!(!generation_dir.join(STAGING_DIR_NAME).exists());
    assert!(!generation_dir.join(BACKUP_DIR_NAME).exists());

    let loaded = crate::run_blocking(
        &runtime,
        open_generation(root, &repo_id(), &revision_id(), generation),
    )?;
    let symbol_hits = crate::run_blocking(
        &runtime,
        loaded.search_hits_filtered_async(
            &[1.0, 0.0, 0.0],
            10,
            Some("SymbolCard"),
            unbounded_watch(),
        ),
    )?;
    let module_hits = crate::run_blocking(
        &runtime,
        loaded.search_hits_filtered_async(
            &[0.0, 1.0, 0.0],
            10,
            Some("ModuleCard"),
            unbounded_watch(),
        ),
    )?;
    assert_eq!(symbol_hits.len(), 1);
    assert_eq!(
        module_hits.len(),
        usize::from(boundary == POST_DATASET_PRE_CONTRACT_PROMOTION),
        "recovered dataset must match the contract selected by recovery"
    );
    Ok(())
}

fn atomic_temporary_paths(
    parent: &Path,
    target: &Path,
) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let file_name = target
        .file_name()
        .ok_or("atomic-write test target has no file name")?
        .to_string_lossy();
    let prefix = format!(".{file_name}.tmp-");
    let mut paths = Vec::new();
    for entry in std::fs::read_dir(parent)? {
        let entry = entry?;
        if entry.file_name().to_string_lossy().starts_with(&prefix) {
            paths.push(entry.path());
        }
    }
    Ok(paths)
}

#[test]
fn atomic_sidecar_write_persists_payload_without_staging_residue() -> TestResult {
    let temp = tempdir()?;
    let target = temp.path().join("manifest.cbor");

    write_atomic(&target, b"durable-manifest", "test durable manifest")?;

    assert_eq!(std::fs::read(&target)?, b"durable-manifest");
    assert!(atomic_temporary_paths(temp.path(), &target)?.is_empty());
    Ok(())
}

#[test]
fn atomic_sidecar_write_cleans_staging_on_pre_rename_failure() -> TestResult {
    const ACTION: &str = "test injected durable manifest";

    let temp = tempdir()?;
    let target = temp.path().join("manifest.cbor");
    set_atomic_write_fail_before_rename_action(Some(ACTION));
    let result = write_atomic(&target, b"must-not-promote", ACTION);
    set_atomic_write_fail_before_rename_action(None);

    assert!(result.is_err());
    assert!(!target.exists());
    assert!(atomic_temporary_paths(temp.path(), &target)?.is_empty());
    Ok(())
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "test asserts subprocess crash recovery invariants"
)]
fn subprocess_crash_during_generation_promotion_recovers_complete_pair() -> TestResult {
    if let Some(root) = child_root_from_env() {
        let boundary = env::var(PROMOTION_CRASH_BOUNDARY_ENV)
            .map_err(|err| format!("promotion crash child missing boundary: {err}"))?;
        return run_promotion_crash_child(&root, &boundary);
    }

    for boundary in [PRE_DATASET_PROMOTION, POST_DATASET_PRE_CONTRACT_PROMOTION] {
        let temp = tempdir()?;
        let root = temp.path().to_path_buf();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let base_batch = promotion_batch(
            ManifestGeneration::new(90),
            "symbol-before-crash",
            OwnerDocKind::Symbol,
            "symbol:before-crash",
            SemanticCorpusKindV1::SymbolCard,
            vec![SemanticCorpusKindV1::SymbolCard],
            vec![1.0, 0.0, 0.0],
        )?;
        build(&runtime, &root, &base_batch)?;

        let test_name =
            "build::tests::subprocess_crash_during_generation_promotion_recovers_complete_pair";
        let status = Command::new(env::current_exe()?)
            .arg("--exact")
            .arg(test_name)
            .arg("--nocapture")
            .env(PROMOTION_CRASH_ROOT_ENV, &root)
            .env(PROMOTION_CRASH_BOUNDARY_ENV, boundary)
            .status()?;
        assert_eq!(
            status.code(),
            Some(PROMOTION_CRASH_EXIT_CODE),
            "child must stop exactly at {boundary}; status={status}"
        );

        assert_recovered_promotion_state(&root, boundary, base_batch)?;
    }
    Ok(())
}

#[test]
fn semantic_scope_authority_allows_distinct_owners_on_same_path() -> TestResult {
    let generation = ManifestGeneration::new(45);
    let mut batch = batch(generation, "src/shared.rs", "symbol-a", vec![1.0, 0.0, 0.0], false)?;
    batch.replace_scopes.push(SemanticReplaceScope {
        scope: scope("src/shared.rs"),
        scope_digest: "scope:src/shared.rs:module-b".to_string(),
        embeddings: vec![embedding(
            "module-b",
            "src/shared.rs",
            OwnerDocKind::Module,
            "module-b",
            SemanticCorpusKindV1::ModuleCard,
            vec![0.0, 1.0, 0.0],
        )?],
        cluster_memberships: Vec::new(),
    });

    admit_scope_authority(&batch)?;

    batch.replace_scopes.push(SemanticReplaceScope {
        scope: scope("src/other.rs"),
        scope_digest: "scope:src/other.rs:symbol-a".to_string(),
        embeddings: vec![embedding(
            "symbol-a-duplicate",
            "src/other.rs",
            OwnerDocKind::Chunk,
            "owner-symbol-a",
            SemanticCorpusKindV1::RawCodeFallback,
            vec![0.0, 0.0, 1.0],
        )?],
        cluster_memberships: Vec::new(),
    });
    let duplicate_owner = admit_scope_authority(&batch)
        .expect_err("duplicate semantic owner authority must remain rejected");
    assert!(matches!(
        duplicate_owner,
        CoreError::InvalidContract(message)
            if message.contains("duplicate replace semantic scope")
    ));
    Ok(())
}

#[test]
fn generation_contract_accumulates_corpora_across_mutation_and_seal_batches() -> TestResult {
    let temp = tempdir()?;
    let generation_dir = temp.path().join("generation");
    let generation = ManifestGeneration::new(46);
    let mut symbol_batch =
        batch(generation, "src/shared.rs", "symbol-a", vec![1.0, 0.0, 0.0], false)?;
    symbol_batch.required_corpora = vec![SemanticCorpusKindV1::SymbolCard];
    symbol_batch.corpus_policy_digest = Some("semantic-source.v1".to_string());
    let symbol_contract = ensure_generation_contract(
        &generation_dir,
        &SemanticIngestHeaderV1::of_batch(&symbol_batch),
    )?;
    assert_eq!(symbol_contract.required_corpora, vec!["SymbolCard"]);
    assert!(!layout::build_contract_path(&generation_dir).exists());
    persist_generation_contract(&generation_dir, &symbol_contract)?;

    let mut module_batch =
        batch(generation, "src/shared.rs", "module-b", vec![0.0, 1.0, 0.0], false)?;
    module_batch.replace_scopes[0].embeddings[0].owner_kind = OwnerDocKind::Module;
    module_batch.replace_scopes[0].embeddings[0].owner_id = "module-b".into();
    module_batch.replace_scopes[0].embeddings[0].corpus_kind = SemanticCorpusKindV1::ModuleCard;
    module_batch.required_corpora = vec![SemanticCorpusKindV1::ModuleCard];
    module_batch.corpus_policy_digest = Some("semantic-source.v1".to_string());
    let merged_contract = ensure_generation_contract(
        &generation_dir,
        &SemanticIngestHeaderV1::of_batch(&module_batch),
    )?;
    assert_eq!(merged_contract.required_corpora, vec!["ModuleCard", "SymbolCard"]);
    let still_symbol_only =
        GenerationContract::decode(&std::fs::read(layout::build_contract_path(&generation_dir))?)?;
    assert_eq!(still_symbol_only.required_corpora, vec!["SymbolCard"]);
    persist_generation_contract(&generation_dir, &merged_contract)?;

    let mut seal_batch = module_batch;
    seal_batch.batch_digest = "batch:46:seal".to_string();
    seal_batch.required_corpora.clear();
    seal_batch.replace_scopes.clear();
    seal_batch.seal = true;
    let sealed_contract = ensure_generation_contract(
        &generation_dir,
        &SemanticIngestHeaderV1::of_batch(&seal_batch),
    )?;
    assert_eq!(sealed_contract.required_corpora, merged_contract.required_corpora);
    persist_generation_contract(&generation_dir, &sealed_contract)?;

    let persisted =
        GenerationContract::decode(&std::fs::read(layout::build_contract_path(&generation_dir))?)?;
    assert_eq!(persisted.required_corpora, sealed_contract.required_corpora);
    Ok(())
}

#[test]
fn generation_contract_recovery_distinguishes_pre_and_post_dataset_promotion() -> TestResult {
    let temp = tempdir()?;
    let generation_dir = temp.path().join("generation");
    std::fs::create_dir_all(layout::dataset_dir(&generation_dir))?;
    let generation = ManifestGeneration::new(49);
    let mut symbol_batch =
        batch(generation, "src/recovery.rs", "symbol-a", vec![1.0, 0.0, 0.0], false)?;
    symbol_batch.required_corpora = vec![SemanticCorpusKindV1::SymbolCard];
    symbol_batch.corpus_policy_digest = Some("semantic-source.v1".to_string());
    let symbol_contract =
        GenerationContract::from_batch(&SemanticIngestHeaderV1::of_batch(&symbol_batch).contract);
    persist_generation_contract(&generation_dir, &symbol_contract)?;

    let mut module_batch = symbol_batch;
    module_batch.required_corpora = vec![SemanticCorpusKindV1::ModuleCard];
    let merged_contract =
        symbol_contract.merge_batch(&SemanticIngestHeaderV1::of_batch(&module_batch).contract)?;
    let staged_path = stage_generation_contract(&generation_dir, &merged_contract)?;
    recover_dataset_artifacts(&generation_dir)?;
    assert!(!staged_path.exists());
    let pre_promotion_recovered =
        GenerationContract::decode(&std::fs::read(layout::build_contract_path(&generation_dir))?)?;
    assert_eq!(pre_promotion_recovered.required_corpora, vec!["SymbolCard"]);

    let staged_path = stage_generation_contract(&generation_dir, &merged_contract)?;
    std::fs::create_dir_all(generation_dir.join(BACKUP_DIR_NAME))?;
    recover_dataset_artifacts(&generation_dir)?;
    assert!(!staged_path.exists());
    assert!(!generation_dir.join(BACKUP_DIR_NAME).exists());
    let post_promotion_recovered =
        GenerationContract::decode(&std::fs::read(layout::build_contract_path(&generation_dir))?)?;
    assert_eq!(post_promotion_recovered.required_corpora, vec!["ModuleCard", "SymbolCard"]);
    Ok(())
}

#[test]
fn failed_append_does_not_advance_generation_corpus_policy() -> TestResult {
    let temp = tempdir()?;
    let root = temp.path().to_path_buf();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let generation = ManifestGeneration::new(47);
    let mut symbol_batch =
        batch(generation, "src/policy.rs", "symbol-a", vec![1.0, 0.0, 0.0], false)?;
    symbol_batch.replace_scopes[0].embeddings[0].owner_kind = OwnerDocKind::Symbol;
    symbol_batch.replace_scopes[0].embeddings[0].owner_id = "symbol-a".into();
    symbol_batch.replace_scopes[0].embeddings[0].corpus_kind = SemanticCorpusKindV1::SymbolCard;
    symbol_batch.required_corpora = vec![SemanticCorpusKindV1::SymbolCard];
    symbol_batch.corpus_policy_digest = Some("semantic-source.v1".to_string());
    build(&runtime, &root, &symbol_batch)?;

    let mut module_batch =
        batch(generation, "src/policy.rs", "module-b", vec![0.0, 1.0, 0.0], false)?;
    module_batch.replace_scopes[0].embeddings[0].owner_kind = OwnerDocKind::Module;
    module_batch.replace_scopes[0].embeddings[0].owner_id = "module-b".into();
    module_batch.replace_scopes[0].embeddings[0].corpus_kind = SemanticCorpusKindV1::ModuleCard;
    module_batch.required_corpora = vec![SemanticCorpusKindV1::ModuleCard];
    module_batch.corpus_policy_digest = Some("semantic-source.v1".to_string());
    failpoint::set_append_fail_path(Some("src/policy.rs"));
    let failure = build(&runtime, &root, &module_batch);
    failpoint::clear_append_fail_path("src/policy.rs");
    assert!(matches!(failure, Err(CoreError::Storage(_))));

    let generation_dir = layout::generation_dir(&root, &repo_id(), &revision_id(), generation);
    let persisted =
        GenerationContract::decode(&std::fs::read(layout::build_contract_path(&generation_dir))?)?;
    assert_eq!(persisted.required_corpora, vec!["SymbolCard"]);
    Ok(())
}

#[test]
fn failed_contract_promotion_rolls_back_dataset_and_policy() -> TestResult {
    let temp = tempdir()?;
    let root = temp.path().to_path_buf();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let generation = ManifestGeneration::new(48);
    let generation_dir = layout::generation_dir(&root, &repo_id(), &revision_id(), generation);
    let mut symbol_batch =
        batch(generation, "src/transaction.rs", "symbol-a", vec![1.0, 0.0, 0.0], false)?;
    symbol_batch.replace_scopes[0].embeddings[0].owner_kind = OwnerDocKind::Symbol;
    symbol_batch.replace_scopes[0].embeddings[0].owner_id = "symbol-a".into();
    symbol_batch.replace_scopes[0].embeddings[0].corpus_kind = SemanticCorpusKindV1::SymbolCard;
    symbol_batch.required_corpora = vec![SemanticCorpusKindV1::SymbolCard];
    symbol_batch.corpus_policy_digest = Some("semantic-source.v1".to_string());
    build(&runtime, &root, &symbol_batch)?;

    let mut module_batch =
        batch(generation, "src/transaction.rs", "module-b", vec![0.0, 1.0, 0.0], false)?;
    module_batch.replace_scopes[0].embeddings[0].owner_kind = OwnerDocKind::Module;
    module_batch.replace_scopes[0].embeddings[0].owner_id = "module-b".into();
    module_batch.replace_scopes[0].embeddings[0].corpus_kind = SemanticCorpusKindV1::ModuleCard;
    module_batch.required_corpora = vec![SemanticCorpusKindV1::ModuleCard];
    module_batch.corpus_policy_digest = Some("semantic-source.v1".to_string());
    let generation_dir_text = generation_dir.to_string_lossy().into_owned();
    failpoint::set_contract_promotion_fail_dir(Some(generation_dir_text.as_str()));
    let failure = build(&runtime, &root, &module_batch);
    failpoint::set_contract_promotion_fail_dir(None);
    assert!(matches!(failure, Err(CoreError::Storage(_))));

    let persisted =
        GenerationContract::decode(&std::fs::read(layout::build_contract_path(&generation_dir))?)?;
    assert_eq!(persisted.required_corpora, vec!["SymbolCard"]);
    assert!(
        !layout::build_contract_path(&generation_dir)
            .with_extension("next")
            .exists()
    );

    let mut seal_batch = symbol_batch;
    seal_batch.batch_digest = "batch:48:seal".to_string();
    seal_batch.required_corpora.clear();
    seal_batch.replace_scopes.clear();
    seal_batch.seal = true;
    build(&runtime, &root, &seal_batch)?;
    let loaded = crate::run_blocking(
        &runtime,
        open_generation(&root, &repo_id(), &revision_id(), generation),
    )?;
    let module_hits = crate::run_blocking(
        &runtime,
        loaded.search_hits_filtered_async(
            &[0.0, 1.0, 0.0],
            10,
            Some("ModuleCard"),
            unbounded_watch(),
        ),
    )?;
    assert!(
        module_hits.is_empty(),
        "failed contract promotion must roll back appended ModuleCard rows: {module_hits:?}"
    );
    Ok(())
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "test asserts append-failure rollback via assert macros"
)]
fn failed_append_does_not_delete_prior_rows() -> TestResult {
    let temp = tempdir()?;
    let root = temp.path().to_path_buf();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let generation = ManifestGeneration::new(1);

    build(
        &runtime,
        &root,
        &batch(generation, "a.rs", "emb-1", vec![1.0, 0.0, 0.0], false)?,
    )?;

    failpoint::set_append_fail_path(Some("a.rs"));
    let Err(err) = build(
        &runtime,
        &root,
        &batch(generation, "a.rs", "emb-2", vec![0.0, 1.0, 0.0], false)?,
    ) else {
        return Err("injected append failure must surface".into());
    };
    failpoint::clear_append_fail_path("a.rs");
    assert!(matches!(err, CoreError::Storage(_)));

    build(
        &runtime,
        &root,
        &SemanticIngestBatch {
            repo_id: repo_id(),
            revision_id: revision_id(),
            generation,
            base_generation: None,
            manifest_digest: "manifest:1".to_string(),
            batch_digest: "batch:1:seal".to_string(),
            mode: BatchIngestMode::ReplaceGeneration,
            model_contract: model_contract(),
            required_corpora: Vec::new(),
            corpus_policy_digest: None,
            clear_surfaces: Vec::new(),
            replace_scopes: Vec::new(),
            tombstone_scopes: Vec::new(),
            seal: true,
        },
    )?;

    let loaded = crate::run_blocking(
        &runtime,
        open_generation(&root, &repo_id(), &revision_id(), generation),
    )?;
    let hits =
        crate::run_blocking(&runtime, loaded.search_async(&[1.0, 0.0, 0.0], 5, unbounded_watch()))?;
    let ids: Vec<String> = hits
        .into_iter()
        .map(|candidate| candidate.candidate_id)
        .collect();
    assert!(
        ids.contains(&"emb-1".to_string()),
        "append failure must preserve prior rows; got {ids:?}"
    );
    Ok(())
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "test asserts v4 metadata preservation via exact storage checks"
)]
fn scv2_02_v4_round_trip_preserves_metadata_fields() -> TestResult {
    let temp = tempdir()?;
    let root = temp.path().to_path_buf();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let generation = ManifestGeneration::new(41);
    let batch = SemanticIngestBatch {
        repo_id: repo_id(),
        revision_id: revision_id(),
        generation,
        base_generation: None,
        manifest_digest: "manifest:41".to_string(),
        batch_digest: "batch:41".to_string(),
        mode: BatchIngestMode::ReplaceGeneration,
        model_contract: model_contract(),
        required_corpora: vec![SemanticCorpusKindV1::SymbolCard],
        corpus_policy_digest: Some("policy:semantic:v1".to_string()),
        clear_surfaces: Vec::new(),
        replace_scopes: vec![SemanticReplaceScope {
            scope: scope("src/lib.rs"),
            scope_digest: "scope:src/lib.rs".to_string(),
            embeddings: vec![embedding(
                "meta-1",
                "src/lib.rs",
                OwnerDocKind::Symbol,
                "symbol-1",
                SemanticCorpusKindV1::SymbolCard,
                vec![1.0, 0.0, 0.0],
            )?],
            cluster_memberships: Vec::new(),
        }],
        tombstone_scopes: Vec::new(),
        seal: true,
    };
    build(&runtime, &root, &batch)?;

    let generation_dir = layout::generation_dir(&root, &repo_id(), &revision_id(), generation);
    let manifest =
        SemanticManifest::decode(&std::fs::read(layout::manifest_path(&generation_dir))?)?;
    assert_eq!(manifest.present_corpora, vec!["SymbolCard".to_string()]);
    assert_eq!(manifest.required_corpora, vec!["SymbolCard".to_string()]);
    assert_eq!(manifest.card_schema_versions, vec![1]);
    assert_eq!(manifest.render_policy_digests, vec!["render:meta-1".to_string()]);
    assert_eq!(manifest.corpus_policy_digest, Some("policy:semantic:v1".to_string()));

    let connection =
        crate::run_blocking(&runtime, open_connection(&layout::dataset_dir(&generation_dir)))?;
    let table = crate::run_blocking(&runtime, connection.open_table(TABLE_NAME).execute())?;
    let stream = crate::run_blocking(&runtime, table.query().execute())?;
    let batches: Vec<arrow_array::RecordBatch> =
        crate::run_blocking(&runtime, stream.try_collect())?;
    let batch = batches
        .into_iter()
        .next()
        .ok_or("expected one result batch")?;
    let record_id_col = column_as::<arrow_array::StringArray>(&batch, COLUMN_RECORD_ID, "Utf8")?;
    let owner_id_col = column_as::<arrow_array::StringArray>(&batch, COLUMN_OWNER_ID, "Utf8")?;
    let corpus_kind_col =
        column_as::<arrow_array::StringArray>(&batch, COLUMN_CORPUS_KIND, "Utf8")?;
    let parent_owner_col =
        column_as::<arrow_array::StringArray>(&batch, COLUMN_PARENT_OWNER_ID, "Utf8")?;
    let source_doc_col =
        column_as::<arrow_array::StringArray>(&batch, COLUMN_SOURCE_DOC_ID, "Utf8")?;
    let package_col = column_as::<arrow_array::StringArray>(&batch, COLUMN_PACKAGE, "Utf8")?;
    let visibility_col = column_as::<arrow_array::StringArray>(&batch, COLUMN_VISIBILITY, "Utf8")?;
    let source_role_col =
        column_as::<arrow_array::StringArray>(&batch, COLUMN_SOURCE_ROLE, "Utf8")?;
    let generated_col =
        column_as::<arrow_array::BooleanArray>(&batch, COLUMN_GENERATED, "Boolean")?;
    let capability_col =
        column_as::<arrow_array::StringArray>(&batch, COLUMN_CAPABILITY_STATUS, "Utf8")?;
    let authority_col =
        column_as::<arrow_array::StringArray>(&batch, COLUMN_AUTHORITY_DIGEST, "Utf8")?;
    let render_col =
        column_as::<arrow_array::StringArray>(&batch, COLUMN_RENDER_POLICY_DIGEST, "Utf8")?;
    let schema_col =
        column_as::<arrow_array::UInt32Array>(&batch, COLUMN_CARD_SCHEMA_VERSION, "UInt32")?;
    assert_eq!(record_id_col.value(0), "record-meta-1");
    assert_eq!(owner_id_col.value(0), "symbol-1");
    assert_eq!(corpus_kind_col.value(0), "SymbolCard");
    assert!(parent_owner_col.is_null(0));
    assert_eq!(source_doc_col.value(0), "doc-meta-1");
    assert_eq!(package_col.value(0), "crate");
    assert_eq!(visibility_col.value(0), "pub");
    assert_eq!(source_role_col.value(0), "CardText");
    assert!(!generated_col.value(0));
    assert_eq!(capability_col.value(0), "Full");
    assert_eq!(authority_col.value(0), "auth:meta-1");
    assert_eq!(render_col.value(0), "render:meta-1");
    assert_eq!(schema_col.value(0), 1);
    Ok(())
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "test asserts semantic-scope tombstone isolation via exact search assertions"
)]
fn scv2_02_same_path_delete_one_owner_keeps_other() -> TestResult {
    let temp = tempdir()?;
    let root = temp.path().to_path_buf();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let generation = ManifestGeneration::new(42);
    let seed = SemanticIngestBatch {
        repo_id: repo_id(),
        revision_id: revision_id(),
        generation,
        base_generation: None,
        manifest_digest: "manifest:42a".to_string(),
        batch_digest: "batch:42a".to_string(),
        mode: BatchIngestMode::ReplaceGeneration,
        model_contract: model_contract(),
        required_corpora: Vec::new(),
        corpus_policy_digest: None,
        clear_surfaces: Vec::new(),
        replace_scopes: vec![SemanticReplaceScope {
            scope: scope("src/shared.rs"),
            scope_digest: "scope:src/shared.rs".to_string(),
            embeddings: vec![
                embedding(
                    "symbol-a",
                    "src/shared.rs",
                    OwnerDocKind::Symbol,
                    "symbol-a",
                    SemanticCorpusKindV1::SymbolCard,
                    vec![1.0, 0.0, 0.0],
                )?,
                embedding(
                    "module-b",
                    "src/shared.rs",
                    OwnerDocKind::Module,
                    "module-b",
                    SemanticCorpusKindV1::ModuleCard,
                    vec![0.0, 1.0, 0.0],
                )?,
            ],
            cluster_memberships: Vec::new(),
        }],
        tombstone_scopes: Vec::new(),
        seal: false,
    };
    build(&runtime, &root, &seed)?;
    let delete_one = SemanticIngestBatch {
        repo_id: repo_id(),
        revision_id: revision_id(),
        generation,
        base_generation: None,
        manifest_digest: "manifest:42b".to_string(),
        batch_digest: "batch:42b".to_string(),
        mode: BatchIngestMode::ReplaceGeneration,
        model_contract: model_contract(),
        required_corpora: Vec::new(),
        corpus_policy_digest: None,
        clear_surfaces: Vec::new(),
        replace_scopes: Vec::new(),
        tombstone_scopes: vec![SemanticTombstoneScope {
            scope: None,
            semantic_scope: Some(semantic_scope(
                SemanticCorpusKindV1::SymbolCard,
                OwnerDocKind::Symbol,
                "symbol-a",
            )),
        }],
        seal: true,
    };
    build(&runtime, &root, &delete_one)?;
    let loaded = crate::run_blocking(
        &runtime,
        open_generation(&root, &repo_id(), &revision_id(), generation),
    )?;
    let removed = crate::run_blocking(
        &runtime,
        loaded.search_hits_filtered_async(
            &[1.0, 0.0, 0.0],
            10,
            Some("SymbolCard"),
            unbounded_watch(),
        ),
    )?;
    assert!(removed.is_empty(), "deleted semantic scope must be gone: {removed:?}");
    let kept = crate::run_blocking(
        &runtime,
        loaded.search_hits_filtered_async(
            &[0.0, 1.0, 0.0],
            10,
            Some("ModuleCard"),
            unbounded_watch(),
        ),
    )?;
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].record_id, "record-module-b");
    assert_eq!(kept[0].owner_id, "module-b");
    Ok(())
}

#[test]
fn clear_symbol_surface_removes_exact_and_fallback_rows_only_v1() -> TestResult {
    let temp = tempdir()?;
    let root = temp.path().to_path_buf();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let generation = ManifestGeneration::new(52);
    let seed = SemanticIngestBatch {
        repo_id: repo_id(),
        revision_id: revision_id(),
        generation,
        base_generation: None,
        manifest_digest: "manifest:52a".to_string(),
        batch_digest: "batch:52a".to_string(),
        mode: BatchIngestMode::ReplaceGeneration,
        model_contract: model_contract(),
        required_corpora: Vec::new(),
        corpus_policy_digest: None,
        clear_surfaces: Vec::new(),
        replace_scopes: vec![SemanticReplaceScope {
            scope: scope("src/shared.rs"),
            scope_digest: "scope:src/shared.rs".to_string(),
            embeddings: vec![
                embedding(
                    "symbol-exact",
                    "src/shared.rs",
                    OwnerDocKind::Symbol,
                    "symbol-exact",
                    SemanticCorpusKindV1::SymbolCard,
                    vec![1.0, 0.0, 0.0],
                )?,
                embedding(
                    "symbol-fallback",
                    "src/shared.rs",
                    OwnerDocKind::Callsite,
                    "callsite-fallback",
                    SemanticCorpusKindV1::RawCodeFallback,
                    vec![0.6, 0.8, 0.0],
                )?,
                embedding(
                    "module-kept",
                    "src/shared.rs",
                    OwnerDocKind::Module,
                    "module-kept",
                    SemanticCorpusKindV1::ModuleCard,
                    vec![0.0, 1.0, 0.0],
                )?,
            ],
            cluster_memberships: Vec::new(),
        }],
        tombstone_scopes: Vec::new(),
        seal: false,
    };
    build(&runtime, &root, &seed)?;

    let clear = SemanticIngestBatch {
        repo_id: repo_id(),
        revision_id: revision_id(),
        generation,
        base_generation: None,
        manifest_digest: "manifest:52b".to_string(),
        batch_digest: "batch:52b".to_string(),
        mode: BatchIngestMode::ReplaceGeneration,
        model_contract: model_contract(),
        required_corpora: Vec::new(),
        corpus_policy_digest: None,
        clear_surfaces: vec![SearchScopeSurface::Symbol],
        replace_scopes: Vec::new(),
        tombstone_scopes: Vec::new(),
        seal: true,
    };
    build(&runtime, &root, &clear)?;

    let loaded = crate::run_blocking(
        &runtime,
        open_generation(&root, &repo_id(), &revision_id(), generation),
    )?;
    let exact_symbol = crate::run_blocking(
        &runtime,
        loaded.search_hits_filtered_async(
            &[1.0, 0.0, 0.0],
            10,
            Some("SymbolCard"),
            unbounded_watch(),
        ),
    )?;
    let fallback_symbol = crate::run_blocking(
        &runtime,
        loaded.search_hits_filtered_async(
            // The direction (0.9, 0.1, 0) at unit norm: the generation is
            // `L2Unit`, so its queries are held to the unit contract.
            &[0.993_883_7, 0.110_431_5, 0.0],
            10,
            Some("RawCodeFallback"),
            unbounded_watch(),
        ),
    )?;
    let module = crate::run_blocking(
        &runtime,
        loaded.search_hits_filtered_async(
            &[0.0, 1.0, 0.0],
            10,
            Some("ModuleCard"),
            unbounded_watch(),
        ),
    )?;
    assert!(exact_symbol.is_empty());
    assert!(fallback_symbol.is_empty());
    assert_eq!(module.len(), 1);
    assert_eq!(module[0].record_id, "record-module-kept");
    Ok(())
}

#[test]
fn delta_empty_seal_preserves_required_raw_corpus_coverage() -> TestResult {
    let temp = tempdir()?;
    let root = temp.path().to_path_buf();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let base_generation = ManifestGeneration::new(51);
    let delta_generation = ManifestGeneration::new(52);

    let mut base = batch(base_generation, "src/base.rs", "base-raw", vec![1.0, 0.0, 0.0], false)?;
    base.required_corpora = vec![SemanticCorpusKindV1::RawCodeFallback];
    build(&runtime, &root, &base)?;
    build(&runtime, &root, &seal_existing_generation_batch(base))?;

    let mut delta =
        batch(delta_generation, "src/base.rs", "delta-raw", vec![0.0, 1.0, 0.0], false)?;
    delta.mode = BatchIngestMode::Delta;
    delta.base_generation = Some(base_generation);
    delta.required_corpora = vec![SemanticCorpusKindV1::RawCodeFallback];
    build(&runtime, &root, &delta)?;
    build(&runtime, &root, &seal_existing_generation_batch(delta))?;

    let generation_dir =
        layout::generation_dir(&root, &repo_id(), &revision_id(), delta_generation);
    let manifest =
        SemanticManifest::decode(&std::fs::read(layout::manifest_path(&generation_dir))?)?;
    assert_eq!(manifest.required_corpora, vec!["RawCodeFallback"]);
    assert_eq!(manifest.present_corpora, vec!["RawCodeFallback"]);
    Ok(())
}

#[test]
fn scv2_02_missing_required_corpus_fails_seal() -> TestResult {
    let temp = tempdir()?;
    let root = temp.path().to_path_buf();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let generation = ManifestGeneration::new(43);
    let batch = SemanticIngestBatch {
        repo_id: repo_id(),
        revision_id: revision_id(),
        generation,
        base_generation: None,
        manifest_digest: "manifest:43".to_string(),
        batch_digest: "batch:43".to_string(),
        mode: BatchIngestMode::ReplaceGeneration,
        model_contract: model_contract(),
        required_corpora: vec![
            SemanticCorpusKindV1::SymbolCard,
            SemanticCorpusKindV1::ModuleCard,
        ],
        corpus_policy_digest: Some("policy:semantic:v1".to_string()),
        clear_surfaces: Vec::new(),
        replace_scopes: vec![SemanticReplaceScope {
            scope: scope("src/missing.rs"),
            scope_digest: "scope:src/missing.rs".to_string(),
            embeddings: vec![embedding(
                "missing-1",
                "src/missing.rs",
                OwnerDocKind::Symbol,
                "symbol-missing",
                SemanticCorpusKindV1::SymbolCard,
                vec![1.0, 0.0, 0.0],
            )?],
            cluster_memberships: Vec::new(),
        }],
        tombstone_scopes: Vec::new(),
        seal: true,
    };
    let err = build(&runtime, &root, &batch).expect_err("missing required corpus must fail seal");
    match err {
        CoreError::Storage(message) => assert!(
            message.contains("required corpus `ModuleCard` missing"),
            "seal failure must name the missing corpus, got {message}"
        ),
        other @ (CoreError::InvalidContract(_)
        | CoreError::Typed { .. }
        | CoreError::NotReady(_)
        | CoreError::NotImplemented(_)
        | CoreError::NotFound(_)) => {
            panic!("expected storage error for missing required corpus, got {other:?}")
        }
    }
    Ok(())
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "test asserts corpus filter excludes other corpora via exact hit metadata"
)]
fn scv2_02_filtered_search_excludes_other_corpora() -> TestResult {
    let temp = tempdir()?;
    let root = temp.path().to_path_buf();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let generation = ManifestGeneration::new(44);
    let batch = SemanticIngestBatch {
        repo_id: repo_id(),
        revision_id: revision_id(),
        generation,
        base_generation: None,
        manifest_digest: "manifest:44".to_string(),
        batch_digest: "batch:44".to_string(),
        mode: BatchIngestMode::ReplaceGeneration,
        model_contract: model_contract(),
        required_corpora: vec![
            SemanticCorpusKindV1::SymbolCard,
            SemanticCorpusKindV1::ModuleCard,
        ],
        corpus_policy_digest: Some("policy:semantic:v1".to_string()),
        clear_surfaces: Vec::new(),
        replace_scopes: vec![SemanticReplaceScope {
            scope: scope("src/filter.rs"),
            scope_digest: "scope:src/filter.rs".to_string(),
            embeddings: vec![
                embedding(
                    "filter-symbol",
                    "src/filter.rs",
                    OwnerDocKind::Symbol,
                    "symbol-filter",
                    SemanticCorpusKindV1::SymbolCard,
                    vec![1.0, 0.0, 0.0],
                )?,
                embedding(
                    "filter-module",
                    "src/filter.rs",
                    OwnerDocKind::Module,
                    "module-filter",
                    SemanticCorpusKindV1::ModuleCard,
                    vec![0.0, 1.0, 0.0],
                )?,
            ],
            cluster_memberships: Vec::new(),
        }],
        tombstone_scopes: Vec::new(),
        seal: true,
    };
    build(&runtime, &root, &batch)?;
    let loaded = crate::run_blocking(
        &runtime,
        open_generation(&root, &repo_id(), &revision_id(), generation),
    )?;
    let symbol_hits = crate::run_blocking(
        &runtime,
        loaded.search_hits_filtered_async(
            &[1.0, 0.0, 0.0],
            10,
            Some("SymbolCard"),
            unbounded_watch(),
        ),
    )?;
    assert_eq!(symbol_hits.len(), 1);
    assert_eq!(symbol_hits[0].record_id, "record-filter-symbol");
    assert_eq!(symbol_hits[0].owner_id, "symbol-filter");
    assert_eq!(symbol_hits[0].corpus_kind, "SymbolCard");
    let module_hits = crate::run_blocking(
        &runtime,
        loaded.search_hits_filtered_async(
            &[0.0, 1.0, 0.0],
            10,
            Some("ModuleCard"),
            unbounded_watch(),
        ),
    )?;
    assert_eq!(module_hits.len(), 1);
    assert_eq!(module_hits[0].record_id, "record-filter-module");
    assert_eq!(module_hits[0].owner_id, "module-filter");
    assert_eq!(module_hits[0].corpus_kind, "ModuleCard");
    Ok(())
}

#[test]
fn cluster_membership_same_seal_replace_base_clone_and_tombstone_v1() -> TestResult {
    let temp = tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = crate::SemanticAdapter::with_state_root(root.clone())?;
    let base_generation = ManifestGeneration::new(71);
    let replacement_generation = ManifestGeneration::new(72);
    let tombstone_generation = ManifestGeneration::new(73);
    let cluster_scope = SemanticSourceScopeKeyV1 {
        corpus_kind: SemanticCorpusKindV1::ClusterCard,
        owner_kind: OwnerDocKind::Module,
        owner_id: "cluster-owner".to_string(),
    };
    let cluster_batch = |generation: ManifestGeneration,
                         base_generation: Option<ManifestGeneration>,
                         id: &str,
                         members: Vec<SymbolId>|
     -> Result<SemanticIngestBatch, String> {
        Ok(SemanticIngestBatch {
            repo_id: repo_id(),
            revision_id: revision_id(),
            generation,
            base_generation,
            manifest_digest: format!("manifest:{}", generation.get()),
            batch_digest: format!("batch:{}", generation.get()),
            mode: if base_generation.is_some() {
                BatchIngestMode::Delta
            } else {
                BatchIngestMode::ReplaceGeneration
            },
            model_contract: model_contract(),
            required_corpora: vec![SemanticCorpusKindV1::ClusterCard],
            corpus_policy_digest: Some("policy:cluster:v1".to_string()),
            clear_surfaces: Vec::new(),
            replace_scopes: vec![SemanticReplaceScope {
                scope: scope("src/cluster.rs"),
                scope_digest: format!("scope:{id}"),
                embeddings: vec![embedding(
                    id,
                    "src/cluster.rs",
                    OwnerDocKind::Module,
                    "cluster-owner",
                    SemanticCorpusKindV1::ClusterCard,
                    vec![1.0, 0.0, 0.0],
                )?],
                cluster_memberships: vec![ClusterMembershipReplaceV1 {
                    cluster_record_id: format!("record-{id}"),
                    authority_digest: format!("auth:{id}"),
                    members,
                }],
            }],
            tombstone_scopes: Vec::new(),
            seal: true,
        })
    };
    build_with(
        &adapter,
        &cluster_batch(
            base_generation,
            None,
            "cluster-a",
            vec![SymbolId::new("symbol:a"), SymbolId::new("symbol:b")],
        )?,
    )?;
    build_with(
        &adapter,
        &cluster_batch(
            replacement_generation,
            Some(base_generation),
            "cluster-b",
            vec![SymbolId::new("symbol:c"), SymbolId::new("symbol:d")],
        )?,
    )?;

    let replacement_searcher = adapter.open(&repo_id(), &revision_id(), replacement_generation)?;
    let available = replacement_searcher
        .cluster_membership_batch_read(&ClusterMembershipBatchReadRequestV1::single_v1(
            ClusterMembershipReadRequestV1 {
                cluster_record_id: "record-cluster-b".to_string(),
                generation: GenerationPin::new(repo_id(), revision_id(), replacement_generation),
                expected_authority_digest: "auth:cluster-b".to_string(),
                limit: 1,
            },
        ))?
        .outcomes
        .into_iter()
        .next()
        .ok_or_else(|| {
            CoreError::InvalidContract("single membership batch returned no outcome".to_string())
        })?;
    assert!(matches!(
        available,
        ClusterMembershipReadOutcomeV1::Available(snapshot)
            if snapshot.members == [SymbolId::new("symbol:c")]
                && snapshot.completeness
                    == quanta_index_contract::ClusterMembershipCompletenessV1::Truncated
    ));
    let replaced = replacement_searcher
        .cluster_membership_batch_read(&ClusterMembershipBatchReadRequestV1::single_v1(
            ClusterMembershipReadRequestV1 {
                cluster_record_id: "record-cluster-a".to_string(),
                generation: GenerationPin::new(repo_id(), revision_id(), replacement_generation),
                expected_authority_digest: "auth:cluster-a".to_string(),
                limit: 2,
            },
        ))?
        .outcomes
        .into_iter()
        .next()
        .ok_or_else(|| {
            CoreError::InvalidContract("single membership batch returned no outcome".to_string())
        })?;
    assert!(matches!(
        replaced,
        ClusterMembershipReadOutcomeV1::Rejected(rejection)
            if rejection.failure
                == ClusterMembershipReadFailureV1::CurrentGenerationMissing
    ));

    build_with(
        &adapter,
        &SemanticIngestBatch {
            repo_id: repo_id(),
            revision_id: revision_id(),
            generation: tombstone_generation,
            base_generation: Some(replacement_generation),
            manifest_digest: "manifest:73".to_string(),
            batch_digest: "batch:73".to_string(),
            mode: BatchIngestMode::Delta,
            model_contract: model_contract(),
            required_corpora: Vec::new(),
            corpus_policy_digest: None,
            clear_surfaces: Vec::new(),
            replace_scopes: Vec::new(),
            tombstone_scopes: vec![SemanticTombstoneScope {
                scope: None,
                semantic_scope: Some(cluster_scope),
            }],
            seal: true,
        },
    )?;
    let tombstoned = adapter
        .open(&repo_id(), &revision_id(), tombstone_generation)?
        .cluster_membership_batch_read(&ClusterMembershipBatchReadRequestV1::single_v1(
            ClusterMembershipReadRequestV1 {
                cluster_record_id: "record-cluster-b".to_string(),
                generation: GenerationPin::new(repo_id(), revision_id(), tombstone_generation),
                expected_authority_digest: "auth:cluster-b".to_string(),
                limit: 2,
            },
        ))?
        .outcomes
        .into_iter()
        .next()
        .ok_or_else(|| {
            CoreError::InvalidContract("single membership batch returned no outcome".to_string())
        })?;
    assert!(matches!(
        tombstoned,
        ClusterMembershipReadOutcomeV1::Rejected(rejection)
            if rejection.failure
                == ClusterMembershipReadFailureV1::CurrentGenerationMissing
    ));

    drop(replacement_searcher);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let replacement_dir =
        layout::generation_dir(&root, &repo_id(), &revision_id(), replacement_generation);
    let connection =
        crate::run_blocking(&runtime, open_connection(&layout::dataset_dir(&replacement_dir)))?;
    let membership_table = crate::run_blocking(
        &runtime,
        connection
            .open_table(CLUSTER_MEMBERSHIP_TABLE_NAME)
            .execute(),
    )?;
    let _delete_result =
        crate::run_blocking(&runtime, membership_table.delete("member_symbol_id = 'symbol:d'"))?;
    assert!(
        crate::run_blocking(
            &runtime,
            open_generation(&root, &repo_id(), &revision_id(), replacement_generation,),
        )
        .is_err(),
        "cold open must reject a membership row deleted after seal"
    );

    let base_dir = layout::generation_dir(&root, &repo_id(), &revision_id(), base_generation);
    let base_connection =
        crate::run_blocking(&runtime, open_connection(&layout::dataset_dir(&base_dir)))?;
    crate::run_blocking(&runtime, base_connection.drop_table(CLUSTER_MEMBERSHIP_TABLE_NAME, &[]))?;
    assert!(
        crate::run_blocking(
            &runtime,
            open_generation(&root, &repo_id(), &revision_id(), base_generation,),
        )
        .is_err(),
        "cold open must reject a deleted membership sidecar table"
    );
    Ok(())
}

/// A same-cardinality content mutation after the seal is a new dataset
/// version on disk; the sealed manifest's file commitment refuses it at
/// cold open (QI-BB-017) without re-deriving the row root.
#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "test asserts same-cardinality content tampering is rejected at cold open"
)]
fn sealed_manifest_rejects_same_row_count_content_mutation() -> TestResult {
    let temp = tempdir()?;
    let root = temp.path().to_path_buf();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let generation = ManifestGeneration::new(701);
    build(
        &runtime,
        &root,
        &batch(generation, "src/root.rs", "emb-root", vec![1.0, 0.0, 0.0], true)?,
    )?;
    let generation_dir = layout::generation_dir(&root, &repo_id(), &revision_id(), generation);
    let connection =
        crate::run_blocking(&runtime, open_connection(&layout::dataset_dir(&generation_dir)))?;
    let table = crate::run_blocking(&runtime, connection.open_table(TABLE_NAME).execute())?;
    let update = crate::run_blocking(
        &runtime,
        table
            .update()
            .only_if("embedding_id = 'emb-root'")
            .column(layout::COLUMN_SNIPPET, "'fn tampered() {}'")
            .execute(),
    )?;
    assert_eq!(update.rows_updated, 1);

    let Err(error) = crate::run_blocking(
        &runtime,
        open_generation(&root, &repo_id(), &revision_id(), generation),
    ) else {
        return Err("same-row-count content mutation must fail closed".into());
    };
    assert!(
        matches!(
            error,
            CoreError::Typed { ref code, .. }
                if *code == quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt
        ),
        "{error:?}"
    );
    Ok(())
}
// ---- QI-BB-021 follow-up #2: scope-streamed build ----

/// `count` legacy path scopes of `chunks_per_scope` chunk owners each,
/// every row a distinct unit direction in the fixture's 3-space.
fn streamed_scopes(
    count: usize,
    chunks_per_scope: usize,
) -> Result<Vec<SemanticReplaceScope>, Box<dyn std::error::Error>> {
    let mut scopes = Vec::with_capacity(count);
    let mut step = 0_usize;
    for scope_index in 0..count {
        let path = format!("src/streamed_{scope_index:02}.rs");
        let mut embeddings = Vec::with_capacity(chunks_per_scope);
        for chunk_index in 0..chunks_per_scope {
            let id = format!("s{scope_index:02}c{chunk_index}");
            // 0.4 rad apart: distinct directions, all unit length.
            let angle = 0.4_f32 * f32::from(u16::try_from(step)?);
            let vector = vec![angle.cos(), angle.sin(), 0.0];
            step = step.saturating_add(1);
            embeddings.push(embedding(
                &id,
                &path,
                OwnerDocKind::Chunk,
                &format!("owner-{id}"),
                SemanticCorpusKindV1::RawCodeFallback,
                vector,
            )?);
        }
        scopes.push(SemanticReplaceScope {
            scope: scope(&path),
            scope_digest: format!("scope:{path}"),
            embeddings,
            cluster_memberships: Vec::new(),
        });
    }
    Ok(scopes)
}

fn streamed_batch(
    generation: ManifestGeneration,
    scopes: Vec<SemanticReplaceScope>,
    seal: bool,
) -> SemanticIngestBatch {
    SemanticIngestBatch {
        repo_id: repo_id(),
        revision_id: revision_id(),
        generation,
        base_generation: None,
        manifest_digest: format!("manifest:{}", generation.get()),
        batch_digest: format!("batch:{}:streamed:{seal}", generation.get()),
        mode: BatchIngestMode::ReplaceGeneration,
        model_contract: model_contract(),
        required_corpora: vec![SemanticCorpusKindV1::RawCodeFallback],
        corpus_policy_digest: None,
        clear_surfaces: Vec::new(),
        replace_scopes: scopes,
        tombstone_scopes: Vec::new(),
        seal,
    }
}

/// The canonical row commitment, recomputed from the fixture's own rows.
///
/// The same leaf and root framing `semantic_row_integrity_v1` documents,
/// computed from the records the test built rather than read from the
/// table, so the sealed root equals it only if the streamed appends
/// landed exactly those rows.
fn independent_row_root(records: &[&EmbeddingRecord]) -> String {
    use sha2::{Digest as _, Sha256};
    fn bytes(hasher: &mut Sha256, value: &[u8]) {
        hasher.update(
            u64::try_from(value.len())
                .map_or(u64::MAX, |len| len)
                .to_le_bytes(),
        );
        hasher.update(value);
    }
    fn optional(hasher: &mut Sha256, value: Option<&str>) {
        match value {
            Some(value) => {
                hasher.update([1]);
                bytes(hasher, value.as_bytes());
            }
            None => hasher.update([0]),
        }
    }
    let mut leaves: Vec<(String, String, [u8; 32])> = records
        .iter()
        .map(|record| {
            let mut leaf = Sha256::new();
            leaf.update(b"quanta-index-semantic-row-leaf-v1\0");
            bytes(&mut leaf, record.record_id.as_bytes());
            bytes(&mut leaf, record.embedding_id.as_str().as_bytes());
            for required in [
                record.repo_relative_path.as_str(),
                record.owner_id.as_ref(),
                record.owner_kind.as_code_str(),
                record.corpus_kind.as_code_str(),
                record.source_doc_id.as_ref(),
                record.language.as_str(),
                record.source_role.as_code_str(),
                record.capability_status.as_code_str(),
                record.authority_digest.as_ref(),
                record.render_policy_digest.as_ref(),
                record.embedding_input_digest.as_ref(),
                record.vector_digest.as_ref(),
                record.snippet.as_ref(),
            ] {
                optional(&mut leaf, Some(required));
            }
            optional(&mut leaf, record.parent_owner_id.as_deref());
            optional(&mut leaf, record.package.as_deref());
            optional(
                &mut leaf,
                record
                    .symbol_kind
                    .as_ref()
                    .map(quanta_index_contract::lex::SymbolKindCode::as_str),
            );
            optional(&mut leaf, record.visibility.as_deref());
            leaf.update([u8::from(record.generated)]);
            leaf.update(record.card_schema_version.to_le_bytes());
            leaf.update(record.start_line.to_le_bytes());
            leaf.update(record.end_line.to_le_bytes());
            leaf.update(
                u64::try_from(record.vector.len())
                    .map_or(u64::MAX, |len| len)
                    .to_le_bytes(),
            );
            for value in &record.vector {
                leaf.update(value.to_bits().to_le_bytes());
            }
            (
                record.record_id.to_string(),
                record.embedding_id.as_str().to_string(),
                leaf.finalize().into(),
            )
        })
        .collect();
    leaves.sort();
    let mut root = Sha256::new();
    root.update(b"quanta-index-semantic-row-root-v1\0");
    root.update(
        u64::try_from(leaves.len())
            .map_or(u64::MAX, |len| len)
            .to_le_bytes(),
    );
    for (_record_id, _embedding_id, leaf) in &leaves {
        root.update(leaf);
    }
    let digest = root.finalize();
    let mut hex = String::with_capacity(71);
    hex.push_str("sha256:");
    for byte in digest {
        use std::fmt::Write as _;
        let _written = write!(hex, "{byte:02x}");
    }
    hex
}

fn manifest_bytes_without_clock(
    root: &Path,
    generation: ManifestGeneration,
) -> Result<(SemanticManifest, Vec<u8>), Box<dyn std::error::Error>> {
    let generation_dir = layout::generation_dir(root, &repo_id(), &revision_id(), generation);
    let mut manifest =
        SemanticManifest::decode(&std::fs::read(layout::manifest_path(&generation_dir))?)?;
    manifest.built_at_unix_nanos = 0;
    let bytes = manifest.encode()?;
    Ok((manifest, bytes))
}

async fn promoted_row_count(
    root: &Path,
    generation: ManifestGeneration,
) -> Result<usize, CoreError> {
    let generation_dir = layout::generation_dir(root, &repo_id(), &revision_id(), generation);
    let connection = open_connection(&layout::dataset_dir(&generation_dir)).await?;
    let table = connection
        .open_table(TABLE_NAME)
        .execute()
        .await
        .map_err(|err| CoreError::Storage(format!("open promoted table: {err}")))?;
    table
        .count_rows(None)
        .await
        .map_err(|err| CoreError::Storage(format!("count promoted rows: {err}")))
}

// CASE-COVERS: a batch of N scopes streamed under a two-owner window
// never has more than one window (two rows of vectors) resident — the
// source's residency ledger is the instrument — and seals to a manifest
// byte-identical (but for the clock) to the one the same batch seals to
// in one window, whose row root equals a commitment recomputed here
// from the fixture's own rows. Every row is then served.
#[test]
fn streamed_windows_stay_bounded_and_seal_to_the_all_at_once_manifest() -> TestResult {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let generation = ManifestGeneration::new(811);
    let scopes = streamed_scopes(7, 2)?;
    let rows: Vec<&EmbeddingRecord> = scopes
        .iter()
        .flat_map(|scope| scope.embeddings.iter())
        .collect();
    let batch = streamed_batch(generation, scopes.clone(), true);
    let header = SemanticIngestHeaderV1::of_batch(&batch);
    let two_owners = SemanticStreamWindowPolicy::new(2, SEMANTIC_STREAM_WINDOW_VECTOR_BYTES)?;
    let two_rows = SemanticStreamWindowPolicy::vector_bytes(2, 3)?;

    // Streamed: seven windows of two chunk owners.
    let streamed_root = tempdir()?;
    let mut source = ResidentScopeSource::new(&batch.replace_scopes, two_owners)?;
    let residency = std::sync::Arc::clone(source.residency());
    let tally = build_stream(
        &runtime,
        streamed_root.path(),
        two_owners,
        &header,
        &mut source,
        &SealTalliesV1::default(),
    )?;
    assert_eq!(tally, source.tally(), "sink and source tallies agree");
    assert_eq!(tally.windows, 7);
    assert_eq!(tally.rows, 14);
    assert_eq!(tally.peak_vector_bytes, two_rows);
    assert_eq!(residency.peak_windows(), 1, "at most one window was resident at any time");
    assert_eq!(
        residency.peak_vector_bytes(),
        two_rows,
        "at most two rows of vectors were resident at any time"
    );
    assert_eq!(residency.outstanding_windows(), 0);

    // All at once: the same batch in one window.
    let whole_root = tempdir()?;
    let mut whole =
        ResidentScopeSource::new(&batch.replace_scopes, SemanticStreamWindowPolicy::DEFAULT)?;
    let whole_tally = build_stream(
        &runtime,
        whole_root.path(),
        SemanticStreamWindowPolicy::DEFAULT,
        &header,
        &mut whole,
        &SealTalliesV1::default(),
    )?;
    assert_eq!(whole_tally.windows, 1);
    assert_eq!(whole_tally.rows, 14);

    let (streamed_manifest, streamed_bytes) =
        manifest_bytes_without_clock(streamed_root.path(), generation)?;
    let (_whole_manifest, whole_bytes) =
        manifest_bytes_without_clock(whole_root.path(), generation)?;
    assert_eq!(
        streamed_bytes, whole_bytes,
        "the sealed manifest is byte-identical but for the clock"
    );
    assert_eq!(streamed_manifest.row_count, 14);
    assert_eq!(
        streamed_manifest.semantic_row_root_digest,
        independent_row_root(&rows),
        "the sealed row root is the commitment over exactly the fixture's rows"
    );

    // Every row is served from the streamed generation.
    let loaded = crate::run_blocking(
        &runtime,
        open_generation(streamed_root.path(), &repo_id(), &revision_id(), generation),
    )?;
    let hits = crate::run_blocking(
        &runtime,
        loaded.search_hits_filtered_async(&[1.0, 0.0, 0.0], 14, None, unbounded_watch()),
    )?;
    let mut served: Vec<String> = hits.into_iter().map(|hit| hit.record_id).collect();
    served.sort();
    let mut expected: Vec<String> = rows.iter().map(|row| row.record_id.to_string()).collect();
    expected.sort();
    assert_eq!(served, expected, "every streamed row is served");
    Ok(())
}

// CASE-COVERS: a window that fails the model contract aborts the build
// typed: the windows before it never reach the promoted dataset, nothing
// is sealed, and the next build of the generation starts from the rows
// that were promoted before, with the aborted staging copy discarded.
#[test]
fn a_refused_third_window_seals_nothing_and_leaves_no_partial_rows() -> TestResult {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let temp = tempdir()?;
    let root = temp.path().to_path_buf();
    let generation = ManifestGeneration::new(812);
    let generation_dir = layout::generation_dir(&root, &repo_id(), &revision_id(), generation);

    // Batch one: two owners, promoted unsealed.
    build(&runtime, &root, &streamed_batch(generation, streamed_scopes(2, 1)?, false))?;
    assert_eq!(crate::run_blocking(&runtime, promoted_row_count(&root, generation))?, 2);

    // Batch two: three more owners, the third breaking the L2Unit contract,
    // streamed one owner per window and sealing.
    let mut scopes = streamed_scopes(3, 1)?;
    for (index, replace_scope) in scopes.iter_mut().enumerate() {
        let path = format!("src/second_{index}.rs");
        replace_scope.scope = scope(&path);
        replace_scope.scope_digest = format!("scope:{path}");
        for record in &mut replace_scope.embeddings {
            record.embedding_id = EmbeddingId::new(format!("second-{index}"));
            record.record_id = format!("record-second-{index}").into_boxed_str();
            record.owner_id = format!("owner-second-{index}").into_boxed_str();
            record.repo_relative_path = RepoRelativePath::new(&path);
        }
    }
    scopes[2].embeddings[0].vector = vec![1.0, 1.0, 0.0];
    let sealing = streamed_batch(generation, scopes, true);
    let header = SemanticIngestHeaderV1::of_batch(&sealing);
    let one_owner = SemanticStreamWindowPolicy::new(1, SEMANTIC_STREAM_WINDOW_VECTOR_BYTES)?;
    let mut source = ResidentScopeSource::new(&sealing.replace_scopes, one_owner)?;
    let Err(error) =
        build_stream(&runtime, &root, one_owner, &header, &mut source, &SealTalliesV1::default())
    else {
        return Err("a window breaking the contract must abort the build".into());
    };
    assert!(
        matches!(&error, CoreError::Typed { message, .. } if message.contains("second-2")),
        "the refusal names the offending embedding: {error:?}"
    );
    assert_eq!(source.tally().windows, 3, "the third window was issued and refused");
    assert!(
        !layout::sealed_marker_path(&generation_dir).exists()
            && !layout::manifest_path(&generation_dir).exists(),
        "nothing is sealed"
    );
    assert_eq!(
        crate::run_blocking(&runtime, promoted_row_count(&root, generation))?,
        2,
        "the promoted dataset holds only the rows promoted before"
    );
    assert!(
        generation_dir.join(STAGING_DIR_NAME).exists(),
        "the aborted staging copy is left for the next recovery"
    );

    // The next build of the generation starts from the promoted rows and
    // discards the aborted copy; a valid seal serves exactly the promoted
    // rows plus its own.
    let mut third = streamed_scopes(1, 1)?;
    let path = "src/third.rs";
    third[0].scope = scope(path);
    third[0].scope_digest = format!("scope:{path}");
    third[0].embeddings[0].embedding_id = EmbeddingId::new("third-0");
    third[0].embeddings[0].record_id = "record-third-0".to_string().into_boxed_str();
    third[0].embeddings[0].owner_id = "owner-third-0".to_string().into_boxed_str();
    third[0].embeddings[0].repo_relative_path = RepoRelativePath::new(path);
    build(&runtime, &root, &streamed_batch(generation, third, true))?;
    assert!(!generation_dir.join(STAGING_DIR_NAME).exists());
    let loaded = crate::run_blocking(
        &runtime,
        open_generation(&root, &repo_id(), &revision_id(), generation),
    )?;
    let hits = crate::run_blocking(
        &runtime,
        loaded.search_hits_filtered_async(&[1.0, 0.0, 0.0], 10, None, unbounded_watch()),
    )?;
    let mut served: Vec<String> = hits.into_iter().map(|hit| hit.record_id).collect();
    served.sort();
    assert_eq!(
        served,
        vec![
            "record-s00c0".to_string(),
            "record-s01c0".to_string(),
            "record-third-0".to_string()
        ],
        "no row of the refused batch is visible"
    );
    Ok(())
}
