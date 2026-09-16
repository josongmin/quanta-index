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

#![forbid(unsafe_code)]

use std::error::Error;
use std::path::{Path, PathBuf};

use quanta_index_contract::{
    GenerationPin, ManifestGeneration, SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcResponse,
    SemanticQueryRequest, TextQueryRequest, TextQuerySyntax,
};
use quanta_index_core::{GenerationQuarantineReasonV1, GenerationStorageKeyV1};
use quanta_index_searchd_harness as e2e_harness;

use e2e_harness::E2eRuntime;

type TestResult = Result<(), Box<dyn Error>>;

const LEXICAL_DOCS_SIDECAR: &str = "text-authority-docs.cbor";
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

/// Rewrite the manifest's `row_count` so the marker still matches the
/// manifest digest (the inventory is satisfied) but the table does not
/// match the manifest (every open refuses).
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

fn typed_code(response: &SearchPlaneQueryIpcResponse) -> Option<(String, String)> {
    match response {
        SearchPlaneQueryIpcResponse::Error(error) => {
            Some((error.code.clone(), error.message.clone()))
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
        | SearchPlaneQueryIpcResponse::RuntimeMetadata(_) => None,
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
    flip_last_byte(&generation_dir(&rt, "indexes/lexical", inactive)?.join(LEXICAL_DOCS_SIDECAR))?;
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
        Some((code, _)) if code == "GENERATION_SIDECAR_CORRUPT" => {}
        other => {
            return Err(format!(
                "lexical query pinned to the damaged generation must be refused typed, got {other:?}"
            )
            .into());
        }
    }
    let semantic_answer = rt.query_once(|_| pinned_semantic(inactive_pin.clone()))?;
    match typed_code(&semantic_answer) {
        Some((_, message)) if message.contains("row count") => {}
        other => {
            return Err(format!(
                "semantic query pinned to the damaged generation must be refused with the row-count error, got {other:?}"
            )
            .into());
        }
    }
    // A quarantined generation is absent from readiness.
    let quarantined_pin = GenerationPin::new(rt.repo(), rt.revision(), ManifestGeneration::new(9));
    let quarantined_answer = rt.query_once(|_| pinned_text(quarantined_pin, "needle"))?;
    match typed_code(&quarantined_answer) {
        Some((code, _)) if code == "NOT_READY" => Ok(()),
        other => Err(format!(
            "query pinned to a quarantined generation must be NOT_READY, got {other:?}"
        )
        .into()),
    }
}

/// The same damage on the active generation refuses to boot.
///
/// The refusal carries the typed activation cause and is raised before any
/// socket binds.
#[test]
fn damage_to_the_active_generation_refuses_to_boot() -> TestResult {
    let mut rt = E2eRuntime::boot()?;
    let (_inactive, active) = seal_two_generations(&mut rt)?;
    let mut rt = rt.reopen();
    flip_last_byte(&generation_dir(&rt, "indexes/lexical", active)?.join(LEXICAL_DOCS_SIDECAR))?;

    match rt.start() {
        Ok(()) => Err("the daemon started on a damaged active generation".into()),
        Err(error) => {
            let rendered = format!("{error:#}");
            if !rendered.contains("ACTIVATION_TARGET_UNOPENABLE")
                || !rendered.contains("GENERATION_SIDECAR_CORRUPT")
            {
                return Err(format!(
                    "boot must refuse with the typed activation cause and the sidecar code: {rendered}"
                )
                .into());
            }
            if rt.boot_inventory().is_some() {
                return Err("a refused boot must not expose a boot inventory".into());
            }
            Ok(())
        }
    }
}
