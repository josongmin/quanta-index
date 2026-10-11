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

use quanta_index_contract::{
    GenerationPin, ManifestGeneration, QuarantineDiscardOutcomeDtoV1, QuarantineTargetV1,
    TextQuerySyntax,
};
use quanta_index_core::{GenerationQuarantineReasonV1, GenerationStorageKeyV1};
use quanta_index_searchd_harness as e2e_harness;
use sha2::{Digest, Sha256};

use e2e_harness::E2eRuntime;

type TestResult = Result<(), Box<dyn Error>>;

const TRACK_ROOTS: [&str; 2] = ["indexes/lexical", "indexes/semantic"];

const SEALED: u64 = 5;
const KEEP: usize = 2;

fn pair_dir(rt: &E2eRuntime, track_root: &str) -> Result<PathBuf, Box<dyn Error>> {
    let canonical = std::fs::canonicalize(rt.state_root())?;
    Ok(canonical
        .join(track_root)
        .join(GenerationStorageKeyV1::for_repo_revision(&rt.repo(), &rt.revision()).as_str()))
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
fn seal_generations(rt: &mut E2eRuntime) -> Result<Vec<ManifestGeneration>, Box<dyn Error>> {
    let mut sealed = Vec::new();
    for index in 0..SEALED {
        rt.ingest_text(
            "repo",
            &format!("src/item_{index}.rs"),
            &format!("fn item_{index}() {{ needle_{index} }}"),
        )?;
        sealed.push(rt.seal()?);
        rt.activate_last_sealed_generation()?;
    }
    Ok(sealed)
}

#[test]
fn reaping_a_generation_removes_its_directories_on_both_tracks() -> TestResult {
    let mut rt = E2eRuntime::boot_with_history_max_generations(KEEP)?;
    let sealed = seal_generations(&mut rt)?;
    let expected: BTreeSet<u64> = sealed
        .iter()
        .rev()
        .take(KEEP)
        .map(|generation| generation.get())
        .collect();

    for track_root in ["indexes/lexical", "indexes/semantic"] {
        let on_disk = generations_on_disk(&pair_dir(&rt, track_root)?)?;
        if on_disk != expected {
            return Err(format!(
                "{track_root}: expected exactly the retained generations {expected:?} on disk, found {on_disk:?}"
            )
            .into());
        }
    }
    Ok(())
}

/// The retained generations are untouched: the active one still serves and
/// the newest is the pinned query target.
#[test]
fn retained_generations_still_serve_after_gc() -> TestResult {
    let mut rt = E2eRuntime::boot_with_history_max_generations(KEEP)?;
    let sealed = seal_generations(&mut rt)?;
    let newest = sealed.last().copied().ok_or("no generation was sealed")?;
    let result = rt.query_text(
        TextQuerySyntax::Native,
        &format!("needle_{}", SEALED.saturating_sub(1)),
        5,
    );
    if let Some(error) = result.typed_error {
        return Err(format!("newest generation {} does not serve: {error}", newest.get()).into());
    }
    if result.candidate_ids.len() != 1 {
        return Err(format!(
            "newest generation served {} rows for its own needle, expected 1",
            result.candidate_ids.len()
        )
        .into());
    }
    let semantic = rt.query_semantic("needle", 5, None);
    if let Some(error) = semantic.typed_error {
        return Err(format!("semantic track does not serve after GC: {error}").into());
    }
    Ok(())
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
    let mut rt = E2eRuntime::boot_with_history_max_generations(KEEP)?;
    let sealed = seal_generations(&mut rt)?;
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

    rt.ingest_text("repo", "src/extra.rs", "fn extra() { needle_extra }")?;
    match rt.seal() {
        Ok(generation) => Err(format!(
            "seal of generation {} succeeded although the sweep met a directory whose identity contradicts its path",
            generation.get()
        )
        .into()),
        Err(error) => {
            let rendered = error.to_string();
            if !rendered.contains("GENERATION_IDENTITY_SCOPE_MISMATCH") {
                return Err(format!("seal failed under an unexpected error: {rendered}").into());
            }
            if !impostor.is_dir() {
                return Err("a directory with a contradicting identity was deleted".into());
            }
            Ok(())
        }
    }
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

/// These fixed positions are the independent F18 File/F6 Native root wires:
/// version at 0, seven map roots at 3..10, each map holding one optional page.
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
    for offset in 3..10 {
        let map = cbor_array(row.get(offset).ok_or("missing root map")?, 2)?;
        let page = map.first().ok_or("missing map page")?;
        if matches!(page, ciborium::Value::Null) {
            if cbor_u64(map.get(1).ok_or("missing map count")?)? != 0 {
                return Err("empty map page has nonzero count".into());
            }
        } else {
            let page = cbor_array(page, 5)?;
            let object = cbor_object_ref(page.first().ok_or("missing page object")?)?;
            if object.kind != 8 {
                return Err("map root does not reference a map page".into());
            }
            roots.push(object);
        }
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
    rt: &E2eRuntime,
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
            18,
            14,
        )?);
        pending.extend(disk_root_objects(
            &directory.join("text-authority/manifest.cbor"),
            6,
            12,
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
    rt: &E2eRuntime,
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
    fn take(rt: &mut E2eRuntime) -> Result<Self, Box<dyn Error>> {
        let snapshot = rt.metrics_snapshot()?;
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

fn pin(rt: &E2eRuntime, generation: ManifestGeneration) -> GenerationPin {
    GenerationPin::new(rt.repo(), rt.revision(), generation)
}

/// Both pinned routes refuse `generation` with `UNKNOWN_GENERATION`.
fn assert_unknown_on_every_route(
    rt: &mut E2eRuntime,
    generation: ManifestGeneration,
    what: &str,
) -> TestResult {
    let text = rt.query_text_with_pin(
        TextQuerySyntax::Native,
        "needle_1",
        5,
        Some(pin(rt, generation)),
    );
    match text.typed_error {
        Some(error) if error.code.as_str() == "UNKNOWN_GENERATION" => {}
        other => {
            return Err(format!("lexical pin to {what} g{}: {other:?}", generation.get()).into());
        }
    }
    let semantic = rt.query_semantic_with_pin("needle", 5, Some(pin(rt, generation)));
    match semantic.typed_error {
        Some(error) if error.code.as_str() == "UNKNOWN_GENERATION" => {}
        other => {
            return Err(format!("semantic pin to {what} g{}: {other:?}", generation.get()).into());
        }
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
    let mut rt = E2eRuntime::boot_with_history_max_generations(KEEP)?;
    let sealed = seal_generations(&mut rt)?;
    let reaped = sealed
        .get(1)
        .copied()
        .ok_or("fewer than two generations sealed")?;
    assert_unknown_on_every_route(&mut rt, reaped, "reaped")?;
    let newest = sealed.last().copied().ok_or("no generation was sealed")?;
    let served = rt.query_text_with_pin(
        TextQuerySyntax::Native,
        &format!("needle_{}", SEALED.saturating_sub(1)),
        5,
        Some(pin(&rt, newest)),
    );
    if let Some(error) = served.typed_error {
        return Err(format!("newest generation refused after a reaped pin: {error}").into());
    }
    Ok(())
}

/// GC reports what it did, in bytes that match the disk.
///
/// One seal under the cap reclaims one generation per track and exactly
/// the bytes those directories held, nothing is deferred, and the retained
/// index bytes the byte cap is enforced against equal an independent
/// inode-set measurement of the retained directories.
#[test]
fn gc_metrics_report_reclaimed_and_retained_bytes_that_match_the_disk() -> TestResult {
    let mut rt = E2eRuntime::boot_with_history_max_generations(KEEP)?;
    let mut sealed = Vec::new();
    for index in 0..SEALED.saturating_sub(1) {
        rt.ingest_text(
            "repo",
            &format!("src/item_{index}.rs"),
            &format!("fn item_{index}() {{ needle_{index} }}"),
        )?;
        sealed.push(rt.seal()?);
        rt.activate_last_sealed_generation()?;
    }
    let before = Scrape::take(&mut rt)?;
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
    let index = SEALED.saturating_sub(1);
    rt.ingest_text(
        "repo",
        &format!("src/item_{index}.rs"),
        &format!("fn item_{index}() {{ needle_{index} }}"),
    )?;
    sealed.push(rt.seal()?);
    rt.activate_last_sealed_generation()?;

    let after = Scrape::take(&mut rt)?;
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
    Ok(())
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
    let mut rt = E2eRuntime::boot()?;
    let sealed = seal_generations(&mut rt)?;
    let orphan = sealed
        .get(1)
        .copied()
        .ok_or("fewer than two generations sealed")?;
    let mut rt = rt.reopen().with_history_max_generations(KEEP);
    rt.start()?;

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
    let inventory = rt.boot_inventory().ok_or("the daemon is running")?.clone();
    let reported: Vec<_> = inventory
        .lexical
        .orphaned
        .iter()
        .chain(inventory.semantic.orphaned.iter())
        .cloned()
        .collect();
    let reported_paths: BTreeSet<PathBuf> =
        reported.iter().map(|entry| entry.path.clone()).collect();
    if reported_paths != on_disk {
        return Err(format!("boot reports orphans {reported_paths:?}, on disk {on_disk:?}").into());
    }
    if reported
        .iter()
        .any(|entry| entry.reason != GenerationQuarantineReasonV1::Orphaned)
    {
        return Err("every orphan is reported under the orphan reason".into());
    }
    assert_unknown_on_every_route(&mut rt, orphan, "orphan")?;

    let orphan_code = GenerationQuarantineReasonV1::Orphaned.as_code_str();
    let listed = rt.quarantine_inventory()?;
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
            .discard_quarantined(QuarantineTargetV1::Generation(entry.clone()))?
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
    let after = rt.quarantine_inventory()?;
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
    Ok(())
}
