//! QI-BB-008 — the `RepoMap` store retires superseded generations on
//! activation, survives a lost or damaged file with a report instead of a
//! failed open, and answers fail-closed for an activation whose snapshot
//! is gone.

#![forbid(unsafe_code)]
#![expect(
    clippy::unreachable,
    reason = "test fixtures use invariant literal constructors for repo and revision IDs"
)]

use std::collections::BTreeMap;
use std::error::Error;
use std::path::Path;

use quanta_index_contract::{
    ManifestGeneration, RepoId, RepoMapActivateGenerationRequest, RepoMapDocType,
    RepoMapExactnessSummary, RepoMapGraphCoverageClass, RepoMapItemIndexAvailability,
    RepoMapQueryRequest, RepoMapRedactionState, RepoMapSnapshotMeta, RevisionId,
};
use quanta_index_core::CoreError;
use quanta_index_repomap::{RepoMapEntry, RepoMapGenerationStore, RepoMapSnapshot};

type TestResult = Result<(), Box<dyn Error>>;

fn repo() -> RepoId {
    match RepoId::new("repo-durable") {
        Ok(repo) => repo,
        Err(err) => unreachable!("static fixture ID satisfies canonical policy: {err}"),
    }
}

fn revision() -> RevisionId {
    match RevisionId::new("rev-durable") {
        Ok(revision) => revision,
        Err(err) => unreachable!("static fixture ID satisfies canonical policy: {err}"),
    }
}

fn snapshot(generation: u64) -> RepoMapSnapshot {
    RepoMapSnapshot {
        repo_id: repo(),
        revision_id: revision(),
        manifest_generation: ManifestGeneration::new(generation),
        snapshot_meta: RepoMapSnapshotMeta {
            snapshot_id: format!("snap-{generation}"),
            projection_version: 1,
            authority_digest: format!("authority-{generation}"),
            item_index_availability: RepoMapItemIndexAvailability::Full,
            graph_coverage_class: RepoMapGraphCoverageClass::Full,
            exactness_summary: RepoMapExactnessSummary::Exact,
        },
        entries: vec![RepoMapEntry {
            subject_identity: format!("src/g{generation}.rs"),
            subject_doc_type: RepoMapDocType::File,
            subject_kind: "file".to_string(),
            owner_path: format!("src/g{generation}.rs"),
            score: 1.0,
            final_score_millis: 1000,
            importance_score_millis: 0,
            utility_score_millis: 0,
            freshness_score_millis: 0,
            evidence_priority_millis: 0,
            token_budget_hint: 8,
            contributing_signals: BTreeMap::new(),
            projection_evidence_kind: "bundle".to_string(),
            projection_authority_artifact_id: "artifact".to_string(),
            projection_authority_digest: "digest".to_string(),
            projection_status: "fresh".to_string(),
            redaction_state: RepoMapRedactionState::Unredacted,
            search_text: format!("generation {generation}"),
            source_symbol_count: 0,
            source_chunk_token_total: 0,
            source_call_incoming_edges: 0,
            source_call_outgoing_edges: 0,
            source_import_incoming_edges: 0,
            source_import_outgoing_edges: 0,
        }],
    }
}

fn activate(store: &RepoMapGenerationStore, generation: u64) -> Result<(), CoreError> {
    store.activate_generation(&RepoMapActivateGenerationRequest {
        repo_id: repo(),
        revision_id: revision(),
        manifest_generation: ManifestGeneration::new(generation),
        manifest_digest: format!("manifest-{generation}"),
    })
}

fn query(store: &RepoMapGenerationStore, generation: u64) -> Result<Vec<String>, CoreError> {
    let response = store.read_query_snapshot(&RepoMapQueryRequest {
        repo_id: repo(),
        revision_id: revision(),
        manifest_generation: ManifestGeneration::new(generation),
        query_text: "generation".to_string(),
        top_k: 4,
        token_budget: 64,
        focus_subjects: Vec::new(),
    })?;
    Ok(response
        .entries
        .into_iter()
        .map(|entry| entry.subject_identity)
        .collect())
}

fn snapshot_files(root: &Path) -> Result<Vec<String>, Box<dyn Error>> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(root.join("snapshots"))? {
        let entry = entry?;
        if let Some(name) = entry.file_name().to_str() {
            names.push(name.to_owned());
        }
    }
    names.sort();
    Ok(names)
}

#[test]
fn activation_retires_older_generations_on_disk_and_in_memory_and_keeps_newer_ones() -> TestResult {
    let temp = tempfile::tempdir()?;
    let opened = RepoMapGenerationStore::open(temp.path())?;
    let store = opened.store;
    for generation in 1..=4 {
        store.insert_snapshot(snapshot(generation))?;
    }
    if snapshot_files(temp.path())?.len() != 4 {
        return Err("four snapshot files before activation".into());
    }
    activate(&store, 3)?;
    if store.resident_generations_for(&repo(), &revision())? != vec![3, 4] {
        return Err(format!(
            "generations older than the activated one are retired, newer kept: {:?}",
            store.resident_generations_for(&repo(), &revision())?
        )
        .into());
    }
    let files = snapshot_files(temp.path())?;
    if files.len() != 2
        || files
            .iter()
            .any(|name| name.contains("--g1.") || name.contains("--g2."))
    {
        return Err(format!("retired generations leave no file behind: {files:?}").into());
    }
    if query(&store, 3)? != vec!["src/g3.rs".to_string()] {
        return Err("the activated generation serves".into());
    }
    // Activating again is idempotent for retention; a reopen agrees.
    activate(&store, 3)?;
    let reopened = RepoMapGenerationStore::open(temp.path())?;
    if !reopened.report.quarantined.is_empty()
        || !reopened.report.activations_without_snapshot.is_empty()
        || reopened.report.snapshots_loaded != 2
        || reopened.report.activations_loaded != 1
    {
        return Err(format!("clean reopen: {:?}", reopened.report).into());
    }
    if reopened
        .store
        .resident_generations_for(&repo(), &revision())?
        != vec![3, 4]
        || query(&reopened.store, 3)? != vec!["src/g3.rs".to_string()]
    {
        return Err("the reopened store serves the activated generation".into());
    }
    Ok(())
}

#[test]
fn a_lost_activated_snapshot_is_reported_and_answers_not_found_never_a_stale_generation()
-> TestResult {
    let temp = tempfile::tempdir()?;
    let store = RepoMapGenerationStore::open(temp.path())?.store;
    store.insert_snapshot(snapshot(1))?;
    store.insert_snapshot(snapshot(2))?;
    activate(&store, 2)?;
    // A newer generation arrives but is not activated yet.
    store.insert_snapshot(snapshot(3))?;
    drop(store);
    // The activated generation's file is gone; the newer one is still there,
    // beside a file that is not a snapshot at all.
    std::fs::write(
        temp.path().join("snapshots").join("stale--marker.json"),
        b"not json at all",
    )?;
    for name in snapshot_files(temp.path())? {
        if name.contains("--g2.") {
            std::fs::remove_file(temp.path().join("snapshots").join(name))?;
        }
    }
    let reopened = RepoMapGenerationStore::open(temp.path())?;
    if reopened.report.activations_without_snapshot.len() != 1
        || reopened.report.quarantined.len() != 1
        || reopened
            .report
            .quarantined
            .first()
            .is_none_or(|entry| entry.file_name != "stale--marker.json")
    {
        return Err(format!("the open reports what it found: {:?}", reopened.report).into());
    }
    match query(&reopened.store, 2) {
        Err(CoreError::NotFound(message)) if message.contains("no activated generation") => {}
        other => return Err(format!("a lost activation answers NOT_FOUND, got {other:?}").into()),
    }
    match query(&reopened.store, 3) {
        Err(CoreError::NotFound(_)) => {}
        other => {
            return Err(format!(
                "an unactivated generation is never served instead, got {other:?}"
            )
            .into());
        }
    }
    if reopened
        .store
        .activated_generation_for(&repo(), &revision())?
        .is_some()
    {
        return Err("no generation is activated after the loss".into());
    }
    // Activating what is still there restores service.
    activate(&reopened.store, 3)?;
    if query(&reopened.store, 3)? != vec!["src/g3.rs".to_string()] {
        return Err("activation serves".into());
    }
    Ok(())
}
