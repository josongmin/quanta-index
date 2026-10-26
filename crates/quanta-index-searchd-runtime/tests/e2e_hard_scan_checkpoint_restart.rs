//! A real daemon resumes a durable hard scan under the original source target.
//!
//! The first 32 immutable runs are deliberately too large for the merge cap;
//! runs 33 and 34 form the first eligible pair. The target uses the staged
//! source-upload route so the exact original body survives a process exit.

use std::error::Error;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::searchd_binary_process::{SearchdBinaryProcess, searchd_command};
use quanta_index_contract::{
    BatchIngestMode, ChunkId, ChunkRecord, DiagnosticAttemptFieldV1, GenerationPin,
    ManifestGeneration, RepoId, RepoRelativePath, RevisionId, SearchCorpusActiveHeadV1,
    SearchCorpusGenerationReservationRequestV1, SearchCorpusGenerationReservationV1,
    SearchCorpusIngestBatch, SearchPlaneIngestIpcRequest, SearchPlaneIngestIpcRequestEnvelope,
    SearchPlaneIngestIpcResponse, SearchPlaneIngestIpcResponseEnvelope, SourceFileKey,
    SourcePublicationBinding, SourcePublicationEvent, SourcePublicationUploadCommit,
    source_event_payload_sha256,
};
use quanta_index_ipc::{
    ClientIoPolicy, for_each_source_publication_upload_part, send_request,
    source_publication_upload_identity, stamp_batch_digest_v1,
};
use quanta_index_sdk::{BatchMode, LanguageCode, QuantaIndex, SdkError, SearchCorpusBatch};
use quanta_index_searchd_harness::{
    fixture_source_scope_v1, semantic_source_scopes_for_chunk_records,
};
use sha2::{Digest as _, Sha256};

type TestResult = Result<(), Box<dyn Error>>;

const REPO: &str = "repo-hard-scan-restart";
const REVISION: &str = "revision-hard-scan-restart";
const STREAM: &str = "fixture:hard-scan-restart";
const BASE_GENERATION: u64 = 34;
const TARGET_GENERATION: u64 = 35;
// The recovery contract pins only active/base and candidate generations.
// All 34 immutable runs remain reachable through the active base root;
// unrelated historical generation directories need not accumulate in setup.
const RETAINED_GENERATIONS: usize = 2;
const CRASH_EXIT_CODE: i32 = 86;

fn source_event(generation: u64) -> SourcePublicationEvent {
    SourcePublicationEvent {
        stream_id: STREAM.to_string(),
        event_id: format!("{STREAM}:{generation}"),
        expected_base_event_id: generation
            .checked_sub(1)
            .filter(|previous| *previous != 0)
            .map(|previous| format!("{STREAM}:{previous}")),
        payload_sha256: [0; 32],
    }
}

fn batch_with_base(
    generation: u64,
    base_generation: Option<u64>,
    event: SourcePublicationEvent,
) -> Result<SearchCorpusBatch, Box<dyn Error>> {
    let path = if generation <= 32 {
        format!("src/g{generation}.rs")
    } else {
        format!("f{generation}")
    };
    // Tail runs contain one filename gram, one content gram and one token;
    // their pair must fit the same 8 KiB cap that excludes the prefix runs.
    let mut text = if generation <= 32 {
        format!("fn needle{generation}() {{}} ")
    } else {
        format!("n{generation}")
    };
    if generation <= 32 {
        // Exercise the 32-run frontier without making retention capacity the
        // admission bottleneck. The fixed checkpoint oracle below still
        // requires every run to be InputCap-ineligible under the 8 KiB cap.
        // Across G1..G32 this ASCII input has at least 95 distinct content
        // grams. Even the page/count Record wire minima (69 + 32 bytes per
        // gram) exceed 8 KiB, before counting their maps or physical leaves.
        for index in 0..32 {
            write!(&mut text, "long_{generation}_{index} ")?;
        }
    }
    let chunk = ChunkRecord {
        chunk_id: ChunkId::new(format!("chunk-g{generation}")),
        repo_relative_path: RepoRelativePath::new(path.clone()),
        language: LanguageCode::new("rust")?,
        start_byte: 0,
        end_byte: u32::try_from(text.len())?,
        start_line: 1,
        end_line: 1,
        text: text.into_boxed_str(),
        structural: None,
        parent_chunk_id: None,
        source_repo_id: None,
    };
    let semantic_scopes = semantic_source_scopes_for_chunk_records(std::slice::from_ref(&chunk));
    let scope = fixture_source_scope_v1(
        SourceFileKey {
            source_repo_id: RepoId::new(REPO)?,
            repo_relative_path: RepoRelativePath::new(path),
        },
        RevisionId::new(REVISION)?,
        vec![chunk],
        Vec::new(),
    )?;
    let repo = RepoId::new(REPO)?;
    let revision = RevisionId::new(REVISION)?;
    let generation_id = ManifestGeneration::new(generation);
    let batch = if let Some(base_generation) = base_generation {
        SearchCorpusBatch::delta(
            repo,
            revision,
            generation_id,
            ManifestGeneration::new(base_generation),
            format!("manifest:hard-scan:g{generation}"),
        )
    } else {
        SearchCorpusBatch::replace_generation(
            repo,
            revision,
            generation_id,
            format!("manifest:hard-scan:g{generation}"),
        )
    };
    let mut batch = batch.source_event(event).replace_scope(
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

fn batch(generation: u64) -> Result<SearchCorpusBatch, Box<dyn Error>> {
    batch_with_base(
        generation,
        generation.checked_sub(1).filter(|previous| *previous != 0),
        source_event(generation),
    )
}

fn wire_batch(batch: &SearchCorpusBatch) -> Result<SearchCorpusIngestBatch, Box<dyn Error>> {
    let mut wire = SearchCorpusIngestBatch {
        source_event: source_event(batch.generation().get()),
        repo_id: batch.repo_id().clone(),
        revision_id: batch.revision_id().clone(),
        generation: batch.generation(),
        base_generation: batch.base_generation(),
        manifest_digest: batch.manifest_digest().to_string(),
        batch_digest: String::new(),
        mode: match batch.mode() {
            BatchMode::ReplaceGeneration => BatchIngestMode::ReplaceGeneration,
            BatchMode::Delta => BatchIngestMode::Delta,
        },
        bundle_payload: None,
        clear_surfaces: batch.clear_surfaces().to_vec(),
        replace_scopes: batch.replace_scopes().to_vec(),
        tombstone_scopes: batch.tombstone_scopes().to_vec(),
        semantic_replace_scopes: batch.semantic_replace_scopes().to_vec(),
        semantic_tombstone_scopes: batch.semantic_tombstone_scopes().to_vec(),
        seal: batch.seal_requested(),
    };
    wire.source_event.payload_sha256 = source_event_payload_sha256(&wire)?;
    stamp_batch_digest_v1(&mut wire)?;
    if wire.batch_digest != batch.batch_digest()? {
        return Err("staged wire digest differs from SDK batch".into());
    }
    Ok(wire)
}

fn reservation_request(
    wire: &SearchCorpusIngestBatch,
) -> SearchCorpusGenerationReservationRequestV1 {
    SearchCorpusGenerationReservationRequestV1 {
        repo_id: wire.repo_id.clone(),
        revision_id: wire.revision_id.clone(),
        source_event: wire.source_event.clone(),
    }
}

fn send_ingest(
    root: &Path,
    request_id: u64,
    payload: SearchPlaneIngestIpcRequest,
) -> Result<SearchPlaneIngestIpcResponse, Box<dyn Error>> {
    let request = SearchPlaneIngestIpcRequestEnvelope {
        request_id,
        payload,
        diagnostic_attempt: DiagnosticAttemptFieldV1::Absent,
    };
    let response: SearchPlaneIngestIpcResponseEnvelope = send_request(
        &root.join("search-plane/ingest.sock"),
        &request,
        ClientIoPolicy::default(),
    )?;
    if response.request_id != request_id {
        return Err("ingest response request ID differs".into());
    }
    Ok(response.payload)
}

fn stage_exact_body(root: &Path, wire: &SearchCorpusIngestBatch) -> TestResult {
    let identity = source_publication_upload_identity(wire)?;
    let mut next_offset = 0_u64;
    for_each_source_publication_upload_part(wire, identity, |part| -> TestResult {
        let end = part
            .offset
            .checked_add(u64::try_from(part.bytes.len())?)
            .ok_or("source upload offset overflow")?;
        let request_id = 1100_u64
            .checked_add(part.offset)
            .ok_or("source upload request ID overflow")?;
        let response = send_ingest(
            root,
            request_id,
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

fn publish_staged(
    root: &Path,
    wire: &SearchCorpusIngestBatch,
) -> Result<SearchPlaneIngestIpcResponse, Box<dyn Error>> {
    let identity = source_publication_upload_identity(wire)?;
    send_ingest(
        root,
        2001,
        SearchPlaneIngestIpcRequest::PublishStagedSourcePublication(
            SourcePublicationUploadCommit {
                identity,
                publication: SourcePublicationBinding::for_batch(wire),
            },
        ),
    )
}

fn checkpoint_sidecar(root: &Path) -> Result<PathBuf, Box<dyn Error>> {
    let attempts = root.join("indexes/lexical/lexical-object-pool/attempts");
    let mut checkpoints = std::fs::read_dir(attempts)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()?;
    checkpoints.retain(|path| {
        path.extension()
            .is_some_and(|extension| extension == "scan")
    });
    if checkpoints.len() != 1 {
        return Err(
            format!("expected one durable hard-scan sidecar, found {checkpoints:?}").into(),
        );
    }
    Ok(checkpoints.remove(0))
}

fn staged_body_path(
    root: &Path,
    wire: &SearchCorpusIngestBatch,
) -> Result<PathBuf, Box<dyn Error>> {
    let identity = source_publication_upload_identity(wire)?;
    let mut digest = String::with_capacity(64);
    for byte in identity.body_sha256 {
        write!(&mut digest, "{byte:02x}")?;
    }
    Ok(root
        .join("source-publication-uploads")
        .join(format!("{digest}-{:016x}.cbor", identity.body_bytes)))
}

fn check_staged_body(path: &Path, wire: &SearchCorpusIngestBatch) -> TestResult {
    let identity = source_publication_upload_identity(wire)?;
    let bytes = std::fs::read(path)?;
    let digest: [u8; 32] = Sha256::digest(&bytes).into();
    if u64::try_from(bytes.len())? != identity.body_bytes || digest != identity.body_sha256 {
        return Err("durable staged body differs from the original input".into());
    }
    Ok(())
}

fn take_checkpoint_field<'a>(
    body: &'a [u8],
    offset: &mut usize,
    count: usize,
) -> Result<&'a [u8], Box<dyn Error>> {
    let end = offset
        .checked_add(count)
        .ok_or("checkpoint offset overflow")?;
    let field = body.get(*offset..end).ok_or("short checkpoint field")?;
    *offset = end;
    Ok(field)
}

fn read_checkpoint_u64(body: &[u8], offset: &mut usize) -> Result<u64, Box<dyn Error>> {
    let field: [u8; 8] = take_checkpoint_field(body, offset, 8)?.try_into()?;
    Ok(u64::from_be_bytes(field))
}

fn check_first_window(
    bytes: &[u8],
    target: &SearchCorpusIngestBatch,
    reservation: &SearchCorpusGenerationReservationV1,
) -> TestResult {
    if reservation.repo_id != target.repo_id
        || reservation.revision_id != target.revision_id
        || reservation.source_event != target.source_event
        || reservation.generation != target.generation
    {
        return Err("reservation differs from the staged source target".into());
    }
    let expected_repo: [u8; 32] = Sha256::digest(REPO.as_bytes()).into();
    let expected_revision: [u8; 32] = Sha256::digest(REVISION.as_bytes()).into();
    let mut encoded_source_event = Vec::new();
    ciborium::into_writer(&target.source_event, &mut encoded_source_event)?;
    let expected_source_event: [u8; 32] = Sha256::digest(&encoded_source_event).into();
    let (body, trailer) = bytes
        .split_at_checked(bytes.len().checked_sub(32).ok_or("short checkpoint")?)
        .ok_or("checkpoint has no checksum")?;
    let mut digest = Sha256::new();
    digest.update(b"lexical-hard-scan-fixed-wire-v1");
    digest.update(body);
    let actual: [u8; 32] = digest.finalize().into();
    if actual.as_slice() != trailer {
        return Err("first-window sidecar checksum differs".into());
    }
    let mut offset = 0_usize;
    macro_rules! take {
        ($count:expr) => {
            take_checkpoint_field(body, &mut offset, $count)?
        };
    }
    macro_rules! read_u64 {
        () => {
            read_checkpoint_u64(body, &mut offset)?
        };
    }
    if take!(8) != b"QLHSCAN1" || take!(2) != [0, 1] {
        return Err("unexpected checkpoint wire header".into());
    }
    if take!(16) != reservation.root_incarnation
        || take!(32) != expected_repo
        || take!(32) != expected_revision
        || take!(32) != expected_source_event
    {
        return Err("first-window attempt differs from the source reservation".into());
    }
    if read_u64!() != TARGET_GENERATION || take!(1) != [1] {
        return Err("checkpoint target or delta base option differs".into());
    }
    let _base_root = take!(32);
    if take!(32) != target.source_event.payload_sha256 {
        return Err("first-window operations differ from the staged source payload".into());
    }
    let _attempt_format = take!(2);
    let _policy = take!(32);
    if take!(1) != [1] || take!(1) != [1] {
        return Err("first checkpoint must hold only the File lane".into());
    }
    let _file_root = take!(32);
    if read_u64!() != BASE_GENERATION || take!(2) != [0, 1] || take!(2) != [0, 0] {
        return Err("File frontier or first-window count differs".into());
    }
    let _prior_chain = take!(32);
    if read_u64!() != 1 || read_u64!() != 33 || read_u64!() != BASE_GENERATION {
        return Err("first window did not freeze and advance the 32-run cursor".into());
    }
    if take!(1) != [0] || take!(1) != [32] {
        return Err("first window is unexpectedly complete or has a wrong width".into());
    }
    for run_id in 1..=32 {
        // This field is the physical-leaf index cost, not the complete
        // merge input. File eligibility also includes the run posting
        // bundle; require its InputCap witness below rather than imposing
        // the complete-input cap on this partial cost.
        if read_u64!() != run_id || read_u64!() == 0 {
            return Err(format!("large run {run_id} has no fixed identity/index cost").into());
        }
        let _header = take!(32);
        if take!(2) != [3, 1] {
            return Err(format!("large run {run_id} was not InputCap-ineligible").into());
        }
    }
    if offset != body.len() {
        return Err("first-window sidecar contains extra candidates or lanes".into());
    }
    Ok(())
}

fn serves(client: &QuantaIndex, generation: u64) -> TestResult {
    let pin = GenerationPin::new(
        RepoId::new(REPO)?,
        RevisionId::new(REVISION)?,
        ManifestGeneration::new(generation),
    );
    for (needle, expected) in [
        ("needle1", "chunk-g1"),
        ("needle32", "chunk-g32"),
        ("n33", "chunk-g33"),
        ("n34", "chunk-g34"),
    ] {
        let query = client
            .lexical()
            .query()
            .native(needle)
            .pinned(pin.clone())
            .top_k(5)
            .execute()?;
        if query.generation != pin
            || query.results.len() != 1
            || query
                .results
                .first()
                .is_none_or(|hit| hit.candidate_id != expected)
        {
            return Err(format!(
                "pinned {generation} failed independent query gold {needle}: {query:?}"
            )
            .into());
        }
    }
    Ok(())
}

fn check_new_doc(client: &QuantaIndex, generation: u64, present: bool) -> TestResult {
    let pin = GenerationPin::new(
        RepoId::new(REPO)?,
        RevisionId::new(REVISION)?,
        ManifestGeneration::new(generation),
    );
    let query = client
        .lexical()
        .query()
        .native("n35")
        .pinned(pin.clone())
        .top_k(5)
        .execute()?;
    let expected = usize::from(present);
    if query.generation != pin
        || query.results.len() != expected
        || (present
            && query
                .results
                .first()
                .is_none_or(|hit| hit.candidate_id != "chunk-g35"))
    {
        return Err(format!("pinned {generation} new-doc gold differs: {query:?}").into());
    }
    Ok(())
}

fn configured_command(root: &Path) -> std::process::Command {
    let mut command = searchd_command(root, RETAINED_GENERATIONS);
    let _configured = command
        .env("QUANTA_INDEX_LEXICAL_MERGE_SOFT_RUNS", "34")
        .env("QUANTA_INDEX_LEXICAL_MERGE_HARD_RUNS", "34")
        .env(
            "QUANTA_INDEX_LEXICAL_MERGE_SOFT_INDEX_BYTES",
            (u64::MAX - 1).to_string(),
        )
        .env(
            "QUANTA_INDEX_LEXICAL_MERGE_HARD_INDEX_BYTES",
            u64::MAX.to_string(),
        )
        .env("QUANTA_INDEX_LEXICAL_MERGE_MAX_FAN_IN", "2")
        .env("QUANTA_INDEX_LEXICAL_MERGE_MAX_INPUT_INDEX_BYTES", "8192")
        .env(
            "QUANTA_INDEX_LEXICAL_STAGING_MAX_BYTES",
            (64 * 1024 * 1024).to_string(),
        )
        .env("QUANTA_INDEX_LEXICAL_STAGING_MAX_AGE_MS", "60000");
    command
}

#[test]
fn staged_target_resumes_first_hard_scan_window_after_daemon_crash() -> TestResult {
    use std::os::unix::fs::PermissionsExt as _;

    let directory = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()?;
    let root = directory.path();
    let seed =
        SearchdBinaryProcess::start_with_history_max_generations(root, RETAINED_GENERATIONS)?;
    let client = seed.connect()?;
    let mut active: Option<SearchCorpusActiveHeadV1> = None;
    for generation in 1..=BASE_GENERATION {
        let (_, ack) = client
            .search_corpus()
            .publish_and_activate(&batch(generation)?, active)
            .map_err(|error| format!("seed generation {generation} publish/activate: {error}"))?;
        active = Some(ack.active);
    }
    serves(&client, BASE_GENERATION)?;
    check_new_doc(&client, BASE_GENERATION, false)?;
    seed.stop()?;

    let target = wire_batch(&batch(TARGET_GENERATION)?)?;
    let request = reservation_request(&target);
    let mut crashing = configured_command(root);
    let _configured = crashing.env("QUANTA_INDEX_CRASH_POINT", "after_hard_scan_first_window");
    let crashing = SearchdBinaryProcess::start_with_command(root, crashing)?;
    let crash_client = crashing.connect()?;
    let original = crash_client.search_corpus().reserve_generation(&request)?;
    if original.generation != target.generation {
        return Err(format!("reservation missed target generation: {original:?}").into());
    }
    stage_exact_body(root, &target)?;
    let upload = staged_body_path(root, &target)?;
    check_staged_body(&upload, &target)?;
    let result = publish_staged(root, &target);
    let status = crashing.wait_for_exit()?;
    if status.code() != Some(CRASH_EXIT_CODE) || result.is_ok() {
        return Err(format!(
            "hard-scan crash did not interrupt staged publish: {status:?}, {result:?}"
        )
        .into());
    }
    let sidecar = checkpoint_sidecar(root)?;
    let first_window_bytes = std::fs::read(&sidecar)?;
    check_first_window(&first_window_bytes, &target, &original)?;
    check_staged_body(&upload, &target)?;

    let resumed = SearchdBinaryProcess::start_with_command(root, configured_command(root))?;
    let client = resumed.connect()?;
    let replay = client.search_corpus().reserve_generation(&request)?;
    if replay != original {
        return Err(format!("restart changed original generation reservation: {replay:?}").into());
    }
    if std::fs::read(&sidecar)? != first_window_bytes {
        return Err("restart changed the durable first-window checkpoint".into());
    }
    check_staged_body(&upload, &target)?;
    serves(&client, BASE_GENERATION)?;
    check_new_doc(&client, BASE_GENERATION, false)?;
    // A second stream cannot commit the G+1 search-corpus authority over
    // the unresolved source target. This fence does not prove that no
    // physical candidate files were written before the refusal.
    // A new stream must start with a full replacement: a delta from G34
    // would fail lineage validation before reaching the pending fence.
    let overtaking = batch_with_base(
        TARGET_GENERATION + 1,
        None,
        SourcePublicationEvent {
            stream_id: "fixture:hard-scan-other-stream".to_string(),
            event_id: "fixture:hard-scan-other-stream:36".to_string(),
            expected_base_event_id: None,
            payload_sha256: [0; 32],
        },
    )?;
    let refused = client.search_corpus().publish(&overtaking);
    if !matches!(&refused, Err(SdkError::Remote { code, .. }) if code.as_wire_str() == "NOT_READY")
    {
        return Err(format!("G+1 did not hit the unresolved-target fence: {refused:?}").into());
    }
    if std::fs::read(&sidecar)? != first_window_bytes {
        return Err("refused G+1 changed the original checkpoint".into());
    }
    check_staged_body(&upload, &target)?;
    serves(&client, BASE_GENERATION)?;
    check_new_doc(&client, BASE_GENERATION, false)?;
    let outcome = publish_staged(root, &target)?;
    let SearchPlaneIngestIpcResponse::SearchCorpusReceipt(outcome) = outcome else {
        return Err(format!("staged retry did not seal target: {outcome:?}").into());
    };
    let receipt = outcome.receipt;
    if !receipt.applied || receipt.batch_digest != target.batch_digest {
        return Err(format!("retry did not apply original body: {receipt:?}").into());
    }
    if upload.exists() {
        return Err("successful staged publication retained its upload".into());
    }
    let again = client
        .search_corpus()
        .publish_outcome(&batch(TARGET_GENERATION)?)?;
    if again.receipt.applied || again.receipt.durable_sequence != receipt.durable_sequence {
        return Err(format!("same body replay did not reuse publication: {again:?}").into());
    }
    let evidence = quanta_index_sdk::PublishedBatchEvidence::from(&again);
    let activated = client
        .search_corpus()
        .activate_published(&evidence, active)?;
    if activated.active.generation.lexical.manifest_generation != target.generation {
        return Err(format!("activation did not publish target: {activated:?}").into());
    }
    serves(&client, TARGET_GENERATION)?;
    check_new_doc(&client, TARGET_GENERATION, true)?;
    resumed.stop()?;
    Ok(())
}
