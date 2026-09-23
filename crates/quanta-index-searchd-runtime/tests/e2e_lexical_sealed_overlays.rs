//! QI-BB-030 at the daemon: a sealed lexical generation's repo-metadata
//! overlays are part of its sealed contract.
//!
//! Before this, the aux ingest routes wrote their overlay snapshots into a
//! sealed generation directory with plain writes the manifest never
//! covered, so a publish that crashed mid-write left an overlay activation
//! and restart admitted and the first query could not decode. Now an
//! overlay publish into a sealed generation is refused typed over the
//! ingest socket, and a torn overlay on disk is refused by the same walk
//! the query open runs: restart refuses to promote a damaged active
//! generation, and a query pinned to a damaged inactive one is refused
//! typed instead of failing to decode.

#![forbid(unsafe_code)]

use std::error::Error;
use std::path::PathBuf;

use quanta_index_contract::{
    GenerationPin, ManifestGeneration, QueryConstraintSetV1, RepoMetaEntry, RepoMetaIngestBatch,
    SearchPlaneIngestIpcRequest, SearchPlaneIngestIpcResponse, SearchPlaneQueryIpcRequest,
    SearchPlaneQueryIpcResponse, TextQueryRequest, TextQuerySyntax,
};
use quanta_index_core::GenerationStorageKeyV1;
use quanta_index_searchd_harness as e2e_harness;

use e2e_harness::E2eRuntime;

type TestResult = Result<(), Box<dyn Error>>;

const REPO_META_FILE: &str = "repo-meta.cbor";

/// The content needle of the generation stamped `value`: one token, so a
/// query names exactly the document that generation added even after a
/// delta carried the earlier documents forward.
fn needle(value: &str) -> String {
    format!("overlayneedle{value}")
}

fn lexical_generation_dir(
    rt: &E2eRuntime,
    generation: ManifestGeneration,
) -> Result<PathBuf, Box<dyn Error>> {
    let canonical = std::fs::canonicalize(rt.state_root())?;
    Ok(canonical
        .join("indexes/lexical")
        .join(GenerationStorageKeyV1::for_repo_revision(&rt.repo(), &rt.revision()).as_str())
        .join(format!("g{}", generation.get())))
}

fn repo_meta_batch(
    rt: &E2eRuntime,
    generation: ManifestGeneration,
    value: &str,
) -> RepoMetaIngestBatch {
    RepoMetaIngestBatch {
        repo_id: rt.repo(),
        revision_id: rt.revision(),
        generation,
        batch_digest: format!("e2e-sealed-overlays:meta:{}:{value}", generation.get()),
        entries: vec![RepoMetaEntry {
            source_repo_id: rt.repo(),
            key: "lifecycle".to_string(),
            value: value.to_string(),
        }],
    }
}

/// One generation whose repo-meta overlay was published before its seal.
fn seal_with_overlay(
    rt: &mut E2eRuntime,
    path: &str,
    value: &str,
) -> Result<ManifestGeneration, Box<dyn Error>> {
    rt.ingest_text("repo", path, &format!("fn f() {{ {} }}", needle(value)))?;
    let generation = rt.current_generation();
    rt.publish_repo_meta_batch(repo_meta_batch(rt, generation, value))?;
    let sealed = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    if sealed != generation {
        return Err(format!("sealed {sealed:?}, expected {generation:?}").into());
    }
    Ok(sealed)
}

fn pinned_meta_query(pin: GenerationPin, value: &str) -> SearchPlaneQueryIpcRequest {
    SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
        syntax: TextQuerySyntax::Sourcegraph,
        query_text: format!("repo:has.meta(lifecycle:{value}) {}", needle(value)),
        constraints: QueryConstraintSetV1::unconstrained(),
        generation: Some(pin),
        generation_selector: None,
        top_k: 5,
        cursor: None,
    })
}

fn typed_query_code(response: &SearchPlaneQueryIpcResponse) -> Option<&'static str> {
    match response {
        SearchPlaneQueryIpcResponse::Error(error) => Some(error.code.as_wire_str()),
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

fn expect_meta_hit(rt: &mut E2eRuntime, value: &str, what: &str) -> TestResult {
    let served = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        &format!("repo:has.meta(lifecycle:{value}) {}", needle(value)),
        5,
    );
    if let Some(error) = served.typed_error {
        return Err(format!(
            "{what}: query refused typed {}: {}",
            error.code, error.message
        )
        .into());
    }
    if served.candidate_ids.len() != 1 {
        return Err(format!(
            "{what}: expected one hit under the sealed overlay, got {:?}",
            served.candidate_ids
        )
        .into());
    }
    Ok(())
}

/// An overlay publish into the sealed, active generation is refused typed
/// over the ingest socket.
///
/// The generation keeps serving its sealed overlay unchanged — through the
/// resident handle and after a restart.
#[test]
fn an_overlay_publish_into_a_sealed_generation_is_refused_typed() -> TestResult {
    let mut rt = E2eRuntime::boot()?;
    let sealed = seal_with_overlay(&mut rt, "src/one.rs", "one")?;
    expect_meta_hit(&mut rt, "one", "before the refused publish")?;

    // A well-formed publish with its canonical digest: the refusal under
    // test is the sealed generation's, not the digest gate's.
    let response = rt.ingest_once(e2e_harness::stamped_ingest_request(
        SearchPlaneIngestIpcRequest::PublishRepoMetaBatch(repo_meta_batch(&rt, sealed, "late")),
    )?)?;
    let SearchPlaneIngestIpcResponse::Error(error) = response else {
        return Err(format!(
            "an overlay publish into a sealed generation must be refused typed GENERATION_IMMUTABLE, got {response:?}"
        )
        .into());
    };
    if error.code.as_wire_str() != "GENERATION_IMMUTABLE" {
        return Err(format!(
            "an overlay publish into a sealed generation must be refused typed GENERATION_IMMUTABLE, got {}: {}",
            error.code, error.message
        )
        .into());
    }
    let overlay = lexical_generation_dir(&rt, sealed)?.join(REPO_META_FILE);
    let after_refusal = std::fs::read(&overlay)?;
    expect_meta_hit(&mut rt, "one", "after the refused publish")?;
    let late = rt.query_text(
        TextQuerySyntax::Sourcegraph,
        &format!("repo:has.meta(lifecycle:late) {}", needle("one")),
        5,
    );
    if !late.candidate_ids.is_empty() {
        return Err("the refused publish must not be visible to a query".into());
    }

    let mut rt = rt.reopen();
    if std::fs::read(&overlay)? != after_refusal {
        return Err("a restart changed the sealed overlay".into());
    }
    expect_meta_hit(&mut rt, "one", "after a restart")
}

/// The audit's scenario, at the daemon: an overlay torn on disk after the
/// seal is refused by restart when it belongs to the active generation,
/// and by the query door when it belongs to an inactive one.
#[test]
fn a_torn_overlay_is_refused_by_restart_and_by_the_query_door() -> TestResult {
    let mut rt = E2eRuntime::boot()?;
    let first = seal_with_overlay(&mut rt, "src/first.rs", "first")?;
    let second = seal_with_overlay(&mut rt, "src/second.rs", "second")?;
    expect_meta_hit(&mut rt, "second", "active before the damage")?;

    // Damage the inactive predecessor: the daemon restarts, the active
    // generation serves, and a query pinned to the damaged one is refused
    // typed by the same walk activation would run.
    let mut rt = rt.reopen();
    let first_overlay = lexical_generation_dir(&rt, first)?.join(REPO_META_FILE);
    let original = std::fs::read(&first_overlay)?;
    let half = original.len().div_euclid(2);
    std::fs::write(&first_overlay, original.get(..half).ok_or("short overlay")?)?;
    rt.start()?;
    expect_meta_hit(&mut rt, "second", "active after the inactive damage")?;
    let pinned = GenerationPin::new(rt.repo(), rt.revision(), first);
    let answer = rt.query_once(|_| pinned_meta_query(pinned.clone(), "first"))?;
    let code = typed_query_code(&answer);
    if code != Some("GENERATION_SIDECAR_CORRUPT") {
        return Err(format!(
            "a query pinned to the generation with the torn overlay must be refused typed GENERATION_SIDECAR_CORRUPT, got {code:?}"
        )
        .into());
    }
    std::fs::write(&first_overlay, &original)?;

    // Damage the active generation: restart refuses to promote it with the
    // typed activation cause, before any socket binds.
    let mut rt = rt.reopen();
    let second_overlay = lexical_generation_dir(&rt, second)?.join(REPO_META_FILE);
    std::fs::write(&second_overlay, b"")?;
    match rt.start() {
        Ok(()) => Err("the daemon started on an active generation whose overlay is torn".into()),
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
            Ok(())
        }
    }
}
