//! End-to-end scenario matrix from SSOT § Scenario Matrix.
//!
//! Each test exercises the full search-plane stack — control plane, lexical
//! adapter, semantic adapter, materialize orchestrator, and where applicable
//! the domain query engine and UDS listener — to demonstrate that the
//! usecase / edge / corner / hellgate behaviour from the SSOT holds end to
//! end. Tests are intentionally chatty about which scenario they cover so
//! grep'ping for `U-SP1`, `E-SP2`, `H-SP3` etc. lands on the canonical proof.
#![expect(
    clippy::redundant_clone,
    reason = "scenario fixtures intentionally clone PathBuf / generation for readable setup"
)]

use quanta_index_contract::{
    BundleArtifactRef, BundleEncoding, GenerationId, LqDirectiveSet, LqExpr, LqFilter, LqFilterSet,
    LqOptionSet, LqQuery, ManifestDigest, ManifestGeneration, PreparedBundleOutbox,
    PublishedGenerationSet, PublishedSearchBundleManifest, PublishedSearchBundlePrepareRequest,
    PublishedSearchGenerationActivateRequest, RepoId, RevisionId, SearchPlaneIpcRequest,
    SearchPlaneIpcRequestEnvelope, SearchPlaneIpcResponse, SearchPlaneLexicalQueryRequest,
};
use quanta_index_control::ControlPlane;
use quanta_index_core::{
    CoreError, GenerationPinPort, PublishedSearchActivationStatePort,
    PublishedSearchBundleInspectPort, PublishedSearchBundlePreparePort,
    PublishedSearchGenerationActivatePort, PublishedSearchGenerationReadinessPort,
    SearchPlaneLexicalIndexStorePort, SearchPlaneLexicalQueryPort, SearchPlaneVectorIndexStorePort,
};
use quanta_index_ipc::{decode_response, encode_request};
use quanta_index_lexical::TantivyLexicalAdapter;
use quanta_index_searchd::app::{MaterializeUseCase, UdsListener};
use quanta_index_searchd::query::DomainQueryEngine;
use quanta_index_semantic::LanceSemanticAdapter;
use sha2::{Digest, Sha256};
use tempfile::tempdir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

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

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        let hi = usize::from(byte >> 4);
        let lo = usize::from(byte & 0x0f);
        out.push(char::from(HEX.get(hi).copied().unwrap_or(b'0')));
        out.push(char::from(HEX.get(lo).copied().unwrap_or(b'0')));
    }
    out
}

fn write_artifact_with_digest(
    bundle_root: &std::path::Path,
    relative: &str,
    body: &[u8],
) -> BundleArtifactRef {
    let full = bundle_root.join(relative);
    if let Some(parent) = full.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        assert!(false, "create parent: {error}");
    }
    if let Err(error) = std::fs::write(&full, body) {
        assert!(false, "write artifact: {error}");
    }
    let mut hasher = Sha256::new();
    hasher.update(body);
    let digest = hex_lower(&hasher.finalize());
    let byte_length = match u64::try_from(body.len()) {
        Ok(v) => v,
        Err(error) => {
            assert!(false, "body overflow: {error}");
            0
        }
    };
    BundleArtifactRef {
        relative_path: relative.into(),
        encoding: BundleEncoding::Json,
        byte_length,
        content_digest: ManifestDigest::new(digest),
    }
}

fn sample_generation(manifest_gen: u64) -> PublishedGenerationSet {
    PublishedGenerationSet {
        repo_id: RepoId::new("repo"),
        revision_id: RevisionId::new("rev"),
        manifest_generation: ManifestGeneration::new(manifest_gen),
        lexical_generation: GenerationId::new(manifest_gen.saturating_mul(10)),
        symbol_generation: GenerationId::new(manifest_gen.saturating_mul(10).saturating_add(1)),
        structural_generation: None,
        history_generation: None,
        semantic_generation: None,
        metadata_generation: None,
    }
}

fn sample_manifest_pair(
    bundle_root: &std::path::Path,
    chunk_rows_json: &[u8],
) -> (PublishedSearchBundleManifest, PublishedGenerationSet) {
    let chunk_ref = write_artifact_with_digest(bundle_root, "bundle/chunk.json", chunk_rows_json);
    let symbol_ref = write_artifact_with_digest(bundle_root, "bundle/symbol.json", b"[]");
    let manifest = PublishedSearchBundleManifest {
        repo_id: RepoId::new("repo"),
        revision_id: RevisionId::new("rev"),
        manifest_generation: ManifestGeneration::new(7),
        bundle_schema_version: 1,
        lexical_chunk_rows: chunk_ref,
        symbol_rows: symbol_ref,
        metadata_rows: None,
        graph_rows: None,
        embedding_input_views: None,
        embedding_records: None,
        mutation_delta: None,
    };
    (manifest, sample_generation(7))
}

// ---------------------------------------------------------------------------
// Usecase scenarios
// ---------------------------------------------------------------------------

#[test]
fn u_sp1_producer_prepare_makes_outbox_row_visible() {
    // U-SP1: prepare carries an outbox; readiness reflects the new row.
    let dir = ok_or_fail!(tempdir(), "tempdir");
    let db = dir.path().join("c.sqlite3");
    let mut control = ok_or_fail!(ControlPlane::open(&db), "open control");
    let outbox = sample_outbox();
    let response = ok_or_fail!(
        control.prepare_bundle(PublishedSearchBundlePrepareRequest {
            outbox: outbox.clone()
        }),
        "prepare"
    );
    assert!(response.accepted);
    let readiness = ok_or_fail!(
        control.read_readiness(&outbox.repo_id, &outbox.revision_id),
        "readiness"
    );
    assert_eq!(readiness.prepared_bundle_count, 1);
}

#[test]
fn u_sp2_activate_then_materialize_then_query_round_trips() {
    // U-SP2: end-to-end happy path through materialize + query engine.
    let dir = ok_or_fail!(tempdir(), "tempdir");
    let state_root = dir.path().to_path_buf();
    let bundle_root = state_root.join("bundles");
    let chunk_rows = br#"[
        {"repo_relative_path":"src/lib.rs","start_line":1,"end_line":10,"text":"the quick brown fox"},
        {"repo_relative_path":"src/main.rs","start_line":20,"end_line":30,"text":"goodbye world"}
    ]"#;
    let (manifest, generation) = sample_manifest_pair(&bundle_root, chunk_rows);

    let mut control = ok_or_fail!(
        ControlPlane::open(&state_root.join("c.sqlite3")),
        "open control"
    );
    let mut lexical = TantivyLexicalAdapter::with_state_root(state_root.clone());
    let mut semantic = LanceSemanticAdapter::with_state_root(state_root.clone());
    let mut uc = MaterializeUseCase {
        control: &mut control,
        lexical: &mut lexical,
        semantic: &mut semantic,
        bundle_root,
        now_ms: 123,
    };
    let _outcome: quanta_index_searchd::app::MaterializeOutcome =
        ok_or_fail!(uc.materialize(manifest, generation.clone()), "materialize");

    // Readiness reflects the new active generation.
    let readiness = ok_or_fail!(
        control.read_readiness(&generation.repo_id, &generation.revision_id),
        "readiness"
    );
    assert_eq!(readiness.active_generation, Some(generation.clone()));
    assert!(readiness.lexical_ready);
    assert!(readiness.semantic_ready);

    // Open both stores: lexical must report ready, semantic (no embeddings)
    // reports Ok by convention (orchestrator owned the no-vectors decision).
    ok_or_fail!(
        lexical.open_lexical_store(&generation),
        "open lexical store"
    );
    ok_or_fail!(semantic.open_vector_store(&generation), "open vector store");

    // Query through DomainQueryEngine.
    let engine = DomainQueryEngine {
        lexical: &lexical,
        semantic: &semantic,
        control: &control,
        embedder: None,
        default_top_k: 5,
    };
    let response = ok_or_fail!(
        engine.lexical_query(SearchPlaneLexicalQueryRequest {
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
            generation: Some(generation.clone()),
        }),
        "lexical query"
    );
    assert_eq!(response.results.len(), 1);
    assert_eq!(
        response
            .results
            .first()
            .map(|hit| hit.repo_relative_path.as_str()),
        Some("src/lib.rs"),
    );
}

#[test]
fn u_sp3_cold_restart_preserves_state() {
    // U-SP3: simulate restart by dropping ControlPlane + reopening from same
    // state_root. Readiness + inspect must still report the previously
    // activated generation.
    let dir = ok_or_fail!(tempdir(), "tempdir");
    let state_root = dir.path().to_path_buf();
    let bundle_root = state_root.join("bundles");
    let chunk_rows =
        br#"[{"repo_relative_path":"a.rs","start_line":1,"end_line":2,"text":"alpha"}]"#;
    let (manifest, generation) = sample_manifest_pair(&bundle_root, chunk_rows);

    {
        let mut control = ok_or_fail!(ControlPlane::open(&state_root.join("c.sqlite3")), "open");
        let mut lexical = TantivyLexicalAdapter::with_state_root(state_root.clone());
        let mut semantic = LanceSemanticAdapter::with_state_root(state_root.clone());
        let mut uc = MaterializeUseCase {
            control: &mut control,
            lexical: &mut lexical,
            semantic: &mut semantic,
            bundle_root: bundle_root.clone(),
            now_ms: 100,
        };
        let _outcome: quanta_index_searchd::app::MaterializeOutcome = ok_or_fail!(
            uc.materialize(manifest.clone(), generation.clone()),
            "materialize"
        );
        // drop control + adapters
    }

    // Restart.
    let control = ok_or_fail!(
        ControlPlane::open(&state_root.join("c.sqlite3")),
        "reopen control"
    );
    let readiness = ok_or_fail!(
        control.read_readiness(&generation.repo_id, &generation.revision_id),
        "readiness post-restart"
    );
    assert_eq!(readiness.active_generation, Some(generation.clone()));
    let inspected = ok_or_fail!(control.inspect_bundle(&generation), "inspect post-restart");
    assert_eq!(
        inspected.manifest.manifest_generation,
        generation.manifest_generation
    );
}

#[test]
fn u_sp4_concurrent_activation_does_not_retroactively_switch_pinned_query() {
    // U-SP4: pin captured at time T sees the generation active at T, even if
    // a later writer activates a different gen for the same (repo, rev).
    let dir = ok_or_fail!(tempdir(), "tempdir");
    let db = dir.path().join("c.sqlite3");
    let mut control = ok_or_fail!(ControlPlane::open(&db), "open");

    let gen_a = sample_generation(1);
    ok_or_fail!(control.mark_active_generation(&gen_a, 100), "mark A active");
    let pin_a = ok_or_fail!(
        control.pin_generation(&gen_a.repo_id, &gen_a.revision_id),
        "pin A"
    );

    // Activate generation B.
    let gen_b = sample_generation(2);
    ok_or_fail!(control.mark_active_generation(&gen_b, 200), "mark B active");

    // Pre-existing pin still observes A.
    let snap_a = ok_or_fail!(pin_a.pinned_generation(), "snap A");
    assert_eq!(
        snap_a.as_ref().map(|g| g.manifest_generation),
        Some(gen_a.manifest_generation)
    );

    // Fresh pin observes B.
    let pin_b = ok_or_fail!(
        control.pin_generation(&gen_b.repo_id, &gen_b.revision_id),
        "pin B"
    );
    let snap_b = ok_or_fail!(pin_b.pinned_generation(), "snap B");
    assert_eq!(
        snap_b.as_ref().map(|g| g.manifest_generation),
        Some(gen_b.manifest_generation)
    );
}

// ---------------------------------------------------------------------------
// Edge scenarios
// ---------------------------------------------------------------------------

#[test]
fn e_sp1_duplicate_prepare_is_idempotent() {
    // E-SP1: prepare twice with identical outbox → first accepted, second
    // accepted=false with explicit reason. No duplicate side-effects.
    let dir = ok_or_fail!(tempdir(), "tempdir");
    let mut control = ok_or_fail!(ControlPlane::open(&dir.path().join("c.sqlite3")), "open");
    let outbox = sample_outbox();
    let first = ok_or_fail!(
        control.prepare_bundle(PublishedSearchBundlePrepareRequest {
            outbox: outbox.clone()
        }),
        "first prepare"
    );
    assert!(first.accepted);
    let second = ok_or_fail!(
        control.prepare_bundle(PublishedSearchBundlePrepareRequest { outbox }),
        "second prepare"
    );
    assert!(!second.accepted);
    assert!(second.reason.is_some());
}

#[test]
fn e_sp2_activation_without_readiness_is_rejected_by_policy() {
    // E-SP2: ActivationPolicy requires both lexical_ready and semantic_ready.
    let dir = ok_or_fail!(tempdir(), "tempdir");
    let mut control = ok_or_fail!(ControlPlane::open(&dir.path().join("c.sqlite3")), "open");
    let generation = sample_generation(1);
    let result = control.activate_generation(PublishedSearchGenerationActivateRequest {
        generation,
        lexical_ready: true,
        semantic_ready: false,
        active_at_ms: 1,
    });
    assert!(
        matches!(result, Err(CoreError::NotReady(_))),
        "expected NotReady, got {result:?}"
    );
}

#[test]
fn e_sp3_invalid_manifest_ref_fails_closed_before_build() {
    // E-SP3: zero byte_length artifact ref → reject before any build runs.
    let dir = ok_or_fail!(tempdir(), "tempdir");
    let state_root = dir.path().to_path_buf();
    let bundle_root = state_root.join("bundles");
    let (mut manifest, generation) = sample_manifest_pair(&bundle_root, b"[]");
    manifest.lexical_chunk_rows.byte_length = 0;

    let mut control = ok_or_fail!(ControlPlane::open(&state_root.join("c.sqlite3")), "open");
    let mut lexical = TantivyLexicalAdapter::with_state_root(state_root.clone());
    let mut semantic = LanceSemanticAdapter::with_state_root(state_root.clone());
    let mut uc = MaterializeUseCase {
        control: &mut control,
        lexical: &mut lexical,
        semantic: &mut semantic,
        bundle_root,
        now_ms: 1,
    };
    let result = uc.materialize(manifest, generation.clone());
    assert!(
        matches!(result, Err(CoreError::InvalidContract(_))),
        "expected InvalidContract, got {result:?}"
    );
    // Activation pointer untouched.
    let readiness = ok_or_fail!(
        control.read_readiness(&generation.repo_id, &generation.revision_id),
        "readiness"
    );
    assert!(readiness.active_generation.is_none());
}

// ---------------------------------------------------------------------------
// Corner scenarios
// ---------------------------------------------------------------------------

#[test]
fn c_sp1_no_active_generation_is_explicit_not_ready_not_empty() {
    // C-SP1: query path returns typed NotReady (NOT silent empty results)
    // when no active generation is available.
    let dir = ok_or_fail!(tempdir(), "tempdir");
    let state_root = dir.path().to_path_buf();
    let control = ok_or_fail!(ControlPlane::open(&state_root.join("c.sqlite3")), "open");
    let lexical = TantivyLexicalAdapter::with_state_root(state_root.clone());
    let semantic = LanceSemanticAdapter::with_state_root(state_root.clone());
    let engine = DomainQueryEngine {
        lexical: &lexical,
        semantic: &semantic,
        control: &control,
        embedder: None,
        default_top_k: 5,
    };
    let result = engine.lexical_query(SearchPlaneLexicalQueryRequest {
        query: LqQuery {
            expr: LqExpr::Raw("anything".into()),
            filters: LqFilterSet {
                filters: vec![LqFilter::Repo("repo".into()), LqFilter::Rev("rev".into())],
            },
            options: LqOptionSet {
                limit: None,
                count_all: false,
                timeout_ms: None,
            },
            directives: LqDirectiveSet { directives: vec![] },
        },
        generation: None,
    });
    assert!(
        matches!(result, Err(CoreError::NotReady(_))),
        "expected NotReady, got {result:?}"
    );
}

#[test]
fn c_sp2_lexical_only_generation_still_serves_lexical_queries() {
    // C-SP2: partial readiness — manifest with no embedding_records means
    // semantic is "no-vectors" but lexical queries still work.
    let dir = ok_or_fail!(tempdir(), "tempdir");
    let state_root = dir.path().to_path_buf();
    let bundle_root = state_root.join("bundles");
    let chunk_rows =
        br#"[{"repo_relative_path":"x.rs","start_line":1,"end_line":1,"text":"only"}]"#;
    let (manifest, generation) = sample_manifest_pair(&bundle_root, chunk_rows);

    let mut control = ok_or_fail!(ControlPlane::open(&state_root.join("c.sqlite3")), "open");
    let mut lexical = TantivyLexicalAdapter::with_state_root(state_root.clone());
    let mut semantic = LanceSemanticAdapter::with_state_root(state_root.clone());
    let mut uc = MaterializeUseCase {
        control: &mut control,
        lexical: &mut lexical,
        semantic: &mut semantic,
        bundle_root,
        now_ms: 1,
    };
    let _outcome: quanta_index_searchd::app::MaterializeOutcome =
        ok_or_fail!(uc.materialize(manifest, generation.clone()), "materialize");

    let engine = DomainQueryEngine {
        lexical: &lexical,
        semantic: &semantic,
        control: &control,
        embedder: None,
        default_top_k: 5,
    };
    let response = ok_or_fail!(
        engine.lexical_query(SearchPlaneLexicalQueryRequest {
            query: LqQuery {
                expr: LqExpr::Raw("only".into()),
                filters: LqFilterSet { filters: vec![] },
                options: LqOptionSet {
                    limit: None,
                    count_all: false,
                    timeout_ms: None,
                },
                directives: LqDirectiveSet { directives: vec![] },
            },
            generation: Some(generation.clone()),
        }),
        "lexical query"
    );
    assert_eq!(response.results.len(), 1);
}

// ---------------------------------------------------------------------------
// Hellgate scenarios
// ---------------------------------------------------------------------------

#[test]
fn h_sp1_build_failure_blocks_activation() {
    // H-SP1: digest mismatch on the manifest's chunk_rows → materialize
    // fails fast; no activation pointer is set; readiness reports no active.
    let dir = ok_or_fail!(tempdir(), "tempdir");
    let state_root = dir.path().to_path_buf();
    let bundle_root = state_root.join("bundles");
    let chunk_rows = br#"[{"repo_relative_path":"x.rs","start_line":1,"end_line":1,"text":"x"}]"#;
    let (mut manifest, generation) = sample_manifest_pair(&bundle_root, chunk_rows);
    // Tamper the digest so the materialize-side verify rejects it.
    manifest.lexical_chunk_rows.content_digest = ManifestDigest::new("0".repeat(64));

    let mut control = ok_or_fail!(ControlPlane::open(&state_root.join("c.sqlite3")), "open");
    let mut lexical = TantivyLexicalAdapter::with_state_root(state_root.clone());
    let mut semantic = LanceSemanticAdapter::with_state_root(state_root.clone());
    let mut uc = MaterializeUseCase {
        control: &mut control,
        lexical: &mut lexical,
        semantic: &mut semantic,
        bundle_root,
        now_ms: 1,
    };
    let result = uc.materialize(manifest, generation.clone());
    assert!(
        matches!(result, Err(CoreError::InvalidContract(_))),
        "expected InvalidContract, got {result:?}"
    );
    let readiness = ok_or_fail!(
        control.read_readiness(&generation.repo_id, &generation.revision_id),
        "readiness"
    );
    assert!(readiness.active_generation.is_none());
}

#[test]
fn h_sp2_query_on_unreadied_generation_returns_typed_error() {
    // H-SP2: query path against a not-yet-materialised generation surfaces
    // NotReady from open_lexical_store rather than empty results.
    let dir = ok_or_fail!(tempdir(), "tempdir");
    let state_root = dir.path().to_path_buf();
    let mut control = ok_or_fail!(ControlPlane::open(&state_root.join("c.sqlite3")), "open");
    let lexical = TantivyLexicalAdapter::with_state_root(state_root.clone());
    let semantic = LanceSemanticAdapter::with_state_root(state_root.clone());
    let generation = sample_generation(99);
    // Mark active but don't actually build — simulates a stale state row
    // pointing at a gen that has no lexical index materialised.
    ok_or_fail!(
        control.mark_active_generation(&generation, 1),
        "mark active without build"
    );
    let engine = DomainQueryEngine {
        lexical: &lexical,
        semantic: &semantic,
        control: &control,
        embedder: None,
        default_top_k: 5,
    };
    let result = engine.lexical_query(SearchPlaneLexicalQueryRequest {
        query: LqQuery {
            expr: LqExpr::Raw("anything".into()),
            filters: LqFilterSet { filters: vec![] },
            options: LqOptionSet {
                limit: None,
                count_all: false,
                timeout_ms: None,
            },
            directives: LqDirectiveSet { directives: vec![] },
        },
        generation: Some(generation),
    });
    assert!(
        matches!(result, Err(CoreError::NotReady(_))),
        "expected NotReady (lexical index not materialised), got {result:?}"
    );
}

// H-SP3 is exercised by the unit tests in `app::uds_listener::tests` —
// `malformed_frame_yields_error_envelope_and_keeps_listener_alive` proves
// that a corrupted CBOR body becomes a typed `SearchPlaneIpcResponse::Error`
// without killing the listener. The integration-flavoured proof below wires
// the real listener and confirms an Error envelope round-trips back.

#[expect(
    clippy::disallowed_methods,
    reason = "tokio::test expands to Runtime::block_on"
)]
#[expect(
    clippy::similar_names,
    reason = "scratch variables length/length_usize and header/header2 are intentional"
)]
#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "test asserts against expected SearchPlaneIpcResponse variant only"
)]
#[expect(
    clippy::items_after_statements,
    reason = "test-local mock dispatcher struct is defined inline at the use site for readability"
)]
#[tokio::test]
async fn h_sp3_uds_framing_error_returns_error_envelope_and_keeps_listener_alive() {
    use std::sync::Arc;
    use tokio::net::UnixStream;

    let dir = ok_or_fail!(tempdir(), "tempdir");
    let socket_path = dir.path().join("searchd.sock");
    let listener = match UdsListener::bind(&socket_path).await {
        Ok(l) => l,
        Err(error) => {
            assert!(false, "bind: {error}");
            return;
        }
    };
    let shutdown = listener.shutdown_trigger();

    struct AlwaysOkDispatcher;
    impl quanta_index_core::SearchPlaneLexicalQueryPort for AlwaysOkDispatcher {
        fn lexical_query(
            &self,
            request: SearchPlaneLexicalQueryRequest,
        ) -> Result<quanta_index_contract::SearchPlaneLexicalQueryResponse, CoreError> {
            Ok(quanta_index_contract::SearchPlaneLexicalQueryResponse {
                generation: request.generation.unwrap_or_else(|| sample_generation(1)),
                results: vec![],
            })
        }
    }
    impl quanta_index_core::SearchPlaneSemanticQueryPort for AlwaysOkDispatcher {
        fn semantic_query(
            &self,
            _request: quanta_index_contract::SearchPlaneSemanticQueryRequest,
        ) -> Result<quanta_index_contract::SearchPlaneSemanticQueryResponse, CoreError> {
            Err(CoreError::NotImplemented("stub".into()))
        }
    }
    impl quanta_index_core::SearchPlaneHybridQueryPort for AlwaysOkDispatcher {
        fn hybrid_query(
            &self,
            _request: quanta_index_contract::SearchPlaneHybridQueryRequest,
        ) -> Result<quanta_index_contract::SearchPlaneHybridQueryResponse, CoreError> {
            Err(CoreError::NotImplemented("stub".into()))
        }
    }
    impl quanta_index_core::SearchPlaneExplainQueryPort for AlwaysOkDispatcher {
        fn explain_query(
            &self,
            _request: quanta_index_contract::SearchPlaneExplainQueryRequest,
        ) -> Result<quanta_index_contract::SearchPlaneExplainQueryResponse, CoreError> {
            Err(CoreError::NotReady("stub".into()))
        }
    }

    let serve = tokio::spawn(listener.serve(Arc::new(AlwaysOkDispatcher)));
    let mut client = match UnixStream::connect(&socket_path).await {
        Ok(s) => s,
        Err(error) => {
            assert!(false, "connect: {error}");
            return;
        }
    };

    // Send a frame with a non-zero length but pure garbage body.
    let garbage = [0xffu8, 0xff, 0xff, 0xff];
    let length = u32::try_from(garbage.len()).unwrap_or(4);
    let mut frame = Vec::with_capacity(8);
    frame.extend_from_slice(&length.to_le_bytes());
    frame.extend_from_slice(&garbage);
    if let Err(error) = client.write_all(&frame).await {
        assert!(false, "write garbage: {error}");
        return;
    }

    // Read response header + body.
    let mut header = [0u8; 4];
    if let Err(error) = client.read_exact(&mut header).await {
        assert!(false, "read header: {error}");
        return;
    }
    let length = u32::from_le_bytes(header);
    let length_usize = match usize::try_from(length) {
        Ok(v) => v,
        Err(error) => {
            assert!(false, "len overflow: {error}");
            return;
        }
    };
    let mut body = vec![0u8; length_usize];
    if let Err(error) = client.read_exact(&mut body).await {
        assert!(false, "read body: {error}");
        return;
    }
    let mut full = Vec::with_capacity(4_usize.saturating_add(length_usize));
    full.extend_from_slice(&header);
    full.extend_from_slice(&body);
    let mut cursor = std::io::Cursor::new(full);
    let response = match decode_response(&mut cursor) {
        Ok(r) => r,
        Err(error) => {
            assert!(false, "decode: {error:?}");
            return;
        }
    };
    match response.payload {
        SearchPlaneIpcResponse::Error(error) => {
            assert_eq!(error.code, "ipc.decode");
        }
        other => {
            assert!(false, "expected Error envelope, got {other:?}");
        }
    }

    // Now send a valid Lexical request to prove the listener is still alive.
    let valid = SearchPlaneIpcRequestEnvelope {
        request_id: 99,
        payload: SearchPlaneIpcRequest::Lexical(SearchPlaneLexicalQueryRequest {
            query: LqQuery {
                expr: LqExpr::Raw("ok".into()),
                filters: LqFilterSet { filters: vec![] },
                options: LqOptionSet {
                    limit: None,
                    count_all: false,
                    timeout_ms: None,
                },
                directives: LqDirectiveSet { directives: vec![] },
            },
            generation: Some(sample_generation(1)),
        }),
    };
    let bytes = match encode_request(&valid) {
        Ok(b) => b,
        Err(error) => {
            assert!(false, "encode: {error:?}");
            return;
        }
    };
    if let Err(error) = client.write_all(&bytes).await {
        assert!(false, "write valid: {error}");
        return;
    }
    let mut header2 = [0u8; 4];
    if let Err(error) = client.read_exact(&mut header2).await {
        assert!(false, "read header 2: {error}");
        return;
    }
    let length2 = u32::from_le_bytes(header2);
    let length2_usize = match usize::try_from(length2) {
        Ok(v) => v,
        Err(error) => {
            assert!(false, "len2 overflow: {error}");
            return;
        }
    };
    let mut body2 = vec![0u8; length2_usize];
    if let Err(error) = client.read_exact(&mut body2).await {
        assert!(false, "read body 2: {error}");
        return;
    }
    let mut full2 = Vec::with_capacity(4_usize.saturating_add(length2_usize));
    full2.extend_from_slice(&header2);
    full2.extend_from_slice(&body2);
    let mut cursor2 = std::io::Cursor::new(full2);
    let response2 = match decode_response(&mut cursor2) {
        Ok(r) => r,
        Err(error) => {
            assert!(false, "decode 2: {error:?}");
            return;
        }
    };
    assert_eq!(response2.request_id, 99);
    assert!(matches!(
        response2.payload,
        SearchPlaneIpcResponse::Lexical(_)
    ));

    shutdown.notify_waiters();
    drop(client);
    let _join: Result<Result<(), anyhow::Error>, tokio::task::JoinError> = serve.await;
}

// ---------------------------------------------------------------------------
// Sample fixtures
// ---------------------------------------------------------------------------

fn sample_outbox() -> PreparedBundleOutbox {
    PreparedBundleOutbox {
        outbox_id: "outbox-scenario".into(),
        repo_id: RepoId::new("repo"),
        revision_id: RevisionId::new("rev"),
        manifest_digest: ManifestDigest::new("digest"),
        bundle_schema_version: 1,
        prepared_at_ms: 1,
        mode: quanta_index_contract::BundleMode::ServeOnly,
        manifest_ref: BundleArtifactRef {
            relative_path: "bundle/manifest.json".into(),
            encoding: BundleEncoding::Json,
            byte_length: 128,
            content_digest: ManifestDigest::new("digest"),
        },
        base_generation: None,
        changed_artifact_mask: 1,
    }
}
