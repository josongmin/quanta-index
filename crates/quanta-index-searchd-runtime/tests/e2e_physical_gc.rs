//! QI-BB-003 — retention reclaims the bytes, not just the authority record.
//!
//! Before this, reaping a sealed generation removed its authority CBOR file
//! and left the Tantivy and `LanceDB` directories behind, so state-root disk
//! use grew with every generation ever sealed. Now the search plane, after
//! reaping the authority and reconciling the ledger, fences the snapshot
//! registry and reclaims each retired generation's directory on both tracks,
//! verifying the sealed identity before it deletes anything.
//!
//! Oracles are the filesystem: which generation directories exist under the
//! pair on each track, and that the retained ones still serve. The serving
//! boundary is the other half (QI-BB-003): a reaped generation, or a sealed
//! directory the durable authority does not retain (an orphan left behind
//! by a restart under a lower cap), is refused `UNKNOWN_GENERATION` on
//! every pinned route, listed by `quarantine list` and removed by
//! `quarantine discard`; the bytes GC reclaims and the bytes retention
//! keeps are reported through the metrics scrape and match an independent
//! inode-set measurement of the directories.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt::Write as _;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use quanta_index_contract::{
    ChunkId, ChunkRecord, GenerationPin, ManifestGeneration, QuarantineDiscardOutcomeDtoV1,
    QuarantineTargetV1, RepoId, RepoRelativePath, RevisionId, SearchCorpusActiveHeadV1,
    SearchPlaneErrorCodeV2, lex::LanguageCode,
};
use quanta_index_core::{
    GenerationQuarantineReasonV1, GenerationStorageKeyV1, IntegrityScrubPolicyV1,
};
use quanta_index_sdk::{QuantaIndex, SdkError, SearchCorpusBatch};
use quanta_index_searchd_harness::{
    legacy_reference::{
        LegacySearchdBinary, SearchdBinaryProcess, legacy_source_batch,
        legacy_source_delta_from_chunks, publish_legacy_source_batch,
    },
    private_tempdir,
    scale_runtime_policy::DEFAULT_HISTORY_MAX_GENERATIONS,
};
use sha2::{Digest, Sha256};

type TestResult = Result<(), Box<dyn Error>>;

const TRACK_ROOTS: [&str; 2] = ["indexes/lexical", "indexes/semantic"];

const SEALED: u64 = 5;
const KEEP: usize = 2;

struct RetentionFixture {
    process: SearchdBinaryProcess,
    binary: LegacySearchdBinary,
    client: QuantaIndex,
    root: tempfile::TempDir,
    repo: RepoId,
    revision: RevisionId,
    active: Option<SearchCorpusActiveHeadV1>,
}

impl RetentionFixture {
    fn boot(keep: usize, scrub_and_quota: &[(&str, String)]) -> Result<Self, Box<dyn Error>> {
        let root = private_tempdir()?;
        let binary = LegacySearchdBinary::from_environment()?;
        let (process, client) = Self::start_process(&binary, root.path(), keep, scrub_and_quota)?;
        Ok(Self {
            process,
            binary,
            client,
            root,
            repo: RepoId::new("repo-e2e")?,
            revision: RevisionId::new("rev-e2e")?,
            active: None,
        })
    }

    fn start_process(
        binary: &LegacySearchdBinary,
        root: &Path,
        keep: usize,
        settings: &[(&str, String)],
    ) -> Result<(SearchdBinaryProcess, QuantaIndex), Box<dyn Error>> {
        let mut command = binary.benchmark_command(root, keep)?;
        let _configured = command
            .env("QUANTA_INDEX_MAINTENANCE_TICK_MS", "50")
            .env("QUANTA_INDEX_INTEGRITY_SCRUB_INTERVAL_MS", "86400000")
            .env(
                "QUANTA_INDEX_INTEGRITY_SCRUB_MAX_BYTES_PER_STEP",
                IntegrityScrubPolicyV1::DEFAULT
                    .max_bytes_per_step
                    .to_string(),
            )
            .envs(settings.iter().map(|(key, value)| (*key, value)));
        let process = SearchdBinaryProcess::start_with_pinned_command(root, command)?;
        let client = process.connect()?;
        Ok((process, client))
    }

    fn reopen(self, keep: usize) -> Result<Self, Box<dyn Error>> {
        let Self {
            process,
            binary,
            client,
            root,
            repo,
            revision,
            active,
        } = self;
        drop(client);
        process.stop()?;
        binary.verify()?;
        let (process, client) = Self::start_process(&binary, root.path(), keep, &[])?;
        Ok(Self {
            process,
            binary,
            client,
            root,
            repo,
            revision,
            active,
        })
    }

    fn stop(self) -> TestResult {
        self.process.stop()?;
        self.binary.verify()
    }

    fn state_root(&self) -> &Path {
        self.root.path()
    }

    fn next_batch(
        &self,
        path: &str,
        body: &str,
        previous_body: Option<&str>,
    ) -> Result<SearchCorpusBatch, Box<dyn Error>> {
        let Some(active) = self.active.as_ref() else {
            return legacy_source_batch(&self.repo, &self.revision, 1, None, &[(path, body, 1)]);
        };
        let previous = active.generation.lexical.manifest_generation.get();
        let generation = previous
            .checked_add(1)
            .ok_or("fixture generation exhausted")?;
        if let Some(previous_body) = previous_body {
            return legacy_source_batch(
                &self.repo,
                &self.revision,
                generation,
                Some((path, previous_body, previous)),
                &[(path, body, generation)],
            );
        }
        legacy_source_delta_from_chunks(
            &self.repo,
            &self.revision,
            generation,
            &[],
            &[],
            vec![ChunkRecord {
                chunk_id: ChunkId::new(format!("e2e-{generation}-{path}")),
                repo_relative_path: RepoRelativePath::new(path),
                language: LanguageCode::new("rust")?,
                start_byte: 0,
                end_byte: u32::try_from(body.len())?,
                start_line: 1,
                end_line: 2,
                text: body.to_owned().into_boxed_str(),
                structural: None,
                parent_chunk_id: None,
                source_repo_id: None,
            }],
        )
    }

    fn publish_file(
        &mut self,
        path: &str,
        body: &str,
        previous_body: Option<&str>,
    ) -> Result<ManifestGeneration, Box<dyn Error>> {
        let batch = self.next_batch(path, body, previous_body)?;
        let (head, receipt) =
            publish_legacy_source_batch(&self.client, &batch, self.active.clone())?;
        if !receipt.sealed || head.generation.semantic.manifest_generation != batch.generation() {
            return Err(format!("publication did not seal both tracks: {receipt:?}").into());
        }
        self.active = Some(head);
        Ok(batch.generation())
    }
}

fn pair_dir(rt: &RetentionFixture, track_root: &str) -> Result<PathBuf, Box<dyn Error>> {
    let canonical = std::fs::canonicalize(rt.state_root())?;
    Ok(canonical
        .join(track_root)
        .join(GenerationStorageKeyV1::for_repo_revision(&rt.repo, &rt.revision).as_str()))
}

/// Generation numbers whose directories exist under `pair`.
fn generations_on_disk(pair: &Path) -> Result<BTreeSet<u64>, Box<dyn Error>> {
    let mut out = BTreeSet::new();
    if !pair.is_dir() {
        return Ok(out);
    }
    for entry in std::fs::read_dir(pair)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if let Some(number) = name.strip_prefix('g') {
            let _new = out.insert(number.parse::<u64>()?);
        }
    }
    Ok(out)
}

/// Seal `SEALED` generations, activating each as it lands, so the reaped set
/// is `SEALED - KEEP` and the retained set is the active one plus the newest.
fn seal_generations(
    rt: &mut RetentionFixture,
    count: u64,
) -> Result<Vec<ManifestGeneration>, Box<dyn Error>> {
    let mut sealed = Vec::new();
    for index in 0..count {
        sealed.push(rt.publish_file(
            &format!("src/item_{index}.rs"),
            &format!("fn item_{index}() {{ needle_{index} }}"),
            None,
        )?);
    }
    Ok(sealed)
}

/// A directory whose sealed identity contradicts its path is never swept.
///
/// The sweep fails closed with the scope-mismatch code and the directory
/// stays under either identity. (The boot scanner refuses the same
/// directory; quarantining it instead of refusing is QI-BB-026 / W2.) The
/// crash-orphan case — a directory whose identity is correct but whose
/// authority record was already reaped — is pinned at the materializer
/// level, where the crash point can be placed.
#[test]
fn a_directory_whose_identity_contradicts_its_path_fails_closed() -> TestResult {
    let mut rt = RetentionFixture::boot(KEEP, &[])?;
    let sealed = seal_generations(&mut rt, SEALED)?;
    let oldest_retained = sealed
        .iter()
        .rev()
        .nth(KEEP.saturating_sub(1))
        .copied()
        .ok_or("fewer generations than KEEP were sealed")?;

    let lexical_pair = pair_dir(&rt, "indexes/lexical")?;
    let donor = lexical_pair.join(format!("g{}", oldest_retained.get()));
    let impostor = lexical_pair.join("g1");
    copy_tree(&donor, &impostor)?;
    if !impostor.is_dir() {
        return Err("impostor fixture was not created".into());
    }

    let batch = rt.next_batch("src/extra.rs", "fn extra() { needle_extra }", None)?;
    let refused = rt.client.search_corpus().publish(&batch);
    if !matches!(
        &refused,
        Err(SdkError::Remote {
            code: SearchPlaneErrorCodeV2::GenerationIdentityScopeMismatch,
            ..
        })
    ) {
        return Err(format!(
            "seal did not refuse the directory identity contradiction: {refused:?}"
        )
        .into());
    }
    if !impostor.is_dir() || !donor.is_dir() {
        return Err("the sweep removed a directory despite the identity contradiction".into());
    }
    rt.stop()
}

fn copy_tree(from: &Path, to: &Path) -> Result<(), Box<dyn Error>> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            let _bytes = std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// Bytes of every regular file under `roots`, each `(device, inode)` once:
/// what `du` reports for the set, independently of the daemon's measure.
fn inode_set_bytes_with_seen(
    roots: &[PathBuf],
    seen: &mut BTreeSet<(u64, u64)>,
) -> Result<u64, Box<dyn Error>> {
    let mut total = 0_u64;
    let mut pending: Vec<PathBuf> = roots.to_vec();
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                pending.push(entry.path());
            } else if file_type.is_file() {
                let metadata = entry.metadata()?;
                if seen.insert((metadata.dev(), metadata.ino())) {
                    total = total
                        .checked_add(metadata.len())
                        .ok_or("inode byte total overflow")?;
                }
            }
        }
    }
    Ok(total)
}

fn inode_set_bytes(roots: &[PathBuf]) -> Result<u64, Box<dyn Error>> {
    inode_set_bytes_with_seen(roots, &mut BTreeSet::new())
}

/// Inspect physical canonical pool files without consulting the quota ledger.
fn physical_pool_inventory(rt: &RetentionFixture) -> Result<BTreeMap<String, u64>, Box<dyn Error>> {
    let pool =
        std::fs::canonicalize(rt.state_root())?.join("indexes/lexical/lexical-object-pool/objects");
    let mut files = BTreeMap::new();
    for shard in std::fs::read_dir(pool)? {
        let shard = shard?;
        if !shard.file_type()?.is_dir() {
            return Err("pool inventory contains a non-directory shard".into());
        }
        for entry in std::fs::read_dir(shard.path())? {
            let entry = entry?;
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            if !file_type.is_file() {
                return Err("pool inventory contains a non-regular object".into());
            }
            let name = entry
                .file_name()
                .into_string()
                .map_err(|name| format!("non-UTF8 object name: {}", name.display()))?;
            if name.len() != 64 || !name.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(format!("unexpected pool object name {name}").into());
            }
            // GC may retire an object after the directory entry was observed.
            // Only absence is transient; malformed or unreadable files fail.
            let encoded = match std::fs::read(entry.path()) {
                Ok(encoded) => encoded,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            let digest = Sha256::digest(&encoded);
            let mut expected_name = String::with_capacity(64);
            for byte in digest {
                write!(&mut expected_name, "{byte:02x}")?;
            }
            if name != expected_name {
                return Err(format!("pool object {name} differs from its digest").into());
            }
            let width = u64::try_from(encoded.len())?;
            if files.insert(name, width).is_some() {
                return Err("duplicate physical pool digest".into());
            }
        }
    }
    Ok(files)
}

fn assert_pool_quota_matches_disk(rt: &RetentionFixture) -> TestResult {
    let inventory = physical_pool_inventory(rt)?;
    let bytes = inventory.values().try_fold(0_u64, |sum, width| {
        sum.checked_add(*width).ok_or("physical pool byte overflow")
    })?;
    let scrape = Scrape::take(rt)?;
    for (name, expected) in [
        ("lexical_pool_quota_enabled", 1.0),
        (
            "lexical_pool_quota_charged_files",
            f64::from(u32::try_from(inventory.len())?),
        ),
        (
            "lexical_pool_quota_charged_bytes",
            f64::from(u32::try_from(bytes)?),
        ),
        (
            "lexical_pool_quota_canonical_bytes",
            f64::from(u32::try_from(bytes)?),
        ),
        ("lexical_pool_quota_orphan_bytes", 0.0),
        ("lexical_pool_quota_in_flight_bytes", 0.0),
    ] {
        let actual = scrape.gauge(name)?;
        if actual.to_bits() != expected.to_bits() {
            return Err(format!("{name}: quota reports {actual}, disk expects {expected}").into());
        }
    }
    Ok(())
}

fn wait_for_pool_quota_to_match_disk(rt: &RetentionFixture) -> TestResult {
    // The collector can unlink between a filesystem walk and the control
    // scrape. Require eventual agreement after its durable callback settles.
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(30))
        .ok_or("quota observation deadline overflow")?;
    loop {
        match assert_pool_quota_matches_disk(rt) {
            Ok(()) => return Ok(()),
            Err(error) if Instant::now() >= deadline => return Err(error),
            Err(_transient) => std::thread::sleep(Duration::from_millis(25)),
        }
    }
}

fn file_body_digest(content: &str) -> Result<String, Box<dyn Error>> {
    // QLSTOR01/v1, FileBody kind 1, zero dependencies, inline blob tag 0,
    // then the harness's one-chunk source bytes (text plus one newline).
    // This fixed-format witness is independent of the producer's object ref.
    let source = format!("{content}\n");
    let mut encoded = Vec::from(b"QLSTOR01".as_slice());
    encoded.extend_from_slice(&1_u16.to_be_bytes());
    encoded.push(1);
    encoded.extend_from_slice(&0_u16.to_be_bytes());
    let payload_len = source
        .len()
        .checked_add(1)
        .ok_or("payload length overflow")?;
    encoded.extend_from_slice(&u64::try_from(payload_len)?.to_be_bytes());
    encoded.push(0);
    encoded.extend_from_slice(source.as_bytes());
    let mut name = String::with_capacity(64);
    for byte in Sha256::digest(encoded) {
        write!(&mut name, "{byte:02x}")?;
    }
    Ok(name)
}

/// Production quota accounting follows actual GC unlink and exact-digest recreation.
#[test]
fn lexical_pool_quota_matches_physical_gc_and_same_digest_recreation() -> TestResult {
    const FIRST: &str = "fn reusable_body() { unique_first_content() }";
    const SECOND: &str = "fn replacement_body() { unique_second_content() }";
    const THIRD: &str = "fn replacement_body() { unique_third_content() }";
    // Shared-pool collection follows scrub ticks, so pace it as before.
    let settings = [
        (
            "QUANTA_INDEX_LEXICAL_POOL_MAX_ENCODED_BYTES",
            "134217728".into(),
        ),
        ("QUANTA_INDEX_LEXICAL_POOL_MAX_FILES", "100000".into()),
        ("QUANTA_INDEX_INTEGRITY_SCRUB_INTERVAL_MS", "100".into()),
    ];
    let mut rt = RetentionFixture::boot(KEEP, &settings)?;
    let first_digest = file_body_digest(FIRST)?;

    let _first = rt.publish_file("src/reused.rs", FIRST, None)?;
    wait_for_pool_quota_to_match_disk(&rt)?;
    let _second = rt.publish_file("src/reused.rs", SECOND, Some(FIRST))?;
    wait_for_pool_quota_to_match_disk(&rt)?;
    if !physical_pool_inventory(&rt)?.contains_key(&first_digest) {
        return Err("first FileBody was not installed under its content digest".into());
    }

    let _third = rt.publish_file("src/reused.rs", THIRD, Some(SECOND))?;
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(30))
        .ok_or("GC deadline overflow")?;
    loop {
        let inventory = physical_pool_inventory(&rt)?;
        if !inventory.contains_key(&first_digest) {
            wait_for_pool_quota_to_match_disk(&mut rt)?;
            break;
        }
        if Instant::now() >= deadline {
            return Err("retired first FileBody was not physically collected".into());
        }
        std::thread::sleep(Duration::from_millis(25));
    }

    let _recreated = rt.publish_file("src/reused.rs", FIRST, Some(THIRD))?;
    if !physical_pool_inventory(&rt)?.contains_key(&first_digest) {
        return Err("same FileBody digest was not recreated after GC".into());
    }
    wait_for_pool_quota_to_match_disk(&rt)?;
    rt.stop()
}

#[derive(Clone, Eq, Ord, PartialEq, PartialOrd)]
struct DiskObjectRef {
    kind: u8,
    bytes: u64,
    sha256: [u8; 32],
}

fn cbor_array(
    value: &ciborium::Value,
    length: usize,
) -> Result<&[ciborium::Value], Box<dyn Error>> {
    if let ciborium::Value::Array(rows) = value
        && rows.len() == length
    {
        return Ok(rows);
    }
    Err(format!("expected CBOR array of {length} elements").into())
}

fn cbor_u64(value: &ciborium::Value) -> Result<u64, Box<dyn Error>> {
    if let ciborium::Value::Integer(number) = value {
        return Ok(u64::try_from(*number)?);
    }
    Err("expected CBOR unsigned integer".into())
}

fn cbor_object_ref(value: &ciborium::Value) -> Result<DiskObjectRef, Box<dyn Error>> {
    let row = cbor_array(value, 3)?;
    let kind = u8::try_from(cbor_u64(row.first().ok_or("missing object kind")?)?)?;
    if !(1..=11).contains(&kind) {
        return Err("unknown object kind".into());
    }
    let bytes = cbor_u64(row.get(1).ok_or("missing object bytes")?)?;
    let digest = match row.get(2) {
        Some(ciborium::Value::Bytes(octets)) => <[u8; 32]>::try_from(octets.as_slice())?,
        Some(ciborium::Value::Array(octets)) if octets.len() == 32 => {
            let mut digest = [0_u8; 32];
            for (slot, octet) in digest.iter_mut().zip(octets) {
                *slot = u8::try_from(cbor_u64(octet)?)?;
            }
            digest
        }
        _ => return Err("object reference lacks a 32-byte digest".into()),
    };
    Ok(DiskObjectRef {
        kind,
        bytes,
        sha256: digest,
    })
}

fn disk_map_root(
    value: &ciborium::Value,
    roots: &mut Vec<DiskObjectRef>,
) -> Result<(), Box<dyn Error>> {
    let map = cbor_array(value, 2)?;
    let page = map.first().ok_or("missing map page")?;
    if matches!(page, ciborium::Value::Null) {
        if cbor_u64(map.get(1).ok_or("missing map count")?)? != 0 {
            return Err("empty map page has nonzero count".into());
        }
    } else {
        let page = cbor_array(page, 6)?;
        let object = cbor_object_ref(page.first().ok_or("missing page object")?)?;
        if object.kind != 8 {
            return Err("map root does not reference a map page".into());
        }
        roots.push(object);
    }
    Ok(())
}

fn disk_run_registry_roots(
    value: &ciborium::Value,
    roots: &mut Vec<DiskObjectRef>,
) -> Result<(), Box<dyn Error>> {
    let registry = cbor_array(value, 8)?;
    if cbor_u64(registry.first().ok_or("missing registry format")?)? != 2 {
        return Err("unexpected run registry format".into());
    }
    disk_map_root(registry.get(1).ok_or("missing runs map")?, roots)?;
    disk_map_root(registry.get(2).ok_or("missing run owners map")?, roots)
}

/// Independently parse the fixed File v22 and Native v8 disk contracts,
/// including run ownership, run postings and optional premerge custody.
fn disk_root_objects(
    path: &Path,
    format: u64,
    width: usize,
) -> Result<Vec<DiskObjectRef>, Box<dyn Error>> {
    let bytes = std::fs::read(path)?;
    let value: ciborium::Value = ciborium::from_reader(bytes.as_slice())?;
    let row = cbor_array(&value, width)?;
    if cbor_u64(row.first().ok_or("missing root format")?)? != format {
        return Err(format!("unexpected root format in {}", path.display()).into());
    }
    let mut roots = Vec::new();
    match (format, width) {
        (22, 16) => {
            for offset in [3, 4, 5, 6, 9, 10, 11] {
                disk_map_root(row.get(offset).ok_or("missing File map")?, &mut roots)?;
            }
            disk_run_registry_roots(row.get(7).ok_or("missing File registry")?, &mut roots)?;
            let bundle = cbor_array(row.get(8).ok_or("missing run postings bundle")?, 3)?;
            let premerge = bundle.first().ok_or("missing premerge registry")?;
            if !matches!(premerge, ciborium::Value::Null) {
                disk_run_registry_roots(premerge, &mut roots)?;
            }
            disk_map_root(bundle.get(1).ok_or("missing run postings map")?, &mut roots)?;
        }
        (8, 13) => {
            for offset in 3..10 {
                disk_map_root(row.get(offset).ok_or("missing Native map")?, &mut roots)?;
            }
            disk_run_registry_roots(row.get(10).ok_or("missing Native registry")?, &mut roots)?;
        }
        _ => return Err("unsupported root wire contract".into()),
    }
    Ok(roots)
}

fn disk_dependency(bytes: &[u8]) -> Result<DiskObjectRef, Box<dyn Error>> {
    if bytes.len() != 41 {
        return Err("object dependency has wrong width".into());
    }
    let kind = *bytes.first().ok_or("missing dependency kind")?;
    if !(1..=11).contains(&kind) {
        return Err("unknown dependency kind".into());
    }
    let length = <[u8; 8]>::try_from(bytes.get(1..9).ok_or("missing dependency length")?)?;
    let sha256 = <[u8; 32]>::try_from(bytes.get(9..41).ok_or("missing dependency digest")?)?;
    Ok(DiskObjectRef {
        kind,
        bytes: u64::from_be_bytes(length),
        sha256,
    })
}

/// Follow only committed root references. Reaped objects can remain in the
/// shared pool until the asynchronous collector completes.
fn retained_pool_bytes(
    rt: &RetentionFixture,
    retained: &BTreeSet<u64>,
    seen_inodes: &mut BTreeSet<(u64, u64)>,
) -> Result<u64, Box<dyn Error>> {
    let lexical_pair = pair_dir(rt, "indexes/lexical")?;
    let pool =
        std::fs::canonicalize(rt.state_root())?.join("indexes/lexical/lexical-object-pool/objects");
    let mut pending = Vec::new();
    for generation in retained {
        let directory = lexical_pair.join(format!("g{generation}"));
        pending.extend(disk_root_objects(
            &directory.join("file-authority/root.cbor"),
            22,
            16,
        )?);
        pending.extend(disk_root_objects(
            &directory.join("text-authority/manifest.cbor"),
            8,
            13,
        )?);
    }
    let mut seen_refs = BTreeMap::<[u8; 32], (u8, u64)>::new();
    let mut total = 0_u64;
    while let Some(reference) = pending.pop() {
        if let Some(previous) =
            seen_refs.insert(reference.sha256, (reference.kind, reference.bytes))
        {
            if previous != (reference.kind, reference.bytes) {
                return Err("one digest has conflicting object references".into());
            }
            continue;
        }
        let mut name = String::with_capacity(64);
        for byte in reference.sha256 {
            write!(&mut name, "{byte:02x}")?;
        }
        let shard = name.get(..2).ok_or("short object digest")?;
        let path = pool.join(shard).join(name);
        let metadata = std::fs::symlink_metadata(&path)?;
        if !metadata.file_type().is_file()
            || metadata.len() != reference.bytes
            || reference.bytes > 96 * 1024
        {
            return Err(format!("invalid pooled object size/type: {}", path.display()).into());
        }
        let encoded = std::fs::read(&path)?;
        let digest: [u8; 32] = Sha256::digest(&encoded).into();
        if u64::try_from(encoded.len())? != reference.bytes || digest != reference.sha256 {
            return Err(format!(
                "pooled object differs from its reference: {}",
                path.display()
            )
            .into());
        }
        if encoded.get(..8) != Some(b"QLSTOR01".as_slice())
            || encoded.get(8..10) != Some(1_u16.to_be_bytes().as_slice())
            || encoded.get(10) != Some(&reference.kind)
        {
            return Err("pooled object has unexpected envelope".into());
        }
        let count = u16::from_be_bytes(<[u8; 2]>::try_from(
            encoded.get(11..13).ok_or("missing dependency count")?,
        )?);
        let payload = u64::from_be_bytes(<[u8; 8]>::try_from(
            encoded.get(13..21).ok_or("missing payload length")?,
        )?);
        if count > 128 || payload > 64 * 1024 {
            return Err("pooled object exceeds physical bounds".into());
        }
        let deps_bytes = usize::from(count)
            .checked_mul(41)
            .ok_or("dependency width overflow")?;
        let payload_start = 21_usize
            .checked_add(deps_bytes)
            .ok_or("object header overflow")?;
        if payload_start.checked_add(usize::try_from(payload)?) != Some(encoded.len()) {
            return Err("pooled object encoded length differs".into());
        }
        for dependency in encoded
            .get(21..payload_start)
            .ok_or("missing dependency rows")?
            .chunks_exact(41)
        {
            pending.push(disk_dependency(dependency)?);
        }
        if seen_inodes.insert((metadata.dev(), metadata.ino())) {
            total = total
                .checked_add(metadata.len())
                .ok_or("pooled byte total overflow")?;
        }
    }
    Ok(total)
}

/// The directories `generations` occupy on both tracks, existing ones only.
fn generation_dirs(
    rt: &RetentionFixture,
    generations: &BTreeSet<u64>,
) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let mut dirs = Vec::new();
    for track_root in TRACK_ROOTS {
        let pair = pair_dir(rt, track_root)?;
        for generation in generations {
            let dir = pair.join(format!("g{generation}"));
            if dir.is_dir() {
                dirs.push(dir);
            }
        }
    }
    Ok(dirs)
}

struct Scrape {
    counters: BTreeMap<String, u64>,
    gauges: BTreeMap<String, f64>,
}

impl Scrape {
    fn take(rt: &RetentionFixture) -> Result<Self, Box<dyn Error>> {
        let snapshot = rt.client.observability().metrics_snapshot()?;
        Ok(Self {
            counters: snapshot
                .counters
                .iter()
                .map(|counter| (counter.name.clone(), counter.value))
                .collect(),
            gauges: snapshot
                .gauges
                .iter()
                .map(|gauge| (gauge.name.clone(), gauge.value))
                .collect(),
        })
    }

    fn counter(&self, name: &str) -> Result<u64, Box<dyn Error>> {
        self.counters
            .get(name)
            .copied()
            .ok_or_else(|| format!("counter `{name}` is in the scrape: {:?}", self.counters).into())
    }

    fn gauge(&self, name: &str) -> Result<f64, Box<dyn Error>> {
        self.gauges
            .get(name)
            .copied()
            .ok_or_else(|| format!("gauge `{name}` is in the scrape: {:?}", self.gauges).into())
    }
}

fn pin(rt: &RetentionFixture, generation: ManifestGeneration) -> GenerationPin {
    GenerationPin::new(rt.repo.clone(), rt.revision.clone(), generation)
}

/// Both pinned routes refuse `generation` with `UNKNOWN_GENERATION`.
fn assert_unknown_on_every_route(
    rt: &RetentionFixture,
    generation: ManifestGeneration,
    what: &str,
) -> TestResult {
    let pin = pin(rt, generation);
    let text = rt
        .client
        .lexical()
        .query()
        .native("needle_1")
        .pinned(pin.clone())
        .top_k(5)
        .execute();
    if !matches!(
        &text,
        Err(SdkError::Remote {
            code: SearchPlaneErrorCodeV2::UnknownGeneration,
            ..
        })
    ) {
        return Err(format!("lexical pin to {what} g{}: {text:?}", generation.get()).into());
    }
    let semantic = rt
        .client
        .semantic()
        .query()
        .text("needle")
        .pinned(pin)
        .top_k(5)
        .execute();
    if !matches!(
        &semantic,
        Err(SdkError::Remote {
            code: SearchPlaneErrorCodeV2::UnknownGeneration,
            ..
        })
    ) {
        return Err(format!("semantic pin to {what} g{}: {semantic:?}", generation.get()).into());
    }
    Ok(())
}

/// A pin to a reaped generation is refused `UNKNOWN_GENERATION`.
///
/// On the lexical and the semantic route alike: not `NOT_FOUND` from a
/// failed open, not served from anything. The retained generations still
/// serve.
#[test]
fn a_reaped_generation_is_unknown_to_every_pinned_route() -> TestResult {
    let mut rt = RetentionFixture::boot(KEEP, &[])?;
    let sealed = seal_generations(&mut rt, SEALED)?;
    let reaped = sealed
        .get(1)
        .copied()
        .ok_or("fewer than two generations sealed")?;
    assert_unknown_on_every_route(&rt, reaped, "reaped")?;
    let newest = sealed.last().copied().ok_or("no generation was sealed")?;
    let pin = pin(&rt, newest);
    let served = rt
        .client
        .lexical()
        .query()
        .native(format!("needle_{}", SEALED - 1))
        .pinned(pin.clone())
        .top_k(5)
        .execute()?;
    if served.generation != pin || served.results.len() != 1 {
        return Err(format!("newest generation lost its pinned row after GC: {served:?}").into());
    }
    rt.stop()
}

/// GC reports what it did, in bytes that match the disk.
///
/// One seal under the cap reclaims one generation per track and exactly
/// the bytes those directories held, nothing is deferred, and the retained
/// index bytes the byte cap is enforced against equal an independent
/// inode-set measurement of the retained directories.
#[test]
fn gc_metrics_report_reclaimed_and_retained_bytes_that_match_the_disk() -> TestResult {
    let mut rt = RetentionFixture::boot(KEEP, &[])?;
    let mut sealed = seal_generations(&mut rt, SEALED - 1)?;
    let before = Scrape::take(&rt)?;
    // With KEEP retained, the next seal reaps the oldest retained one.
    let victim: BTreeSet<u64> = sealed
        .iter()
        .rev()
        .nth(KEEP.saturating_sub(1))
        .map(|generation| generation.get())
        .into_iter()
        .collect();
    let victim_dirs = generation_dirs(&rt, &victim)?;
    if victim_dirs.len() != TRACK_ROOTS.len() {
        return Err(format!("victim {victim:?} is not on both tracks: {victim_dirs:?}").into());
    }
    let victim_bytes = inode_set_bytes(&victim_dirs)?;
    let index = SEALED - 1;
    sealed.push(rt.publish_file(
        &format!("src/item_{index}.rs"),
        &format!("fn item_{index}() {{ needle_{index} }}"),
        None,
    )?);
    let after = Scrape::take(&rt)?;
    let reclaimed_generations = after
        .counter("search_corpus_gc_reclaimed_generations_total")?
        .saturating_sub(before.counter("search_corpus_gc_reclaimed_generations_total")?);
    if reclaimed_generations != u64::try_from(TRACK_ROOTS.len())? {
        return Err(format!(
            "one seal under KEEP={KEEP} reclaims one generation per track, observed {reclaimed_generations}"
        )
        .into());
    }
    if after.counter("search_corpus_gc_deferred_pinned_total")? != 0 {
        return Err("nothing held the reaped generation; nothing is deferred".into());
    }
    if after.counter("search_corpus_gc_failures_total")? != 0 {
        return Err("a clean pass records no reclaim failure (QI-BB-020)".into());
    }
    let per_track = after
        .counter("search_corpus_gc_lexical_reclaimed_bytes_total")?
        .saturating_add(after.counter("search_corpus_gc_semantic_reclaimed_bytes_total")?);
    if per_track != after.counter("search_corpus_gc_reclaimed_bytes_total")? {
        return Err("the per-track reclaimed bytes do not add up to the total".into());
    }
    let reclaimed_bytes = after
        .counter("search_corpus_gc_reclaimed_bytes_total")?
        .saturating_sub(before.counter("search_corpus_gc_reclaimed_bytes_total")?);
    if reclaimed_bytes != victim_bytes {
        return Err(format!(
            "GC reports {reclaimed_bytes} reclaimed bytes, the victim's directories held {victim_bytes}"
        )
        .into());
    }
    let retained: BTreeSet<u64> = sealed
        .iter()
        .rev()
        .take(KEEP)
        .map(|generation| generation.get())
        .collect();
    let mut seen_inodes = BTreeSet::new();
    let retained_directories = generation_dirs(&rt, &retained)?;
    let retained_directory_bytes =
        inode_set_bytes_with_seen(&retained_directories, &mut seen_inodes)?;
    let retained_pool_bytes = retained_pool_bytes(&rt, &retained, &mut seen_inodes)?;
    let retained_bytes = retained_directory_bytes
        .checked_add(retained_pool_bytes)
        .ok_or("retained index byte total overflow")?;
    let reported = after.gauge("search_corpus_retained_index_bytes")?;
    // Integer-valued gauges are exact in f64 below 2^53; compare the bits.
    let expected = f64::from(u32::try_from(retained_bytes)?);
    if retained_bytes == 0 || reported.to_bits() != expected.to_bits() {
        return Err(format!(
            "retention reports {reported} retained index bytes, retained directories hold {retained_directory_bytes} and their pooled closure holds {retained_pool_bytes}"
        )
        .into());
    }
    rt.stop()
}

/// A restart under a lower cap leaves orphans, which are listed and discarded.
///
/// The restart reaps the authority records of the oldest generations while
/// their directories are still on disk. They are never seeded (a pin
/// answers `UNKNOWN_GENERATION`), boot reports each with its path,
/// `quarantine list` names each directory under
/// `GENERATION_QUARANTINE_ORPHANED`, `quarantine discard` removes it, and
/// afterwards nothing lists it and only the retained directories remain.
#[test]
fn an_orphan_left_by_a_restart_is_unknown_listed_and_discardable() -> TestResult {
    let mut rt = RetentionFixture::boot(DEFAULT_HISTORY_MAX_GENERATIONS, &[])?;
    let sealed = seal_generations(&mut rt, SEALED)?;
    let orphan = sealed
        .get(1)
        .copied()
        .ok_or("fewer than two generations sealed")?;
    let rt = rt.reopen(KEEP)?;

    let expected_orphans: BTreeSet<u64> = sealed
        .iter()
        .rev()
        .skip(KEEP)
        .map(|generation| generation.get())
        .collect();
    let on_disk: BTreeSet<PathBuf> = generation_dirs(&rt, &expected_orphans)?
        .into_iter()
        .collect();
    if on_disk.len() != expected_orphans.len().saturating_mul(TRACK_ROOTS.len()) {
        return Err(format!("the orphan directories survive the restart: {on_disk:?}").into());
    }
    let boot = Scrape::take(&rt)?;
    let expected_count = f64::from(u32::try_from(expected_orphans.len())?);
    for name in [
        "boot_lexical_orphaned_generations",
        "boot_semantic_orphaned_generations",
    ] {
        if boot.gauge(name)?.to_bits() != expected_count.to_bits() {
            return Err(format!("old-Q {name} differs from {expected_orphans:?}").into());
        }
    }
    assert_unknown_on_every_route(&rt, orphan, "orphan")?;

    let orphan_code = GenerationQuarantineReasonV1::Orphaned.as_code_str();
    let listed = rt.client.quarantine().inventory()?;
    let listed_orphans: Vec<_> = listed
        .lexical
        .iter()
        .chain(listed.semantic.iter())
        .filter(|entry| entry.reason == orphan_code)
        .cloned()
        .collect();
    let listed_paths: BTreeSet<PathBuf> = listed_orphans
        .iter()
        .map(|entry| PathBuf::from(&entry.path))
        .collect();
    if listed_paths != on_disk {
        return Err(format!("quarantine list names {listed_paths:?}, on disk {on_disk:?}").into());
    }
    for entry in &listed_orphans {
        let ack = rt
            .client
            .quarantine()
            .discard(&QuarantineTargetV1::Generation(entry.clone()))
            .map_err(|error| format!("discard of {} refused: {error:?}", entry.path))?;
        match ack.outcome {
            QuarantineDiscardOutcomeDtoV1::Discarded { bytes } if bytes > 0 => {}
            other @ (QuarantineDiscardOutcomeDtoV1::Discarded { .. }
            | QuarantineDiscardOutcomeDtoV1::Absent) => {
                return Err(format!("discard of {} answered {other:?}", entry.path).into());
            }
        }
        if Path::new(&entry.path).exists() {
            return Err(format!("{} still exists after discard", entry.path).into());
        }
    }
    let after = rt.client.quarantine().inventory()?;
    if after
        .lexical
        .iter()
        .chain(after.semantic.iter())
        .any(|entry| entry.reason == orphan_code)
    {
        return Err(format!("orphans still listed after discard: {after:?}").into());
    }
    let expected_retained: BTreeSet<u64> = sealed
        .iter()
        .rev()
        .take(KEEP)
        .map(|generation| generation.get())
        .collect();
    for track_root in TRACK_ROOTS {
        let on_disk = generations_on_disk(&pair_dir(&rt, track_root)?)?;
        if on_disk != expected_retained {
            return Err(format!(
                "{track_root}: expected {expected_retained:?} on disk after discard, found {on_disk:?}"
            )
            .into());
        }
    }
    rt.stop()
}
