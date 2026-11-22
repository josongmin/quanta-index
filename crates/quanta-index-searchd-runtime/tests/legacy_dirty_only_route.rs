//! Pinned old-Q runtime route and restart regressions for Runtime Catalog and Dirty.

#![forbid(unsafe_code)]

use std::error::Error;
use std::time::Duration;

use quanta_index_contract::{
    ChunkId, ManifestGeneration, RepoId, RevisionId, RuntimeCatalogIngestBatch,
    RuntimeChangedRecord, RuntimeDocFacetRecord, RuntimeEdgeAuthorityRecord, RuntimeSnapshotRecord,
    lex::DirtyRecord,
};
use quanta_index_sdk::{DirtyBatch, GenerationPin, PublishedBatchEvidence, QuantaIndex};
use quanta_index_searchd_harness::legacy_reference::{
    LegacySearchdBinary, legacy_source_batch, publish_legacy_source, publish_legacy_source_batch,
    request_when_ready,
};

use crate::fail_closed_wait::{RealTicker, wait_for};

type TestResult = Result<(), Box<dyn Error>>;

const DIRTY_PATH: &str = "src/dirty.rs";
const DIRTY_ID: &str = "e2e-1-src/dirty.rs";

fn paths(
    client: &QuantaIndex,
    pin: &GenerationPin,
    sourcegraph: bool,
) -> Result<Vec<String>, Box<dyn Error>> {
    let query = client.runtime().query();
    let response = if sourcegraph {
        query
            .sourcegraph("dirty:only todo")
            .pinned(pin.clone())
            .top_k(10)
            .execute()?
    } else {
        query
            .native("dirty:yes todo")
            .pinned(pin.clone())
            .top_k(10)
            .execute()?
    };
    if response.generation != *pin {
        return Err(format!("runtime query changed selected generation: {response:?}").into());
    }
    let mut paths = response
        .results
        .iter()
        .map(|candidate| candidate.repo_relative_path.as_str().to_owned())
        .collect::<Vec<_>>();
    paths.sort();
    Ok(paths)
}

#[test]
fn sourcegraph_dirty_only_does_not_poison_follow_up_native_dirty_query() -> TestResult {
    let parent = quanta_index_searchd_harness::private_tempdir()?;
    let state_root = parent.path().join("state");
    let binary = LegacySearchdBinary::from_environment()?;
    let process = binary.start(&state_root, 8)?;
    let outcome = (|| -> TestResult {
        let client = process.connect()?;
        let repo = RepoId::new("repo-e2e")?;
        let revision = RevisionId::new("rev-e2e")?;
        let (_active, _receipt) = publish_legacy_source(
            &client,
            &repo,
            &revision,
            1,
            None,
            &[
                (DIRTY_PATH, "todo dirty scope", 1),
                ("src/clean.rs", "todo clean scope", 2),
            ],
            None,
        )?;
        let pin = GenerationPin::new(repo.clone(), revision.clone(), ManifestGeneration::new(1));
        let receipt = client.runtime().publish_dirty(
            &DirtyBatch::new(repo, revision, pin.manifest_generation, 100).upsert(DirtyRecord {
                wire_version: 1,
                doc_id: ChunkId::new(DIRTY_ID),
                applied_at_ms: 100,
                payload_hash: [0x5a; 32],
            }),
        )?;
        if !receipt.applied || receipt.durable_sequence == 0 {
            return Err("dirty fixture was not durably published".into());
        }
        let _ready = wait_for(
            &RealTicker::new(),
            Duration::from_secs(20),
            Duration::from_millis(50),
            "pinned old-Q runtime query backend ready",
            || client.observability().process_readiness(),
            |report| report.ready,
            |_| true,
        )?;
        let dirty_only = paths(&client, &pin, true)?;
        if dirty_only != vec![DIRTY_PATH.to_owned()] {
            return Err(format!("dirty:only must isolate the dirty doc: {dirty_only:?}").into());
        }
        let follow_up = paths(&client, &pin, false)?;
        if follow_up != vec![DIRTY_PATH.to_owned()] {
            return Err(format!("dirty:only poisoned the next Native query: {follow_up:?}").into());
        }
        Ok(())
    })();
    let stopped = process.stop();
    outcome.and(stopped)?;
    binary.verify()
}

const RUNTIME_REPLAY_FILES: &[(&str, &str, u64)] = &[
    ("src/changed.rs", "fn catalog_changed_needle() {}", 1),
    ("src/changed-other.rs", "fn catalog_changed_needle() {}", 2),
    ("src/unchanged.rs", "fn catalog_unchanged_needle() {}", 3),
    ("src/owner.rs", "fn catalog_owner_needle() {}", 4),
    ("src/service.rs", "fn catalog_service_needle() {}", 5),
    ("src/layer.rs", "fn catalog_layer_needle() {}", 6),
    ("src/surface.rs", "fn catalog_surface_needle() {}", 7),
    ("src/snap.rs", "fn catalog_snapshot_needle() {}", 8),
    ("src/stale.rs", "fn catalog_stale_needle() {}", 9),
];

fn runtime_pin(generation: u64) -> Result<GenerationPin, Box<dyn Error>> {
    Ok(GenerationPin::new(
        RepoId::new("repo-e2e")?,
        RevisionId::new("rev-e2e")?,
        ManifestGeneration::new(generation),
    ))
}

fn runtime_ids(
    client: &QuantaIndex,
    pin: &GenerationPin,
    sourcegraph: bool,
    query: &str,
) -> Result<Vec<String>, Box<dyn Error>> {
    let response = request_when_ready(|| {
        let builder = client.runtime().query();
        let request = if sourcegraph {
            builder.sourcegraph(query)
        } else {
            builder.native(query)
        };
        request.pinned(pin.clone()).top_k(10).execute()
    })?;
    if response.generation != *pin {
        return Err(format!("Runtime replay selected a different generation: {response:?}").into());
    }
    Ok(response
        .results
        .into_iter()
        .map(|row| row.candidate_id)
        .collect())
}

fn require_runtime_replay(
    pin: &GenerationPin,
    query: &str,
    sourcegraph: bool,
    expected: &[&str],
    publish: impl FnOnce(&QuantaIndex) -> TestResult,
) -> TestResult {
    let root = quanta_index_searchd_harness::private_tempdir()?;
    let binary = LegacySearchdBinary::from_environment()?;
    let first = binary.start(root.path(), 8)?;
    let client = first.connect()?;
    publish(&client)?;
    let before = runtime_ids(&client, pin, sourcegraph, query)?;
    if !before
        .iter()
        .map(String::as_str)
        .eq(expected.iter().copied())
    {
        return Err(
            format!("{query}: expected {expected:?} before restart, got {before:?}").into(),
        );
    }
    drop(client);
    first.stop()?;
    binary.verify()?;

    let reopened = binary.start(root.path(), 8)?;
    let client = reopened.connect()?;
    let after = runtime_ids(&client, pin, sourcegraph, query)?;
    if after != before {
        return Err(format!("{query}: restart changed rows from {before:?} to {after:?}").into());
    }
    drop(client);
    reopened.stop()?;
    binary.verify()
}

fn publish_runtime_catalog(client: &QuantaIndex, pin: &GenerationPin) -> TestResult {
    let source = legacy_source_batch(
        &pin.repo_id,
        &pin.revision_id,
        pin.manifest_generation.get(),
        None,
        RUNTIME_REPLAY_FILES,
    )?;
    let source = client.search_corpus().publish_outcome(&source)?;
    if !source.receipt.applied || !source.receipt.sealed || source.receipt.durable_sequence == 0 {
        return Err(format!(
            "Runtime Catalog fixture did not seal Source: {:?}",
            source.receipt
        )
        .into());
    }
    let changed = || ChunkId::new("e2e-1-src/changed.rs");
    let edge = || RuntimeEdgeAuthorityRecord {
        key: "rebuild=lexical".into(),
        doc_ids: vec![changed()],
    };
    let receipt = client
        .runtime()
        .publish_catalog(RuntimeCatalogIngestBatch {
            repo_id: pin.repo_id.clone(),
            revision_id: pin.revision_id.clone(),
            generation: pin.manifest_generation,
            overlay_epoch_ms: 20,
            batch_digest: String::new(),
            producer_head_applied_at_ms: 100,
            generation_materialized_at_ms: 20,
            changed_entries: vec![
                RuntimeChangedRecord {
                    doc_id: changed(),
                    applied_at_ms: 25,
                    payload_hash: [0xaa; 32],
                },
                RuntimeChangedRecord {
                    doc_id: ChunkId::new("e2e-9-src/stale.rs"),
                    applied_at_ms: 15,
                    payload_hash: [0xaa; 32],
                },
            ],
            facet_entries: [
                "e2e-4-src/owner.rs",
                "e2e-5-src/service.rs",
                "e2e-6-src/layer.rs",
                "e2e-7-src/surface.rs",
            ]
            .into_iter()
            .map(|id| RuntimeDocFacetRecord {
                doc_id: ChunkId::new(id),
                owner: Some("team-a".into()),
                service: Some("search".into()),
                layer: Some("index".into()),
                surface: Some("lexical".into()),
            })
            .collect(),
            snapshot_entries: vec![RuntimeSnapshotRecord {
                name: "active".into(),
                doc_ids: vec![changed(), ChunkId::new("e2e-8-src/snap.rs")],
            }],
            affected_entries: vec![edge()],
            invalidated_by_entries: vec![edge()],
        })?;
    if receipt.generation != pin.manifest_generation
        || !receipt.applied
        || receipt.durable_sequence == 0
    {
        return Err(format!(
            "Runtime Catalog did not commit to the source generation: {receipt:?}"
        )
        .into());
    }
    let _activation = client
        .search_corpus()
        .activate_published(&PublishedBatchEvidence::from(&source), None)?;
    Ok(())
}

fn require_catalog_replay(query: &str, id: &str) -> TestResult {
    let pin = runtime_pin(1)?;
    require_runtime_replay(&pin, query, true, &[id], |client| {
        publish_runtime_catalog(client, &pin)
    })
}

#[test]
fn reopen_preserves_runtime_catalog_changed_query() -> TestResult {
    require_catalog_replay(
        "changed:since=1970-01-01T00:00:00.010Z file:src/changed.rs catalog_changed_needle",
        "e2e-1-src/changed.rs",
    )
}

#[test]
fn reopen_preserves_runtime_catalog_invalidated_by_query() -> TestResult {
    require_catalog_replay(
        "invalidated_by:rebuild=lexical catalog_changed_needle",
        "e2e-1-src/changed.rs",
    )
}

#[test]
fn reopen_preserves_runtime_catalog_affected_query() -> TestResult {
    require_catalog_replay(
        "affected:rebuild=lexical catalog_changed_needle",
        "e2e-1-src/changed.rs",
    )
}

#[test]
fn reopen_preserves_runtime_catalog_stale_query() -> TestResult {
    require_catalog_replay(
        "stale:before=1970-01-01T00:00:00.030Z file:src/stale.rs catalog_stale_needle",
        "e2e-9-src/stale.rs",
    )
}

#[test]
fn reopen_preserves_runtime_catalog_stale_miss_and_follow_up() -> TestResult {
    const MISS: &str =
        "stale:before=1970-01-01T00:00:00.010Z file:src/stale.rs catalog_stale_needle";
    const HIT: &str =
        "stale:before=1970-01-01T00:00:00.030Z file:src/stale.rs catalog_stale_needle";
    const ID: &str = "e2e-9-src/stale.rs";

    let pin = runtime_pin(1)?;
    let root = quanta_index_searchd_harness::private_tempdir()?;
    let binary = LegacySearchdBinary::from_environment()?;
    let first = binary.start(root.path(), 8)?;
    let client = first.connect()?;
    publish_runtime_catalog(&client, &pin)?;
    let miss = runtime_ids(&client, &pin, true, MISS)?;
    if !miss.is_empty() {
        return Err(format!("Runtime stale miss returned candidates: {miss:?}").into());
    }
    let hit = runtime_ids(&client, &pin, true, HIT)?;
    if hit != [ID] {
        return Err(format!("Runtime query after stale miss diverged: {hit:?}").into());
    }
    drop(client);
    first.stop()?;
    binary.verify()?;

    let reopened = binary.start(root.path(), 8)?;
    let client = reopened.connect()?;
    let replayed_miss = runtime_ids(&client, &pin, true, MISS)?;
    let replayed_hit = runtime_ids(&client, &pin, true, HIT)?;
    if !replayed_miss.is_empty() || replayed_hit != [ID] {
        return Err(format!(
            "Runtime stale miss/follow-up changed after restart: miss={replayed_miss:?} hit={replayed_hit:?}"
        )
        .into());
    }
    drop(client);
    reopened.stop()?;
    binary.verify()
}

#[test]
fn reopen_preserves_runtime_catalog_snapshot_query() -> TestResult {
    require_catalog_replay(
        "snapshot:active file:src/snap.rs catalog_snapshot_needle",
        "e2e-8-src/snap.rs",
    )
}

#[test]
fn reopen_preserves_runtime_catalog_meta_owner_query() -> TestResult {
    require_catalog_replay(
        "meta.owner:team-a catalog_owner_needle",
        "e2e-4-src/owner.rs",
    )
}

#[test]
fn reopen_preserves_runtime_catalog_meta_service_query() -> TestResult {
    require_catalog_replay(
        "meta.service:search catalog_service_needle",
        "e2e-5-src/service.rs",
    )
}

#[test]
fn reopen_preserves_runtime_catalog_meta_layer_query() -> TestResult {
    require_catalog_replay(
        "meta.layer:index catalog_layer_needle",
        "e2e-6-src/layer.rs",
    )
}

#[test]
fn reopen_preserves_runtime_catalog_meta_surface_query() -> TestResult {
    require_catalog_replay(
        "meta.surface:lexical catalog_surface_needle",
        "e2e-7-src/surface.rs",
    )
}

fn upsert_dirty(client: &QuantaIndex, pin: &GenerationPin, id: &str, timestamp: u64) -> TestResult {
    let receipt = client.runtime().publish_dirty(
        &DirtyBatch::new(
            pin.repo_id.clone(),
            pin.revision_id.clone(),
            pin.manifest_generation,
            timestamp,
        )
        .upsert(DirtyRecord {
            wire_version: 1,
            doc_id: ChunkId::new(id),
            applied_at_ms: timestamp,
            payload_hash: [0x5a; 32],
        }),
    )?;
    if receipt.generation != pin.manifest_generation
        || !receipt.applied
        || receipt.durable_sequence == 0
    {
        return Err(format!("Dirty overlay was not applied: {receipt:?}").into());
    }
    Ok(())
}

#[test]
fn reopen_preserves_runtime_dirty_no_clean_complement() -> TestResult {
    let pin = runtime_pin(1)?;
    require_runtime_replay(
        &pin,
        "dirty:no clean",
        false,
        &["e2e-2-src/clean.rs"],
        |client| {
            let source = legacy_source_batch(
                &pin.repo_id,
                &pin.revision_id,
                1,
                None,
                &[
                    (DIRTY_PATH, "todo dirty scope", 1),
                    ("src/clean.rs", "todo clean scope", 2),
                ],
            )?;
            upsert_dirty(client, &pin, DIRTY_ID, 100)?;
            let (_head, _receipt) = publish_legacy_source_batch(client, &source, None)?;
            Ok(())
        },
    )
}

#[test]
fn reopen_preserves_runtime_dirty_evict_empty_state() -> TestResult {
    const BODY: &str = "todo dirty scope";
    const QUERY: &str = "dirty:yes todo";
    const NEXT_ID: &str = "e2e-2-src/dirty.rs";
    let g1 = runtime_pin(1)?;
    let g2 = runtime_pin(2)?;
    require_runtime_replay(&g2, QUERY, false, &[], |client| {
        upsert_dirty(client, &g1, DIRTY_ID, 100)?;
        let (active, _receipt) = publish_legacy_source(
            client,
            &g1.repo_id,
            &g1.revision_id,
            1,
            None,
            &[(DIRTY_PATH, BODY, 1)],
            None,
        )?;
        let before = runtime_ids(client, &g1, false, QUERY)?;
        if before != [DIRTY_ID] {
            return Err(
                format!("G1 must contain one dirty candidate before eviction: {before:?}").into(),
            );
        }
        upsert_dirty(client, &g2, NEXT_ID, 200)?;
        let evicted = client.runtime().publish_dirty(
            &DirtyBatch::new(
                g2.repo_id.clone(),
                g2.revision_id.clone(),
                g2.manifest_generation,
                0,
            )
            .delete(ChunkId::new(NEXT_ID)),
        )?;
        if evicted.generation != g2.manifest_generation
            || !evicted.applied
            || evicted.accepted_tombstone_scopes != 1
            || evicted.durable_sequence == 0
        {
            return Err(format!("G2 did not commit the eviction: {evicted:?}").into());
        }
        let (_head, _receipt) = publish_legacy_source(
            client,
            &g2.repo_id,
            &g2.revision_id,
            2,
            Some((DIRTY_PATH, BODY, 1)),
            &[(DIRTY_PATH, BODY, 2)],
            Some(active),
        )?;
        Ok(())
    })
}
