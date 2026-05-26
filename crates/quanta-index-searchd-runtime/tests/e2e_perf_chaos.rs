//! E2E-07 — boundedness / fail-closed / observability owner rail.
//!
//! This is not a stopwatch benchmark. The rail exercises the hard runtime
//! shapes that must stay bounded and typed:
//! - regex false positives must be rejected by exact verify,
//! - unsupported regex syntax must fail typed and not poison the next query,
//! - hybrid over-fetch / fuse paths must surface a truthful early-stop reason.

#![forbid(unsafe_code)]

#[path = "common/e2e_harness.rs"]
mod e2e_harness;

use anyhow::Result as AnyResult;
use quanta_index_channel::{BundleChannelPublisher, open_lexical_publisher};
use quanta_index_contract::lex::{
    LanguageCode, ParseNode, ParseTreeRecord, compute_parse_tree_source_hash,
};
use quanta_index_contract::{
    ChunkId, ChunkRecord, DeleteChunk, EarlyStopReason, EngineTouched, GenerationPin,
    LexicalChannelOp, ManifestGeneration, RepoId, RepoRelativePath, RevisionId,
    SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcRequestEnvelope, SearchPlaneQueryIpcResponse,
    SearchPlaneQueryIpcResponseEnvelope, SearchPlaneTrackKind, StructuralQueryRequest,
    TextQueryRequest, TextQuerySyntax, UpsertChunk, UpsertParseTree,
};
use quanta_index_ipc::send_request;
use quanta_index_searchd::{SearchdConfig, drive};
use quanta_index_searchd_runtime::build_runtime;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use crate::e2e_harness::{E2eRuntime, E2eTypedError};

const STRUCTURAL_READINESS_TIMEOUT: Duration = Duration::from_secs(15);
const STRUCTURAL_SOCKET_TIMEOUT: Duration = Duration::from_secs(5);

fn require_no_typed_error(error: Option<E2eTypedError>, context: &str) -> AnyResult<()> {
    if let Some(error) = error {
        return Err(anyhow::anyhow!(
            "{context}: unexpected typed error code={} message={}",
            error.code,
            error.message
        ));
    }
    Ok(())
}

fn seed_regex_fixture(rt: &mut E2eRuntime) -> AnyResult<()> {
    rt.ingest_text("repo-e2e", "src/exact.txt", "needle_x exact")?;
    rt.ingest_text("repo-e2e", "src/bait.txt", "needle_xx bait")?;
    _ = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    Ok(())
}

fn seed_hybrid_count_fixture(rt: &mut E2eRuntime) -> AnyResult<()> {
    rt.ingest_text("repo-e2e", "src/alpha.rs", "scope alpha keep")?;
    rt.ingest_text("repo-e2e", "src/beta.rs", "scope beta keep")?;
    rt.ingest_text("repo-e2e", "src/gamma.rs", "scope gamma keep")?;

    rt.ingest_semantic_embedding_for_path("src/alpha.rs", &[1.0, 0.0])?;
    rt.ingest_semantic_embedding_for_path("src/beta.rs", &[0.9, 0.1])?;
    rt.ingest_semantic_embedding_for_path("src/gamma.rs", &[0.8, 0.2])?;

    _ = rt.seal_tracks(&[
        SearchPlaneTrackKind::Lexical,
        SearchPlaneTrackKind::Semantic,
    ])?;
    rt.activate_last_sealed_generation_with_tracks(&[
        SearchPlaneTrackKind::Lexical,
        SearchPlaneTrackKind::Semantic,
    ])?;
    Ok(())
}

fn seed_hybrid_tie_fixture(rt: &mut E2eRuntime, count: usize) -> AnyResult<()> {
    for idx in 0..count {
        let path = format!("src/tie-{idx:02}.rs");
        rt.ingest_text("repo-e2e", &path, "scope tie keep")?;
        rt.ingest_semantic_embedding_for_path(&path, &[1.0, 0.0])?;
    }
    _ = rt.seal_tracks(&[
        SearchPlaneTrackKind::Lexical,
        SearchPlaneTrackKind::Semantic,
    ])?;
    rt.activate_last_sealed_generation_with_tracks(&[
        SearchPlaneTrackKind::Lexical,
        SearchPlaneTrackKind::Semantic,
    ])?;
    Ok(())
}

fn oversized_raw_substring_query() -> String {
    "abcdefghijklmnopq".repeat(1024)
}

fn structural_repo() -> RepoId {
    RepoId::new("repo-e2e")
}

fn structural_revision() -> RevisionId {
    RevisionId::new("rev-e2e")
}

fn structural_generation() -> ManifestGeneration {
    ManifestGeneration::new(1)
}

fn wait_until(timeout: Duration, mut predicate: impl FnMut() -> bool) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        if predicate() {
            return true;
        }
        thread::sleep(Duration::from_millis(25));
    }
    false
}

#[test]
fn regex_false_positive_candidate_is_rejected_by_exact_verify() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    seed_regex_fixture(&mut rt)?;

    let exact_id = rt.candidate_id_for_path("src/exact.txt")?;
    let bait_id = rt.candidate_id_for_path("src/bait.txt")?;

    let result = rt.query_text(TextQuerySyntax::Native, "/needle_x\\b/", 10);
    require_no_typed_error(result.typed_error, "regex exact-verify query")?;
    if result.candidate_ids != vec![exact_id] {
        return Err(anyhow::anyhow!(
            "expected only exact regex hit, got {:?}",
            result.candidate_ids
        ));
    }
    if result.candidate_ids.contains(&bait_id) {
        return Err(anyhow::anyhow!(
            "regex verify leaked trigram false positive {bait_id}"
        ));
    }
    Ok(())
}

#[test]
fn regex_typed_rejection_does_not_poison_next_query() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    seed_regex_fixture(&mut rt)?;

    let invalid = rt.query_text(TextQuerySyntax::Native, "/(?<=needle_)x/", 10);
    let error = invalid
        .typed_error
        .ok_or_else(|| anyhow::anyhow!("expected typed regex rejection"))?;
    if error.code != "PARSE_FAIL" {
        return Err(anyhow::anyhow!("expected PARSE_FAIL, got {}", error.code));
    }
    if !error.message.contains("regex") {
        return Err(anyhow::anyhow!(
            "regex rejection lost fail-closed regex detail: {}",
            error.message
        ));
    }

    let exact_id = rt.candidate_id_for_path("src/exact.txt")?;
    let follow_up = rt.query_text(TextQuerySyntax::Native, "needle_x", 10);
    require_no_typed_error(
        follow_up.typed_error,
        "follow-up lexical query after regex reject",
    )?;
    if follow_up.candidate_ids != vec![exact_id] {
        return Err(anyhow::anyhow!(
            "follow-up lexical query diverged after regex reject: {:?}",
            follow_up.candidate_ids
        ));
    }
    Ok(())
}

#[test]
fn oversized_raw_substring_query_fails_parse_and_does_not_poison_next_query() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    seed_regex_fixture(&mut rt)?;

    let query = format!("'{}'", oversized_raw_substring_query());
    let limited = rt.query_text(TextQuerySyntax::Native, &query, 10);
    let error = limited.typed_error.ok_or_else(|| {
        anyhow::anyhow!(
            "expected oversized raw-substring parse rejection, observed success ids={:?}",
            limited.candidate_ids
        )
    })?;
    if error.code != "PARSE_FAIL" {
        return Err(anyhow::anyhow!("expected PARSE_FAIL, got {}", error.code));
    }
    if !error.message.contains("16 KiB cap") {
        return Err(anyhow::anyhow!(
            "oversized raw-substring rejection lost parser byte-cap detail: {}",
            error.message
        ));
    }

    let exact_id = rt.candidate_id_for_path("src/exact.txt")?;
    let follow_up = rt.query_text(TextQuerySyntax::Native, "needle_x", 10);
    require_no_typed_error(
        follow_up.typed_error,
        "follow-up lexical query after oversized raw-substring reject",
    )?;
    if follow_up.candidate_ids != vec![exact_id] {
        return Err(anyhow::anyhow!(
            "follow-up lexical query diverged after oversized raw-substring reject: {:?}",
            follow_up.candidate_ids
        ));
    }
    Ok(())
}

#[test]
fn hybrid_count_cap_surfaces_truthful_early_stop_reason() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    seed_hybrid_count_fixture(&mut rt)?;

    let result = rt.query_hybrid(TextQuerySyntax::Native, "scope", &[1.0, 0.0], 2);
    require_no_typed_error(result.typed_error, "hybrid count-cap query")?;
    if result.candidate_ids.len() != 2 {
        return Err(anyhow::anyhow!(
            "expected exactly 2 hybrid results after top_k cap, got {:?}",
            result.candidate_ids
        ));
    }
    let explanation = result
        .explanation
        .ok_or_else(|| anyhow::anyhow!("hybrid count-cap query returned no explanation"))?;
    if explanation.early_stop_reason != Some(EarlyStopReason::CountReached) {
        return Err(anyhow::anyhow!(
            "expected CountReached, got {:?}",
            explanation.early_stop_reason
        ));
    }
    if explanation.engines_touched != vec![EngineTouched::Lexical, EngineTouched::Semantic] {
        return Err(anyhow::anyhow!(
            "unexpected hybrid count-cap engines_touched: {:?}",
            explanation.engines_touched
        ));
    }
    if explanation.summary != "hybrid fused 3 lexical and 3 semantic candidates into 2 results" {
        return Err(anyhow::anyhow!(
            "hybrid count-cap summary missing fused-count detail: {}",
            explanation.summary
        ));
    }
    Ok(())
}

#[test]
fn hybrid_large_tied_result_set_keeps_order_stable() -> AnyResult<()> {
    let mut rt = E2eRuntime::boot()?;
    seed_hybrid_tie_fixture(&mut rt, 24)?;
    let mut baseline: Option<Vec<String>> = None;

    for run in 0..5 {
        let result = rt.query_hybrid(TextQuerySyntax::Native, "scope", &[1.0, 0.0], 10);
        require_no_typed_error(result.typed_error, "hybrid tied-order query")?;
        if result.candidate_ids.len() != 10 {
            return Err(anyhow::anyhow!(
                "hybrid tied-order query returned {} results on run {run}, expected 10",
                result.candidate_ids.len()
            ));
        }
        let explanation = result
            .explanation
            .ok_or_else(|| anyhow::anyhow!("hybrid tied-order query returned no explanation"))?;
        if explanation.early_stop_reason != Some(EarlyStopReason::CountReached) {
            return Err(anyhow::anyhow!(
                "expected CountReached on hybrid tied-order query, got {:?}",
                explanation.early_stop_reason
            ));
        }
        match baseline.as_ref() {
            Some(previous) if previous != &result.candidate_ids => {
                return Err(anyhow::anyhow!(
                    "hybrid tied-order query drifted across runs: baseline={previous:?} current={:?}",
                    result.candidate_ids
                ));
            }
            None => baseline = Some(result.candidate_ids),
            Some(_) => {}
        }
    }
    Ok(())
}

#[test]
fn structural_orphan_chunk_authority_fails_typed_generation_not_ready() -> AnyResult<()> {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();
    let content = "fn orphaned() {}";
    {
        let publisher = open_lexical_publisher(state_root)?;
        let chunk_id = ChunkId::new("chunk-tree");
        let chunk = ChunkRecord {
            chunk_id: chunk_id.clone(),
            repo_relative_path: RepoRelativePath::new("src/tree.rs"),
            language: LanguageCode::new("rust")
                .map_err(|err| anyhow::anyhow!("invalid structural test language: {err}"))?,
            start_byte: 0,
            end_byte: u32::try_from(content.len())
                .map_err(|err| anyhow::anyhow!("structural test content overflow: {err}"))?,
            start_line: 1,
            end_line: 1,
            snippet: content.to_string().into_boxed_str(),
            indexed_text: content.to_string().into_boxed_str(),
            text_digest: "text:tree".to_string().into_boxed_str(),
            shape_digest: "shape:tree".to_string().into_boxed_str(),
            structural: None,
            parent_chunk_id: None,
        };
        let mut chunk_payload = Vec::new();
        ciborium::into_writer(&chunk, &mut chunk_payload)?;
        let _seq = publisher.publish(LexicalChannelOp::UpsertChunk(UpsertChunk {
            repo_id: structural_repo(),
            revision_id: structural_revision(),
            generation: structural_generation(),
            chunk_id: chunk_id.clone(),
            payload: chunk_payload,
        }))?;
        let tree = ParseTreeRecord {
            wire_version: 1,
            lang: LanguageCode::new("rust")
                .map_err(|err| anyhow::anyhow!("invalid structural test language: {err}"))?,
            root: ParseNode {
                kind: "function_item".to_string().into_boxed_str(),
                byte_start: 0,
                byte_end: u32::try_from(content.len())
                    .map_err(|err| anyhow::anyhow!("structural test content overflow: {err}"))?,
                children: vec![ParseNode {
                    kind: "identifier".to_string().into_boxed_str(),
                    byte_start: 3,
                    byte_end: 11,
                    children: Vec::new(),
                }],
            },
            source_hash: compute_parse_tree_source_hash(content),
            role_tag_schema_version: 1,
            role_tags: Vec::new(),
        };
        let mut tree_payload = Vec::new();
        ciborium::into_writer(&tree, &mut tree_payload)?;
        let _seq = publisher.publish(LexicalChannelOp::UpsertParseTree(UpsertParseTree {
            repo_id: structural_repo(),
            revision_id: structural_revision(),
            generation: structural_generation(),
            chunk_id: chunk_id.clone(),
            payload: tree_payload,
        }))?;
        let _seq = publisher.publish(LexicalChannelOp::DeleteChunk(DeleteChunk {
            repo_id: structural_repo(),
            revision_id: structural_revision(),
            generation: structural_generation(),
            chunk_id,
        }))?;
        _ = publisher.seal(
            structural_repo(),
            structural_revision(),
            structural_generation(),
        )?;
    }

    let runtime = build_runtime(SearchdConfig::from_state_root(state_root.to_path_buf()))?;
    let socket = runtime.query_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("e2e-perf-chaos-structural-shard".into())
        .spawn(move || drive(runtime, shutdown_for_drive))?;
    if !wait_until(STRUCTURAL_SOCKET_TIMEOUT, || socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(anyhow::anyhow!(
            "structural shard-unavailable socket never appeared"
        ));
    }

    let request = SearchPlaneQueryIpcRequestEnvelope {
        request_id: 77,
        payload: SearchPlaneQueryIpcRequest::Structural(StructuralQueryRequest {
            text_query: TextQueryRequest {
                syntax: TextQuerySyntax::Native,
                query_text: "match { :[x] }".to_string(),
                generation: Some(GenerationPin::new(
                    structural_repo(),
                    structural_revision(),
                    structural_generation(),
                )),
                generation_selector: None,
                top_k: 10,
            },
        }),
    };
    let mut observed: Option<String> = None;
    let saw_expected = wait_until(STRUCTURAL_READINESS_TIMEOUT, || {
        match send_request::<_, SearchPlaneQueryIpcResponseEnvelope>(&socket, &request) {
            Ok(response) => match response.payload {
                SearchPlaneQueryIpcResponse::Error(err) => {
                    observed = Some(err.code.clone());
                    err.code == "STR_GENERATION_NOT_READY"
                }
                other @ (SearchPlaneQueryIpcResponse::Text(_)
                | SearchPlaneQueryIpcResponse::Symbol(_)
                | SearchPlaneQueryIpcResponse::Semantic(_)
                | SearchPlaneQueryIpcResponse::Hybrid(_)
                | SearchPlaneQueryIpcResponse::History(_)
                | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
                | SearchPlaneQueryIpcResponse::Structural(_)
                | SearchPlaneQueryIpcResponse::Bridge(_)
                | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
                | SearchPlaneQueryIpcResponse::Explain(_)) => {
                    observed = Some(format!("{other:?}"));
                    false
                }
            },
            Err(err) => {
                observed = Some(err.to_string());
                false
            }
        }
    });
    shutdown.store(true, Ordering::Release);
    let join_result = match join.join() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(err)) => Err(err),
        Err(panic) => Err(anyhow::anyhow!("driver panic: {panic:?}")),
    };
    if !saw_expected {
        join_result?;
        return Err(anyhow::anyhow!(
            "expected STR_GENERATION_NOT_READY after orphaning structural chunk authority, observed {observed:?}"
        ));
    }
    join_result?;
    Ok(())
}
