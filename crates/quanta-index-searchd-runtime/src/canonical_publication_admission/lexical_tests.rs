//! Real lexical publication against the canonical Control, Work and file authority.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fs;
use std::io;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use quanta_index_contract::{
    BatchIngestMode, ManifestGeneration, RepoId, RepoRelativePath, RevisionId,
    SearchCorpusIngestBatch, SearchCorpusReplaceScope, SourceFileCoverage, SourceFileKey,
    SourceFileRevision, SourcePublicationEvent, SymbolCoverage, SymbolNameSourcePolicyV1,
    source_event_payload_sha256, source_file_unit_set_sha256,
};
use quanta_index_core::publication_admission::{PublicationAdmissionRequest, PublicationBuildWork};
use quanta_index_core::publication_capacity::PublicationCapacity;
use quanta_index_core::{
    CoreError, INGEST_RESOURCE_BUDGET_EXCEEDED_CODE, IngestBatchFootprint,
    LexicalDocumentNamespaceV1, LexicalDocumentVersionRangeV1, LexicalDocumentVersionReservationV1,
    PublicationValidationOwner, RequestBudgetV1, SearchCorpusBatchBuildPort as _,
    SourceEventAllocationRequestV1, SourceEventAllocationV1,
};
use quanta_index_lexical::LexicalAdapter;
use quanta_index_searchd::app::runtime::StateRootLease;
use sha2::{Digest as _, Sha256};

use super::{CanonicalPublicationAdmission, PublicationAllowanceLimit};
use crate::publication_file_capacity::PublicationFileCapacity;
use crate::publication_file_deletions::PublicationFileDeletions;
use crate::publication_inventory::{PublicationBootInventoryLimits, seed_publication_capacity};

type TestResult = Result<(), Box<dyn Error>>;

macro_rules! require {
    ($condition:expr $(,)?) => {
        if !$condition {
            return Err(
                io::Error::other(concat!("assertion failed: ", stringify!($condition))).into(),
            );
        }
    };
}

struct Fixture {
    _directory: tempfile::TempDir,
    root: PathBuf,
    capacity: PublicationCapacity,
    binder: Arc<PublicationFileCapacity>,
    admission: CanonicalPublicationAdmission,
}

fn fixture(
    maximum_bytes: u64,
    control: PublicationAllowanceLimit,
    work: PublicationAllowanceLimit,
) -> Result<Fixture, Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let requested = directory.path().join("state");
    fs::create_dir(&requested)?;
    fs::set_permissions(&requested, fs::Permissions::from_mode(0o700))?;
    let lease = StateRootLease::acquire(&requested)?;
    let root = lease.state_root_identity_v1().to_path_buf();
    let capacity = seed_publication_capacity(
        &lease,
        &root,
        PublicationBootInventoryLimits {
            max_bytes: maximum_bytes,
            max_files: 30_000,
            max_pending: 30_000,
            max_entries: 30_000,
            max_depth: 16,
            max_pending_frames: 32,
        },
        &[],
        &RequestBudgetV1::unbounded(),
    )?;
    let binder = PublicationFileCapacity::new(lease, capacity.clone(), 256, 30_000, 30_000)?;
    let deletions = Arc::new(PublicationFileDeletions::new(Arc::clone(&binder)));
    let admission = CanonicalPublicationAdmission::new(deletions, control, work);
    Ok(Fixture {
        _directory: directory,
        root,
        capacity,
        binder,
        admission,
    })
}

fn one_file_batch() -> Result<SearchCorpusIngestBatch, Box<dyn Error>> {
    let source = b"fn lexical_fixture() {}";
    let repository = RepoId::new("lexical-physical-fixture")?;
    let revision = RevisionId::new("revision-1")?;
    let scope = SearchCorpusReplaceScope {
        coverage: SourceFileCoverage {
            source: SourceFileRevision {
                file: SourceFileKey {
                    source_repo_id: repository.clone(),
                    repo_relative_path: RepoRelativePath::new("src/lib.rs"),
                },
                revision_id: revision.clone(),
                source_sha256: Sha256::digest(source).into(),
            },
            language: quanta_index_contract::lex::LanguageCode::new("rust")?,
            producer_policy_sha256: [8; 32],
            symbol_name_source_policy: SymbolNameSourcePolicyV1::Unspecified,
            unit_set_sha256: source_file_unit_set_sha256(&[], &[])?,
            text_admitted: false,
            symbols: SymbolCoverage::NotRequested,
        },
        source_bytes: source.to_vec(),
        chunks: Vec::new(),
        symbols: Vec::new(),
    };
    let mut batch = SearchCorpusIngestBatch {
        source_event: SourcePublicationEvent {
            stream_id: "lexical-physical-stream".into(),
            event_id: "lexical-physical-event".into(),
            expected_base_event_id: None,
            payload_sha256: [0; 32],
        },
        repo_id: repository,
        revision_id: revision,
        generation: ManifestGeneration::new(1),
        base_generation: None,
        manifest_digest: "lexical-physical-manifest".into(),
        batch_digest: "0".repeat(64),
        mode: BatchIngestMode::ReplaceGeneration,
        bundle_payload: None,
        clear_surfaces: Vec::new(),
        replace_scopes: vec![scope],
        tombstone_scopes: Vec::new(),
        semantic_replace_scopes: Vec::new(),
        semantic_tombstone_scopes: Vec::new(),
        seal: true,
    };
    batch.source_event.payload_sha256 = source_event_payload_sha256(&batch)?;
    Ok(batch)
}

fn request(batch: &SearchCorpusIngestBatch) -> PublicationAdmissionRequest<'_> {
    PublicationAdmissionRequest {
        batch,
        footprint: IngestBatchFootprint {
            carried_records: 0,
            embedded_records: 0,
            source_bytes: 0,
            text_bytes: 0,
            vector_bytes: 0,
        },
        work: PublicationBuildWork::FinalizeOnly,
    }
}

fn owner(
    batch: &SearchCorpusIngestBatch,
    admission: &CanonicalPublicationAdmission,
) -> Result<PublicationValidationOwner, Box<dyn Error>> {
    let budget = RequestBudgetV1::unbounded();
    let mut owner = PublicationValidationOwner::default();
    owner.admit_publication_control(batch, &budget, admission)?;
    owner.admit_publication_resources(&request(batch), &budget, admission)?;
    let incarnation = [0x4b; 16];
    let allocation = SourceEventAllocationV1 {
        request: SourceEventAllocationRequestV1 {
            repo_id: batch.repo_id.clone(),
            revision_id: batch.revision_id.clone(),
            event: batch.source_event.clone(),
        },
        generation: batch.generation,
        root_incarnation: incarnation,
    };
    let reservation = LexicalDocumentVersionReservationV1 {
        allocation,
        file: LexicalDocumentVersionRangeV1 {
            first: 1,
            count: 1,
            scope_sha256: LexicalDocumentVersionRangeV1::scope_digest(
                &incarnation,
                &batch.repo_id,
                LexicalDocumentNamespaceV1::File,
            ),
        },
        native_text: LexicalDocumentVersionRangeV1 {
            first: 0,
            count: 0,
            scope_sha256: LexicalDocumentVersionRangeV1::scope_digest(
                &incarnation,
                &batch.repo_id,
                LexicalDocumentNamespaceV1::NativeText,
            ),
        },
    };
    owner.bind_lexical_reservation(batch, &reservation)?;
    Ok(owner)
}

type Tree = BTreeMap<PathBuf, (bool, u64, [u8; 32], (u64, u64))>;

fn tree(root: &Path) -> Result<Tree, Box<dyn Error>> {
    fn visit(root: &Path, current: &Path, entries: &mut Tree) -> Result<(), Box<dyn Error>> {
        for entry in fs::read_dir(current)? {
            let path = entry?.path();
            let metadata = fs::symlink_metadata(&path)?;
            let relative = path.strip_prefix(root)?.to_path_buf();
            let inode = (metadata.dev(), metadata.ino());
            if metadata.is_dir() {
                if entries
                    .insert(relative, (true, 0, [0; 32], inode))
                    .is_some()
                {
                    return Err(io::Error::other("duplicate tree directory").into());
                }
                visit(root, &path, entries)?;
            } else if metadata.is_file() {
                let bytes = fs::read(&path)?;
                if entries
                    .insert(
                        relative,
                        (false, metadata.len(), Sha256::digest(bytes).into(), inode),
                    )
                    .is_some()
                {
                    return Err(io::Error::other("duplicate tree file").into());
                }
            } else {
                return Err(io::Error::other("unexpected nonregular tree entry").into());
            }
        }
        Ok(())
    }
    let mut entries = Tree::new();
    visit(root, root, &mut entries)?;
    Ok(entries)
}

fn pool_census(pool: &Path) -> Result<(u64, u64), Box<dyn Error>> {
    let entries = tree(pool)?;
    let mut files = 0_u64;
    let mut bytes = 0_u64;
    let mut seen = BTreeSet::new();
    for (directory, width, _digest, inode) in entries.values() {
        if !directory && seen.insert(*inode) {
            files = files.checked_add(1).ok_or("pool file count overflow")?;
            bytes = bytes
                .checked_add(*width)
                .ok_or("pool byte count overflow")?;
        }
    }
    Ok((files, bytes))
}

#[test]
fn real_lexical_publication_retires_pins_and_charges_exact_pool_files() -> TestResult {
    let fixture = fixture(
        1024 * 1024 * 1024,
        PublicationAllowanceLimit::new(16 * 1024 * 1024, 512)?,
        PublicationAllowanceLimit::new(256 * 1024 * 1024, 15_000)?,
    )?;
    let before = fixture.capacity.snapshot()?;
    let batch = one_file_batch()?;
    let mut owner = owner(&batch, &fixture.admission)?;
    let adapter = LexicalAdapter::with_state_root(fixture.root.join("indexes/lexical"))
        .with_object_lifetime(fixture.admission.object_lifetime());
    require!(
        adapter
            .build_batch_with_owner(&batch, &mut owner, None)?
            .is_some()
    );
    drop(owner);
    let pool = fixture.root.join("indexes/lexical/lexical-object-pool");
    require!(pool.join(".lifecycle-gate").is_file());
    require!(pool.join(".lifecycle-epoch").is_file());
    let (files, bytes) = pool_census(&pool)?;
    let after = fixture.capacity.snapshot()?;
    require!(files > 2);
    require!(
        after.used_files
            == before
                .used_files
                .checked_add(files)
                .ok_or("files overflow")?
    );
    require!(
        after.used_bytes
            == before
                .used_bytes
                .checked_add(bytes)
                .ok_or("bytes overflow")?
    );
    require!(after.uncertain_pending == before.uncertain_pending);
    require!(fixture.binder.is_quiescent()?);
    Ok(())
}

#[test]
fn control_parent_refusal_precedes_any_lexical_mutation() -> TestResult {
    let fixture = fixture(
        1024 * 1024,
        PublicationAllowanceLimit::new(2 * 1024 * 1024, 1)?,
        PublicationAllowanceLimit::new(1, 1)?,
    )?;
    let batch = one_file_batch()?;
    let before_tree = tree(&fixture.root)?;
    let before_capacity = fixture.capacity.snapshot()?;
    let mut owner = PublicationValidationOwner::default();
    let error = match owner.admit_publication_control(
        &batch,
        &RequestBudgetV1::unbounded(),
        &fixture.admission,
    ) {
        Ok(()) => return Err(io::Error::other("Control parent unexpectedly admitted").into()),
        Err(error) => error,
    };
    require!(
        matches!(error, CoreError::Typed { code, .. } if code == INGEST_RESOURCE_BUDGET_EXCEEDED_CODE)
    );
    require!(tree(&fixture.root)? == before_tree);
    require!(fixture.capacity.snapshot()? == before_capacity);
    require!(fixture.binder.is_quiescent()?);
    Ok(())
}

#[test]
fn small_control_parent_keeps_admitted_gate_prefix_when_epoch_is_refused() -> TestResult {
    let fixture = fixture(
        512 * 1024 * 1024,
        PublicationAllowanceLimit::new(1, 1)?,
        PublicationAllowanceLimit::new(128 * 1024 * 1024, 10_000)?,
    )?;
    let before = fixture.capacity.snapshot()?;
    let batch = one_file_batch()?;
    let mut owner = owner(&batch, &fixture.admission)?;
    let adapter = LexicalAdapter::with_state_root(fixture.root.join("indexes/lexical"))
        .with_object_lifetime(fixture.admission.object_lifetime());
    let outcome = adapter.build_batch_with_owner(&batch, &mut owner, None);
    require!(
        matches!(outcome, Err(CoreError::Typed { code, .. }) if code == INGEST_RESOURCE_BUDGET_EXCEEDED_CODE)
    );
    drop(owner);
    let pool = fixture.root.join("indexes/lexical/lexical-object-pool");
    require!(pool.join(".lifecycle-gate").is_file());
    require!(!pool.join(".lifecycle-epoch").exists());
    let (files, bytes) = pool_census(&pool)?;
    require!(files == 1 && bytes == 0);
    let after = fixture.capacity.snapshot()?;
    require!(after.used_files == before.used_files.checked_add(1).ok_or("files overflow")?);
    require!(after.used_bytes == before.used_bytes);
    require!(after.uncertain_pending == before.uncertain_pending);
    require!(fixture.binder.is_quiescent()?);
    Ok(())
}

#[test]
fn foreign_lexical_lifetime_refuses_before_publication_directory_creation() -> TestResult {
    let primary = fixture(
        1024 * 1024 * 1024,
        PublicationAllowanceLimit::new(16 * 1024 * 1024, 512)?,
        PublicationAllowanceLimit::new(256 * 1024 * 1024, 15_000)?,
    )?;
    let foreign = fixture(
        1024 * 1024 * 1024,
        PublicationAllowanceLimit::new(16 * 1024 * 1024, 512)?,
        PublicationAllowanceLimit::new(256 * 1024 * 1024, 15_000)?,
    )?;
    let batch = one_file_batch()?;
    let mut owner = owner(&batch, &primary.admission)?;
    let before_tree = tree(&primary.root)?;
    let adapter = LexicalAdapter::with_state_root(primary.root.join("indexes/lexical"))
        .with_object_lifetime(foreign.admission.object_lifetime());
    let result = adapter.build_batch_with_owner(&batch, &mut owner, None);
    require!(matches!(result, Err(CoreError::InvalidContract(_))));
    require!(tree(&primary.root)? == before_tree);
    drop(owner);
    require!(primary.binder.is_quiescent()?);
    Ok(())
}
