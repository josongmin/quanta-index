//! QI-BB-030: sealed lexical overlays survive a pinned Legacy daemon restart.
//! Reservation, overlay ingest, Source publication and queries use the SDK
//! against one real daemon state root.

#![forbid(unsafe_code)]

use std::error::Error;
use std::path::PathBuf;

use quanta_index_contract::{
    ChunkId, ChunkRecord, GenerationPin, ManifestGeneration, QueryConstraintSetV1, RepoId,
    RepoRelativePath, RevisionId, SearchCorpusActiveHeadV1, SearchPlaneErrorCodeV2,
    TextQueryRequest, TextQueryResponse, TextQuerySyntax, lex::LanguageCode,
};
use quanta_index_core::GenerationStorageKeyV1;
use quanta_index_sdk::{QuantaIndex, RepoMetaBatch, SdkError, SearchCorpusBatch};
use quanta_index_searchd_harness::{
    legacy_reference::{
        LegacySearchdBinary, SearchdBinaryProcess, daemon_socket_paths,
        legacy_source_delta_from_chunks, legacy_source_replacement_from_chunks,
        publish_legacy_source_batch, request_when_ready,
    },
    private_tempdir,
    scale_runtime_policy::DEFAULT_HISTORY_MAX_GENERATIONS,
};

type TestResult = Result<(), Box<dyn Error>>;

const REPO_META_FILE: &str = "repo-meta.cbor";
fn needle(value: &str) -> String {
    format!("overlayneedle{value}")
}

struct OverlayFixture {
    process: Option<SearchdBinaryProcess>,
    binary: LegacySearchdBinary,
    root: tempfile::TempDir,
    repo: RepoId,
    revision: RevisionId,
    active: Option<SearchCorpusActiveHeadV1>,
}

impl OverlayFixture {
    fn new() -> Result<Self, Box<dyn Error>> {
        let root = private_tempdir()?;
        let binary = LegacySearchdBinary::from_environment()?;
        let mut fixture = Self {
            process: None,
            binary,
            root,
            repo: RepoId::new("repo-e2e")?,
            revision: RevisionId::new("rev-e2e")?,
            active: None,
        };
        fixture.start()?;
        Ok(fixture)
    }

    fn start(&mut self) -> TestResult {
        if self.process.is_some() {
            return Err("Legacy overlay daemon is already running".into());
        }
        let command = self
            .binary
            .benchmark_command(self.root.path(), DEFAULT_HISTORY_MAX_GENERATIONS)?;
        self.process = Some(SearchdBinaryProcess::start_with_pinned_command(
            self.root.path(),
            command,
        )?);
        Ok(())
    }

    fn stop(&mut self) -> TestResult {
        self.process
            .take()
            .ok_or("Legacy overlay daemon is not running")?
            .stop()?;
        Ok(())
    }

    fn restart(&mut self) -> TestResult {
        self.stop()?;
        self.start()
    }

    fn client(&self) -> Result<QuantaIndex, Box<dyn Error>> {
        self.process
            .as_ref()
            .ok_or_else(|| -> Box<dyn Error> { "Legacy overlay daemon is not running".into() })?
            .connect()
    }

    fn pin(&self, generation: ManifestGeneration) -> GenerationPin {
        GenerationPin::new(self.repo.clone(), self.revision.clone(), generation)
    }

    fn lexical_generation_dir(
        &self,
        generation: ManifestGeneration,
    ) -> Result<PathBuf, Box<dyn Error>> {
        let canonical = std::fs::canonicalize(self.root.path())?;
        Ok(canonical
            .join("indexes/lexical")
            .join(GenerationStorageKeyV1::for_repo_revision(&self.repo, &self.revision).as_str())
            .join(format!("g{}", generation.get())))
    }

    fn source_batch(
        &self,
        generation: u64,
        path: &str,
        body: String,
    ) -> Result<SearchCorpusBatch, Box<dyn Error>> {
        // The original harness shares one counter between chunks and IPC:
        // G1 chunk 1, reservation 2, overlay 3, Source 4, active-head 5,
        // activation CAS 6; the next chunk is 7. Source events are 1 and 2.
        let original_chunk_id = if generation == 1 { 1 } else { 7 };
        let chunk = ChunkRecord {
            chunk_id: ChunkId::new(format!("e2e-{original_chunk_id}-{path}")),
            repo_relative_path: RepoRelativePath::new(path),
            language: LanguageCode::new("rust")?,
            start_byte: 0,
            end_byte: u32::try_from(body.len())?,
            start_line: 1,
            end_line: 2,
            text: body.into_boxed_str(),
            structural: None,
            parent_chunk_id: None,
            source_repo_id: None,
        };
        if generation == 1 {
            legacy_source_replacement_from_chunks(
                &self.repo,
                &self.revision,
                generation,
                vec![chunk],
            )
        } else {
            // DELTA carries the previous file into generation two unchanged.
            legacy_source_delta_from_chunks(
                &self.repo,
                &self.revision,
                generation,
                &[],
                &[],
                vec![chunk],
            )
        }
    }

    fn repo_meta_batch(&self, generation: ManifestGeneration, value: &str) -> RepoMetaBatch {
        RepoMetaBatch::new(self.repo.clone(), self.revision.clone(), generation).entry(
            self.repo.clone(),
            "lifecycle",
            value,
        )
    }

    fn reserve(&self, client: &QuantaIndex, batch: &SearchCorpusBatch) -> TestResult {
        let request = batch.generation_reservation_request()?;
        let reserved = client.search_corpus().reserve_generation(&request)?;
        if reserved.generation != batch.generation()
            || reserved.repo_id != self.repo
            || reserved.revision_id != self.revision
            || reserved.source_event != request.source_event
        {
            return Err("Source reservation differs from the canonical batch".into());
        }
        Ok(())
    }

    fn publish_overlay(
        &self,
        client: &QuantaIndex,
        generation: ManifestGeneration,
        value: &str,
    ) -> TestResult {
        let receipt = client
            .history()
            .publish_repo_meta(&self.repo_meta_batch(generation, value))?;
        if !receipt.applied || receipt.durable_sequence == 0 {
            return Err("repo-metadata overlay was not durably published".into());
        }
        Ok(())
    }

    fn publish_source(&mut self, client: &QuantaIndex, batch: &SearchCorpusBatch) -> TestResult {
        let (head, _) = publish_legacy_source_batch(client, batch, self.active.clone())?;
        self.active = Some(head);
        Ok(())
    }

    fn seal_with_overlay(
        &mut self,
        generation: u64,
        path: &str,
        value: &str,
    ) -> Result<ManifestGeneration, Box<dyn Error>> {
        let client = self.client()?;
        let batch =
            self.source_batch(generation, path, format!("fn f() {{ {} }}", needle(value)))?;
        self.reserve(&client, &batch)?;
        self.publish_overlay(&client, batch.generation(), value)?;
        self.publish_source(&client, &batch)?;
        Ok(batch.generation())
    }

    fn finish(mut self) -> TestResult {
        self.stop()?;
        self.binary.verify()?;
        Ok(())
    }
}

fn query_meta(
    client: &QuantaIndex,
    pin: GenerationPin,
    value: &str,
    token: &str,
) -> Result<TextQueryResponse, SdkError> {
    client.lexical().query_request(TextQueryRequest {
        syntax: TextQuerySyntax::Sourcegraph,
        query_text: format!("repo:has.meta(lifecycle:{value}) {}", needle(token)),
        constraints: QueryConstraintSetV1::unconstrained(),
        generation: Some(pin),
        generation_selector: None,
        top_k: 5,
        cursor: None,
    })
}

fn expect_meta_hit(fixture: &OverlayFixture, value: &str, what: &str) -> TestResult {
    let client = fixture.client()?;
    let generation = fixture
        .active
        .as_ref()
        .ok_or("no active Source generation")?
        .generation
        .lexical
        .manifest_generation;
    let pin = fixture.pin(generation);
    let served = request_when_ready(|| query_meta(&client, pin.clone(), value, value))?;
    if served.generation != pin || served.results.len() != 1 {
        return Err(format!(
            "{what}: expected one hit under the sealed overlay at {pin:?}, got {:?}",
            served.results
        )
        .into());
    }
    Ok(())
}

fn expect_remote_code<T>(
    result: Result<T, SdkError>,
    expected: SearchPlaneErrorCodeV2,
) -> TestResult {
    match result {
        Err(SdkError::Remote { code, .. }) if code == expected => Ok(()),
        Err(error) => Err(format!("expected typed {}, got {error}", expected.as_wire_str()).into()),
        Ok(_) => Err(format!("expected typed {}, got success", expected.as_wire_str()).into()),
    }
}

#[test]
fn first_overlay_requires_durable_source_reservation_before_writing() -> TestResult {
    let mut fixture = OverlayFixture::new()?;
    let client = fixture.client()?;
    let batch = fixture.source_batch(
        1,
        "src/reserved.rs",
        format!("fn reserved() {{ {} }}", needle("ready")),
    )?;
    let generation = batch.generation();
    let directory = fixture.lexical_generation_dir(generation)?;
    expect_remote_code(
        client
            .history()
            .publish_repo_meta(&fixture.repo_meta_batch(generation, "unreserved")),
        SearchPlaneErrorCodeV2::NotReady,
    )?;
    if directory.exists() {
        return Err("unreserved overlay created a generation directory".into());
    }

    fixture.reserve(&client, &batch)?;
    fixture.publish_overlay(&client, generation, "ready")?;
    fixture.publish_source(&client, &batch)?;
    expect_meta_hit(&fixture, "ready", "after reserved overlay and seal")?;
    fixture.restart()?;
    expect_meta_hit(&fixture, "ready", "after reserved overlay restart")?;
    fixture.finish()
}

#[test]
fn an_overlay_publish_into_a_sealed_generation_is_refused_typed() -> TestResult {
    let mut fixture = OverlayFixture::new()?;
    let sealed = fixture.seal_with_overlay(1, "src/one.rs", "one")?;
    expect_meta_hit(&fixture, "one", "before the refused publish")?;
    let client = fixture.client()?;
    expect_remote_code(
        client
            .history()
            .publish_repo_meta(&fixture.repo_meta_batch(sealed, "late")),
        SearchPlaneErrorCodeV2::GenerationImmutable,
    )?;
    let overlay = fixture.lexical_generation_dir(sealed)?.join(REPO_META_FILE);
    let after_refusal = std::fs::read(&overlay)?;
    expect_meta_hit(&fixture, "one", "after the refused publish")?;
    let late = request_when_ready(|| query_meta(&client, fixture.pin(sealed), "late", "one"))?;
    if !late.results.is_empty() {
        return Err("the refused publish must not be visible to a query".into());
    }

    fixture.restart()?;
    if std::fs::read(&overlay)? != after_refusal {
        return Err("a restart changed the sealed overlay".into());
    }
    expect_meta_hit(&fixture, "one", "after a restart")?;
    fixture.finish()
}

#[test]
fn a_torn_overlay_is_refused_by_restart_and_by_the_query_door() -> TestResult {
    let mut fixture = OverlayFixture::new()?;
    let first = fixture.seal_with_overlay(1, "src/first.rs", "first")?;
    let second = fixture.seal_with_overlay(2, "src/second.rs", "second")?;
    expect_meta_hit(&fixture, "second", "active before the damage")?;

    fixture.stop()?;
    let first_overlay = fixture.lexical_generation_dir(first)?.join(REPO_META_FILE);
    let original = std::fs::read(&first_overlay)?;
    let half = original.len().div_euclid(2);
    std::fs::write(&first_overlay, original.get(..half).ok_or("short overlay")?)?;
    fixture.start()?;
    expect_meta_hit(&fixture, "second", "active after the inactive damage")?;
    let client = fixture.client()?;
    expect_remote_code(
        query_meta(&client, fixture.pin(first), "first", "first"),
        SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
    )?;
    std::fs::write(&first_overlay, &original)?;

    fixture.stop()?;
    let second_overlay = fixture.lexical_generation_dir(second)?.join(REPO_META_FILE);
    std::fs::write(&second_overlay, b"")?;
    let command = fixture
        .binary
        .benchmark_command(fixture.root.path(), DEFAULT_HISTORY_MAX_GENERATIONS)?;
    let output = crate::legacy_start_probe::wait_for_refusal(&fixture.binary, command)?;
    for socket in daemon_socket_paths(fixture.root.path()) {
        match std::fs::symlink_metadata(&socket) {
            Ok(_) => {
                return Err(format!(
                    "daemon bound socket before refusing torn overlay: {}",
                    socket.display()
                )
                .into());
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    let rendered = String::from_utf8_lossy(&output.stderr);
    if !rendered.contains("ACTIVATION_TARGET_UNOPENABLE")
        || !rendered.contains("GENERATION_SIDECAR_CORRUPT")
    {
        return Err(format!(
            "boot must refuse with the typed activation and sidecar causes: {rendered}"
        )
        .into());
    }
    fixture.binary.verify()?;
    Ok(())
}
