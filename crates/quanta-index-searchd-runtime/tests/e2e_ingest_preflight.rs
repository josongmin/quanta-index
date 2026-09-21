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
//! Raw IPC can send anything, so it drives the whole matrix: invalid
//! mode/base pairings, a base that cannot precede its target, a delta on a
//! base nothing ever sealed, an empty digest, a digest of the wrong shape,
//! and a body that is not what its digest names. The SDK computes the digest
//! itself and its typestate cannot express a mode/base mismatch, so its side
//! of the matrix is what a producer can still get wrong: a delta on an
//! unsealed base and a batch past the resource envelope, both typed with no
//! record, plus the proof that what the SDK sends carries the canonical
//! digest the receipt names.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::error::Error;
use std::path::{Path, PathBuf};

use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{
    BatchIngestMode, ChunkId, ChunkRecord, ManifestGeneration, RepoRelativePath,
    SearchPlaneErrorCodeV2, SearchPlaneIngestIpcRequest, SearchPlaneIngestIpcResponse,
    SearchScopeKey, SearchScopeSurface,
};
use quanta_index_core::{
    BATCH_DIGEST_MISMATCH_CODE, INGEST_RESOURCE_BUDGET_EXCEEDED_CODE, IngestResourcePolicy,
};
use quanta_index_ipc::stamp_batch_digest_v1;
use quanta_index_sdk::{ConnectOptions, QuantaIndex, SdkError, SearchCorpusBatch};
use quanta_index_searchd_harness as e2e_harness;

use e2e_harness::E2eRuntime;

type TestResult = Result<(), Box<dyn Error>>;

const SHAPE_INVALID: SearchPlaneErrorCodeV2 = SearchPlaneErrorCodeV2::SearchCorpusBatchShapeInvalid;
const DELTA_BASE_NOT_SEALED: SearchPlaneErrorCodeV2 =
    SearchPlaneErrorCodeV2::SearchCorpusDeltaBaseNotSealed;

/// The idempotency table's row count, read straight from the catalog file.
fn idempotency_rows(rt: &E2eRuntime) -> Result<u64, Box<dyn Error>> {
    let path = quanta_index_catalog::catalog_dir(rt.state_root())
        .join(quanta_index_catalog::CATALOG_FILE_NAME);
    let connection =
        rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let rows: i64 =
        connection.query_row("SELECT COUNT(*) FROM idempotency_v1", [], |row| row.get(0))?;
    Ok(u64::try_from(rows)?)
}

/// Every file under the trees a search-corpus batch may write, with its
/// length: the byte-level oracle for "zero bytes changed".
fn durable_tree(rt: &E2eRuntime) -> Result<BTreeMap<PathBuf, u64>, Box<dyn Error>> {
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
        walk(&rt.state_root().join(tree), &mut files)?;
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
        | SearchPlaneIngestIpcResponse::RepoMapReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoMetaReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoDescriptionReceipt(_) => None,
    }
}

/// Publish one batch over the raw socket and require the typed refusal
/// `expected`, with the catalog and the durable trees exactly as before.
fn refused_with_nothing_changed(
    rt: &mut E2eRuntime,
    label: &str,
    batch: quanta_index_contract::SearchCorpusIngestBatch,
    expected: SearchPlaneErrorCodeV2,
) -> TestResult {
    let rows_before = idempotency_rows(rt)?;
    let tree_before = durable_tree(rt)?;
    let response = rt.ingest_once(SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(batch))?;
    if typed_code(&response) != Some(expected.as_wire_str()) {
        return Err(format!("{label}: expected {expected}, got {response:?}").into());
    }
    if idempotency_rows(rt)? != rows_before {
        return Err(format!("{label}: a refused batch left an idempotency record").into());
    }
    if durable_tree(rt)? != tree_before {
        return Err(format!("{label}: a refused batch changed bytes under the state root").into());
    }
    Ok(())
}

/// Every refusal the raw socket can provoke before intent, each leaving
/// the catalog and the durable trees untouched; then a well-formed batch
/// applies on the same daemon.
#[test]
fn every_preflight_refusal_over_raw_ipc_changes_nothing() -> TestResult {
    let mut rt = E2eRuntime::boot()?;
    rt.start()?;
    let good = rt.text_search_corpus_batch("src/preflight.rs", "fn preflight_body() { pf }")?;
    if idempotency_rows(&rt)? != 0 {
        return Err("a fresh daemon starts with no records".into());
    }

    // Shape: ReplaceGeneration names a base.
    let mut replace_with_base = good.clone();
    replace_with_base.base_generation = Some(ManifestGeneration::new(0));
    stamp_batch_digest_v1(&mut replace_with_base)?;
    refused_with_nothing_changed(&mut rt, "replace+base", replace_with_base, SHAPE_INVALID)?;

    // Shape: Delta names no base.
    let mut delta_without_base = good.clone();
    delta_without_base.mode = BatchIngestMode::Delta;
    stamp_batch_digest_v1(&mut delta_without_base)?;
    refused_with_nothing_changed(&mut rt, "delta-no-base", delta_without_base, SHAPE_INVALID)?;

    // Shape: the base cannot precede its target.
    let mut base_not_older = good.clone();
    base_not_older.mode = BatchIngestMode::Delta;
    base_not_older.base_generation = Some(good.generation);
    stamp_batch_digest_v1(&mut base_not_older)?;
    refused_with_nothing_changed(&mut rt, "base>=target", base_not_older, SHAPE_INVALID)?;

    // Cross-track preflight: a delta on a base nothing ever sealed.
    let mut unsealed_base = good.clone();
    unsealed_base.generation = ManifestGeneration::new(good.generation.get().saturating_add(1));
    unsealed_base.mode = BatchIngestMode::Delta;
    unsealed_base.base_generation = Some(good.generation);
    stamp_batch_digest_v1(&mut unsealed_base)?;
    refused_with_nothing_changed(
        &mut rt,
        "unsealed-base",
        unsealed_base,
        DELTA_BASE_NOT_SEALED,
    )?;

    // Digest: empty, wrong shape, and a body that is not what its digest
    // names — verified before anything else looks at the batch.
    let mut empty_digest = good.clone();
    empty_digest.batch_digest = String::new();
    refused_with_nothing_changed(
        &mut rt,
        "empty-digest",
        empty_digest,
        BATCH_DIGEST_MISMATCH_CODE,
    )?;
    let mut opaque_digest = good.clone();
    opaque_digest.batch_digest = "batch:preflight".to_string();
    refused_with_nothing_changed(
        &mut rt,
        "opaque-digest",
        opaque_digest,
        BATCH_DIGEST_MISMATCH_CODE,
    )?;
    let mut forged = good.clone();
    forged.manifest_digest = format!("{}-forged", good.manifest_digest);
    refused_with_nothing_changed(&mut rt, "forged-digest", forged, BATCH_DIGEST_MISMATCH_CODE)?;

    // The daemon is unharmed: the well-formed batch applies and is recorded.
    let applied = rt.ingest_once(SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(good))?;
    match &applied {
        SearchPlaneIngestIpcResponse::SearchCorpusReceipt(receipt) if receipt.applied => {}
        SearchPlaneIngestIpcResponse::SearchCorpusReceipt(_)
        | SearchPlaneIngestIpcResponse::Error(_)
        | SearchPlaneIngestIpcResponse::HistoryReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoCommitRecencyReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoTopicReceipt(_)
        | SearchPlaneIngestIpcResponse::FileOwnershipReceipt(_)
        | SearchPlaneIngestIpcResponse::FileContributorReceipt(_)
        | SearchPlaneIngestIpcResponse::DirtyReceipt(_)
        | SearchPlaneIngestIpcResponse::RuntimeCatalogReceipt(_)
        | SearchPlaneIngestIpcResponse::StructuralReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoMapReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoMetaReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoDescriptionReceipt(_) => {
            return Err(format!("the well-formed batch must apply, got {applied:?}").into());
        }
    }
    if idempotency_rows(&rt)? != 1 {
        return Err("the applied batch must be the only record".into());
    }
    Ok(())
}

/// A producer client over the daemon's sockets.
fn sdk_client(rt: &E2eRuntime) -> Result<QuantaIndex, Box<dyn Error>> {
    let (query, control, ingest) = rt
        .socket_paths()
        .ok_or("the daemon must be running to connect the SDK")?;
    Ok(QuantaIndex::connect(
        ConnectOptions::from_state_root(rt.state_root())
            .with_query_socket(query)
            .with_control_socket(control)
            .with_ingest_socket(ingest),
    )?)
}

fn sdk_batch(
    rt: &E2eRuntime,
    generation: u64,
    base_generation: Option<u64>,
    path: &str,
    text: &str,
) -> Result<SearchCorpusBatch<false>, Box<dyn Error>> {
    let manifest_digest = format!("manifest:sdk-preflight:{generation}");
    let batch = match base_generation {
        Some(base) => SearchCorpusBatch::delta(
            rt.repo(),
            rt.revision(),
            ManifestGeneration::new(generation),
            ManifestGeneration::new(base),
            manifest_digest,
        ),
        None => SearchCorpusBatch::replace_generation(
            rt.repo(),
            rt.revision(),
            ManifestGeneration::new(generation),
            manifest_digest,
        ),
    };
    Ok(batch
        .replace_scope(
            SearchScopeKey {
                doc_surface: SearchScopeSurface::File,
                repo_relative_path: RepoRelativePath::new(path),
            },
            format!("scope:{path}"),
            vec![ChunkRecord {
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
            }],
            Vec::new(),
        )
        .without_seal())
}

fn remote_code(error: &SdkError) -> Option<&str> {
    match error {
        SdkError::Remote { code, .. } => Some(code.as_wire_str()),
        SdkError::Usage(_)
        | SdkError::Protocol(_)
        | SdkError::Serialization(_)
        | SdkError::Transport(_) => None,
    }
}

/// Through the SDK: a delta on an unsealed base and a batch past the
/// resource envelope are typed refusals that record nothing; the batch
/// that applies carries the canonical digest the receipt names.
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
    let mut rt = E2eRuntime::boot_with_ingest_resource_policy(policy)?;
    rt.start()?;
    let client = sdk_client(&rt)?;

    let rows_before = idempotency_rows(&rt)?;
    let tree_before = durable_tree(&rt)?;

    let unsealed_base = sdk_batch(&rt, 2, Some(1), "src/sdk_delta.rs", "fn sdk_delta() {}")?;
    let refused = client
        .search_corpus()
        .publish(&unsealed_base)
        .expect_err("a delta on an unsealed base must be refused");
    if remote_code(&refused) != Some(DELTA_BASE_NOT_SEALED.as_wire_str()) {
        return Err(
            format!("expected {DELTA_BASE_NOT_SEALED} through the SDK, got {refused:?}").into(),
        );
    }

    let oversized = sdk_batch(&rt, 1, None, "src/sdk_big.rs", "fn sdk_big() {}")?.replace_scope(
        SearchScopeKey {
            doc_surface: SearchScopeSurface::File,
            repo_relative_path: RepoRelativePath::new("src/sdk_big_two.rs"),
        },
        "scope:sdk_big_two",
        vec![ChunkRecord {
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
        }],
        Vec::new(),
    );
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

    if idempotency_rows(&rt)? != rows_before {
        return Err("SDK refusals must leave no idempotency record".into());
    }
    if durable_tree(&rt)? != tree_before {
        return Err("SDK refusals must change no bytes under the state root".into());
    }

    let fits = sdk_batch(&rt, 1, None, "src/sdk_fits.rs", "fn sdk_fits() {}")?;
    let receipt = client.search_corpus().publish(&fits)?;
    if !receipt.applied || receipt.batch_digest != fits.batch_digest()? {
        return Err(format!(
            "the applied batch must be recorded under its canonical digest: {receipt:?}"
        )
        .into());
    }
    if idempotency_rows(&rt)? != rows_before.saturating_add(1) {
        return Err("the applied batch must be the one new record".into());
    }
    // The same batch again is the recorded apply, through the SDK too.
    let replay = client.search_corpus().publish(&fits)?;
    if replay.applied || replay.durable_sequence != receipt.durable_sequence {
        return Err(format!("an SDK resend must replay the record: {replay:?}").into());
    }
    Ok(())
}
