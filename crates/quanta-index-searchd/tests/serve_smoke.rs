//! End-to-end binary smoke tests.
//!
//! Three scenarios, each spawning the real `searchd` binary:
//!   1. Cold serve + fail-closed: empty `state_root`, request returns
//!      `not_ready`, SIGINT clean exit (H-SP2 / H-SP3 happy clamp).
//!   2. Happy-path lexical roundtrip: `state_root` pre-materialized with a
//!      Tantivy index + control plane active row, request returns real
//!      `LexicalCandidate` hits.
//!   3. Process restart preserves state: spawn → query → SIGTERM → respawn →
//!      query again, expect same active generation + same hits (U-SP3).
#![expect(
    clippy::wildcard_enum_match_arm,
    reason = "test asserts against expected SearchPlaneIpcResponse variant only"
)]
#![expect(
    clippy::disallowed_methods,
    reason = "test driver reads JoinHandle outputs via .join().unwrap_or_default()"
)]
#![expect(
    clippy::single_match_else,
    reason = "match arms keep the error context next to its assertion"
)]

pub mod support;

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use quanta_index_contract::{
    BundleArtifactRef, BundleEncoding, GenerationId, LqDirectiveSet, LqExpr, LqFilter, LqFilterSet,
    LqOptionSet, LqQuery, ManifestDigest, ManifestGeneration, PublishedGenerationSet,
    PublishedSearchBundleManifest, RepoId, RevisionId, SearchPlaneIpcRequest,
    SearchPlaneIpcRequestEnvelope, SearchPlaneIpcResponse, SearchPlaneLexicalQueryRequest,
};
use quanta_index_control::ControlPlane;
use quanta_index_core::{
    LexicalBuildInput, PublishedSearchActivationStatePort, PublishedSearchGenerationCatalogPort,
    SearchPlaneLexicalIndexBuildPort,
};
use quanta_index_ipc::{decode_response, encode_request};
use quanta_index_lexical::TantivyLexicalAdapter;

use self::support::SearchdTestPaths;

macro_rules! ok_or_fail {
    ($expr:expr, $msg:expr) => {
        match $expr {
            Ok(v) => v,
            Err(error) => {
                assert!(false, "{}: {error}", $msg);
                return;
            }
        }
    };
}

#[test]
fn serve_binary_listens_accepts_request_and_shuts_down_on_sigint() {
    let paths = ok_or_fail!(SearchdTestPaths::new(), "test path setup");

    let mut child = ok_or_fail!(
        Command::new(searchd_binary_path())
            .arg("serve")
            .env("QUANTA_INDEX_STATE_ROOT", paths.state_root())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn(),
        "spawn searchd"
    );
    let Some(stdout) = child.stdout.take() else {
        assert!(false, "child stdout was not captured");
        return;
    };
    let Some(stderr) = child.stderr.take() else {
        assert!(false, "child stderr was not captured");
        return;
    };

    let stdout_collector = thread::spawn(move || drain_to_string(stdout));
    let stderr_collector = thread::spawn(move || drain_to_string(stderr));

    // Wait until the socket file is present + connectable.
    let socket_path = paths.socket_path().to_path_buf();
    if !wait_for_socket(&socket_path, Duration::from_secs(5)) {
        let _killed: std::io::Result<()> = child.kill();
        let _waited: std::io::Result<std::process::ExitStatus> = child.wait();
        assert!(false, "searchd did not bind socket within 5s");
        return;
    }

    // Open a connection and send a lexical request that has no matching
    // generation in the control plane → expect typed NotReady Error envelope.
    let mut stream = ok_or_fail!(UnixStream::connect(&socket_path), "client connect");
    let envelope = SearchPlaneIpcRequestEnvelope {
        request_id: 1,
        payload: SearchPlaneIpcRequest::Lexical(SearchPlaneLexicalQueryRequest {
            query: LqQuery {
                expr: LqExpr::Raw("anything".into()),
                filters: LqFilterSet {
                    filters: vec![
                        LqFilter::Repo("nonexistent".into()),
                        LqFilter::Rev("rev".into()),
                    ],
                },
                options: LqOptionSet {
                    limit: None,
                    count_all: false,
                    timeout_ms: None,
                },
                directives: LqDirectiveSet { directives: vec![] },
            },
            generation: None,
        }),
    };
    let bytes = ok_or_fail!(encode_request(&envelope), "encode");
    ok_or_fail!(stream.write_all(&bytes), "write request");
    ok_or_fail!(stream.flush(), "flush");

    // Read response: 4-byte LE header + body.
    let mut header = [0u8; 4];
    ok_or_fail!(stream.read_exact(&mut header), "read header");
    let length = u32::from_le_bytes(header);
    let length_usize = match usize::try_from(length) {
        Ok(v) => v,
        Err(error) => {
            assert!(false, "length overflow: {error}");
            return;
        }
    };
    let mut body = vec![0u8; length_usize];
    ok_or_fail!(stream.read_exact(&mut body), "read body");
    let mut full = Vec::with_capacity(4_usize.saturating_add(length_usize));
    full.extend_from_slice(&header);
    full.extend_from_slice(&body);
    let mut cursor = std::io::Cursor::new(full);
    let response = ok_or_fail!(decode_response(&mut cursor), "decode response");
    assert_eq!(response.request_id, 1, "request_id should round-trip");
    match response.payload {
        SearchPlaneIpcResponse::Error(error) => {
            // Expected: typed not_ready error because no generation is active.
            assert_eq!(error.code, "NOT_READY", "unexpected code: {error:?}");
        }
        other => {
            assert!(false, "expected Error envelope, got {other:?}");
        }
    }

    // Close the client connection then send SIGINT.
    drop(stream);
    send_sigint(child.id());

    let exit = ok_or_fail!(
        wait_with_timeout(&mut child, Duration::from_secs(5)),
        "wait child"
    );
    assert!(exit.success(), "searchd exited non-zero: {exit:?}");

    let stdout = stdout_collector.join().unwrap_or_default();
    let stderr = stderr_collector.join().unwrap_or_default();
    assert!(
        stdout.contains("listening on"),
        "stdout missing startup line; stdout={stdout:?}"
    );
    assert!(
        stderr.contains("SIGINT") || stderr.contains("draining"),
        "stderr missing shutdown notice; stderr={stderr:?}"
    );
    assert!(
        paths.control_plane_path().exists(),
        "control-plane DB should have been created"
    );
}

fn drain_to_string<R: Read>(mut reader: R) -> String {
    let mut buf = String::new();
    let _bytes: std::io::Result<usize> = reader.read_to_string(&mut buf);
    buf
}

fn wait_for_socket(path: &std::path::Path, deadline: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < deadline {
        if path.exists() && UnixStream::connect(path).is_ok() {
            return true;
        }
        thread::sleep(Duration::from_millis(50));
    }
    false
}

fn send_sigint(pid: u32) {
    // SAFETY: libc::kill is FFI; we wrap it via `Command` to keep the test
    // crate free of `unsafe`. The lint forbids unsafe at the workspace level.
    let _status: std::io::Result<std::process::ExitStatus> = Command::new("kill")
        .arg("-INT")
        .arg(pid.to_string())
        .status();
}

fn wait_with_timeout(
    child: &mut std::process::Child,
    deadline: Duration,
) -> std::io::Result<std::process::ExitStatus> {
    let start = Instant::now();
    loop {
        match child.try_wait()? {
            Some(status) => return Ok(status),
            None => {
                if start.elapsed() >= deadline {
                    let _killed: std::io::Result<()> = child.kill();
                    return child.wait();
                }
                thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

fn searchd_binary_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_quanta-index-searchd"))
}

// ---------------------------------------------------------------------------
// Happy-path + restart tests
// ---------------------------------------------------------------------------

/// Sample generation used by both happy-path tests.
fn happy_path_generation() -> PublishedGenerationSet {
    PublishedGenerationSet {
        repo_id: RepoId::new("repo"),
        revision_id: RevisionId::new("rev"),
        manifest_generation: ManifestGeneration::new(7),
        lexical_generation: GenerationId::new(10),
        symbol_generation: GenerationId::new(11),
        structural_generation: None,
        history_generation: None,
        semantic_generation: None,
        metadata_generation: None,
    }
}

/// Pre-materialise a `state_root` for the happy-path / restart tests.
///
/// Builds a Tantivy index, records the manifest in the control plane, marks
/// the generation active. After this returns, spawning `searchd` against the
/// same `state_root` yields a serve loop that can answer lexical queries for
/// `repo=repo, rev=rev, gen=7`.
fn prepare_state_root(state_root: &std::path::Path) {
    // Build a tiny lexical index with one matching chunk and one decoy.
    let lexical = TantivyLexicalAdapter::with_state_root(state_root.to_path_buf());
    let manifest = PublishedSearchBundleManifest {
        repo_id: RepoId::new("repo"),
        revision_id: RevisionId::new("rev"),
        manifest_generation: ManifestGeneration::new(7),
        bundle_schema_version: 1,
        lexical_chunk_rows: BundleArtifactRef {
            relative_path: "bundle/chunk.json".into(),
            encoding: BundleEncoding::Json,
            byte_length: 1,
            content_digest: ManifestDigest::new("d"),
        },
        symbol_rows: BundleArtifactRef {
            relative_path: "bundle/symbol.json".into(),
            encoding: BundleEncoding::Json,
            byte_length: 1,
            content_digest: ManifestDigest::new("d"),
        },
        metadata_rows: None,
        graph_rows: None,
        embedding_input_views: None,
        embedding_records: None,
        mutation_delta: None,
    };
    let chunk_rows = br#"[
        {"repo_relative_path":"src/lib.rs","start_line":1,"end_line":10,"text":"the brown fox jumps"},
        {"repo_relative_path":"src/main.rs","start_line":20,"end_line":30,"text":"goodbye world"}
    ]"#;
    let outcome = lexical.build_lexical_index(
        &manifest,
        LexicalBuildInput {
            chunk_rows,
            symbol_rows: b"[]",
        },
    );
    if let Err(error) = outcome {
        assert!(false, "build_lexical: {error}");
        return;
    }

    let control_path = state_root.join("control-plane.sqlite3");
    let mut control = match ControlPlane::open(&control_path) {
        Ok(store) => store,
        Err(error) => {
            assert!(false, "open control: {error}");
            return;
        }
    };
    if let Err(error) = control.record_generation_manifest(manifest) {
        assert!(false, "record manifest: {error}");
        return;
    }
    if let Err(error) = control.mark_active_generation(&happy_path_generation(), 100) {
        assert!(false, "mark active: {error}");
    }
}

/// Build a Lexical-Raw("fox") request targeting the pre-materialised
/// generation. Returns a fresh envelope per call so the same fixture can be
/// re-used across multiple roundtrips.
fn happy_path_lexical_request(request_id: u64) -> SearchPlaneIpcRequestEnvelope {
    SearchPlaneIpcRequestEnvelope {
        request_id,
        payload: SearchPlaneIpcRequest::Lexical(SearchPlaneLexicalQueryRequest {
            query: LqQuery {
                expr: LqExpr::Raw("fox".into()),
                filters: LqFilterSet { filters: vec![] },
                options: LqOptionSet {
                    limit: None,
                    count_all: false,
                    timeout_ms: None,
                },
                directives: LqDirectiveSet { directives: vec![] },
            },
            generation: Some(happy_path_generation()),
        }),
    }
}

fn read_one_response(stream: &mut UnixStream) -> SearchPlaneIpcResponse {
    let mut header = [0u8; 4];
    if let Err(error) = stream.read_exact(&mut header) {
        assert!(false, "read header: {error}");
        return SearchPlaneIpcResponse::Error(quanta_index_contract::SearchPlaneIpcError {
            code: "test".into(),
            message: "header read failed".into(),
        });
    }
    let length = u32::from_le_bytes(header);
    let length_usize = match usize::try_from(length) {
        Ok(v) => v,
        Err(error) => {
            assert!(false, "length overflow: {error}");
            return SearchPlaneIpcResponse::Error(quanta_index_contract::SearchPlaneIpcError {
                code: "test".into(),
                message: "length overflow".into(),
            });
        }
    };
    let mut body = vec![0u8; length_usize];
    if let Err(error) = stream.read_exact(&mut body) {
        assert!(false, "read body: {error}");
        return SearchPlaneIpcResponse::Error(quanta_index_contract::SearchPlaneIpcError {
            code: "test".into(),
            message: "body read failed".into(),
        });
    }
    let mut full = Vec::with_capacity(4_usize.saturating_add(length_usize));
    full.extend_from_slice(&header);
    full.extend_from_slice(&body);
    let mut cursor = std::io::Cursor::new(full);
    match decode_response(&mut cursor) {
        Ok(r) => r.payload,
        Err(error) => {
            assert!(false, "decode response: {error:?}");
            SearchPlaneIpcResponse::Error(quanta_index_contract::SearchPlaneIpcError {
                code: "test".into(),
                message: "decode failed".into(),
            })
        }
    }
}

#[test]
fn serve_binary_happy_path_lexical_roundtrip_over_uds() {
    // Real binary + pre-materialised state. Sends a lexical request through
    // the UDS front door and asserts we get a `Lexical` response with
    // candidates pointing at `src/lib.rs` (the only doc matching "fox").
    let paths = ok_or_fail!(SearchdTestPaths::new(), "test path setup");
    prepare_state_root(paths.state_root());

    let mut child = ok_or_fail!(
        Command::new(searchd_binary_path())
            .arg("serve")
            .env("QUANTA_INDEX_STATE_ROOT", paths.state_root())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn(),
        "spawn searchd"
    );

    let socket_path = paths.socket_path().to_path_buf();
    if !wait_for_socket(&socket_path, Duration::from_secs(5)) {
        let _killed: std::io::Result<()> = child.kill();
        let _waited: std::io::Result<std::process::ExitStatus> = child.wait();
        assert!(false, "searchd did not bind socket within 5s");
        return;
    }

    let mut client = ok_or_fail!(UnixStream::connect(&socket_path), "client connect");
    let envelope = happy_path_lexical_request(101);
    let bytes = ok_or_fail!(encode_request(&envelope), "encode");
    ok_or_fail!(client.write_all(&bytes), "write");
    ok_or_fail!(client.flush(), "flush");

    let payload = read_one_response(&mut client);
    match payload {
        SearchPlaneIpcResponse::Lexical(response) => {
            assert_eq!(
                response.generation.manifest_generation.get(),
                7,
                "response should pin manifest_generation=7"
            );
            assert_eq!(
                response.results.len(),
                1,
                "expected exactly one hit for 'fox', got {:?}",
                response.results
            );
            let Some(hit) = response.results.first() else {
                assert!(false, "no hit");
                return;
            };
            assert_eq!(hit.repo_relative_path.as_str(), "src/lib.rs");
            assert!(hit.snippet.contains("fox"));
        }
        other => {
            assert!(false, "expected Lexical response, got {other:?}");
        }
    }

    drop(client);
    send_sigint(child.id());
    let exit = ok_or_fail!(
        wait_with_timeout(&mut child, Duration::from_secs(5)),
        "wait child"
    );
    assert!(exit.success(), "searchd exited non-zero: {exit:?}");
}

#[test]
fn serve_binary_restart_preserves_active_generation_and_index() {
    // U-SP3: real process restart. Spawn → query → SIGTERM → spawn again →
    // query against the same state_root. The second spawn must observe the
    // same materialised lexical index and active generation pointer.
    let paths = ok_or_fail!(SearchdTestPaths::new(), "test path setup");
    prepare_state_root(paths.state_root());

    // ---- first generation of searchd ----
    let mut child = ok_or_fail!(
        Command::new(searchd_binary_path())
            .arg("serve")
            .env("QUANTA_INDEX_STATE_ROOT", paths.state_root())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn(),
        "spawn 1"
    );
    let socket_path = paths.socket_path().to_path_buf();
    if !wait_for_socket(&socket_path, Duration::from_secs(5)) {
        let _killed: std::io::Result<()> = child.kill();
        let _waited: std::io::Result<std::process::ExitStatus> = child.wait();
        assert!(false, "spawn 1: socket bind timeout");
        return;
    }
    let mut client1 = ok_or_fail!(UnixStream::connect(&socket_path), "client connect 1");
    let bytes = ok_or_fail!(encode_request(&happy_path_lexical_request(1)), "encode 1");
    ok_or_fail!(client1.write_all(&bytes), "write 1");
    ok_or_fail!(client1.flush(), "flush 1");
    let payload1 = read_one_response(&mut client1);
    let first_hits = match payload1 {
        SearchPlaneIpcResponse::Lexical(r) => r.results,
        other => {
            assert!(false, "spawn 1 expected Lexical, got {other:?}");
            return;
        }
    };
    assert_eq!(first_hits.len(), 1);
    drop(client1);
    send_sigint(child.id());
    let exit = ok_or_fail!(
        wait_with_timeout(&mut child, Duration::from_secs(5)),
        "wait 1"
    );
    assert!(exit.success(), "spawn 1 exit non-zero: {exit:?}");

    // ---- second generation of searchd against the same state_root ----
    let mut child2 = ok_or_fail!(
        Command::new(searchd_binary_path())
            .arg("serve")
            .env("QUANTA_INDEX_STATE_ROOT", paths.state_root())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn(),
        "spawn 2"
    );
    if !wait_for_socket(&socket_path, Duration::from_secs(5)) {
        let _killed: std::io::Result<()> = child2.kill();
        let _waited: std::io::Result<std::process::ExitStatus> = child2.wait();
        assert!(false, "spawn 2: socket bind timeout");
        return;
    }
    let mut client2 = ok_or_fail!(UnixStream::connect(&socket_path), "client connect 2");
    let bytes = ok_or_fail!(encode_request(&happy_path_lexical_request(2)), "encode 2");
    ok_or_fail!(client2.write_all(&bytes), "write 2");
    ok_or_fail!(client2.flush(), "flush 2");
    let payload2 = read_one_response(&mut client2);
    match payload2 {
        SearchPlaneIpcResponse::Lexical(response) => {
            assert_eq!(response.generation.manifest_generation.get(), 7);
            assert_eq!(
                response.results.len(),
                first_hits.len(),
                "post-restart hit count must match pre-restart"
            );
            let first_path = first_hits
                .first()
                .map(|c| c.repo_relative_path.as_str().to_owned())
                .unwrap_or_default();
            let after_path = response
                .results
                .first()
                .map(|c| c.repo_relative_path.as_str().to_owned())
                .unwrap_or_default();
            assert_eq!(first_path, after_path, "post-restart hit path must match");
        }
        other => {
            assert!(false, "spawn 2 expected Lexical, got {other:?}");
        }
    }
    drop(client2);
    send_sigint(child2.id());
    let exit2 = ok_or_fail!(
        wait_with_timeout(&mut child2, Duration::from_secs(5)),
        "wait 2"
    );
    assert!(exit2.success(), "spawn 2 exit non-zero: {exit2:?}");
}
