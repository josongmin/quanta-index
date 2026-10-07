//! Fixed-source, independently decoded on-disk publication oracles.
//!
//! No F15 producer, root decoder, codec decoder, or trigram helper constructs
//! the expected bytes or memberships. SIGKILL establishes process-crash
//! recovery only; it does not establish power-loss durability.

#![expect(
    clippy::panic_in_result_fn,
    clippy::print_stderr,
    reason = "fixed-source assertions reject incomplete publication; stderr identifies the last real cut and child cleanup errors"
)]

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::io::{Cursor, Read};
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{
    BatchIngestMode, ChunkId, ChunkRecord, GenerationSnapshot, LQ_VERSION_TAG, LqExpr, LqFilter,
    LqLeaf, LqOptions, LqPatternType, LqQuery, LqSelect, LqSpan, ManifestGeneration,
    QueryConstraintSetV1, RepoId, RepoRelativePath, RevisionId, SearchCorpusIngestBatch,
    SearchCorpusReplaceScope, SearchCorpusTombstoneScope, SearchPlaneTrackKind, SourceFileKey,
    SourceFileRevision,
};
use quanta_index_core::{
    GenerationIdentityValidatePort, GenerationStorageKeyV1, LexicalIndexOpenPort, LexicalPageSpec,
    RequestBudgetV1, SearchCorpusBatchBuildPort,
};
use sha2::{Digest as _, Sha256};

use super::{Case, Cut, Guard, Mode, Side};
use crate::LexicalAdapter;

#[path = "../../tests/support/current_source_fixture.rs"]
mod current_source_fixture;

type TestResult = Result<(), Box<dyn Error>>;
type Truth = BTreeMap<String, Vec<u8>>;
type SourceWire = (
    SourceFileRevision,
    bool,
    LanguageCode,
    u32,
    u64,
    u64,
    u64,
    [u8; 32],
);
type PartitionWire = (u16, [u8; 32], [u8; 32], u64, u64, u32);
type RootWire = (
    u32,
    [u8; 32],
    u64,
    Vec<SourceWire>,
    Vec<PartitionWire>,
    Vec<PartitionWire>,
    Vec<PartitionWire>,
);
type Postings = BTreeMap<[u8; 3], BTreeSet<u64>>;
type CustodyTree = BTreeMap<PathBuf, (u64, u64, [u8; 32])>;

const KEEP: &str = "src/keep.rs";
const EDIT: &str = "src/edit.rs";
const RETIRE: &str = "src/retire.rs";
const KEEP_BODY: &str = "keepneedle stable\n";
const OLD_BODY: &str = "oldneedle original\n";
const NEW_BODY: &str = "newneedle updated\n";
const RETIRE_BODY: &str = "retiredneedle gone\n";
const CHILD_TEST: &str = "publication_faults::tests::publication_crash_child";

fn truth(updated: bool) -> Truth {
    let mut rows = BTreeMap::from([
        (KEEP.to_string(), KEEP_BODY.as_bytes().to_vec()),
        (
            EDIT.to_string(),
            if updated { NEW_BODY } else { OLD_BODY }
                .as_bytes()
                .to_vec(),
        ),
    ]);
    if !updated {
        assert!(
            rows.insert(RETIRE.to_string(), RETIRE_BODY.as_bytes().to_vec())
                .is_none()
        );
    }
    rows
}

fn repo() -> Result<RepoId, Box<dyn Error>> {
    Ok(RepoId::new("f15-publication")?)
}
fn revision() -> Result<RevisionId, Box<dyn Error>> {
    Ok(RevisionId::new("revision")?)
}

fn source_scope(path: &str, body: &str) -> Result<SearchCorpusReplaceScope, Box<dyn Error>> {
    let language = LanguageCode::new("rust")?;
    let chunk = ChunkRecord {
        chunk_id: ChunkId::new(format!("publication-{path}")),
        repo_relative_path: RepoRelativePath::new(path),
        language: language.clone(),
        start_byte: 0,
        end_byte: u32::try_from(body.len())?,
        start_line: 1,
        end_line: 1,
        text: body.to_string().into_boxed_str(),
        structural: None,
        parent_chunk_id: None,
        source_repo_id: Some(RepoId::new("source")?),
    };
    current_source_fixture::text_scope(
        &RepoId::new("source")?,
        &RevisionId::new("source-revision")?,
        path,
        language,
        body,
        vec![chunk],
    )
}

fn batch(updated: bool) -> Result<SearchCorpusIngestBatch, Box<dyn Error>> {
    let scopes = if updated {
        vec![source_scope(EDIT, NEW_BODY)?]
    } else {
        vec![
            source_scope(KEEP, KEEP_BODY)?,
            source_scope(EDIT, OLD_BODY)?,
            source_scope(RETIRE, RETIRE_BODY)?,
        ]
    };
    let mut batch = SearchCorpusIngestBatch {
        source_event: current_source_fixture::empty_event(),
        repo_id: repo()?,
        revision_id: revision()?,
        generation: ManifestGeneration::new(if updated { 2 } else { 1 }),
        base_generation: updated.then(|| ManifestGeneration::new(1)),
        manifest_digest: format!("publication-manifest-{}", if updated { 2 } else { 1 }),
        batch_digest: "0".repeat(64),
        mode: if updated {
            BatchIngestMode::Delta
        } else {
            BatchIngestMode::ReplaceGeneration
        },
        bundle_payload: None,
        clear_surfaces: Vec::new(),
        replace_scopes: scopes,
        tombstone_scopes: if updated {
            vec![SearchCorpusTombstoneScope {
                file: SourceFileKey {
                    source_repo_id: RepoId::new("source")?,
                    repo_relative_path: RepoRelativePath::new(RETIRE),
                },
            }]
        } else {
            Vec::new()
        },
        semantic_replace_scopes: Vec::new(),
        semantic_tombstone_scopes: Vec::new(),
        seal: true,
    };
    current_source_fixture::finish_batch(&mut batch)?;
    Ok(batch)
}

fn candidate(updated: bool) -> Result<GenerationSnapshot, Box<dyn Error>> {
    let batch = batch(updated)?;
    Ok(GenerationSnapshot {
        repo_id: batch.repo_id,
        revision_id: batch.revision_id,
        track: SearchPlaneTrackKind::Lexical,
        manifest_generation: batch.generation,
        manifest_digest: batch.manifest_digest,
    })
}

fn generation(root: &Path, updated: bool) -> Result<PathBuf, Box<dyn Error>> {
    Ok(
        GenerationStorageKeyV1::for_repo_revision(&repo()?, &revision()?)
            .generation_dir(root, ManifestGeneration::new(if updated { 2 } else { 1 })),
    )
}

fn cases() -> Vec<Case> {
    let mut result = Vec::new();
    for cut in [
        Cut::InheritedObjectLink,
        Cut::ObjectWrite,
        Cut::ObjectFileSync,
        Cut::ObjectLink,
        Cut::ObjectTemporaryCleanup,
        Cut::ObjectDirectorySync,
        Cut::RootWrite,
        Cut::RootFileSync,
        Cut::RootRename,
        Cut::RootDirectorySync,
        Cut::RootTemporaryCleanup,
        Cut::RootTemporaryDirectorySync,
        Cut::ObsoleteObjectCleanup,
        Cut::StagingCleanup,
        Cut::AuthorityDirectorySync,
    ] {
        for side in [Side::Before, Side::After] {
            result.push(Case {
                cut,
                side,
                occurrence: 1,
            });
            if cut == Cut::ObjectDirectorySync {
                // The second barrier follows orphan cleanup, after root publication.
                result.push(Case {
                    cut,
                    side,
                    occurrence: 2,
                });
            }
        }
    }
    result
}

fn hex(digest: &[u8; 32]) -> String {
    digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .concat()
}

fn word<const N: usize>(reader: &mut impl Read) -> Result<[u8; N], Box<dyn Error>> {
    let mut bytes = [0; N];
    reader.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn source_pack(raw: &[u8]) -> Result<BTreeMap<[u8; 32], Vec<u8>>, Box<dyn Error>> {
    let mut reader = Cursor::new(raw);
    assert_eq!(&word::<8>(&mut reader)?, b"QISPACK1");
    assert_eq!(u16::from_le_bytes(word(&mut reader)?), 1);
    assert_eq!(word::<2>(&mut reader)?, [0, 0]);
    let count = usize::try_from(u32::from_le_bytes(word(&mut reader)?))?;
    assert!(count <= 3, "fixed source pack exceeded fixture universe");
    let payload = usize::try_from(u64::from_le_bytes(word(&mut reader)?))?;
    let mut rows = Vec::new();
    for _ in 0..count {
        rows.push((
            word::<32>(&mut reader)?,
            usize::try_from(u64::from_le_bytes(word(&mut reader)?))?,
            usize::try_from(u32::from_le_bytes(word(&mut reader)?))?,
        ));
    }
    let start = usize::try_from(reader.position())?;
    assert_eq!(start.checked_add(payload), Some(raw.len()));
    let mut result = BTreeMap::new();
    let mut next_offset = 0;
    for (digest, offset, length) in rows {
        assert_eq!(offset, next_offset);
        let from = start.checked_add(offset).ok_or("pack offset overflow")?;
        let to = from.checked_add(length).ok_or("pack length overflow")?;
        let body = raw.get(from..to).ok_or("pack body is truncated")?;
        assert_eq!(<[u8; 32]>::from(Sha256::digest(body)), digest);
        assert!(result.insert(digest, body.to_vec()).is_none());
        next_offset = offset.checked_add(length).ok_or("pack payload overflow")?;
    }
    assert_eq!(next_offset, payload);
    Ok(result)
}

fn posting_block(raw: &[u8], surface: u8) -> Result<Postings, Box<dyn Error>> {
    let mut reader = Cursor::new(raw);
    assert_eq!(&word::<8>(&mut reader)?, b"QIPOST02");
    assert_eq!(u16::from_le_bytes(word(&mut reader)?), 2);
    assert_eq!(word::<2>(&mut reader)?, [surface, 0]);
    let count = usize::try_from(u32::from_le_bytes(word(&mut reader)?))?;
    assert!(
        count <= 64,
        "fixed posting block exceeded fixture gram universe"
    );
    let memberships = usize::try_from(u64::from_le_bytes(word(&mut reader)?))?;
    let payload = usize::try_from(u64::from_le_bytes(word(&mut reader)?))?;
    assert_eq!(memberships.checked_mul(8), Some(payload));
    let mut rows = Vec::new();
    for _ in 0..count {
        let gram = word::<3>(&mut reader)?;
        assert_eq!(word::<1>(&mut reader)?, [0]);
        rows.push((
            gram,
            usize::try_from(u64::from_le_bytes(word(&mut reader)?))?,
            usize::try_from(u32::from_le_bytes(word(&mut reader)?))?,
            word::<32>(&mut reader)?,
        ));
    }
    let start = usize::try_from(reader.position())?;
    assert_eq!(start.checked_add(payload), Some(raw.len()));
    let mut result = BTreeMap::new();
    let mut next_offset = 0;
    for (gram, offset, length, digest) in rows {
        assert_eq!(offset, next_offset);
        assert!(length > 0 && length <= 3);
        let from = start.checked_add(offset).ok_or("posting offset overflow")?;
        let to = from
            .checked_add(length.checked_mul(8).ok_or("ID length overflow")?)
            .ok_or("ID range overflow")?;
        let encoded_ids = raw.get(from..to).ok_or("posting IDs are truncated")?;
        assert_eq!(<[u8; 32]>::from(Sha256::digest(encoded_ids)), digest);
        let mut ids = Cursor::new(encoded_ids);
        let mut values = BTreeSet::new();
        let mut previous = None;
        for _ in 0..length {
            let id = u64::from_le_bytes(word(&mut ids)?);
            assert!(previous.is_none_or(|prior| prior < id));
            assert!(values.insert(id));
            previous = Some(id);
        }
        assert!(result.insert(gram, values).is_none());
        next_offset = offset
            .checked_add(length.checked_mul(8).ok_or("ID offset overflow")?)
            .ok_or("ID offset overflow")?;
    }
    assert_eq!(next_offset, payload);
    Ok(result)
}

fn bucket(source: &SourceFileRevision) -> Result<u8, Box<dyn Error>> {
    let repo = source.file.source_repo_id.as_str().as_bytes();
    let path = source.file.repo_relative_path.as_str().as_bytes();
    let mut hash = Sha256::new();
    hash.update(b"quanta-file-authority-source-key-v15\0");
    hash.update(u64::try_from(repo.len())?.to_le_bytes());
    hash.update(repo);
    hash.update(u64::try_from(path.len())?.to_le_bytes());
    hash.update(path);
    Ok(*hash.finalize().first().ok_or("empty SHA256")?)
}

fn read_partition(directory: &Path, row: &PartitionWire) -> Result<Vec<u8>, Box<dyn Error>> {
    assert_eq!(row.0, 8);
    assert!(row.1.iter().skip(1).all(|byte| *byte == 0));
    let path = directory
        .join("file-authority/objects")
        .join(format!("{}.bin", hex(&row.2)));
    assert!(std::fs::symlink_metadata(&path)?.file_type().is_file());
    let raw = std::fs::read(path)?;
    assert_eq!(u64::try_from(raw.len())?, row.3);
    assert_eq!(<[u8; 32]>::from(Sha256::digest(&raw)), row.2);
    Ok(raw)
}

fn audit_root(directory: &Path, expected: &Truth) -> Result<RootWire, Box<dyn Error>> {
    let raw = std::fs::read(directory.join("file-authority/root.cbor"))?;
    let mut cursor = Cursor::new(&raw);
    let root: RootWire = ciborium::from_reader(&mut cursor)?;
    assert_eq!(usize::try_from(cursor.position())?, raw.len());
    assert_eq!(root.0, 15);
    assert_eq!(root.3.len(), expected.len());
    let mut observed = BTreeSet::new();
    let mut ids = BTreeSet::new();
    let mut packs = BTreeMap::new();
    for pack in &root.4 {
        let decoded = source_pack(&read_partition(directory, pack)?)?;
        assert_eq!(u64::try_from(decoded.len())?, pack.4);
        assert_eq!(pack.5, 0);
        assert!(packs.insert(pack.2, decoded).is_none());
    }
    for row in &root.3 {
        let path = row.0.file.repo_relative_path.as_str();
        assert_eq!(row.0.file.source_repo_id.as_str(), "source");
        assert_eq!(row.0.revision_id.as_str(), "source-revision");
        let body = expected
            .get(path)
            .ok_or("root contains an unexpected source")?;
        assert!(observed.insert(path.to_string()));
        assert!(ids.insert(row.6));
        assert!(row.6 < root.2);
        assert!(row.1);
        assert_eq!(row.2.as_str(), "rust");
        assert_eq!(row.4, u64::try_from(body.len())?);
        let path_grams: BTreeSet<[u8; 3]> = path
            .as_bytes()
            .windows(3)
            .map(TryInto::try_into)
            .collect::<Result<_, _>>()?;
        let body_grams: BTreeSet<[u8; 3]> = body
            .windows(3)
            .map(TryInto::try_into)
            .collect::<Result<_, _>>()?;
        assert_eq!(
            row.3,
            u32::try_from(
                path_grams
                    .len()
                    .checked_add(body_grams.len())
                    .ok_or("fixture membership count overflow")?
            )?
        );
        assert_eq!(row.0.source_sha256, <[u8; 32]>::from(Sha256::digest(body)));
        assert_eq!(
            packs
                .get(&row.7)
                .and_then(|pack| pack.get(&row.0.source_sha256)),
            Some(body)
        );
    }
    assert_eq!(observed, expected.keys().cloned().collect());
    for (surface, partitions) in [(1_u8, &root.5), (2_u8, &root.6)] {
        let expected_buckets: BTreeSet<u8> = root
            .3
            .iter()
            .map(|row| bucket(&row.0))
            .collect::<Result<_, _>>()?;
        let mut observed_buckets = BTreeSet::new();
        for partition in partitions {
            let slot = *partition.1.first().ok_or("empty partition prefix")?;
            assert!(observed_buckets.insert(slot));
            let mut grams: Postings = BTreeMap::new();
            for row in &root.3 {
                if bucket(&row.0)? != slot {
                    continue;
                }
                let path = row.0.file.repo_relative_path.as_str();
                let bytes = if surface == 1 {
                    path.as_bytes()
                } else {
                    expected.get(path).ok_or("missing source truth")?.as_slice()
                };
                for gram in bytes.windows(3) {
                    let _inserted = grams.entry(gram.try_into()?).or_default().insert(row.6);
                }
            }
            assert_eq!(u32::try_from(grams.len())?, partition.5);
            assert_eq!(
                u64::try_from(grams.values().map(BTreeSet::len).sum::<usize>())?,
                partition.4
            );
            assert_eq!(
                posting_block(&read_partition(directory, partition)?, surface)?,
                grams
            );
        }
        assert_eq!(observed_buckets, expected_buckets);
    }
    Ok(root)
}

fn file_tree(directory: &Path) -> Result<CustodyTree, Box<dyn Error>> {
    let mut result = BTreeMap::new();
    let mut pending = vec![directory.to_path_buf()];
    while let Some(path) = pending.pop() {
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() {
                let path = entry.path();
                let meta = entry.metadata()?;
                assert!(
                    result
                        .insert(
                            path.strip_prefix(directory)?.to_path_buf(),
                            (
                                meta.dev(),
                                meta.ino(),
                                Sha256::digest(std::fs::read(&path)?).into(),
                            ),
                        )
                        .is_none()
                );
            } else {
                return Err("unexpected non-regular fixture entry".into());
            }
        }
    }
    Ok(result)
}

fn assert_not_admitted(adapter: &LexicalAdapter) -> TestResult {
    assert!(
        adapter
            .validate_generation_identity(&candidate(true)?)
            .is_err(),
        "activation's physical identity validator admitted an unsealed target"
    );
    assert!(
        adapter
            .open(
                &repo()?,
                &revision()?,
                ManifestGeneration::new(2),
                &RequestBudgetV1::unbounded()
            )
            .is_err(),
        "query open admitted an unsealed target"
    );
    adapter.validate_generation_identity(&candidate(false)?)?;
    assert!(
        adapter
            .open(
                &repo()?,
                &revision()?,
                ManifestGeneration::new(1),
                &RequestBudgetV1::unbounded()
            )
            .is_ok()
    );
    Ok(())
}

fn assert_complete_root_if_published(root: &Path, case: Case) -> TestResult {
    let target = generation(root, true)?;
    // A killed clone can precede the completion of inherited files. That
    // unsealed staging directory is refused by both serving admissions above.
    if case.cut == Cut::InheritedObjectLink {
        return Ok(());
    }
    let bytes = std::fs::read(target.join("file-authority/root.cbor"))?;
    let old_bytes = std::fs::read(generation(root, false)?.join("file-authority/root.cbor"))?;
    if bytes == old_bytes {
        drop(audit_root(&target, &truth(false))?);
    } else {
        drop(audit_root(&target, &truth(true))?);
    }
    Ok(())
}

fn recover(root: &Path, base_tree: &CustodyTree) -> TestResult {
    let adapter = LexicalAdapter::with_state_root(root.to_path_buf());
    assert!(adapter.build_batch(&batch(true)?)?.is_some());
    adapter.validate_generation_identity(&candidate(true)?)?;
    assert!(
        adapter
            .open(
                &repo()?,
                &revision()?,
                ManifestGeneration::new(2),
                &RequestBudgetV1::unbounded()
            )
            .is_ok()
    );
    assert_recovered_queries_match_fresh(&adapter)?;
    let base = generation(root, false)?;
    let target = generation(root, true)?;
    assert_eq!(&file_tree(&base)?, base_tree);
    let old = audit_root(&base, &truth(false))?;
    let new = audit_root(&target, &truth(true))?;
    let expected_objects: BTreeSet<String> = new
        .4
        .iter()
        .chain(&new.5)
        .chain(&new.6)
        .map(|row| format!("{}.bin", hex(&row.2)))
        .collect();
    let actual_objects: BTreeSet<String> =
        std::fs::read_dir(target.join("file-authority/objects"))?
            .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned()))
            .collect::<Result<_, _>>()?;
    assert_eq!(
        actual_objects, expected_objects,
        "unreferenced object survived recovery and seal"
    );
    let authority_entries: BTreeSet<String> = std::fs::read_dir(target.join("file-authority"))?
        .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned()))
        .collect::<Result<_, _>>()?;
    assert_eq!(
        authority_entries,
        BTreeSet::from(["objects".to_string(), "root.cbor".to_string()]),
        "interrupted root temporary or staging survived recovery and seal"
    );
    let kept = |root: &RootWire| {
        root.3
            .iter()
            .find(|row| row.0.file.repo_relative_path.as_str() == KEEP)
            .cloned()
            .ok_or("missing inherited source")
    };
    let before = kept(&old)?;
    let after = kept(&new)?;
    assert_eq!(before.6, after.6, "inherited stable source ID changed");
    assert_eq!(before.7, after.7, "untouched source pack was rebuilt");
    let object = format!("file-authority/objects/{}.bin", hex(&before.7));
    let left = std::fs::metadata(base.join(&object))?;
    let right = std::fs::metadata(target.join(&object))?;
    assert_eq!((left.dev(), left.ino()), (right.dev(), right.ino()));
    let mut inherited_pages = 0_u32;
    for (name, (device, inode, digest)) in base_tree {
        if !name
            .to_string_lossy()
            .starts_with("source-file-coverage-page-")
        {
            continue;
        }
        let path = target.join(name);
        if path.exists() {
            let meta = std::fs::metadata(&path)?;
            assert_eq!((meta.dev(), meta.ino()), (*device, *inode));
            assert_eq!(
                <[u8; 32]>::from(Sha256::digest(std::fs::read(path)?)),
                *digest
            );
            inherited_pages = inherited_pages
                .checked_add(1_u32)
                .ok_or("inherited page count overflow")?;
        }
    }
    assert!(
        inherited_pages > 0,
        "untouched committed coverage page was not inherited"
    );
    Ok(())
}

fn assert_recovered_queries_match_fresh(adapter: &LexicalAdapter) -> TestResult {
    let fresh_root = tempfile::tempdir()?;
    let fresh = LexicalAdapter::with_state_root(fresh_root.path().to_path_buf());
    let mut full = batch(false)?;
    full.generation = ManifestGeneration::new(2);
    full.manifest_digest = "publication-manifest-2".to_string();
    full.replace_scopes = vec![
        source_scope(KEEP, KEEP_BODY)?,
        source_scope(EDIT, NEW_BODY)?,
    ];
    current_source_fixture::finish_batch(&mut full)?;
    assert!(fresh.build_batch(&full)?.is_some());
    let budget = RequestBudgetV1::unbounded();
    let recovered = adapter.open(&repo()?, &revision()?, ManifestGeneration::new(2), &budget)?;
    let rebuilt = fresh.open(&repo()?, &revision()?, ManifestGeneration::new(2), &budget)?;
    for (needle, expected_path) in [
        ("keepneedle", Some(KEEP)),
        ("newneedle", Some(EDIT)),
        ("oldneedle", None),
        ("retiredneedle", None),
    ] {
        for file_surface in [false, true] {
            let mut options = LqOptions::defaults();
            if file_surface {
                options.pattern_type = LqPatternType::CodeSearch;
            }
            let request = LqQuery {
                lq_version: LQ_VERSION_TAG,
                expr: LqExpr::Leaf(if file_surface {
                    LqLeaf::RawString(needle.to_string())
                } else {
                    LqLeaf::Keyword(needle.to_string())
                }),
                filters: if file_surface {
                    vec![LqFilter::Select {
                        dim: LqSelect::File,
                    }]
                } else {
                    Vec::new()
                },
                options,
                directives: Vec::new(),
                source_span: LqSpan::eof(0),
            };
            let observed = recovered
                .search_constrained(
                    &request,
                    &QueryConstraintSetV1::default(),
                    &LexicalPageSpec::first(10),
                    &budget,
                )?
                .candidates;
            let reference = rebuilt
                .search_constrained(
                    &request,
                    &QueryConstraintSetV1::default(),
                    &LexicalPageSpec::first(10),
                    &budget,
                )?
                .candidates;
            assert_eq!(observed.len(), usize::from(expected_path.is_some()));
            assert_eq!(reference.len(), usize::from(expected_path.is_some()));
            for (actual, expected) in observed.iter().zip(&reference) {
                assert_eq!(Some(actual.repo_relative_path.as_str()), expected_path);
                assert_eq!(actual.source_repo_id.as_str(), "source");
                assert_eq!(actual.candidate_id, expected.candidate_id);
                assert_eq!(actual.source, expected.source);
                assert_eq!(actual.score.to_bits(), expected.score.to_bits());
            }
        }
    }
    Ok(())
}

fn setup(root: &Path) -> Result<CustodyTree, Box<dyn Error>> {
    let adapter = LexicalAdapter::with_state_root(root.to_path_buf());
    assert!(adapter.build_batch(&batch(false)?)?.is_some());
    adapter.validate_generation_identity(&candidate(false)?)?;
    drop(audit_root(&generation(root, false)?, &truth(false))?);
    drop(adapter);
    file_tree(&generation(root, false)?)
}

fn prepare_case(root: &Path, case: Case) -> TestResult {
    if matches!(
        case.cut,
        Cut::RootTemporaryCleanup | Cut::RootTemporaryDirectorySync
    ) {
        let adapter = LexicalAdapter::with_state_root(root.to_path_buf());
        let guard = Guard::install(
            Case {
                cut: Cut::RootWrite,
                side: Side::Before,
                occurrence: 1,
            },
            Mode::Error,
        );
        let result = adapter.build_batch(&batch(true)?);
        assert!(guard.fired());
        assert!(
            result.is_err(),
            "preparatory root I/O refusal was suppressed"
        );
        drop(guard);
        assert_not_admitted(&adapter)?;
    }
    Ok(())
}

#[test]
fn f15_oracle_rejects_a_rehashed_pack_and_root_that_forge_fixed_source_bytes() -> TestResult {
    let root = tempfile::tempdir()?;
    let _base_tree = setup(root.path())?;
    let directory = generation(root.path(), false)?;
    let mut wire = audit_root(&directory, &truth(false))?;
    let source = wire
        .3
        .iter_mut()
        .find(|row| row.0.file.repo_relative_path.as_str() == EDIT)
        .ok_or("missing editable source")?;
    let pack = wire
        .4
        .iter_mut()
        .find(|pack| pack.2 == source.7)
        .ok_or("missing editable pack")?;
    assert_eq!(pack.4, 1, "fixed source keys must occupy distinct buckets");
    let mut raw = read_partition(&directory, pack)?;
    let forged = b"badneedle original\n";
    assert_eq!(forged.len(), OLD_BODY.len());
    let source_digest: [u8; 32] = Sha256::digest(forged).into();
    raw.get_mut(24..56)
        .ok_or("missing pack source digest")?
        .copy_from_slice(&source_digest);
    let payload = raw
        .get_mut(68..)
        .ok_or("missing single-source pack payload")?;
    assert_eq!(payload.len(), forged.len());
    payload.copy_from_slice(forged);
    let pack_digest: [u8; 32] = Sha256::digest(&raw).into();
    source.0.source_sha256 = source_digest;
    source.7 = pack_digest;
    pack.2 = pack_digest;
    std::fs::write(
        directory
            .join("file-authority/objects")
            .join(format!("{}.bin", hex(&pack_digest))),
        raw,
    )?;
    let mut encoded = Vec::new();
    ciborium::into_writer(&wire, &mut encoded)?;
    std::fs::write(directory.join("file-authority/root.cbor"), encoded)?;
    assert!(
        std::panic::catch_unwind(|| audit_root(&directory, &truth(false))).is_err(),
        "rehashing must not let forged source bytes become their own expected outcome"
    );
    Ok(())
}

#[test]
fn f15_sealed_replay_refuses_extra_temporaries_and_preserves_committed_custody() -> TestResult {
    let root = tempfile::tempdir()?;
    let _base_tree = setup(root.path())?;
    let directory = generation(root.path(), false)?;
    let temporary = directory.join("file-authority/.root.cbor.tmp-4242-1");
    std::fs::write(&temporary, b"uncommitted sealed-root entry")?;
    let before = file_tree(&directory)?;
    let adapter = LexicalAdapter::with_state_root(root.path().to_path_buf());
    assert!(adapter.build_batch(&batch(false)?).is_err());
    assert!(
        adapter
            .validate_generation_identity(&candidate(false)?)
            .is_err()
    );
    assert!(
        adapter
            .open(
                &repo()?,
                &revision()?,
                ManifestGeneration::new(1),
                &RequestBudgetV1::unbounded()
            )
            .is_err()
    );
    assert_eq!(
        file_tree(&directory)?,
        before,
        "sealed replay silently cleaned or changed committed storage"
    );
    assert_eq!(std::fs::read(temporary)?, b"uncommitted sealed-root entry");
    Ok(())
}

#[test]
fn f15_replay_reissues_the_directory_barrier_after_interrupted_staging_cleanup() -> TestResult {
    for side in [Side::Before, Side::After] {
        let root = tempfile::tempdir()?;
        let base_tree = setup(root.path())?;
        let adapter = LexicalAdapter::with_state_root(root.path().to_path_buf());
        let guard = Guard::install(
            Case {
                cut: Cut::AuthorityDirectorySync,
                side,
                occurrence: 1,
            },
            Mode::Error,
        );
        assert!(adapter.build_batch(&batch(true)?).is_err());
        assert!(guard.fired());
        drop(guard);
        let target = generation(root.path(), true)?;
        assert!(!target.join("file-authority/staging").exists());
        let scopes = [
            source_scope(KEEP, KEEP_BODY)?,
            source_scope(EDIT, NEW_BODY)?,
        ];
        let coverage = scopes
            .into_iter()
            .map(|scope| (scope.coverage.source.file.clone(), scope.coverage))
            .collect();
        let guard = Guard::install(
            Case {
                cut: Cut::AuthorityDirectorySync,
                side: Side::Before,
                occurrence: 1,
            },
            Mode::Error,
        );
        let replay = crate::file_authority::build_for_seal(
            &target,
            Some(&generation(root.path(), false)?),
            &coverage,
        );
        assert!(
            guard.fired(),
            "absence of staging cannot prove that its prior unlink was synchronized"
        );
        assert!(
            replay.is_err(),
            "replay suppressed the final directory barrier refusal"
        );
        drop(guard);
        assert_not_admitted(&adapter)?;
        drop(adapter);
        recover(root.path(), &base_tree)?;
    }
    Ok(())
}

#[test]
fn f15_io_refusals_never_admit_an_unsealed_target_and_retry_preserves_base_custody() -> TestResult {
    for case in cases() {
        eprintln!("F15-IO-CUT {case:?}");
        let root = tempfile::tempdir()?;
        let base_tree = setup(root.path())?;
        prepare_case(root.path(), case)?;
        let adapter = LexicalAdapter::with_state_root(root.path().to_path_buf());
        let guard = Guard::install(case, Mode::Error);
        let result = adapter.build_batch(&batch(true)?);
        assert!(guard.fired(), "cut was not reached: {case:?}");
        let error = result.err().ok_or("injected I/O failure was suppressed")?;
        assert!(
            error
                .to_string()
                .contains("injected F15 publication I/O refusal"),
            "wrong error at {case:?}: {error}"
        );
        drop(guard);
        drop(adapter);
        let cold = LexicalAdapter::with_state_root(root.path().to_path_buf());
        assert_not_admitted(&cold)?;
        assert_complete_root_if_published(root.path(), case)?;
        assert_eq!(file_tree(&generation(root.path(), false)?)?, base_tree);
        drop(cold);
        recover(root.path(), &base_tree)?;
    }
    Ok(())
}

struct ReapedChild(Child);

impl Drop for ReapedChild {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            if let Err(error) = self.0.kill() {
                eprintln!("crash child cleanup kill: {error}");
            }
            if let Err(error) = self.0.wait() {
                eprintln!("crash child cleanup reap: {error}");
            }
        }
    }
}

#[test]
fn f15_sigkill_cuts_never_admit_staging_and_restart_recovers_complete_objects() -> TestResult {
    for (index, case) in cases().into_iter().enumerate() {
        eprintln!("F15-SIGKILL-CUT {case:?}");
        let root = tempfile::tempdir()?;
        let base_tree = setup(root.path())?;
        prepare_case(root.path(), case)?;
        let control = tempfile::tempdir()?;
        let marker = control.path().join("reached");
        let log = control.path().join("child.log");
        let output = std::fs::File::create(&log)?;
        let mut child = ReapedChild(
            Command::new(std::env::current_exe()?)
                .args([
                    "--exact",
                    CHILD_TEST,
                    "--ignored",
                    "--test-threads",
                    "1",
                    "--nocapture",
                ])
                .env("QI_F15_TEST_ROOT", root.path())
                .env("QI_F15_TEST_MARKER", &marker)
                .env("QI_F15_TEST_CASE", index.to_string())
                .stdin(Stdio::null())
                .stdout(output.try_clone()?)
                .stderr(output)
                .spawn()?,
        );
        let started = Instant::now();
        while !marker.exists() {
            if let Some(status) = child.0.try_wait()? {
                return Err(format!(
                    "child exited before {case:?}: {status}; {}",
                    std::fs::read_to_string(&log)?
                )
                .into());
            }
            if started.elapsed() > Duration::from_secs(30) {
                return Err(format!(
                    "child did not reach {case:?}; {}",
                    std::fs::read_to_string(&log)?
                )
                .into());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            std::fs::read_to_string(&marker)?,
            format!("{:?}/{:?}", case.cut, case.side)
        );
        child.0.kill()?;
        assert_eq!(
            child.0.wait()?.signal(),
            Some(9),
            "child was not killed by SIGKILL"
        );
        let cold = LexicalAdapter::with_state_root(root.path().to_path_buf());
        assert_not_admitted(&cold)?;
        assert_complete_root_if_published(root.path(), case)?;
        assert_eq!(file_tree(&generation(root.path(), false)?)?, base_tree);
        drop(cold);
        recover(root.path(), &base_tree)?;
    }
    Ok(())
}

#[test]
#[ignore = "private subprocess entrypoint; parent matrix supplies the exact fixture and kills this process"]
fn publication_crash_child() -> TestResult {
    let root = PathBuf::from(std::env::var_os("QI_F15_TEST_ROOT").ok_or("missing parent fixture")?);
    let marker =
        PathBuf::from(std::env::var_os("QI_F15_TEST_MARKER").ok_or("missing parent marker")?);
    let index: usize = std::env::var("QI_F15_TEST_CASE")?.parse()?;
    let case = *cases().get(index).ok_or("unknown crash cut")?;
    let _guard = Guard::install(case, Mode::Pause(marker));
    let adapter = LexicalAdapter::with_state_root(root);
    let result = adapter.build_batch(&batch(true)?);
    Err(format!("crash cut was not reached: {case:?}; {result:?}").into())
}
