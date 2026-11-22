//! E2E-05 History and Structural restart/replay over the pinned Legacy daemon.

#![forbid(unsafe_code)]

use std::{error::Error, path::Path};

use quanta_index_contract::{
    BatchIngestMode, BatchPublishReceipt, ChunkId, GenerationPin, HistoryIngestBatch,
    HistoryOrderV1, HistoryRefDelete, HistoryRefMutation, ManifestGeneration, RepoId,
    RepoRelativePath, RevisionId, SearchCorpusActiveHeadV1, SearchPlaneErrorCodeV2,
    SearchPlaneIngestIpcRequest, SearchPlaneIngestIpcRequestEnvelope, SearchPlaneIngestIpcResponse,
    SearchPlaneIngestIpcResponseEnvelope, SearchPlaneStructuralQueryResponse, SearchScopeKey,
    SearchScopeSurface, SourceFileKey, SourcePublicationEvent, StructuralIngestBatch,
    StructuralReplaceScope, StructuralTombstoneScope, StructuralTreeRecord, TextQuerySyntax,
};
use quanta_index_ipc::{ClientIoPolicy, send_request};
use quanta_index_sdk::{PublishedBatchEvidence, QuantaIndex, SdkError, SearchCorpusBatch};
use quanta_index_searchd_harness::{
    history_fixture::{HISTORY_LEDGER, HistoryFixtureSpec},
    legacy_reference::{
        LegacySearchdBinary, daemon_socket_paths, legacy_source_batch, publish_legacy_source_batch,
        request_when_ready,
    },
    private_tempdir, stamped_ingest_request, structural_function_tree_record,
};

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

const TREE_PATH: &str = "src/structural.rs";
const TREE_ID: &str = "e2e-1-src/structural.rs";
const TYPED_HOLE: &str = "match { function_item { { :[name.expr] } } }";
const TREE_NAME: &str = "restart_structural_alpha";
const TREE_TEXT: &str = "fn restart_structural_alpha() {}";

fn pin(generation: u64) -> TestResult<GenerationPin> {
    Ok(GenerationPin::new(
        RepoId::new("repo-e2e")?,
        RevisionId::new("rev-e2e")?,
        ManifestGeneration::new(generation),
    ))
}

fn with_daemon<T>(
    binary: &LegacySearchdBinary,
    root: &Path,
    run: impl FnOnce(&QuantaIndex) -> TestResult<T>,
) -> TestResult<T> {
    let process = binary.start(root, 8)?;
    let outcome = process.connect().and_then(|client| run(&client));
    let stopped = process.stop();
    let value = outcome?;
    stopped?;
    binary.verify()?;
    Ok(value)
}

fn send_auxiliary(
    root: &Path,
    payload: SearchPlaneIngestIpcRequest,
) -> TestResult<SearchPlaneIngestIpcResponse> {
    let [_, _, ingest] = daemon_socket_paths(root);
    let request = SearchPlaneIngestIpcRequestEnvelope {
        request_id: 1,
        diagnostic_attempt: quanta_index_contract::ipc::DiagnosticAttemptFieldV1::Absent,
        payload: stamped_ingest_request(payload)?,
    };
    let reply: SearchPlaneIngestIpcResponseEnvelope =
        send_request(&ingest, &request, ClientIoPolicy::default())?;
    Ok(reply.payload)
}

fn publish_history(root: &Path, batch: HistoryIngestBatch) -> TestResult<BatchPublishReceipt> {
    let generation = batch.generation;
    let response = send_auxiliary(
        root,
        SearchPlaneIngestIpcRequest::PublishHistoryBatch(batch),
    )?;
    match response {
        SearchPlaneIngestIpcResponse::HistoryReceipt(receipt)
            if receipt.applied
                && receipt.durable_sequence > 0
                && receipt.generation == generation =>
        {
            Ok(receipt)
        }
        other => Err(format!(
            "expected a durable History receipt for g{}: {other:?}",
            generation.get()
        )
        .into()),
    }
}

fn publish_structural(
    root: &Path,
    pin: &GenerationPin,
    replace_scopes: Vec<StructuralReplaceScope>,
    tombstone_scopes: Vec<StructuralTombstoneScope>,
) -> TestResult {
    let batch_name = if tombstone_scopes.is_empty() {
        "struct"
    } else {
        "struct-del"
    };
    let response = send_auxiliary(
        root,
        SearchPlaneIngestIpcRequest::PublishStructuralBatch(StructuralIngestBatch {
            repo_id: pin.repo_id.clone(),
            revision_id: pin.revision_id.clone(),
            generation: pin.manifest_generation,
            base_generation: None,
            manifest_digest: format!("{batch_name}:{TREE_PATH}:{}", pin.manifest_generation.get()),
            batch_digest: String::new(),
            mode: BatchIngestMode::Delta,
            replace_scopes,
            tombstone_scopes,
            seal: false,
        }),
    )?;
    if !matches!(&response, SearchPlaneIngestIpcResponse::StructuralReceipt(receipt)
        if receipt.applied && receipt.durable_sequence > 0 && receipt.generation == pin.manifest_generation)
    {
        return Err(format!("expected a durable Structural receipt: {response:?}").into());
    }
    Ok(())
}

fn tree_scope(pin: &GenerationPin) -> SearchScopeKey {
    SearchScopeKey {
        doc_surface: SearchScopeSurface::Chunk,
        source_file: SourceFileKey {
            source_repo_id: pin.repo_id.clone(),
            repo_relative_path: RepoRelativePath::new(TREE_PATH),
        },
    }
}

fn publish_tree(
    root: &Path,
    pin: &GenerationPin,
    id: &str,
    identifier: &str,
    body: &str,
) -> TestResult {
    publish_structural(
        root,
        pin,
        vec![StructuralReplaceScope {
            scope: tree_scope(pin),
            scope_digest: format!("struct-scope:{TREE_PATH}:{}", pin.manifest_generation.get()),
            trees: vec![StructuralTreeRecord {
                chunk_id: ChunkId::new(id),
                record: structural_function_tree_record(TREE_PATH, body, identifier)?,
            }],
        }],
        Vec::new(),
    )
}

fn seed_tree(
    client: &QuantaIndex,
    root: &Path,
    pin: &GenerationPin,
    identifier: &str,
    body: &str,
) -> TestResult<SearchCorpusActiveHeadV1> {
    let source = legacy_source_batch(
        &pin.repo_id,
        &pin.revision_id,
        1,
        None,
        &[(TREE_PATH, body, 1)],
    )?;
    let published = client.search_corpus().publish_outcome(&source)?;
    publish_tree(root, pin, TREE_ID, identifier, body)?;
    let ack = client
        .search_corpus()
        .activate_published(&PublishedBatchEvidence::from(&published), None)?;
    Ok(ack.active)
}

fn structural_query(
    client: &QuantaIndex,
    pin: &GenerationPin,
    syntax: TextQuerySyntax,
    query: &str,
) -> Result<SearchPlaneStructuralQueryResponse, SdkError> {
    let builder = client.structural().query();
    let request = match syntax {
        TextQuerySyntax::Native => builder.native(query),
        TextQuerySyntax::Sourcegraph => builder.sourcegraph(query),
        TextQuerySyntax::CodeSearch => {
            return Err(SdkError::Usage("expected Structural DSL syntax".into()));
        }
    };
    request.pinned(pin.clone()).top_k(10).execute()
}

fn require_tree_rows(
    client: &QuantaIndex,
    pin: &GenerationPin,
    queries: &[(TextQuerySyntax, &str)],
) -> TestResult {
    for &(syntax, query) in queries {
        let answer = request_when_ready(|| structural_query(client, pin, syntax, query))?;
        let [row] = answer.results.as_slice() else {
            return Err(format!("{query}: expected one Structural result, got {answer:?}").into());
        };
        if answer.generation != *pin || row.candidate_id.as_str() != TREE_ID {
            return Err(format!("{query}: expected {TREE_ID} at {pin:?}, got {answer:?}").into());
        }
    }
    Ok(())
}

fn structural_restart(queries: &[(TextQuerySyntax, &str)]) -> TestResult {
    let root = private_tempdir()?;
    let binary = LegacySearchdBinary::from_environment()?;
    let pin = pin(1)?;
    with_daemon(&binary, root.path(), |client| {
        let _active = seed_tree(client, root.path(), &pin, TREE_NAME, TREE_TEXT)?;
        require_tree_rows(client, &pin, queries)
    })?;
    with_daemon(&binary, root.path(), |client| {
        require_tree_rows(client, &pin, queries)
    })
}

#[test]
fn reopen_preserves_structural_typed_hole_and_sourcegraph_boolean_ids() -> TestResult {
    structural_restart(&[
        (TextQuerySyntax::Native, TYPED_HOLE),
        (
            TextQuerySyntax::Sourcegraph,
            r#"patterntype:structural "function_item { { :[name.expr] } }" AND NOT "trait_item""#,
        ),
    ])
}

#[test]
fn fresh_reingest_replays_equivalent_structural_native_and_sourcegraph_ids() -> TestResult {
    let binary = LegacySearchdBinary::from_environment()?;
    let pin = pin(1)?;
    let queries = [
        (
            TextQuerySyntax::Native,
            "match { function_item { { :[name.expr] } } } OR match { trait_item }",
        ),
        (
            TextQuerySyntax::Sourcegraph,
            r#"patterntype:structural "function_item { { :[name.expr] } }" OR patterntype:structural "trait_item""#,
        ),
    ];
    for _ in 0..2 {
        let root = private_tempdir()?;
        with_daemon(&binary, root.path(), |client| {
            let _active = seed_tree(client, root.path(), &pin, TREE_NAME, TREE_TEXT)?;
            require_tree_rows(client, &pin, &queries)
        })?;
    }
    Ok(())
}

#[test]
fn reopen_preserves_structural_mixed_lexical_boolean_ids() -> TestResult {
    structural_restart(&[(
        TextQuerySyntax::Native,
        "restart_structural_alpha AND match { function_item }",
    )])
}

#[test]
fn reopen_preserves_structural_tombstone_not_ready_state() -> TestResult {
    const BODY: &str = "fn restart_structural_tombstone() {}";
    const IDENTIFIER: &str = "restart_structural_tombstone";
    let root = private_tempdir()?;
    let binary = LegacySearchdBinary::from_environment()?;
    let g1 = pin(1)?;
    let g2 = pin(2)?;
    let require_refusal = |client: &QuantaIndex| -> TestResult {
        let refused = structural_query(client, &g2, TextQuerySyntax::Native, TYPED_HOLE);
        if !matches!(
            &refused,
            Err(SdkError::Remote {
                code: SearchPlaneErrorCodeV2::StrGenerationNotReady,
                ..
            })
        ) {
            return Err(format!(
                "G2 Structural tombstone did not refuse as not ready: {refused:?}"
            )
            .into());
        }
        Ok(())
    };
    with_daemon(&binary, root.path(), |client| {
        let _active = seed_tree(client, root.path(), &g1, IDENTIFIER, BODY)?;
        require_tree_rows(client, &g1, &[(TextQuerySyntax::Native, TYPED_HOLE)])?;
        let delta = legacy_source_batch(
            &g2.repo_id,
            &g2.revision_id,
            2,
            Some((TREE_PATH, BODY, 1)),
            &[(TREE_PATH, BODY, 2)],
        )?;
        let source = client.search_corpus().publish(&delta)?;
        if !source.sealed || !source.applied || source.durable_sequence == 0 {
            return Err(format!("G2 Source did not seal: {source:?}").into());
        }
        publish_tree(
            root.path(),
            &g2,
            "e2e-2-src/structural.rs",
            IDENTIFIER,
            BODY,
        )?;
        publish_structural(
            root.path(),
            &g2,
            Vec::new(),
            vec![StructuralTombstoneScope {
                scope: tree_scope(&g2),
            }],
        )?;
        // Keep this published generation pinned and unactivated, as in the
        // original replay case. The Structural tombstone must be durable.
        require_refusal(client)
    })?;
    with_daemon(&binary, root.path(), require_refusal)
}

fn require_history_ids(
    client: &QuantaIndex,
    pin: &GenerationPin,
    query: &str,
    expected: &[&str],
) -> TestResult {
    let answer = request_when_ready(|| {
        client
            .history()
            .query()
            .sourcegraph(query)
            .pinned(pin.clone())
            .order(HistoryOrderV1::Recency)
            .top_k(10)
            .execute()
    })?;
    let ids: Vec<_> = answer
        .commits
        .iter()
        .map(|candidate| candidate.sha.to_hex())
        .collect();
    if answer.generation != *pin || !ids.iter().map(String::as_str).eq(expected.iter().copied()) {
        return Err(format!("{query}: expected {expected:?} at {pin:?}, got {answer:?}").into());
    }
    Ok(())
}

#[test]
fn reopen_preserves_history_rev_delete_state_without_dropping_commit_matches() -> TestResult {
    const FILE: &str = "src/history.rs";
    const REF: &str = "type:commit rev:refs/heads/main fix";
    const TAG: &str = "type:commit rev:v1.0.0 fix";
    const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";
    let root = private_tempdir()?;
    let binary = LegacySearchdBinary::from_environment()?;
    let g1 = pin(1)?;
    let g2 = pin(2)?;
    let require_tombstones = |client: &QuantaIndex| -> TestResult {
        require_history_ids(client, &g2, "type:commit fix", &[COMMIT])?;
        require_history_ids(client, &g2, REF, &[])?;
        require_history_ids(client, &g2, TAG, &[])
    };
    with_daemon(&binary, root.path(), |client| {
        let history = HistoryFixtureSpec {
            applied_at_ms: 13,
            removed_text: "",
            ..HISTORY_LEDGER
        };
        let batch = history.build_batch(
            g1.repo_id.clone(),
            g1.revision_id.clone(),
            g1.manifest_generation,
        )?;
        let _receipt = publish_history(root.path(), batch)?;
        let source = legacy_source_batch(
            &g1.repo_id,
            &g1.revision_id,
            1,
            None,
            &[(FILE, "history lexical proof", 1)],
        )?;
        let (active, _receipt) = publish_legacy_source_batch(client, &source, None)?;
        require_history_ids(client, &g1, REF, &[COMMIT])?;
        require_history_ids(client, &g1, TAG, &[COMMIT])?;

        let mut delete = HistoryFixtureSpec {
            message: "fix: sample history",
            ..history
        }
        .build_batch(
            g2.repo_id.clone(),
            g2.revision_id.clone(),
            g2.manifest_generation,
        )?;
        delete.manifest_digest = Some("history-delete:src/history.rs:2".into());
        delete
            .refs
            .push(HistoryRefMutation::Delete(HistoryRefDelete {
                name: "refs/heads/main".into(),
            }));
        delete
            .tags
            .push(HistoryRefMutation::Delete(HistoryRefDelete {
                name: "v1.0.0".into(),
            }));
        delete.diff_hunks.clear();
        let _receipt = publish_history(root.path(), delete)?;
        let source_event = source.generation_reservation_request()?.source_event;
        let delta = SearchCorpusBatch::delta(
            g2.repo_id.clone(),
            g2.revision_id.clone(),
            g2.manifest_generation,
            g1.manifest_generation,
            "lex-seal:2",
        )
        .source_event(SourcePublicationEvent {
            stream_id: source_event.stream_id,
            event_id: "e2e-source-event-2".into(),
            expected_base_event_id: Some(source_event.event_id),
            payload_sha256: [0; 32],
        });
        let (_head, _receipt) = publish_legacy_source_batch(client, &delta, Some(active))?;
        require_tombstones(client)
    })?;
    with_daemon(&binary, root.path(), require_tombstones)
}
