//! Existing Lexical physical verifier component proof with real temporary files.
//! These finite ports do not supply installed Legacy root or publication authority.
#![cfg(feature = "legacy-tantivy")]
#![forbid(unsafe_code)]

#[path = "support/publication_fixture.rs"]
mod publication_fixture;
#[path = "support/source_fixture.rs"]
mod source_fixture;

use publication_fixture::FixtureBatchBuildPort;
use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{
    ChunkId, ChunkRecord, GenerationSnapshot, ManifestGeneration, RepoId, RepoRelativePath,
    RevisionId, SearchPlaneTrackKind,
};
use quanta_index_core::index::{
    IndexError, MemoryLease, MemoryLeaseOwner, MemoryLifetime, MemoryScope, MemoryScopePort,
    ObjectReadFile, ObjectReadIoPort, WorkBudget, WorkLimits, WorkUsage,
    existing_file::ExistingFileReadFailureV1 as Failure,
};
use quanta_index_core::{ExistingTreeEntryKindV1, ExistingTreeMetadataIoPortV1};
use quanta_index_lexical::{LexicalAdapter, legacy::verify_existing_lexical_generation_read_only};
use std::{
    ffi::OsStr,
    fs::{self, File},
    os::unix::fs::MetadataExt as _,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

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
struct Fixture {
    root: tempfile::TempDir,
    expected: GenerationSnapshot,
    owner: u32,
    maximum_objects: usize,
    files: Files,
}
impl Fixture {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let repo = RepoId::new("legacy-existing-repo")?;
        let revision = RevisionId::new("legacy-existing-revision")?;
        let generation = ManifestGeneration::new(1);
        let body = "fn main() { legacy_existing_needle(); }";
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
                chunk_id: ChunkId::new("legacy-existing-chunk"),
                repo_relative_path: RepoRelativePath::new("src/lib.rs"),
                language,
                start_byte: 0,
                end_byte: u32::try_from(body.len())?,
                start_line: 1,
                end_line: 1,
                text: body.to_owned().into_boxed_str(),
                structural: None,
                parent_chunk_id: None,
                source_repo_id: None,
            }],
            Vec::new(),
        )?;
        let batch = source_fixture::sealed_batch(&repo, &revision, generation, vec![scope])?;
        let adapter = LexicalAdapter::with_state_root(root.path().to_path_buf());
        let _stage = adapter.build_fixture_batch(&batch)?;
        let owner = fs::metadata(root.path())?.uid();
        let maximum_objects = physical_file_count(root.path())?;
        let files = Files {
            root: root.path().to_path_buf(),
            opens: AtomicU64::new(0),
            visits: AtomicU64::new(0),
        };
        let expected = GenerationSnapshot {
            repo_id: repo,
            revision_id: revision,
            track: SearchPlaneTrackKind::Lexical,
            manifest_generation: generation,
            manifest_digest: batch.manifest_digest,
        };
        Ok(Self {
            root,
            expected,
            owner,
            maximum_objects,
            files,
        })
    }
    fn verify(&self, budget: &mut WorkBudget) -> Result<(), Failure> {
        verify_existing_lexical_generation_read_only(
            self.root.path(),
            &self.expected,
            self.owner,
            self.maximum_objects,
            &self.files,
            &self.files,
            budget,
        )
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
fn lexical_existing_same_length_coverage_tamper_refuses() -> TestResult {
    let fixture = Fixture::new()?;
    let generation = quanta_index_core::GenerationStorageKeyV1::for_repo_revision(
        &fixture.expected.repo_id,
        &fixture.expected.revision_id,
    )
    .generation_dir(fixture.root.path(), fixture.expected.manifest_generation);
    let path = generation.join("source-file-coverage.cbor");
    let (mut baseline_budget, baseline_memory) = original_budget()?;
    fixture.verify(&mut baseline_budget)?;
    if baseline_memory.bytes.load(Ordering::SeqCst) != 0
        || baseline_memory.bad_release.load(Ordering::SeqCst)
    {
        return Err("intact baseline did not refund original memory".into());
    }
    let mut bytes = fs::read(&path)?;
    let last = bytes.last_mut().ok_or("empty coverage root")?;
    *last ^= 1;
    fs::write(path, bytes)?;
    let (mut budget, memory) = original_budget()?;
    let result = fixture.verify(&mut budget);
    if !matches!(
        result,
        Err(Failure::Canonical(quanta_index_core::CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
            ..
        }))
    ) {
        return Err(
            format!("tampered coverage root did not retain typed corruption: {result:?}").into(),
        );
    }
    if memory.bytes.load(Ordering::SeqCst) != 0 || memory.bad_release.load(Ordering::SeqCst) {
        return Err("coverage refusal leaked original allocation".into());
    }
    Ok(())
}
