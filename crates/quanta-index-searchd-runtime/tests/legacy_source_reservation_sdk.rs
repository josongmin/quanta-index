//! A pinned old-Q process preserves Source allocation across three OS lifetimes.

#![forbid(unsafe_code)]
#![expect(
    clippy::panic_in_result_fn,
    reason = "the L2 process oracle asserts exact reservation and activation identities"
)]

use std::error::Error;
use std::time::{Duration, Instant};

use quanta_index_contract::{
    BatchIngestMode, ManifestGeneration, RepoId, RevisionId,
    SearchCorpusGenerationReservationRequestV1, SearchCorpusIngestBatch, SourcePublicationEvent,
    source_event_payload_sha256,
};
use quanta_index_sdk::{ConnectOptions, PublishedBatchEvidence, QuantaIndex, SearchCorpusBatch};
use quanta_index_searchd_harness::legacy_reference::{LegacySearchdBinary, SearchdBinaryProcess};

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

#[test]
fn reserved_generation_survives_restart_and_requires_explicit_activation() -> TestResult {
    let binary = LegacySearchdBinary::from_environment()?;
    // Preserve the original deep Source owner path on Linux.
    let prefix = if cfg!(target_os = "linux") {
        "source-owner-".repeat(13)
    } else {
        "qi-reserve-l2-".to_owned()
    };
    let root = tempfile::Builder::new()
        .prefix(&prefix)
        .tempdir_in("/tmp")?;
    let state = root.path().join("state");
    let start = || -> TestResult<(SearchdBinaryProcess, QuantaIndex)> {
        let mut command = binary.benchmark_command(&state, 4)?;
        let _configured = command
            .env("QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_BYTES", "67108864")
            .env("QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_REVISION_PAIRS", "8")
            .env(
                "QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_TOTAL_BYTES",
                "536870912",
            );
        let limit = Duration::from_secs(20);
        let deadline = Instant::now() + limit;
        let process =
            SearchdBinaryProcess::start_with_pinned_command_and_timeout(&state, command, limit)?;
        let probe = QuantaIndex::connect_legacy(
            ConnectOptions::from_state_root(&state)
                .with_request_io_timeout(Duration::from_millis(200)),
        )?;
        loop {
            match probe.observability().process_readiness() {
                Ok(report) if report.ready => break,
                Ok(report) if Instant::now() >= deadline => {
                    return Err(format!("old-Q remains unready: {report:?}").into());
                }
                Err(error) if Instant::now() >= deadline => return Err(error.into()),
                Ok(_) | Err(_) => std::thread::sleep(Duration::from_millis(20)),
            }
        }
        let client = QuantaIndex::connect_legacy(
            ConnectOptions::from_state_root(&state)
                .with_request_io_timeout(Duration::from_secs(15)),
        )?;
        Ok((process, client))
    };
    let repo = RepoId::new("l2-process")?;
    let revision = RevisionId::new("original")?;

    // Source intent excludes the not-yet-allocated physical target.
    let draft = SearchCorpusIngestBatch {
        source_event: SourcePublicationEvent {
            stream_id: "l2-process-stream".into(),
            event_id: "reserved-first".into(),
            expected_base_event_id: None,
            payload_sha256: [0; 32],
        },
        repo_id: repo.clone(),
        revision_id: revision.clone(),
        generation: ManifestGeneration::new(0),
        base_generation: None,
        manifest_digest: "unallocated".into(),
        batch_digest: String::new(),
        mode: BatchIngestMode::ReplaceGeneration,
        bundle_payload: None,
        clear_surfaces: Vec::new(),
        replace_scopes: Vec::new(),
        tombstone_scopes: Vec::new(),
        semantic_replace_scopes: Vec::new(),
        semantic_tombstone_scopes: Vec::new(),
        seal: true,
    };
    let mut event = draft.source_event.clone();
    event.payload_sha256 = source_event_payload_sha256(&draft)?;
    let request = SearchCorpusGenerationReservationRequestV1 {
        repo_id: repo.clone(),
        revision_id: revision.clone(),
        source_event: event.clone(),
    };

    let (process, client) = start()?;
    let allocation_result = (|| -> TestResult<_> {
        let allocation = client.search_corpus().reserve_generation(&request)?;
        assert_eq!(allocation.generation, ManifestGeneration::new(1));
        assert_eq!(
            client.search_corpus().reserve_generation(&request)?,
            allocation
        );
        assert!(
            client
                .generations()
                .active_head(repo.clone(), revision.clone())?
                .is_none()
        );
        Ok(allocation)
    })();
    let stopped = process.stop();
    let allocation = allocation_result?;
    stopped?;
    binary.verify()?;

    let (process, client) = start()?;
    let activation_result = (|| -> TestResult<_> {
        assert_eq!(
            client.search_corpus().reserve_generation(&request)?,
            allocation
        );
        let batch = SearchCorpusBatch::replace_generation(
            repo.clone(),
            revision.clone(),
            allocation.generation,
            "manifest:reserved-first",
        )
        .source_event(SourcePublicationEvent {
            stream_id: "l2-process-stream".into(),
            event_id: "reserved-first".into(),
            expected_base_event_id: None,
            payload_sha256: [0; 32],
        });
        let published = client.search_corpus().publish_outcome(&batch)?;
        assert_eq!(
            published.publication.target.manifest_generation,
            allocation.generation
        );
        assert!(
            client
                .generations()
                .active_head(repo.clone(), revision.clone())?
                .is_none()
        );
        let evidence = PublishedBatchEvidence::from(&published);
        let activated = client.search_corpus().activate_published(&evidence, None)?;
        assert_eq!(
            activated.active.generation.lexical.manifest_generation,
            allocation.generation
        );
        Ok(activated)
    })();
    let stopped = process.stop();
    let activated = activation_result?;
    stopped?;
    binary.verify()?;

    let (process, client) = start()?;
    let result = (|| -> TestResult {
        assert_eq!(
            client.search_corpus().reserve_generation(&request)?,
            allocation
        );
        assert_eq!(
            client.generations().active_head(repo, revision)?,
            Some(activated.active)
        );
        Ok(())
    })();
    let stopped = process.stop();
    result.and(stopped)?;
    binary.verify()
}
