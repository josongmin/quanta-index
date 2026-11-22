//! QI-BB-021 — a batch outside the ingest resource envelope is refused
//! typed with zero bytes changed, and the envelope does not touch batches
//! that fit.
//!
//! A pinned old-Q process runs under a deliberately tight vector envelope.
//! The typed SDK refusal, absent generation directories, and a served fitting
//! batch are observations outside the daemon implementation.

#![forbid(unsafe_code)]

use std::error::Error;
use std::path::Path;

use quanta_index_contract::{
    GenerationPin, ManifestGeneration, RepoId, RevisionId, SourcePublicationEvent,
};
use quanta_index_core::{
    GenerationStorageKeyV1, INGEST_RESOURCE_BUDGET_EXCEEDED_CODE, IngestResourcePolicy,
};
use quanta_index_sdk::SdkError;
use quanta_index_searchd_harness::{
    legacy_reference::{LegacySearchdBinary, SearchdBinaryProcess, legacy_source_batch},
    private_tempdir,
};

type TestResult = Result<(), Box<dyn Error>>;

/// The hash embedder produces one vector of this size per source record.
fn one_record_vector_bytes() -> Result<u64, Box<dyn Error>> {
    Ok(
        u64::try_from(quanta_index_search_plane::SEARCH_OWNED_SEMANTIC_DIMENSION)?
            .saturating_mul(4),
    )
}

fn pair_dir(
    state_root: &Path,
    track_root: &str,
    repo: &RepoId,
    revision: &RevisionId,
) -> std::path::PathBuf {
    state_root
        .join(track_root)
        .join(GenerationStorageKeyV1::for_repo_revision(repo, revision).as_str())
}

fn source_event(sequence: u64) -> SourcePublicationEvent {
    SourcePublicationEvent {
        stream_id: "e2e-harness-source-v1".into(),
        event_id: format!("e2e-source-event-{sequence}"),
        expected_base_event_id: None,
        payload_sha256: [0; 32],
    }
}

/// Under an envelope that admits one record's vectors but not two, a
/// two-record batch is refused typed before either track writes, and a
/// one-record batch still applies, seals and serves.
#[test]
fn a_batch_past_the_vector_envelope_is_refused_before_any_track_writes() -> TestResult {
    let policy = IngestResourcePolicy::new(
        usize::MAX,
        IngestResourcePolicy::DEFAULT.max_text_bytes(),
        one_record_vector_bytes()?
            .saturating_mul(2)
            .saturating_sub(1),
    )?;
    let parent = private_tempdir()?;
    let state_root = parent.path().join("state");
    let binary = LegacySearchdBinary::from_environment()?;
    let mut command = binary.command()?;
    let _serve = command.arg("serve").arg("--state-root").arg(&state_root);
    let _configured = command.apply_searchd_env(8);
    let _policy = command
        .env(
            "QUANTA_INDEX_INGEST_MAX_RECORDS",
            policy.max_records().to_string(),
        )
        .env(
            "QUANTA_INDEX_INGEST_MAX_TEXT_BYTES",
            policy.max_text_bytes().to_string(),
        )
        .env(
            "QUANTA_INDEX_INGEST_MAX_VECTOR_BYTES",
            policy.max_vector_bytes().to_string(),
        );
    let process = SearchdBinaryProcess::start_with_pinned_command(&state_root, command)?;

    let outcome = (|| -> TestResult {
        let client = process.connect()?;
        let repo = RepoId::new("repo-e2e")?;
        let revision = RevisionId::new("rev-e2e")?;
        let pin = GenerationPin::new(repo.clone(), revision.clone(), ManifestGeneration::new(1));
        let first = (
            "src/envelope.rs",
            "fn envelope_body() { envelope_needle }",
            1,
        );
        let second = (
            "src/envelope_two.rs",
            "fn envelope_second() { envelope_second_needle }",
            2,
        );
        // The original fixture issued two draft sequence numbers before
        // stamping the combined body as event 3.
        let oversized = legacy_source_batch(&repo, &revision, 1, None, &[first, second])?
            .source_event(source_event(3));
        let refused = client.search_corpus().publish_outcome(&oversized);
        match refused {
            Err(SdkError::Remote { code, .. })
                if code.as_wire_str() == INGEST_RESOURCE_BUDGET_EXCEEDED_CODE.as_wire_str() => {}
            other => {
                return Err(format!(
                    "a two-record batch under a one-record envelope must be refused typed, got {other:?}"
                )
                .into());
            }
        }
        for track in ["indexes/lexical", "indexes/semantic"] {
            let pair = pair_dir(&state_root, track, &repo, &revision);
            if pair.exists() {
                return Err(format!(
                    "a refused batch must leave no generation on disk, found {}",
                    pair.display()
                )
                .into());
            }
        }

        // The refused publication did not advance the durable source stream.
        // The fitting batch uses the original target generation.
        let fits =
            legacy_source_batch(&repo, &revision, 1, None, &[first])?.source_event(source_event(4));
        let accepted = client.search_corpus().publish_outcome(&fits)?;
        if !accepted.receipt.applied || accepted.receipt.durable_sequence == 0 {
            return Err(format!("a fitting batch must apply durably, got {accepted:?}").into());
        }
        // Preserve the original second publish of the same fitting body.
        let _replayed = client.search_corpus().publish_outcome(&fits)?;
        let evidence = quanta_index_sdk::PublishedBatchEvidence::from(&accepted);
        let activated = client.search_corpus().activate_published(&evidence, None)?;
        let active = client
            .generations()
            .active_head(repo.clone(), revision.clone())?
            .ok_or("fitting batch activation has no observed active head")?;
        if active != activated.active
            || active.generation.lexical.manifest_generation != pin.manifest_generation
        {
            return Err("fitting batch activation selected another generation".into());
        }
        let served = client
            .lexical()
            .query()
            .native("envelope_needle")
            .active(repo, revision)
            .top_k(5)
            .execute()?;
        if served.generation != pin || served.results.len() != 1 {
            return Err(format!(
                "the fitting batch must serve one chunk from generation 1, got {served:?}"
            )
            .into());
        }
        Ok(())
    })();
    let stopped = process.stop();
    outcome.and(stopped)?;
    binary.verify()
}

// NOTE (TOPT-07): the record-ceiling daemon case lived here and was
// deleted after its four stronger owners passed: the core policy unit
// test, the search-plane preflight/publish proofs, the config
// field-to-policy mapping proof, and the vector-envelope daemon E2E
// above, which keeps the one daemon-level envelope refusal.
