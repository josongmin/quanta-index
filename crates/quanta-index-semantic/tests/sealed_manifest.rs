//! QI-BB-017 — a sealed semantic generation commits to its files once; the
//! doors that admit it check the layout cheaply, and a bounded scrub proves
//! the bytes off the serving path.
//!
//! Before this, the activation validator and every cold open re-hashed the
//! whole dataset tree — every data file and every index file — once at
//! activation, again at the first query, again after every eviction and
//! restart. Now the seal writes a sealed manifest naming every dataset
//! file, the scope manifest and the build contract with length and SHA-256;
//! `validate_generation_identity` (activation, restart) and `open` (query)
//! hash only the two sidecars they decode and check every dataset file for
//! existence and length from directory metadata; the integrity scrub
//! hashes the dataset in bounded, resumable steps and quarantines a
//! generation durably when a byte does not match.
//!
//! The oracles are fault injection on the real files and independent
//! counts: every dataset file in turn is truncated, removed, and rewritten
//! at the same length; a foreign file is added; the sidecars are forged.
//! Shape defects must be refused at both doors under the typed code;
//! same-length rewrites must pass both doors (which proves the doors read
//! no dataset payload byte) and be found by the scrub, after which every
//! door refuses the generation typed and the inventory lists it. A delta
//! seal's hashed bytes are counted through the adapter's scrape and must
//! equal the bytes of the files the delta added, not the base's.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::error::Error;
use std::path::{Path, PathBuf};

use quanta_index_contract::{
    EmbeddingRecord, GenerationSnapshot, ManifestGeneration, RepoId, RevisionId,
    SearchPlaneTrackKind,
};
use quanta_index_core::{
    CoreError, GenerationIdentityValidatePort, GenerationQuarantineReasonV1,
    GenerationStorageKeyV1, IntegrityScrubBudgetV1, IntegrityScrubCursorV1,
    IntegrityScrubOutcomeV1, IntegrityScrubPort, MetricSourcePort, MetricValueV1,
    QuarantineDiscardOutcomeV1, QuarantinedGenerationDiscardPort, RequestBudgetV1,
    SealedGenerationReclaimOutcomeV1, SealedGenerationReclaimPort, SemanticIndexOpenPort,
};
use quanta_index_semantic::{
    SemanticAdapter, build_resident_batch_v1, inventory_persisted_generations,
    legacy_chunk_embedding_record_v1, sealed_replace_batch_v1,
};

type TestResult = Result<(), Box<dyn Error>>;

const SEALED_MANIFEST: &str = "semantic-sealed-manifest.cbor";
const SCOPE_MANIFEST: &str = "semantic-manifest.cbor";
const BUILD_CONTRACT: &str = "semantic-build-contract.cbor";
const DATASET: &str = "dataset";

fn repo() -> RepoId {
    RepoId::new("sealed-repo")
}

fn revision() -> RevisionId {
    RevisionId::new("sealed-rev")
}

fn embedding(id: &str, path: &str, vector: Vec<f32>) -> Result<EmbeddingRecord, Box<dyn Error>> {
    legacy_chunk_embedding_record_v1(id, path, vector)
        .map_err(|err| -> Box<dyn Error> { err.into() })
}

fn seal(adapter: &SemanticAdapter, generation: ManifestGeneration) -> TestResult {
    build_resident_batch_v1(
        adapter,
        &sealed_replace_batch_v1(
            repo(),
            revision(),
            generation,
            "src/lib.rs",
            vec![
                embedding("emb-a", "src/lib.rs", vec![1.0, 0.0, 0.0])?,
                embedding("emb-b", "src/lib.rs", vec![0.0, 1.0, 0.0])?,
            ],
            3,
        ),
    )?;
    Ok(())
}

fn identity(generation: ManifestGeneration) -> GenerationSnapshot {
    GenerationSnapshot {
        repo_id: repo(),
        revision_id: revision(),
        track: SearchPlaneTrackKind::Semantic,
        manifest_generation: generation,
        manifest_digest: format!("manifest:{}", generation.get()),
    }
}

fn generation_dir(root: &Path, generation: ManifestGeneration) -> PathBuf {
    GenerationStorageKeyV1::for_repo_revision(&repo(), &revision()).generation_dir(root, generation)
}

/// Every regular file under the dataset tree, sorted, relative to the
/// generation directory.
fn dataset_files(generation_dir: &Path) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let mut files = Vec::new();
    let mut pending = vec![generation_dir.join(DATASET)];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                pending.push(entry.path());
            } else {
                files.push(entry.path());
            }
        }
    }
    files.sort();
    Ok(files)
}

/// What every door said about a generation: the validator, the query's
/// open, and the activation's proven open (the proof whose handle is
/// promoted).
struct Doors {
    validate: Result<(), CoreError>,
    open: Result<usize, CoreError>,
    proven: Result<usize, CoreError>,
}

fn knock(adapter: &SemanticAdapter, generation: ManifestGeneration) -> Doors {
    let validate = adapter.validate_generation_identity(&identity(generation));
    let open = adapter
        .open(&repo(), &revision(), generation)
        .and_then(|searcher| searcher.search(&[1.0, 0.0, 0.0], 5, &RequestBudgetV1::unbounded()))
        .map(|hits| hits.len());
    let proven = adapter
        .open_proven(&identity(generation))
        .and_then(|searcher| searcher.search(&[1.0, 0.0, 0.0], 5, &RequestBudgetV1::unbounded()))
        .map(|hits| hits.len());
    Doors {
        validate,
        open,
        proven,
    }
}

fn typed_code<T>(result: &Result<T, CoreError>) -> Option<String> {
    match result {
        Err(CoreError::Typed { code, .. }) => Some(code.clone()),
        _ => None,
    }
}

fn expect_admitted(doors: &Doors, what: &str) -> TestResult {
    if let Err(err) = &doors.validate {
        return Err(format!("{what}: validator refused an intact generation: {err}").into());
    }
    for (door, result) in [("open", &doors.open), ("proven open", &doors.proven)] {
        match result {
            Ok(2) => {}
            Ok(other) => {
                return Err(format!(
                    "{what}: {door} of an intact generation served {other} hits, expected 2"
                )
                .into());
            }
            Err(err) => {
                return Err(format!("{what}: {door} refused an intact generation: {err}").into());
            }
        }
    }
    Ok(())
}

/// Both doors admitted the generation as far as the sealed layout goes;
/// whether a query then serves is the library's business, not the door's.
fn expect_doors_open(doors: &Doors, what: &str) -> TestResult {
    if let Err(err) = &doors.validate {
        return Err(
            format!("{what}: validator must admit a layout-intact generation: {err}").into(),
        );
    }
    if let Some(code) = typed_code(&doors.open) {
        return Err(
            format!("{what}: open must admit a layout-intact generation, got {code}").into(),
        );
    }
    Ok(())
}

fn expect_refused(doors: &Doors, what: &str, code: &str) -> TestResult {
    if typed_code(&doors.validate).as_deref() != Some(code) {
        return Err(format!(
            "{what}: validator answered {:?}, expected typed {code}",
            doors.validate
        )
        .into());
    }
    for (door, result) in [("open", &doors.open), ("proven open", &doors.proven)] {
        if typed_code(result).as_deref() != Some(code) {
            return Err(format!(
                "{what}: {door} answered {:?}, expected typed {code}",
                result.as_ref().map(|_| "served")
            )
            .into());
        }
    }
    Ok(())
}

fn flip_last_byte(path: &Path) -> TestResult {
    let mut bytes = std::fs::read(path)?;
    let last = bytes.len().checked_sub(1).ok_or("empty file")?;
    let byte = bytes.get_mut(last).ok_or("index")?;
    *byte ^= 0xff;
    std::fs::write(path, bytes)?;
    Ok(())
}

const UNBOUNDED: IntegrityScrubBudgetV1 = IntegrityScrubBudgetV1 {
    max_bytes: u64::MAX,
};

fn counter(adapter: &SemanticAdapter, name: &str) -> Result<u64, Box<dyn Error>> {
    adapter
        .scrape()?
        .into_iter()
        .find(|point| point.name == name)
        .and_then(|point| match point.value {
            MetricValueV1::Counter(value) => Some(value),
            MetricValueV1::Gauge(_) => None,
        })
        .ok_or_else(|| format!("no counter {name} in the scrape").into())
}

/// Every dataset file, truncated or removed, is refused by both doors.
///
/// A foreign file is refused too, because for a versioned dataset an extra
/// file can change what opens. Each is admitted again once restored.
///
/// A same-length rewrite passes both doors: the doors read no dataset
/// payload byte, so they cannot tell, and that is the oracle for their
/// cost. The scrub finds it every time, quarantines the generation
/// durably, and from then on both doors refuse it typed until the
/// quarantine is discarded.
#[test]
fn shape_defects_are_refused_at_the_doors_and_byte_defects_by_the_scrub() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = SemanticAdapter::with_state_root(root.clone())?;
    let generation = ManifestGeneration::new(1);
    seal(&adapter, generation)?;
    let generation_dir = generation_dir(&root, generation);
    expect_admitted(&knock(&adapter, generation), "intact")?;

    let files = dataset_files(&generation_dir)?;
    if files.is_empty() {
        return Err("a sealed generation has dataset files".into());
    }
    for file in &files {
        let name = file.display().to_string();
        let original = std::fs::read(file)?;

        // Truncated: the layout check refuses at both doors.
        let truncated = original
            .get(..original.len().saturating_sub(1))
            .ok_or("slice")?;
        std::fs::write(file, truncated)?;
        expect_refused(
            &knock(&adapter, generation),
            &format!("{name} truncated"),
            "GENERATION_SIDECAR_CORRUPT",
        )?;
        std::fs::write(file, &original)?;

        // Removed: likewise.
        std::fs::remove_file(file)?;
        expect_refused(
            &knock(&adapter, generation),
            &format!("{name} removed"),
            "GENERATION_SIDECAR_CORRUPT",
        )?;
        std::fs::write(file, &original)?;
        expect_admitted(&knock(&adapter, generation), &format!("{name} restored"))?;
    }

    // A foreign file the seal never committed to.
    let foreign = generation_dir
        .join(DATASET)
        .join("semantic.lance")
        .join("_versions")
        .join("999.manifest");
    std::fs::create_dir_all(foreign.parent().ok_or("foreign parent")?)?;
    std::fs::write(&foreign, b"foreign version")?;
    expect_refused(
        &knock(&adapter, generation),
        "foreign file",
        "GENERATION_SIDECAR_CORRUPT",
    )?;
    std::fs::remove_file(&foreign)?;
    expect_admitted(&knock(&adapter, generation), "foreign file removed")?;

    // Bit-flipped at the same length, one file at a time: admitted at
    // both doors, found by the scrub, quarantined, refused, discarded.
    // Every round reseals the generation, and the library names its files
    // afresh, so the file set is listed again per round.
    let mut scrubbed_files = 0_u64;
    for position in 0..files.len() {
        let files = dataset_files(&generation_dir)?;
        let Some(file) = files.get(position) else {
            return Err(
                format!("a resealed generation has fewer files at round {position}").into(),
            );
        };
        let name = file.display().to_string();
        let original = std::fs::read(file)?;
        if original.is_empty() {
            continue;
        }
        flip_last_byte(file)?;
        expect_doors_open(&knock(&adapter, generation), &format!("{name} bit-flipped"))?;
        let report = adapter.scrub(&identity(generation), None, UNBOUNDED)?;
        let IntegrityScrubOutcomeV1::Corrupt { quarantined } = &report.outcome else {
            return Err(
                format!("{name} bit-flipped: the scrub must find it, got {report:?}").into(),
            );
        };
        if quarantined.reason != GenerationQuarantineReasonV1::ContentCorrupt
            || quarantined.path != generation_dir
            || !quarantined.detail.contains("content digest differs")
        {
            return Err(
                format!("{name} bit-flipped: unexpected quarantine {quarantined:?}").into(),
            );
        }
        // The scrub read every committed file up to and including the
        // damaged one, and no more: its bytes are accounted for.
        let expected_bytes: u64 = files
            .iter()
            .take(position.saturating_add(1))
            .map(|path| std::fs::metadata(path).map(|meta| meta.len()))
            .sum::<Result<u64, _>>()?;
        if report.bytes_read != expected_bytes {
            return Err(format!(
                "{name} bit-flipped: the scrub read {} bytes, expected {expected_bytes}",
                report.bytes_read
            )
            .into());
        }
        expect_refused(
            &knock(&adapter, generation),
            &format!("{name} bit-flipped and quarantined"),
            "GENERATION_QUARANTINED",
        )?;
        let restarted = SemanticAdapter::with_state_root(root.clone())?;
        expect_refused(
            &knock(&restarted, generation),
            &format!("{name} bit-flipped and quarantined, after a restart"),
            "GENERATION_QUARANTINED",
        )?;
        std::fs::write(file, &original)?;
        // The quarantine is durable: restoring the bytes does not lift it.
        expect_refused(
            &knock(&adapter, generation),
            &format!("{name} restored while quarantined"),
            "GENERATION_QUARANTINED",
        )?;
        let inventory = inventory_persisted_generations(&root)?;
        let Some(entry) = inventory
            .quarantined
            .iter()
            .find(|entry| entry.path == generation_dir)
        else {
            return Err(format!("{name}: the inventory must list the quarantine").into());
        };
        if entry.reason != GenerationQuarantineReasonV1::ContentCorrupt
            || inventory
                .sealed
                .iter()
                .any(|record| record.generation == generation)
        {
            return Err(
                format!("{name}: the inventory must not seed a quarantined generation").into(),
            );
        }
        // Discarding the quarantine removes the whole generation; it is
        // sealed again for the next file.
        if !matches!(
            adapter.discard_quarantined_generation(entry)?,
            QuarantineDiscardOutcomeV1::Discarded { .. }
        ) || generation_dir.exists()
        {
            return Err(format!("{name}: the discard must remove the generation").into());
        }
        seal(&adapter, generation)?;
        expect_admitted(&knock(&adapter, generation), &format!("{name} resealed"))?;
        scrubbed_files = scrubbed_files.saturating_add(1);
    }
    if scrubbed_files == 0 {
        return Err("at least one dataset file has bytes to flip".into());
    }
    Ok(())
}

/// The doors read no dataset payload byte.
///
/// With every payload file made unreadable, activation validation and the
/// cold open still admit the
/// generation (its layout is intact and its sidecars decode), and only a
/// query that reads the rows fails. A scrub over the same generation
/// cannot claim anything and answers with an I/O error, not a verdict.
#[cfg(unix)]
#[test]
fn the_doors_read_no_dataset_payload_byte() -> TestResult {
    use std::os::unix::fs::PermissionsExt as _;
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = SemanticAdapter::with_state_root(root.clone())?;
    let generation = ManifestGeneration::new(9);
    seal(&adapter, generation)?;
    let generation_dir = generation_dir(&root, generation);
    let payload: Vec<PathBuf> = dataset_files(&generation_dir)?
        .into_iter()
        .filter(|path| {
            path.parent()
                .and_then(Path::file_name)
                .is_some_and(|name| name == "data")
        })
        .collect();
    if payload.is_empty() {
        return Err("a sealed generation has payload files under data/".into());
    }
    let mut modes = Vec::new();
    for path in &payload {
        let permissions = std::fs::metadata(path)?.permissions();
        modes.push((path.clone(), permissions.mode()));
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o000))?;
    }
    // A privileged test process reads through the mode; the doors' proof
    // still holds, only the query-side refusal is not observable then.
    let unreadable = payload
        .first()
        .is_some_and(|path| std::fs::File::open(path).is_err());

    let validate = adapter.validate_generation_identity(&identity(generation));
    let opened = adapter.open(&repo(), &revision(), generation);
    for (path, mode) in &modes {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(*mode))?;
    }
    if let Err(err) = validate {
        return Err(format!("the validator read a payload byte: {err}").into());
    }
    let searcher = opened.map_err(|err| format!("the open read a payload byte: {err}"))?;
    if unreadable {
        // The rows are unreadable at the moment the query runs: the
        // searcher was opened without them.
        for (path, _) in &modes {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o000))?;
        }
        let served = searcher.search(&[1.0, 0.0, 0.0], 5, &RequestBudgetV1::unbounded());
        for (path, mode) in &modes {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(*mode))?;
        }
        if served.is_ok() {
            return Err("a query over unreadable rows cannot serve".into());
        }
    }
    expect_admitted(&knock(&adapter, generation), "payload readable again")?;
    let report = adapter.scrub(&identity(generation), None, UNBOUNDED)?;
    if report.outcome != IntegrityScrubOutcomeV1::Completed {
        return Err(format!("the bytes were never changed: {report:?}").into());
    }
    Ok(())
}

/// The sidecars the manifest commits to are refused when forged, missing,
/// or absent as a whole — at the doors, since the doors decode them.
#[test]
fn both_doors_refuse_forged_or_missing_sidecars() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = SemanticAdapter::with_state_root(root.clone())?;
    let generation = ManifestGeneration::new(2);
    seal(&adapter, generation)?;
    let generation_dir = generation_dir(&root, generation);

    for sidecar in [SCOPE_MANIFEST, BUILD_CONTRACT] {
        let path = generation_dir.join(sidecar);
        let original = std::fs::read(&path)?;
        flip_last_byte(&path)?;
        expect_refused(
            &knock(&adapter, generation),
            &format!("{sidecar} bit-flipped"),
            "GENERATION_SIDECAR_CORRUPT",
        )?;
        std::fs::remove_file(&path)?;
        expect_refused(
            &knock(&adapter, generation),
            &format!("{sidecar} removed"),
            "GENERATION_SIDECAR_CORRUPT",
        )?;
        std::fs::write(&path, &original)?;
        expect_admitted(&knock(&adapter, generation), &format!("{sidecar} restored"))?;
    }

    // No sealed manifest at all: a generation sealed before the format.
    let sealed_manifest = generation_dir.join(SEALED_MANIFEST);
    let original = std::fs::read(&sealed_manifest)?;
    std::fs::remove_file(&sealed_manifest)?;
    expect_refused(
        &knock(&adapter, generation),
        "sealed manifest missing",
        "GENERATION_MANIFEST_MISSING",
    )?;
    std::fs::write(&sealed_manifest, &original)?;
    expect_admitted(&knock(&adapter, generation), "sealed manifest restored")
}

/// A sealed manifest copied from another generation binds to that
/// generation's identity digest and is refused here.
#[test]
fn the_sealed_manifest_binds_the_identity_digest() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = SemanticAdapter::with_state_root(root.clone())?;
    let first = ManifestGeneration::new(3);
    let second = ManifestGeneration::new(4);
    seal(&adapter, first)?;
    seal(&adapter, second)?;
    let _copied = std::fs::copy(
        generation_dir(&root, first).join(SEALED_MANIFEST),
        generation_dir(&root, second).join(SEALED_MANIFEST),
    )?;
    expect_refused(
        &knock(&adapter, second),
        "sealed manifest from another generation",
        "GENERATION_IDENTITY_DIGEST_MISMATCH",
    )
}

/// Opening writes nothing the commitment would notice: the same generation
/// is admitted by both doors repeatedly, with a search in between, and a
/// scrub after all of it still finds every byte as sealed.
#[test]
fn a_sealed_generation_is_admitted_repeatedly() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = SemanticAdapter::with_state_root(root)?;
    let generation = ManifestGeneration::new(5);
    seal(&adapter, generation)?;
    for round in 0..3 {
        expect_admitted(&knock(&adapter, generation), &format!("round {round}"))?;
    }
    let report = adapter.scrub(&identity(generation), None, UNBOUNDED)?;
    if report.outcome != IntegrityScrubOutcomeV1::Completed {
        return Err(format!("three opens must leave the bytes as sealed, got {report:?}").into());
    }
    Ok(())
}

/// A scrub resumed over a reclaimed generation meets it gone, typed.
///
/// The generation is reclaimed between two steps of one pass. The next
/// step is refused as not ready — never a corruption verdict over files
/// removed on purpose — and leaves nothing behind: no directory recreated
/// for a receipt, nothing quarantined, no candidate left.
#[test]
fn a_scrub_resumed_over_a_reclaimed_generation_is_refused_not_quarantined() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = SemanticAdapter::with_state_root(root.clone())?;
    let generation = ManifestGeneration::new(6);
    seal(&adapter, generation)?;
    let one_byte = IntegrityScrubBudgetV1 { max_bytes: 1 };
    let first = adapter.scrub(&identity(generation), None, one_byte)?;
    let IntegrityScrubOutcomeV1::Paused { cursor } = first.outcome else {
        return Err(format!("a one-byte first step pauses: {first:?}").into());
    };
    let reclaimed = adapter.reclaim_sealed_generation(&identity(generation))?;
    if !matches!(
        reclaimed,
        SealedGenerationReclaimOutcomeV1::Reclaimed { .. }
    ) {
        return Err(format!("the sealed generation is reclaimed: {reclaimed:?}").into());
    }
    match adapter.scrub(&identity(generation), Some(cursor), one_byte) {
        Err(CoreError::NotReady(_)) => {}
        other => {
            return Err(format!("a reclaimed generation is refused not ready: {other:?}").into());
        }
    }
    if generation_dir(&root, generation).exists() {
        return Err("the refused step recreated the reclaimed directory".into());
    }
    let inventory = inventory_persisted_generations(&root)?;
    if !inventory.sealed.is_empty() || !inventory.quarantined.is_empty() {
        return Err(format!("nothing is left to list: {inventory:?}").into());
    }
    if !adapter.scrub_candidates()?.is_empty() {
        return Err("a reclaimed generation is no scrub candidate".into());
    }
    Ok(())
}

/// The scrub is bounded and resumable.
///
/// With a one-byte budget every step hashes exactly one committed file and
/// pauses with a cursor, the steps
/// together read every committed byte exactly once, the completed pass
/// leaves a receipt the candidates list reports, and a step resumed past a
/// file that was damaged after it was verified still finds the damage
/// through the layout check when it is a shape defect.
#[test]
fn the_scrub_is_bounded_resumable_and_accounted() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = SemanticAdapter::with_state_root(root.clone())?;
    let generation = ManifestGeneration::new(6);
    seal(&adapter, generation)?;
    let generation_dir = generation_dir(&root, generation);
    let files = dataset_files(&generation_dir)?;
    let committed_bytes: u64 = files
        .iter()
        .map(|path| std::fs::metadata(path).map(|meta| meta.len()))
        .sum::<Result<u64, _>>()?;

    let candidates = adapter.scrub_candidates()?;
    let Some(candidate) = candidates
        .iter()
        .find(|candidate| candidate.identity == identity(generation))
    else {
        return Err(format!("the sealed generation is a scrub candidate: {candidates:?}").into());
    };
    if candidate.last_completed_unix.is_some() {
        return Err("a never-scrubbed generation has no completion".into());
    }

    let mut cursor: Option<IntegrityScrubCursorV1> = None;
    let mut steps = 0_u64;
    let mut bytes_read = 0_u64;
    let mut files_verified = 0_u64;
    loop {
        let report = adapter.scrub(
            &identity(generation),
            cursor,
            IntegrityScrubBudgetV1 { max_bytes: 1 },
        )?;
        steps = steps.saturating_add(1);
        bytes_read = bytes_read.saturating_add(report.bytes_read);
        files_verified = files_verified.saturating_add(report.files_verified);
        if report.files_verified != 1 {
            return Err(format!(
                "a one-byte budget hashes exactly one file per step, got {report:?}"
            )
            .into());
        }
        match report.outcome {
            IntegrityScrubOutcomeV1::Paused { cursor: next } => {
                if next.next_artifact != steps {
                    return Err(format!("the cursor advances one file per step: {next:?}").into());
                }
                cursor = Some(next);
            }
            IntegrityScrubOutcomeV1::Completed => break,
            IntegrityScrubOutcomeV1::Corrupt { quarantined } => {
                return Err(format!("an intact generation is not corrupt: {quarantined:?}").into());
            }
        }
    }
    if files_verified != u64::try_from(files.len())? || bytes_read != committed_bytes {
        return Err(format!(
            "the steps together verify every committed file ({files_verified} of {}) and byte ({bytes_read} of {committed_bytes}) exactly once",
            files.len()
        )
        .into());
    }
    let candidates = adapter.scrub_candidates()?;
    let Some(candidate) = candidates
        .iter()
        .find(|candidate| candidate.identity == identity(generation))
    else {
        return Err("still a candidate after completion".into());
    };
    if candidate.last_completed_unix.is_none() {
        return Err("a completed pass leaves a receipt the candidates report".into());
    }

    // A file truncated after an earlier step verified it is still found by
    // a later step, through the layout check that precedes every step.
    let Some(first) = files.first() else {
        return Err("dataset files".into());
    };
    let original = std::fs::read(first)?;
    std::fs::write(
        first,
        original
            .get(..original.len().saturating_sub(1))
            .ok_or("the dataset file is not empty")?,
    )?;
    let resumed = adapter.scrub(
        &identity(generation),
        Some(IntegrityScrubCursorV1 { next_artifact: 1 }),
        UNBOUNDED,
    )?;
    let IntegrityScrubOutcomeV1::Corrupt { quarantined } = resumed.outcome else {
        return Err(format!("a resumed step checks the whole layout first: {resumed:?}").into());
    };
    if !quarantined.detail.contains("bytes on disk") {
        return Err(format!("the quarantine names the resized file: {quarantined:?}").into());
    }
    Ok(())
}

/// The bytes a delta seal hashes are proportional to what the delta added
/// (QI-BB-006 #4).
///
/// Every file it still shares with its base by inode
/// carries the base's digest, and the adapter's scrape counts hashed
/// against inherited bytes so the proportion is externally observable.
/// The inherited digests are the truth: a scrub over the delta hashes
/// every file from scratch and finds them all as sealed.
#[test]
fn a_delta_seal_hashes_only_the_files_it_added() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = SemanticAdapter::with_state_root(root.clone())?;
    let base = ManifestGeneration::new(7);
    let delta = ManifestGeneration::new(8);
    seal(&adapter, base)?;
    let base_dir = generation_dir(&root, base);
    let base_files: BTreeMap<PathBuf, u64> = dataset_files(&base_dir)?
        .into_iter()
        .map(|path| {
            let relative = path.strip_prefix(&base_dir).map(Path::to_path_buf);
            let bytes = std::fs::metadata(&path).map(|meta| meta.len());
            relative
                .map_err(Box::<dyn Error>::from)
                .and_then(|relative| bytes.map(|bytes| (relative, bytes)).map_err(Box::from))
        })
        .collect::<Result<_, _>>()?;
    let hashed_after_base = counter(&adapter, "semantic_seal_hashed_bytes_total")?;
    let inherited_after_base = counter(&adapter, "semantic_seal_inherited_bytes_total")?;
    if inherited_after_base != 0 || hashed_after_base == 0 {
        return Err("a fresh seal hashes everything and inherits nothing".into());
    }

    let mut batch = sealed_replace_batch_v1(
        repo(),
        revision(),
        delta,
        "src/new.rs",
        vec![embedding("emb-c", "src/new.rs", vec![0.0, 0.0, 1.0])?],
        3,
    );
    batch.mode = quanta_index_contract::BatchIngestMode::Delta;
    batch.base_generation = Some(base);
    build_resident_batch_v1(&adapter, &batch)?;

    let delta_dir = generation_dir(&root, delta);
    let mut added_bytes = 0_u64;
    let mut shared_bytes = 0_u64;
    let mut shared_files = 0_u64;
    for path in dataset_files(&delta_dir)? {
        let relative = path.strip_prefix(&delta_dir)?.to_path_buf();
        let bytes = std::fs::metadata(&path)?.len();
        let shared = match base_files.get(&relative) {
            Some(base_bytes) if *base_bytes == bytes => {
                same_file(&path, &base_dir.join(&relative))?
            }
            Some(_) | None => false,
        };
        if shared {
            shared_bytes = shared_bytes.saturating_add(bytes);
            shared_files = shared_files.saturating_add(1);
        } else {
            added_bytes = added_bytes.saturating_add(bytes);
        }
    }
    if shared_files == 0 || added_bytes == 0 {
        return Err(format!(
            "a delta shares files with its base and adds its own: shared {shared_files}, added {added_bytes} bytes"
        )
        .into());
    }
    let hashed = counter(&adapter, "semantic_seal_hashed_bytes_total")? - hashed_after_base;
    let inherited = counter(&adapter, "semantic_seal_inherited_bytes_total")?;
    let inherited_files = counter(&adapter, "semantic_seal_inherited_files_total")?;
    if hashed != added_bytes || inherited != shared_bytes || inherited_files != shared_files {
        return Err(format!(
            "the delta seal must hash exactly the added bytes ({hashed} vs {added_bytes}) and inherit exactly the shared ones ({inherited} vs {shared_bytes} in {inherited_files} vs {shared_files} files)"
        )
        .into());
    }
    // The inherited digests are honest: a from-scratch scrub agrees.
    let report = adapter.scrub(&identity(delta), None, UNBOUNDED)?;
    if report.outcome != IntegrityScrubOutcomeV1::Completed
        || report.bytes_read != added_bytes.saturating_add(shared_bytes)
    {
        return Err(format!("the scrub hashes every delta byte and agrees: {report:?}").into());
    }
    expect_admitted(&knock(&adapter, base), "base after the delta")?;
    Ok(())
}

#[cfg(unix)]
fn same_file(left: &Path, right: &Path) -> Result<bool, Box<dyn Error>> {
    use std::os::unix::fs::MetadataExt as _;
    let left = std::fs::metadata(left)?;
    let right = std::fs::metadata(right)?;
    Ok(left.dev() == right.dev() && left.ino() == right.ino())
}

#[cfg(not(unix))]
fn same_file(_left: &Path, _right: &Path) -> Result<bool, Box<dyn Error>> {
    Ok(false)
}
