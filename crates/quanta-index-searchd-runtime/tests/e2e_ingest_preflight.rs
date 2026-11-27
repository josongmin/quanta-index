//! QI-BB-029 / QI-BB-032 — the ingest fault matrix over the real socket:
//! a batch refused by digest verification or by the search-corpus preflight
//! changes nothing anywhere.
//!
//! Every refused batch is a typed code, and after each one the oracles are
//! read from outside the daemon: the idempotency table's row count straight
//! from the catalog file, and the set of files under the state root's index,
//! authority and activation trees (a refused batch creates no generation
//! directory). The same daemon then applies a well-formed batch, so the
//! refusals cost it nothing.
//!
//! Raw IPC drives invalid mode/base pairings, a base that cannot precede
//! its target, an unsealed delta base, invalid digests and a forged body.
//! Invalid raw shapes and digests stop before the journal; mutable source
//! preflight retains one prepared row so the original event can be retried.
//! The SDK computes the digest and its typestate rules out mode/base
//! mismatches. Its matrix covers unsealed base and resource refusals plus
//! the canonical digest on a successful receipt.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{
    BatchIngestMode, ChunkId, ChunkRecord, ManifestGeneration, RepoId, RepoRelativePath,
    RevisionId, SearchCorpusIngestBatch, SearchPlaneErrorCodeV2, SearchPlaneIngestIpcRequest,
    SearchPlaneIngestIpcRequestEnvelope, SearchPlaneIngestIpcResponse,
    SearchPlaneIngestIpcResponseEnvelope, SourcePublicationBinding, SourcePublicationUploadCommit,
    source_event_payload_sha256,
};
use quanta_index_core::{
    BATCH_DIGEST_MISMATCH_CODE, INGEST_RESOURCE_BUDGET_EXCEEDED_CODE, IngestResourcePolicy,
};
use quanta_index_ipc::{
    ClientIoPolicy, for_each_source_publication_upload_part, send_request,
    source_publication_upload_identity, stamp_batch_digest_v1,
};
use quanta_index_sdk::{PublishedBatchEvidence, QuantaIndex, SdkError, SearchCorpusBatch};
use quanta_index_searchd_harness::{
    SourceCorpusFixture, fixture_source_scope_v1,
    legacy_reference::{LegacySearchdBinary, SearchdBinaryProcess, daemon_socket_paths},
    private_tempdir, semantic_source_scopes_for_chunk_records,
};

type TestResult = Result<(), Box<dyn Error>>;
static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

const RAW_SHAPE_INVALID: SearchPlaneErrorCodeV2 = SearchPlaneErrorCodeV2::InvalidRequest;
const DELTA_BASE_NOT_SEALED: SearchPlaneErrorCodeV2 =
    SearchPlaneErrorCodeV2::SearchCorpusDeltaBaseNotSealed;

fn repo() -> Result<RepoId, Box<dyn Error>> {
    Ok(RepoId::new("repo-e2e")?)
}

fn revision() -> Result<RevisionId, Box<dyn Error>> {
    Ok(RevisionId::new("rev-e2e")?)
}

fn with_process(
    vector_max_bytes: Option<u64>,
    check: impl FnOnce(&Path, &Path, &SearchdBinaryProcess) -> TestResult,
) -> TestResult {
    let parent = private_tempdir()?;
    let state_root = parent.path().join("state");
    let binary = LegacySearchdBinary::from_environment()?;
    let mut command = binary.command()?;
    let _configured = command.arg("serve").arg("--state-root").arg(&state_root);
    let _configured = command.apply_searchd_env(8);
    let defaults = IngestResourcePolicy::DEFAULT;
    let max_records = if vector_max_bytes.is_some() {
        usize::MAX
    } else {
        defaults.max_records()
    };
    let _configured = command
        .env("QUANTA_INDEX_INGEST_MAX_RECORDS", max_records.to_string())
        .env(
            "QUANTA_INDEX_INGEST_MAX_TEXT_BYTES",
            defaults.max_text_bytes().to_string(),
        )
        .env(
            "QUANTA_INDEX_INGEST_MAX_VECTOR_BYTES",
            vector_max_bytes
                .unwrap_or(defaults.max_vector_bytes())
                .to_string(),
        );
    let process = SearchdBinaryProcess::start_with_pinned_command(&state_root, command)?;
    binary.verify()?;
    let ingest_socket = daemon_socket_paths(&state_root)[2].clone();
    let result = check(&state_root, &ingest_socket, &process);
    let stopped = process.stop();
    let verified = binary.verify();
    result?;
    stopped?;
    verified
}

fn fixture_chunk(path: &str, text: &str) -> Result<ChunkRecord, Box<dyn Error>> {
    Ok(ChunkRecord {
        chunk_id: ChunkId::new(format!("e2e-idem-{path}")),
        repo_relative_path: RepoRelativePath::new(path),
        language: LanguageCode::new("rust")?,
        start_byte: 0,
        end_byte: u32::try_from(text.len())?,
        start_line: 1,
        end_line: 2,
        text: text.to_string().into_boxed_str(),
        structural: None,
        parent_chunk_id: None,
        source_repo_id: None,
    })
}

fn fixture_batch(
    path: &str,
    text: &str,
    generation: u64,
    event_id: String,
) -> Result<SearchCorpusIngestBatch, Box<dyn Error>> {
    let mut fixture = SourceCorpusFixture::new(
        repo()?,
        revision()?,
        ManifestGeneration::new(generation),
        event_id,
    );
    fixture.replace_chunks(vec![fixture_chunk(path, text)?])?;
    Ok(fixture.frozen_batch()?)
}

fn issue_fixture_source_event(
    batch: &mut SearchCorpusIngestBatch,
    next_event_id: &mut u64,
) -> TestResult {
    batch.source_event.event_id = format!("e2e-source-event-{next_event_id}");
    *next_event_id = next_event_id
        .checked_add(1)
        .ok_or("source event ID overflow")?;
    batch.source_event.payload_sha256 = source_event_payload_sha256(batch)?;
    stamp_batch_digest_v1(batch)?;
    Ok(())
}

fn ingest_request(
    socket: &Path,
    payload: SearchPlaneIngestIpcRequest,
) -> Result<SearchPlaneIngestIpcResponse, Box<dyn Error>> {
    let request_id = NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed);
    let response: SearchPlaneIngestIpcResponseEnvelope = send_request(
        socket,
        &SearchPlaneIngestIpcRequestEnvelope {
            request_id,
            diagnostic_attempt: quanta_index_contract::DiagnosticAttemptFieldV1::Absent,
            payload,
        },
        ClientIoPolicy::default(),
    )?;
    if response.request_id != request_id {
        return Err("ingest response request ID changed".into());
    }
    Ok(response.payload)
}

fn ingest_once(
    socket: &Path,
    batch: SearchCorpusIngestBatch,
) -> Result<SearchPlaneIngestIpcResponse, Box<dyn Error>> {
    ingest_request(
        socket,
        SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(batch),
    )
}

fn stage_source_publication(socket: &Path, batch: &SearchCorpusIngestBatch) -> TestResult {
    let identity = source_publication_upload_identity(batch)?;
    let mut next_offset = 0_u64;
    for_each_source_publication_upload_part(batch, identity, |part| -> TestResult {
        let end = part
            .offset
            .checked_add(u64::try_from(part.bytes.len())?)
            .ok_or("source upload offset overflow")?;
        let response = ingest_request(
            socket,
            SearchPlaneIngestIpcRequest::StageSourcePublication(part),
        )?;
        let SearchPlaneIngestIpcResponse::SourcePublicationUploadAck(ack) = response else {
            return Err(format!("staged upload was not acknowledged: {response:?}").into());
        };
        if ack.identity != identity || ack.next_offset != end {
            return Err(format!("staged upload acknowledgment differs: {ack:?}").into());
        }
        next_offset = end;
        Ok(())
    })
    .map_err(|error| -> Box<dyn Error> {
        match error {
            quanta_index_ipc::SourcePublicationUploadError::Transport(error) => error,
            quanta_index_ipc::SourcePublicationUploadError::Encoding(error) => Box::new(error),
        }
    })?;
    if next_offset != identity.body_bytes {
        return Err("staged upload is incomplete".into());
    }
    Ok(())
}

fn publish_staged_source(
    socket: &Path,
    batch: &SearchCorpusIngestBatch,
) -> Result<SearchPlaneIngestIpcResponse, Box<dyn Error>> {
    ingest_request(
        socket,
        SearchPlaneIngestIpcRequest::PublishStagedSourcePublication(
            SourcePublicationUploadCommit {
                identity: source_publication_upload_identity(batch)?,
                publication: SourcePublicationBinding::for_batch(batch),
            },
        ),
    )
}

/// The idempotency table's row count, read straight from the catalog file.
fn idempotency_rows(state_root: &Path) -> Result<u64, Box<dyn Error>> {
    let path =
        quanta_index_catalog::catalog_dir(state_root).join(quanta_index_catalog::CATALOG_FILE_NAME);
    let connection =
        rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let rows: i64 =
        connection.query_row("SELECT COUNT(*) FROM idempotency_v2", [], |row| row.get(0))?;
    Ok(u64::try_from(rows)?)
}

/// Every file under the trees a search-corpus batch may write, with its
/// length: the byte-level oracle for "zero bytes changed".
fn durable_tree(state_root: &Path) -> Result<BTreeMap<PathBuf, u64>, Box<dyn Error>> {
    fn walk(dir: &Path, into: &mut BTreeMap<PathBuf, u64>) -> Result<(), Box<dyn Error>> {
        if !dir.exists() {
            return Ok(());
        }
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                walk(&path, into)?;
            } else {
                let _prior = into.insert(path, entry.metadata()?.len());
            }
        }
        Ok(())
    }
    let mut files = BTreeMap::new();
    for tree in ["indexes", "authorities", "activations"] {
        walk(&state_root.join(tree), &mut files)?;
    }
    Ok(files)
}

fn typed_code(response: &SearchPlaneIngestIpcResponse) -> Option<&str> {
    match response {
        SearchPlaneIngestIpcResponse::Error(error) => Some(error.code.as_wire_str()),
        SearchPlaneIngestIpcResponse::SearchCorpusReceipt(_)
        | SearchPlaneIngestIpcResponse::HistoryReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoCommitRecencyReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoTopicReceipt(_)
        | SearchPlaneIngestIpcResponse::FileOwnershipReceipt(_)
        | SearchPlaneIngestIpcResponse::FileContributorReceipt(_)
        | SearchPlaneIngestIpcResponse::DirtyReceipt(_)
        | SearchPlaneIngestIpcResponse::RuntimeCatalogReceipt(_)
        | SearchPlaneIngestIpcResponse::StructuralReceipt(_)
        | SearchPlaneIngestIpcResponse::SourcePublicationUploadAck(_)

        | SearchPlaneIngestIpcResponse::RepoMetaReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoDescriptionReceipt(_) => None,
    }
}

#[derive(Debug, Eq, PartialEq)]
struct VisibleSourceText {
    generation: u64,
    source_candidates: Vec<String>,
    text_candidates: Vec<String>,
}

fn visible_source_text(
    client: &QuantaIndex,
    needle: &str,
) -> Result<VisibleSourceText, Box<dyn Error>> {
    let source = client
        .lexical()
        .query()
        .native(format!("index:no {needle}"))
        .active(repo()?, revision()?)
        .top_k(5)
        .execute()?;
    let text = client
        .lexical()
        .query()
        .native(format!("type:file {needle}"))
        .active(repo()?, revision()?)
        .top_k(5)
        .execute()?;
    if source.generation != text.generation {
        return Err(format!(
            "Source and Text resolved different visible generations: source={:?} text={:?}",
            source.generation, text.generation
        )
        .into());
    }
    let mut source_candidates = source
        .results
        .into_iter()
        .map(|candidate| candidate.candidate_id)
        .collect::<Vec<_>>();
    let mut text_candidates = text
        .results
        .into_iter()
        .map(|candidate| candidate.candidate_id)
        .collect::<Vec<_>>();
    source_candidates.sort();
    text_candidates.sort();
    Ok(VisibleSourceText {
        generation: source.generation.manifest_generation.get(),
        source_candidates,
        text_candidates,
    })
}

fn require_one_visible_source_and_text(
    visible: &VisibleSourceText,
    generation: u64,
    label: &str,
) -> TestResult {
    if visible.generation != generation
        || visible.source_candidates.len() != 1
        || visible.text_candidates.len() != 1
        || visible.source_candidates != visible.text_candidates
    {
        return Err(format!(
            "{label}: expected one matching visible Source and Text row at generation {generation}, got {visible:?}"
        )
        .into());
    }
    Ok(())
}

/// Publish one batch over the raw socket and require the typed refusal
/// `expected`, with the catalog and the durable trees exactly as before.
///
/// Intrinsic refusals only (digest verification): the journal is never
/// reached, so no row exists afterwards.
fn refused_with_nothing_changed(
    state_root: &Path,
    ingest_socket: &Path,
    label: &str,
    batch: quanta_index_contract::SearchCorpusIngestBatch,
    expected: SearchPlaneErrorCodeV2,
) -> TestResult {
    let rows_before = idempotency_rows(state_root)?;
    let tree_before = durable_tree(state_root)?;
    let response = ingest_once(ingest_socket, batch)?;
    if typed_code(&response) != Some(expected.as_wire_str()) {
        return Err(format!("{label}: expected {expected}, got {response:?}").into());
    }
    if idempotency_rows(state_root)? != rows_before {
        return Err(format!("{label}: an intrinsic refusal left an idempotency record").into());
    }
    if durable_tree(state_root)? != tree_before {
        return Err(format!("{label}: a refused batch changed bytes under the state root").into());
    }
    Ok(())
}

/// Source preflight retains one prepared journal row so the same source
/// event can be retried after its base or resource condition changes.
fn refused_with_one_prepared_row(
    state_root: &Path,
    ingest_socket: &Path,
    label: &str,
    batch: quanta_index_contract::SearchCorpusIngestBatch,
    expected: SearchPlaneErrorCodeV2,
) -> TestResult {
    let rows_before = idempotency_rows(state_root)?;
    let tree_before = durable_tree(state_root)?;
    let response = ingest_once(ingest_socket, batch.clone())?;
    if typed_code(&response) != Some(expected.as_wire_str()) {
        return Err(format!("{label}: expected {expected}, got {response:?}").into());
    }
    let expected_rows = rows_before
        .checked_add(1)
        .ok_or("idempotency row count overflow")?;
    if idempotency_rows(state_root)? != expected_rows {
        return Err(format!("{label}: a preflight refusal must prepare exactly one row").into());
    }
    if durable_tree(state_root)? != tree_before {
        return Err(format!("{label}: a refused batch changed bytes under the state root").into());
    }
    let replayed = ingest_once(ingest_socket, batch)?;
    if typed_code(&replayed) != Some(expected.as_wire_str()) {
        return Err(format!("{label}: the unchanged retry must refuse, got {replayed:?}").into());
    }
    if idempotency_rows(state_root)? != expected_rows {
        return Err(format!("{label}: a refused replay must add no row").into());
    }
    if durable_tree(state_root)? != tree_before {
        return Err(format!("{label}: a refused replay changed bytes under the state root").into());
    }
    Ok(())
}

/// Every refusal the raw socket can provoke: intrinsic (digest)
///
/// refusals record nothing, mutable source preflight retains one
/// prepared row, and no refusal changes a byte under the
/// state root; then a well-formed batch applies on the same daemon.
#[test]
fn every_preflight_refusal_over_raw_ipc_changes_nothing() -> TestResult {
    with_process(None, |state_root, ingest_socket, process| {
        let good = fixture_batch(
            "src/preflight.rs",
            "fn preflight_body() { pf }",
            1,
            "e2e-source-event-1".into(),
        )?;
        if idempotency_rows(state_root)? != 0 {
            return Err("a fresh daemon starts with no records".into());
        }
        let mut next_event_id = 2;

        // Shape: ReplaceGeneration names a base.
        let mut replace_with_base = good.clone();
        replace_with_base.base_generation = Some(ManifestGeneration::new(0));
        issue_fixture_source_event(&mut replace_with_base, &mut next_event_id)?;
        refused_with_nothing_changed(
            state_root,
            ingest_socket,
            "replace+base",
            replace_with_base,
            RAW_SHAPE_INVALID,
        )?;

        // Shape: Delta names no base.
        let mut delta_without_base = good.clone();
        delta_without_base.mode = BatchIngestMode::Delta;
        issue_fixture_source_event(&mut delta_without_base, &mut next_event_id)?;
        refused_with_nothing_changed(
            state_root,
            ingest_socket,
            "delta-no-base",
            delta_without_base,
            RAW_SHAPE_INVALID,
        )?;

        // Shape: the base cannot precede its target.
        let mut base_not_older = good.clone();
        base_not_older.mode = BatchIngestMode::Delta;
        base_not_older.base_generation = Some(good.generation);
        issue_fixture_source_event(&mut base_not_older, &mut next_event_id)?;
        refused_with_nothing_changed(
            state_root,
            ingest_socket,
            "base>=target",
            base_not_older,
            RAW_SHAPE_INVALID,
        )?;

        // Cross-track preflight: a delta on a base nothing ever sealed.
        let mut unsealed_base = good.clone();
        unsealed_base.generation = ManifestGeneration::new(good.generation.get().saturating_add(1));
        unsealed_base.mode = BatchIngestMode::Delta;
        unsealed_base.base_generation = Some(good.generation);
        issue_fixture_source_event(&mut unsealed_base, &mut next_event_id)?;
        refused_with_one_prepared_row(
            state_root,
            ingest_socket,
            "unsealed-base",
            unsealed_base,
            DELTA_BASE_NOT_SEALED,
        )?;

        // Digest: empty, wrong shape, and a body that is not what its digest
        // names — verified before anything else looks at the batch.
        let mut empty_digest = good.clone();
        empty_digest.batch_digest = String::new();
        refused_with_nothing_changed(
            state_root,
            ingest_socket,
            "empty-digest",
            empty_digest,
            BATCH_DIGEST_MISMATCH_CODE,
        )?;
        let mut opaque_digest = good.clone();
        opaque_digest.batch_digest = "batch:preflight".to_string();
        refused_with_nothing_changed(
            state_root,
            ingest_socket,
            "opaque-digest",
            opaque_digest,
            BATCH_DIGEST_MISMATCH_CODE,
        )?;
        let mut forged = good.clone();
        forged.manifest_digest = format!("{}-forged", good.manifest_digest);
        refused_with_nothing_changed(
            state_root,
            ingest_socket,
            "forged-digest",
            forged,
            BATCH_DIGEST_MISMATCH_CODE,
        )?;

        // The daemon is unharmed: the well-formed batch applies and is recorded.
        let applied = ingest_once(ingest_socket, good.clone())?;
        let SearchPlaneIngestIpcResponse::SearchCorpusReceipt(applied) = applied else {
            return Err(format!("the well-formed batch must apply, got {applied:?}").into());
        };
        if !applied.receipt.applied {
            return Err(format!("the well-formed batch must apply, got {applied:?}").into());
        }
        // One prepared source refusal plus the one apply. Invalid raw shapes
        // and digests never enter the journal.
        if idempotency_rows(state_root)? != 2 {
            return Err(format!(
                "one prepared refusal and one apply must leave two rows, found {}",
                idempotency_rows(state_root)?
            )
            .into());
        }

        let client = process.connect()?;
        let initial_activation = client
            .search_corpus()
            .activate_published(&PublishedBatchEvidence::from(&applied), None)?;
        let visible_before = visible_source_text(&client, "preflight_body")?;
        require_one_visible_source_and_text(&visible_before, 1, "initial publication")?;

        // The staged route has a separate pre-journal seal gate. Prove it over
        // the real socket with an already-visible Source + Text generation:
        // staging may own its upload file, but commit must not reserve a source
        // event, advance the journal, alter durable corpus state, or change the
        // active rows.
        let mut unsealed_staged = fixture_batch(
            "src/staged-unsealed.rs",
            "fn staged_unsealed_body() { staged_unsealed_visible }",
            2,
            "unused-staged-event".into(),
        )?;
        unsealed_staged.source_event.expected_base_event_id =
            Some(good.source_event.event_id.clone());
        unsealed_staged.seal = false;
        issue_fixture_source_event(&mut unsealed_staged, &mut next_event_id)?;
        stage_source_publication(ingest_socket, &unsealed_staged)?;

        let rows_before_staged_commit = idempotency_rows(state_root)?;
        let tree_before_staged_commit = durable_tree(state_root)?;
        let refused = publish_staged_source(ingest_socket, &unsealed_staged)?;
        let SearchPlaneIngestIpcResponse::Error(error) = refused else {
            return Err(format!("unsealed staged publication was accepted: {refused:?}").into());
        };
        if error.code != RAW_SHAPE_INVALID || !error.message.contains("requires a sealed batch") {
            return Err(format!(
                "unsealed staged publication did not return the seal-policy refusal: {error:?}"
            )
            .into());
        }
        if idempotency_rows(state_root)? != rows_before_staged_commit {
            return Err("unsealed staged publication reserved a journal row".into());
        }
        if durable_tree(state_root)? != tree_before_staged_commit {
            return Err("unsealed staged publication changed durable corpus state".into());
        }
        let visible_after = visible_source_text(&client, "preflight_body")?;
        if visible_after != visible_before {
            return Err(format!(
                "unsealed staged publication changed visible Source or Text: before={visible_before:?} after={visible_after:?}"
            )
            .into());
        }
        let refused_visible = visible_source_text(&client, "staged_unsealed_visible")?;
        if refused_visible.generation != 1
            || !refused_visible.source_candidates.is_empty()
            || !refused_visible.text_candidates.is_empty()
        {
            return Err(format!(
                "unsealed staged rows escaped into the active Source or Text view: {refused_visible:?}"
            )
            .into());
        }

        // The refusal consumes no source authority: a normal successor can use
        // the original active event and becomes visible through both routes on
        // the same daemon.
        let mut recovered = fixture_batch(
            "src/post-staged.rs",
            "fn post_staged_body() { post_staged_visible }",
            2,
            "unused-recovery-event".into(),
        )?;
        recovered.source_event.expected_base_event_id = Some(good.source_event.event_id.clone());
        issue_fixture_source_event(&mut recovered, &mut next_event_id)?;
        let recovered_response = ingest_once(ingest_socket, recovered)?;
        let SearchPlaneIngestIpcResponse::SearchCorpusReceipt(recovered) = recovered_response
        else {
            return Err(format!(
                "normal publication after the staged refusal did not return a receipt: {recovered_response:?}"
            )
            .into());
        };
        if !recovered.receipt.applied {
            return Err(format!(
                "normal publication after the staged refusal did not apply: {recovered:?}"
            )
            .into());
        }
        let _recovered_activation = client.search_corpus().activate_published(
            &PublishedBatchEvidence::from(&recovered),
            Some(initial_activation.active),
        )?;
        let recovered_visible = visible_source_text(&client, "post_staged_visible")?;
        require_one_visible_source_and_text(
            &recovered_visible,
            2,
            "publication after staged refusal",
        )?;
        if idempotency_rows(state_root)? != rows_before_staged_commit.saturating_add(1) {
            return Err(
                "normal publication after staged refusal did not add exactly one row".into(),
            );
        }
        Ok(())
    })
}

fn sdk_batch(
    generation: u64,
    base_generation: Option<u64>,
    path: &str,
    text: &str,
    event_id: u64,
) -> Result<SearchCorpusBatch<true>, Box<dyn Error>> {
    let manifest_digest = format!("manifest:sdk-preflight:{generation}");
    let batch = match base_generation {
        Some(base) => SearchCorpusBatch::delta(
            repo()?,
            revision()?,
            ManifestGeneration::new(generation),
            ManifestGeneration::new(base),
            manifest_digest,
        ),
        None => SearchCorpusBatch::replace_generation(
            repo()?,
            revision()?,
            ManifestGeneration::new(generation),
            manifest_digest,
        ),
    };
    let chunk = ChunkRecord {
        chunk_id: ChunkId::new(format!("sdk-preflight-{path}")),
        repo_relative_path: RepoRelativePath::new(path),
        language: LanguageCode::new("rust")?,
        start_byte: 0,
        end_byte: u32::try_from(text.len())?,
        start_line: 1,
        end_line: 1,
        text: text.to_string().into_boxed_str(),
        structural: None,
        parent_chunk_id: None,
        source_repo_id: None,
    };
    let semantic_scopes =
        semantic_source_scopes_for_chunk_records(&repo()?, std::slice::from_ref(&chunk));
    let scope = fixture_source_scope_v1(
        quanta_index_contract::SourceFileKey {
            source_repo_id: repo()?,
            repo_relative_path: RepoRelativePath::new(path),
        },
        revision()?,
        vec![chunk],
        Vec::new(),
    )?;
    let mut batch = batch
        .source_event(
            fixture_batch(path, text, 1, format!("e2e-source-event-{event_id}"))?.source_event,
        )
        .replace_scope(
            scope.coverage,
            scope.source_bytes,
            scope.chunks,
            scope.symbols,
        );

    for scope in semantic_scopes {
        batch = batch.replace_semantic_scope(
            scope.scope,
            scope.scope_digest,
            scope.sources,
            scope.cluster_memberships,
        );
    }
    Ok(batch)
}

fn remote_code(error: &SdkError) -> Option<&str> {
    match error {
        SdkError::Remote { code, .. } => Some(code.as_wire_str()),
        SdkError::UnsupportedNativeQuery { .. }
        | SdkError::Usage(_)
        | SdkError::Protocol(_)
        | SdkError::Native(_)
        | SdkError::Serialization(_)
        | SdkError::Transport(_)
        | SdkError::Binding { .. }
        | SdkError::AfterPublish { .. }
        | SdkError::PlaneUnavailable { .. } => None,
    }
}

/// Through the SDK: a delta on an unsealed base and a batch past the
///
/// resource envelope are typed refusals that each retain one prepared
/// journal row for retry; the batch that applies carries
/// the canonical digest the receipt names.
#[test]
fn sdk_publishes_carry_the_canonical_digest_and_refusals_record_nothing() -> TestResult {
    // One chunk plus one derived semantic source per batch fits the
    // one-record vector envelope; the two-scope batch below does not.
    let one_record_vector_bytes =
        u64::try_from(quanta_index_search_plane::SEARCH_OWNED_SEMANTIC_DIMENSION)?
            .saturating_mul(4);
    // The byte ceilings are declared under the process memory envelope
    // (QI-BB-016), so the text ceiling this test does not exercise keeps
    // its default rather than an unbounded value the envelope would refuse.
    let policy = IngestResourcePolicy::new(
        usize::MAX,
        IngestResourcePolicy::DEFAULT.max_text_bytes(),
        one_record_vector_bytes.saturating_mul(2).saturating_sub(1),
    )?;
    with_process(
        Some(policy.max_vector_bytes()),
        |state_root, _ingest_socket, process| {
            let client = process.connect()?;

            let rows_before = idempotency_rows(state_root)?;
            let tree_before = durable_tree(state_root)?;

            let unsealed_base = sdk_batch(2, Some(1), "src/sdk_delta.rs", "fn sdk_delta() {}", 1)?;
            let refused = client
                .search_corpus()
                .publish(&unsealed_base)
                .expect_err("a delta on an unsealed base must be refused");
            if remote_code(&refused) != Some(DELTA_BASE_NOT_SEALED.as_wire_str()) {
                return Err(format!(
                    "expected {DELTA_BASE_NOT_SEALED} through the SDK, got {refused:?}"
                )
                .into());
            }

            let extra_chunk = ChunkRecord {
                chunk_id: ChunkId::new("sdk-preflight-big-two"),
                repo_relative_path: RepoRelativePath::new("src/sdk_big_two.rs"),
                language: LanguageCode::new("rust")?,
                start_byte: 0,
                end_byte: 16,
                start_line: 1,
                end_line: 1,
                text: "fn sdk_big_two()".to_string().into_boxed_str(),
                structural: None,
                parent_chunk_id: None,
                source_repo_id: None,
            };
            let semantic_scopes = semantic_source_scopes_for_chunk_records(
                &repo()?,
                std::slice::from_ref(&extra_chunk),
            );
            let extra_scope = fixture_source_scope_v1(
                quanta_index_contract::SourceFileKey {
                    source_repo_id: repo()?,
                    repo_relative_path: RepoRelativePath::new("src/sdk_big_two.rs"),
                },
                revision()?,
                vec![extra_chunk],
                Vec::new(),
            )?;
            let mut oversized = sdk_batch(1, None, "src/sdk_big.rs", "fn sdk_big() {}", 2)?
                .replace_scope(
                    extra_scope.coverage,
                    extra_scope.source_bytes,
                    extra_scope.chunks,
                    extra_scope.symbols,
                );

            for scope in semantic_scopes {
                oversized = oversized.replace_semantic_scope(
                    scope.scope,
                    scope.scope_digest,
                    scope.sources,
                    scope.cluster_memberships,
                );
            }
            let refused = client
                .search_corpus()
                .publish(&oversized)
                .expect_err("a batch past the envelope must be refused");
            if remote_code(&refused) != Some(INGEST_RESOURCE_BUDGET_EXCEEDED_CODE.as_wire_str()) {
                return Err(format!(
            "expected {INGEST_RESOURCE_BUDGET_EXCEEDED_CODE} through the SDK, got {refused:?}"
        )
        .into());
            }

            // Two preflight refusals: two prepared retryable rows, zero changed
            // bytes.
            if idempotency_rows(state_root)? != rows_before.saturating_add(2) {
                return Err(format!(
                    "SDK refusals must prepare exactly one row each, found {}",
                    idempotency_rows(state_root)?
                )
                .into());
            }
            if durable_tree(state_root)? != tree_before {
                return Err("SDK refusals must change no bytes under the state root".into());
            }

            let fits = sdk_batch(1, None, "src/sdk_fits.rs", "fn sdk_fits() {}", 3)?;
            let receipt = client.search_corpus().publish(&fits)?;
            if !receipt.applied || receipt.batch_digest != fits.batch_digest()? {
                return Err(format!(
                    "the applied batch must be recorded under its canonical digest: {receipt:?}"
                )
                .into());
            }
            if idempotency_rows(state_root)? != rows_before.saturating_add(3) {
                return Err("the applied batch must be the third new record".into());
            }
            // The same batch again is the recorded apply, through the SDK too.
            let replay = client.search_corpus().publish(&fits)?;
            if replay.applied || replay.durable_sequence != receipt.durable_sequence {
                return Err(format!("an SDK resend must replay the record: {replay:?}").into());
            }
            Ok(())
        },
    )
}
