//! Existing Lexical physical verifier component proof with real temporary files.
//! These finite ports do not supply installed Legacy root or publication authority.
#![forbid(unsafe_code)]

#[path = "support/source_fixture.rs"]
mod source_fixture;

use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{
    ChunkId, ChunkRecord, GenerationSnapshot, LqVisibility, ManifestGeneration, RepoId,
    RepoRelativePath, RevisionId, SearchCorpusGenerationReservationRequestV1, SearchPlaneTrackKind,
};
use quanta_index_core::index::{
    IndexError, MemoryLease, MemoryLeaseOwner, MemoryLifetime, MemoryScope, MemoryScopePort,
    ObjectReadFile, ObjectReadIoPort, WorkBudget, WorkLimits, WorkUsage,
    existing_file::ExistingFileReadFailureV1 as Failure,
};
use quanta_index_core::{ExistingTreeEntryKindV1, ExistingTreeMetadataIoPortV1};
use quanta_index_lexical::legacy::{
    SourceVisitFailureV1, verify_existing_lexical_generation_read_only,
    visit_existing_complete_source_files_v1,
};
use quanta_index_sdk::{
    FileContributorBatch, FileOwnershipBatch, QuantaIndex, RepoCommitRecencyBatch,
    RepoDescriptionBatch, RepoMetaBatch, RepoTopicBatch, SearchCorpusBatch,
};
use quanta_index_searchd_harness::{
    SourceCorpusFixture,
    legacy_reference::{LegacySearchdBinary, daemon_socket_paths},
};
use std::{
    cell::Cell,
    ffi::OsStr,
    fs::{self, File},
    os::unix::fs::{MetadataExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const OVERLAY_NAMES: [&str; 7] = [
    "repo-metadata.cbor",
    "repo-commit-recency.cbor",
    "repo-meta.cbor",
    "repo-topic.cbor",
    "repo-description.cbor",
    "file-ownership.cbor",
    "file-contributor.cbor",
];

#[derive(Default)]
struct LiveMemory {
    bytes: AtomicU64,
    bad_release: AtomicBool,
}
impl MemoryLeaseOwner for LiveMemory {
    fn release(&self, bytes: u64) {
        if self
            .bytes
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |old| {
                old.checked_sub(bytes)
            })
            .is_err()
        {
            self.bad_release.store(true, Ordering::SeqCst);
        }
    }
}
struct FiniteMemory(Arc<LiveMemory>);
impl MemoryScopePort for FiniteMemory {
    fn reserve(&self, bytes: u64, _: MemoryLifetime) -> Result<MemoryLease, IndexError> {
        let _prior = self
            .0
            .bytes
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |old| {
                old.checked_add(bytes)
                    .filter(|next| *next <= 512 * 1024 * 1024)
            })
            .map_err(|_| IndexError::ResourceLimited)?;
        Ok(MemoryLease::new(bytes, self.0.clone()))
    }
}
fn original_budget() -> Result<(WorkBudget, Arc<LiveMemory>), Box<dyn std::error::Error>> {
    let mut budget = WorkBudget::new(
        WorkLimits {
            reads: 1_000_000,
            writes: 0,
            bytes: 1024 * 1024 * 1024,
            mutations: 0,
        },
        Instant::now()
            .checked_add(Duration::from_secs(60))
            .ok_or("deadline overflow")?,
    );
    let memory = Arc::new(LiveMemory::default());
    budget.bind_memory_scope(Some(MemoryScope::new(Arc::new(FiniteMemory(
        memory.clone(),
    )))));
    Ok((budget, memory))
}

#[derive(Debug)]
struct Files {
    root: PathBuf,
    opens: AtomicU64,
    visits: AtomicU64,
}
struct Opened {
    file: File,
    _memory: MemoryLease,
}
impl ObjectReadFile for Opened {
    fn file(&self) -> Result<&File, IndexError> {
        Ok(&self.file)
    }
}
impl Files {
    fn contains(&self, path: &Path) -> Result<(), IndexError> {
        let relative = path
            .strip_prefix(&self.root)
            .map_err(|_| IndexError::DigestConflict)?;
        if relative
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
        {
            return Err(IndexError::DigestConflict);
        }
        Ok(())
    }
}
impl ObjectReadIoPort for Files {
    fn open(
        &self,
        path: &Path,
        budget: &mut WorkBudget,
    ) -> Result<Box<dyn ObjectReadFile>, IndexError> {
        self.contains(path)?;
        budget.checkpoint()?;
        let grant = budget
            .reserve_memory(
                u64::try_from(std::mem::size_of::<Opened>())
                    .map_err(|_| IndexError::ResourceLimited)?,
                MemoryLifetime::Temporary,
            )?
            .ok_or(IndexError::ResourceLimited)?;
        let metadata =
            fs::symlink_metadata(path).map_err(|error| IndexError::Storage(error.to_string()))?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(IndexError::DigestConflict);
        }
        let file = File::open(path).map_err(|error| IndexError::Storage(error.to_string()))?;
        let _prior = self.opens.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(Opened {
            file,
            _memory: grant,
        }))
    }
}
impl ExistingTreeMetadataIoPortV1 for Files {
    fn visit_directory_v1(
        &self,
        path: &Path,
        budget: &mut WorkBudget,
        visit: &mut dyn FnMut(
            &OsStr,
            ExistingTreeEntryKindV1,
            u64,
            &mut WorkBudget,
        ) -> Result<(), Failure>,
    ) -> Result<(), Failure> {
        self.contains(path)?;
        budget.charge(WorkUsage {
            reads: 1,
            ..Default::default()
        })?;
        let _grant = budget
            .reserve_memory(4096, MemoryLifetime::Temporary)?
            .ok_or(IndexError::ResourceLimited)?;
        let _prior = self.visits.fetch_add(1, Ordering::SeqCst);
        for entry in fs::read_dir(path)? {
            budget.charge(WorkUsage {
                reads: 1,
                ..Default::default()
            })?;
            let entry = entry?;
            let meta = fs::symlink_metadata(entry.path())?;
            let kind = if meta.file_type().is_symlink() {
                ExistingTreeEntryKindV1::Symlink
            } else if meta.is_dir() {
                ExistingTreeEntryKindV1::Directory
            } else if meta.is_file() {
                ExistingTreeEntryKindV1::File
            } else {
                ExistingTreeEntryKindV1::Other
            };
            visit(&entry.file_name(), kind, meta.len(), budget)?;
        }
        Ok(())
    }
}

fn physical_file_count(root: &Path) -> Result<usize, Box<dyn std::error::Error>> {
    let mut pending = vec![root.to_path_buf()];
    let mut count = 0_usize;
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let meta = fs::symlink_metadata(entry.path())?;
            if meta.is_dir() {
                pending.push(entry.path());
            } else if meta.is_file() {
                count = count.checked_add(1).ok_or("file count overflow")?;
            }
        }
    }
    Ok(count)
}

fn repo_metadata_bundle_payload(label: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut visibility = Vec::new();
    ciborium::into_writer(&LqVisibility::Private, &mut visibility)?;
    let visibility: ciborium::Value = ciborium::from_reader(visibility.as_slice())?;
    let row = ciborium::Value::Map(vec![
        (
            ciborium::Value::Text("fork".to_string()),
            ciborium::Value::Bool(false),
        ),
        (
            ciborium::Value::Text("archived".to_string()),
            ciborium::Value::Bool(false),
        ),
        (ciborium::Value::Text("visibility".to_string()), visibility),
        (
            ciborium::Value::Text("contexts".to_string()),
            ciborium::Value::Array(vec![ciborium::Value::Text(label.to_string())]),
        ),
    ]);
    let mut payload = Vec::new();
    ciborium::into_writer(&row, &mut payload)?;
    Ok(payload)
}

fn publish_auxiliary_overlays(
    client: &QuantaIndex,
    repo: &RepoId,
    revision: &RevisionId,
    generation: ManifestGeneration,
    label: &str,
) -> TestResult {
    let source = repo.clone();
    let discriminator = *label.as_bytes().first().ok_or("empty overlay label")?;
    let receipts = [
        client.history().publish_repo_commit_recency(
            &RepoCommitRecencyBatch::new(repo.clone(), revision.clone(), generation)
                .entry(source.clone(), 1_700_000_000_000 + u64::from(discriminator)),
        )?,
        client.history().publish_repo_meta(
            &RepoMetaBatch::new(repo.clone(), revision.clone(), generation).entry(
                source.clone(),
                "lifecycle",
                label,
            ),
        )?,
        client.history().publish_repo_topic(
            &RepoTopicBatch::new(repo.clone(), revision.clone(), generation)
                .entry(source.clone(), format!("topic-{label}")),
        )?,
        client.history().publish_repo_description(
            &RepoDescriptionBatch::new(repo.clone(), revision.clone(), generation)
                .entry(source.clone(), format!("{label} description")),
        )?,
        client.history().publish_file_ownership(
            &FileOwnershipBatch::new(repo.clone(), revision.clone(), generation).entry(
                source.clone(),
                RepoRelativePath::new("src/lib.rs"),
                vec![format!("@{label}-owner")],
            ),
        )?,
        client.history().publish_file_contributor(
            &FileContributorBatch::new(repo.clone(), revision.clone(), generation).entry(
                source,
                RepoRelativePath::new("src/lib.rs"),
                vec![format!("{label}-contributor")],
            ),
        )?,
    ];
    for receipt in receipts {
        if !receipt.applied || receipt.generation != generation || receipt.durable_sequence == 0 {
            return Err(format!("overlay publication was not durable: {receipt:?}").into());
        }
    }
    Ok(())
}

struct Fixture {
    _root: tempfile::TempDir,
    lexical_root: PathBuf,
    expected: GenerationSnapshot,
    owner: u32,
    maximum_objects: usize,
    files: Files,
}
impl Fixture {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        Self::new_source_fixture("baseline")
    }

    fn with_overlays(label: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700))?;
        // The stale-artifact cases need a second valid old-Q root with the
        // same encoded width but different File authority bytes. Source body
        // changes alone do not change that path/ownership map, so bind the
        // fixture label into a same-width repository identity as well.
        let repo = RepoId::new(format!("legacy-existing-repo-{label}"))?;
        let revision = RevisionId::new("legacy-existing-revision")?;
        let generation = ManifestGeneration::new(1);
        let body = format!("fn main() {{ legacy_existing_needle_{label}(); }}");
        let chunk = ChunkRecord {
            chunk_id: ChunkId::new(format!("legacy-existing-overlay-chunk-{label}")),
            repo_relative_path: RepoRelativePath::new("src/lib.rs"),
            language: LanguageCode::new("rust").map_err(|error| -> Box<dyn std::error::Error> {
                format!("language: {error}").into()
            })?,
            start_byte: 0,
            end_byte: u32::try_from(body.len())?,
            start_line: 1,
            end_line: 1,
            text: body.into_boxed_str(),
            structural: None,
            parent_chunk_id: None,
            source_repo_id: None,
        };

        let legacy = LegacySearchdBinary::from_environment()?;
        let process = legacy.start(root.path(), 8)?;
        let client = process.connect()?;
        let mut corpus = SourceCorpusFixture::new(
            repo.clone(),
            revision.clone(),
            generation,
            format!("legacy-existing-overlay-event-{label}"),
        );
        corpus.replace_chunks(vec![chunk])?;
        corpus.set_metadata(repo_metadata_bundle_payload(label)?)?;
        let batch = corpus.frozen_batch()?;
        let reservation_request = SearchCorpusGenerationReservationRequestV1 {
            repo_id: repo.clone(),
            revision_id: revision.clone(),
            source_event: batch.source_event.clone(),
        };
        let reserved = client
            .search_corpus()
            .reserve_generation(&reservation_request)?;
        if reserved.repo_id != repo
            || reserved.revision_id != revision
            || reserved.generation != generation
            || reserved.source_event != reservation_request.source_event
        {
            return Err(format!("Legacy overlay reservation differs: {reserved:?}").into());
        }
        publish_auxiliary_overlays(&client, &repo, &revision, generation, label)?;
        let ingest_socket = daemon_socket_paths(root.path())[2].clone();
        corpus.publish(&ingest_socket, 1)?;
        let expected = GenerationSnapshot {
            repo_id: repo,
            revision_id: revision,
            track: SearchPlaneTrackKind::Lexical,
            manifest_generation: generation,
            manifest_digest: batch.manifest_digest,
        };
        drop(client);
        process.stop()?;
        legacy.verify()?;
        Self::from_stopped_root(root, expected)
    }

    fn new_source_fixture(label: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700))?;
        let repo = RepoId::new("legacy-existing-repo")?;
        let revision = RevisionId::new("legacy-existing-revision")?;
        let generation = ManifestGeneration::new(1);
        let body = if label == "baseline" {
            "fn main() { legacy_existing_needle(); }".to_string()
        } else {
            format!("fn main() {{ legacy_existing_needle_{label}(); }}")
        };
        let language =
            LanguageCode::new("rust").map_err(|error| -> Box<dyn std::error::Error> {
                format!("language: {error}").into()
            })?;
        let scope = source_fixture::complete_file(
            source_fixture::file_key(&repo, "src/lib.rs"),
            &revision,
            language.clone(),
            body.as_bytes(),
            vec![ChunkRecord {
                chunk_id: ChunkId::new(format!("legacy-existing-chunk-{label}")),
                repo_relative_path: RepoRelativePath::new("src/lib.rs"),
                language,
                start_byte: 0,
                end_byte: u32::try_from(body.len())?,
                start_line: 1,
                end_line: 1,
                text: body.clone().into_boxed_str(),
                structural: None,
                parent_chunk_id: None,
                source_repo_id: None,
            }],
            Vec::new(),
        )?;
        let foreign_repo = RepoId::new("legacy-existing-foreign-source")?;
        let binary = vec![0xff_u8; 80 * 1024];
        let binary_scope = source_fixture::complete_file(
            source_fixture::file_key(&foreign_repo, "assets/data.bin"),
            &revision,
            LanguageCode::new("binary").map_err(|error| -> Box<dyn std::error::Error> {
                format!("language: {error}").into()
            })?,
            &binary,
            Vec::new(),
            Vec::new(),
        )?;
        let batch =
            source_fixture::sealed_batch(&repo, &revision, generation, vec![scope, binary_scope])?;
        let requested = GenerationSnapshot {
            repo_id: repo.clone(),
            revision_id: revision.clone(),
            track: SearchPlaneTrackKind::Lexical,
            manifest_generation: generation,
            manifest_digest: batch.manifest_digest.clone(),
        };
        let mut sdk_batch = SearchCorpusBatch::replace_generation(
            repo.clone(),
            revision.clone(),
            generation,
            batch.manifest_digest,
        )
        .source_event(batch.source_event);
        for scope in batch.replace_scopes {
            sdk_batch = sdk_batch.replace_scope(
                scope.coverage,
                scope.source_bytes,
                scope.chunks,
                scope.symbols,
            );
        }
        let legacy = LegacySearchdBinary::from_environment()?;
        let process = legacy.start(root.path(), 8)?;
        let client = process.connect()?;
        assert_eq!(
            client
                .generations()
                .active_head(repo.clone(), revision.clone())?,
            None
        );
        let reservation_request = sdk_batch.generation_reservation_request()?;
        let reserved = client
            .search_corpus()
            .reserve_generation(&reservation_request)?;
        if reserved.repo_id != repo
            || reserved.revision_id != revision
            || reserved.generation != generation
            || reserved.source_event != reservation_request.source_event
        {
            return Err(format!("Legacy Source reservation differs: {reserved:?}").into());
        }
        let (receipt, activation) = client
            .search_corpus()
            .publish_and_activate(&sdk_batch, None)?;
        assert!(receipt.applied && receipt.sealed && receipt.durable_sequence > 0);
        assert_eq!(receipt.generation, generation);
        assert_eq!(
            receipt.manifest_digest.as_deref(),
            Some(requested.manifest_digest.as_str())
        );
        assert_eq!(receipt.accepted_replace_scopes, 2);
        let observed = client
            .generations()
            .active_head(repo.clone(), revision.clone())?
            .ok_or("Legacy Source activation has no observed head")?;
        assert_eq!(observed, activation.active);
        assert_eq!(observed.generation.lexical, requested);
        drop(client);
        process.stop()?;
        legacy.verify()?;
        Self::from_stopped_root(root, requested)
    }

    fn from_stopped_root(
        root: tempfile::TempDir,
        expected: GenerationSnapshot,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let lexical_root = root.path().join("indexes/lexical");
        let owner = fs::metadata(&lexical_root)?.uid();
        let maximum_objects = physical_file_count(&lexical_root)?;
        let files = Files {
            root: lexical_root.clone(),
            opens: AtomicU64::new(0),
            visits: AtomicU64::new(0),
        };
        Ok(Self {
            _root: root,
            lexical_root,
            expected,
            owner,
            maximum_objects,
            files,
        })
    }
    fn verify(&self, budget: &mut WorkBudget) -> Result<(), Failure> {
        verify_existing_lexical_generation_read_only(
            &self.lexical_root,
            &self.expected,
            self.owner,
            self.maximum_objects,
            &self.files,
            &self.files,
            budget,
        )
    }

    fn sealed_manifest_path(&self) -> PathBuf {
        self.generation_dir()
            .join("search-corpus-generation-manifest.cbor")
    }

    fn generation_dir(&self) -> PathBuf {
        quanta_index_core::GenerationStorageKeyV1::for_repo_revision(
            &self.expected.repo_id,
            &self.expected.revision_id,
        )
        .generation_dir(&self.lexical_root, self.expected.manifest_generation)
    }
}

#[test]
fn lexical_existing_real_generation_authenticates_full_physical_bytes() -> TestResult {
    let fixture = Fixture::new()?;
    let (mut budget, memory) = original_budget()?;
    fixture.verify(&mut budget)?;
    if fixture.files.opens.load(Ordering::SeqCst) == 0
        || fixture.files.visits.load(Ordering::SeqCst) == 0
        || memory.bytes.load(Ordering::SeqCst) != 0
        || memory.bad_release.load(Ordering::SeqCst)
    {
        return Err("full Lexical physical verifier skipped IO or leaked allocation".into());
    }
    Ok(())
}

#[test]
fn lexical_existing_whole_corpus_manifest_requires_rebuild() -> TestResult {
    use quanta_index_contract::SearchPlaneErrorCodeV2::GenerationTextAuthorityFormatUnsupported;
    use quanta_index_core::CoreError;

    let fixture = Fixture::new()?;
    let path = fixture.sealed_manifest_path();
    let original = fs::read(&path)?;
    let mut manifest: ciborium::Value = ciborium::from_reader(original.as_slice())?;
    let format = manifest
        .as_array_mut()
        .and_then(|fields| fields.first_mut())
        .ok_or("fixture manifest has no format field")?;
    if format.as_integer() == Some(2u8.into()) {
        return Err("fixture must publish a supported format before mutation".into());
    }
    *format = ciborium::Value::Integer(2u8.into());
    let mut obsolete = Vec::new();
    ciborium::into_writer(&manifest, &mut obsolete)?;
    fs::write(&path, &obsolete)?;
    expect_data_custody_refusal(&fixture, "format-2 whole-corpus manifest", |failure| {
        matches!(
            failure,
            Failure::Canonical(CoreError::Typed {
                code: GenerationTextAuthorityFormatUnsupported,
                ..
            })
        )
    })?;
    assert_eq!(fs::read(&path)?, obsolete);

    fs::write(path, original)?;
    let (mut restored_budget, restored_memory) = original_budget()?;
    fixture.verify(&mut restored_budget)?;
    assert_eq!(restored_memory.bytes.load(Ordering::SeqCst), 0);
    assert!(!restored_memory.bad_release.load(Ordering::SeqCst));
    Ok(())
}

#[test]
fn lexical_existing_rejects_invalid_manifest_before_visiting_committed_objects() -> TestResult {
    use quanta_index_contract::SearchPlaneErrorCodeV2::{
        GenerationManifestFormatUnsupported, GenerationSidecarCorrupt,
    };

    let fixture = Fixture::new()?;
    let path = fixture.sealed_manifest_path();
    let original = fs::read(&path)?;
    let manifest: ciborium::Value = ciborium::from_reader(original.as_slice())?;

    let mut trailing = original.clone();
    trailing.push(0xff);
    let mut cases = vec![(trailing, GenerationSidecarCorrupt, "trailing")];
    // The retired reader explicitly rejected formats 8 through 18. Preserve
    // that rebuild diagnosis at the remaining read-only DATA entry point.
    for version in 8u32..=18 {
        let mut unsupported = manifest.clone();
        let format = unsupported
            .as_array_mut()
            .and_then(|fields| fields.first_mut())
            .ok_or("fixture manifest has no format field")?;
        *format = ciborium::Value::Integer(version.into());
        let mut bytes = Vec::new();
        ciborium::into_writer(&unsupported, &mut bytes)?;
        cases.push((
            bytes,
            GenerationManifestFormatUnsupported,
            "must be rebuilt",
        ));
    }

    let mut oversized_commit = manifest.clone();
    let commit_size = oversized_commit
        .as_array_mut()
        .and_then(|fields| fields.get_mut(3))
        .and_then(ciborium::Value::as_array_mut)
        .and_then(|commitment| commitment.get_mut(1))
        .ok_or("fixture manifest has no index commit length")?;
    *commit_size = ciborium::Value::Integer((16 * 1024 * 1024 + 1u32).into());
    let mut bytes = Vec::new();
    ciborium::into_writer(&oversized_commit, &mut bytes)?;
    cases.push((
        bytes,
        GenerationSidecarCorrupt,
        "index commit exceeds the control-file byte limit",
    ));

    let mut oversized_root = manifest;
    let root_size = oversized_root
        .as_array_mut()
        .and_then(|fields| fields.get_mut(8))
        .and_then(ciborium::Value::as_array_mut)
        .and_then(|roots| roots.first_mut())
        .and_then(ciborium::Value::as_array_mut)
        .and_then(|commitment| commitment.get_mut(1))
        .ok_or("fixture manifest has no Native root commitment length")?;
    *root_size = ciborium::Value::Integer((16 * 1024 + 1u32).into());
    let mut bytes = Vec::new();
    ciborium::into_writer(&oversized_root, &mut bytes)?;
    cases.push((
        bytes,
        GenerationSidecarCorrupt,
        "Native authority requires exactly one bounded root commitment",
    ));

    for (bytes, expected_code, detail) in cases {
        fs::write(&path, &bytes)?;
        expect_data_custody_refusal(&fixture, detail, |failure| {
            matches!(failure,
                Failure::Canonical(quanta_index_core::CoreError::Typed { code, message })
                    if *code == expected_code && message.contains(detail))
        })?;
        assert_eq!(fixture.files.visits.load(Ordering::SeqCst), 0);
        assert_eq!(fs::read(&path)?, bytes);
    }

    fs::write(path, original)?;
    let (mut budget, memory) = original_budget()?;
    fixture.verify(&mut budget)?;
    assert_eq!(memory.bytes.load(Ordering::SeqCst), 0);
    assert!(!memory.bad_release.load(Ordering::SeqCst));
    Ok(())
}

#[test]
fn lexical_existing_oversized_physical_manifest_requires_rebuild() -> TestResult {
    use quanta_index_contract::SearchPlaneErrorCodeV2::GenerationManifestFormatUnsupported;
    use quanta_index_core::CoreError;
    use quanta_index_core::index::existing_file::read_existing_file_v1;

    const MANIFEST_LIMIT: u64 = 16 * 1024 * 1024;
    const OVERSIZED_LENGTH: u64 = MANIFEST_LIMIT + 1;
    let fixture = Fixture::new()?;
    let path = fixture.sealed_manifest_path();
    let original = fs::read(&path)?;
    File::options()
        .write(true)
        .open(&path)?
        .set_len(OVERSIZED_LENGTH)?;

    let (mut verify_budget, verify_memory) = original_budget()?;
    let verified = fixture.verify(&mut verify_budget);
    assert!(
        matches!(&verified, Err(Failure::Canonical(CoreError::Typed {
            code: GenerationManifestFormatUnsupported,
            message,
        })) if message.contains("must be rebuilt")),
        "unexpected physical verification result: {verified:?}"
    );

    let began = Cell::new(false);
    let (mut export_budget, export_memory) = original_budget()?;
    let exported = visit_existing_complete_source_files_v1(
        &fixture.lexical_root,
        &fixture.expected,
        fixture.owner,
        fixture.maximum_objects,
        &fixture.files,
        &fixture.files,
        &mut export_budget,
        &mut |_, _, _| -> Result<(), std::io::Error> {
            began.set(true);
            Ok(())
        },
        &mut |_, _, _, _| -> Result<(), std::io::Error> { Ok(()) },
    );
    assert!(
        matches!(&exported, Err(SourceVisitFailureV1::Physical(Failure::Canonical(CoreError::Typed {
            code: GenerationManifestFormatUnsupported,
            message,
        }))) if message.contains("must be rebuilt")),
        "unexpected Source export result: {exported:?}"
    );
    assert!(!began.get());
    assert_eq!(fixture.files.visits.load(Ordering::SeqCst), 0);
    assert_eq!(fs::metadata(&path)?.len(), OVERSIZED_LENGTH);

    let (mut generic_budget, generic_memory) = original_budget()?;
    assert!(matches!(
        read_existing_file_v1(
            &path,
            fixture.owner,
            MANIFEST_LIMIT,
            &fixture.files,
            &mut generic_budget,
        ),
        Err(Failure::AdmissionOrIo(IndexError::DigestConflict))
    ));
    for (budget, memory) in [
        (&verify_budget, verify_memory),
        (&export_budget, export_memory),
        (&generic_budget, generic_memory),
    ] {
        assert!(budget.usage().bytes < OVERSIZED_LENGTH);
        assert_eq!(budget.usage().writes, 0);
        assert_eq!(budget.usage().mutations, 0);
        assert_eq!(memory.bytes.load(Ordering::SeqCst), 0);
        assert!(!memory.bad_release.load(Ordering::SeqCst));
    }

    fs::write(path, original)?;
    let (mut budget, _) = original_budget()?;
    fixture.verify(&mut budget)?;
    Ok(())
}

fn expect_data_custody_refusal(
    fixture: &Fixture,
    case: &str,
    expected: impl Fn(&Failure) -> bool,
) -> TestResult {
    let (mut verify_budget, verify_memory) = original_budget()?;
    let verified = fixture.verify(&mut verify_budget);
    assert!(
        matches!(&verified, Err(error) if expected(error)),
        "{case}: {verified:?}"
    );

    let began = Cell::new(false);
    let (mut export_budget, export_memory) = original_budget()?;
    let exported = visit_existing_complete_source_files_v1(
        &fixture.lexical_root,
        &fixture.expected,
        fixture.owner,
        fixture.maximum_objects,
        &fixture.files,
        &fixture.files,
        &mut export_budget,
        &mut |_, _, _| -> Result<(), std::io::Error> {
            began.set(true);
            Ok(())
        },
        &mut |_, _, _, _| -> Result<(), std::io::Error> { Ok(()) },
    );
    assert!(
        matches!(&exported, Err(SourceVisitFailureV1::Physical(error)) if expected(error)),
        "{case}: {exported:?}"
    );
    assert!(
        !began.get(),
        "{case}: export began without physical custody"
    );
    for (budget, memory) in [
        (&verify_budget, verify_memory),
        (&export_budget, export_memory),
    ] {
        assert_eq!(budget.usage().writes, 0);
        assert_eq!(budget.usage().mutations, 0);
        assert_eq!(memory.bytes.load(Ordering::SeqCst), 0);
        assert!(!memory.bad_release.load(Ordering::SeqCst));
    }
    Ok(())
}

#[test]
fn lexical_existing_missing_manifest_refuses_and_restores() -> TestResult {
    use quanta_index_contract::SearchPlaneErrorCodeV2::{
        GenerationManifestMissing, GenerationSidecarCorrupt,
    };
    use quanta_index_core::CoreError;
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new()?;
    let path = fixture.sealed_manifest_path();
    let original = fs::read(&path)?;
    fs::remove_file(&path)?;
    expect_data_custody_refusal(&fixture, "sealed manifest missing", |failure| {
        matches!(failure,
            Failure::Canonical(CoreError::Typed { code: GenerationManifestMissing, message })
                if message.contains("must be rebuilt"))
    })?;
    let saved = fixture.lexical_root.join("saved-sealed-manifest.cbor");
    fs::write(&saved, &original)?;
    symlink(&saved, &path)?;
    expect_data_custody_refusal(&fixture, "sealed manifest symlink", |failure| {
        matches!(failure,
            Failure::Canonical(CoreError::Typed { code: GenerationSidecarCorrupt, message })
                if message.contains("not a regular file"))
    })?;
    fs::remove_file(&path)?;
    fs::remove_file(saved)?;
    fs::write(path, original)?;

    let (mut budget, memory) = original_budget()?;
    fixture.verify(&mut budget)?;
    assert_eq!(memory.bytes.load(Ordering::SeqCst), 0);
    assert!(!memory.bad_release.load(Ordering::SeqCst));
    Ok(())
}

#[test]
fn lexical_existing_refuses_non_directory_text_authority_parent_and_restores() -> TestResult {
    use quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt;
    use quanta_index_core::CoreError;
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new()?;
    for directory in ["file-authority", "text-authority"] {
        let path = fixture.generation_dir().join(directory);
        let saved = fixture.lexical_root.join(format!("saved-{directory}"));
        fs::rename(&path, &saved)?;
        let missing_parent = |failure: &Failure| {
            matches!(failure,
                Failure::Canonical(CoreError::Typed { code: GenerationSidecarCorrupt, message })
                    if message.contains(directory) && message.contains("directory is missing"))
        };
        let non_directory_parent = |failure: &Failure| {
            matches!(failure,
                Failure::Canonical(CoreError::Typed { code: GenerationSidecarCorrupt, message })
                    if message.contains(directory) && message.contains("not a directory"))
        };

        expect_data_custody_refusal(
            &fixture,
            &format!("missing {directory} parent"),
            missing_parent,
        )?;
        symlink(format!("missing-{directory}-target"), &path)?;
        expect_data_custody_refusal(
            &fixture,
            &format!("dangling {directory} parent"),
            non_directory_parent,
        )?;
        fs::remove_file(&path)?;
        fs::write(&path, b"not a directory")?;
        expect_data_custody_refusal(
            &fixture,
            &format!("regular {directory} parent"),
            non_directory_parent,
        )?;
        fs::remove_file(&path)?;
        fs::rename(saved, path)?;
    }

    let (mut budget, memory) = original_budget()?;
    fixture.verify(&mut budget)?;
    assert_eq!(memory.bytes.load(Ordering::SeqCst), 0);
    assert!(!memory.bad_release.load(Ordering::SeqCst));
    Ok(())
}

fn committed_names(
    manifest: &ciborium::Value,
    field: usize,
    what: &str,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let list = manifest
        .as_array()
        .and_then(|fields| fields.get(field))
        .and_then(ciborium::Value::as_array)
        .filter(|rows| !rows.is_empty())
        .ok_or_else(|| format!("fixture manifest has no committed {what} list"))?;
    list.iter()
        .map(|row| {
            row.as_array()
                .and_then(|fields| fields.first())
                .and_then(ciborium::Value::as_text)
                .map(str::to_string)
                .ok_or_else(|| format!("fixture {what} commitment has no name").into())
        })
        .collect()
}

#[test]
fn lexical_existing_refuses_changed_or_uncommitted_artifacts() -> TestResult {
    use quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt;
    use quanta_index_core::CoreError;
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new()?;
    let manifest_path = fixture.sealed_manifest_path();
    let generation = manifest_path
        .parent()
        .ok_or("fixture has no generation directory")?;
    let manifest: ciborium::Value = ciborium::from_reader(fs::read(&manifest_path)?.as_slice())?;
    let (mut budget, _) = original_budget()?;
    fixture.verify(&mut budget)?;

    let corrupt = |failure: &Failure| {
        matches!(
            failure,
            Failure::Canonical(CoreError::Typed {
                code: GenerationSidecarCorrupt,
                ..
            })
        )
    };
    let missing =
        |failure: &Failure| matches!(failure, Failure::AdmissionOrIo(IndexError::Storage(_)));

    let meta = generation.join("meta.json");
    let original_meta = fs::read(&meta)?;
    let mut whitespace = original_meta.clone();
    whitespace.push(b'\n');
    fs::write(&meta, whitespace)?;
    expect_data_custody_refusal(&fixture, "same commit JSON with different bytes", corrupt)?;
    File::options()
        .write(true)
        .open(&meta)?
        .set_len(16 * 1024 * 1024 + 1)?;
    expect_data_custody_refusal(&fixture, "physical index commit above its ceiling", corrupt)?;
    fs::write(&meta, original_meta)?;

    let native_root = generation.join("text-authority/manifest.cbor");
    let original_root = fs::read(&native_root)?;
    File::options()
        .write(true)
        .open(&native_root)?
        .set_len(16 * 1024 + 1)?;
    expect_data_custody_refusal(
        &fixture,
        "physical Native root above its ceiling",
        |failure| matches!(failure, Failure::AdmissionOrIo(IndexError::DigestConflict)),
    )?;
    fs::write(native_root, original_root)?;

    let ranked_names = committed_names(&manifest, 6, "ranked-key")?;
    let mut payloads = committed_names(&manifest, 5, "segment")?;
    payloads.extend(ranked_names.iter().cloned());
    payloads.push("search-corpus-live-bm25.cbor".to_string());
    for name in payloads {
        let path = generation.join(&name);
        let original = fs::read(&path)?;
        assert!(
            !original.is_empty(),
            "{name}: fixture committed an empty file"
        );
        fs::remove_file(&path)?;
        expect_data_custody_refusal(&fixture, &format!("{name} missing"), missing)?;
        fs::write(&path, [])?;
        expect_data_custody_refusal(&fixture, &format!("{name} truncated"), corrupt)?;
        let mut longer = original.clone();
        longer.push(0);
        fs::write(&path, longer)?;
        expect_data_custody_refusal(&fixture, &format!("{name} length increased"), corrupt)?;
        let mut changed = original.clone();
        *changed.first_mut().ok_or("empty committed payload")? ^= 1;
        fs::write(&path, changed)?;
        expect_data_custody_refusal(&fixture, &format!("{name} digest changed"), corrupt)?;
        fs::write(path, original)?;
    }

    let repo_bytes = fixture.expected.repo_id.as_str().as_bytes();
    let mut ranked_key_target: Option<(PathBuf, Vec<u8>, usize)> = None;
    for name in &ranked_names {
        let path = generation.join(name);
        let original = fs::read(&path)?;
        for (offset, bytes) in original.windows(repo_bytes.len()).enumerate() {
            if bytes != repo_bytes {
                continue;
            }
            if ranked_key_target.is_some() {
                return Err("fixture contains the target repository key more than once".into());
            }
            ranked_key_target = Some((path.clone(), original.clone(), offset));
        }
    }
    let (path, original, offset) = ranked_key_target.ok_or("ranked-key repository key missing")?;
    let mut changed_key = original.clone();
    let first = changed_key
        .get_mut(offset)
        .ok_or("ranked-key repository key offset is invalid")?;
    if *first != b'l' {
        return Err("fixture repository key does not start with l".into());
    }
    *first = b'm';
    fs::write(&path, changed_key)?;
    expect_data_custody_refusal(
        &fixture,
        "same-length structurally valid ranked-key replacement",
        corrupt,
    )?;
    fs::write(path, original)?;

    for name in [
        "repo-metadata.cbor",
        "repo-commit-recency.cbor",
        "repo-meta.cbor",
        "repo-topic.cbor",
        "repo-description.cbor",
        "file-ownership.cbor",
        "file-contributor.cbor",
    ] {
        let extra = generation.join(name);
        assert!(!extra.exists(), "fixture already contains {name}");
        let uncommitted_overlay = |failure: &Failure| {
            matches!(failure,
            Failure::Canonical(CoreError::Typed { code: GenerationSidecarCorrupt, message })
                if message.contains("uncommitted overlay"))
        };
        fs::write(&extra, [0x80])?;
        expect_data_custody_refusal(&fixture, name, uncommitted_overlay)?;
        fs::remove_file(&extra)?;
        symlink("missing-overlay-target", &extra)?;
        expect_data_custody_refusal(&fixture, name, uncommitted_overlay)?;
        fs::remove_file(extra)?;
    }

    let uncommitted_ranked = |failure: &Failure| {
        matches!(failure,
        Failure::Canonical(CoreError::Typed { code: GenerationSidecarCorrupt, message })
            if message.contains("uncommitted ranked-key file"))
    };
    let extra_ranked = generation.join("ranked-keys-ffffffffffffffffffffffffffffffff.bin");
    assert!(!extra_ranked.exists());
    fs::write(&extra_ranked, [0u8])?;
    expect_data_custody_refusal(&fixture, "uncommitted ranked-key", uncommitted_ranked)?;
    fs::remove_file(extra_ranked)?;
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::ffi::OsStringExt as _;

        let invalid_name = std::ffi::OsString::from_vec(b"ranked-keys-\xff.bin".to_vec());
        let invalid_extra = generation.join(invalid_name);
        fs::write(&invalid_extra, [0u8])?;
        expect_data_custody_refusal(
            &fixture,
            "uncommitted non-UTF-8 ranked-key",
            uncommitted_ranked,
        )?;
        fs::remove_file(invalid_extra)?;
    }

    let (mut budget, memory) = original_budget()?;
    fixture.verify(&mut budget)?;
    assert_eq!(memory.bytes.load(Ordering::SeqCst), 0);
    assert!(!memory.bad_release.load(Ordering::SeqCst));
    Ok(())
}

#[test]
fn lexical_existing_refuses_committed_native_root_and_overlays_from_real_old_q() -> TestResult {
    use quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt;
    use quanta_index_core::CoreError;

    let fixture = Fixture::with_overlays("one")?;
    let donor = Fixture::with_overlays("two")?;
    let generation = fixture.generation_dir();
    let donor_generation = donor.generation_dir();
    let manifest: ciborium::Value =
        ciborium::from_reader(fs::read(fixture.sealed_manifest_path())?.as_slice())?;
    let overlay_names = committed_names(&manifest, 10, "overlay")?;
    let expected_overlay_names = OVERLAY_NAMES.map(str::to_string).to_vec();
    if overlay_names != expected_overlay_names {
        return Err(
            format!("old-Q fixture did not commit every overlay: {overlay_names:?}").into(),
        );
    }

    let corrupt = |failure: &Failure| {
        matches!(
            failure,
            Failure::Canonical(CoreError::Typed {
                code: GenerationSidecarCorrupt,
                ..
            })
        )
    };
    let stale_root_refusal = |failure: &Failure| {
        corrupt(failure) || matches!(failure, Failure::AdmissionOrIo(IndexError::DigestConflict))
    };
    for (authority, relative) in [
        ("File root", "file-authority/root.cbor"),
        ("Native root", "text-authority/manifest.cbor"),
    ] {
        let path = generation.join(relative);
        let donor_path = donor_generation.join(relative);
        let original = fs::read(&path)?;
        let stale = fs::read(&donor_path)?;
        // The bit-flip case below proves same-length digest binding. This
        // donor separately proves refusal of an authentic stale old-Q row;
        // canonical rows may have different encoded lengths.
        if original == stale {
            return Err(format!("old-Q donor {authority} is not a distinct artifact").into());
        }
        fs::remove_file(&path)?;
        expect_data_custody_refusal(&fixture, &format!("committed {authority} missing"), corrupt)?;
        fs::write(
            &path,
            original
                .get(..original.len().div_euclid(2))
                .ok_or("authority root truncation range missing")?,
        )?;
        expect_data_custody_refusal(
            &fixture,
            &format!("committed {authority} truncated"),
            corrupt,
        )?;
        let mut flipped = original.clone();
        let middle = flipped.len().div_euclid(2);
        *flipped.get_mut(middle).ok_or("authority root is empty")? ^= 1;
        fs::write(&path, flipped)?;
        expect_data_custody_refusal(
            &fixture,
            &format!("committed {authority} bit-flipped"),
            corrupt,
        )?;
        fs::write(&path, stale)?;
        // A longer authentic stale root is refused by the physical committed
        // read bound before hashing; equal/shorter stale bytes reach the typed
        // seal mismatch. Both are custody refusals, and the bit-flip case above
        // independently fixes the same-length typed-digest contract.
        expect_data_custody_refusal(
            &fixture,
            &format!("committed {authority} stale"),
            stale_root_refusal,
        )?;
        fs::remove_file(&path)?;
        std::os::unix::fs::symlink(&donor_path, &path)?;
        expect_data_custody_refusal(&fixture, &format!("committed {authority} symlink"), corrupt)?;
        fs::remove_file(&path)?;
        fs::write(path, original)?;
    }

    let missing =
        |failure: &Failure| matches!(failure, Failure::AdmissionOrIo(IndexError::Storage(_)));
    for name in overlay_names {
        let path = generation.join(&name);
        let donor_path = donor_generation.join(&name);
        let original = fs::read(&path)?;
        let stale = fs::read(donor_path)?;
        // Same-length corruption is covered by the bit flip; this donor is
        // required to be authentic and distinct, not serializer-length equal.
        if original == stale {
            return Err(format!("{name}: old-Q donor is not a distinct overlay").into());
        }
        fs::remove_file(&path)?;
        expect_data_custody_refusal(&fixture, &format!("{name} missing"), missing)?;
        fs::write(
            &path,
            original
                .get(..original.len().div_euclid(2))
                .ok_or("overlay truncation range missing")?,
        )?;
        expect_data_custody_refusal(&fixture, &format!("{name} truncated"), corrupt)?;
        let mut flipped = original.clone();
        let middle = flipped.len().div_euclid(2);
        *flipped.get_mut(middle).ok_or("overlay is empty")? ^= 1;
        fs::write(&path, flipped)?;
        expect_data_custody_refusal(&fixture, &format!("{name} bit-flipped"), corrupt)?;
        fs::write(&path, stale)?;
        expect_data_custody_refusal(&fixture, &format!("{name} stale"), corrupt)?;
        fs::write(path, original)?;
    }

    let (mut budget, memory) = original_budget()?;
    fixture.verify(&mut budget)?;
    assert_eq!(memory.bytes.load(Ordering::SeqCst), 0);
    assert!(!memory.bad_release.load(Ordering::SeqCst));
    Ok(())
}

#[test]
fn lexical_existing_binds_missing_changed_and_uncommitted_coverage_pages() -> TestResult {
    use quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt;
    use quanta_index_core::CoreError;

    let fixture = Fixture::new()?;
    let generation = fixture.generation_dir();
    let mut pages = fs::read_dir(&generation)?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("source-file-coverage-page-")
        })
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    pages.sort();
    let page = pages
        .first()
        .ok_or("old-Q fixture has no committed coverage page")?;
    let original = fs::read(page)?;
    let corrupt = |failure: &Failure| {
        matches!(
            failure,
            Failure::Canonical(CoreError::Typed {
                code: GenerationSidecarCorrupt,
                ..
            })
        )
    };
    let missing =
        |failure: &Failure| matches!(failure, Failure::AdmissionOrIo(IndexError::Storage(_)));

    fs::remove_file(page)?;
    expect_data_custody_refusal(&fixture, "committed coverage page missing", missing)?;
    let mut changed = original.clone();
    *changed
        .last_mut()
        .ok_or("committed coverage page is empty")? ^= 1;
    fs::write(page, changed)?;
    expect_data_custody_refusal(&fixture, "committed coverage page changed", corrupt)?;
    fs::write(page, &original)?;

    let extra = generation.join("source-file-coverage-page-uncommitted.cbor");
    fs::write(&extra, &original)?;
    expect_data_custody_refusal(&fixture, "uncommitted coverage page", |failure| {
        matches!(failure,
            Failure::Canonical(CoreError::Typed { code: GenerationSidecarCorrupt, message })
                if message.contains("uncommitted coverage page"))
    })?;
    fs::remove_file(extra)?;

    let (mut budget, memory) = original_budget()?;
    fixture.verify(&mut budget)?;
    assert_eq!(memory.bytes.load(Ordering::SeqCst), 0);
    assert!(!memory.bad_release.load(Ordering::SeqCst));
    Ok(())
}

#[test]
fn lexical_existing_source_export_loans_full_foreign_binary_and_text_census() -> TestResult {
    let fixture = Fixture::new()?;
    let (mut budget, memory) = original_budget()?;
    let began = Cell::new(false);
    let mut seen_text = false;
    let mut seen_binary = false;
    let count = visit_existing_complete_source_files_v1(
        &fixture.lexical_root,
        &fixture.expected,
        fixture.owner,
        fixture.maximum_objects,
        &fixture.files,
        &fixture.files,
        &mut budget,
        &mut |event, source_heads, _| -> Result<(), std::io::Error> {
            assert_eq!(event.event_id, "event-1");
            assert_eq!(source_heads.entries, 2);
            assert!(source_heads.root_sha256.is_some());
            began.set(true);
            Ok(())
        },
        &mut |source, text_admitted, bytes, _| -> Result<(), std::io::Error> {
            assert!(began.get());
            match source.file.repo_relative_path.as_str() {
                "src/lib.rs" => {
                    assert!(text_admitted);
                    assert_eq!(bytes, b"fn main() { legacy_existing_needle(); }");
                    seen_text = true;
                }
                "assets/data.bin" => {
                    assert!(!text_admitted);
                    assert_eq!(
                        source.file.source_repo_id.as_str(),
                        "legacy-existing-foreign-source"
                    );
                    assert_eq!(bytes.len(), 80 * 1024);
                    assert!(bytes.iter().all(|byte| *byte == 0xff));
                    seen_binary = true;
                }
                _ => panic!("unexpected Source file"),
            }
            Ok(())
        },
    )?;
    assert_eq!(count, 2);
    assert!(began.get() && seen_text && seen_binary);
    assert_eq!(memory.bytes.load(Ordering::SeqCst), 0);
    assert!(!memory.bad_release.load(Ordering::SeqCst));
    Ok(())
}

#[test]
fn lexical_existing_source_export_rechecks_generation_after_file_callback() -> TestResult {
    use quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt;
    use quanta_index_core::CoreError;

    let fixture = Fixture::new()?;
    let meta = fixture.generation_dir().join("meta.json");
    let original = fs::read(&meta)?;
    let mut changed = original.clone();
    changed.push(b'\n');
    let callback_ran = Cell::new(false);
    let (mut budget, memory) = original_budget()?;
    let exported = visit_existing_complete_source_files_v1(
        &fixture.lexical_root,
        &fixture.expected,
        fixture.owner,
        fixture.maximum_objects,
        &fixture.files,
        &fixture.files,
        &mut budget,
        &mut |_, _, _| -> Result<(), std::io::Error> { Ok(()) },
        &mut |_, _, _, _| -> Result<(), std::io::Error> {
            if !callback_ran.replace(true) {
                fs::write(&meta, &changed)?;
            }
            Ok(())
        },
    );
    assert!(callback_ran.get());
    assert!(
        matches!(
            exported,
            Err(SourceVisitFailureV1::Physical(Failure::Canonical(
                CoreError::Typed {
                    code: GenerationSidecarCorrupt,
                    ..
                }
            )))
        ),
        "Source export promoted callbacks after committed metadata changed: {exported:?}"
    );
    assert_eq!(fs::read(&meta)?, changed);
    assert_eq!(memory.bytes.load(Ordering::SeqCst), 0);
    assert!(!memory.bad_release.load(Ordering::SeqCst));

    fs::write(&meta, original)?;
    let (mut restored_budget, restored_memory) = original_budget()?;
    let restored = visit_existing_complete_source_files_v1(
        &fixture.lexical_root,
        &fixture.expected,
        fixture.owner,
        fixture.maximum_objects,
        &fixture.files,
        &fixture.files,
        &mut restored_budget,
        &mut |_, _, _| -> Result<(), std::io::Error> { Ok(()) },
        &mut |_, _, _, _| -> Result<(), std::io::Error> { Ok(()) },
    )?;
    assert_eq!(restored, 2);
    assert_eq!(restored_memory.bytes.load(Ordering::SeqCst), 0);
    assert!(!restored_memory.bad_release.load(Ordering::SeqCst));
    Ok(())
}

#[test]
fn lexical_existing_source_export_refuses_corrupt_physical_body() -> TestResult {
    use quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt;
    use quanta_index_core::CoreError;

    let fixture = Fixture::new()?;
    let pool = fixture.lexical_root.join("lexical-object-pool/objects");
    let mut pending = vec![pool];
    let mut changed = None;
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                pending.push(entry.path());
                continue;
            }
            let bytes = fs::read(entry.path())?;
            if bytes.get(10) != Some(&1) {
                continue; // Canonical lexical object kind 1 is FileBody.
            }
            if let Some(offset) = bytes
                .windows(b"legacy_existing_needle".len())
                .position(|window| window == b"legacy_existing_needle")
            {
                let mut tampered = bytes.clone();
                tampered[offset] ^= 1;
                let path = entry.path();
                fs::write(&path, tampered)?;
                changed = Some((path, bytes));
                break;
            }
        }
        if changed.is_some() {
            break;
        }
    }
    let (path, original) = changed.ok_or("fixture must retain the exact original text body")?;
    expect_data_custody_refusal(
        &fixture,
        "same-length pooled FileBody corruption",
        |failure| {
            matches!(
                failure,
                Failure::Canonical(CoreError::Typed {
                    code: GenerationSidecarCorrupt,
                    ..
                })
            )
        },
    )?;
    fs::write(path, original)?;
    let (mut restored_budget, restored_memory) = original_budget()?;
    fixture.verify(&mut restored_budget)?;
    assert_eq!(restored_memory.bytes.load(Ordering::SeqCst), 0);
    assert!(!restored_memory.bad_release.load(Ordering::SeqCst));
    Ok(())
}

#[test]
fn lexical_existing_source_export_refuses_unfunded_original_budget() -> TestResult {
    let fixture = Fixture::new()?;
    let (_, memory) = original_budget()?;
    let mut budget = WorkBudget::new(
        WorkLimits {
            reads: 0,
            writes: 0,
            bytes: 0,
            mutations: 0,
        },
        Instant::now()
            .checked_add(Duration::from_secs(60))
            .ok_or("deadline overflow")?,
    );
    budget.bind_memory_scope(Some(MemoryScope::new(Arc::new(FiniteMemory(
        memory.clone(),
    )))));
    let mut began = false;
    let result = visit_existing_complete_source_files_v1(
        &fixture.lexical_root,
        &fixture.expected,
        fixture.owner,
        fixture.maximum_objects,
        &fixture.files,
        &fixture.files,
        &mut budget,
        &mut |_, _, _| -> Result<(), std::io::Error> {
            began = true;
            Ok(())
        },
        &mut |_, _, _, _| -> Result<(), std::io::Error> { Ok(()) },
    );
    assert!(result.is_err());
    assert!(!began);
    assert_eq!(memory.bytes.load(Ordering::SeqCst), 0);
    Ok(())
}

#[test]
fn lexical_existing_same_length_coverage_tamper_refuses() -> TestResult {
    let fixture = Fixture::new()?;
    let generation = quanta_index_core::GenerationStorageKeyV1::for_repo_revision(
        &fixture.expected.repo_id,
        &fixture.expected.revision_id,
    )
    .generation_dir(&fixture.lexical_root, fixture.expected.manifest_generation);
    let path = generation.join("source-file-coverage.cbor");
    let (mut baseline_budget, baseline_memory) = original_budget()?;
    fixture.verify(&mut baseline_budget)?;
    if baseline_memory.bytes.load(Ordering::SeqCst) != 0
        || baseline_memory.bad_release.load(Ordering::SeqCst)
    {
        return Err("intact baseline did not refund original memory".into());
    }
    let original = fs::read(&path)?;
    let mut bytes = original.clone();
    let last = bytes.last_mut().ok_or("empty coverage root")?;
    *last ^= 1;
    fs::write(&path, bytes)?;
    expect_data_custody_refusal(&fixture, "same-length coverage root tamper", |failure| {
        matches!(
            failure,
            Failure::Canonical(quanta_index_core::CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
                ..
            })
        )
    })?;
    fs::write(path, original)?;
    let (mut restored_budget, restored_memory) = original_budget()?;
    fixture.verify(&mut restored_budget)?;
    assert_eq!(restored_memory.bytes.load(Ordering::SeqCst), 0);
    assert!(!restored_memory.bad_release.load(Ordering::SeqCst));
    Ok(())
}

#[test]
fn lexical_existing_identity_digest_mismatch_refuses_and_restores() -> TestResult {
    let fixture = Fixture::new()?;
    let generation = quanta_index_core::GenerationStorageKeyV1::for_repo_revision(
        &fixture.expected.repo_id,
        &fixture.expected.revision_id,
    )
    .generation_dir(&fixture.lexical_root, fixture.expected.manifest_generation);
    let identity_path = generation.join("search-corpus-generation-identity.cbor");
    let original = fs::read(&identity_path)?;

    let (mut baseline_budget, baseline_memory) = original_budget()?;
    fixture.verify(&mut baseline_budget)?;
    assert_eq!(baseline_memory.bytes.load(Ordering::SeqCst), 0);
    assert!(!baseline_memory.bad_release.load(Ordering::SeqCst));

    let mut forged = fixture.expected.clone();
    forged.manifest_digest.push_str("-wrong");
    let mut forged_bytes = Vec::new();
    ciborium::into_writer(&forged, &mut forged_bytes)?;
    fs::write(&identity_path, forged_bytes)?;

    expect_data_custody_refusal(&fixture, "forged sealed identity", |failure| {
        matches!(
            failure,
            Failure::Canonical(quanta_index_core::CoreError::Typed {
                code:
                    quanta_index_contract::SearchPlaneErrorCodeV2::GenerationIdentityDigestMismatch,
                ..
            })
        )
    })?;

    fs::write(&identity_path, original)?;
    let (mut restored_budget, restored_memory) = original_budget()?;
    fixture.verify(&mut restored_budget)?;
    assert_eq!(restored_memory.bytes.load(Ordering::SeqCst), 0);
    assert!(!restored_memory.bad_release.load(Ordering::SeqCst));
    Ok(())
}
