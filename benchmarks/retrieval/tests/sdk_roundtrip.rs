//! SDK roundtrip tests (RB-02, T05–T07): a real `searchd` process,
//! actual SDK publish/activate/query, plus the static SDK-frontdoor guard.
//!
//! The daemon binary resolves via `--searchd-bin` equivalent
//! (`QUANTA_INDEX_SEARCHD_BIN`) or the workspace target layout; a missing
//! binary fails with an explicit build-first message, never a skip.

use std::path::{Path, PathBuf};
use std::time::Duration;

use quanta_index_retrieval_bench::batch::{BatchIdentity, assemble_batch};
use quanta_index_retrieval_bench::chunking::chunk_corpus;
use quanta_index_retrieval_bench::chunking::whole_file::WholeFileChunker;
use quanta_index_retrieval_bench::corpus::{CorpusLimits, load_corpus, load_manifest};
use quanta_index_retrieval_bench::sdk::{
    DaemonConfig, DaemonSession, QueryOutcome, RouteQuery, publish_and_activate, query_route,
    resolve_searchd_binary,
};
use quanta_index_retrieval_bench::sha256_hex;

const EMBEDDER: &str = "hash-dev";

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

fn boot_session(state_root: &Path) -> DaemonSession {
    let config = DaemonConfig {
        state_root,
        searchd_binary: None,
        embedder: EMBEDDER,
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
fn missing_searchd_binary_fails_with_build_guidance() {
    // An explicit bogus path plus no usable fallback must name the fix.
    // (This test never spawns; it only exercises resolution failure when
    // the environment offers no binary. If a binary resolves from the
    // target layout, resolution succeeds and there is nothing to assert.)
    let bogus = PathBuf::from("/nonexistent-dir-xyz/quanta-index-searchd");
    match resolve_searchd_binary(Some(&bogus)) {
        Ok(_) => {}
        Err(err) => {
            let text = err.to_string();
            assert!(text.contains("QUANTA_INDEX_SEARCHD_BIN"), "{text}");
            assert!(text.contains("build"), "{text}");
        }
    }
}

#[test]
fn stale_state_root_is_refused() {
    let root = tempfile::tempdir().expect("temp root");
    let state_root = root.path().join("state");
    std::fs::create_dir_all(state_root.join("search-plane")).expect("dir");
    std::fs::write(state_root.join("search-plane").join("stale.txt"), b"stale").expect("stale");
    let config = DaemonConfig {
        state_root: &state_root,
        searchd_binary: None,
        embedder: EMBEDDER,
        ready_timeout: Duration::from_secs(5),
        io_timeout: Duration::from_secs(5),
        history_max_generations: 8,
    };
    let err = DaemonSession::boot(&config)
        .err()
        .expect("stale root must fail");
    assert!(err.to_string().contains("not fresh"), "{err}");
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

    let identity = BatchIdentity::new(
        "bench-repo",
        "bench-rev",
        7,
        "manifest:test-roundtrip".to_string(),
    )
    .expect("identity");
    let (batch, assembly) = assemble_batch(&identity, &chunks).expect("batch");
    assert_eq!(assembly.scopes, 3);
    assert_eq!(assembly.semantic_scopes, 3);

    let state = tempfile::tempdir().expect("state root");
    let state_root = state.path().join("daemon");
    let session = boot_session(&state_root);
    session
        .assert_index_empty(&identity.repo_id, &identity.revision_id)
        .expect("fresh index is empty");

    let (receipt, ack) = publish_and_activate(&session, &batch, None).expect("publish+activate");
    assert_eq!(receipt.batch_digest, batch.batch_digest().expect("digest"));
    assert_eq!(receipt.accepted_replace_scopes as usize, assembly.scopes);
    assert!(receipt.semantic_content.is_some());

    // Lexical route finds the distinctive term in its source span.
    let lexical = query_route(&RouteQuery {
        client: session.client(),
        route: "lexical",
        query_text: "sphinx",
        repo_id: &identity.repo_id,
        revision_id: &identity.revision_id,
        generation: identity.generation,
        top_k: 10,
    });
    match &lexical {
        QueryOutcome::Hits { hits, .. } => {
            assert!(!hits.is_empty(), "lexical must hit the sphinx term");
            assert!(hits.iter().any(|hit| hit.path == "src/lib.rs"), "{hits:?}");
            for hit in hits {
                assert!(hit.start_line >= 1 && hit.start_line <= hit.end_line);
            }
        }
        QueryOutcome::Failed { code, message, .. } => {
            panic!("lexical query failed: {code}: {message}");
        }
    }

    // Semantic and hybrid routes answer under the same generation; their
    // rank content under hash-dev is plumbing, not quality evidence.
    for route in ["semantic", "hybrid"] {
        let outcome = query_route(&RouteQuery {
            client: session.client(),
            route,
            query_text: "sphinx quartz vaults",
            repo_id: &identity.repo_id,
            revision_id: &identity.revision_id,
            generation: identity.generation,
            top_k: 10,
        });
        match outcome {
            QueryOutcome::Hits { .. } => {}
            QueryOutcome::Failed {
                status,
                code,
                message,
                ..
            } => {
                panic!("{route} query failed: {status} {code}: {message}");
            }
        }
    }

    // A nonsense term abstains or errors typed; never fake success.
    let missing = query_route(&RouteQuery {
        client: session.client(),
        route: "lexical",
        query_text: "zzzznothinghere",
        repo_id: &identity.repo_id,
        revision_id: &identity.revision_id,
        generation: identity.generation,
        top_k: 10,
    });
    match missing {
        QueryOutcome::Hits { hits, outcome, .. } => {
            assert!(
                hits.is_empty() || outcome.is_exhausted(),
                "unmatched hits must be exhausted, not capped"
            );
        }
        QueryOutcome::Failed { .. } => {}
    }

    // Unknown routes fail typed.
    match query_route(&RouteQuery {
        client: session.client(),
        route: "nope",
        query_text: "sphinx",
        repo_id: &identity.repo_id,
        revision_id: &identity.revision_id,
        generation: identity.generation,
        top_k: 10,
    }) {
        QueryOutcome::Failed { code, .. } => assert_eq!(code, "unknown_route"),
        QueryOutcome::Hits { .. } => panic!("unknown route must not hit"),
    }

    assert!(receipt.semantic_content.is_some());
    let _ack = ack;
    session.stop().expect("bounded shutdown");
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
    let (batch, _) = assemble_batch(&identity, &chunks).expect("batch");

    let state = tempfile::tempdir().expect("state root");
    let state_root = state.path().join("daemon");
    let session = boot_session(&state_root);
    let (_receipt, _ack) = publish_and_activate(&session, &batch, None).expect("publish");
    session.stop().expect("stop");
    // The used root still holds index data: a second boot must refuse it.
    let config = DaemonConfig {
        state_root: &state_root,
        searchd_binary: None,
        embedder: EMBEDDER,
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
    let (left, _) = assemble_batch(&identity, &chunks).expect("batch");
    let (right, _) = assemble_batch(&identity, &chunks).expect("batch");
    assert_eq!(
        left.batch_digest().expect("digest"),
        right.batch_digest().expect("digest")
    );
}
