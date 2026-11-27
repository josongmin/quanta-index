//! Exercise the actual activation files and committed Catalog journal at the
//! restore-forward Source census boundary. No receipt or active head is mocked.

#![expect(
    clippy::panic_in_result_fn,
    reason = "the regression asserts the observed physical census and typed refusal"
)]

use super::*;
use std::{
    collections::BTreeSet,
    error::Error,
    fs,
    os::unix::fs::PermissionsExt as _,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use quanta_index_catalog::SqliteCatalog;
use quanta_index_contract::{
    FileId, GenerationSnapshot, IngestOperationKindV1, ManifestGeneration,
    RepoMapActivateGenerationRequestV2, RepoMapExactnessSummary, RepoMapFileNode,
    RepoMapGraphCoverage, RepoMapGraphCoverageClass, RepoMapItemIndexAvailability, RepoMapNode,
    RepoMapPublishBundleRequestV2, RepoMapRedactionState, RepoMapSourceBundle, RepoRelativePath,
    SearchPlaneTrackKind, SemanticContentRootsV1,
};
use quanta_index_core::{
    ClaimOutcomeV1, CoreError, IdempotencyCatalogPort, IdempotencyKeyV1, RequestBudgetV1,
    SealedGenerationBytesV1,
    domains::generation::RetentionMeasurementPolicy,
    index::{MemoryLeaseOwner, MemoryScope, MemoryScopePort, WorkLimits},
};
use quanta_index_search_plane::{
    SearchCorpusLifecycleOwner,
    readiness::{
        ActivationReadFailureV1, PreparedSearchCorpusGenerationV1, SearchCorpusGenerationV1,
        SearchCorpusHistoryRetentionPolicyV1, SearchCorpusIndexBytesPort,
        inspect_existing_activation_repository_v1,
    },
};
use quanta_index_searchd::app::runtime::StateRootLease;

use crate::{
    native_object_read_io::NativeObjectReadIo,
    publication_file_capacity::PublicationFileCapacity,
    publication_inventory::{PublicationBootInventoryLimits, seed_publication_capacity},
};

type TestResult<T = ()> = std::result::Result<T, Box<dyn Error>>;
const REPO: &str = "repo-source-high-water";
const REVISION: &str = "revision-source-high-water";

#[derive(Debug)]
struct FixtureIndexBytes;

impl SearchCorpusIndexBytesPort for FixtureIndexBytes {
    fn measurement_policy(&self) -> RetentionMeasurementPolicy {
        RetentionMeasurementPolicy::DEFAULT
    }

    fn measure_index_bytes(
        &self,
        _repo: &RepoId,
        _revision: &RevisionId,
        _generations: &BTreeSet<ManifestGeneration>,
        _budget: &RequestBudgetV1,
        _diagnostic: Option<&quanta_index_core::DiagnosticContextV1>,
    ) -> std::result::Result<SealedGenerationBytesV1, CoreError> {
        Ok(SealedGenerationBytesV1 {
            bytes: 10,
            absent: BTreeSet::new(),
        })
    }

    fn measure_state_root_index_bytes(
        &self,
        _budget: &RequestBudgetV1,
        _diagnostic: Option<&quanta_index_core::DiagnosticContextV1>,
    ) -> std::result::Result<Option<u64>, CoreError> {
        Ok(Some(10))
    }
}

#[derive(Clone, Default)]
struct FixtureMemory(Arc<AtomicU64>);

impl MemoryLeaseOwner for FixtureMemory {
    fn release(&self, bytes: u64) {
        assert!(
            self.0
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |live| live
                    .checked_sub(bytes))
                .is_ok(),
            "fixture released unreserved memory"
        );
    }
}

impl MemoryScopePort for FixtureMemory {
    fn reserve(
        &self,
        bytes: u64,
        _lifetime: MemoryLifetime,
    ) -> std::result::Result<MemoryLease, IndexError> {
        let _previous_live = self
            .0
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |live| {
                live.checked_add(bytes)
                    .filter(|next| *next <= 64 * 1024 * 1024)
            })
            .map_err(|_| IndexError::ResourceLimited)?;
        Ok(MemoryLease::new(bytes, Arc::new(self.clone())))
    }
}

fn read_budget() -> WorkBudget {
    let mut budget = WorkBudget::new(
        WorkLimits {
            reads: 100_000,
            writes: 0,
            bytes: 64 * 1024 * 1024,
            mutations: 0,
        },
        Instant::now() + Duration::from_secs(30),
    );
    budget.bind_memory_scope(Some(MemoryScope::new(Arc::new(FixtureMemory::default()))));
    budget
}

fn roots() -> SemanticContentRootsV1 {
    SemanticContentRootsV1 {
        row_root_digest: format!("sha256:{:064x}", 7),
        membership_root_digest: format!("sha256:{:064x}", 8),
    }
}

fn identity(generation: u64) -> TestResult<SearchCorpusGenerationV1> {
    identity_for(REVISION, generation)
}

fn identity_for(revision: &str, generation: u64) -> TestResult<SearchCorpusGenerationV1> {
    let repo = RepoId::new(REPO)?;
    let revision = RevisionId::new(revision)?;
    let snapshot = |track| GenerationSnapshot {
        repo_id: repo.clone(),
        revision_id: revision.clone(),
        track,
        manifest_generation: ManifestGeneration::new(generation),
        manifest_digest: "digest-seven".into(),
    };
    Ok(SearchCorpusGenerationV1::new(
        snapshot(SearchPlaneTrackKind::Lexical),
        snapshot(SearchPlaneTrackKind::Semantic),
        roots(),
    )?)
}

fn commit(catalog: &SqliteCatalog, generation: u64, digest: &str, sealed: bool) -> TestResult<u64> {
    commit_with_raw_receipt(catalog, generation, digest, sealed, |_| {})
}

fn commit_with_raw_receipt(
    catalog: &SqliteCatalog,
    generation: u64,
    digest: &str,
    sealed: bool,
    change_receipt: impl FnOnce(&mut BatchPublishReceipt),
) -> TestResult<u64> {
    let key = IdempotencyKeyV1 {
        kind: IngestOperationKindV1::SearchCorpus,
        repo_id: RepoId::new(REPO)?,
        revision_id: RevisionId::new(REVISION)?,
        generation: ManifestGeneration::new(generation),
        batch_digest: digest.into(),
    };
    let body = [u8::try_from(digest.len())?; 32];
    let ClaimOutcomeV1::Claimed(claim) = catalog.claim_prepared(
        &key,
        &body,
        "source-census-fixture",
        i64::MAX.unsigned_abs(),
        &body,
    )?
    else {
        return Err("fixture expected a fresh Catalog claim".into());
    };
    catalog.mark_applying(&claim)?;
    let mut receipt =
        BatchPublishReceipt::empty_for(key.generation, Some("digest-seven".into()), digest);
    receipt.attest_semantic_content(roots());
    if sealed {
        receipt.mark_sealed();
    }
    change_receipt(&mut receipt);
    Ok(catalog.commit(&claim, &receipt)?)
}

fn actual_census(
    root: &Path,
) -> TestResult<std::result::Result<ActiveSourceCensusV1, anyhow::Error>> {
    with_source_read_authority(root, collect_active_source_heads_v1)
}

fn with_source_read_authority<T>(
    root: &Path,
    read: impl FnOnce(
        &Path,
        &dyn CatalogReadOnlyIoAdmission,
        &dyn ObjectReadIoPort,
        &dyn ExistingTreeMetadataIoPortV1,
        &mut WorkBudget,
    ) -> anyhow::Result<T>,
) -> TestResult<anyhow::Result<T>> {
    let lease = StateRootLease::acquire(root)?;
    let root = lease.state_root_identity_v1().to_path_buf();
    let authority = seed_publication_capacity(
        &lease,
        &root,
        PublicationBootInventoryLimits {
            max_bytes: 64 * 1024 * 1024,
            max_files: 64,
            max_pending: 128,
            max_entries: 256,
            max_depth: 16,
            max_pending_frames: 32,
        },
        &[],
        &RequestBudgetV1::unbounded(),
    )?;
    let common = PublicationFileCapacity::new(lease, authority, 32, 256, 256)?;
    let objects = NativeObjectReadIo::new(Arc::clone(&common));
    let metadata = common.existing_tree_read_io_v1();
    let result = read(
        &root,
        common.as_ref(),
        &objects,
        &metadata,
        &mut read_budget(),
    );
    common.sweep_retired()?;
    Ok(result)
}

fn fixture(
    steps: &[(u64, &str, bool)],
    activate: bool,
) -> TestResult<(tempfile::TempDir, Vec<u64>)> {
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    fs::set_permissions(root, fs::Permissions::from_mode(0o700))?;
    let owner = SearchCorpusLifecycleOwner::open(
        root,
        SearchCorpusHistoryRetentionPolicyV1::new(2, 1024 * 1024, 8, 8 * 1024 * 1024)?,
        Arc::new(FixtureIndexBytes),
    )?;
    if activate {
        let candidate = identity(7)?;
        let prepared = PreparedSearchCorpusGenerationV1::new(candidate, None)?;
        let _activated = owner
            .activation_catalog()
            .activate_prepared_search_corpus_generation_v1(&prepared)?;
    }
    let catalog = SqliteCatalog::open(root, Duration::from_secs(1))?;
    let mut sequences = Vec::new();
    for &(generation, digest, sealed) in steps {
        sequences.push(commit(&catalog, generation, digest, sealed)?);
    }
    drop(catalog);
    let catalog_dir = quanta_index_catalog::catalog_dir(root);
    fs::set_permissions(&catalog_dir, fs::Permissions::from_mode(0o700))?;
    for entry in fs::read_dir(&catalog_dir)? {
        fs::set_permissions(entry?.path(), fs::Permissions::from_mode(0o600))?;
    }
    Ok((directory, sequences))
}

#[test]
fn source_high_water_accepts_only_the_latest_matching_actual_ack() -> TestResult {
    let (directory, sequences) = fixture(
        &[(7, "early-unsealed", false), (7, "active-sealed", true)],
        true,
    )?;
    let census = actual_census(directory.path())??;
    assert_eq!(census.heads.len(), 1);
    assert_eq!(census.catalog.published_search_corpus_rows, 2);
    assert_eq!(
        census.heads[0]
            .applied_ack
            .as_ref()
            .map(|ack| ack.durable_sequence),
        sequences.get(1).copied()
    );
    Ok(())
}

#[test]
fn legacy_source_census_retains_physical_repomap_without_source_head() -> TestResult {
    let (directory, _) = fixture(&[], false)?;
    let root = directory.path();
    let catalog = Arc::new(SqliteCatalog::open(root, Duration::from_secs(1))?);
    let store =
        quanta_index_repomap::RepoMapGenerationStore::open(root.join("repo-map"), catalog)?.store;
    let bundle = RepoMapSourceBundle::new(
        RepoId::new(REPO)?,
        RevisionId::new(REVISION)?,
        ManifestGeneration::new(7),
        "a".repeat(64),
        "map-only-snapshot",
        1,
        "b".repeat(64),
        RepoMapGraphCoverage {
            item_index_availability: RepoMapItemIndexAvailability::Available,
            graph_coverage_class: RepoMapGraphCoverageClass::Complete,
        },
        RepoMapExactnessSummary::Exact,
        RepoMapRedactionState::Unredacted,
    )
    .with_node(RepoMapNode::File(RepoMapFileNode {
        file_id: FileId::new("file://src/map_only.rs"),
        repo_relative_path: RepoRelativePath::new("src/map_only.rs"),
        line_count: 1,
    }));
    let published = store.ingest_bundle_v2(&RepoMapPublishBundleRequestV2::new(bundle.clone())?)?;
    let activated =
        store.activate_generation_v2(&RepoMapActivateGenerationRequestV2::for_bundle(&bundle)?)?;
    assert!(!published.mutation.replayed);
    assert!(!activated.mutation.replayed);
    drop(store);

    let census = actual_census(root)??;
    assert!(census.heads.is_empty());
    assert_eq!(census.catalog.published_search_corpus_rows, 0);
    assert_eq!(census.repomap_heads.len(), 1);
    assert_eq!(census.catalog.active_repomap_heads, 1);
    assert_eq!(census.repomap_heads[0].activation.repo_id, REPO);
    assert_eq!(census.repomap_heads[0].activation.revision_id, REVISION);
    assert_eq!(
        census.repomap_heads[0].activation.epoch,
        activated.mutation.activation_epoch
    );
    Ok(())
}

#[test]
fn single_scope_recovery_refuses_a_physical_active_sibling() -> TestResult {
    let (directory, _) = fixture(&[], true)?;
    let primary = identity(7)?.to_contract_v1();
    let inspect = || {
        with_source_read_authority(directory.path(), |root, _, objects, metadata, budget| {
            inspect_existing_activation_repository_v1(
                &root.join("activations"),
                &primary.lexical.repo_id,
                &primary.lexical.revision_id,
                objects,
                metadata,
                budget,
            )
            .map_err(Into::into)
        })
    };
    let before = inspect()??;
    assert_eq!(
        before.head_v1().map(|head| &head.generation),
        Some(&primary)
    );
    drop(before);

    let owner = SearchCorpusLifecycleOwner::open(
        directory.path(),
        SearchCorpusHistoryRetentionPolicyV1::new(2, 1024 * 1024, 8, 8 * 1024 * 1024)?,
        Arc::new(FixtureIndexBytes),
    )?;
    let sibling = identity_for("sibling", 7)?;
    let prepared = PreparedSearchCorpusGenerationV1::new(sibling, None)?;
    let _activated = owner
        .activation_catalog()
        .activate_prepared_search_corpus_generation_v1(&prepared)?;
    drop(owner);
    let rejected = inspect()?;
    assert!(matches!(
        rejected
            .as_ref()
            .err()
            .and_then(|error| error.downcast_ref::<ActivationReadFailureV1>()),
        Some(ActivationReadFailureV1::AdmissionOrIo(
            IndexError::DigestConflict
        ))
    ));
    Ok(())
}

#[test]
fn source_high_water_refuses_prestamped_stored_receipt() -> TestResult {
    let (directory, _) = fixture(&[], true)?;
    let catalog = SqliteCatalog::open(directory.path(), Duration::from_secs(1))?;
    let sequence = commit_with_raw_receipt(&catalog, 7, "pre-stamped", true, |receipt| {
        receipt.durable_sequence = u64::MAX;
    })?;
    assert_ne!(sequence, 0);
    drop(catalog);
    let Err(error) = actual_census(directory.path())? else {
        return Err("pre-stamped stored receipt was accepted as a Catalog ACK".into());
    };
    assert!(matches!(
        error.downcast_ref::<IndexError>(),
        Some(IndexError::DigestConflict)
    ));
    Ok(())
}

#[test]
fn source_high_water_refuses_replayed_stored_receipt() -> TestResult {
    let (directory, _) = fixture(&[], true)?;
    let catalog = SqliteCatalog::open(directory.path(), Duration::from_secs(1))?;
    let sequence = commit_with_raw_receipt(&catalog, 7, "replayed", true, |receipt| {
        receipt.applied = false;
    })?;
    assert_ne!(sequence, 0);
    drop(catalog);
    let Err(error) = actual_census(directory.path())? else {
        return Err("replayed stored receipt was accepted as an applied ACK".into());
    };
    assert!(matches!(
        error.downcast_ref::<IndexError>(),
        Some(IndexError::DigestConflict)
    ));
    Ok(())
}

#[test]
fn source_high_water_refuses_old_ack_after_new_matching_commit() -> TestResult {
    let (directory, _) = fixture(&[(7, "active-sealed", true), (7, "new-sealed", true)], true)?;
    let Err(error) = actual_census(directory.path())? else {
        return Err("a second matching sealed ACK displaced the active ACK".into());
    };
    assert!(matches!(
        error.downcast_ref::<IndexError>(),
        Some(IndexError::DigestConflict)
    ));
    Ok(())
}

#[test]
fn source_high_water_refuses_later_unapplied_committed_batch() -> TestResult {
    let (directory, _) = fixture(
        &[(7, "active-sealed", true), (7, "later-unsealed", false)],
        true,
    )?;
    let Err(error) = actual_census(directory.path())? else {
        return Err("later committed work was omitted from the active ACK".into());
    };
    assert!(
        error
            .to_string()
            .contains("Source activation lacks the latest committed Catalog ACK")
    );
    Ok(())
}

#[test]
fn source_high_water_refuses_committed_scope_without_active_head() -> TestResult {
    let (directory, _) = fixture(&[(7, "orphan-sealed", true)], false)?;
    let Err(error) = actual_census(directory.path())? else {
        return Err("committed Source scope without an active head was accepted".into());
    };
    assert!(matches!(
        error.downcast_ref::<IndexError>(),
        Some(IndexError::Conflict)
    ));
    Ok(())
}
