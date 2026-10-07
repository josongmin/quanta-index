//! SDK -> UDS -> production provider/cache -> persisted semantic adapter.
//!
//! Only HTTP I/O is replaced. Fixed orthogonal vectors define the ranking
//! oracle independently of the cache, provider normalization and query path.

#![expect(
    clippy::panic_in_result_fn,
    reason = "Result-returning integration steps assert fixed public-contract invariants"
)]

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, ensure};
use quanta_index_contract::{
    ChunkId, ChunkRecord, GenerationPin, ManifestGeneration, MetricsSnapshotV1, RepoId,
    RepoRelativePath, RevisionId, SearchCorpusActiveHeadV1, SemanticQueryRequest,
    SemanticQueryResponse, SourceFileKey, SourcePublicationEvent, lex::LanguageCode,
};
use quanta_index_core::CoreError;
use quanta_index_sdk::{ConnectOptions, QuantaIndex, SdkError, SearchCorpusBatch};
use quanta_index_searchd::app::config::{OpenAiEmbedderTuning, ProviderEgressGrantConfig};
use quanta_index_searchd::app::runtime::test_provider_transport::{
    EmbeddingTransport, HttpResponse,
};
use quanta_index_searchd::app::{KernelResidentMemoryProbe, SemanticEmbedderProfile};
use quanta_index_searchd_harness::{
    fixture_source_scope_v1, private_tempdir, semantic_source_scopes_for_chunk_records,
};

use super::{SearchdConfig, SearchdRuntime, build_runtime_with_assembly};

const NORTH: &str = "FooBar";
const EAST: &str = "foobar";
const QUERY_CASES: [(&str, &str); 4] = [
    (NORTH, "north"),
    (EAST, "east"),
    ("foo_bar", "north"),
    ("foo bar", "north"),
];
const BASE_MODEL: &str = "fixture-model";
const BASE_REVISION: &str = "r1";
const WAIT: Duration = Duration::from_secs(10);

#[derive(Clone)]
struct FixedTransport {
    model: &'static str,
    rotated: bool,
    calls: Arc<Mutex<Vec<Vec<String>>>>,
}

impl FixedTransport {
    fn call_count(&self) -> Result<usize> {
        Ok(self.calls.lock().map_err(|error| anyhow!("{error}"))?.len())
    }

    fn texts(&self) -> Result<Vec<String>> {
        Ok(self
            .calls
            .lock()
            .map_err(|error| anyhow!("{error}"))?
            .iter()
            .flatten()
            .cloned()
            .collect())
    }
}

impl EmbeddingTransport for FixedTransport {
    fn post_embeddings(
        &self,
        url: &str,
        _api_key: &str,
        body: &str,
        _timeout: Duration,
    ) -> std::result::Result<HttpResponse, CoreError> {
        let invalid = |message: String| CoreError::InvalidContract(message);
        let request: serde_json::Value =
            serde_json::from_str(body).map_err(|error| invalid(error.to_string()))?;
        if url != "https://api.openai.com/v1/embeddings"
            || request.get("model").and_then(serde_json::Value::as_str) != Some(self.model)
            || request
                .get("dimensions")
                .and_then(serde_json::Value::as_u64)
                != Some(3)
        {
            return Err(invalid("unexpected provider request identity".into()));
        }
        let texts: Vec<String> = serde_json::from_value(
            request
                .get("input")
                .ok_or_else(|| invalid("provider input missing".into()))?
                .clone(),
        )
        .map_err(|error| invalid(error.to_string()))?;
        let data = texts
            .iter()
            .enumerate()
            .map(|(index, text)| {
                let vector = match (text.as_str(), self.rotated) {
                    ("north document", false) | ("east document", true) => [2.0, 0.0, 0.0],
                    ("east document", false) | ("north document", true) => [0.0, 3.0, 0.0],
                    (NORTH | "foo_bar" | "foo bar", _) => [4.0, 0.0, 0.0],
                    (EAST, _) => [0.0, 5.0, 0.0],
                    _ => return Err(invalid(format!("unexpected provider text: {text}"))),
                };
                Ok(serde_json::json!({"index": index, "embedding": vector}))
            })
            .collect::<std::result::Result<Vec<_>, _>>()?;
        self.calls
            .lock()
            .map_err(|error| invalid(error.to_string()))?
            .push(texts);
        Ok(HttpResponse {
            status: 200,
            body: serde_json::json!({"data": data}).to_string(),
        })
    }
}

struct Running {
    shutdown: Arc<AtomicBool>,
    join: Option<JoinHandle<Result<()>>>,
    client: QuantaIndex,
    transport: FixedTransport,
}

impl Running {
    fn start(
        root: &Path,
        model: &'static str,
        revision: &str,
        cache_enabled: bool,
    ) -> Result<Self> {
        let transport = FixedTransport {
            model,
            rotated: model != BASE_MODEL || revision != BASE_REVISION,
            calls: Arc::new(Mutex::new(Vec::new())),
        };
        let profile = SemanticEmbedderProfile::OpenAi {
            model: model.into(),
            model_revision: revision.into(),
            dimension: 3,
            api_key: "fixture-key".into(),
            tuning: OpenAiEmbedderTuning {
                cache_enabled,
                max_retries: 0,
                concurrency: 1,
                ..OpenAiEmbedderTuning::default()
            },
        };
        let config = SearchdConfig::from_test_state_root(root.to_path_buf())
            .try_with_search_corpus_history_retention_limits(8, 16 << 20, 128, 256 << 20)?
            .with_semantic_embedder_profile(profile)
            .with_provider_egress_grant(ProviderEgressGrantConfig {
                tenant_id: "fixture-tenant".into(),
                endpoint: "https://api.openai.com/v1/embeddings".into(),
                region: "fixture-region".into(),
                retention: "fixture-retention".into(),
                profile: "fixture-profile".into(),
                source_content_consent: true,
            });
        let http = transport.clone();
        let runtime = build_runtime_with_assembly(
            config,
            Arc::new(KernelResidentMemoryProbe),
            |builder| builder,
            |_| {},
            move |config, parts| {
                SearchdRuntime::assemble_with_embedding_transport(config, parts, Box::new(http))
            },
        )?;
        let client = QuantaIndex::connect(ConnectOptions::from_state_root(root))?;
        let shutdown = Arc::new(AtomicBool::new(false));
        let signal = Arc::clone(&shutdown);
        let join = thread::spawn(move || quanta_index_searchd::drive(runtime, &signal));
        let running = Self {
            shutdown,
            join: Some(join),
            client,
            transport,
        };
        let started = Instant::now();
        loop {
            if running.client.observability().metrics_snapshot().is_ok() {
                return Ok(running);
            }
            ensure!(
                started.elapsed() < WAIT,
                "cache fixture UDS start timed out"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn stop(mut self) -> Result<()> {
        self.shutdown.store(true, Ordering::Release);
        self.join
            .take()
            .ok_or_else(|| anyhow!("runtime driver already joined"))?
            .join()
            .map_err(|payload| anyhow!("runtime driver panicked: {payload:?}"))??;
        Ok(())
    }

    fn query(
        &self,
        text: &str,
        generation: u64,
    ) -> std::result::Result<SemanticQueryResponse, SdkError> {
        self.client.semantic().query_request(SemanticQueryRequest {
            query_text: text.into(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: Some(pin(generation)),
            generation_selector: None,
            lexical_scope: None,
            top_k: 2,
        })
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        if let Some(join) = self.join.take() {
            let _outcome = join.join();
        }
    }
}

fn pin(generation: u64) -> GenerationPin {
    GenerationPin::new(
        RepoId::new("repo-cache").expect("fixed repo ID"),
        RevisionId::new("rev-cache").expect("fixed revision ID"),
        ManifestGeneration::new(generation),
    )
}

fn publish(
    running: &Running,
    generation: u64,
    active: Option<SearchCorpusActiveHeadV1>,
) -> Result<SearchCorpusActiveHeadV1> {
    let mut batch = SearchCorpusBatch::replace_generation(
        pin(generation).repo_id,
        pin(generation).revision_id,
        ManifestGeneration::new(generation),
        format!("manifest:cache-g{generation}"),
    )
    .source_event(SourcePublicationEvent {
        stream_id: "fixture:cache".into(),
        event_id: format!("fixture:cache-g{generation}"),
        expected_base_event_id: generation
            .checked_sub(1)
            .filter(|base| *base > 0)
            .map(|base| format!("fixture:cache-g{base}")),
        payload_sha256: [0; 32],
    });
    for (id, text) in [("north", "north document"), ("east", "east document")] {
        let scope = fixture_source_scope_v1(
            SourceFileKey {
                source_repo_id: pin(generation).repo_id,
                repo_relative_path: RepoRelativePath::new(format!("src/{id}.txt")),
            },
            pin(generation).revision_id,
            vec![ChunkRecord {
                chunk_id: ChunkId::new(id),
                repo_relative_path: RepoRelativePath::new(format!("src/{id}.txt")),
                language: LanguageCode::new("text")
                    .map_err(|error| anyhow!("fixture language: {error}"))?,
                start_byte: 0,
                end_byte: u32::try_from(text.len())?,
                start_line: 1,
                end_line: 1,
                text: text.into(),
                structural: None,
                parent_chunk_id: None,
                source_repo_id: Some(pin(generation).repo_id),
            }],
            Vec::new(),
        )?;
        for semantic in semantic_source_scopes_for_chunk_records(&scope.chunks) {
            batch = batch.replace_semantic_scope(
                semantic.scope,
                semantic.scope_digest,
                semantic.sources,
                semantic.cluster_memberships,
            );
        }
        batch = batch.replace_scope(
            scope.coverage,
            scope.source_bytes,
            scope.chunks,
            scope.symbols,
        );
    }
    let (_receipt, activation) = running
        .client
        .search_corpus()
        .publish_and_activate(&batch, active)?;
    Ok(activation.active)
}

fn counter(snapshot: &MetricsSnapshotV1, name: &str) -> Result<u64> {
    snapshot
        .counters
        .iter()
        .find(|counter| counter.name == name)
        .map(|counter| counter.value)
        .ok_or_else(|| anyhow!("required cache counter absent: {name}"))
}

fn signature(response: &SemanticQueryResponse) -> Vec<(String, u32)> {
    response
        .results
        .iter()
        .map(|row| (row.candidate_id.clone(), row.score.to_bits()))
        .collect()
}

fn cached_query(
    running: &Running,
    text: &str,
    generation: u64,
    miss: bool,
    first: &str,
) -> Result<SemanticQueryResponse> {
    let before = running.client.observability().metrics_snapshot()?;
    let calls = running.transport.call_count()?;
    let response = running.query(text, generation)?;
    let after = running.client.observability().metrics_snapshot()?;
    assert_eq!(response.generation, pin(generation));
    let (first, second) = match first {
        "north" => ("north", "east"),
        "east" => ("east", "north"),
        other => return Err(anyhow!("unknown fixture ranking: {other}")),
    };
    assert_eq!(
        signature(&response),
        vec![
            (first.into(), 1.0_f32.to_bits()),
            (second.into(), 0.0_f32.to_bits()),
        ],
        "orthogonal unit vectors have fixed cosine scores and exact identities"
    );
    assert_eq!(
        running
            .transport
            .call_count()?
            .checked_sub(calls)
            .ok_or_else(|| anyhow!("provider call count decreased"))?,
        usize::from(miss)
    );
    assert_eq!(
        counter(&after, "embedding_cache_misses_total")?
            .checked_sub(counter(&before, "embedding_cache_misses_total")?)
            .ok_or_else(|| anyhow!("cache miss counter decreased"))?,
        u64::from(miss)
    );
    assert_eq!(
        counter(&after, "embedding_cache_hits_total")?
            .checked_sub(counter(&before, "embedding_cache_hits_total")?)
            .ok_or_else(|| anyhow!("cache hit counter decreased"))?,
        u64::from(!miss)
    );
    Ok(response)
}

#[test]
fn sdk_cache_is_text_scoped_and_survives_reopen() -> Result<()> {
    let root = private_tempdir()?;
    let running = Running::start(root.path(), BASE_MODEL, BASE_REVISION, true)?;
    let _active = publish(&running, 1, None)?;
    let mut expected = Vec::new();
    for (text, first) in QUERY_CASES {
        let cold = cached_query(&running, text, 1, true, first)?;
        let warm = cached_query(&running, text, 1, false, first)?;
        assert_eq!(signature(&cold), signature(&warm));
        expected.push(signature(&cold));
    }
    for (text, _) in QUERY_CASES {
        assert_eq!(
            running
                .transport
                .texts()?
                .iter()
                .filter(|input| input.as_str() == text)
                .count(),
            1
        );
    }
    running.stop()?;
    let reopened = Running::start(root.path(), BASE_MODEL, BASE_REVISION, true)?;
    for ((text, first), expected) in QUERY_CASES.into_iter().zip(&expected) {
        let response = cached_query(&reopened, text, 1, false, first)?;
        assert_eq!(&signature(&response), expected);
    }
    assert_eq!(
        reopened.transport.call_count()?,
        0,
        "persisted query vectors must survive teardown/reopen"
    );
    reopened.stop()?;

    // An independently uncached run has exactly the same ranked identities
    // and score bits; a test-only cache cannot manufacture the warm answer.
    let uncached_root = private_tempdir()?;
    let uncached = Running::start(uncached_root.path(), BASE_MODEL, BASE_REVISION, false)?;
    let _active = publish(&uncached, 1, None)?;
    for ((text, _), expected) in QUERY_CASES.into_iter().zip(expected) {
        for _ in 0..2 {
            assert_eq!(signature(&uncached.query(text, 1)?), expected);
        }
    }
    for (text, _) in QUERY_CASES {
        assert_eq!(
            uncached
                .transport
                .texts()?
                .iter()
                .filter(|input| input.as_str() == text)
                .count(),
            2
        );
    }
    uncached.stop()
}

#[test]
fn sdk_cache_model_and_revision_rotation_preserve_separate_namespaces() -> Result<()> {
    for (model, revision) in [(BASE_MODEL, "r2"), ("fixture-other-model", BASE_REVISION)] {
        let root = private_tempdir()?;
        let original = Running::start(root.path(), BASE_MODEL, BASE_REVISION, true)?;
        let active = publish(&original, 1, None)?;
        let old = cached_query(&original, NORTH, 1, true, "north")?;
        original.stop()?;

        let rotated = Running::start(root.path(), model, revision, true)?;
        match rotated.query(NORTH, 1) {
            Err(SdkError::Remote { code, .. }) => {
                assert_eq!(code.as_wire_str(), "SEM_MODEL_MISMATCH");
            }
            other => {
                return Err(anyhow!(
                    "old pin must refuse a different model/revision: {other:?}"
                ));
            }
        }
        // The public contract embeds before model gating so provider failures
        // retain precedence. Even this rejected query must use the new
        // namespace; silently reading the old cache would skip the transport.
        assert_eq!(rotated.transport.texts()?, vec![NORTH.to_string()]);
        let _new_active = publish(&rotated, 2, Some(active))?;
        let corpus_texts = rotated.transport.texts()?;
        assert_eq!(
            corpus_texts
                .iter()
                .filter(|text| text.as_str() == "north document")
                .count(),
            1
        );
        assert_eq!(
            corpus_texts
                .iter()
                .filter(|text| text.as_str() == "east document")
                .count(),
            1
        );
        let cold = cached_query(&rotated, EAST, 2, true, "north")?;
        let warm = cached_query(&rotated, EAST, 2, false, "north")?;
        assert_eq!(signature(&cold), signature(&warm));
        let rotated_north = cached_query(&rotated, NORTH, 2, false, "east")?;
        assert_ne!(signature(&old), signature(&rotated_north));
        rotated.stop()?;

        let reopened = Running::start(root.path(), model, revision, true)?;
        assert_eq!(
            signature(&cached_query(&reopened, NORTH, 2, false, "east")?),
            signature(&rotated_north)
        );
        assert_eq!(reopened.transport.call_count()?, 0);
        reopened.stop()?;

        let restored = Running::start(root.path(), BASE_MODEL, BASE_REVISION, true)?;
        assert_eq!(
            signature(&cached_query(&restored, NORTH, 1, false, "north")?),
            signature(&old)
        );
        assert_eq!(
            restored.transport.call_count()?,
            0,
            "rotating back must reuse only the old namespace"
        );
        restored.stop()?;
    }
    Ok(())
}
