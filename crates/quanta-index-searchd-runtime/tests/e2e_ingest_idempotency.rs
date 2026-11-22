//! QI-BB-032 fast idempotency oracles against the pinned Legacy executable.
//! The current SDK and raw ingest wire must name the same Source batch.

use std::collections::BTreeSet;
use std::error::Error;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use quanta_index_contract::{
    BatchIngestMode, BatchPublishReceipt, ChunkId, ChunkRecord, ERR_SERVER_OVERLOADED,
    GenerationPin, ManifestGeneration, RepoId, RepoRelativePath, RevisionId,
    SearchCorpusIngestBatch, SearchPlaneIngestIpcRequest, SearchPlaneIngestIpcRequestEnvelope,
    SearchPlaneIngestIpcResponse, SearchPlaneIngestIpcResponseEnvelope, SourceFileKey,
    SourcePublicationEvent, lex::LanguageCode,
};
use quanta_index_core::BATCH_DIGEST_MISMATCH_CODE;
use quanta_index_ipc::{ClientIoPolicy, send_request, stamp_batch_digest_v1};
use quanta_index_sdk::SearchCorpusBatch;
use quanta_index_searchd_harness::legacy_reference::{LegacySearchdBinary, daemon_socket_paths};
use quanta_index_searchd_harness::{
    fixture_source_scope_v1, private_tempdir, semantic_source_scopes_for_chunk_records,
};

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

fn source_batches(
    repo: &RepoId,
    revision: &RevisionId,
    path: &str,
    body: &str,
) -> TestResult<(SearchCorpusIngestBatch, SearchCorpusBatch)> {
    let chunk = ChunkRecord {
        chunk_id: ChunkId::new(format!("e2e-idem-{path}")),
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
    };
    let scope = fixture_source_scope_v1(
        SourceFileKey {
            source_repo_id: repo.clone(),
            repo_relative_path: RepoRelativePath::new(path),
        },
        revision.clone(),
        vec![chunk],
        Vec::new(),
    )?;
    let semantic_scopes = semantic_source_scopes_for_chunk_records(repo, &scope.chunks);
    let mut sdk = SearchCorpusBatch::replace_generation(
        repo.clone(),
        revision.clone(),
        ManifestGeneration::new(1),
        "lex-seal:1",
    )
    .source_event(SourcePublicationEvent {
        stream_id: "e2e-harness-source-v1".into(),
        event_id: "e2e-source-event-1".into(),
        expected_base_event_id: None,
        payload_sha256: [0; 32],
    });
    for semantic in &semantic_scopes {
        sdk = sdk.replace_semantic_scope(
            semantic.scope.clone(),
            semantic.scope_digest.clone(),
            semantic.sources.clone(),
            semantic.cluster_memberships.clone(),
        );
    }
    sdk = sdk.replace_scope(
        scope.coverage.clone(),
        scope.source_bytes.clone(),
        scope.chunks.clone(),
        scope.symbols.clone(),
    );
    let mut wire = SearchCorpusIngestBatch {
        source_event: SourcePublicationEvent {
            stream_id: "e2e-harness-source-v1".into(),
            event_id: "e2e-source-event-1".into(),
            expected_base_event_id: None,
            payload_sha256: [0; 32],
        },
        repo_id: repo.clone(),
        revision_id: revision.clone(),
        generation: ManifestGeneration::new(1),
        base_generation: None,
        manifest_digest: "lex-seal:1".into(),
        batch_digest: String::new(),
        mode: BatchIngestMode::ReplaceGeneration,
        bundle_payload: None,
        clear_surfaces: Vec::new(),
        semantic_replace_scopes: semantic_scopes,
        replace_scopes: vec![scope],
        tombstone_scopes: Vec::new(),
        semantic_tombstone_scopes: Vec::new(),
        seal: true,
    };
    wire.source_event.payload_sha256 = quanta_index_contract::source_event_payload_sha256(&wire)?;
    stamp_batch_digest_v1(&mut wire)?;
    if wire.batch_digest != sdk.batch_digest()? {
        return Err("raw IPC fixture differs from public SDK source batch".into());
    }
    Ok((wire, sdk))
}

fn idempotency_rows(state_root: &Path) -> TestResult<u64> {
    let catalog =
        quanta_index_catalog::catalog_dir(state_root).join(quanta_index_catalog::CATALOG_FILE_NAME);
    let connection =
        rusqlite::Connection::open_with_flags(catalog, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let count: i64 =
        connection.query_row("SELECT COUNT(*) FROM idempotency_v2", [], |row| row.get(0))?;
    Ok(u64::try_from(count)?)
}

fn receipt_of(response: SearchPlaneIngestIpcResponse) -> TestResult<BatchPublishReceipt> {
    match response {
        SearchPlaneIngestIpcResponse::SearchCorpusReceipt(outcome) => Ok(outcome.receipt),
        other => Err(format!("expected SearchCorpus receipt, got {other:?}").into()),
    }
}

fn ingest_once(
    socket: &Path,
    request_id: u64,
    batch: SearchCorpusIngestBatch,
) -> TestResult<SearchPlaneIngestIpcResponse> {
    let response: SearchPlaneIngestIpcResponseEnvelope = send_request(
        socket,
        &SearchPlaneIngestIpcRequestEnvelope {
            diagnostic_attempt: quanta_index_contract::DiagnosticAttemptFieldV1::Absent,
            request_id,
            payload: SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(batch),
        },
        ClientIoPolicy::default(),
    )?;
    if response.request_id != request_id {
        return Err(format!("ingest response request ID changed: {response:?}").into());
    }
    Ok(response.payload)
}

fn publish_with_backoff(
    socket: &Path,
    request_id: u64,
    batch: SearchCorpusIngestBatch,
) -> Result<SearchPlaneIngestIpcResponse, Box<dyn Error + Send + Sync>> {
    const PATIENCE: Duration = Duration::from_secs(120);
    let started = Instant::now();
    loop {
        let response = ingest_once(socket, request_id, batch.clone())
            .map_err(|error| -> Box<dyn Error + Send + Sync> { error.to_string().into() })?;
        let overloaded = matches!(
            &response,
            SearchPlaneIngestIpcResponse::Error(error) if error.code == ERR_SERVER_OVERLOADED
        );
        if !overloaded || started.elapsed() >= PATIENCE {
            return Ok(response);
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn serve_count(
    client: &quanta_index_sdk::QuantaIndex,
    repo: &RepoId,
    revision: &RevisionId,
    text: &str,
) -> TestResult<usize> {
    let pin = GenerationPin::new(repo.clone(), revision.clone(), ManifestGeneration::new(1));
    Ok(client
        .lexical()
        .query()
        .native(text)
        .pinned(pin)
        .top_k(5)
        .execute()?
        .results
        .len())
}

#[test]
fn a_replay_is_acked_from_the_record_and_a_forged_digest_never_lands() -> TestResult {
    let parent = private_tempdir()?;
    let state = parent.path().join("state");
    let binary = LegacySearchdBinary::from_environment()?;
    let repo = RepoId::new("repo-e2e")?;
    let revision = RevisionId::new("rev-e2e")?;
    let (batch, sdk) = source_batches(
        &repo,
        &revision,
        "src/idem.rs",
        "fn first_body() { idem_first }",
    )?;
    let process = binary.start(&state, 8)?;
    let outcome = (|| -> TestResult {
        let socket = daemon_socket_paths(&state)[2].clone();
        let first = receipt_of(ingest_once(&socket, 1, batch.clone())?)?;
        if !first.applied || first.durable_sequence == 0 || first.batch_digest != batch.batch_digest
        {
            return Err(format!("first publish must apply under a sequence: {first:?}").into());
        }
        let rows_after_apply = idempotency_rows(&state)?;
        if rows_after_apply != 1 {
            return Err(
                format!("one apply must leave one record, found {rows_after_apply}").into(),
            );
        }
        let replay = receipt_of(ingest_once(&socket, 2, batch.clone())?)?;
        if replay.applied
            || replay.durable_sequence != first.durable_sequence
            || replay.generation != first.generation
            || replay.manifest_digest != first.manifest_digest
            || replay.accepted_replace_scopes != first.accepted_replace_scopes
        {
            return Err(format!(
                "replay must carry original receipt: first={first:?} replay={replay:?}"
            )
            .into());
        }
        let mut forged = batch.clone();
        forged
            .replace_scopes
            .first_mut()
            .and_then(|scope| scope.chunks.first_mut())
            .ok_or("the batch carries one chunk")?
            .text = "fn second_body() { idem_second }".into();
        let refused = ingest_once(&socket, 3, forged)?;
        if !matches!(&refused, SearchPlaneIngestIpcResponse::Error(error)
            if error.code.as_wire_str() == BATCH_DIGEST_MISMATCH_CODE.as_wire_str())
        {
            return Err(format!("forged digest must be refused typed: {refused:?}").into());
        }
        if idempotency_rows(&state)? != rows_after_apply {
            return Err("refused forged batch changed the catalog".into());
        }
        let client = process.connect()?;
        let (activation_receipt, _) = client.search_corpus().publish_and_activate(&sdk, None)?;
        if activation_receipt.applied
            || activation_receipt.durable_sequence != first.durable_sequence
        {
            return Err(format!(
                "activation replay changed original receipt: {activation_receipt:?}"
            )
            .into());
        }
        if serve_count(&client, &repo, &revision, "idem_first")? != 1 {
            return Err("applied body must serve exactly once".into());
        }
        if serve_count(&client, &repo, &revision, "idem_second")? != 0 {
            return Err("refused body must never be served".into());
        }
        Ok(())
    })();
    let stopped = process.stop();
    outcome.and(stopped)?;
    binary.verify()
}

fn duplicate_publishes_converge_on_one_apply(publishers: u64) -> TestResult {
    let parent = private_tempdir()?;
    let state = parent.path().join("state");
    let binary = LegacySearchdBinary::from_environment()?;
    let repo = RepoId::new("repo-e2e")?;
    let revision = RevisionId::new("rev-e2e")?;
    let (batch, sdk) = source_batches(
        &repo,
        &revision,
        "src/race.rs",
        "fn raced_body() { idem_race }",
    )?;
    let process = binary.start(&state, 8)?;
    let outcome = (|| -> TestResult {
        let socket = daemon_socket_paths(&state)[2].clone();
        let publishers = (0..publishers)
            .map(|index| {
                let socket = socket.clone();
                let batch = batch.clone();
                thread::spawn(move || publish_with_backoff(&socket, 1_000 + index, batch))
            })
            .collect::<Vec<_>>();
        let mut receipts = Vec::new();
        for publisher in publishers {
            let response = publisher
                .join()
                .map_err(|panic| format!("publisher panicked: {panic:?}"))?
                .map_err(|error| -> Box<dyn Error> { error })?;
            receipts.push(receipt_of(response)?);
        }
        let applied = receipts.iter().filter(|receipt| receipt.applied).count();
        if applied != 1 {
            return Err(
                format!("exactly one publisher may apply, {applied} did: {receipts:?}").into(),
            );
        }
        let sequences: BTreeSet<u64> = receipts
            .iter()
            .map(|receipt| receipt.durable_sequence)
            .collect();
        if sequences.len() != 1 {
            return Err(format!("every receipt must carry one sequence: {sequences:?}").into());
        }
        if idempotency_rows(&state)? != 1 {
            return Err("duplicates must leave exactly one record".into());
        }
        let client = process.connect()?;
        let (activation_receipt, _) = client.search_corpus().publish_and_activate(&sdk, None)?;
        if activation_receipt.applied
            || activation_receipt.durable_sequence != receipts[0].durable_sequence
        {
            return Err(format!(
                "activation replay changed original receipt: {activation_receipt:?}"
            )
            .into());
        }
        if serve_count(&client, &repo, &revision, "idem_race")? != 1 {
            return Err("duplicates must leave one served row".into());
        }
        Ok(())
    })();
    let stopped = process.stop();
    outcome.and(stopped)?;
    binary.verify()
}

#[test]
fn eight_concurrent_duplicate_publishes_converge_on_one_apply() -> TestResult {
    duplicate_publishes_converge_on_one_apply(8)
}

#[test]
fn thirty_two_concurrent_duplicate_publishes_converge_on_one_apply() -> TestResult {
    duplicate_publishes_converge_on_one_apply(32)
}

#[test]
fn a_replay_after_restart_is_still_a_replay() -> TestResult {
    let parent = private_tempdir()?;
    let state = parent.path().join("state");
    let binary = LegacySearchdBinary::from_environment()?;
    let repo = RepoId::new("repo-e2e")?;
    let revision = RevisionId::new("rev-e2e")?;
    let (batch, _) = source_batches(
        &repo,
        &revision,
        "src/durable.rs",
        "fn durable_body() { idem_durable }",
    )?;
    let first_process = binary.start(&state, 8)?;
    let first = receipt_of(ingest_once(
        &daemon_socket_paths(&state)[2],
        1,
        batch.clone(),
    )?)?;
    first_process.stop()?;
    if !first.applied || first.durable_sequence == 0 || idempotency_rows(&state)? != 1 {
        return Err(format!("first publish was not durable: {first:?}").into());
    }
    let second_process = binary.start(&state, 8)?;
    let replay = receipt_of(ingest_once(&daemon_socket_paths(&state)[2], 2, batch)?)?;
    second_process.stop()?;
    if replay.applied || replay.durable_sequence != first.durable_sequence {
        return Err(format!(
            "replay across restart must be recorded apply: first={first:?} replay={replay:?}"
        )
        .into());
    }
    if idempotency_rows(&state)? != 1 {
        return Err("restart replay changed idempotency row count".into());
    }
    binary.verify()
}
