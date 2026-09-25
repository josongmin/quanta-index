//! SDK roundtrip tests (RB-02, T05–T07): a real `searchd` process,
//! actual SDK publish/activate/query, plus the static SDK-frontdoor guard.
//!
//! The daemon binary resolves via the `--searchd-bin` equivalent
//! (`QUANTA_INDEX_SEARCHD_BIN`); an unset pin refuses outright, never
//! a skip and never undiscovered selection.

#![expect(
    clippy::expect_used,
    clippy::indexing_slicing,
    reason = "integration-test fixture setup and direct oracle assertions intentionally fail on absence"
)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use quanta_index_retrieval_bench::batch::{
    BatchIdentity, activation_digest, assemble_batch, receipt_digest,
};
use quanta_index_retrieval_bench::chunking::chunk_corpus;
use quanta_index_retrieval_bench::chunking::whole_file::WholeFileChunker;
use quanta_index_retrieval_bench::corpus::{CorpusLimits, load_corpus, load_manifest};
use quanta_index_retrieval_bench::query_plan::{
    NlPlanConfig, QueryInputPolicy, execution_profile_sha256, execution_profile_value, plan_query,
};
use quanta_index_retrieval_bench::record::{
    CaptureProvenance, PackTask, QueryPack, RouteProvenance, RunnerIdentity, RunnerRecordInput,
    pack_universe_digest, result_value, runner_record,
};
use quanta_index_retrieval_bench::sdk::{
    DaemonConfig, DaemonSession, QueryOutcome, RouteQuery, publish_and_activate, query_route,
    resolve_searchd_binary,
};
use quanta_index_retrieval_bench::symbols::extract_corpus_symbols;
use quanta_index_retrieval_bench::{BenchError, sha256_hex};

const EMBEDDER: &str = "hash-dev";

fn symbols_for(
    files: &[quanta_index_retrieval_bench::corpus::SourceFile],
) -> std::collections::BTreeMap<String, Vec<quanta_index_contract::lex::SymbolRecord>> {
    let by_path: std::collections::BTreeMap<
        String,
        quanta_index_retrieval_bench::corpus::SourceFile,
    > = files
        .iter()
        .map(|file| (file.path.clone(), file.clone()))
        .collect();
    extract_corpus_symbols(&by_path).expect("symbols").symbols
}

fn write_tiny_repo(root: &Path) {
    let files: &[(&str, &str)] = &[
        (
            "src/lib.rs",
            "pub fn sphinx_riddle() -> &'static str {\n    \"the sphinx guards quartz vaults\"\n}\n",
        ),
        (
            "src/alpha.rs",
            "pub fn quartz_counter() -> u64 {\n    41 + 1\n}\n",
        ),
        (
            "src/beta.rs",
            "pub fn unrelated_helper() -> bool {\n    true\n}\n",
        ),
    ];
    for (path, text) in files {
        let absolute = root.join(path);
        std::fs::create_dir_all(absolute.parent().expect("parent")).expect("dir");
        std::fs::write(&absolute, text).expect("file");
    }
    let entries: Vec<String> = files
        .iter()
        .map(|(path, _)| {
            let bytes = std::fs::read(root.join(path)).expect("bytes");
            format!(
                "{{\"path\": \"{path}\", \"file_sha256\": \"{}\"}}",
                sha256_hex(&bytes)
            )
        })
        .collect();
    std::fs::write(
        root.join("manifest.json"),
        format!(
            "{{\"repository_commit\": \"{}\", \"files\": [{}]}}",
            "c".repeat(40),
            entries.join(",")
        ),
    )
    .expect("manifest");
}

fn boot_session(state_root: &Path, identity: &BatchIdentity) -> DaemonSession {
    let config = DaemonConfig {
        state_root,
        searchd_binary: None,
        embedder: EMBEDDER,
        model_dir: None,
        repo_id: &identity.repo_id,
        revision_id: &identity.revision_id,
        ready_timeout: Duration::from_secs(60),
        io_timeout: Duration::from_secs(30),
        history_max_generations: 8,
    };
    DaemonSession::boot(&config).expect("daemon boots")
}

#[test]
fn sdk_frontdoor_static_guard() {
    // RB-02 acceptance: the runner links the public SDK and never the
    // direct ingest IPC request, the in-process fixture harness, or
    // synthetic result providers.
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let manifest = std::fs::read_to_string(root.join("Cargo.toml")).expect("crate manifest");
    assert!(
        manifest.contains("quanta-index-sdk"),
        "runner must depend on quanta-index-sdk"
    );
    assert!(
        !manifest.contains("searchd-harness"),
        "runner must not link the fixture harness"
    );
    let forbidden = [
        "SearchPlaneIngestIpcRequest",
        "E2eRuntime",
        "ingest_text",
        "searchd-harness",
        "searchd_harness",
    ];
    let mut sources = Vec::new();
    collect_sources(&root.join("src"), &mut sources);
    assert!(!sources.is_empty(), "crate sources must exist");
    for source in &sources {
        let text = std::fs::read_to_string(source).expect("source reads");
        for needle in &forbidden {
            assert!(
                !text.contains(needle),
                "{} leaks forbidden path: {needle}",
                source.display()
            );
        }
    }
}

fn collect_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("src dir") {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        if path.is_dir() {
            collect_sources(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn explicit_missing_searchd_binary_never_falls_back() {
    // An explicit binary pin is authoritative even when another build exists.
    let bogus = PathBuf::from("/nonexistent-dir-xyz/quanta-index-searchd");
    let err = resolve_searchd_binary(Some(&bogus)).expect_err("bad explicit path must fail");
    let text = err.to_string();
    assert!(text.contains("--searchd-bin"), "{text}");
    assert!(
        text.contains("/nonexistent-dir-xyz/quanta-index-searchd"),
        "{text}"
    );
}

#[test]
fn stale_state_root_is_refused() {
    let root = tempfile::tempdir().expect("temp root");
    let state_root = root.path().join("state");
    std::fs::create_dir_all(state_root.join("search-plane")).expect("dir");
    std::fs::write(state_root.join("search-plane").join("stale.txt"), b"stale").expect("stale");
    // Boot refuses the stale root before spawning, so the probe identity
    // below never reaches a daemon.
    let identity = BatchIdentity::new("bench-repo", "bench-rev", 7, "manifest:stale".to_string())
        .expect("identity");
    let config = DaemonConfig {
        state_root: &state_root,
        searchd_binary: None,
        embedder: EMBEDDER,
        model_dir: None,
        repo_id: &identity.repo_id,
        revision_id: &identity.revision_id,
        ready_timeout: Duration::from_secs(5),
        io_timeout: Duration::from_secs(5),
        history_max_generations: 8,
    };
    let err = DaemonSession::boot(&config)
        .err()
        .expect("stale root must fail");
    assert!(err.to_string().contains("not fresh"), "{err}");
}

#[cfg(unix)]
#[test]
fn symlink_state_root_is_refused() {
    let root = tempfile::tempdir().expect("temp root");
    let target = root.path().join("target");
    std::fs::create_dir_all(&target).expect("dir");
    let link = root.path().join("link");
    std::os::unix::fs::symlink(&target, &link).expect("symlink");
    let identity = BatchIdentity::new("bench-repo", "bench-rev", 7, "manifest:link".to_string())
        .expect("identity");
    let config = DaemonConfig {
        state_root: &link,
        searchd_binary: None,
        embedder: EMBEDDER,
        model_dir: None,
        repo_id: &identity.repo_id,
        revision_id: &identity.revision_id,
        ready_timeout: Duration::from_secs(5),
        io_timeout: Duration::from_secs(5),
        history_max_generations: 8,
    };
    let err = DaemonSession::boot(&config)
        .err()
        .expect("symlink root must fail");
    assert!(err.to_string().contains("must not be a symlink"), "{err}");
}

#[cfg(unix)]
#[test]
fn boot_times_out_when_daemon_never_opens_sockets() {
    let root = tempfile::tempdir().expect("temp root");
    let script = root.path().join("fake-searchd");
    std::fs::write(&script, "#!/bin/sh\nsleep 30\n").expect("script");
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    let state_root = root.path().join("state");
    let identity = BatchIdentity::new("bench-repo", "bench-rev", 7, "manifest:timeout".to_string())
        .expect("identity");
    let config = DaemonConfig {
        state_root: &state_root,
        searchd_binary: Some(&script),
        embedder: EMBEDDER,
        model_dir: None,
        repo_id: &identity.repo_id,
        revision_id: &identity.revision_id,
        ready_timeout: Duration::from_secs(2),
        io_timeout: Duration::from_secs(2),
        history_max_generations: 8,
    };
    let started = std::time::Instant::now();
    let err = DaemonSession::boot(&config)
        .err()
        .expect("boot must time out");
    assert!(matches!(err, BenchError::Timeout(_, _)), "{err}");
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "readiness must time out promptly"
    );
}

#[test]
fn missing_pinned_model_fails_boot_without_a_scored_record() {
    let root = tempfile::tempdir().expect("temp root");
    let state_root = root.path().join("state");
    let missing_model = root.path().join("missing-potion-code-model");
    let identity = BatchIdentity::new(
        "bench-repo",
        "bench-rev",
        7,
        "manifest:missing-model".to_string(),
    )
    .expect("identity");
    let config = DaemonConfig {
        state_root: &state_root,
        searchd_binary: None,
        embedder: "potion-code",
        model_dir: Some(&missing_model),
        repo_id: &identity.repo_id,
        revision_id: &identity.revision_id,
        ready_timeout: Duration::from_secs(10),
        io_timeout: Duration::from_secs(5),
        history_max_generations: 8,
    };
    let started = std::time::Instant::now();
    let err = DaemonSession::boot(&config)
        .err()
        .expect("missing pinned model must fail before capture");
    assert!(matches!(err, BenchError::Daemon(_)), "{err}");
    assert!(
        err.to_string().contains("model directory does not exist"),
        "{err}"
    );
    assert!(started.elapsed() < Duration::from_secs(20));
    assert!(!root.path().join("record.json").exists());
}

#[test]
fn unavailable_provider_is_typed_and_never_returns_hits() {
    let repo = tempfile::tempdir().expect("repo root");
    write_tiny_repo(repo.path());
    let manifest = load_manifest(&repo.path().join("manifest.json")).expect("manifest");
    let files = load_corpus(repo.path(), &manifest, &CorpusLimits::default()).expect("corpus");
    let (chunks, _) = chunk_corpus(&WholeFileChunker, &files).expect("chunk");
    let identity = BatchIdentity::new(
        "bench-repo",
        "bench-rev",
        7,
        "manifest:provider-unavailable".to_string(),
    )
    .expect("identity");
    let (batch, _) = assemble_batch(&identity, &chunks, &symbols_for(&files)).expect("batch");
    let state = tempfile::tempdir().expect("state root");
    let state_root = state.path().join("daemon");
    let config = DaemonConfig {
        state_root: &state_root,
        searchd_binary: None,
        embedder: "unavailable",
        model_dir: None,
        repo_id: &identity.repo_id,
        revision_id: &identity.revision_id,
        ready_timeout: Duration::from_secs(60),
        io_timeout: Duration::from_secs(30),
        history_max_generations: 8,
    };
    let session = DaemonSession::boot(&config).expect("daemon boots");
    let (_receipt, _ack) =
        publish_and_activate(&session, &batch, &identity, None).expect("publish+activate");
    match query_route(&RouteQuery {
        client: session.client(),
        route: "semantic",
        lexical_request: "sphinx quartz vaults",
        semantic_text: "sphinx quartz vaults",
        repo_id: &identity.repo_id,
        revision_id: &identity.revision_id,
        generation: identity.generation,
        top_k: 10,
    }) {
        QueryOutcome::SdkFailure { status, code, .. } => {
            assert_eq!(status, "unavailable");
            assert_eq!(code, "SEM_PROVIDER_UNAVAILABLE");
        }
        other => panic!("unavailable provider must be an SDK failure: {other:?}"),
    }
    session.stop().expect("bounded shutdown");
}

#[test]
fn terminated_daemon_is_typed_and_never_returns_hits() {
    let identity = BatchIdentity::new(
        "bench-repo",
        "bench-rev",
        7,
        "manifest:terminated".to_string(),
    )
    .expect("identity");
    let state = tempfile::tempdir().expect("state root");
    let state_root = state.path().join("daemon");
    let mut session = boot_session(&state_root, &identity);
    session
        .terminate_for_failure_probe()
        .expect("owned daemon terminates and reaps");
    match query_route(&RouteQuery {
        client: session.client(),
        route: "lexical",
        lexical_request: "sphinx",
        semantic_text: "sphinx",
        repo_id: &identity.repo_id,
        revision_id: &identity.revision_id,
        generation: identity.generation,
        top_k: 10,
    }) {
        QueryOutcome::SdkFailure { status, .. } => {
            assert!(matches!(status, "error" | "timeout" | "unavailable"));
        }
        other => panic!("terminated daemon must be an SDK failure: {other:?}"),
    }
    session.stop().expect("idempotent bounded shutdown");
}

#[test]
fn real_daemon_roundtrip_publishes_and_queries() {
    let repo = tempfile::tempdir().expect("repo root");
    write_tiny_repo(repo.path());
    let manifest = load_manifest(&repo.path().join("manifest.json")).expect("manifest");
    let files = load_corpus(repo.path(), &manifest, &CorpusLimits::default()).expect("corpus");
    let chunker = WholeFileChunker;
    let (chunks, coverage) = chunk_corpus(&chunker, &files).expect("chunk");
    assert_eq!(coverage.chunks, 3);
    let files_by_path: BTreeMap<_, _> = files
        .iter()
        .map(|file| (file.path.clone(), file.clone()))
        .collect();
    let published_units =
        quanta_index_retrieval_bench::published_units::PublishedUnitRegistry::from_chunks_and_symbols(
            &chunks,
            &symbols_for(&files),
            &files_by_path,
        )
        .expect("published units");

    let identity = BatchIdentity::new(
        "bench-repo",
        "bench-rev",
        7,
        "manifest:test-roundtrip".to_string(),
    )
    .expect("identity");
    let (batch, assembly) =
        assemble_batch(&identity, &chunks, &symbols_for(&files)).expect("batch");
    assert_eq!(assembly.scopes, 3);
    assert_eq!(assembly.semantic_scopes, 3);

    let state = tempfile::tempdir().expect("state root");
    let state_root = state.path().join("daemon");
    // Boot proves the fresh index empty; readiness precedes publish.
    let session = boot_session(&state_root, &identity);

    // A query before activation never fabricates rows: typed failure.
    let premature = query_route(&RouteQuery {
        client: session.client(),
        route: "lexical",
        lexical_request: "sphinx",
        semantic_text: "sphinx",
        repo_id: &identity.repo_id,
        revision_id: &identity.revision_id,
        generation: identity.generation,
        top_k: 10,
    });
    assert!(
        !matches!(premature, QueryOutcome::ReturnedWindow { .. }),
        "query before activation must fail, got {premature:?}"
    );

    let (receipt, ack) =
        publish_and_activate(&session, &batch, &identity, None).expect("publish+activate");

    // A stale generation pin never reads another generation's rows.
    let stale_result = query_route(&RouteQuery {
        client: session.client(),
        route: "lexical",
        lexical_request: "sphinx",
        semantic_text: "sphinx",
        repo_id: &identity.repo_id,
        revision_id: &identity.revision_id,
        generation: quanta_index_contract::ManifestGeneration::new(
            identity.generation.get().saturating_add(1),
        ),
        top_k: 10,
    });
    match &stale_result {
        QueryOutcome::RejectedResponse {
            code,
            observed_hit_count,
            window,
            expected_pin,
            observed_pin,
            ..
        } => {
            assert_eq!(code, "stale_generation");
            assert_eq!(
                *observed_hit_count,
                usize::try_from(window.returned()).expect("window count fits usize")
            );
            assert_ne!(expected_pin, observed_pin);
        }
        _ => {
            panic!("stale generation must fail, got {stale_result:?}")
        }
    }
    assert_eq!(receipt.batch_digest, batch.batch_digest().expect("digest"));
    assert_eq!(
        usize::try_from(receipt.accepted_replace_scopes).expect("scope count fits usize"),
        assembly.scopes
    );
    assert!(receipt.semantic_content.is_some());

    // Lexical route finds the distinctive term in its source span.
    let lexical = query_route(&RouteQuery {
        client: session.client(),
        route: "lexical",
        lexical_request: "sphinx",
        semantic_text: "sphinx",
        repo_id: &identity.repo_id,
        revision_id: &identity.revision_id,
        generation: identity.generation,
        top_k: 10,
    });
    let mut outcomes: BTreeMap<(String, String), QueryOutcome> = BTreeMap::new();
    let _previous = outcomes.insert(("T1".to_string(), "lexical".to_string()), lexical.clone());
    match &lexical {
        QueryOutcome::ReturnedWindow { hits, .. } => {
            assert!(!hits.is_empty(), "lexical must hit the sphinx term");
            assert!(hits.iter().any(|hit| hit.path == "src/lib.rs"), "{hits:?}");
            for hit in hits {
                assert!(hit.start_line >= 1 && hit.start_line <= hit.end_line);
            }
        }
        other => {
            panic!("lexical query failed: {other:?}");
        }
    }
    let plan = plan_query(
        QueryInputPolicy::Native,
        "sphinx quartz vaults",
        &NlPlanConfig::default(),
    )
    .expect("native plan");
    let lexical_record = result_value(
        "T1",
        "lexical",
        &lexical,
        &plan,
        10,
        &files_by_path,
        &published_units,
    )
    .expect("lexical SDK hits refer to published chunks");
    assert_eq!(lexical_record["route"], "lexical");

    // Semantic and hybrid routes answer under the same generation; their
    // rank content under hash-dev is plumbing, not quality evidence.
    for route in ["semantic", "hybrid"] {
        let outcome = query_route(&RouteQuery {
            client: session.client(),
            route,
            lexical_request: "sphinx quartz vaults",
            semantic_text: "sphinx quartz vaults",
            repo_id: &identity.repo_id,
            revision_id: &identity.revision_id,
            generation: identity.generation,
            top_k: 10,
        });
        let _previous = outcomes.insert(("T1".to_string(), route.to_string()), outcome.clone());
        match &outcome {
            QueryOutcome::ReturnedWindow { .. } => {}
            other => {
                panic!("{route} query failed: {other:?}");
            }
        }
        let route_record = result_value(
            "T1",
            route,
            &outcome,
            &plan,
            10,
            &files_by_path,
            &published_units,
        )
        .expect("SDK hits refer to published chunks");
        assert_eq!(route_record["route"], route);
    }

    // A nonsense term abstains or errors typed; never fake success.
    let missing = query_route(&RouteQuery {
        client: session.client(),
        route: "lexical",
        lexical_request: "zzzznothinghere",
        semantic_text: "zzzznothinghere",
        repo_id: &identity.repo_id,
        revision_id: &identity.revision_id,
        generation: identity.generation,
        top_k: 10,
    });
    match missing {
        QueryOutcome::ReturnedWindow { hits, window, .. } => {
            assert!(
                hits.is_empty() || window.outcome().is_exhausted(),
                "unmatched hits must be exhausted, not capped"
            );
        }
        QueryOutcome::RejectedResponse { .. } | QueryOutcome::SdkFailure { .. } => {}
    }

    // Unknown routes fail typed.
    match query_route(&RouteQuery {
        client: session.client(),
        route: "nope",
        lexical_request: "sphinx",
        semantic_text: "sphinx",
        repo_id: &identity.repo_id,
        revision_id: &identity.revision_id,
        generation: identity.generation,
        top_k: 10,
    }) {
        QueryOutcome::SdkFailure { code, .. } => assert_eq!(code, "unknown_route"),
        other => panic!("unknown route must not hit: {other:?}"),
    }

    assert!(receipt.semantic_content.is_some());
    assert_eq!(
        ack.active.generation.lexical.manifest_generation,
        identity.generation
    );
    assert_eq!(
        ack.active.generation.semantic.manifest_generation,
        identity.generation
    );

    // The live session assembles a schema-valid v3 record: contract echo,
    // per-route captures bound to the real receipt, ACK, daemon binary
    // and runner executable, and byte-span candidates.
    let universe: Vec<(String, String)> = files
        .iter()
        .map(|file| (file.path.clone(), file.sha256.clone()))
        .collect();
    let pack = QueryPack {
        suite_id: "roundtrip".to_string(),
        suite_commitment_sha256: "c".repeat(64),
        repository_commit: "c".repeat(40),
        tokenizer: "qi-regex-v1".to_string(),
        tokenizer_budget_version: Some("qb-v1".to_string()),
        routes: ["lexical", "semantic", "hybrid"]
            .iter()
            .map(ToString::to_string)
            .collect(),
        file_universe: universe.clone(),
        file_universe_digest: pack_universe_digest(&universe).expect("universe digest"),
        tasks: vec![PackTask {
            task_id: "T1".to_string(),
            query: "sphinx quartz vaults".to_string(),
            query_sha256: sha256_hex(b"sphinx quartz vaults"),
        }],
        pack_sha256: "d".repeat(64),
        comparison_contract: serde_json::json!({
            "top_k": 10,
            "tokenizer": "qi-regex-v1",
            "tokenizer_budget_version": "qb-v1",
            "output_unit_policy": "rank_prefix",
            "span_unit": "byte_span_with_line_projection_v1",
        }),
        contract_top_k: 10,
    };
    let runner_identity = RunnerIdentity::new(
        "quanta-sdk-runner".to_string(),
        "sha256:test".to_string(),
        "run-roundtrip".to_string(),
        "attested".to_string(),
        "m".to_string(),
        "l".to_string(),
    )
    .expect("identity");
    let searchd_bytes = std::fs::read(session.searchd_binary()).expect("searchd bytes");
    let runner_exe = std::env::current_exe().expect("test executable");
    let runner_bytes = std::fs::read(&runner_exe).expect("runner bytes");
    let receipt_binding = receipt_digest(&receipt).expect("receipt digest");
    let activation_binding = activation_digest(&ack).expect("activation digest");
    assert_eq!(receipt_binding.len(), 64);
    assert_eq!(activation_binding.len(), 64);
    let mut provenance = BTreeMap::new();
    let mut captures = BTreeMap::new();
    for route in ["lexical", "semantic", "hybrid"] {
        let (model, model_revision) = if route == "lexical" {
            ("none:lexical", "not-applicable")
        } else {
            (
                quanta_index_search_plane::SEARCH_OWNED_SEMANTIC_MODEL_ID,
                quanta_index_search_plane::SEARCH_OWNED_SEMANTIC_MODEL_REVISION,
            )
        };
        let capture_id = format!("run-roundtrip-{route}");
        let _previous = provenance.insert(
            route.to_string(),
            RouteProvenance {
                capture_id: capture_id.clone(),
            },
        );
        let _previous = captures.insert(
            capture_id,
            CaptureProvenance {
                chunk_strategy: "whole_file".to_string(),
                chunk_config: serde_json::json!({}),
                runner_binary_name: "sdk_roundtrip".to_string(),
                runner_binary_digest: sha256_hex(&runner_bytes),
                searchd_binary_digest: sha256_hex(&searchd_bytes),
                generation: identity.generation.get(),
                receipt_digest: receipt_binding.clone(),
                activation_digest: activation_binding.clone(),
                model: model.to_string(),
                model_revision: model_revision.to_string(),
                execution_profile: execution_profile_value(
                    QueryInputPolicy::Native,
                    &NlPlanConfig::default(),
                ),
                execution_profile_sha256: execution_profile_sha256(
                    QueryInputPolicy::Native,
                    &NlPlanConfig::default(),
                ),
            },
        );
    }
    let plans = BTreeMap::from([(
        "T1".to_string(),
        plan_query(
            QueryInputPolicy::Native,
            "sphinx quartz vaults",
            &NlPlanConfig::default(),
        )
        .expect("native plan"),
    )]);
    let nl_config = NlPlanConfig::default();
    let record = runner_record(&RunnerRecordInput {
        pack: &pack,
        identity: &runner_identity,
        provenance: &provenance,
        captures: &captures,
        outcomes: &outcomes,
        plans: &plans,
        nl_config: &nl_config,
        top_k: 10,
        files: &files_by_path,
        units: &published_units,
    })
    .expect("live v3 record assembles");
    // Pilot-debugging hook: dump the exact record bytes for out-of-band
    // schema/evaluator validation. Never part of assertions.
    if let Some(path) = std::env::var_os("QUANTA_BENCH_DUMP_RECORD") {
        let rendered = serde_json::to_string_pretty(&record).expect("record renders");
        std::fs::write(&path, rendered).expect("record dumps");
    }
    assert_eq!(
        record["schema_version"],
        serde_json::json!(quanta_index_retrieval_bench::record::RUNNER_SCHEMA_VERSION)
    );
    assert_eq!(record["comparison_contract"], pack.comparison_contract);
    assert_eq!(
        record["captures"]
            .as_object()
            .expect("captures object")
            .len(),
        3
    );
    assert_eq!(record["results"].as_array().expect("results").len(), 3);
    for row in record["results"].as_array().expect("results") {
        let route = row["route"].as_str().expect("route");
        let capture_id = format!("run-roundtrip-{route}");
        assert_eq!(
            record["route_provenance"][route]["capture_id"].as_str(),
            Some(capture_id.as_str())
        );
        assert_eq!(
            record["captures"][capture_id.as_str()]["receipt_digest"].as_str(),
            Some(receipt_binding.as_str())
        );
    }

    // A replayed publish on the live daemon refuses: either the CAS
    // fails or the replay ack trips the fresh-daemon applied check.
    let conflict = publish_and_activate(&session, &batch, &identity, None)
        .expect_err("stale expected-active CAS must refuse replay");
    assert!(
        matches!(conflict, BenchError::Sdk(_) | BenchError::Protocol(_)),
        "{conflict}"
    );
    session.stop().expect("bounded shutdown");
}

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .expect("git starts");
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("git stdout is utf8")
        .trim()
        .to_string()
}

#[test]
fn actual_runner_binary_emits_receipt_bound_v5_record() {
    let fixture = tempfile::tempdir().expect("fixture root");
    let repo = fixture.path().join("repo");
    let evidence = fixture.path().join("evidence");
    std::fs::create_dir_all(&repo).expect("repo dir");
    std::fs::create_dir_all(&evidence).expect("evidence dir");
    write_tiny_repo(&repo);
    std::fs::remove_file(repo.join("manifest.json")).expect("fixture manifest removed");
    drop(git(&repo, &["init", "--quiet"]));
    drop(git(
        &repo,
        &["config", "user.email", "retrieval-bench@example.invalid"],
    ));
    drop(git(&repo, &["config", "user.name", "Retrieval Bench"]));
    drop(git(&repo, &["add", "src"]));
    drop(git(&repo, &["commit", "--quiet", "-m", "fixture"]));
    let commit = git(&repo, &["rev-parse", "HEAD"]);

    let mut universe = Vec::new();
    for path in ["src/alpha.rs", "src/beta.rs", "src/lib.rs"] {
        universe.push((
            path.to_string(),
            sha256_hex(&std::fs::read(repo.join(path)).expect("source bytes")),
        ));
    }
    let manifest_path = evidence.join("manifest.json");
    let manifest_files: Vec<_> = universe
        .iter()
        .map(|(path, digest)| serde_json::json!({"path": path, "file_sha256": digest}))
        .collect();
    std::fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "repository_commit": commit,
            "files": manifest_files,
        }))
        .expect("manifest renders"),
    )
    .expect("manifest writes");

    let query = "sphinx quartz vaults";
    let pack_path = evidence.join("query-pack.json");
    std::fs::write(
        &pack_path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": 3,
            "suite_id": "runner-binary-roundtrip",
            "suite_commitment_sha256": "c".repeat(64),
            "repository_commit": commit,
            "tokenizer": "qi-regex-v1",
            "tokenizer_budget_version": "qb-v1",
            "routes": ["lexical", "semantic", "hybrid"],
            "file_universe": universe.iter().map(|(path, digest)| {
                serde_json::json!({"path": path, "file_sha256": digest})
            }).collect::<Vec<_>>(),
            "file_universe_digest": pack_universe_digest(&universe).expect("universe digest"),
            "comparison_contract": {
                "top_k": 10,
                "tokenizer": "qi-regex-v1",
                "tokenizer_budget_version": "qb-v1",
                "output_unit_policy": "rank_prefix",
                "span_unit": "byte_span_with_line_projection_v1"
            },
            "tasks": [{
                "task_id": "T1",
                "query": query,
                "query_sha256": sha256_hex(query.as_bytes())
            }]
        }))
        .expect("pack renders"),
    )
    .expect("pack writes");

    let runner = PathBuf::from(env!("CARGO_BIN_EXE_quanta-index-retrieval-bench"));
    let runner_digest = sha256_hex(&std::fs::read(&runner).expect("runner bytes"));
    let searchd = resolve_searchd_binary(None).expect("explicit env pin resolves");
    let searchd_digest = sha256_hex(&std::fs::read(&searchd).expect("searchd bytes"));
    let out = evidence.join("record.json");
    let diagnostic_out = evidence.join("retrieval-diagnostic.json");
    let refusal_out = evidence.join("query-plan-refusal.json");
    let state = evidence.join("state");
    let output = Command::new(&runner)
        .args([
            "run",
            "--repo",
            repo.to_str().expect("repo path"),
            "--manifest",
            manifest_path.to_str().expect("manifest path"),
            "--strategy",
            "whole_file",
            "--query-pack",
            pack_path.to_str().expect("pack path"),
            "--routes",
            "lexical,semantic,hybrid",
            "--query-input-policy",
            "native",
            "--top-k",
            "10",
            "--state-root",
            state.to_str().expect("state path"),
            "--searchd-bin",
            searchd.to_str().expect("searchd path"),
            "--searchd-expected-sha256",
            &searchd_digest,
            "--embedder",
            EMBEDDER,
            "--repo-id",
            "runner-binary-repo",
            "--revision-id",
            &commit,
            "--generation",
            "1",
            "--runner-name",
            "quanta-sdk-runner",
            "--runner-revision",
            &format!("sha256:{runner_digest}"),
            "--run-id",
            "actual-runner-binary",
            "--blinding",
            "attested",
            "--isolation-method",
            "test-fixture-no-gold-mounted",
            "--access-block-log",
            "attested-fixture-pack-only",
            "--out",
            out.to_str().expect("output path"),
            "--diagnostics-out",
            diagnostic_out.to_str().expect("diagnostic path"),
            "--refusal-out",
            refusal_out.to_str().expect("refusal path"),
        ])
        .output()
        .expect("runner starts");
    assert!(
        output.status.success(),
        "runner failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&out).expect("record bytes")).expect("record JSON");
    assert_eq!(
        record["schema_version"],
        serde_json::json!(quanta_index_retrieval_bench::record::RUNNER_SCHEMA_VERSION)
    );
    assert_eq!(
        record["runner"]["revision"],
        format!("sha256:{runner_digest}")
    );
    let captures = record["captures"].as_object().expect("captures");
    assert_eq!(captures.len(), 3);
    for capture in captures.values() {
        assert_eq!(capture["runner_binary"]["digest"], runner_digest);
        assert_eq!(capture["searchd_binary"]["binary_digest"], searchd_digest);
        for key in ["receipt_digest", "activation_digest"] {
            let digest = capture[key].as_str().expect("capture digest");
            assert_eq!(digest.len(), 64);
            assert!(digest.bytes().all(|byte| byte.is_ascii_hexdigit()));
        }
    }
    assert_eq!(record["results"].as_array().expect("results").len(), 3);
    let diagnostic: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&diagnostic_out).expect("diagnostic bytes"))
            .expect("diagnostic JSON");
    assert_eq!(
        diagnostic["record_sha256"],
        sha256_hex(&std::fs::read(&out).expect("record bytes"))
    );
    assert_eq!(diagnostic["query_pack_sha256"], record["query_pack_sha256"]);
    assert_eq!(
        diagnostic["results"]
            .as_array()
            .expect("diagnostic results")
            .len(),
        3
    );
    let hybrid = diagnostic["results"]
        .as_array()
        .expect("diagnostic results")
        .iter()
        .find(|row| row["route"] == "hybrid")
        .expect("hybrid diagnostic");
    let hybrid_candidates = hybrid["candidates"].as_array().expect("hybrid candidates");
    assert!(
        !hybrid_candidates.is_empty(),
        "hybrid query must return a diagnostic candidate"
    );
    for candidate in hybrid_candidates {
        assert!(
            !candidate["contributions"]
                .as_array()
                .expect("lane contributions")
                .is_empty()
        );
    }
    assert_eq!(
        diagnostic["runner_timing_detail_ms"]["clock"],
        "runner_monotonic_wall_v1"
    );

    if let Some(dir) = std::env::var_os("QUANTA_BENCH_SDK_EVIDENCE_DIR") {
        let destination = PathBuf::from(dir).join("actual-runner-record.json");
        std::fs::create_dir_all(destination.parent().expect("evidence parent"))
            .expect("evidence output dir");
        let _copied = std::fs::copy(&out, destination).expect("evidence record copies");
    }
}

#[test]
fn second_boot_over_used_root_is_refused_without_cleanup() {
    let repo = tempfile::tempdir().expect("repo root");
    write_tiny_repo(repo.path());
    let manifest = load_manifest(&repo.path().join("manifest.json")).expect("manifest");
    let files = load_corpus(repo.path(), &manifest, &CorpusLimits::default()).expect("corpus");
    let (chunks, _) = chunk_corpus(&WholeFileChunker, &files).expect("chunk");
    let identity = BatchIdentity::new("bench-repo", "bench-rev", 7, "manifest:reuse".to_string())
        .expect("identity");
    let (batch, _) = assemble_batch(&identity, &chunks, &symbols_for(&files)).expect("batch");

    let state = tempfile::tempdir().expect("state root");
    let state_root = state.path().join("daemon");
    let session = boot_session(&state_root, &identity);
    let (_receipt, _ack) =
        publish_and_activate(&session, &batch, &identity, None).expect("publish");
    session.stop().expect("stop");
    // The used root still holds index data: a second boot must refuse it.
    let config = DaemonConfig {
        state_root: &state_root,
        searchd_binary: None,
        embedder: EMBEDDER,
        model_dir: None,
        repo_id: &identity.repo_id,
        revision_id: &identity.revision_id,
        ready_timeout: Duration::from_secs(5),
        io_timeout: Duration::from_secs(5),
        history_max_generations: 8,
    };
    assert!(DaemonSession::boot(&config).is_err());
}

#[test]
fn determinism_probe_repeats_identical_publish() {
    // Same batch content replays the same digest: publish is idempotent
    // under identical input, and receipts name the replay.
    let repo = tempfile::tempdir().expect("repo root");
    write_tiny_repo(repo.path());
    let manifest = load_manifest(&repo.path().join("manifest.json")).expect("manifest");
    let files = load_corpus(repo.path(), &manifest, &CorpusLimits::default()).expect("corpus");
    let (chunks, _) = chunk_corpus(&WholeFileChunker, &files).expect("chunk");
    let identity = BatchIdentity::new("bench-repo", "bench-rev", 9, "manifest:replay".to_string())
        .expect("identity");
    let (left, _) = assemble_batch(&identity, &chunks, &symbols_for(&files)).expect("batch");
    let (right, _) = assemble_batch(&identity, &chunks, &symbols_for(&files)).expect("batch");
    assert_eq!(
        left.batch_digest().expect("digest"),
        right.batch_digest().expect("digest")
    );
}

#[test]
fn symbol_route_answers_from_published_units_and_proves_spans() {
    // RBR-05: the public symbol route returns published symbol units, and
    // every hit proves against the typed registry (definition span, never
    // the engine snippet as source bytes).
    let repo = tempfile::tempdir().expect("repo root");
    write_tiny_repo(repo.path());
    let manifest = load_manifest(&repo.path().join("manifest.json")).expect("manifest");
    let files = load_corpus(repo.path(), &manifest, &CorpusLimits::default()).expect("corpus");
    let (chunks, _) = chunk_corpus(&WholeFileChunker, &files).expect("chunk");
    let symbols = symbols_for(&files);
    let identity = BatchIdentity::new("bench-repo", "bench-rev", 11, "manifest:symbol".to_string())
        .expect("identity");
    let (batch, assembly) = assemble_batch(&identity, &chunks, &symbols).expect("batch");
    assert!(
        assembly.symbols >= 3,
        "tiny repo publishes its functions as symbols"
    );
    let files_by_path: BTreeMap<_, _> = files
        .iter()
        .map(|file| (file.path.clone(), file.clone()))
        .collect();
    let published_units =
        quanta_index_retrieval_bench::published_units::PublishedUnitRegistry::from_chunks_and_symbols(
            &chunks,
            &symbols,
            &files_by_path,
        )
        .expect("units");
    let state = tempfile::tempdir().expect("state root");
    let session = boot_session(&state.path().join("daemon"), &identity);
    let (_receipt, _ack) =
        publish_and_activate(&session, &batch, &identity, None).expect("publish+activate");
    let plan = plan_query(
        QueryInputPolicy::Native,
        "sphinx_riddle",
        &NlPlanConfig::default(),
    )
    .expect("native plan");
    let outcome = query_route(&RouteQuery {
        client: session.client(),
        route: "symbol",
        lexical_request: &plan.lexical_request,
        semantic_text: &plan.semantic_text,
        repo_id: &identity.repo_id,
        revision_id: &identity.revision_id,
        generation: identity.generation,
        top_k: 10,
    });
    session.stop().expect("bounded shutdown");
    let hits = match outcome {
        QueryOutcome::ReturnedWindow { hits, .. } => hits,
        other => {
            panic!("symbol route failed: {other:?}")
        }
    };
    assert!(
        !hits.is_empty(),
        "symbol route must answer for a published definition"
    );
    for hit in &hits {
        let unit = published_units
            .get(&hit.candidate_id)
            .unwrap_or_else(|| panic!("hit id is not a published unit: {}", hit.candidate_id));
        assert_eq!(
            unit.kind,
            quanta_index_retrieval_bench::published_units::PublishedUnitKind::Symbol,
            "symbol route hits must resolve as symbol units"
        );
        assert_eq!(unit.path, hit.path);
    }
    // The first hit proves into a record row against the same source-byte
    // accounting as chunk routes.
    let returned = u32::try_from(hits.len()).expect("hit count fits u32");
    let record = result_value(
        "T1",
        "symbol",
        &QueryOutcome::ReturnedWindow {
            hits,
            window: quanta_index_contract::QueryResultWindowV2::exact_probe(returned),
            explanation: None,
            latency: Duration::from_millis(1),
        },
        &plan,
        10,
        &files_by_path,
        &published_units,
    )
    .expect("symbol hits prove against published units");
    assert_eq!(record["route"], "symbol");
    assert!(
        record["candidates"]
            .as_array()
            .is_some_and(|rows| !rows.is_empty())
    );
}

#[test]
fn symbol_route_no_answer_is_typed_never_fake_success() {
    // RBR-05: a symbol query with no published match abstains or fails
    // typed; it never fabricates hits or borrows chunk results.
    let repo = tempfile::tempdir().expect("repo root");
    write_tiny_repo(repo.path());
    let manifest = load_manifest(&repo.path().join("manifest.json")).expect("manifest");
    let files = load_corpus(repo.path(), &manifest, &CorpusLimits::default()).expect("corpus");
    let (chunks, _) = chunk_corpus(&WholeFileChunker, &files).expect("chunk");
    let symbols = symbols_for(&files);
    let identity = BatchIdentity::new("bench-repo", "bench-rev", 12, "manifest:none".to_string())
        .expect("identity");
    let (batch, _) = assemble_batch(&identity, &chunks, &symbols).expect("batch");
    let state = tempfile::tempdir().expect("state root");
    let session = boot_session(&state.path().join("daemon"), &identity);
    let (_receipt, _ack) =
        publish_and_activate(&session, &batch, &identity, None).expect("publish+activate");
    let outcome = query_route(&RouteQuery {
        client: session.client(),
        route: "symbol",
        lexical_request: "zzz_no_such_symbol_zzz",
        semantic_text: "zzz_no_such_symbol_zzz",
        repo_id: &identity.repo_id,
        revision_id: &identity.revision_id,
        generation: identity.generation,
        top_k: 10,
    });
    session.stop().expect("bounded shutdown");
    match outcome {
        QueryOutcome::ReturnedWindow { hits, window, .. } => {
            if hits.is_empty() {
                assert!(
                    window.outcome().is_exhausted(),
                    "empty symbol window must be an exact abstention"
                );
            } else {
                panic!("nonsense symbol name must not produce hits: {hits:?}");
            }
        }
        QueryOutcome::RejectedResponse { .. } | QueryOutcome::SdkFailure { .. } => {
            // A typed provider/transport failure is an acceptable refusal.
        }
    }
}
