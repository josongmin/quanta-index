//! QI-BB-017 / QI-BB-027 — the doors are cheap, the scrub is deep, and an
//! earlier format is refused rather than served.
//!
//! Before this, every semantic open re-hashed the whole dataset tree, so a
//! same-length byte defect was found at activation and at every cold open
//! by reading every byte, twice per generation and again after every
//! eviction and restart. Now the doors check the layout and the sidecars
//! only, and the daemon proves the bytes as quota'd maintenance: a bounded
//! scrub step every interval, off the serving path. The oracles are
//! external: a daemon restarted over a byte-tampered active generation
//! boots and proves its active pair without reading the tamper; the scrub
//! then finds it, the metrics say so, the quarantine surface lists the
//! generation as content-corrupt, and the next semantic query — even one
//! whose handle was resident before the scrub — is refused typed. A
//! generation written under an earlier manifest format is set aside at
//! boot under the format reason and is absent from readiness.

#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use quanta_index_catalog::SqliteCatalog;
use quanta_index_contract::{
    ChunkId, ChunkRecord, FileId, GenerationPin, ManifestGeneration, MetricsSnapshotV1,
    QuarantineDiscardOutcomeDtoV1, QuarantineTargetV1, QuarantinedGenerationEntryV1, RepoId,
    RepoMapExactnessSummary, RepoMapFileNode, RepoMapGraphCoverage, RepoMapGraphCoverageClass,
    RepoMapItemIndexAvailability, RepoMapNode, RepoMapPublishBundleRequestV2,
    RepoMapRedactionState, RepoMapSourceBundle, RepoRelativePath, RevisionId,
    SearchCorpusActiveHeadV1, SearchPlaneErrorCodeV2,
    SearchPlaneRollbackSearchCorpusGenerationCasRequest, SearchPlaneTrackKind, TextQueryResponse,
    lex::LanguageCode,
};
use quanta_index_core::{
    GenerationQuarantineReasonV1, GenerationStorageKeyV1, IntegrityScrubPolicyV1,
};
use quanta_index_repomap::RepoMapGenerationStore;
use quanta_index_sdk::{QuantaIndex, SdkError};
use quanta_index_searchd_harness::{
    legacy_reference::{
        LegacySearchdBinary, LegacySearchdCommand, SearchdBinaryProcess, daemon_socket_paths,
        legacy_source_batch, legacy_source_delta_from_chunks, publish_legacy_source_batch,
    },
    private_tempdir,
    scale_runtime_policy::DEFAULT_HISTORY_MAX_GENERATIONS,
};
use sha2::{Digest as _, Sha256};

use crate::legacy_start_probe;

type TestResult = Result<(), Box<dyn Error>>;

const SEMANTIC_MANIFEST: &str = "semantic-manifest.cbor";
const SEALED_MANIFEST: &str = "semantic-sealed-manifest.cbor";
/// The durable record of a completed semantic scrub pass.
const SEMANTIC_SCRUB_RECEIPT: &str = "semantic-scrub-receipt.cbor";
/// How long the daemon is given to run enough scrub steps; the scrub is
/// paced at [`SCRUB_INTERVAL`] and every wait polls an observable.
const SCRUB_WAIT: Duration = Duration::from_secs(120);
const SCRUB_INTERVAL: Duration = Duration::from_millis(100);
const DORMANT_SCRUB: Duration = Duration::from_secs(24 * 60 * 60);
const LEXICAL_MANIFEST: &str = "text-authority/manifest.cbor";

fn fixture_pin(generation: u64) -> Result<GenerationPin, Box<dyn Error>> {
    Ok(GenerationPin::new(
        RepoId::new("repo-e2e")?,
        RevisionId::new("rev-e2e")?,
        ManifestGeneration::new(generation),
    ))
}

fn configured_command<'binary>(
    binary: &'binary LegacySearchdBinary,
    root: &Path,
    interval: Duration,
    max_bytes_per_step: u64,
) -> Result<LegacySearchdCommand<'binary>, Box<dyn Error>> {
    let mut command = binary.benchmark_command(root, DEFAULT_HISTORY_MAX_GENERATIONS)?;
    let _configured = command
        .env("QUANTA_INDEX_MAINTENANCE_TICK_MS", "50")
        .env(
            "QUANTA_INDEX_INTEGRITY_SCRUB_INTERVAL_MS",
            interval.as_millis().to_string(),
        )
        .env(
            "QUANTA_INDEX_INTEGRITY_SCRUB_MAX_BYTES_PER_STEP",
            max_bytes_per_step.to_string(),
        );
    Ok(command)
}

fn start_scrub(
    binary: &LegacySearchdBinary,
    root: &Path,
) -> Result<SearchdBinaryProcess, Box<dyn Error>> {
    // One file per step so the durable scrub cursor is exercised.
    let command = configured_command(binary, root, SCRUB_INTERVAL, 1)?;
    SearchdBinaryProcess::start_with_pinned_command(root, command)
}

fn publish_file(
    client: &QuantaIndex,
    pin: &GenerationPin,
    path: &str,
    body: &str,
    active: Option<SearchCorpusActiveHeadV1>,
) -> Result<SearchCorpusActiveHeadV1, Box<dyn Error>> {
    let generation = pin.manifest_generation.get();
    let batch = if generation == 1 {
        legacy_source_batch(
            &pin.repo_id,
            &pin.revision_id,
            generation,
            None,
            &[(path, body, 1)],
        )?
    } else {
        legacy_source_delta_from_chunks(
            &pin.repo_id,
            &pin.revision_id,
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
        )?
    };
    let (head, receipt) = publish_legacy_source_batch(client, &batch, active)?;
    if !receipt.sealed
        || receipt.accepted_replace_scopes != 1
        || receipt.accepted_semantic_replace_scopes != 1
    {
        return Err(format!("expected one sealed Source file in {pin:?}: {receipt:?}").into());
    }
    Ok(head)
}

fn generation_dir(
    root: &Path,
    track: &str,
    pin: &GenerationPin,
) -> Result<PathBuf, Box<dyn Error>> {
    let track_dir = std::fs::canonicalize(root)?.join("indexes").join(track);
    Ok(
        GenerationStorageKeyV1::for_repo_revision(&pin.repo_id, &pin.revision_id)
            .generation_dir(&track_dir, pin.manifest_generation),
    )
}

/// Pool objects of the `FileBody` kind, identified by the durable object
/// envelope. A one-file publication below must add exactly one such body.
fn pooled_file_bodies(root: &Path) -> Result<BTreeSet<PathBuf>, Box<dyn Error>> {
    let objects = root.join("indexes/lexical/lexical-object-pool/objects");
    let mut bodies = BTreeSet::new();
    for shard in std::fs::read_dir(objects)? {
        let shard = shard?;
        if !shard.file_type()?.is_dir() {
            continue;
        }
        for object in std::fs::read_dir(shard.path())? {
            let object = object?;
            if !object.file_type()?.is_file() {
                continue;
            }
            let bytes = std::fs::read(object.path())?;
            if bytes.get(..8) == Some(b"QLSTOR01".as_slice()) && bytes.get(10) == Some(&1) {
                let _new = bodies.insert(object.path());
            }
        }
    }
    Ok(bodies)
}

fn pinned_file_query(
    client: &QuantaIndex,
    pin: &GenerationPin,
    needle: &str,
) -> Result<TextQueryResponse, SdkError> {
    client
        .lexical()
        .query()
        .code_search(needle)
        .pinned(pin.clone())
        .top_k(5)
        .execute()
}

fn assert_pinned_file_path(
    client: &QuantaIndex,
    pin: &GenerationPin,
    needle: &str,
    path: &str,
) -> TestResult {
    let answer = pinned_file_query(client, pin, needle)?;
    let [hit] = answer.results.as_slice() else {
        return Err(format!("expected one pinned {path} result: {answer:?}").into());
    };
    if answer.generation != *pin || hit.repo_relative_path.as_str() != path {
        return Err(format!("expected {pin:?} and {path}: {answer:?}").into());
    }
    Ok(())
}

/// One corrupt pooled `FileBody` must fence every retained generation that
/// references it. An older generation sealed before that object existed
/// remains queryable through its own pin.
#[test]
fn shared_lexical_body_corruption_quarantines_all_referencing_generations() -> TestResult {
    const SAFE_PATH: &str = "src/safe.rs";
    const SHARED_PATH: &str = "src/shared.rs";
    const SAFE_NEEDLE: &str = "safe_generation_needle_71";
    const SHARED_NEEDLE: &str = "shared_generation_needle_83";

    let root = private_tempdir()?;
    let binary = LegacySearchdBinary::from_environment()?;
    let daemon = start_scrub(&binary, root.path())?;
    let client = daemon.connect()?;
    let unaffected = fixture_pin(1)?;
    let g1 = publish_file(
        &client,
        &unaffected,
        SAFE_PATH,
        "fn safe_generation_needle_71() {}",
        None,
    )?;
    let before_shared = pooled_file_bodies(root.path())?;

    let first_referencing = fixture_pin(2)?;
    let g2 = publish_file(
        &client,
        &first_referencing,
        SHARED_PATH,
        "fn shared_generation_needle_83() {}",
        Some(g1),
    )?;
    let after_shared = pooled_file_bodies(root.path())?;
    let added: Vec<PathBuf> = after_shared.difference(&before_shared).cloned().collect();
    let [shared_body] = added.as_slice() else {
        return Err(format!("one target FileBody was published, found {added:?}").into());
    };

    let second_referencing = fixture_pin(3)?;
    let _g3 = publish_file(
        &client,
        &second_referencing,
        "src/other.rs",
        "fn unrelated_generation_needle_97() {}",
        Some(g2),
    )?;
    assert_pinned_file_path(&client, &unaffected, SAFE_NEEDLE, SAFE_PATH)?;
    assert_pinned_file_path(&client, &first_referencing, SHARED_NEEDLE, SHARED_PATH)?;
    assert_pinned_file_path(&client, &second_referencing, SHARED_NEEDLE, SHARED_PATH)?;
    if !pooled_file_bodies(root.path())?.contains(shared_body) {
        return Err("the shared FileBody was lost before corruption".into());
    }

    // The scheduled scrub discovers the changed bytes before a selected read.
    flip_middle_byte(shared_body)?;
    let _found = wait_for_scrape(&client, "the shared lexical body defect", |snapshot| {
        Ok(counter(snapshot, "scrub_corruptions_total")? >= 1)
    })?;
    let listed: BTreeSet<String> = client
        .quarantine()
        .inventory()?
        .lexical
        .iter()
        .filter(|entry| entry.reason == GenerationQuarantineReasonV1::ContentCorrupt.as_code_str())
        .map(|entry| entry.path.clone())
        .collect();
    let expected = BTreeSet::from([
        generation_dir(root.path(), "lexical", &first_referencing)?
            .display()
            .to_string(),
        generation_dir(root.path(), "lexical", &second_referencing)?
            .display()
            .to_string(),
    ]);
    if listed != expected {
        return Err(format!(
            "shared corruption quarantine fan-out: expected {expected:?}, found {listed:?}"
        )
        .into());
    }
    for pin in [&first_referencing, &second_referencing] {
        let refused = pinned_file_query(&client, pin, SHARED_NEEDLE);
        if !matches!(
            &refused,
            Err(SdkError::Remote {
                code: SearchPlaneErrorCodeV2::GenerationQuarantined,
                ..
            })
        ) {
            return Err(format!(
                "g{} was not fenced after shared corruption: {refused:?}",
                pin.manifest_generation.get()
            )
            .into());
        }
    }
    assert_pinned_file_path(&client, &unaffected, SAFE_NEEDLE, SAFE_PATH)?;
    drop(client);
    daemon.stop()?;
    binary.verify()
}

/// Compact metadata can admit a body-only defect at restart. A selected read
/// must reject the bad body, and maintenance must quarantine its generation.
#[test]
fn corrupted_pooled_file_body_boots_then_refuses_selected_read_and_quarantines() -> TestResult {
    const PATH: &str = "src/boot-corrupt.rs";
    const NEEDLE: &str = "boot_corrupt_needle_47";

    let root = private_tempdir()?;
    let binary = LegacySearchdBinary::from_environment()?;
    let daemon = SearchdBinaryProcess::start_with_pinned_command(
        root.path(),
        dormant_command(&binary, root.path())?,
    )?;
    let client = daemon.connect()?;
    let pin = fixture_pin(1)?;
    let _head = publish_file(&client, &pin, PATH, "fn boot_corrupt_needle_47() {}", None)?;
    assert_pinned_file_path(&client, &pin, NEEDLE, PATH)?;
    let bodies = pooled_file_bodies(root.path())?;
    if bodies.len() != 1 {
        return Err(format!("expected one active FileBody, found {bodies:?}").into());
    }
    let body = bodies.iter().next().ok_or("active FileBody absent")?;

    drop(client);
    daemon.stop()?;
    flip_middle_byte(body)?;
    let damaged_bytes = std::fs::read(body)?;
    // Exercise the selected read before the scheduler can quarantine this body.
    let cold = SearchdBinaryProcess::start_with_pinned_command(
        root.path(),
        dormant_command(&binary, root.path())?,
    )
    .map_err(|error| format!("body-only corruption refused compact-root boot: {error}"))?;
    let client = cold.connect()?;
    let boot = client.observability().metrics_snapshot()?;
    require_boot_count(&boot, "boot_active_pairs_validated", 1.0)?;
    require_boot_count(&boot, "boot_lexical_quarantined_generations", 0.0)?;
    let selected = pinned_file_query(&client, &pin, NEEDLE);
    if !matches!(
        &selected,
        Err(SdkError::Remote {
            code: SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
            ..
        })
    ) {
        return Err(format!(
            "corrupted FileBody selected read was not refused typed: {selected:?}"
        )
        .into());
    }
    let after_read = client.quarantine().inventory()?;
    let metrics = client.observability().metrics_snapshot()?;
    if !after_read.lexical.is_empty()
        || counter(&metrics, "scrub_corruptions_total")? != 0
        || std::fs::read(body)? != damaged_bytes
    {
        return Err(format!("the selected read changed the damaged state: {after_read:?}").into());
    }
    drop(client);
    cold.stop()?;

    let reopened = start_scrub(&binary, root.path())?;
    let client = reopened.connect()?;
    let found = wait_for_scrape(&client, "the restarted FileBody defect", |snapshot| {
        Ok(counter(snapshot, "scrub_corruptions_total")? >= 1)
    })?;
    if counter(&found, "scrub_corruptions_total")? != 1 {
        return Err(format!("expected one FileBody corruption: {:?}", found.counters).into());
    }
    let listed: BTreeSet<String> = client
        .quarantine()
        .inventory()?
        .lexical
        .iter()
        .filter(|entry| entry.reason == GenerationQuarantineReasonV1::ContentCorrupt.as_code_str())
        .map(|entry| entry.path.clone())
        .collect();
    let expected = BTreeSet::from([generation_dir(root.path(), "lexical", &pin)?
        .display()
        .to_string()]);
    if listed != expected {
        return Err(
            format!("restarted body quarantine: expected {expected:?}, found {listed:?}").into(),
        );
    }
    let fenced = pinned_file_query(&client, &pin, NEEDLE);
    if !matches!(
        &fenced,
        Err(SdkError::Remote {
            code: SearchPlaneErrorCodeV2::GenerationQuarantined,
            ..
        })
    ) {
        return Err(format!("restarted body was not fenced after scrub: {fenced:?}").into());
    }
    drop(client);
    reopened.stop()?;
    binary.verify()
}

/// Every regular file under the generation's `dataset/data/`: the rows.
fn payload_files(generation_dir: &Path) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let mut files = Vec::new();
    let mut pending = vec![generation_dir.join("dataset")];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                pending.push(entry.path());
            } else if directory.file_name().is_some_and(|name| name == "data") {
                files.push(entry.path());
            }
        }
    }
    files.sort();
    Ok(files)
}

/// Flip one byte in the middle of `path`: same length, different content.
fn flip_middle_byte(path: &Path) -> TestResult {
    let mut bytes = std::fs::read(path)?;
    let middle = bytes.len().div_euclid(2);
    let byte = bytes.get_mut(middle).ok_or("empty file")?;
    *byte ^= 0xff;
    std::fs::write(path, bytes)?;
    Ok(())
}

fn flip_last_byte(path: &Path) -> TestResult {
    let mut bytes = std::fs::read(path)?;
    let last = bytes.last_mut().ok_or("empty lexical manifest")?;
    *last ^= 0xff;
    Ok(std::fs::write(path, bytes)?)
}

fn tamper_semantic_row_count(path: &Path) -> TestResult {
    let bytes = std::fs::read(path)?;
    let mut manifest: ciborium::Value = ciborium::from_reader(bytes.as_slice())?;
    let ciborium::Value::Map(fields) = &mut manifest else {
        return Err("expected a semantic scope manifest map".into());
    };
    let (_, count) = fields
        .iter_mut()
        .find(|(key, _)| key.as_text() == Some("row_count"))
        .ok_or("semantic scope manifest has no row_count")?;
    *count = ciborium::Value::Integer(99_u64.into());
    let mut tampered = Vec::new();
    ciborium::into_writer(&manifest, &mut tampered)?;
    Ok(std::fs::write(path, tampered)?)
}

fn counter(snapshot: &MetricsSnapshotV1, name: &str) -> Result<u64, Box<dyn Error>> {
    snapshot
        .counters
        .iter()
        .find(|counter| counter.name == name)
        .map(|counter| counter.value)
        .ok_or_else(|| format!("missing scrub counter `{name}`").into())
}

fn gauge(snapshot: &MetricsSnapshotV1, name: &str) -> Result<f64, Box<dyn Error>> {
    snapshot
        .gauges
        .iter()
        .find(|gauge| gauge.name == name)
        .map(|gauge| gauge.value)
        .ok_or_else(|| format!("gauge `{name}` is in the scrape").into())
}

/// Poll the metrics scrape until `done` holds or the wait is spent.
fn wait_for_scrape(
    client: &QuantaIndex,
    what: &str,
    done: impl Fn(&MetricsSnapshotV1) -> Result<bool, Box<dyn Error>>,
) -> Result<MetricsSnapshotV1, Box<dyn Error>> {
    let started = Instant::now();
    loop {
        let snapshot = client.observability().metrics_snapshot()?;
        if done(&snapshot)? {
            return Ok(snapshot);
        }
        if started.elapsed() > SCRUB_WAIT {
            return Err(format!(
                "{what} did not happen within {SCRUB_WAIT:?}: counters {:?} gauges {:?}",
                snapshot.counters, snapshot.gauges
            )
            .into());
        }
        std::thread::sleep(SCRUB_INTERVAL);
    }
}

/// A byte defect in the active generation's rows is quarantined by the scrub.
///
/// It survives boot (the active pair is proven by layout, not by bytes),
/// is found by the scrub, counted, listed as content-corrupt, and refuses
/// the next query typed although a handle was resident before the scrub
/// ran.
#[test]
fn a_byte_defect_survives_the_doors_and_is_quarantined_by_the_scrub() -> TestResult {
    let root = private_tempdir()?;
    let binary = LegacySearchdBinary::from_environment()?;
    let daemon = start_scrub(&binary, root.path())?;
    let client = daemon.connect()?;
    let active = fixture_pin(1)?;
    let _head = publish_file(
        &client,
        &active,
        "src/first.rs",
        "fn first() { needle_first }",
        None,
    )?;

    // A clean generation is scrubbed to completion while the daemon serves,
    // and the completion is visible in the scrape. The scrape's completion
    // gauge spans both tracks, and the scheduler may finish either track's
    // generation first, so the wait is for the semantic pass's own durable
    // receipt — the record the restart below must find.
    let generation_dir = generation_dir(root.path(), "semantic", &active)?;
    let receipt = generation_dir.join(SEMANTIC_SCRUB_RECEIPT);
    let completed = wait_for_scrape(
        &client,
        "the semantic generation's completed scrub",
        |snapshot| {
            let last_completed = gauge(snapshot, "scrub_last_completed_unix")?;
            let corruptions = counter(snapshot, "scrub_corruptions_total")?;
            if corruptions != 0 {
                return Err(
                    format!("the unmodified fixture reported {corruptions} corruptions").into(),
                );
            }
            Ok(receipt.is_file() && last_completed > 0.0)
        },
    )?;
    if counter(&completed, "scrub_runs_total")? == 0
        || counter(&completed, "scrub_bytes_total")? == 0
    {
        return Err(format!(
            "a completed scrub ran steps and read bytes: {:?}",
            completed.counters
        )
        .into());
    }

    // Restart over a same-length byte defect in the rows.
    drop(client);
    daemon.stop()?;
    let Some(payload) = payload_files(&generation_dir)?.into_iter().next() else {
        return Err("the active generation has payload files".into());
    };
    flip_middle_byte(&payload)?;
    let reopened = start_scrub(&binary, root.path())?;
    let client = reopened.connect()?;
    let boot_scrape = client.observability().metrics_snapshot()?;
    if gauge(&boot_scrape, "boot_active_pairs_validated")?.to_bits() != 1.0_f64.to_bits()
        || gauge(&boot_scrape, "boot_semantic_quarantined_generations")?.to_bits()
            != 0.0_f64.to_bits()
        || gauge(&boot_scrape, "boot_semantic_scrub_last_completed_unix")? <= 0.0
    {
        return Err(format!(
            "boot did not prove the active pair and retain its clean scrub receipt: {boot_scrape:?}"
        )
        .into());
    }

    // A query while the scrub is still walking the generation goes through
    // the doors and leaves a resident handle when the library serves the
    // rows past the defect; whether it does is the library's business.
    // The doors themselves never refuse a layout-intact generation: that
    // is proven byte for byte in `sealed_manifest.rs`, not by timing here.
    let query = || {
        client
            .semantic()
            .query()
            .text("needle")
            .pinned(active.clone())
            .top_k(5)
            .execute()
    };
    match query() {
        Ok(_) | Err(SdkError::Remote { .. }) => {}
        Err(error) => return Err(error.into()),
    }

    // The scrub finds the defect.
    let found = wait_for_scrape(&client, "the scrub finding the defect", |snapshot| {
        Ok(counter(snapshot, "scrub_corruptions_total")? >= 1)
    })?;
    if counter(&found, "scrub_corruptions_total")? != 1 {
        return Err(format!("exactly one corruption: {:?}", found.counters).into());
    }
    // Listed live as content-corrupt, naming the generation directory.
    let inventory = client.quarantine().inventory()?;
    let listed: Vec<(String, String)> = inventory
        .semantic
        .iter()
        .map(|entry| (entry.path.clone(), entry.reason.clone()))
        .collect();
    if listed
        != vec![(
            generation_dir.display().to_string(),
            GenerationQuarantineReasonV1::ContentCorrupt
                .as_code_str()
                .to_string(),
        )]
    {
        return Err(
            format!("the quarantine listing names the corrupt generation: {listed:?}").into(),
        );
    }
    if !inventory
        .semantic
        .iter()
        .all(|entry| entry.detail.contains("content digest differs"))
    {
        return Err(format!("the detail names the defect: {:?}", inventory.semantic).into());
    }
    // The next query is refused typed: the resident handle was fenced.
    let after = query();
    if !matches!(&after, Err(SdkError::Remote {
        code: SearchPlaneErrorCodeV2::GenerationQuarantined, message, ..
    }) if message.contains("GENERATION_QUARANTINE_CONTENT_CORRUPT"))
    {
        return Err(
            format!("semantic query did not carry the typed scrub quarantine: {after:?}").into(),
        );
    }
    let lexical = client
        .lexical()
        .query()
        .native("needle_first")
        .pinned(active.clone())
        .top_k(5)
        .execute()?;
    if lexical.generation != active || lexical.results.len() != 1 {
        return Err(format!("lexical peer did not preserve its pinned row: {lexical:?}").into());
    }

    // The durable quarantine also fences a fresh boot.
    drop(client);
    reopened.stop()?;
    let command = configured_command(&binary, root.path(), SCRUB_INTERVAL, 1)?;
    let output = legacy_start_probe::wait_for_refusal(&binary, command)?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    if output.status.code() != Some(70)
        || !stderr.contains("ACTIVATION_TARGET_UNOPENABLE")
        || !stderr.contains("GENERATION_QUARANTINED")
    {
        return Err(
            format!("boot lost the typed activation and quarantine causes: {stderr}").into(),
        );
    }
    Ok(())
}

/// Rewrite the generation's scope manifest as format 9 and re-commit it.
///
/// Format 9 is the last format before the per-segment build record; the
/// sealed manifest re-commits the rewritten file, so the generation is
/// exactly what a format-9 seal left.
fn downgrade_to_format_9(generation_dir: &Path) -> TestResult {
    let manifest_path = generation_dir.join(SEMANTIC_MANIFEST);
    let bytes = std::fs::read(&manifest_path)?;
    let mut value: ciborium::value::Value =
        ciborium::from_reader(&bytes[..]).map_err(|err| format!("decode manifest: {err}"))?;
    let ciborium::value::Value::Map(entries) = &mut value else {
        return Err("scope manifest is not a map".into());
    };
    let mut rewritten = false;
    for (key, field) in entries.iter_mut() {
        if key.as_text() == Some("format_version") {
            *field = ciborium::value::Value::Integer(9.into());
            rewritten = true;
        }
    }
    if !rewritten {
        return Err("manifest carries no format_version".into());
    }
    let mut legacy = Vec::new();
    ciborium::into_writer(&value, &mut legacy).map_err(|err| format!("encode manifest: {err}"))?;
    std::fs::write(&manifest_path, &legacy)?;

    let sealed_path = generation_dir.join(SEALED_MANIFEST);
    let sealed_bytes = std::fs::read(&sealed_path)?;
    let mut sealed: ciborium::value::Value = ciborium::from_reader(&sealed_bytes[..])
        .map_err(|err| format!("decode sealed manifest: {err}"))?;
    let ciborium::value::Value::Array(row) = &mut sealed else {
        return Err("sealed manifest is not an array".into());
    };
    let Some(commitment) = row.get_mut(2) else {
        return Err("sealed manifest has no scope commitment".into());
    };
    let digest: [u8; 32] = Sha256::digest(&legacy).into();
    *commitment = ciborium::value::Value::Array(vec![
        ciborium::value::Value::Integer(u64::try_from(legacy.len())?.into()),
        ciborium::value::Value::Bytes(digest.to_vec()),
    ]);
    let mut resealed = Vec::new();
    ciborium::into_writer(&sealed, &mut resealed)
        .map_err(|err| format!("encode sealed manifest: {err}"))?;
    std::fs::write(&sealed_path, &resealed)?;
    Ok(())
}

/// A retained generation of an unsupported manifest format refuses boot.
///
/// History restoration must not guess its physical accounting or delete either
/// generation before the producer rebuilds the old format explicitly. The
/// semantic inventory's quarantine classification is covered separately.
#[test]
fn a_retained_earlier_format_refuses_boot_without_deleting_history() -> TestResult {
    let root = private_tempdir()?;
    let binary = LegacySearchdBinary::from_environment()?;
    let command = || {
        configured_command(
            &binary,
            root.path(),
            DORMANT_SCRUB,
            IntegrityScrubPolicyV1::DEFAULT.max_bytes_per_step,
        )
    };
    let daemon = SearchdBinaryProcess::start_with_pinned_command(root.path(), command()?)?;
    let client = daemon.connect()?;
    let inactive = fixture_pin(1)?;
    let g1 = publish_file(
        &client,
        &inactive,
        "src/first.rs",
        "fn first() { needle_first }",
        None,
    )?;
    let active = fixture_pin(2)?;
    let _g2 = publish_file(
        &client,
        &active,
        "src/second.rs",
        "fn second() { needle_second }",
        Some(g1),
    )?;
    drop(client);
    daemon.stop()?;

    let inactive_dir = generation_dir(root.path(), "semantic", &inactive)?;
    let active_dir = generation_dir(root.path(), "semantic", &active)?;
    downgrade_to_format_9(&inactive_dir)?;
    let retained_payloads = [payload_files(&inactive_dir)?, payload_files(&active_dir)?];
    if retained_payloads.iter().any(Vec::is_empty) {
        return Err("both retained generations must contain dataset payload files".into());
    }
    let output = legacy_start_probe::require_start_failure(
        &binary,
        root.path(),
        DEFAULT_HISTORY_MAX_GENERATIONS,
    )?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    if output.status.code() != Some(70)
        || !output.stdout.is_empty()
        || !stderr.contains("GENERATION_MANIFEST_FORMAT_UNSUPPORTED")
        || daemon_socket_paths(root.path())
            .iter()
            .any(|path| path.exists())
    {
        return Err(format!(
            "unsupported retained format lost its typed pre-serve refusal: {stderr}"
        )
        .into());
    }
    for (directory, before) in [inactive_dir, active_dir]
        .iter()
        .zip(retained_payloads.iter())
    {
        if payload_files(directory)?.as_slice() != before.as_slice() {
            return Err(format!(
                "boot refusal changed retained payload set {}",
                directory.display()
            )
            .into());
        }
        if !directory.join(SEMANTIC_MANIFEST).is_file()
            || !directory.join(SEALED_MANIFEST).is_file()
        {
            return Err(format!(
                "boot refusal deleted retained generation {}",
                directory.display()
            )
            .into());
        }
    }
    Ok(())
}

fn require_active_corruption_refusal(
    track: &str,
    manifest: &str,
    damage: impl FnOnce(&Path) -> TestResult,
    error_details: &[&str],
) -> TestResult {
    let root = private_tempdir()?;
    let binary = LegacySearchdBinary::from_environment()?;
    let command = || {
        configured_command(
            &binary,
            root.path(),
            DORMANT_SCRUB,
            IntegrityScrubPolicyV1::DEFAULT.max_bytes_per_step,
        )
    };
    let daemon = SearchdBinaryProcess::start_with_pinned_command(root.path(), command()?)?;
    let client = daemon.connect()?;
    let g1 = publish_file(
        &client,
        &fixture_pin(1)?,
        "src/first.rs",
        "fn first() { needle_first }",
        None,
    )?;
    let pin = fixture_pin(2)?;
    let active = publish_file(
        &client,
        &pin,
        "src/second.rs",
        "fn second() { needle_second }",
        Some(g1),
    )?;
    drop(client);
    daemon.stop()?;

    let path = generation_dir(root.path(), track, &pin)?.join(manifest);
    let original = std::fs::read(&path)?;
    damage(&path)?;
    let mutated = std::fs::read(&path)?;
    let output = legacy_start_probe::wait_for_refusal(&binary, command()?)?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    if output.status.code() != Some(70)
        || !output.stdout.is_empty()
        || !stderr.contains("ACTIVATION_TARGET_UNOPENABLE")
        || !stderr.contains("GENERATION_SIDECAR_CORRUPT")
        || stderr.contains("; quarantined as")
        || !error_details.iter().all(|&detail| stderr.contains(detail))
        || daemon_socket_paths(root.path())
            .iter()
            .any(|socket| socket.exists())
        || std::fs::read(&path)? != mutated
    {
        return Err(format!(
            "boot did not preserve the damaged active root and its typed refusal: {stderr}"
        )
        .into());
    }
    std::fs::write(path, original)?;
    let reopened = SearchdBinaryProcess::start_with_pinned_command(root.path(), command()?)?;
    let client = reopened.connect()?;
    let served = client
        .lexical()
        .query()
        .native("needle_second")
        .pinned(pin.clone())
        .top_k(5)
        .execute()?;
    let [row] = served.results.as_slice() else {
        return Err(format!("repaired G2 did not return its one row: {served:?}").into());
    };
    if served.generation != pin
        || row.repo_relative_path.as_str() != "src/second.rs"
        || client
            .generations()
            .active_head(pin.repo_id, pin.revision_id)?
            != Some(active)
    {
        return Err(format!("repaired boot changed the active pair: {served:?}").into());
    }
    let listing = client.quarantine().inventory()?;
    if !listing.lexical.is_empty() || !listing.semantic.is_empty() {
        return Err(format!("failed boot recorded quarantine: {listing:?}").into());
    }
    drop(client);
    reopened.stop()?;
    binary.verify()
}

#[test]
fn damage_to_the_active_generation_refuses_to_boot() -> TestResult {
    require_active_corruption_refusal("lexical", LEXICAL_MANIFEST, flip_last_byte, &[])
}

#[test]
fn damage_to_the_active_semantic_generation_refuses_to_boot_and_records_nothing() -> TestResult {
    require_active_corruption_refusal(
        "semantic",
        SEMANTIC_MANIFEST,
        tamper_semantic_row_count,
        &["track=Semantic", SEMANTIC_MANIFEST],
    )
}

fn dormant_command<'binary>(
    binary: &'binary LegacySearchdBinary,
    root: &Path,
) -> Result<LegacySearchdCommand<'binary>, Box<dyn Error>> {
    configured_command(
        binary,
        root,
        DORMANT_SCRUB,
        IntegrityScrubPolicyV1::DEFAULT.max_bytes_per_step,
    )
}

fn seed_boot_generations(
    binary: &LegacySearchdBinary,
    root: &Path,
    count: usize,
) -> Result<Vec<SearchCorpusActiveHeadV1>, Box<dyn Error>> {
    let daemon =
        SearchdBinaryProcess::start_with_pinned_command(root, dormant_command(binary, root)?)?;
    let client = daemon.connect()?;
    let mut heads = Vec::new();
    for (generation, (path, body)) in (1_u64..).zip(
        [
            ("src/first.rs", "fn first() { needle_first }"),
            ("src/second.rs", "fn second() { needle_second }"),
            ("src/third.rs", "fn third() { needle_third }"),
        ]
        .into_iter()
        .take(count),
    ) {
        let pin = fixture_pin(generation)?;
        let active = publish_file(&client, &pin, path, body, heads.last().cloned())?;
        heads.push(active);
    }
    drop(client);
    daemon.stop()?;
    Ok(heads)
}

fn listed(entries: &[QuarantinedGenerationEntryV1]) -> BTreeSet<(PathBuf, String)> {
    entries
        .iter()
        .map(|entry| (PathBuf::from(&entry.path), entry.reason.clone()))
        .collect()
}

fn native_query(
    client: &QuantaIndex,
    pin: &GenerationPin,
    needle: &str,
) -> Result<TextQueryResponse, SdkError> {
    client
        .lexical()
        .query()
        .native(needle)
        .pinned(pin.clone())
        .top_k(5)
        .execute()
}

fn require_boot_count(snapshot: &MetricsSnapshotV1, name: &str, expected: f64) -> TestResult {
    let value = gauge(snapshot, name)?;
    if value.to_bits() != expected.to_bits() {
        return Err(format!("{name}: expected {expected}, observed {value}").into());
    }
    Ok(())
}

#[test]
fn damage_to_an_inactive_generation_does_not_stop_the_daemon() -> TestResult {
    let root = private_tempdir()?;
    let binary = LegacySearchdBinary::from_environment()?;
    let _heads = seed_boot_generations(&binary, root.path(), 2)?;
    let g1 = fixture_pin(1)?;
    let g2 = fixture_pin(2)?;
    let g9 = fixture_pin(9)?;
    flip_last_byte(&generation_dir(root.path(), "lexical", &g1)?.join(LEXICAL_MANIFEST))?;
    tamper_semantic_row_count(
        &generation_dir(root.path(), "semantic", &g1)?.join(SEMANTIC_MANIFEST),
    )?;
    let old_family = std::fs::canonicalize(root.path())?.join("indexes/lexical/repo-legacy");
    std::fs::create_dir_all(old_family.join("rev-legacy/g1"))?;
    let unreadable = generation_dir(root.path(), "lexical", &g9)?;
    std::fs::create_dir_all(&unreadable)?;
    std::fs::write(
        unreadable.join("search-corpus-generation-identity.cbor"),
        b"\xff\x00not-cbor",
    )?;

    let daemon = SearchdBinaryProcess::start_with_pinned_command(
        root.path(),
        dormant_command(&binary, root.path())?,
    )?;
    let client = daemon.connect()?;
    let snapshot = client.observability().metrics_snapshot()?;
    for (name, count) in [
        ("boot_active_pairs_validated", 1.0),
        ("boot_lexical_sealed_generations", 2.0),
        ("boot_semantic_sealed_generations", 2.0),
        ("boot_lexical_quarantined_generations", 2.0),
        ("boot_semantic_quarantined_generations", 0.0),
    ] {
        require_boot_count(&snapshot, name, count)?;
    }
    let inventory = client.quarantine().inventory()?;
    let expected = BTreeSet::from([
        (
            old_family,
            GenerationQuarantineReasonV1::NonCanonicalLayout
                .as_code_str()
                .into(),
        ),
        (
            unreadable,
            GenerationQuarantineReasonV1::IdentityUnreadable
                .as_code_str()
                .into(),
        ),
    ]);
    if listed(&inventory.lexical) != expected || !inventory.semantic.is_empty() {
        return Err(format!("unexpected boot quarantine: {inventory:?}").into());
    }
    let served = native_query(&client, &g2, "needle_second")?;
    if served.generation != g2 || served.results.len() != 1 {
        return Err(format!("active G2 did not retain its one row: {served:?}").into());
    }
    let lexical = native_query(&client, &g1, "needle_first");
    if !matches!(
        &lexical,
        Err(SdkError::Remote {
            code: SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
            ..
        })
    ) {
        return Err(
            format!("G1 lexical query did not refuse its corrupt sidecar: {lexical:?}").into(),
        );
    }
    let semantic = client
        .semantic()
        .query()
        .text("needle")
        .pinned(g1)
        .top_k(5)
        .execute();
    if !matches!(&semantic, Err(SdkError::Remote { code: SearchPlaneErrorCodeV2::GenerationSidecarCorrupt, message, .. })
        if message.contains(SEMANTIC_MANIFEST))
    {
        return Err(
            format!("G1 semantic query did not refuse its corrupt manifest: {semantic:?}").into(),
        );
    }
    let quarantined = native_query(&client, &g9, "needle");
    if !matches!(
        &quarantined,
        Err(SdkError::Remote {
            code: SearchPlaneErrorCodeV2::NotReady,
            ..
        })
    ) {
        return Err(
            format!("unreadable G9 must stay out of the read view: {quarantined:?}").into(),
        );
    }
    drop(client);
    daemon.stop()?;
    binary.verify()
}

#[test]
fn a_rollback_that_proves_inactive_damage_quarantines_it() -> TestResult {
    let root = private_tempdir()?;
    let binary = LegacySearchdBinary::from_environment()?;
    let heads = seed_boot_generations(&binary, root.path(), 3)?;
    let [first, second, active] = heads.as_slice() else {
        return Err("rollback fixture did not publish three active head observations".into());
    };
    let g1 = fixture_pin(1)?;
    let g2 = fixture_pin(2)?;
    let lexical_first = generation_dir(root.path(), "lexical", &g1)?;
    let semantic_second = generation_dir(root.path(), "semantic", &g2)?;
    let lexical_manifest = lexical_first.join(LEXICAL_MANIFEST);
    let semantic_manifest = semantic_second.join(SEMANTIC_MANIFEST);
    let lexical_bytes = std::fs::read(&lexical_manifest)?;
    let semantic_bytes = std::fs::read(&semantic_manifest)?;
    flip_last_byte(&lexical_manifest)?;
    tamper_semantic_row_count(&semantic_manifest)?;

    let daemon = SearchdBinaryProcess::start_with_pinned_command(
        root.path(),
        dormant_command(&binary, root.path())?,
    )?;
    let client = daemon.connect()?;
    let selected = native_query(&client, &g1, "needle_first");
    if !matches!(
        &selected,
        Err(SdkError::Remote {
            code: SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
            ..
        })
    ) {
        return Err(
            format!("the pinned query did not reach the damaged sidecar: {selected:?}").into(),
        );
    }
    let before = client.quarantine().inventory()?;
    if !before.lexical.is_empty() || !before.semantic.is_empty() {
        return Err(format!("the query wrote a quarantine receipt: {before:?}").into());
    }
    let rollback = |target: &SearchCorpusActiveHeadV1| {
        client
            .generations()
            .rollback(SearchPlaneRollbackSearchCorpusGenerationCasRequest {
                expected_active: active.clone(),
                target: target.generation.clone(),
            })
    };
    let reason = GenerationQuarantineReasonV1::ContentCorrupt.as_code_str();
    for (target, damaged) in [(first, &lexical_first), (second, &semantic_second)] {
        let recorded = format!("; quarantined as {reason} at {}", damaged.display());
        let refused = rollback(target);
        if !matches!(&refused, Err(SdkError::Remote { code: SearchPlaneErrorCodeV2::RollbackTargetUnopenable, message, .. })
            if message.contains("GENERATION_SIDECAR_CORRUPT") && message.contains(&recorded))
        {
            return Err(format!("rollback did not record {recorded}: {refused:?}").into());
        }
    }
    let expected_lexical = BTreeSet::from([(lexical_first.clone(), reason.to_string())]);
    let expected_semantic = BTreeSet::from([(semantic_second.clone(), reason.to_string())]);
    let after = client.quarantine().inventory()?;
    if listed(&after.lexical) != expected_lexical || listed(&after.semantic) != expected_semantic {
        return Err(format!(
            "quarantine named a different set than the failed rollback: {after:?}"
        )
        .into());
    }
    std::fs::write(lexical_manifest, lexical_bytes)?;
    std::fs::write(semantic_manifest, semantic_bytes)?;
    for target in [first, second] {
        let refused = rollback(target);
        if !matches!(&refused, Err(SdkError::Remote { code: SearchPlaneErrorCodeV2::RollbackTargetUnopenable, message, .. })
            if message.contains("GENERATION_QUARANTINED") && !message.contains("; quarantined as"))
        {
            return Err(
                format!("repairing bytes lifted or recreated quarantine: {refused:?}").into(),
            );
        }
    }
    let fenced = native_query(&client, &g1, "needle_first");
    if !matches!(
        &fenced,
        Err(SdkError::Remote {
            code: SearchPlaneErrorCodeV2::GenerationQuarantined,
            ..
        })
    ) || client
        .generations()
        .active_head(g1.repo_id.clone(), g1.revision_id.clone())?
        .as_ref()
        != Some(active)
    {
        return Err(
            format!("failed rollback changed a generation's read authority: {fenced:?}").into(),
        );
    }
    drop(client);
    daemon.stop()?;

    let reopened = SearchdBinaryProcess::start_with_pinned_command(
        root.path(),
        dormant_command(&binary, root.path())?,
    )?;
    let client = reopened.connect()?;
    let snapshot = client.observability().metrics_snapshot()?;
    require_boot_count(&snapshot, "boot_active_pairs_validated", 1.0)?;
    require_boot_count(&snapshot, "boot_lexical_quarantined_generations", 1.0)?;
    require_boot_count(&snapshot, "boot_semantic_quarantined_generations", 1.0)?;
    let inventory = client.quarantine().inventory()?;
    if listed(&inventory.lexical) != expected_lexical
        || listed(&inventory.semantic) != expected_semantic
        || client
            .generations()
            .active_head(g1.repo_id, g1.revision_id)?
            .as_ref()
            != Some(active)
    {
        return Err(format!(
            "reopen did not preserve the active pair and both quarantine receipts: {inventory:?}"
        )
        .into());
    }
    for entry in inventory.lexical.into_iter().chain(inventory.semantic) {
        let ack = client
            .quarantine()
            .discard(&QuarantineTargetV1::Generation(entry.clone()))?;
        if !matches!(ack.outcome, QuarantineDiscardOutcomeDtoV1::Discarded { .. }) {
            return Err(format!(
                "discard of {} did not retire the generation: {ack:?}",
                entry.path
            )
            .into());
        }
    }
    let emptied = client.quarantine().inventory()?;
    if !emptied.lexical.is_empty()
        || !emptied.semantic.is_empty()
        || lexical_first.try_exists()?
        || semantic_second.try_exists()?
    {
        return Err(format!("both corrupt generations should be gone: {emptied:?}").into());
    }
    drop(client);
    reopened.stop()?;
    binary.verify()
}

/// The quarantine control surface (QI-BB-026 follow-up) lists what boot
/// set aside and discards it only as listed.
///
/// What boot set aside is listed live over the control socket with paths
/// and reasons, each entry is discarded exactly as listed, a stale or
/// invented entry is refused typed and removes nothing, and after every
/// discard the disk and a fresh boot agree that nothing is quarantined.
#[test]
fn quarantine_is_listed_discarded_as_named_and_gone_after_a_reboot() -> TestResult {
    let root = private_tempdir()?;
    let binary = LegacySearchdBinary::from_environment()?;
    let _heads = seed_boot_generations(&binary, root.path(), 2)?;
    let active = fixture_pin(2)?;
    let state_root = std::fs::canonicalize(root.path())?;
    let lexical_legacy = state_root.join("indexes/lexical").join("repo-legacy");
    std::fs::create_dir_all(lexical_legacy.join("rev-legacy/g1"))?;
    std::fs::write(
        lexical_legacy.join("rev-legacy/g1/leftover.bin"),
        [7_u8; 64],
    )?;
    let lexical_garbage = generation_dir(root.path(), "lexical", &fixture_pin(9)?)?;
    std::fs::create_dir_all(&lexical_garbage)?;
    std::fs::write(
        lexical_garbage.join("search-corpus-generation-identity.cbor"),
        b"\xff\x00not-cbor",
    )?;
    let semantic_legacy = state_root.join("indexes/semantic").join("repo-legacy");
    std::fs::create_dir_all(semantic_legacy.join("rev-legacy/g1"))?;
    // Seed a real catalog-backed candidate, then corrupt its sealed object.
    // Legacy V1 snapshot roots are intentionally left for the P10 importer.
    let repo_map_root = state_root.join("repo-map");
    let catalog = Arc::new(SqliteCatalog::open(&state_root, Duration::from_secs(5))?);
    let store = RepoMapGenerationStore::open(&repo_map_root, catalog)?.store;
    let bundle = RepoMapSourceBundle::new(
        active.repo_id.clone(),
        active.revision_id.clone(),
        ManifestGeneration::new(1),
        "ab".repeat(32),
        "snap-quarantine-e2e".to_string(),
        1,
        "d".repeat(64),
        RepoMapGraphCoverage {
            item_index_availability: RepoMapItemIndexAvailability::Available,
            graph_coverage_class: RepoMapGraphCoverageClass::Complete,
        },
        RepoMapExactnessSummary::Exact,
        RepoMapRedactionState::Unredacted,
    )
    .with_node(RepoMapNode::File(RepoMapFileNode {
        file_id: FileId::new("file://src/quarantine.rs"),
        repo_relative_path: RepoRelativePath::new("src/quarantine.rs"),
        line_count: 1,
    }));
    let _receipt = store.ingest_bundle_v2(&RepoMapPublishBundleRequestV2::new(bundle)?)?;
    let object_root = repo_map_root.join("objects/sha256");
    let mut objects = Vec::new();
    for first in std::fs::read_dir(&object_root)? {
        for second in std::fs::read_dir(first?.path())? {
            for object in std::fs::read_dir(second?.path())? {
                objects.push(object?.path());
            }
        }
    }
    let [object_path] = objects.as_slice() else {
        return Err(format!("expected one sealed repo-map object, found {objects:?}").into());
    };
    std::fs::write(object_path, b"not json at all")?;
    drop(store);

    let daemon = SearchdBinaryProcess::start_with_pinned_command(
        root.path(),
        dormant_command(&binary, root.path())?,
    )?;
    let client = daemon.connect()?;

    // Listed live, per authority, with reasons.
    let inventory = client.quarantine().inventory()?;
    let mut lexical: Vec<(String, String)> = inventory
        .lexical
        .iter()
        .map(|entry| (entry.path.clone(), entry.reason.clone()))
        .collect();
    lexical.sort();
    let mut expected = vec![
        (
            lexical_legacy.display().to_string(),
            "GENERATION_QUARANTINE_NON_CANONICAL_LAYOUT".to_string(),
        ),
        (
            lexical_garbage.display().to_string(),
            "GENERATION_QUARANTINE_IDENTITY_UNREADABLE".to_string(),
        ),
    ];
    expected.sort();
    if lexical != expected {
        return Err(format!("lexical quarantine listing drifted: {lexical:?}").into());
    }
    let semantic: Vec<(String, String)> = inventory
        .semantic
        .iter()
        .map(|entry| (entry.path.clone(), entry.reason.clone()))
        .collect();
    if semantic
        != vec![(
            semantic_legacy.display().to_string(),
            "GENERATION_QUARANTINE_NON_CANONICAL_LAYOUT".to_string(),
        )]
    {
        return Err(format!("semantic quarantine listing drifted: {semantic:?}").into());
    }
    let repo_map: Vec<&str> = inventory
        .repo_map
        .iter()
        .map(|entry| entry.file_name.as_str())
        .collect();
    if repo_map.len() != 1
        || repo_map
            .first()
            .is_none_or(|name| !name.starts_with("incident-"))
        || repo_map.first().is_none_or(|name| {
            std::path::Path::new(name).extension() != Some(std::ffi::OsStr::new("cbor"))
        })
        || object_path.exists()
    {
        return Err(format!("repo-map quarantine listing drifted: {repo_map:?}").into());
    }
    if inventory
        .repo_map
        .first()
        .is_some_and(|entry| entry.reason.is_empty())
    {
        return Err("the repo-map entry carries the reason it was set aside".into());
    }

    // A stale entry (wrong reason for a listed path) removes nothing.
    let Some(listed_garbage) = inventory
        .lexical
        .iter()
        .find(|entry| entry.path == lexical_garbage.display().to_string())
        .cloned()
    else {
        return Err("the garbage generation is listed".into());
    };
    let mut stale = listed_garbage.clone();
    stale.reason = "GENERATION_QUARANTINE_SCOPE_MISMATCH".to_string();
    match client
        .quarantine()
        .discard(&QuarantineTargetV1::Generation(stale))
    {
        Err(SdkError::Remote {
            code: SearchPlaneErrorCodeV2::QuarantineTargetNotQuarantined,
            ..
        }) => {}
        other => return Err(format!("a stale entry must be refused typed: {other:?}").into()),
    }
    if !lexical_garbage.is_dir() {
        return Err("a refused discard removes nothing".into());
    }
    // A sealed generation named as if quarantined removes nothing.
    let sealed = QuarantinedGenerationEntryV1 {
        track: SearchPlaneTrackKind::Lexical,
        path: generation_dir(root.path(), "lexical", &active)?
            .display()
            .to_string(),
        reason: "GENERATION_QUARANTINE_IDENTITY_UNREADABLE".to_string(),
        detail: String::new(),
    };
    match client
        .quarantine()
        .discard(&QuarantineTargetV1::Generation(sealed))
    {
        Err(SdkError::Remote {
            code: SearchPlaneErrorCodeV2::QuarantineTargetNotQuarantined,
            ..
        }) => {}
        other => {
            return Err(
                format!("a sealed generation must never be discarded here: {other:?}").into(),
            );
        }
    }
    if !generation_dir(root.path(), "lexical", &active)?.is_dir() {
        return Err("the active generation is untouched".into());
    }

    // Every listed entry discards exactly as listed, with its bytes.
    for entry in inventory.lexical.iter().chain(inventory.semantic.iter()) {
        let ack = client
            .quarantine()
            .discard(&QuarantineTargetV1::Generation(entry.clone()))
            .map_err(|error| format!("discard {}: {error:?}", entry.path))?;
        match ack.outcome {
            QuarantineDiscardOutcomeDtoV1::Discarded { .. } => {}
            QuarantineDiscardOutcomeDtoV1::Absent => {
                return Err(
                    format!("{} was listed, so it was there to discard", entry.path).into(),
                );
            }
        }
        if ack.target != QuarantineTargetV1::Generation(entry.clone()) {
            return Err("the ack names the target as it was sent".into());
        }
        if Path::new(&entry.path).exists() {
            return Err(format!("{} is gone after its discard", entry.path).into());
        }
    }
    let Some(repo_map_entry) = inventory.repo_map.first().cloned() else {
        return Err("the repo-map entry is listed".into());
    };
    let mut stale_file = repo_map_entry.clone();
    stale_file.reason = "some other reason".to_string();
    match client
        .quarantine()
        .discard(&QuarantineTargetV1::RepoMapFile(stale_file))
    {
        Err(SdkError::Remote {
            code: SearchPlaneErrorCodeV2::QuarantineTargetNotQuarantined,
            ..
        }) => {}
        other => return Err(format!("a stale repo-map reason is refused typed: {other:?}").into()),
    }
    if client.quarantine().inventory()?.repo_map != inventory.repo_map {
        return Err("a refused repo-map discard removes nothing".into());
    }
    let ack = client
        .quarantine()
        .discard(&QuarantineTargetV1::RepoMapFile(repo_map_entry.clone()))
        .map_err(|error| format!("discard the repo-map file: {error:?}"))?;
    if ack.outcome != (QuarantineDiscardOutcomeDtoV1::Discarded { bytes: 15 }) {
        return Err(format!(
            "the repo-map file's 15 bytes are reported: {:?}",
            ack.outcome
        )
        .into());
    }
    if !client.quarantine().inventory()?.repo_map.is_empty() {
        return Err("the repo-map file is gone after its discard".into());
    }
    // Discarding again is idempotent: nothing is there.
    let again = client
        .quarantine()
        .discard(&QuarantineTargetV1::RepoMapFile(repo_map_entry))
        .map_err(|error| format!("a second repo-map discard: {error:?}"))?;
    if again.outcome != QuarantineDiscardOutcomeDtoV1::Absent {
        return Err(format!("a second discard is Absent, got {:?}", again.outcome).into());
    }
    let again = client
        .quarantine()
        .discard(&QuarantineTargetV1::Generation(listed_garbage))
        .map_err(|error| format!("a second generation discard: {error:?}"))?;
    if again.outcome != QuarantineDiscardOutcomeDtoV1::Absent {
        return Err(format!("a second discard is Absent, got {:?}", again.outcome).into());
    }

    // The live listing and a fresh boot agree: nothing is quarantined.
    let after = client.quarantine().inventory()?;
    if !(after.lexical.is_empty() && after.semantic.is_empty() && after.repo_map.is_empty()) {
        return Err(format!("nothing is quarantined after the discards: {after:?}").into());
    }
    drop(client);
    daemon.stop()?;
    let reopened = SearchdBinaryProcess::start_with_pinned_command(
        root.path(),
        dormant_command(&binary, root.path())?,
    )?;
    let client = reopened.connect()?;
    let snapshot = client.observability().metrics_snapshot()?;
    for (name, count) in [
        ("boot_lexical_quarantined_generations", 0.0),
        ("boot_semantic_quarantined_generations", 0.0),
        ("boot_repomap_quarantined_files", 0.0),
        ("boot_lexical_sealed_generations", 2.0),
        ("boot_semantic_sealed_generations", 2.0),
    ] {
        require_boot_count(&snapshot, name, count)?;
    }
    let after_reopen = client.quarantine().inventory()?;
    if !after_reopen.lexical.is_empty()
        || !after_reopen.semantic.is_empty()
        || !after_reopen.repo_map.is_empty()
    {
        return Err(format!("discarded entries reappeared after reboot: {after_reopen:?}").into());
    }
    drop(client);
    reopened.stop()?;
    binary.verify()
}
