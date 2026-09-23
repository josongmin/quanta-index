//! QI-BB-026 — a damaged generation nobody serves does not stop the daemon;
//! a damaged generation everybody serves stops it before any socket binds.
//!
//! Before this, boot deep-validated every sealed generation on disk and
//! turned the first defect anywhere into a failed start. Now boot reads
//! identities, quarantines directories it cannot trust, seeds everything
//! else, and proves exactly the active pair. The oracles are external: the
//! daemon starts or refuses to; the active generation serves; a query
//! pinned to a content-corrupted inactive generation is refused at its own
//! door with the same typed code the validator would give; a query pinned
//! to a quarantined generation is `NOT_READY`; and the boot report on the
//! harness names what was set aside.
//!
//! The follow-up adds the operator's side: what boot set aside is listed
//! live over the control socket and discarded exactly as listed, never a
//! sealed generation and never a stale listing.
//!
//! Wave B closes the rest. A query door is a read and records nothing; a
//! rollback that proves inactive damage has the owning adapter re-prove it
//! and leave a content-corrupt receipt, listed and discarded like any
//! other; damage to the active semantic generation refuses to boot
//! without recording anything; and a generation sealed on one track only
//! is named by the boot report (QI-BB-029).

#![forbid(unsafe_code)]

use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use quanta_index_catalog::SqliteCatalog;
use quanta_index_contract::{
    BatchIngestMode, FileId, GenerationPin, ManifestGeneration, QuarantineDiscardOutcomeDtoV1,
    QuarantineTargetV1, QuarantinedGenerationEntryV1, RepoMapExactnessSummary, RepoMapFileNode,
    RepoMapGraphCoverage, RepoMapGraphCoverageClass, RepoMapItemIndexAvailability, RepoMapNode,
    RepoMapRedactionState, RepoMapSourceBundle, RepoRelativePath, SearchCorpusGenerationIdentityV1,
    SearchCorpusIngestBatch, SearchPlaneControlIpcResponse, SearchPlaneQueryIpcRequest,
    SearchPlaneQueryIpcResponse, SearchPlaneRollbackSearchCorpusGenerationCasRequest,
    SearchPlaneTrackKind, SemanticQueryRequest, TextQueryRequest, TextQuerySyntax,
};
use quanta_index_core::{
    GenerationQuarantineReasonV1, GenerationStorageKeyV1, QuarantinedGenerationV1,
    SearchCorpusBatchBuildPort,
};
use quanta_index_lexical::LexicalAdapter;
use quanta_index_repomap::RepoMapGenerationStore;
use quanta_index_searchd::app::{HalfSealedPair, SealedGenerationKey};
use quanta_index_searchd_harness as e2e_harness;

use e2e_harness::E2eRuntime;

type TestResult = Result<(), Box<dyn Error>>;

/// The lexical text-authority manifest inside a generation directory: one
/// of the files the seal commits to.
const LEXICAL_TEXT_AUTHORITY_MANIFEST: &str = "text-authority/manifest.cbor";
const LEXICAL_IDENTITY: &str = "search-corpus-generation-identity.cbor";
const SEMANTIC_MANIFEST: &str = "semantic-manifest.cbor";

fn pair_dir(rt: &E2eRuntime, track_root: &str) -> Result<PathBuf, Box<dyn Error>> {
    let canonical = std::fs::canonicalize(rt.state_root())?;
    Ok(canonical
        .join(track_root)
        .join(GenerationStorageKeyV1::for_repo_revision(&rt.repo(), &rt.revision()).as_str()))
}

fn generation_dir(
    rt: &E2eRuntime,
    track_root: &str,
    generation: ManifestGeneration,
) -> Result<PathBuf, Box<dyn Error>> {
    Ok(pair_dir(rt, track_root)?.join(format!("g{}", generation.get())))
}

fn flip_last_byte(path: &Path) -> TestResult {
    let mut bytes = std::fs::read(path)?;
    let last = bytes.len().checked_sub(1).ok_or("empty file")?;
    let byte = bytes.get_mut(last).ok_or("index")?;
    *byte ^= 0xff;
    std::fs::write(path, bytes)?;
    Ok(())
}

/// Rewrite the scope manifest's `row_count` in place.
///
/// The marker still matches the manifest digest (the inventory is
/// satisfied) but the scope manifest no longer matches the sealed manifest's
/// commitment (every open refuses).
fn tamper_semantic_row_count(manifest_path: &Path) -> TestResult {
    let bytes = std::fs::read(manifest_path)?;
    let mut value: ciborium::value::Value =
        ciborium::from_reader(bytes.as_slice()).map_err(|err| format!("decode manifest: {err}"))?;
    let mut bumped = false;
    if let ciborium::value::Value::Map(entries) = &mut value {
        for (key, val) in entries.iter_mut() {
            if key.as_text() == Some("row_count") {
                *val = ciborium::value::Value::Integer(ciborium::value::Integer::from(99_u64));
                bumped = true;
                break;
            }
        }
    }
    if !bumped {
        return Err("manifest has no row_count field".into());
    }
    let mut tampered = Vec::new();
    ciborium::into_writer(&value, &mut tampered)
        .map_err(|err| format!("encode manifest: {err}"))?;
    std::fs::write(manifest_path, tampered)?;
    Ok(())
}

/// Two sealed and activated generations; the first becomes the inactive
/// predecessor the tests damage.
fn seal_two_generations(
    rt: &mut E2eRuntime,
) -> Result<(ManifestGeneration, ManifestGeneration), Box<dyn Error>> {
    rt.ingest_text("repo", "src/first.rs", "fn first() { needle_first }")?;
    let first = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    rt.ingest_text("repo", "src/second.rs", "fn second() { needle_second }")?;
    let second = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    Ok((first, second))
}

fn pinned_text(pin: GenerationPin, needle: &str) -> SearchPlaneQueryIpcRequest {
    SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
        syntax: TextQuerySyntax::Native,
        query_text: needle.to_string(),
        constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
        generation: Some(pin),
        generation_selector: None,
        top_k: 5,
        cursor: None,
    })
}

fn pinned_semantic(pin: GenerationPin) -> SearchPlaneQueryIpcRequest {
    SearchPlaneQueryIpcRequest::Semantic(SemanticQueryRequest {
        query_text: "needle".to_string(),
        constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
        generation: Some(pin),
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

/// Inactive damage on both tracks does not stop the daemon.
///
/// With two directories the inventory cannot trust beside it, the daemon
/// restarts, proves exactly one active pair, serves the active generation,
/// quarantines the two directories with their reasons, and refuses the
/// damaged inactive generation only when a query pins it — at that door,
/// typed.
#[test]
fn damage_to_an_inactive_generation_does_not_stop_the_daemon() -> TestResult {
    let mut rt = E2eRuntime::boot()?;
    let (inactive, active) = seal_two_generations(&mut rt)?;
    let mut rt = rt.reopen();

    // Content damage the inventory does not look for.
    flip_last_byte(
        &generation_dir(&rt, "indexes/lexical", inactive)?.join(LEXICAL_TEXT_AUTHORITY_MANIFEST),
    )?;
    tamper_semantic_row_count(
        &generation_dir(&rt, "indexes/semantic", inactive)?.join(SEMANTIC_MANIFEST),
    )?;
    // Directories the inventory sets aside.
    let legacy_family = std::fs::canonicalize(rt.state_root())?
        .join("indexes/lexical")
        .join("repo-legacy");
    std::fs::create_dir_all(legacy_family.join("rev-legacy/g1"))?;
    let garbage = pair_dir(&rt, "indexes/lexical")?.join("g9");
    std::fs::create_dir_all(&garbage)?;
    std::fs::write(garbage.join(LEXICAL_IDENTITY), b"\xff\x00not-cbor")?;

    rt.start()?;
    let report = rt
        .boot_inventory()
        .ok_or("the running daemon must expose its boot inventory")?
        .clone();
    if report.active_pairs_validated != 1 {
        return Err(format!(
            "boot must prove exactly the one active pair, proved {}",
            report.active_pairs_validated
        )
        .into());
    }
    if report.lexical.sealed_generations != 2 || report.semantic.sealed_generations != 2 {
        return Err(format!(
            "both sealed generations must be inventoried on both tracks: {report:?}"
        )
        .into());
    }
    let mut lexical_quarantine = report
        .lexical
        .quarantined
        .iter()
        .map(|entry| (entry.path.clone(), entry.reason))
        .collect::<Vec<_>>();
    lexical_quarantine.sort();
    let mut expected = vec![
        (
            legacy_family,
            GenerationQuarantineReasonV1::NonCanonicalLayout,
        ),
        (garbage, GenerationQuarantineReasonV1::IdentityUnreadable),
    ];
    expected.sort();
    if lexical_quarantine != expected {
        return Err(format!("unexpected lexical quarantine set: {lexical_quarantine:?}").into());
    }
    if !report.semantic.quarantined.is_empty() {
        return Err(format!(
            "a content-level manifest tamper is not a quarantine: {:?}",
            report.semantic.quarantined
        )
        .into());
    }

    // The active generation serves.
    let served = rt.query_text(TextQuerySyntax::Native, "needle_second", 5);
    if let Some(error) = served.typed_error {
        return Err(format!("active generation {} does not serve: {error}", active.get()).into());
    }
    if served.candidate_ids.len() != 1 {
        return Err(format!(
            "active generation served {} rows",
            served.candidate_ids.len()
        )
        .into());
    }

    // The damaged inactive generation is refused at its own door, typed.
    let inactive_pin = GenerationPin::new(rt.repo(), rt.revision(), inactive);
    let lexical_answer = rt.query_once(|_| pinned_text(inactive_pin.clone(), "needle_first"))?;
    match typed_code(&lexical_answer) {
        Some(("GENERATION_SIDECAR_CORRUPT", _)) => {}
        other => {
            return Err(format!(
                "lexical query pinned to the damaged generation must be refused typed, got {other:?}"
            )
            .into());
        }
    }
    let semantic_answer = rt.query_once(|_| pinned_semantic(inactive_pin.clone()))?;
    match typed_code(&semantic_answer) {
        Some((code, message))
            if code == "GENERATION_SIDECAR_CORRUPT" && message.contains(SEMANTIC_MANIFEST) => {}
        other => {
            return Err(format!(
                "semantic query pinned to the damaged generation must be refused typed, got {other:?}"
            )
            .into());
        }
    }
    // A quarantined generation is absent from readiness.
    let quarantined_pin = GenerationPin::new(rt.repo(), rt.revision(), ManifestGeneration::new(9));
    let quarantined_answer = rt.query_once(|_| pinned_text(quarantined_pin, "needle"))?;
    match typed_code(&quarantined_answer) {
        Some(("NOT_READY", _)) => Ok(()),
        other => Err(format!(
            "query pinned to a quarantined generation must be NOT_READY, got {other:?}"
        )
        .into()),
    }
}

/// The same damage on the active generation refuses to boot.
///
/// The refusal carries the typed activation cause and is raised before any
/// socket binds. Nothing is recorded: once the bytes are restored the
/// daemon starts on the same active generation, which a quarantine would
/// forbid.
#[test]
fn damage_to_the_active_generation_refuses_to_boot() -> TestResult {
    let mut rt = E2eRuntime::boot()?;
    let (_inactive, active) = seal_two_generations(&mut rt)?;
    let mut rt = rt.reopen();
    let damaged =
        generation_dir(&rt, "indexes/lexical", active)?.join(LEXICAL_TEXT_AUTHORITY_MANIFEST);
    let original = std::fs::read(&damaged)?;
    flip_last_byte(&damaged)?;

    match rt.start() {
        Ok(()) => return Err("the daemon started on a damaged active generation".into()),
        Err(error) => {
            let rendered = format!("{error:#}");
            if !rendered.contains("ACTIVATION_TARGET_UNOPENABLE")
                || !rendered.contains("GENERATION_SIDECAR_CORRUPT")
                || rendered.contains("; quarantined as")
            {
                return Err(format!(
                    "boot must refuse with the typed activation cause and the sidecar code, recording nothing: {rendered}"
                )
                .into());
            }
            if rt.boot_inventory().is_some() {
                return Err("a refused boot must not expose a boot inventory".into());
            }
        }
    }

    std::fs::write(&damaged, &original)?;
    rt.start()?;
    let served = rt.query_text(TextQuerySyntax::Native, "needle_second", 5);
    if served.typed_error.is_some() || served.candidate_ids.len() != 1 {
        return Err(format!(
            "the restored active generation must serve its one row: {:?}",
            served.typed_error
        )
        .into());
    }
    Ok(())
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
    let mut rt = E2eRuntime::boot()?;
    let (_inactive, active) = seal_two_generations(&mut rt)?;
    let mut rt = rt.reopen();

    let state_root = std::fs::canonicalize(rt.state_root())?;
    let lexical_legacy = state_root.join("indexes/lexical").join("repo-legacy");
    std::fs::create_dir_all(lexical_legacy.join("rev-legacy/g1"))?;
    std::fs::write(
        lexical_legacy.join("rev-legacy/g1/leftover.bin"),
        [7_u8; 64],
    )?;
    let lexical_garbage = pair_dir(&rt, "indexes/lexical")?.join("g9");
    std::fs::create_dir_all(&lexical_garbage)?;
    std::fs::write(lexical_garbage.join(LEXICAL_IDENTITY), b"\xff\x00not-cbor")?;
    let semantic_legacy = state_root.join("indexes/semantic").join("repo-legacy");
    std::fs::create_dir_all(semantic_legacy.join("rev-legacy/g1"))?;
    // Seed a real catalog-backed candidate, then corrupt its sealed object.
    // Legacy V1 snapshot roots are intentionally left for the P10 importer.
    let repo_map_root = state_root.join("repo-map");
    let catalog = Arc::new(SqliteCatalog::open(&state_root, Duration::from_secs(5))?);
    let store = RepoMapGenerationStore::open(&repo_map_root, catalog)?.store;
    let bundle = RepoMapSourceBundle::new(
        rt.repo(),
        rt.revision(),
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
    let _receipt = store.ingest_bundle(&bundle)?;
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

    rt.start()?;

    // Listed live, per authority, with reasons.
    let inventory = rt.quarantine_inventory()?;
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
    match rt.discard_quarantined(QuarantineTargetV1::Generation(stale))? {
        Err(error) if error.code.as_wire_str() == "QUARANTINE_TARGET_NOT_QUARANTINED" => {}
        other => return Err(format!("a stale entry must be refused typed: {other:?}").into()),
    }
    if !lexical_garbage.is_dir() {
        return Err("a refused discard removes nothing".into());
    }
    // A sealed generation named as if quarantined removes nothing.
    let sealed = QuarantinedGenerationEntryV1 {
        track: SearchPlaneTrackKind::Lexical,
        path: generation_dir(&rt, "indexes/lexical", active)?
            .display()
            .to_string(),
        reason: "GENERATION_QUARANTINE_IDENTITY_UNREADABLE".to_string(),
        detail: String::new(),
    };
    match rt.discard_quarantined(QuarantineTargetV1::Generation(sealed))? {
        Err(error) if error.code.as_wire_str() == "QUARANTINE_TARGET_NOT_QUARANTINED" => {}
        other => {
            return Err(
                format!("a sealed generation must never be discarded here: {other:?}").into(),
            );
        }
    }
    if !generation_dir(&rt, "indexes/lexical", active)?.is_dir() {
        return Err("the active generation is untouched".into());
    }

    // Every listed entry discards exactly as listed, with its bytes.
    for entry in inventory.lexical.iter().chain(inventory.semantic.iter()) {
        let ack = rt
            .discard_quarantined(QuarantineTargetV1::Generation(entry.clone()))?
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
    match rt.discard_quarantined(QuarantineTargetV1::RepoMapFile(stale_file))? {
        Err(error) if error.code.as_wire_str() == "QUARANTINE_TARGET_NOT_QUARANTINED" => {}
        other => return Err(format!("a stale repo-map reason is refused typed: {other:?}").into()),
    }
    if rt.quarantine_inventory()?.repo_map != inventory.repo_map {
        return Err("a refused repo-map discard removes nothing".into());
    }
    let ack = rt
        .discard_quarantined(QuarantineTargetV1::RepoMapFile(repo_map_entry.clone()))?
        .map_err(|error| format!("discard the repo-map file: {error:?}"))?;
    if ack.outcome != (QuarantineDiscardOutcomeDtoV1::Discarded { bytes: 15 }) {
        return Err(format!(
            "the repo-map file's 15 bytes are reported: {:?}",
            ack.outcome
        )
        .into());
    }
    if !rt.quarantine_inventory()?.repo_map.is_empty() {
        return Err("the repo-map file is gone after its discard".into());
    }
    // Discarding again is idempotent: nothing is there.
    let again = rt
        .discard_quarantined(QuarantineTargetV1::RepoMapFile(repo_map_entry))?
        .map_err(|error| format!("a second repo-map discard: {error:?}"))?;
    if again.outcome != QuarantineDiscardOutcomeDtoV1::Absent {
        return Err(format!("a second discard is Absent, got {:?}", again.outcome).into());
    }
    let again = rt
        .discard_quarantined(QuarantineTargetV1::Generation(listed_garbage))?
        .map_err(|error| format!("a second generation discard: {error:?}"))?;
    if again.outcome != QuarantineDiscardOutcomeDtoV1::Absent {
        return Err(format!("a second discard is Absent, got {:?}", again.outcome).into());
    }

    // The live listing and a fresh boot agree: nothing is quarantined.
    let after = rt.quarantine_inventory()?;
    if !(after.lexical.is_empty() && after.semantic.is_empty() && after.repo_map.is_empty()) {
        return Err(format!("nothing is quarantined after the discards: {after:?}").into());
    }
    let mut rt = rt.reopen();
    rt.start()?;
    let report = rt
        .boot_inventory()
        .ok_or("the running daemon must expose its boot inventory")?;
    if !(report.lexical.quarantined.is_empty()
        && report.semantic.quarantined.is_empty()
        && report.repo_map.quarantined.is_empty())
    {
        return Err(format!("a fresh boot finds nothing to quarantine: {report:?}").into());
    }
    if report.lexical.sealed_generations != 2 || report.semantic.sealed_generations != 2 {
        return Err("the sealed generations survived the discards".into());
    }
    Ok(())
}

/// Three sealed and activated generations, each as its sealed receipt
/// attested it, oldest first; the last is active.
fn seal_three_identities(
    rt: &mut E2eRuntime,
) -> Result<Vec<SearchCorpusGenerationIdentityV1>, Box<dyn Error>> {
    let mut identities = Vec::new();
    for (path, body) in [
        ("src/first.rs", "fn first() { needle_first }"),
        ("src/second.rs", "fn second() { needle_second }"),
        ("src/third.rs", "fn third() { needle_third }"),
    ] {
        rt.ingest_text("repo", path, body)?;
        let _sealed = rt.seal()?;
        identities.push(
            rt.last_sealed_search_corpus_identity()
                .ok_or("a sealed receipt names its composite identity")?,
        );
        rt.activate_last_sealed_generation()?;
    }
    Ok(identities)
}

fn control_refusal(response: &SearchPlaneControlIpcResponse) -> Option<(&'static str, String)> {
    match response {
        SearchPlaneControlIpcResponse::Error(error) => {
            Some((error.code.as_wire_str(), error.message.clone()))
        }
        SearchPlaneControlIpcResponse::SearchCorpusActivationCasAck(_)
        | SearchPlaneControlIpcResponse::SearchCorpusRollbackCasAck(_)
        | SearchPlaneControlIpcResponse::RepoMapMutationAck(_)
        | SearchPlaneControlIpcResponse::RepoMapTerminalReceiptV2(_)
        | SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(_)
        | SearchPlaneControlIpcResponse::GenerationStatusReport(_)
        | SearchPlaneControlIpcResponse::MetricsSnapshot(_)
        | SearchPlaneControlIpcResponse::ProcessReadinessReport(_)
        | SearchPlaneControlIpcResponse::QuarantineInventory(_)
        | SearchPlaneControlIpcResponse::QuarantineDiscardAck(_) => None,
    }
}

fn rollback_to(
    rt: &mut E2eRuntime,
    active: &SearchCorpusGenerationIdentityV1,
    target: &SearchCorpusGenerationIdentityV1,
) -> Result<Option<(&'static str, String)>, Box<dyn Error>> {
    let answer =
        rt.rollback_search_corpus_cas_raw(SearchPlaneRollbackSearchCorpusGenerationCasRequest {
            expected_active: active.clone(),
            target: target.clone(),
        })?;
    Ok(control_refusal(&answer))
}

/// The daemon's active pair is exactly `active`: both tracks and the
/// semantic content roots.
fn expect_active(rt: &mut E2eRuntime, active: &SearchCorpusGenerationIdentityV1) -> TestResult {
    let status = rt.generation_status()?;
    for expected in [&active.lexical, &active.semantic] {
        let observed = status
            .tracks
            .iter()
            .find(|record| record.track == expected.track)
            .ok_or_else(|| format!("no active {:?} track: {status:?}", expected.track))?;
        if observed.manifest_generation != expected.manifest_generation
            || observed.manifest_digest != expected.manifest_digest
        {
            return Err(format!("the active {:?} track moved: {status:?}", expected.track).into());
        }
    }
    if status.semantic_content.as_ref() != Some(&active.semantic_content) {
        return Err(format!("the active content roots moved: {status:?}").into());
    }
    Ok(())
}

fn listed(entries: &[QuarantinedGenerationEntryV1]) -> Vec<(String, String)> {
    entries
        .iter()
        .map(|entry| (entry.path.clone(), entry.reason.clone()))
        .collect()
}

fn content_corrupt_at(dir: &Path) -> (String, String) {
    (
        dir.display().to_string(),
        GenerationQuarantineReasonV1::ContentCorrupt
            .as_code_str()
            .to_string(),
    )
}

fn boot_listed(
    entries: &[QuarantinedGenerationV1],
) -> Vec<(PathBuf, GenerationQuarantineReasonV1)> {
    entries
        .iter()
        .map(|entry| (entry.path.clone(), entry.reason))
        .collect()
}

/// A rollback that proves inactive damage quarantines what it proved
/// (QI-BB-026 보완 #2).
///
/// A query door that meets the damage answers typed and records nothing.
/// A rollback to the damaged generation is refused typed, and the adapter
/// that owns the damaged track re-proves it and leaves a content-corrupt
/// receipt — the lexical track of one generation, the semantic track of
/// another. From then on the live listing names exactly those two; the
/// bytes coming back lifts nothing, so the rollback and a pinned query are
/// refused as quarantined; the active pair never moves, and a reboot
/// starts on it and lists both; each is discarded exactly as listed.
#[test]
fn a_rollback_that_proves_inactive_damage_quarantines_it() -> TestResult {
    let mut rt = E2eRuntime::boot()?;
    let identities = seal_three_identities(&mut rt)?;
    let [first, second, active] = identities.as_slice() else {
        return Err("three generations were sealed".into());
    };
    let mut rt = rt.reopen();
    let lexical_first = generation_dir(&rt, "indexes/lexical", first.lexical.manifest_generation)?;
    let semantic_second =
        generation_dir(&rt, "indexes/semantic", second.semantic.manifest_generation)?;
    let lexical_file = lexical_first.join(LEXICAL_TEXT_AUTHORITY_MANIFEST);
    let semantic_file = semantic_second.join(SEMANTIC_MANIFEST);
    let lexical_bytes = std::fs::read(&lexical_file)?;
    let semantic_bytes = std::fs::read(&semantic_file)?;
    flip_last_byte(&lexical_file)?;
    tamper_semantic_row_count(&semantic_file)?;
    rt.start()?;

    // A query door meets the damage, answers typed, and records nothing.
    let first_pin = GenerationPin::new(rt.repo(), rt.revision(), first.lexical.manifest_generation);
    match typed_code(&rt.query_once(|_| pinned_text(first_pin.clone(), "needle_first"))?) {
        Some(("GENERATION_SIDECAR_CORRUPT", _)) => {}
        other => {
            return Err(
                format!("the pinned query must meet the damage typed, got {other:?}").into(),
            );
        }
    }
    let before = rt.quarantine_inventory()?;
    if !(before.lexical.is_empty() && before.semantic.is_empty()) {
        return Err(format!("a query door recorded a quarantine: {before:?}").into());
    }

    // Each rollback proves its target's damaged track, is refused, and
    // the owning adapter records what it proved.
    for (target, damaged) in [(first, &lexical_first), (second, &semantic_second)] {
        let recorded = format!(
            "; quarantined as GENERATION_QUARANTINE_CONTENT_CORRUPT at {}",
            damaged.display()
        );
        match rollback_to(&mut rt, active, target)? {
            Some((code, message))
                if code == "ROLLBACK_TARGET_UNOPENABLE"
                    && message.contains("GENERATION_SIDECAR_CORRUPT")
                    && message.contains(&recorded) => {}
            other => {
                return Err(format!(
                    "the rollback to generation {} must be refused and recorded, got {other:?}",
                    target.lexical.manifest_generation.get()
                )
                .into());
            }
        }
    }
    let after = rt.quarantine_inventory()?;
    if listed(&after.lexical) != vec![content_corrupt_at(&lexical_first)]
        || listed(&after.semantic) != vec![content_corrupt_at(&semantic_second)]
    {
        return Err(format!("the listing must name exactly what was proved: {after:?}").into());
    }

    // The receipts are durable: the bytes coming back lifts nothing.
    std::fs::write(&lexical_file, &lexical_bytes)?;
    std::fs::write(&semantic_file, &semantic_bytes)?;
    for target in [first, second] {
        match rollback_to(&mut rt, active, target)? {
            Some((code, message))
                if code == "ROLLBACK_TARGET_UNOPENABLE"
                    && message.contains("GENERATION_QUARANTINED")
                    && !message.contains("; quarantined as") => {}
            other => {
                return Err(format!(
                    "a quarantined target must be refused as quarantined, got {other:?}"
                )
                .into());
            }
        }
    }
    match typed_code(&rt.query_once(|_| pinned_text(first_pin.clone(), "needle_first"))?) {
        Some(("GENERATION_QUARANTINED", _)) => {}
        other => {
            return Err(
                format!("a pinned query must be refused as quarantined, got {other:?}").into(),
            );
        }
    }
    expect_active(&mut rt, active)?;

    // A reboot starts on the active pair and lists both.
    let mut rt = rt.reopen();
    rt.start()?;
    let report = rt
        .boot_inventory()
        .ok_or("the running daemon must expose its boot inventory")?
        .clone();
    if report.active_pairs_validated != 1
        || boot_listed(&report.lexical.quarantined)
            != vec![(
                lexical_first.clone(),
                GenerationQuarantineReasonV1::ContentCorrupt,
            )]
        || boot_listed(&report.semantic.quarantined)
            != vec![(
                semantic_second.clone(),
                GenerationQuarantineReasonV1::ContentCorrupt,
            )]
    {
        return Err(format!("the reboot must prove one pair and list both: {report:?}").into());
    }
    expect_active(&mut rt, active)?;

    // Each is discarded exactly as listed.
    let listing = rt.quarantine_inventory()?;
    for entry in listing.lexical.iter().chain(listing.semantic.iter()) {
        match rt.discard_quarantined(QuarantineTargetV1::Generation(entry.clone()))? {
            Ok(ack) if matches!(ack.outcome, QuarantineDiscardOutcomeDtoV1::Discarded { .. }) => {}
            other => return Err(format!("discard {}: {other:?}", entry.path).into()),
        }
    }
    let emptied = rt.quarantine_inventory()?;
    if !(emptied.lexical.is_empty() && emptied.semantic.is_empty())
        || lexical_first.exists()
        || semantic_second.exists()
    {
        return Err(format!("the discards must remove both: {emptied:?}").into());
    }
    Ok(())
}

/// Damage to the active semantic generation refuses to boot and records
/// nothing (QI-BB-026).
///
/// The serve head fails closed before any socket binds, naming the
/// activation cause, the track and the semantic manifest. No receipt is
/// left: once the bytes are restored the daemon starts and serves the same
/// active pair, which a quarantine would forbid.
#[test]
fn damage_to_the_active_semantic_generation_refuses_to_boot_and_records_nothing() -> TestResult {
    let mut rt = E2eRuntime::boot()?;
    let (_inactive, active) = seal_two_generations(&mut rt)?;
    let mut rt = rt.reopen();
    let manifest = generation_dir(&rt, "indexes/semantic", active)?.join(SEMANTIC_MANIFEST);
    let original = std::fs::read(&manifest)?;
    tamper_semantic_row_count(&manifest)?;

    match rt.start() {
        Ok(()) => return Err("the daemon started on a damaged active semantic generation".into()),
        Err(error) => {
            let rendered = format!("{error:#}");
            if !(rendered.contains("ACTIVATION_TARGET_UNOPENABLE")
                && rendered.contains("GENERATION_SIDECAR_CORRUPT")
                && rendered.contains("track=Semantic")
                && rendered.contains(SEMANTIC_MANIFEST))
                || rendered.contains("; quarantined as")
            {
                return Err(format!(
                    "boot must refuse with the typed activation cause on the semantic track and record nothing: {rendered}"
                )
                .into());
            }
            if rt.boot_inventory().is_some() {
                return Err("a refused boot must not expose a boot inventory".into());
            }
        }
    }

    std::fs::write(&manifest, &original)?;
    rt.start()?;
    let served = rt.query_text(TextQuerySyntax::Native, "needle_second", 5);
    if let Some(error) = served.typed_error {
        return Err(format!("the restored active generation does not serve: {error}").into());
    }
    if served.candidate_ids.len() != 1 {
        return Err(format!(
            "the active generation served {} rows",
            served.candidate_ids.len()
        )
        .into());
    }
    let listing = rt.quarantine_inventory()?;
    if !(listing.lexical.is_empty() && listing.semantic.is_empty()) {
        return Err(format!("a refused boot recorded a quarantine: {listing:?}").into());
    }
    Ok(())
}

/// A lexical-only seal of `generation` for the harness's pair: what a crash
/// between the two tracks' seals leaves behind.
fn lexical_half(rt: &E2eRuntime, generation: ManifestGeneration) -> SearchCorpusIngestBatch {
    SearchCorpusIngestBatch {
        repo_id: rt.repo(),
        revision_id: rt.revision(),
        generation,
        base_generation: None,
        manifest_digest: format!("lex-seal:{}", generation.get()),
        batch_digest: String::new(),
        mode: BatchIngestMode::ReplaceGeneration,
        bundle_payload: None,
        clear_surfaces: Vec::new(),
        replace_scopes: Vec::new(),
        tombstone_scopes: Vec::new(),
        semantic_replace_scopes: Vec::new(),
        semantic_tombstone_scopes: Vec::new(),
        seal: true,
    }
}

/// A generation sealed on one track only is named at boot (QI-BB-029
/// 보완 #4).
///
/// A crash between the two tracks' seals leaves a generation's lexical
/// track sealed, its semantic track without it, and no authority record;
/// the fixture is that state, sealed by the lexical adapter itself on the
/// stopped daemon's state root. Boot starts on the active pair, the report
/// names the half — pair generation, sealed track, path — and the gauge
/// counts it, the live listing offers the sealed half as an orphan, and
/// once it is discarded as listed the next boot finds no half-sealed pair.
#[test]
fn a_generation_sealed_on_one_track_only_is_named_at_boot() -> TestResult {
    let mut rt = E2eRuntime::boot()?;
    rt.ingest_text("repo", "src/first.rs", "fn first() { needle_first }")?;
    let _first = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    let mut rt = rt.reopen();
    let half = ManifestGeneration::new(7);
    let lexical_root = std::fs::canonicalize(rt.state_root())?.join("indexes/lexical");
    LexicalAdapter::with_state_root(lexical_root).build_batch(&lexical_half(&rt, half))?;
    let half_dir = generation_dir(&rt, "indexes/lexical", half)?;
    if !half_dir.is_dir() || generation_dir(&rt, "indexes/semantic", half)?.exists() {
        return Err("the fixture seals the lexical half only".into());
    }

    rt.start()?;
    let report = rt
        .boot_inventory()
        .ok_or("the running daemon must expose its boot inventory")?
        .clone();
    let expected = vec![HalfSealedPair {
        key: SealedGenerationKey {
            repo_id: rt.repo(),
            revision_id: rt.revision(),
            generation: half,
        },
        sealed_track: SearchPlaneTrackKind::Lexical,
        path: half_dir.clone(),
    }];
    if report.half_sealed_pairs != expected || report.active_pairs_validated != 1 {
        return Err(format!("the boot report must name the half: {report:?}").into());
    }
    let gauge = |rt: &mut E2eRuntime| -> Result<f64, Box<dyn Error>> {
        rt.metrics_snapshot()?
            .gauges
            .iter()
            .find(|gauge| gauge.name == "boot_half_sealed_pairs")
            .map(|gauge| gauge.value)
            .ok_or_else(|| "the scrape carries boot_half_sealed_pairs".into())
    };
    if (gauge(&mut rt)? - 1.0).abs() > f64::EPSILON {
        return Err("the gauge counts the one half-sealed pair".into());
    }
    let listing = rt.quarantine_inventory()?;
    let orphan = (
        half_dir.display().to_string(),
        GenerationQuarantineReasonV1::Orphaned
            .as_code_str()
            .to_string(),
    );
    if listed(&listing.lexical) != vec![orphan] || !listing.semantic.is_empty() {
        return Err(format!("the listing must offer the sealed half: {listing:?}").into());
    }
    let entry = listing
        .lexical
        .first()
        .cloned()
        .ok_or("the half is listed")?;
    match rt.discard_quarantined(QuarantineTargetV1::Generation(entry))? {
        Ok(ack) if matches!(ack.outcome, QuarantineDiscardOutcomeDtoV1::Discarded { .. }) => {}
        other => return Err(format!("discard the half: {other:?}").into()),
    }

    let mut rt = rt.reopen();
    rt.start()?;
    let report = rt
        .boot_inventory()
        .ok_or("the running daemon must expose its boot inventory")?;
    if !report.half_sealed_pairs.is_empty() || half_dir.exists() {
        return Err(format!("no half-sealed pair is left: {report:?}").into());
    }
    if gauge(&mut rt)?.abs() > f64::EPSILON {
        return Err("the gauge reads zero once the half is gone".into());
    }
    Ok(())
}
