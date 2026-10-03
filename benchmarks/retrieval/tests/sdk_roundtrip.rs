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

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use quanta_index_retrieval_bench::batch::{
    BatchIdentity, activation_digest, assemble_batch, receipt_digest,
};
use quanta_index_retrieval_bench::chunking::chunk_corpus;
use quanta_index_retrieval_bench::chunking::fixed_window::StrictWindowChunker;
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
    query_route_with_policy, resolve_searchd_binary,
};
use quanta_index_retrieval_bench::symbols::extract_corpus_symbols;
use quanta_index_retrieval_bench::{BenchError, sha256_hex};
use quanta_index_search_plane::{HybridFetchFloorPolicy, QueryStageObservationPolicy};

const EMBEDDER: &str = "hash-dev";

/// Only the owned child receives budget overrides; concurrently running SDK
/// tests keep their process environment unchanged. The guard also reaps on panic.
struct BudgetFixtureDaemon {
    client: quanta_index_sdk::QuantaIndex,
    _child: BudgetFixtureChild,
}

struct BudgetFixtureChild(std::process::Child);

impl Drop for BudgetFixtureChild {
    fn drop(&mut self) {
        if !self.0.try_wait().is_ok_and(|status| status.is_some()) {
            drop(self.0.kill());
        }
        drop(self.0.wait());
    }
}

fn boot_budget_fixture(
    state: &Path,
    identity: &BatchIdentity,
    examined: Option<usize>,
    response_bytes: Option<u64>,
) -> BudgetFixtureDaemon {
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixStream;

    std::fs::create_dir(state).expect("fresh budget state");
    std::fs::set_permissions(state, std::fs::Permissions::from_mode(0o700))
        .expect("secure budget state");
    let log = state.with_extension("stderr.log");
    let mut command = Command::new(resolve_searchd_binary(None).expect("pinned daemon"));
    let _configured = command
        .arg("serve")
        .arg("--state-root")
        .arg(state)
        .env("QUANTA_INDEX_EMBEDDER", EMBEDDER)
        .env("QUANTA_INDEX_QUERY_STAGE_OBSERVATION", "disabled")
        .env("QUANTA_INDEX_EXPERIMENTAL_HYBRID_FETCH_FLOOR", "100")
        .env("QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_GENERATIONS", "8")
        .env("QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_BYTES", "1073741824")
        .env(
            "QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_REVISION_PAIRS",
            "128",
        )
        .env(
            "QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_TOTAL_BYTES",
            "4294967296",
        )
        .env_remove("QUANTA_INDEX_LEXICAL_MAX_EXAMINED_CANDIDATES")
        .env_remove("QUANTA_INDEX_QUERY_RESPONSE_MAX_BYTES")
        .stdout(std::process::Stdio::null())
        .stderr(std::fs::File::create_new(&log).expect("external daemon log"));
    if let Some(limit) = examined {
        let _configured = command.env(
            "QUANTA_INDEX_LEXICAL_MAX_EXAMINED_CANDIDATES",
            limit.to_string(),
        );
    }
    if let Some(limit) = response_bytes {
        let _configured = command.env("QUANTA_INDEX_QUERY_RESPONSE_MAX_BYTES", limit.to_string());
    }
    let mut child = BudgetFixtureChild(command.spawn().expect("budget daemon spawn"));
    // Construct the guard immediately, before any readiness assertion can panic.
    let deadline = std::time::Instant::now()
        .checked_add(Duration::from_secs(60))
        .expect("readiness deadline");
    loop {
        let ready = ["query.sock", "control.sock", "ingest.sock"]
            .iter()
            .all(|name| UnixStream::connect(state.join("search-plane").join(name)).is_ok());
        if ready {
            break;
        }
        assert!(
            child.0.try_wait().expect("owned child status").is_none()
                && std::time::Instant::now() < deadline,
            "budget daemon readiness failed: {}",
            std::fs::read_to_string(&log).expect("daemon stderr")
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let client = quanta_index_sdk::QuantaIndex::connect(
        quanta_index_sdk::ConnectOptions::from_state_root(state)
            .with_request_io_timeout(Duration::from_secs(30)),
    )
    .expect("budget SDK transports");
    let daemon = BudgetFixtureDaemon {
        client,
        _child: child,
    };
    let status = daemon
        .client
        .generations()
        .status(identity.repo_id.clone(), identity.revision_id.clone())
        .expect("fresh budget daemon status");
    assert_eq!(status.repo_id, identity.repo_id);
    assert_eq!(status.revision_id, identity.revision_id);
    assert!(status.tracks.is_empty() && status.semantic_content.is_none());
    daemon
}

#[test]
fn literal_file_public_route_enforces_execution_and_response_budgets() {
    assert_file_policy_budgets(QueryInputPolicy::LiteralFile, "needle", 35);
}

#[test]
fn keyword_and_substring_file_public_routes_enforce_execution_and_response_budgets() {
    // Identical files score equally, so the scored keyword keeps path order here too.
    assert_file_policy_budgets(QueryInputPolicy::KeywordFile, "needle", 36);
    // The raw substring's candidate collection is bounded by the same typed budget.
    assert_file_policy_budgets(QueryInputPolicy::SubstringFile, "eedl", 37);
}

fn assert_file_policy_budgets(policy: QueryInputPolicy, raw: &str, generation: u64) {
    use quanta_index_contract::SearchPlaneErrorCodeV2;
    use quanta_index_sdk::SdkError;

    let fixture = tempfile::tempdir().expect("budget fixture");
    let repo = fixture.path().join("repo");
    let text = format!("package fixture\n// needle {}\n", "evidence ".repeat(180));
    let sources: Vec<_> = (0..10)
        .map(|index| (format!("budget/file{index:02}.go"), text.clone()))
        .collect();
    let references: Vec<_> = sources
        .iter()
        .map(|(path, text)| (path.as_str(), text.as_str()))
        .collect();
    write_repo(&repo, &references);
    let manifest = load_manifest(&repo.join("manifest.json")).expect("budget manifest");
    let files = load_corpus(&repo, &manifest, &CorpusLimits::default()).expect("budget corpus");
    let (chunks, _) = chunk_corpus(&WholeFileChunker, &files).expect("budget chunks");
    let identity = BatchIdentity::new(
        "bench-repo",
        "bench-rev",
        generation,
        "manifest:budget".to_string(),
    )
    .expect("budget identity");
    let (batch, _) = assemble_fixture_batch(&identity, &chunks, &files).expect("budget batch");
    let plan = plan_query(policy, raw, &NlPlanConfig::default()).expect("budget file plan");
    let expected: Vec<_> = sources.iter().map(|(path, _)| path.clone()).collect();
    let publish = |daemon: &BudgetFixtureDaemon| {
        daemon
            .client
            .search_corpus()
            .publish_and_activate(&batch, None)
            .expect("budget publish")
    };
    let query = |daemon: &BudgetFixtureDaemon, top_k| {
        daemon
            .client
            .lexical()
            .query()
            .native(&plan.lexical_request)
            .active(identity.repo_id.clone(), identity.revision_id.clone())
            .top_k(top_k)
            .execute()
    };

    let baseline = boot_budget_fixture(&fixture.path().join("baseline"), &identity, None, None);
    let _published = publish(&baseline);
    let full = query(&baseline, 10).expect("unrestricted positive control");
    assert_eq!(
        full.results
            .iter()
            .map(|row| row.repo_relative_path.as_str().to_string())
            .collect::<Vec<_>>(),
        expected
    );
    assert!(full.window.outcome().is_exhausted());
    let one = query(&baseline, 1).expect("one-row positive control");
    assert_eq!(one.results.len(), 1);
    assert!(one.next_cursor.is_some());
    let one_bytes = quanta_index_ipc::cbor_payload_len(&one).expect("one-row encoding");
    let full_bytes = quanta_index_ipc::cbor_payload_len(&full).expect("full encoding");
    let lower = one_bytes.checked_add(1_024).expect("cap arithmetic");
    assert!(
        lower < full_bytes,
        "fixture requires a nonempty byte-cut interval: one={one_bytes}, full={full_bytes}"
    );
    let cap = lower.midpoint(full_bytes);
    drop(baseline);

    let limited = boot_budget_fixture(&fixture.path().join("examined"), &identity, Some(1), None);
    let _published = publish(&limited);
    let refused = query(&limited, 10).expect_err("ten matching files exceed execution budget one");
    assert!(
        matches!(
            refused,
            SdkError::Remote {
                code: SearchPlaneErrorCodeV2::LexicalExaminedBudgetExceeded
                    | SearchPlaneErrorCodeV2::LexicalCollectionBudgetExceeded,
                ..
            }
        ),
        "expected typed lexical budget refusal: {refused:?}"
    );
    drop(limited);

    let cut = boot_budget_fixture(&fixture.path().join("cut"), &identity, None, Some(cap));
    let _published = publish(&cut);
    let mut page = query(&cut, 10).expect("byte-cut file page");
    assert!(!page.results.is_empty() && page.results.len() < 10);
    assert_eq!(page.window.has_more(), Some(true));
    let pinned = page.generation.clone();
    let mut walked = Vec::new();
    let mut pages: u32 = 0;
    loop {
        pages = pages.saturating_add(1);
        assert!(pages <= 10, "nonempty pages must finish within ten files");
        assert!(quanta_index_ipc::cbor_payload_len(&page).expect("page encoding") <= cap);
        assert_eq!(
            usize::try_from(page.window.returned()).expect("returned count"),
            page.results.len()
        );
        assert!(!page.results.is_empty());
        walked.extend(
            page.results
                .iter()
                .map(|row| row.repo_relative_path.as_str().to_string()),
        );
        let Some(cursor) = page.next_cursor else {
            assert!(page.window.outcome().is_exhausted());
            break;
        };
        assert_eq!(page.window.has_more(), Some(true));
        page = cut
            .client
            .lexical()
            .query()
            .native(&plan.lexical_request)
            .pinned(pinned.clone())
            .top_k(10)
            .after(cursor)
            .execute()
            .expect("continued byte-cut file page");
    }
    assert!(pages >= 2);
    assert_eq!(
        walked, expected,
        "byte cuts preserve every unique file and rank order"
    );
    drop(cut);

    let tiny = boot_budget_fixture(&fixture.path().join("tiny"), &identity, None, Some(1));
    let _published = publish(&tiny);
    let refused = query(&tiny, 10).expect_err("one byte cannot hold a ranked row");
    assert!(
        matches!(
            refused,
            SdkError::Remote {
                code: SearchPlaneErrorCodeV2::ResultTooLarge,
                ..
            }
        ),
        "expected typed response-size refusal: {refused:?}"
    );
}

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

fn assemble_fixture_batch(
    identity: &BatchIdentity,
    chunks: &BTreeMap<String, Vec<quanta_index_retrieval_bench::chunking::Chunk>>,
    files: &[quanta_index_retrieval_bench::corpus::SourceFile],
) -> quanta_index_retrieval_bench::BenchResult<(
    quanta_index_sdk::SearchCorpusBatch,
    quanta_index_retrieval_bench::batch::BatchAssemblyReport,
)> {
    use quanta_index_retrieval_bench::symbols::{
        SymbolCoveragePolicy, SymbolPreflightOptions, preflight_corpus_symbols,
    };
    let by_path = files
        .iter()
        .map(|file| (file.path.clone(), file.clone()))
        .collect();
    let preflight = preflight_corpus_symbols(&by_path, &SymbolPreflightOptions::default())?;
    assemble_batch(
        identity,
        chunks,
        &by_path,
        &preflight,
        SymbolCoveragePolicy::RequireComplete,
        quanta_index_contract::SourcePublicationEvent {
            stream_id: "sdk-roundtrip-fixture".to_string(),
            event_id: "source-fixture-initial".to_string(),
            expected_base_event_id: None,
            payload_sha256: [0; 32],
        },
        true,
    )
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
    write_repo(root, files);
}

/// Write a fixture repository plus its manifest (helper for fixture
/// variants beyond the tiny repo).
fn write_repo(root: &Path, files: &[(&str, &str)]) {
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
    boot_session_with_policy(state_root, identity, QueryStageObservationPolicy::Enabled)
}

fn boot_session_with_policy(
    state_root: &Path,
    identity: &BatchIdentity,
    policy: QueryStageObservationPolicy,
) -> DaemonSession {
    boot_session_with_policies(
        state_root,
        identity,
        policy,
        HybridFetchFloorPolicy::default(),
    )
}

fn boot_session_with_policies(
    state_root: &Path,
    identity: &BatchIdentity,
    policy: QueryStageObservationPolicy,
    floor: HybridFetchFloorPolicy,
) -> DaemonSession {
    let config = DaemonConfig {
        state_root,
        searchd_binary: None,
        embedder: EMBEDDER,
        model_dir: None,
        query_stage_observation: policy,
        hybrid_fetch_floor: floor,
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

#[test]
fn literal_file_public_route_groups_before_top_k_and_records_distinct_files() {
    let repo = tempfile::tempdir().expect("repo");
    let repeated = format!(
        "package fixture\n{}",
        "// needle evidence in the repeated file padding to span windows\n".repeat(40)
    );
    let mut sources = vec![("src/repeated.go".to_string(), repeated)];
    for index in 0..9 {
        sources.push((
            format!("src/unique{index:02}.go"),
            format!("package fixture\n// needle evidence in unique file {index}\n"),
        ));
    }
    let references: Vec<(&str, &str)> = sources
        .iter()
        .map(|(path, text)| (path.as_str(), text.as_str()))
        .collect();
    write_repo(repo.path(), &references);
    let manifest = load_manifest(&repo.path().join("manifest.json")).expect("manifest");
    let files = load_corpus(repo.path(), &manifest, &CorpusLimits::default()).expect("corpus");
    let (chunks, _) = chunk_corpus(&StrictWindowChunker::new(128, 16), &files).expect("chunks");
    assert!(
        chunks["src/repeated.go"]
            .iter()
            .filter(|chunk| chunk.text.contains("needle"))
            .count()
            >= 15,
        "fixture must publish at least 15 matching chunks from one file"
    );
    let identity = BatchIdentity::new("bench-repo", "bench-rev", 13, "manifest:file".to_string())
        .expect("identity");
    let (batch, _) = assemble_fixture_batch(&identity, &chunks, &files).expect("batch");
    let files_by_path: BTreeMap<_, _> = files
        .iter()
        .map(|file| (file.path.clone(), file.clone()))
        .collect();
    let registry =
        quanta_index_retrieval_bench::published_units::PublishedUnitRegistry::from_chunks_and_symbols(
            &chunks,
            &symbols_for(&files),
            &files_by_path,
        )
        .expect("published units");
    let state = tempfile::tempdir().expect("state");
    let session = boot_session(state.path(), &identity);
    let _published = publish_and_activate(&session, &batch, &identity, None).expect("publish");
    let plan = plan_query(
        QueryInputPolicy::LiteralFile,
        "needle",
        &NlPlanConfig::default(),
    )
    .expect("file plan");
    let outcome = query_route_with_policy(
        &RouteQuery {
            client: session.client(),
            route: "lexical",
            lexical_request: &plan.lexical_request,
            semantic_text: &plan.semantic_text,
            repo_id: &identity.repo_id,
            revision_id: &identity.revision_id,
            generation: identity.generation,
            top_k: 10,
        },
        QueryInputPolicy::LiteralFile,
    );
    let repeated_outcome = query_route_with_policy(
        &RouteQuery {
            client: session.client(),
            route: "lexical",
            lexical_request: &plan.lexical_request,
            semantic_text: &plan.semantic_text,
            repo_id: &identity.repo_id,
            revision_id: &identity.revision_id,
            generation: identity.generation,
            top_k: 10,
        },
        QueryInputPolicy::LiteralFile,
    );
    let mut paged_paths = Vec::new();
    let mut page = session
        .client()
        .lexical()
        .query()
        .native(&plan.lexical_request)
        .active(identity.repo_id.clone(), identity.revision_id)
        .top_k(3)
        .execute()
        .expect("first file page");
    let pinned_generation = page.generation.clone();
    let mut pages = 0;
    loop {
        pages += 1;
        assert!(pages <= 4, "file pagination must terminate after ten rows");
        assert_eq!(
            usize::try_from(page.window.returned()).expect("u32 count fits usize"),
            page.results.len()
        );
        paged_paths.extend(
            page.results
                .iter()
                .map(|candidate| candidate.repo_relative_path.as_str().to_string()),
        );
        let Some(cursor) = page.next_cursor else {
            assert!(page.window.outcome().is_exhausted());
            break;
        };
        assert_eq!(page.window.returned(), 3, "nonfinal file page fills cap");
        page = session
            .client()
            .lexical()
            .query()
            .native(&plan.lexical_request)
            .pinned(pinned_generation.clone())
            .top_k(3)
            .after(cursor)
            .execute()
            .expect("continued file page");
    }
    session.stop().expect("shutdown");
    let QueryOutcome::ReturnedWindow { ref hits, .. } = outcome else {
        panic!("literal file query failed: {outcome:?}");
    };
    let paths: BTreeSet<_> = hits.iter().map(|hit| hit.path.as_str()).collect();
    assert_eq!(hits.len(), 10, "top_k must count distinct files");
    assert_eq!(paths.len(), 10, "file projection must group before top_k");
    assert_eq!(
        paths,
        sources
            .iter()
            .map(|(path, _)| path.as_str())
            .collect::<BTreeSet<_>>()
    );
    let QueryOutcome::ReturnedWindow {
        hits: repeated_hits,
        window,
        ..
    } = repeated_outcome
    else {
        panic!("repeated literal file query failed: {repeated_outcome:?}");
    };
    assert_eq!(window.returned(), 10);
    assert_eq!(
        hits.iter()
            .map(|hit| (&hit.path, &hit.candidate_id, hit.score))
            .collect::<Vec<_>>(),
        repeated_hits
            .iter()
            .map(|hit| (&hit.path, &hit.candidate_id, hit.score))
            .collect::<Vec<_>>(),
        "repeat must preserve order, representative, and score"
    );
    assert_eq!(
        paged_paths,
        hits.iter().map(|hit| hit.path.clone()).collect::<Vec<_>>(),
        "public continuation must preserve complete file ranking"
    );
    let recorded = result_value(
        "T1",
        "lexical",
        &outcome,
        &plan,
        10,
        &files_by_path,
        &registry,
    )
    .expect("source-proven result");
    assert_eq!(recorded["rank_unit"], "distinct_file");
    assert_eq!(recorded["candidates"].as_array().expect("rows").len(), 10);
}

#[test]
fn native_file_public_routes_group_order_scope_and_case() {
    let repo = tempfile::tempdir().expect("repo");
    let repeated = format!(
        "package fixture\n{}",
        "// needle evidence in the repeated file padding to span windows\n".repeat(40)
    );
    let mut sources = vec![("src/repeated.go".to_string(), repeated)];
    for index in 0..9 {
        sources.push((
            format!("src/unique{index:02}.go"),
            format!("package fixture\n// needle evidence in unique file {index}\n"),
        ));
    }
    // Counterexamples: the name only in a path component, and only in upper case.
    sources.push((
        "src/needle/only_path.go".to_string(),
        "package fixture\n// nothing to see here\n".to_string(),
    ));
    sources.push((
        "src/upper.go".to_string(),
        "package fixture\n// NEEDLE EEDL in upper case only\n".to_string(),
    ));
    assert!(
        sources.iter().all(|(_, text)| !text
            .to_ascii_lowercase()
            .contains("neverpresentbenchmarktoken")),
        "the second NL token must be absent so AND cannot satisfy the OR fixture"
    );
    let references: Vec<(&str, &str)> = sources
        .iter()
        .map(|(path, text)| (path.as_str(), text.as_str()))
        .collect();
    write_repo(repo.path(), &references);
    let manifest = load_manifest(&repo.path().join("manifest.json")).expect("manifest");
    let files = load_corpus(repo.path(), &manifest, &CorpusLimits::default()).expect("corpus");
    let (chunks, _) = chunk_corpus(&StrictWindowChunker::new(128, 16), &files).expect("chunks");
    assert!(
        chunks["src/repeated.go"]
            .iter()
            .filter(|chunk| chunk.text.contains("needle"))
            .count()
            >= 15,
        "fixture must publish at least 15 matching chunks from one file"
    );
    let identity = BatchIdentity::new("bench-repo", "bench-rev", 17, "manifest:file".to_string())
        .expect("identity");
    let (batch, _) = assemble_fixture_batch(&identity, &chunks, &files).expect("batch");
    let files_by_path: BTreeMap<_, _> = files
        .iter()
        .map(|file| (file.path.clone(), file.clone()))
        .collect();
    let registry =
        quanta_index_retrieval_bench::published_units::PublishedUnitRegistry::from_chunks_and_symbols(
            &chunks,
            &symbols_for(&files),
            &files_by_path,
        )
        .expect("published units");
    let state = tempfile::tempdir().expect("state");
    let session = boot_session(state.path(), &identity);
    let _published = publish_and_activate(&session, &batch, &identity, None).expect("publish");
    let content_files: BTreeSet<&str> = sources
        .iter()
        .map(|(path, _)| path.as_str())
        .filter(|path| path.starts_with("src/repeated") || path.starts_with("src/unique"))
        .collect();
    let run = |policy: QueryInputPolicy, raw: &str, top_k: u32| {
        let plan = plan_query(policy, raw, &NlPlanConfig::default()).expect("file plan");
        let outcome = query_route_with_policy(
            &RouteQuery {
                client: session.client(),
                route: "lexical",
                lexical_request: &plan.lexical_request,
                semantic_text: &plan.semantic_text,
                repo_id: &identity.repo_id,
                revision_id: &identity.revision_id,
                generation: identity.generation,
                top_k,
            },
            policy,
        );
        (plan, outcome)
    };
    for (policy, raw, ordering) in [
        (
            QueryInputPolicy::KeywordFile,
            "needle",
            "score_desc_path_tiebreak",
        ),
        (
            QueryInputPolicy::SubstringFile,
            "eedl",
            "path_order_constant_score",
        ),
        (
            QueryInputPolicy::NaturalLanguageFile,
            "needle NeverPresentBenchmarkToken",
            "score_desc_path_tiebreak",
        ),
    ] {
        // Ten distinct files: the 40-chunk file keeps one representative
        // chosen before truncation, so the nine other files still fit.
        let (plan, outcome) = run(policy, raw, 10);
        let (_plan, repeated) = run(policy, raw, 10);
        let QueryOutcome::ReturnedWindow { ref hits, .. } = outcome else {
            panic!("{} query failed: {outcome:?}", policy.as_str());
        };
        let QueryOutcome::ReturnedWindow {
            hits: ref repeated_hits,
            ..
        } = repeated
        else {
            panic!("{} repeat failed: {repeated:?}", policy.as_str());
        };
        let paths: Vec<&str> = hits.iter().map(|hit| hit.path.as_str()).collect();
        assert_eq!(
            hits.len(),
            10,
            "{}: top_k counts distinct files",
            policy.as_str()
        );
        let returned: BTreeSet<&str> = paths.iter().copied().collect();
        assert_eq!(
            returned.len(),
            10,
            "{}: files are distinct",
            policy.as_str()
        );
        assert!(
            returned.contains("src/repeated.go"),
            "{}: the 40-chunk file keeps one representative",
            policy.as_str()
        );
        if policy == QueryInputPolicy::SubstringFile {
            assert_eq!(
                returned, content_files,
                "content-only matches, grouped before top_k"
            );
        } else if policy == QueryInputPolicy::NaturalLanguageFile {
            let mut candidates = content_files.clone();
            let _inserted = candidates.insert("src/upper.go");
            assert!(
                returned.is_subset(&candidates),
                "folded phrase tokens match content, not path-only text"
            );
        } else {
            // Eleven files match content or path; the scored top ten is a subset.
            let mut candidates = content_files.clone();
            let _inserted = candidates.insert("src/needle/only_path.go");
            assert!(
                returned.is_subset(&candidates),
                "keyword files are genuine matches"
            );
        }
        assert_eq!(
            hits.iter()
                .map(|hit| (&hit.path, &hit.candidate_id, hit.score))
                .collect::<Vec<_>>(),
            repeated_hits
                .iter()
                .map(|hit| (&hit.path, &hit.candidate_id, hit.score))
                .collect::<Vec<_>>(),
            "{}: repeat preserves order, representative and score",
            policy.as_str()
        );
        if policy == QueryInputPolicy::SubstringFile {
            let mut sorted = paths.clone();
            sorted.sort_unstable();
            assert_eq!(paths, sorted, "match-only restriction returns path order");
            assert!(
                hits.windows(2)
                    .all(|pair| pair[0].score.to_bits() == pair[1].score.to_bits()),
                "match-only restriction scores every file equally"
            );
        } else {
            assert!(
                hits.windows(2).all(|pair| pair[0].score >= pair[1].score),
                "keyword files are ranked by descending score"
            );
        }
        let recorded = result_value(
            "T1",
            "lexical",
            &outcome,
            &plan,
            10,
            &files_by_path,
            &registry,
        )
        .expect("source-proven result");
        assert_eq!(recorded["rank_unit"], "distinct_file");
        assert_eq!(recorded["ordering"], ordering);
        if policy == QueryInputPolicy::SubstringFile {
            assert!(recorded.get("score_evidence").is_none());
            assert!(
                recorded["candidates"]
                    .as_array()
                    .expect("recorded rows")
                    .iter()
                    .all(|candidate| candidate.get("score").is_none())
            );
        } else {
            assert_eq!(recorded["score_evidence"], "native_sdk_score_v1");
            for (candidate, hit) in recorded["candidates"]
                .as_array()
                .expect("recorded rows")
                .iter()
                .zip(hits)
            {
                assert_eq!(candidate["score"].as_f64(), Some(hit.score));
                assert_eq!(candidate["span_accounting"]["unit_kind"], "chunk");
                assert_eq!(candidate["span_accounting"]["unit_id"], hit.candidate_id);
            }
        }
        // Continuation pages preserve the complete file list without repeats.
        let mut paged_paths = Vec::new();
        let mut page = session
            .client()
            .lexical()
            .query()
            .native(&plan.lexical_request)
            .active(identity.repo_id.clone(), identity.revision_id.clone())
            .top_k(3)
            .execute()
            .expect("first file page");
        let pinned_generation = page.generation.clone();
        let mut pages = 0;
        loop {
            pages += 1;
            assert!(pages <= 5, "file pagination must terminate");
            paged_paths.extend(
                page.results
                    .iter()
                    .map(|candidate| candidate.repo_relative_path.as_str().to_string()),
            );
            let Some(cursor) = page.next_cursor else {
                break;
            };
            page = session
                .client()
                .lexical()
                .query()
                .native(&plan.lexical_request)
                .pinned(pinned_generation.clone())
                .top_k(3)
                .after(cursor)
                .execute()
                .expect("continued file page");
        }
        // Pages continue past the top ten to every match, in the same order,
        // without repeating or skipping a file.
        assert_eq!(
            paged_paths[..10],
            paths
                .iter()
                .map(|path| (*path).to_string())
                .collect::<Vec<_>>(),
            "{}: pages preserve the top-ten order",
            policy.as_str()
        );
        assert_eq!(
            paged_paths.iter().collect::<BTreeSet<_>>().len(),
            paged_paths.len(),
            "{}: pages never repeat a file",
            policy.as_str()
        );
        assert_eq!(
            paged_paths.len(),
            if policy == QueryInputPolicy::SubstringFile {
                10
            } else {
                11
            },
            "{}: pages reach every matching file",
            policy.as_str()
        );
    }
    // Scope: a bare keyword also searches path tokens; a raw substring is content only.
    let (_plan, keyword_wide) = run(QueryInputPolicy::KeywordFile, "needle", 20);
    let (_plan, substring_wide) = run(QueryInputPolicy::SubstringFile, "eedl", 20);
    let (exact_plan, exact_content) =
        run(QueryInputPolicy::CodeSearchExactContentFile, "needle", 10);
    let (_plan, exact_upper) = run(QueryInputPolicy::CodeSearchExactContentFile, "NEEDLE", 10);
    session.stop().expect("shutdown");
    let QueryOutcome::ReturnedWindow {
        hits: keyword_hits, ..
    } = keyword_wide
    else {
        panic!("wide keyword query failed");
    };
    let QueryOutcome::ReturnedWindow {
        hits: substring_hits,
        ..
    } = substring_wide
    else {
        panic!("wide substring query failed");
    };
    let keyword_paths: BTreeSet<&str> = keyword_hits.iter().map(|hit| hit.path.as_str()).collect();
    let substring_paths: BTreeSet<&str> =
        substring_hits.iter().map(|hit| hit.path.as_str()).collect();
    assert!(keyword_paths.contains("src/needle/only_path.go"));
    assert!(!substring_paths.contains("src/needle/only_path.go"));
    // Case: `case:yes` excludes the upper-case-only file on both policies.
    assert!(!keyword_paths.contains("src/upper.go"));
    assert!(!substring_paths.contains("src/upper.go"));
    assert_eq!(substring_paths, content_files);
    let QueryOutcome::ReturnedWindow {
        hits: exact_hits, ..
    } = &exact_content
    else {
        panic!("exact-content query failed: {exact_content:?}");
    };
    let exact_paths: BTreeSet<&str> = exact_hits.iter().map(|hit| hit.path.as_str()).collect();
    assert_eq!(exact_paths, content_files);
    let QueryOutcome::ReturnedWindow {
        hits: upper_hits, ..
    } = &exact_upper
    else {
        panic!("exact-content upper-case query failed: {exact_upper:?}");
    };
    assert_eq!(upper_hits.len(), 1);
    assert_eq!(upper_hits[0].path, "src/upper.go");
    let exact_record = result_value(
        "T1",
        "lexical",
        &exact_content,
        &exact_plan,
        10,
        &files_by_path,
        &registry,
    )
    .expect("exact-content source-proven result");
    assert_eq!(exact_record["rank_unit"], "distinct_file");
    assert_eq!(exact_record["score_evidence"], "native_sdk_score_v1");
}

#[test]
fn real_daemon_query_observation_off_preserves_results_and_marks_unmeasured() {
    let repo = tempfile::tempdir().expect("repo");
    write_tiny_repo(repo.path());
    let manifest = load_manifest(&repo.path().join("manifest.json")).expect("manifest");
    let files = load_corpus(repo.path(), &manifest, &CorpusLimits::default()).expect("corpus");
    let (chunks, _coverage) = chunk_corpus(&WholeFileChunker, &files).expect("chunks");
    let identity = BatchIdentity::new(
        "bench-repo",
        "bench-rev",
        7,
        "manifest:observation".to_string(),
    )
    .expect("identity");
    let (batch, _assembly) = assemble_fixture_batch(&identity, &chunks, &files).expect("batch");
    let state = tempfile::tempdir().expect("state");
    let mut observations = Vec::new();
    for (name, policy) in [
        ("enabled", QueryStageObservationPolicy::Enabled),
        ("disabled", QueryStageObservationPolicy::Disabled),
    ] {
        let session = boot_session_with_policy(&state.path().join(name), &identity, policy);
        let (_receipt, _ack, _ingest, _sdk_timings) =
            publish_and_activate(&session, &batch, &identity, None).expect("publish");
        let mut routes = BTreeMap::new();
        for route in ["lexical", "semantic", "hybrid"] {
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
            let QueryOutcome::ReturnedWindow {
                hits,
                window,
                explanation,
                ..
            } = outcome
            else {
                panic!("observation policy changed success: {outcome:?}");
            };
            assert!(!hits.is_empty(), "fixture must exercise nonempty results");
            let mut explanation = explanation.expect("transport explanation");
            assert!(explanation.request_id.is_some_and(|id| id > 0));
            match policy {
                QueryStageObservationPolicy::Enabled => assert!(
                    explanation
                        .stage_timings
                        .take()
                        .is_some_and(|stages| !stages.is_empty())
                ),
                QueryStageObservationPolicy::Disabled => {
                    assert!(explanation.stage_timings.is_none());
                }
            }
            let rows: Vec<_> = hits.into_iter().map(|hit| serde_json::json!({
                "id": hit.candidate_id, "path": hit.path, "start": hit.start_line,
                "end": hit.end_line, "snippet": hit.snippet, "score": hit.score,
                "contributions": hit.contributions.iter().map(|part| serde_json::json!({"lane": part.lane, "rank": part.rank, "score": part.raw_score})).collect::<Vec<_>>(),
            })).collect();
            let _old = routes.insert(route, (rows, window, explanation));
        }
        observations.push(routes);
        session.stop().expect("stop");
    }
    assert_eq!(
        observations[0], observations[1],
        "only stage observations may differ"
    );
}

#[test]
fn real_daemon_experimental_fetch_floor_matches_initial_probe_without_changing_default() {
    let repo = tempfile::tempdir().expect("repo");
    write_tiny_repo(repo.path());
    let manifest = load_manifest(&repo.path().join("manifest.json")).expect("manifest");
    let files = load_corpus(repo.path(), &manifest, &CorpusLimits::default()).expect("corpus");
    let (chunks, _) = chunk_corpus(&WholeFileChunker, &files).expect("chunks");
    let identity = BatchIdentity::new("bench-repo", "bench-rev", 7, "manifest:floor".to_string())
        .expect("identity");
    let (batch, _) = assemble_fixture_batch(&identity, &chunks, &files).expect("batch");
    let state = tempfile::tempdir().expect("state");
    assert_eq!(
        HybridFetchFloorPolicy::default(),
        HybridFetchFloorPolicy::Floor100
    );
    for (policy, expected) in [
        (HybridFetchFloorPolicy::Floor25, [25, 25, 101]),
        (HybridFetchFloorPolicy::Floor50, [50, 50, 101]),
        (HybridFetchFloorPolicy::Floor100, [100, 100, 101]),
    ] {
        let session = boot_session_with_policies(
            &state.path().join(policy.as_str()),
            &identity,
            QueryStageObservationPolicy::Disabled,
            policy,
        );
        let (_receipt, _ack, _ingest, _sdk_timings) =
            publish_and_activate(&session, &batch, &identity, None).expect("publish");
        for (top_k, fetch) in [1, 10, 100].into_iter().zip(expected) {
            let outcome = query_route(&RouteQuery {
                client: session.client(),
                route: "hybrid",
                lexical_request: "sphinx quartz vaults",
                semantic_text: "sphinx quartz vaults",
                repo_id: &identity.repo_id,
                revision_id: &identity.revision_id,
                generation: identity.generation,
                top_k,
            });
            let QueryOutcome::ReturnedWindow {
                hits, explanation, ..
            } = outcome
            else {
                panic!("experimental floor failed: {outcome:?}");
            };
            assert!(!hits.is_empty());
            let explanation = explanation.expect("actual response explanation");
            assert!(explanation.stage_timings.is_none());
            let actual: Vec<_> = explanation
                .planner_trace
                .expect("actual planner trace")
                .into_iter()
                .filter(|entry| entry.detail.starts_with("hybrid.internal_top_k="))
                .collect();
            assert_eq!(
                actual,
                vec![quanta_index_contract::PlannerTraceEntry {
                    stage: quanta_index_contract::PlannerStage::Plan,
                    detail: format!("hybrid.internal_top_k={fetch}"),
                }]
            );
        }
        session.stop().expect("stop");
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
        query_stage_observation: QueryStageObservationPolicy::Enabled,
        hybrid_fetch_floor: HybridFetchFloorPolicy::default(),
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
        query_stage_observation: QueryStageObservationPolicy::Enabled,
        hybrid_fetch_floor: HybridFetchFloorPolicy::default(),
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
        query_stage_observation: QueryStageObservationPolicy::Enabled,
        hybrid_fetch_floor: HybridFetchFloorPolicy::default(),
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
        query_stage_observation: QueryStageObservationPolicy::Enabled,
        hybrid_fetch_floor: HybridFetchFloorPolicy::default(),
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
    let (batch, _) = assemble_fixture_batch(&identity, &chunks, &files).expect("batch");
    let state = tempfile::tempdir().expect("state root");
    let state_root = state.path().join("daemon");
    let config = DaemonConfig {
        state_root: &state_root,
        searchd_binary: None,
        embedder: "unavailable",
        model_dir: None,
        query_stage_observation: QueryStageObservationPolicy::Enabled,
        hybrid_fetch_floor: HybridFetchFloorPolicy::default(),
        repo_id: &identity.repo_id,
        revision_id: &identity.revision_id,
        ready_timeout: Duration::from_secs(60),
        io_timeout: Duration::from_secs(30),
        history_max_generations: 8,
    };
    let session = DaemonSession::boot(&config).expect("daemon boots");
    let (_receipt, _ack, _observation, _sdk_timings) =
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
        other @ (QueryOutcome::ReturnedWindow { .. } | QueryOutcome::RejectedResponse { .. }) => {
            panic!("unavailable provider must be an SDK failure: {other:?}")
        }
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
        other @ (QueryOutcome::ReturnedWindow { .. } | QueryOutcome::RejectedResponse { .. }) => {
            panic!("terminated daemon must be an SDK failure: {other:?}")
        }
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
    let (batch, assembly) = assemble_fixture_batch(&identity, &chunks, &files).expect("batch");
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

    let (receipt, ack, observation, sdk_timings) =
        publish_and_activate(&session, &batch, &identity, None).expect("publish+activate");
    assert_eq!(observation.repo_id, identity.repo_id);
    assert_eq!(observation.revision_id, identity.revision_id);
    assert_eq!(observation.generation, receipt.generation);
    assert_eq!(observation.batch_digest, receipt.batch_digest);
    assert!(observation.request_id > 0);
    assert_eq!(
        observation.status,
        quanta_index_contract::IngestObservationStatus::Executed
    );
    assert!(
        observation
            .semantic
            .as_ref()
            .expect("semantic measured")
            .durations
            .seal
            .is_some()
    );
    assert!(observation.lexical_build_ns.is_some());
    let stages = observation
        .lexical_stages
        .as_ref()
        .expect("lexical stages measured");
    assert!(stages.seal_file_admission_ns.is_some());
    assert!(sdk_timings.publish_ns > 0);
    assert!(sdk_timings.activation_ns > 0);
    assert!(observation.finalize_ns.is_some());
    assert!(
        observation.activation_ns.is_none(),
        "separate activation is not a server ingest stage"
    );
    let replay = session
        .client()
        .producer()
        .publish_search_corpus_observed(&batch)
        .expect("observed replay");
    assert_eq!(replay.receipt, receipt.clone().replayed());
    let replayed = replay.observation.expect("explicit replay observation");
    assert_ne!(replayed.request_id, observation.request_id);
    assert_eq!(
        replayed.status,
        quanta_index_contract::IngestObservationStatus::Replayed
    );
    assert!(replayed.semantic.is_none());
    assert!(replayed.lexical_build_ns.is_none());
    assert!(replayed.lexical_stages.is_none());
    assert!(replayed.finalize_ns.is_none());
    assert!(replayed.activation_ns.is_none());

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
        QueryOutcome::ReturnedWindow { .. } | QueryOutcome::SdkFailure { .. } => {
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
        QueryOutcome::ReturnedWindow {
            hits,
            explanation: Some(explanation),
            ..
        } => {
            assert!(!hits.is_empty(), "lexical must hit the sphinx term");
            assert!(hits.iter().any(|hit| hit.path == "src/lib.rs"), "{hits:?}");
            assert!(explanation.request_id.is_some_and(|id| id > 0));
            let stages = explanation
                .stage_timings
                .as_ref()
                .expect("lexical stage timings");
            assert_eq!(
                stages.first().expect("prepare").stage.as_str(),
                "lexical.prepare"
            );
            assert_eq!(
                stages.last().expect("project").stage.as_str(),
                "lexical.project"
            );
            assert_eq!(
                stages.last().expect("project").returned_candidates,
                Some(u64::try_from(hits.len()).expect("hit count fits u64"))
            );
            for hit in hits {
                assert!(hit.start_line >= 1 && hit.start_line <= hit.end_line);
            }
        }
        QueryOutcome::ReturnedWindow {
            explanation: None, ..
        } => panic!("lexical query omitted explanation"),
        other @ (QueryOutcome::RejectedResponse { .. } | QueryOutcome::SdkFailure { .. }) => {
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
            QueryOutcome::ReturnedWindow {
                hits,
                explanation: Some(explanation),
                ..
            } => {
                assert!(explanation.request_id.is_some_and(|id| id > 0));
                let stages = explanation
                    .stage_timings
                    .as_ref()
                    .expect("server stage timing");
                assert!(
                    stages
                        .iter()
                        .all(|stage| stage.stage.as_str().starts_with(route))
                );
                assert_eq!(
                    stages.last().and_then(|stage| stage.returned_candidates),
                    Some(u64::try_from(hits.len()).expect("hit count fits u64"))
                );
            }
            QueryOutcome::ReturnedWindow {
                explanation: None, ..
            } => {
                panic!("{route} query omitted explanation");
            }
            other @ (QueryOutcome::RejectedResponse { .. } | QueryOutcome::SdkFailure { .. }) => {
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
        other @ (QueryOutcome::ReturnedWindow { .. } | QueryOutcome::RejectedResponse { .. }) => {
            panic!("unknown route must not hit: {other:?}")
        }
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
                source_repo_id: identity.repo_id.as_str().to_string(),
                source_revision_id: identity.revision_id.as_str().to_string(),
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
    let completed_results = outcomes
        .iter()
        .map(|((task, route), outcome)| {
            let row = quanta_index_retrieval_bench::record::result_value(
                task,
                route,
                outcome,
                &plans[task],
                10,
                &files_by_path,
                &published_units,
            )
            .expect("live completed response");
            ((task.clone(), route.clone()), row)
        })
        .collect();
    let record = runner_record(&RunnerRecordInput {
        pack: &pack,
        identity: &runner_identity,
        provenance: &provenance,
        captures: &captures,
        outcomes: &outcomes,
        completed_results: &completed_results,
        plans: &plans,
        nl_config: &nl_config,
        top_k: 10,
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
    assert_eq!(record["span_accounting_version"], 1);
    let mut witnessed = 0;
    for row in record["results"].as_array().expect("results") {
        for candidate in row["candidates"].as_array().expect("candidates") {
            let accounting = &candidate["span_accounting"];
            assert!(accounting["unit_id"].as_str().is_some());
            assert!(accounting["producer_identity"].as_str().is_some());
            assert!(
                accounting["indexed_start_byte"]
                    .as_u64()
                    .expect("indexed start")
                    < accounting["indexed_end_byte"]
                        .as_u64()
                        .expect("indexed end")
            );
            witnessed += 1;
        }
    }
    assert!(witnessed > 0, "live record must emit indexed-span evidence");
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
fn actual_preflight_binary_reports_all_files_before_strict_refusal() {
    let fixture = tempfile::tempdir().expect("fixture");
    let repo = fixture.path().join("repo");
    std::fs::create_dir(&repo).expect("repo");
    write_repo(
        &repo,
        &[
            (
                "a.ts",
                include_str!("fixtures/l5_parser/generic_import_trailing_comma.ts"),
            ),
            (
                "b.tsx",
                include_str!("fixtures/l5_parser/type_namespace_export.ts"),
            ),
            ("empty.ts", ""),
            ("z.ts", "invalid code"),
        ],
    );
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(repo.join("manifest.json")).expect("manifest"))
            .expect("JSON");
    std::fs::remove_file(repo.join("manifest.json")).expect("externalize manifest");
    drop(git(&repo, &["init", "--quiet"]));
    drop(git(
        &repo,
        &["config", "user.email", "retrieval-bench@example.invalid"],
    ));
    drop(git(&repo, &["config", "user.name", "Retrieval Bench"]));
    drop(git(&repo, &["add", "."]));
    drop(git(&repo, &["commit", "--quiet", "-m", "fixture"]));
    manifest["repository_commit"] = git(&repo, &["rev-parse", "HEAD"]).into();
    let manifest_path = fixture.path().join("manifest.json");
    std::fs::write(&manifest_path, serde_json::to_vec(&manifest).expect("JSON")).expect("manifest");
    let runner = env!("CARGO_BIN_EXE_quanta-index-retrieval-bench");
    for (policy, success) in [("require-complete", false), ("allow-incomplete", true)] {
        let report_path = fixture.path().join(format!("{policy}.json"));
        let output = Command::new(runner)
            .args([
                "preflight",
                "--repo",
                repo.to_str().expect("path"),
                "--manifest",
                manifest_path.to_str().expect("path"),
                "--symbol-coverage",
                policy,
                "--out",
                report_path.to_str().expect("path"),
            ])
            .output()
            .expect("preflight process");
        assert_eq!(
            output.status.success(),
            success,
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.status.code(), Some(if success { 0 } else { 2 }));
        if !success {
            assert!(String::from_utf8_lossy(&output.stderr).contains("symbol preflight refused"));
        }
        let report: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&report_path).expect("complete report"))
                .expect("JSON");
        let rows = report["preflight"]["files"]
            .as_array()
            .expect("file census");
        assert_eq!(rows.len(), 4);
        assert_eq!(report["preflight"]["incomplete_files"], 1);
        for (row, (path, state, count)) in rows.iter().zip([
            ("a.ts", "complete", Some(1)),
            ("b.tsx", "complete", Some(1)),
            ("empty.ts", "complete", Some(0)),
            ("z.ts", "parse_failed", None),
        ]) {
            assert_eq!(row["path"], path);
            assert_eq!(row["coverage"]["state"], state);
            assert_eq!(row["coverage"]["symbol_count"].as_u64(), count);
        }
        // A repeated invocation must preserve the first diagnostic artifact.
        let before = std::fs::read(&report_path).expect("report");
        let retry = Command::new(runner)
            .args([
                "preflight",
                "--repo",
                repo.to_str().expect("path"),
                "--manifest",
                manifest_path.to_str().expect("path"),
                "--symbol-coverage",
                policy,
                "--out",
                report_path.to_str().expect("path"),
            ])
            .output()
            .expect("repeat process");
        assert!(!retry.status.success());
        assert_eq!(
            std::fs::read(&report_path).expect("retained report"),
            before
        );
    }
}

#[test]
fn incomplete_symbol_profile_publishes_malformed_text_and_refuses_symbol_authority() {
    use quanta_index_retrieval_bench::symbols::{
        SymbolCoveragePolicy, SymbolPreflightOptions, preflight_corpus_symbols,
    };
    let repo = tempfile::tempdir().expect("repo");
    write_repo(
        repo.path(),
        &[
            ("a.ts", "function sentinel() {}"),
            ("empty.ts", ""),
            ("notes.md", "unsupportedmarker source text"),
            ("notes.txt", "plaintextmarker source text"),
            ("LICENSE", "extensionlessmarker source text"),
            ("opaque.custom", "unclassifiedmarker source text"),
            ("z.ts", "invalid code"),
        ],
    );
    let manifest = load_manifest(&repo.path().join("manifest.json")).expect("manifest");
    let files = load_corpus(repo.path(), &manifest, &CorpusLimits::default()).expect("corpus");
    let (chunks, _) = chunk_corpus(&WholeFileChunker, &files).expect("chunks");
    let by_path = files
        .into_iter()
        .map(|file| (file.path.clone(), file))
        .collect();
    let preflight =
        preflight_corpus_symbols(&by_path, &SymbolPreflightOptions::default()).expect("preflight");
    assert!(
        preflight
            .admit(SymbolCoveragePolicy::RequireComplete)
            .is_err()
    );
    let identity = BatchIdentity::new(
        "incomplete-repo",
        "source-revision",
        1,
        "manifest:incomplete".into(),
    )
    .expect("identity");
    let (batch, assembly) = assemble_batch(
        &identity,
        &chunks,
        &by_path,
        &preflight,
        SymbolCoveragePolicy::AllowIncomplete,
        quanta_index_contract::SourcePublicationEvent {
            stream_id: "incomplete-fixture".into(),
            event_id: "initial-source".into(),
            expected_base_event_id: None,
            payload_sha256: [0; 32],
        },
        true,
    )
    .expect("explicit incomplete admission");
    assert_eq!(assembly.scopes, 7);
    assert_eq!(assembly.empty_scopes, ["empty.ts"]);
    let state = tempfile::tempdir().expect("state");
    let session = boot_session(&state.path().join("daemon"), &identity);
    let (receipt, _activation, observation, _sdk_timings) =
        publish_and_activate(&session, &batch, &identity, None).expect("publish source facts");
    assert_eq!(receipt.accepted_replace_scopes, 7);
    assert_eq!(observation.repo_id, identity.repo_id);
    let query = |route, text| {
        query_route(&RouteQuery {
            client: session.client(),
            route,
            lexical_request: text,
            semantic_text: text,
            repo_id: &identity.repo_id,
            revision_id: &identity.revision_id,
            generation: identity.generation,
            top_k: 10,
        })
    };
    let lexical = query("lexical", "invalid");
    let unsupported_text = query("lexical", "unsupportedmarker");
    let plain_text = [
        ("notes.txt", query("lexical", "plaintextmarker")),
        ("LICENSE", query("lexical", "extensionlessmarker")),
        ("opaque.custom", query("lexical", "unclassifiedmarker")),
    ];
    let narrow = query("symbol", "path:a.ts sentinel");
    let empty = query("symbol", "path:empty.ts sentinel");
    let refusals = [
        ("symbol", "sentinel"),
        ("symbol", "path:z.ts absent"),
        ("lexical", "select:symbol sentinel"),
        ("lexical", "type:symbol sentinel"),
    ]
    .map(|(route, text)| (route, text, query(route, text)));
    session.stop().expect("stop");
    let QueryOutcome::ReturnedWindow { hits, .. } = lexical else {
        panic!("malformed source text must remain searchable: {lexical:?}");
    };
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].path, "z.ts");
    let QueryOutcome::ReturnedWindow { hits, .. } = unsupported_text else {
        panic!("unsupported-language source must remain searchable: {unsupported_text:?}");
    };
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].path, "notes.md");
    for (path, outcome) in plain_text {
        let QueryOutcome::ReturnedWindow { hits, .. } = outcome else {
            panic!("admitted plain source must remain searchable: {outcome:?}");
        };
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, path);
    }
    let QueryOutcome::ReturnedWindow { hits, .. } = narrow else {
        panic!("complete narrow symbol scope must remain queryable: {narrow:?}");
    };
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].path, "a.ts");
    let QueryOutcome::ReturnedWindow { hits, window, .. } = empty else {
        panic!("complete zero-symbol scope must be an ordinary empty result: {empty:?}");
    };
    assert!(hits.is_empty());
    assert!(window.outcome().is_exhausted());
    for (route, text, outcome) in refusals {
        assert!(
            matches!(outcome, QueryOutcome::SdkFailure { ref code, .. } if code == "SYMBOL_COVERAGE_INCOMPLETE"),
            "{route} {text}: {outcome:?}"
        );
    }
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
    let metrics_out = evidence.join("phase-metrics.json");
    let refusal_out = evidence.join("query-plan-refusal.json");
    let state = evidence.join("state");
    let mut command = Command::new(&runner);
    let _command = command.args([
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
        "--experimental-hybrid-fetch-floor",
        "50",
        "--metrics-out",
        metrics_out.to_str().expect("metrics path"),
        "--refusal-out",
        refusal_out.to_str().expect("refusal path"),
    ]);
    let sibling = evidence.join("sibling");
    std::fs::create_dir(&sibling).expect("sibling dir");
    let mut aliases = vec![sibling.join("../record.json"), state.clone()];
    #[cfg(unix)]
    {
        let parent_alias = fixture.path().join("evidence-alias");
        std::os::unix::fs::symlink(&evidence, &parent_alias).expect("parent alias");
        aliases.push(parent_alias.join("phase-metrics.json"));
    }
    for alias in aliases {
        let refused = Command::new(&runner)
            .args(command.get_args())
            .arg("--symbol-preflight-out")
            .arg(&alias)
            .output()
            .expect("conflicting output probe starts");
        assert_eq!(refused.status.code(), Some(2));
        assert!(
            String::from_utf8_lossy(&refused.stderr)
                .contains("must differ from every other output/state path"),
            "unexpected refusal: {}",
            String::from_utf8_lossy(&refused.stderr)
        );
        for path in [
            &out,
            &metrics_out,
            &diagnostic_out,
            &refusal_out,
            &state,
            &alias,
        ] {
            assert!(
                !path.exists(),
                "conflict must refuse before creating {path:?}"
            );
        }
    }
    let output = command.output().expect("runner starts");
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
    assert_eq!(record["span_accounting_version"], 1);
    let metrics: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&metrics_out).expect("phase metrics bytes"))
            .expect("phase metrics JSON");
    assert_eq!(metrics["schema_version"], 4);
    let query_observations = metrics["query_timing"]["observations"]
        .as_array()
        .expect("complete query timing observations");
    assert!(!query_observations.is_empty());
    for observation in query_observations {
        let start = observation["start_ns"].as_u64().expect("query start");
        let end = observation["end_ns"].as_u64().expect("query end");
        let children = [
            "sdk_execute_ns",
            "sdk_post_execute_ns",
            "runner_result_materialize_ns",
        ]
        .iter()
        .map(|key| observation[*key].as_u64().expect("query timing child"))
        .sum::<u64>();
        assert!(start <= end && children <= end - start);
    }
    let preflight_bytes = std::fs::read(
        evidence.join(
            metrics["symbol_preflight_out"]
                .as_str()
                .expect("preflight artifact path"),
        ),
    )
    .expect("preflight artifact bytes");
    assert_eq!(
        metrics["symbol_preflight_sha256"],
        sha256_hex(&preflight_bytes)
    );
    let preflight: serde_json::Value =
        serde_json::from_slice(&preflight_bytes).expect("preflight JSON");
    assert_eq!(preflight["preflight"]["admitted_files"], universe.len());
    assert_eq!(preflight["preflight"]["incomplete_files"], 0);
    assert_eq!(metrics["symbol_coverage_policy"], "require-complete");
    assert_eq!(metrics["symbol_incomplete_files"], 0);
    assert!(
        metrics["phases_ms"]["symbol_preflight"]
            .as_f64()
            .is_some_and(|value| value >= 0.0)
    );
    // Schema 4 retains SDK child intervals inside the publish envelope.
    // Only disjoint phases partition total time; children must fit their parent.
    let phases = metrics["phases_ms"].as_object().expect("phase ledger");
    let sdk_children = ["sdk_publish", "sdk_activate"];
    let sdk_sum = sdk_children
        .iter()
        .map(|name| {
            let measured = phases[*name].as_f64().expect("measured SDK child");
            assert!(measured.is_finite() && measured >= 0.0);
            measured
        })
        .sum::<f64>();
    let publish = phases["embed_publish_seal_activate"]
        .as_f64()
        .expect("publish envelope");
    assert!(
        sdk_sum <= publish + 0.01,
        "SDK children exceed publish envelope"
    );
    let phase_sum = phases
        .iter()
        .filter(|(name, _)| !sdk_children.contains(&name.as_str()))
        .map(|(_, value)| {
            let measured = value.as_f64().expect("measured disjoint phase");
            assert!(measured.is_finite() && measured >= 0.0);
            measured
        })
        .sum::<f64>();
    let total = metrics["total_ms"].as_f64().expect("overall measured time");
    assert!(
        (phase_sum - total).abs() <= total.max(1.0) * 1e-9,
        "phase ledger must account for each duration once: sum={phase_sum} total={total}"
    );
    let coverage = metrics["symbol_coverage"]
        .as_array()
        .expect("per-file symbol coverage");
    assert_eq!(coverage.len(), universe.len());
    for (row, (path, source_sha)) in coverage.iter().zip(&universe) {
        assert_eq!(row["path"], *path);
        assert_eq!(row["source_sha256"], *source_sha);
        assert_eq!(row["language"], "rust");
    }
    assert_eq!(
        coverage
            .iter()
            .map(|row| row["definition_count"].as_u64().expect("definition count"))
            .sum::<u64>(),
        metrics["symbol_count"].as_u64().expect("symbol count")
    );
    let diagnostic: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&diagnostic_out).expect("diagnostic bytes"))
            .expect("diagnostic JSON");
    assert_eq!(
        diagnostic["schema_version"],
        serde_json::json!(quanta_index_retrieval_bench::diagnostics::DIAGNOSTIC_SCHEMA_VERSION)
    );
    assert_eq!(
        diagnostic["hybrid_fetch_policy"],
        quanta_index_retrieval_bench::diagnostics::hybrid_fetch_policy_value(
            HybridFetchFloorPolicy::Floor50,
        )
        .expect("canonical floor config")
    );
    assert_eq!(
        diagnostic["server_observation"],
        quanta_index_retrieval_bench::diagnostics::server_observation_value(
            QueryStageObservationPolicy::Enabled
        )
        .expect("canonical config")
    );
    let raw_ingest = &diagnostic["ingest"];
    let raw_receipt: quanta_index_sdk::BatchReceipt =
        serde_json::from_value(raw_ingest["receipt"].clone()).expect("strict durable receipt");
    let raw_ack: quanta_index_contract::SearchPlaneSearchCorpusActivationCasAck =
        serde_json::from_value(raw_ingest["activation_ack"].clone()).expect("strict activation");
    let observation: quanta_index_contract::SearchCorpusIngestObservation =
        serde_json::from_value(raw_ingest["observation"].clone())
            .expect("strict transient observation");
    assert_eq!(observation.repo_id.as_str(), "runner-binary-repo");
    assert_eq!(observation.revision_id.as_str(), commit);
    assert_eq!(observation.generation.get(), 1);
    assert_eq!(observation.batch_digest, raw_receipt.batch_digest);
    assert_eq!(
        observation.status,
        quanta_index_contract::IngestObservationStatus::Executed
    );
    assert!(observation.activation_ns.is_none());
    assert!(observation.request_id > 0);
    for capture in captures.values() {
        assert_eq!(
            capture["receipt_digest"],
            receipt_digest(&raw_receipt).expect("receipt hash")
        );
        assert_eq!(
            capture["activation_digest"],
            activation_digest(&raw_ack).expect("activation hash")
        );
    }
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
    for route in ["lexical", "semantic", "hybrid"] {
        let row = diagnostic["results"]
            .as_array()
            .expect("diagnostic results")
            .iter()
            .find(|row| row["route"] == route)
            .expect("measured route result");
        let explanation = &row["response"]["explanation"];
        assert!(
            explanation["request_id"]
                .as_u64()
                .is_some_and(|request_id| request_id > 0),
            "measured route must retain transport request id"
        );
        let stages = explanation["stage_timings"]
            .as_array()
            .expect("server stage timings");
        assert!(stages.iter().all(|stage| {
            stage["stage"]
                .as_str()
                .is_some_and(|name| name.starts_with(route))
        }));
        assert_eq!(
            stages.last().expect("last stage")["returned_candidates"],
            row["response"]["window"]["returned"]
        );
    }
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
        let destination = PathBuf::from(dir);
        std::fs::create_dir_all(&destination).expect("evidence output dir");
        for (source, name) in [
            (&out, "actual-runner-record.json"),
            (&pack_path, "actual-runner-pack.json"),
            (&diagnostic_out, "actual-runner-diagnostic.json"),
        ] {
            let _copied =
                std::fs::copy(source, destination.join(name)).expect("evidence artifact copies");
        }
    }
    verify_rank_study_runner(&command, &pack_path, &evidence);
}

/// Exercise the separately pinned runner and daemon.
///
/// The top-one file fixture has three `pub` occurrences and needs three pages.
/// A one-page diagnostic cap must retain the original capped record.
fn verify_rank_study_runner(original_command: &Command, original_pack: &Path, evidence: &Path) {
    let mut pack: serde_json::Value =
        serde_json::from_slice(&std::fs::read(original_pack).expect("original blind pack"))
            .expect("pack JSON");
    pack["routes"] = serde_json::json!(["lexical"]);
    pack["comparison_contract"]["top_k"] = serde_json::json!(1);
    pack["tasks"][0]["query"] = serde_json::json!("pub");
    pack["tasks"][0]["query_sha256"] = serde_json::json!(sha256_hex(b"pub"));
    let pack_path = evidence.join("rank-study-pack.json");
    std::fs::write(
        &pack_path,
        serde_json::to_vec_pretty(&pack).expect("blind pack JSON"),
    )
    .expect("rank pack");
    let original_args: Vec<_> = original_command
        .get_args()
        .map(|arg| arg.to_str().expect("fixture UTF-8 argument").to_string())
        .collect();
    for (name, max_pages, complete) in [("complete", "10", true), ("limited", "1", false)] {
        let root = evidence.join(format!("rank-study-{name}"));
        std::fs::create_dir(&root).expect("fresh diagnostic root");
        let replacements = BTreeMap::from([
            ("--query-pack", pack_path.display().to_string()),
            ("--routes", "lexical".into()),
            ("--query-input-policy", "code_search_file".into()),
            ("--top-k", "1".into()),
            ("--state-root", root.join("state").display().to_string()),
            ("--out", root.join("record.json").display().to_string()),
            (
                "--diagnostics-out",
                root.join("diagnostic.json").display().to_string(),
            ),
            (
                "--metrics-out",
                root.join("phase.json").display().to_string(),
            ),
            (
                "--refusal-out",
                root.join("refusal.json").display().to_string(),
            ),
        ]);
        let mut args = vec![original_args[0].clone()];
        for pair in original_args[1..].chunks_exact(2) {
            args.push(pair[0].clone());
            args.push(
                replacements
                    .get(pair[0].as_str())
                    .cloned()
                    .unwrap_or_else(|| pair[1].clone()),
            );
        }
        let artifact_path = root.join("rank-study.json");
        let output = Command::new(original_command.get_program())
            .args(args)
            .arg("--rank-study-out")
            .arg(&artifact_path)
            .arg("--rank-study-max-pages")
            .arg(max_pages)
            .output()
            .expect("actual study runner");
        assert!(
            output.status.success(),
            "study failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let record_bytes =
            std::fs::read(root.join("record.json")).expect("original quality record");
        let record: serde_json::Value = serde_json::from_slice(&record_bytes).expect("record");
        assert_eq!(record["results"][0]["status"], "capped");
        assert_eq!(
            record["results"][0]["candidates"]
                .as_array()
                .expect("top-one")
                .len(),
            1
        );
        let artifact: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&artifact_path).expect("study artifact"))
                .expect("study JSON");
        assert_eq!(artifact["record_sha256"], sha256_hex(&record_bytes));
        assert_eq!(artifact["qualification"], "diagnostic_unqualified");
        let row = &artifact["results"][0];
        let capture = &record["captures"][record["route_provenance"]["lexical"]["capture_id"]
            .as_str()
            .expect("capture ID")];
        assert_eq!(capture["source_repo_id"], row["generation"]["repo_id"]);
        assert_eq!(
            capture["source_revision_id"],
            row["generation"]["revision_id"]
        );
        assert_eq!(row["effective_request"]["generation"], row["generation"]);
        let collection = &row["collection"];
        assert_eq!(collection["pool_complete"], complete);
        assert_eq!(
            collection["pages"].as_array().expect("native pages").len(),
            if complete { 3 } else { 1 }
        );
        if complete {
            assert_eq!(collection["status"], "returned");
            let paths: Vec<_> = collection["explanations"]
                .as_array()
                .expect("native explanations")
                .iter()
                .map(|item| {
                    assert_eq!(item["status"], "returned");
                    let response: quanta_index_contract::SearchPlaneExplainQueryResponse =
                        serde_json::from_value(item["response"].clone())
                            .expect("strict native explain DTO");
                    assert_eq!(
                        response.presence,
                        quanta_index_contract::CandidatePresenceV1::Indexed
                    );
                    assert_eq!(response.explanation.contributions.len(), 1);
                    assert_eq!(
                        f64::from(response.explanation.contributions[0].contribution).to_bits(),
                        item["candidate"]["score"]
                            .as_f64()
                            .expect("native candidate score")
                            .to_bits()
                    );
                    item["candidate"]["repo_relative_path"]
                        .as_str()
                        .expect("native path")
                        .to_string()
                })
                .collect();
            assert_eq!(paths, ["src/alpha.rs", "src/beta.rs", "src/lib.rs"]);
        } else {
            assert_eq!(collection["status"], "partial");
            assert_eq!(collection["reason"], "diagnostic_page_limit");
        }
        if let Some(dir) = std::env::var_os("QUANTA_BENCH_SDK_EVIDENCE_DIR") {
            let destination = PathBuf::from(dir);
            for (source, suffix) in [
                (root.join("record.json"), "record"),
                (artifact_path, "study"),
                (pack_path.clone(), "pack"),
            ] {
                let _copied = std::fs::copy(
                    source,
                    destination.join(format!("rank-study-{name}-{suffix}.json")),
                )
                .expect("external diagnostic copy");
            }
        }
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
    let (batch, _) = assemble_fixture_batch(&identity, &chunks, &files).expect("batch");

    let state = tempfile::tempdir().expect("state root");
    let state_root = state.path().join("daemon");
    let session = boot_session(&state_root, &identity);
    let (_receipt, _ack, _observation, _sdk_timings) =
        publish_and_activate(&session, &batch, &identity, None).expect("publish");
    session.stop().expect("stop");
    // The used root still holds index data: a second boot must refuse it.
    let config = DaemonConfig {
        state_root: &state_root,
        searchd_binary: None,
        embedder: EMBEDDER,
        model_dir: None,
        query_stage_observation: QueryStageObservationPolicy::Enabled,
        hybrid_fetch_floor: HybridFetchFloorPolicy::default(),
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
    let (left, _) = assemble_fixture_batch(&identity, &chunks, &files).expect("batch");
    let (right, _) = assemble_fixture_batch(&identity, &chunks, &files).expect("batch");
    assert_eq!(
        left.batch_digest().expect("digest"),
        right.batch_digest().expect("digest")
    );
}

#[test]
fn exact_symbol_name_public_route_preserves_case_homonyms_and_source_spans() {
    use quanta_index_retrieval_bench::published_units::{PublishedUnitKind, PublishedUnitRegistry};

    let repo = tempfile::tempdir().expect("repo root");
    write_repo(
        repo.path(),
        &[
            (
                "lower/a.go",
                "package fixture\n\nfunc writeContentType() {}\n",
            ),
            (
                "lower/b.go",
                "package fixture\n\nfunc writeContentType() {}\n",
            ),
            (
                "upper/c.go",
                "package fixture\n\nfunc WriteContentType() {}\n",
            ),
        ],
    );
    let manifest = load_manifest(&repo.path().join("manifest.json")).expect("manifest");
    let files = load_corpus(repo.path(), &manifest, &CorpusLimits::default()).expect("corpus");
    let (chunks, _) = chunk_corpus(&WholeFileChunker, &files).expect("chunks");
    let symbols = symbols_for(&files);
    let files_by_path: BTreeMap<_, _> = files
        .iter()
        .map(|file| (file.path.clone(), file.clone()))
        .collect();
    let units = PublishedUnitRegistry::from_chunks_and_symbols(&chunks, &symbols, &files_by_path)
        .expect("published units");
    let identity = BatchIdentity::new(
        "bench-repo",
        "bench-rev",
        24,
        "manifest:exact-case".to_string(),
    )
    .expect("identity");
    let (batch, _) = assemble_fixture_batch(&identity, &chunks, &files).expect("batch");
    let state = tempfile::tempdir().expect("state root");
    let session = boot_session(&state.path().join("daemon"), &identity);
    let (_receipt, _ack, _observation, _sdk_timings) =
        publish_and_activate(&session, &batch, &identity, None).expect("publish+activate");

    for (name, expected_paths) in [
        ("writeContentType", vec!["lower/a.go", "lower/b.go"]),
        ("WriteContentType", vec!["upper/c.go"]),
        ("WRITECONTENTTYPE", Vec::new()),
        ("missingSymbol", Vec::new()),
    ] {
        let expected_ids: BTreeMap<_, _> = symbols
            .values()
            .flatten()
            .filter(|symbol| symbol.local_name.as_ref() == name)
            .map(|symbol| {
                (
                    symbol.repo_relative_path.as_str().to_string(),
                    symbol.symbol_id.as_str().to_string(),
                )
            })
            .collect();
        assert_eq!(
            expected_ids.keys().map(String::as_str).collect::<Vec<_>>(),
            expected_paths,
            "publication must contain precisely the independent fixture definitions"
        );
        let plan = plan_query(
            QueryInputPolicy::ExactSymbolName,
            name,
            &NlPlanConfig::default(),
        )
        .expect("exact symbol plan");
        assert_eq!(
            plan.lexical_request,
            format!("symbol.local_name.exact({name}) case:yes")
        );
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
        let hits = match &outcome {
            QueryOutcome::ReturnedWindow { hits, .. } => hits,
            other @ (QueryOutcome::RejectedResponse { .. } | QueryOutcome::SdkFailure { .. }) => {
                panic!("exact symbol query {name} failed: {other:?}")
            }
        };
        let actual_ids: BTreeMap<_, _> = hits
            .iter()
            .map(|hit| (hit.path.clone(), hit.candidate_id.clone()))
            .collect();
        assert_eq!(
            hits.len(),
            expected_paths.len(),
            "exact hit count for {name}"
        );
        assert_eq!(
            actual_ids, expected_ids,
            "exact paths and published IDs for {name}"
        );
        for hit in hits {
            let unit = units.get(&hit.candidate_id).expect("published symbol hit");
            assert_eq!(unit.kind, PublishedUnitKind::Symbol);
            // Independent fixture oracle: each definition is bytes [17,43), line 3.
            assert_eq!((unit.byte_start, unit.byte_end), (17, 43));
            assert_eq!((hit.start_line, hit.end_line), (3, 3));
        }
        let record = result_value(
            "T-exact",
            "symbol",
            &outcome,
            &plan,
            10,
            &files_by_path,
            &units,
        )
        .expect("exact symbol result record");
        assert_eq!(
            record["status"],
            if expected_paths.is_empty() {
                "abstained"
            } else {
                "success"
            }
        );
        assert_eq!(record["rank_unit"], "symbol");
        let candidates = record["candidates"].as_array().expect("candidates");
        assert_eq!(candidates.len(), expected_paths.len());
        for candidate in candidates {
            assert_eq!(candidate["span_accounting"]["unit_kind"], "symbol");
            assert_eq!(candidate["span_accounting"]["indexed_start_byte"], 17);
            assert_eq!(candidate["span_accounting"]["indexed_end_byte"], 43);
            assert_eq!(candidate["start_byte"], 17);
            assert_eq!(candidate["end_byte"], 44);
            let path = candidate["path"].as_str().expect("path");
            assert_eq!(candidate["span_accounting"]["unit_id"], expected_ids[path]);
            assert_eq!(candidate["file_sha256"], files_by_path[path].sha256);
        }
    }
    session.stop().expect("bounded shutdown");
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
    let (batch, assembly) = assemble_fixture_batch(&identity, &chunks, &files).expect("batch");
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
    let (_receipt, _ack, _observation, _sdk_timings) =
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
    let literal = plan_query(
        QueryInputPolicy::Literal,
        "sphinx_riddle",
        &NlPlanConfig::default(),
    )
    .expect("literal plan");
    let unsupported = query_route(&RouteQuery {
        client: session.client(),
        route: "symbol",
        lexical_request: &literal.lexical_request,
        semantic_text: &literal.semantic_text,
        repo_id: &identity.repo_id,
        revision_id: &identity.revision_id,
        generation: identity.generation,
        top_k: 10,
    });
    session.stop().expect("bounded shutdown");
    let hits = match outcome {
        QueryOutcome::ReturnedWindow { hits, .. } => hits,
        other @ (QueryOutcome::RejectedResponse { .. } | QueryOutcome::SdkFailure { .. }) => {
            panic!("symbol route failed: {other:?}")
        }
    };
    match unsupported {
        QueryOutcome::SdkFailure { code, message, .. } => {
            assert_eq!(code, "LEX_PLANNER_UNSUPPORTED_FILTER_COMBO");
            assert!(
                !message.is_empty(),
                "unsupported symbol text needs a reason"
            );
        }
        other @ (QueryOutcome::ReturnedWindow { .. } | QueryOutcome::RejectedResponse { .. }) => {
            panic!("unsupported literal symbol must refuse, not exhaust: {other:?}");
        }
    }
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
    let (batch, _) = assemble_fixture_batch(&identity, &chunks, &files).expect("batch");
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
        .expect("published units");
    let state = tempfile::tempdir().expect("state root");
    let session = boot_session(&state.path().join("daemon"), &identity);
    let (_receipt, _ack, _observation, _sdk_timings) =
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
    let outcome = &outcome;
    match outcome {
        QueryOutcome::ReturnedWindow { hits, window, .. } => {
            assert!(
                hits.is_empty(),
                "nonsense symbol name must not produce hits: {hits:?}"
            );
            assert!(
                window.outcome().is_exhausted(),
                "empty symbol window must be an exact abstention"
            );
            // The record layer maps this exact shape to "abstained", never
            // to a fake success or a silent error (audit finding 16).
            let record = result_value(
                "T1",
                "symbol",
                outcome,
                &plan_query(
                    QueryInputPolicy::Native,
                    "zzz_no_such_symbol_zzz",
                    &NlPlanConfig::default(),
                )
                .expect("plan"),
                10,
                &files_by_path,
                &published_units,
            )
            .expect("no-answer maps to a record row");
            assert_eq!(record["status"], "abstained");
        }
        QueryOutcome::RejectedResponse { code, .. } => {
            // Only binding-level refusals are acceptable here; a ranking
            // path defect must not hide behind transport classes.
            assert!(
                code.contains("generation") || code.contains("pin"),
                "unexpected rejection for a no-answer symbol query: {code}"
            );
        }
        QueryOutcome::SdkFailure { status, code, .. } => {
            assert!(
                matches!(*status, "unavailable" | "timeout"),
                "no-answer must not surface as {status}/{code}"
            );
        }
    }
}

#[test]
fn sentence_and_identifier_queries_anchor_the_same_definition_over_distractors() {
    // RBR-02 acceptance: a natural-language sentence and a bare
    // identifier must find the same definition while comment and
    // reference distractors compete, and the executed requests must be
    // the planned, policy-distinct ones.
    let repo = tempfile::tempdir().expect("repo root");
    write_repo(
        repo.path(),
        &[
            (
                "src/registry.rs",
                concat!(
                    "// The cache_key_for helper resolves tenant cache keys.\n",
                    "// Callers mention cache_key_for in comments only.\n",
                    "pub fn cache_key_for(tenant: &str) -> String {\n",
                    "    format!(\"tenant:{tenant}\")\n",
                    "}\n",
                ),
            ),
            (
                "src/elsewhere.rs",
                concat!(
                    "// A reference site, not the definition.\n",
                    "pub fn warmup() {\n",
                    "    let _ = crate::registry::cache_key_for(\"acme\");\n",
                    "}\n",
                ),
            ),
        ],
    );
    let manifest = load_manifest(&repo.path().join("manifest.json")).expect("manifest");
    let files = load_corpus(repo.path(), &manifest, &CorpusLimits::default()).expect("corpus");
    let (chunks, _) = chunk_corpus(&WholeFileChunker, &files).expect("chunk");
    let symbols = symbols_for(&files);
    let identity = BatchIdentity::new(
        "bench-repo",
        "bench-rev",
        21,
        "manifest:distract".to_string(),
    )
    .expect("identity");
    let (batch, _) = assemble_fixture_batch(&identity, &chunks, &files).expect("batch");
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
        .expect("published units");
    let state = tempfile::tempdir().expect("state root");
    let session = boot_session(&state.path().join("daemon"), &identity);
    let (_receipt, _ack, _observation, _sdk_timings) =
        publish_and_activate(&session, &batch, &identity, None).expect("publish+activate");

    // The two policies produce distinct executed lexical requests for the
    // same definition intent; both must anchor hits in registry.rs.
    let sentence = "Where is the tenant cache key computed?";
    let identifier = "cache_key_for";
    let sentence_plan = plan_query(
        QueryInputPolicy::NaturalLanguage,
        sentence,
        &NlPlanConfig::default(),
    )
    .expect("nl plan");
    let identifier_plan = plan_query(
        QueryInputPolicy::Native,
        identifier,
        &NlPlanConfig::default(),
    )
    .expect("native plan");
    assert_ne!(
        sentence_plan.effective_lexical_request_sha256,
        identifier_plan.effective_lexical_request_sha256,
        "the two policies must execute distinct requests"
    );
    for plan in [&sentence_plan, &identifier_plan] {
        let outcome = query_route(&RouteQuery {
            client: session.client(),
            route: "lexical",
            lexical_request: &plan.lexical_request,
            semantic_text: &plan.semantic_text,
            repo_id: &identity.repo_id,
            revision_id: &identity.revision_id,
            generation: identity.generation,
            top_k: 5,
        });
        let hits = match &outcome {
            QueryOutcome::ReturnedWindow { hits, .. } => hits,
            QueryOutcome::RejectedResponse { code, message, .. }
            | QueryOutcome::SdkFailure { code, message, .. } => {
                panic!("lexical query failed: {code} {message}")
            }
        };
        assert!(
            hits.iter().any(|hit| hit.path == "src/registry.rs"),
            "the definition file must surface for policy-planned query"
        );
        let record = result_value(
            "T1",
            "lexical",
            &outcome,
            plan,
            5,
            &files_by_path,
            &published_units,
        )
        .expect("policy-planned hits prove");
        assert!(
            record["candidates"]
                .as_array()
                .is_some_and(|rows| !rows.is_empty()),
            "the record must carry proven candidates"
        );
    }
    session.stop().expect("bounded shutdown");
}

#[test]
fn homonymous_symbols_stay_distinct_units_on_the_symbol_route() {
    // RBR-05 acceptance: same-named definitions in different files must
    // resolve as distinct symbol units; the route never conflates them
    // and never borrows a chunk identity.
    let repo = tempfile::tempdir().expect("repo root");
    write_repo(
        repo.path(),
        &[
            ("src/agent.rs", "pub fn register() -> u32 {\n    1\n}\n"),
            ("src/device.rs", "pub fn register() -> u32 {\n    2\n}\n"),
        ],
    );
    let manifest = load_manifest(&repo.path().join("manifest.json")).expect("manifest");
    let files = load_corpus(repo.path(), &manifest, &CorpusLimits::default()).expect("corpus");
    let (chunks, _) = chunk_corpus(&WholeFileChunker, &files).expect("chunk");
    let symbols = symbols_for(&files);
    let identity = BatchIdentity::new("bench-repo", "bench-rev", 22, "manifest:homon".to_string())
        .expect("identity");
    let (batch, _) = assemble_fixture_batch(&identity, &chunks, &files).expect("batch");
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
        .expect("published units");
    // Two same-named symbols, distinct ids and paths.
    let registers: Vec<_> = symbols
        .values()
        .flatten()
        .filter(|symbol| &*symbol.local_name == "register")
        .collect();
    assert_eq!(registers.len(), 2, "both definitions publish");
    assert_ne!(
        registers[0].symbol_id.as_str(),
        registers[1].symbol_id.as_str()
    );
    let state = tempfile::tempdir().expect("state root");
    let session = boot_session(&state.path().join("daemon"), &identity);
    let (_receipt, _ack, _observation, _sdk_timings) =
        publish_and_activate(&session, &batch, &identity, None).expect("publish+activate");
    let plan = plan_query(
        QueryInputPolicy::Native,
        "register",
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
    let hits = match &outcome {
        QueryOutcome::ReturnedWindow { hits, .. } => hits,
        QueryOutcome::RejectedResponse { code, message, .. }
        | QueryOutcome::SdkFailure { code, message, .. } => {
            panic!("symbol query failed: {code} {message}")
        }
    };
    let mut seen_ids = std::collections::BTreeSet::new();
    for hit in hits {
        let unit = published_units
            .get(&hit.candidate_id)
            .unwrap_or_else(|| panic!("hit is not a published unit: {}", hit.candidate_id));
        assert_eq!(
            unit.kind,
            quanta_index_retrieval_bench::published_units::PublishedUnitKind::Symbol,
            "homonym hits must stay symbol units, never chunk identities"
        );
        assert!(
            matches!(hit.path.as_str(), "src/agent.rs" | "src/device.rs"),
            "unexpected path: {}",
            hit.path
        );
        let _duplicate = seen_ids.insert(hit.candidate_id.clone());
    }
    assert!(
        seen_ids.len() <= 2 && !seen_ids.is_empty(),
        "at most the two homonyms, each at most once: {seen_ids:?}"
    );
}
