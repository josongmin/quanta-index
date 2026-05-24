//! Smoke test for the Tantivy-backed lexical adapter.
//!
//! Builds an index from a sealed batch of `UpsertChunk` ops, opens a searcher,
//! and verifies BM25 scoring + boolean composition both return the expected
//! candidate sets. Follows the `wal_roundtrip.rs` test idiom: returns
//! `Result<(), Box<dyn Error>>` and propagates errors via `?` (no `.unwrap()`
//! or `.expect()` per the workspace lint policy).

#![forbid(unsafe_code)]

use std::error::Error;

use quanta_index_contract::{
    ChunkId, LexicalChannelOp, LqDirectiveSet, LqExpr, LqFilterSet, LqOptionSet, LqQuery,
    ManifestGeneration, RepoId, RevisionId, UpsertChunk,
};
use quanta_index_core::{LexicalIndexBuildPort, LexicalIndexOpenPort};
use quanta_index_lexical::LexicalAdapter;

type TestResult = Result<(), Box<dyn Error>>;

fn repo() -> RepoId {
    RepoId::new("smoke-repo")
}

fn revision() -> RevisionId {
    RevisionId::new("smoke-rev")
}

fn generation() -> ManifestGeneration {
    ManifestGeneration::new(1)
}

fn upsert(chunk_id: &str, text: &str) -> LexicalChannelOp {
    LexicalChannelOp::UpsertChunk(UpsertChunk {
        repo_id: repo(),
        revision_id: revision(),
        generation: generation(),
        chunk_id: ChunkId::new(chunk_id),
        payload: text.as_bytes().to_vec(),
    })
}

fn make_query(expr: LqExpr) -> LqQuery {
    LqQuery {
        expr,
        filters: LqFilterSet::default(),
        options: LqOptionSet::default(),
        directives: LqDirectiveSet::default(),
    }
}

#[test]
fn tantivy_index_round_trip() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());

    let ops = vec![
        upsert("c1", "fox jumps"),
        upsert("c2", "lazy dog"),
        upsert("c3", "fox is quick"),
    ];
    adapter.build(&repo(), &revision(), generation(), &ops)?;

    let searcher = adapter.open(&repo(), &revision(), generation())?;

    // 1) "fox" -> expects c1 and c3 (both contain "fox").
    let fox_hits = searcher.search(&make_query(LqExpr::Raw("fox".to_string())), 10)?;
    if fox_hits.len() != 2 {
        return Err(format!(
            "expected 2 candidates for `fox`, got {}: {:?}",
            fox_hits.len(),
            fox_hits.iter().map(|c| &c.candidate_id).collect::<Vec<_>>()
        )
        .into());
    }
    let mut fox_ids: Vec<String> = fox_hits.iter().map(|c| c.candidate_id.clone()).collect();
    fox_ids.sort();
    if fox_ids != vec!["c1".to_string(), "c3".to_string()] {
        return Err(format!("expected ids [c1, c3] for `fox`, got {fox_ids:?}").into());
    }

    // 2) "lazy" -> expects only c2.
    let lazy_hits = searcher.search(&make_query(LqExpr::Raw("lazy".to_string())), 10)?;
    if lazy_hits.len() != 1 {
        return Err(format!("expected 1 candidate for `lazy`, got {}", lazy_hits.len()).into());
    }
    let first_lazy = lazy_hits
        .first()
        .ok_or("lazy hits empty after length check")?;
    if first_lazy.candidate_id != "c2" {
        return Err(format!("expected id c2 for `lazy`, got {}", first_lazy.candidate_id).into());
    }

    // 3) All([Raw("fox"), Raw("quick")]) -> expects only c3.
    let all_expr = LqExpr::All(vec![
        LqExpr::Raw("fox".to_string()),
        LqExpr::Raw("quick".to_string()),
    ]);
    let all_hits = searcher.search(&make_query(all_expr), 10)?;
    if all_hits.len() != 1 {
        return Err(format!(
            "expected 1 candidate for All([fox, quick]), got {}",
            all_hits.len()
        )
        .into());
    }
    let first_all = all_hits
        .first()
        .ok_or("all hits empty after length check")?;
    if first_all.candidate_id != "c3" {
        return Err(format!(
            "expected id c3 for All([fox, quick]), got {}",
            first_all.candidate_id
        )
        .into());
    }

    Ok(())
}
