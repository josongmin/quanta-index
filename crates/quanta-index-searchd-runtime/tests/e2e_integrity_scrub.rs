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

use std::error::Error;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use quanta_index_contract::{
    GenerationPin, ManifestGeneration, MetricsSnapshotV1, SearchPlaneControlIpcResponse,
    SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcResponse, SemanticQueryRequest, TextQuerySyntax,
};
use quanta_index_core::{
    GenerationQuarantineReasonV1, GenerationStorageKeyV1, IntegrityScrubPolicyV1,
};
use quanta_index_searchd_harness as e2e_harness;
use sha2::{Digest as _, Sha256};

use e2e_harness::E2eRuntime;

type TestResult = Result<(), Box<dyn Error>>;

const SEMANTIC_MANIFEST: &str = "semantic-manifest.cbor";
const SEALED_MANIFEST: &str = "semantic-sealed-manifest.cbor";
/// The durable record of a completed semantic scrub pass.
const SEMANTIC_SCRUB_RECEIPT: &str = "semantic-scrub-receipt.cbor";
/// How long the daemon is given to run enough scrub steps; the scrub is
/// paced at [`SCRUB_INTERVAL`] and every wait polls an observable.
const SCRUB_WAIT: Duration = Duration::from_secs(120);
const SCRUB_INTERVAL: Duration = Duration::from_millis(100);

/// One file per step: the smallest budget, so a pass over a generation
/// takes as many steps as it has files and the cursor is exercised.
fn scrub_policy() -> Result<IntegrityScrubPolicyV1, Box<dyn Error>> {
    Ok(IntegrityScrubPolicyV1::new(
        u64::try_from(SCRUB_INTERVAL.as_millis())?,
        1,
    )?)
}

fn semantic_generation_dir(
    rt: &E2eRuntime,
    generation: ManifestGeneration,
) -> Result<PathBuf, Box<dyn Error>> {
    let canonical = std::fs::canonicalize(rt.state_root())?;
    Ok(canonical
        .join("indexes/semantic")
        .join(GenerationStorageKeyV1::for_repo_revision(&rt.repo(), &rt.revision()).as_str())
        .join(format!("g{}", generation.get())))
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

fn semantic_query(pin: Option<GenerationPin>) -> SearchPlaneQueryIpcRequest {
    SearchPlaneQueryIpcRequest::Semantic(SemanticQueryRequest {
        query_text: "needle".to_string(),
        constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
        generation: pin,
        generation_selector: None,
        lexical_scope: None,
        top_k: 5,
    })
}

fn typed_code(response: &SearchPlaneQueryIpcResponse) -> Option<(&'static str, String)> {
    match response {
        SearchPlaneQueryIpcResponse::Error(error) => {
            Some((error.code.as_wire_str(), error.message.clone()))
        }
        SearchPlaneQueryIpcResponse::Text(_)
        | SearchPlaneQueryIpcResponse::Symbol(_)
        | SearchPlaneQueryIpcResponse::Semantic(_)
        | SearchPlaneQueryIpcResponse::SemanticWorkBoundedV1(_)
        | SearchPlaneQueryIpcResponse::Hybrid(_)
        | SearchPlaneQueryIpcResponse::HybridSeed(_)
        | SearchPlaneQueryIpcResponse::History(_)
        | SearchPlaneQueryIpcResponse::Structural(_)
        | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
        | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
        | SearchPlaneQueryIpcResponse::ActiveGenerationSnapshot(_)
        | SearchPlaneQueryIpcResponse::ResolvedLexicalGeneration(_) => None,
    }
}

fn counter(snapshot: &MetricsSnapshotV1, name: &str) -> u64 {
    snapshot
        .counters
        .iter()
        .find(|counter| counter.name == name)
        .map_or(0, |counter| counter.value)
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
    rt: &mut E2eRuntime,
    what: &str,
    done: impl Fn(&MetricsSnapshotV1) -> bool,
) -> Result<MetricsSnapshotV1, Box<dyn Error>> {
    let started = Instant::now();
    loop {
        let snapshot = rt.metrics_snapshot()?;
        if done(&snapshot) {
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
    let mut rt = E2eRuntime::boot_with_integrity_scrub_policy(scrub_policy()?)?;
    rt.ingest_text("repo", "src/first.rs", "fn first() { needle_first }")?;
    let active = rt.seal()?;
    rt.activate_last_sealed_generation()?;

    // A clean generation is scrubbed to completion while the daemon serves,
    // and the completion is visible in the scrape. The scrape's completion
    // gauge spans both tracks, and the scheduler may finish either track's
    // generation first, so the wait is for the semantic pass's own durable
    // receipt — the record the restart below must find.
    let receipt = semantic_generation_dir(&rt, active)?.join(SEMANTIC_SCRUB_RECEIPT);
    let completed = wait_for_scrape(
        &mut rt,
        "the semantic generation's completed scrub",
        |snapshot| {
            receipt.is_file()
                && gauge(snapshot, "scrub_last_completed_unix").is_ok_and(|unix| unix > 0.0)
                && counter(snapshot, "scrub_corruptions_total") == 0
        },
    )?;
    if counter(&completed, "scrub_runs_total") == 0 || counter(&completed, "scrub_bytes_total") == 0
    {
        return Err(format!(
            "a completed scrub ran steps and read bytes: {:?}",
            completed.counters
        )
        .into());
    }

    // Restart over a same-length byte defect in the rows.
    let mut rt = rt.reopen();
    let generation_dir = semantic_generation_dir(&rt, active)?;
    let Some(payload) = payload_files(&generation_dir)?.into_iter().next() else {
        return Err("the active generation has payload files".into());
    };
    flip_middle_byte(&payload)?;
    rt.start()?;
    let report = rt
        .boot_inventory()
        .ok_or("the running daemon exposes its boot inventory")?
        .clone();
    if report.active_pairs_validated != 1 || !report.semantic.quarantined.is_empty() {
        return Err(format!(
            "boot proves the active pair by layout and quarantines nothing yet: {report:?}"
        )
        .into());
    }
    let Some(scrub) = report.semantic.scrub else {
        return Err("the semantic track reports its scrub receipts at boot".into());
    };
    if scrub.last_completed_unix.is_none() {
        return Err(format!("the completed scrub's receipt survives a restart: {scrub:?}").into());
    }
    let boot_scrape = rt.metrics_snapshot()?;
    if gauge(&boot_scrape, "boot_semantic_scrub_last_completed_unix")? <= 0.0 {
        return Err("the boot scrape names the last completed scrub".into());
    }

    // A query while the scrub is still walking the generation goes through
    // the doors and leaves a resident handle when the library serves the
    // rows past the defect; whether it does is the library's business.
    // The doors themselves never refuse a layout-intact generation: that
    // is proven byte for byte in `sealed_manifest.rs`, not by timing here.
    let _before = rt.query_once(semantic_query)?;

    // The scrub finds the defect.
    let found = wait_for_scrape(&mut rt, "the scrub finding the defect", |snapshot| {
        counter(snapshot, "scrub_corruptions_total") >= 1
    })?;
    if counter(&found, "scrub_corruptions_total") != 1 {
        return Err(format!("exactly one corruption: {:?}", found.counters).into());
    }
    // Listed live as content-corrupt, naming the generation directory.
    let inventory = rt.quarantine_inventory()?;
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
    let after = rt.query_once(semantic_query)?;
    match typed_code(&after) {
        Some(("GENERATION_QUARANTINED", message)) => {
            if !message.contains("GENERATION_QUARANTINE_CONTENT_CORRUPT") {
                return Err(format!("the refusal names the quarantine reason: {message}").into());
            }
        }
        other => {
            return Err(format!(
                "the next semantic query must be refused typed after the scrub, got {other:?}"
            )
            .into());
        }
    }
    // The lexical half of the pair still serves; only the corrupt track is
    // refused.
    let lexical = rt.query_text(TextQuerySyntax::Native, "needle_first", 5);
    if let Some(error) = lexical.typed_error {
        return Err(format!("the lexical track is untouched: {error}").into());
    }
    // A restart keeps the quarantine: the receipt is durable, the boot
    // inventory sets the generation aside, and — being the active
    // semantic generation — boot refuses to serve on it.
    let mut rt = rt.reopen();
    match rt.start() {
        Ok(()) => Err("boot must not prove a quarantined active generation".into()),
        Err(error) => {
            let rendered = format!("{error:#}");
            if !rendered.contains("ACTIVATION_TARGET_UNOPENABLE")
                || !rendered.contains("GENERATION_QUARANTINED")
            {
                return Err(format!(
                    "boot refuses with the typed activation cause and the quarantine code: {rendered}"
                )
                .into());
            }
            Ok(())
        }
    }
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

/// An inactive generation of an earlier manifest format is quarantined at boot.
///
/// It is set aside under the format reason with the rebuild instruction,
/// listed live, and absent from readiness; the daemon serves the active
/// generation beside it.
#[test]
fn a_generation_of_an_earlier_format_is_quarantined_at_boot_with_the_format_reason() -> TestResult {
    let mut rt = E2eRuntime::boot()?;
    rt.ingest_text("repo", "src/first.rs", "fn first() { needle_first }")?;
    let inactive = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    rt.ingest_text("repo", "src/second.rs", "fn second() { needle_second }")?;
    let _active = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    let mut rt = rt.reopen();

    let generation_dir = semantic_generation_dir(&rt, inactive)?;
    downgrade_to_format_9(&generation_dir)?;
    rt.start()?;

    let report = rt
        .boot_inventory()
        .ok_or("the running daemon exposes its boot inventory")?
        .clone();
    let quarantined: Vec<(PathBuf, GenerationQuarantineReasonV1, String)> = report
        .semantic
        .quarantined
        .iter()
        .map(|entry| (entry.path.clone(), entry.reason, entry.detail.clone()))
        .collect();
    match quarantined.as_slice() {
        [(path, GenerationQuarantineReasonV1::FormatUnsupported, detail)]
            if *path == generation_dir
                && detail.contains("format version 9")
                && detail.contains("rebuild") => {}
        other => {
            return Err(format!(
                "boot quarantines the format-9 generation with the format reason and the remedy: {other:?}"
            )
            .into());
        }
    }
    if report.semantic.sealed_generations != 1 || report.active_pairs_validated != 1 {
        return Err(
            format!("only the current-format generation is seeded and proven: {report:?}").into(),
        );
    }
    let listed: Vec<(String, String)> = rt
        .quarantine_inventory()?
        .semantic
        .iter()
        .map(|entry| (entry.path.clone(), entry.reason.clone()))
        .collect();
    if listed
        != vec![(
            generation_dir.display().to_string(),
            "GENERATION_QUARANTINE_FORMAT_UNSUPPORTED".to_string(),
        )]
    {
        return Err(format!("the quarantine listing names the format: {listed:?}").into());
    }
    // Absent from readiness: a query pinned to it meets the semantic
    // route's readiness refusal, never the generation's content.
    let pin = GenerationPin::new(rt.repo(), rt.revision(), inactive);
    let pinned = rt.query_once(|_| semantic_query(Some(pin)))?;
    match typed_code(&pinned) {
        Some(("SEMANTIC_GENERATION_NOT_MATERIALIZED", _)) => {}
        other => {
            return Err(format!(
                "a query pinned to a quarantined generation is refused as not materialized, got {other:?}"
            )
            .into());
        }
    }
    // The active generation serves.
    let served = rt.query_text(TextQuerySyntax::Native, "needle_second", 5);
    if let Some(error) = served.typed_error {
        return Err(format!("the active generation serves: {error}").into());
    }
    Ok(())
}

/// Activation names the semantic content roots the plane sealed (QI-BB-028).
///
/// A candidate that carries the sealed receipt's roots is
/// activated; the same tracks under other roots — what an independent
/// build of the same source manifest in another state root would carry —
/// are refused typed as `SEMANTIC_ROW_ROOT_MISMATCH` before any durable
/// activation mutation, and the status report exposes the roots the
/// active pair was activated under.
#[test]
fn activation_refuses_semantic_content_roots_the_generation_did_not_seal() -> TestResult {
    let mut rt = E2eRuntime::boot()?;
    rt.ingest_text("repo", "src/first.rs", "fn first() { needle_first }")?;
    let _sealed = rt.seal()?;
    let attested = rt
        .last_sealed_search_corpus_identity()
        .ok_or("the harness keeps the sealed receipt's identity")?;
    if !attested.semantic_content.is_canonical_v1() {
        return Err(format!("the receipt attests canonical roots: {attested:?}").into());
    }

    // The same tracks, other roots: refused at the activation door.
    let mut foreign = attested.clone();
    foreign.semantic_content = quanta_index_contract::SemanticContentRootsV1 {
        row_root_digest: format!("sha256:{:0>64x}", 0xdead_beef_u64),
        membership_root_digest: attested.semantic_content.membership_root_digest.clone(),
    };
    let refused = rt.activate_search_corpus_cas_raw(
        quanta_index_contract::SearchPlaneActivateSearchCorpusGenerationCasRequest {
            candidate: foreign,
            expected_active: None,
        },
    )?;
    let SearchPlaneControlIpcResponse::Error(error) = &refused else {
        return Err(format!(
            "foreign roots must be refused typed as SEMANTIC_ROW_ROOT_MISMATCH, got {refused:?}"
        )
        .into());
    };
    if error.code.as_wire_str() != "SEMANTIC_ROW_ROOT_MISMATCH" {
        return Err(format!(
            "foreign roots must be refused typed as SEMANTIC_ROW_ROOT_MISMATCH, got {}: {}",
            error.code, error.message
        )
        .into());
    }
    if !error
        .message
        .contains(&attested.semantic_content.row_root_digest)
    {
        return Err(format!("the refusal names the sealed root: {}", error.message).into());
    }
    // Nothing was activated by the refused request: the pair has no
    // active root, so the status report names no track and no roots.
    let status = rt.generation_status()?;
    if !status.tracks.is_empty() || status.semantic_content.is_some() {
        return Err(format!("a refused activation must not activate: {status:?}").into());
    }

    // The attested roots activate, and the status report names them.
    rt.activate_last_sealed_generation()?;
    let served = rt.query_text(TextQuerySyntax::Native, "needle_first", 5);
    if let Some(error) = served.typed_error {
        return Err(format!("the attested candidate activates and serves: {error}").into());
    }
    let status = rt.generation_status()?;
    if status.semantic_content.as_ref() != Some(&attested.semantic_content) {
        return Err(format!(
            "the status report names the roots the pair was activated under: {status:?}"
        )
        .into());
    }
    Ok(())
}
