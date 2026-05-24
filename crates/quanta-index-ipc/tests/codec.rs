//! Wire codec tests for the search-plane IPC frame format.
//!
//! Frame contract under test: `[u32 little-endian body length][CBOR body]`.
//! Length field counts body bytes only; bodies are capped at
//! [`MAX_FRAME_BODY_BYTES`].

#![deny(unused_must_use)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::map_err_ignore)]

use std::io::{self, Cursor, Read};

use proptest::prelude::*;
use quanta_index_contract::{
    GenerationId, LexicalCandidate, LqDirective, LqDirectiveSet, LqExpr, LqFilter, LqFilterSet,
    LqOptionSet, LqQuery, ManifestGeneration, PublishedGenerationSet, RepoId, RepoRelativePath,
    RevisionId, SearchExplanation, SearchPlaneExplainQueryRequest, SearchPlaneExplainQueryResponse,
    SearchPlaneHybridQueryRequest, SearchPlaneHybridQueryResponse, SearchPlaneIpcError,
    SearchPlaneIpcRequest, SearchPlaneIpcRequestEnvelope, SearchPlaneIpcResponse,
    SearchPlaneIpcResponseEnvelope, SearchPlaneLexicalQueryRequest,
    SearchPlaneLexicalQueryResponse, SearchPlaneSemanticQueryRequest,
    SearchPlaneSemanticQueryResponse,
};
use quanta_index_ipc::{
    IpcError, MAX_FRAME_BODY_BYTES, decode_request, decode_response, encode_request,
    encode_response,
};

/// Reduce assertion boilerplate while honouring the workspace panic ban.
macro_rules! ok_or_fail {
    ($expr:expr, $msg:expr) => {
        match $expr {
            Ok(v) => v,
            Err(error) => {
                assert!(false, "{}: {error:?}", $msg);
                return;
            }
        }
    };
}

fn sample_repo_id() -> RepoId {
    RepoId::new("repo-alpha")
}

fn sample_revision_id() -> RevisionId {
    RevisionId::new("rev-001")
}

fn sample_generation() -> PublishedGenerationSet {
    PublishedGenerationSet {
        repo_id: sample_repo_id(),
        revision_id: sample_revision_id(),
        manifest_generation: ManifestGeneration::new(7),
        lexical_generation: GenerationId::new(10),
        symbol_generation: GenerationId::new(11),
        structural_generation: Some(GenerationId::new(12)),
        history_generation: None,
        semantic_generation: Some(GenerationId::new(13)),
        metadata_generation: None,
    }
}

fn sample_lq_query() -> LqQuery {
    LqQuery {
        expr: LqExpr::All(vec![
            LqExpr::MatchAll,
            LqExpr::Raw("foo".to_owned()),
            LqExpr::Any(vec![
                LqExpr::Raw("bar".to_owned()),
                LqExpr::Not(Box::new(LqExpr::Raw("baz".to_owned()))),
            ]),
        ]),
        filters: LqFilterSet {
            filters: vec![
                LqFilter::Repo("repo-alpha".to_owned()),
                LqFilter::Custom {
                    key: "k".to_owned(),
                    value: "v".to_owned(),
                },
            ],
        },
        options: LqOptionSet {
            limit: Some(100),
            count_all: true,
            timeout_ms: Some(5_000),
        },
        directives: LqDirectiveSet {
            directives: vec![
                LqDirective::IntoCodeQl,
                LqDirective::ScopeResults,
                LqDirective::WithLexical,
                LqDirective::Custom("trace".to_owned()),
            ],
        },
    }
}

fn sample_candidate() -> LexicalCandidate {
    LexicalCandidate {
        candidate_id: "cand-1".to_owned(),
        repo_id: sample_repo_id(),
        revision_id: sample_revision_id(),
        manifest_generation: ManifestGeneration::new(7),
        repo_relative_path: RepoRelativePath::new("src/lib.rs"),
        start_line: 10,
        end_line: 20,
        score: 0.875,
        snippet: "fn main() {}".to_owned(),
    }
}

fn lexical_request() -> SearchPlaneIpcRequestEnvelope {
    SearchPlaneIpcRequestEnvelope {
        request_id: 1,
        payload: SearchPlaneIpcRequest::Lexical(SearchPlaneLexicalQueryRequest {
            query: sample_lq_query(),
            generation: Some(sample_generation()),
        }),
    }
}

fn semantic_request() -> SearchPlaneIpcRequestEnvelope {
    SearchPlaneIpcRequestEnvelope {
        request_id: 2,
        payload: SearchPlaneIpcRequest::Semantic(SearchPlaneSemanticQueryRequest {
            query_text: "needle".to_owned(),
            generation: None,
            lexical_filters: LqFilterSet::default(),
            top_k: 5,
        }),
    }
}

fn hybrid_request() -> SearchPlaneIpcRequestEnvelope {
    SearchPlaneIpcRequestEnvelope {
        request_id: 3,
        payload: SearchPlaneIpcRequest::Hybrid(SearchPlaneHybridQueryRequest {
            lexical_query: sample_lq_query(),
            semantic_query_text: "needle".to_owned(),
            generation: Some(sample_generation()),
            top_k: 20,
        }),
    }
}

fn explain_request() -> SearchPlaneIpcRequestEnvelope {
    SearchPlaneIpcRequestEnvelope {
        request_id: 4,
        payload: SearchPlaneIpcRequest::Explain(SearchPlaneExplainQueryRequest {
            generation: sample_generation(),
            candidate: sample_candidate(),
        }),
    }
}

fn lexical_response() -> SearchPlaneIpcResponseEnvelope {
    SearchPlaneIpcResponseEnvelope {
        request_id: 11,
        payload: SearchPlaneIpcResponse::Lexical(SearchPlaneLexicalQueryResponse {
            generation: sample_generation(),
            results: vec![sample_candidate()],
        }),
    }
}

fn semantic_response() -> SearchPlaneIpcResponseEnvelope {
    SearchPlaneIpcResponseEnvelope {
        request_id: 12,
        payload: SearchPlaneIpcResponse::Semantic(SearchPlaneSemanticQueryResponse {
            generation: sample_generation(),
            results: vec![sample_candidate()],
        }),
    }
}

fn hybrid_response() -> SearchPlaneIpcResponseEnvelope {
    SearchPlaneIpcResponseEnvelope {
        request_id: 13,
        payload: SearchPlaneIpcResponse::Hybrid(SearchPlaneHybridQueryResponse {
            generation: sample_generation(),
            results: vec![sample_candidate()],
        }),
    }
}

fn explain_response() -> SearchPlaneIpcResponseEnvelope {
    SearchPlaneIpcResponseEnvelope {
        request_id: 14,
        payload: SearchPlaneIpcResponse::Explain(SearchPlaneExplainQueryResponse {
            generation: sample_generation(),
            explanation: SearchExplanation {
                summary: "why".to_owned(),
            },
        }),
    }
}

fn error_response() -> SearchPlaneIpcResponseEnvelope {
    SearchPlaneIpcResponseEnvelope {
        request_id: 15,
        payload: SearchPlaneIpcResponse::Error(SearchPlaneIpcError {
            code: "PLANE_BUSY".to_owned(),
            message: "search-plane is currently rebuilding".to_owned(),
        }),
    }
}

fn roundtrip_request(value: &SearchPlaneIpcRequestEnvelope) -> Result<(), IpcError> {
    let frame = encode_request(value)?;
    let mut cursor = Cursor::new(frame);
    let decoded = decode_request(&mut cursor)?;
    if &decoded != value {
        return Err(IpcError::Decode(format!(
            "request roundtrip mismatch: encoded {value:?} got {decoded:?}"
        )));
    }
    Ok(())
}

fn roundtrip_response(value: &SearchPlaneIpcResponseEnvelope) -> Result<(), IpcError> {
    let frame = encode_response(value)?;
    let mut cursor = Cursor::new(frame);
    let decoded = decode_response(&mut cursor)?;
    if &decoded != value {
        return Err(IpcError::Decode(format!(
            "response roundtrip mismatch: encoded {value:?} got {decoded:?}"
        )));
    }
    Ok(())
}

#[test]
fn lexical_request_roundtrip() {
    ok_or_fail!(roundtrip_request(&lexical_request()), "lexical request");
}

#[test]
fn semantic_request_roundtrip() {
    ok_or_fail!(roundtrip_request(&semantic_request()), "semantic request");
}

#[test]
fn hybrid_request_roundtrip() {
    ok_or_fail!(roundtrip_request(&hybrid_request()), "hybrid request");
}

#[test]
fn explain_request_roundtrip() {
    ok_or_fail!(roundtrip_request(&explain_request()), "explain request");
}

#[test]
fn lexical_response_roundtrip() {
    ok_or_fail!(roundtrip_response(&lexical_response()), "lexical response");
}

#[test]
fn semantic_response_roundtrip() {
    ok_or_fail!(
        roundtrip_response(&semantic_response()),
        "semantic response"
    );
}

#[test]
fn hybrid_response_roundtrip() {
    ok_or_fail!(roundtrip_response(&hybrid_response()), "hybrid response");
}

#[test]
fn explain_response_roundtrip() {
    ok_or_fail!(roundtrip_response(&explain_response()), "explain response");
}

#[test]
fn error_response_roundtrip() {
    ok_or_fail!(roundtrip_response(&error_response()), "error response");
}

#[test]
fn frame_layout_uses_little_endian_length_prefix() {
    let envelope = lexical_request();
    let frame = ok_or_fail!(encode_request(&envelope), "encode");
    assert!(
        frame.len() > 4,
        "frame must include header + body, got {}",
        frame.len()
    );
    let Some(header_slice) = frame.get(..4) else {
        assert!(false, "frame shorter than 4 bytes");
        return;
    };
    let Some(body_slice) = frame.get(4..) else {
        assert!(false, "frame missing body slice");
        return;
    };
    let mut header = [0u8; 4];
    header.copy_from_slice(header_slice);
    let declared = u32::from_le_bytes(header);
    let body_len = match u32::try_from(body_slice.len()) {
        Ok(v) => v,
        Err(error) => {
            assert!(false, "body length did not fit in u32: {error}");
            return;
        }
    };
    assert_eq!(
        declared, body_len,
        "length prefix must match body byte count"
    );
}

#[test]
fn truncated_header_returns_truncated() {
    // Only 3 bytes — header demands 4.
    let mut cursor = Cursor::new(vec![0u8, 0u8, 0u8]);
    match decode_request(&mut cursor) {
        Err(IpcError::Truncated) => {}
        other => assert!(false, "expected Truncated, got {other:?}"),
    }
}

#[test]
fn empty_reader_returns_truncated() {
    let mut cursor = Cursor::new(Vec::<u8>::new());
    match decode_request(&mut cursor) {
        Err(IpcError::Truncated) => {}
        other => assert!(false, "expected Truncated, got {other:?}"),
    }
}

#[test]
fn oversize_header_returns_oversized_without_reading_body() {
    // Construct a frame that declares MAX_FRAME_BODY_BYTES + 1 bytes,
    // but supplies no body bytes at all. Decode must fail before consuming
    // any body — i.e. cursor position must remain at exactly 4.
    let oversized_len = match u32::try_from(MAX_FRAME_BODY_BYTES) {
        Ok(v) => v,
        Err(error) => {
            assert!(false, "MAX_FRAME_BODY_BYTES did not fit in u32: {error}");
            return;
        }
    };
    let Some(oversized_len) = oversized_len.checked_add(1) else {
        assert!(false, "cap + 1 overflowed u32");
        return;
    };
    let mut frame = Vec::with_capacity(4);
    frame.extend_from_slice(&oversized_len.to_le_bytes());
    let mut cursor = Cursor::new(frame);
    match decode_request(&mut cursor) {
        Err(IpcError::Oversized(reported)) => {
            assert_eq!(reported, u64::from(oversized_len));
            assert_eq!(
                cursor.position(),
                4,
                "decoder must not consume body bytes on oversize"
            );
        }
        other => assert!(false, "expected Oversized, got {other:?}"),
    }
}

#[test]
fn zero_length_header_returns_empty_frame() {
    let mut cursor = Cursor::new(vec![0u8, 0u8, 0u8, 0u8]);
    match decode_response(&mut cursor) {
        Err(IpcError::EmptyFrame) => {}
        other => assert!(false, "expected EmptyFrame, got {other:?}"),
    }
}

#[test]
fn declared_length_exceeds_actual_body_returns_truncated() {
    // Declare 10 bytes of body, supply only 3.
    let mut frame = Vec::with_capacity(4 + 3);
    frame.extend_from_slice(&10u32.to_le_bytes());
    frame.extend_from_slice(&[1u8, 2u8, 3u8]);
    let mut cursor = Cursor::new(frame);
    match decode_request(&mut cursor) {
        Err(IpcError::Truncated) => {}
        other => assert!(false, "expected Truncated, got {other:?}"),
    }
}

#[test]
fn garbage_body_returns_decode_error() {
    // 16-byte payload of non-CBOR garbage that exceeds what CBOR would
    // recognise as a valid envelope map.
    let body: [u8; 16] = [
        0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
        0xFF,
    ];
    let len = match u32::try_from(body.len()) {
        Ok(v) => v,
        Err(error) => {
            assert!(false, "len didn't fit u32: {error}");
            return;
        }
    };
    let mut frame = Vec::with_capacity(4 + body.len());
    frame.extend_from_slice(&len.to_le_bytes());
    frame.extend_from_slice(&body);
    let mut cursor = Cursor::new(frame);
    match decode_request(&mut cursor) {
        Err(IpcError::Decode(_)) => {}
        other => assert!(false, "expected Decode, got {other:?}"),
    }
}

#[test]
fn oversize_encode_returns_oversized() {
    // 17 MiB message string blows past the 16 MiB cap by itself, so the
    // serialized envelope is guaranteed to exceed MAX_FRAME_BODY_BYTES.
    let Some(bloat_len) = MAX_FRAME_BODY_BYTES.checked_add(1024 * 1024) else {
        assert!(false, "bloat length overflowed usize");
        return;
    };
    let envelope = SearchPlaneIpcResponseEnvelope {
        request_id: 99,
        payload: SearchPlaneIpcResponse::Error(SearchPlaneIpcError {
            code: "TOO_BIG".to_owned(),
            message: "A".repeat(bloat_len),
        }),
    };
    let cap = match u64::try_from(MAX_FRAME_BODY_BYTES) {
        Ok(v) => v,
        Err(error) => {
            assert!(false, "MAX_FRAME_BODY_BYTES did not fit in u64: {error}");
            return;
        }
    };
    match encode_response(&envelope) {
        Err(IpcError::Oversized(reported)) => {
            assert!(
                reported > cap,
                "reported length {reported} should exceed cap {cap}"
            );
        }
        other => assert!(false, "expected Oversized, got {other:?}"),
    }
}

/// Reader that returns one byte at a time, exercising the short-read loop in
/// `read_exact_or_truncated` and proving that partial reads do not corrupt
/// the frame state machine.
struct ChunkOneReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Read for ChunkOneReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pos >= self.data.len() || buf.is_empty() {
            return Ok(0);
        }
        let Some(out) = buf.get_mut(0) else {
            return Ok(0);
        };
        let Some(byte) = self.data.get(self.pos) else {
            return Ok(0);
        };
        *out = *byte;
        self.pos = match self.pos.checked_add(1) {
            Some(v) => v,
            None => return Err(io::Error::other("position overflow")),
        };
        Ok(1)
    }
}

#[test]
fn decode_tolerates_one_byte_at_a_time_reader() {
    let envelope = hybrid_request();
    let frame = ok_or_fail!(encode_request(&envelope), "encode hybrid");
    let mut reader = ChunkOneReader {
        data: frame.as_slice(),
        pos: 0,
    };
    let decoded = ok_or_fail!(decode_request(&mut reader), "decode hybrid chunked");
    assert_eq!(decoded, envelope);
}

/// Reader that fails after returning the first 4 bytes (the header), so the
/// body read should surface as [`IpcError::Io`], not [`IpcError::Truncated`].
struct HeaderThenIoErrorReader {
    header: [u8; 4],
    pos: usize,
}

impl Read for HeaderThenIoErrorReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pos < 4 {
            let Some(byte) = self.header.get(self.pos) else {
                return Err(io::Error::other("header index out of range"));
            };
            let Some(slot) = buf.get_mut(0) else {
                return Ok(0);
            };
            *slot = *byte;
            self.pos = match self.pos.checked_add(1) {
                Some(v) => v,
                None => return Err(io::Error::other("position overflow")),
            };
            return Ok(1);
        }
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "transport closed",
        ))
    }
}

#[test]
fn body_read_io_error_surfaces_as_io_variant() {
    let mut reader = HeaderThenIoErrorReader {
        header: 8u32.to_le_bytes(),
        pos: 0,
    };
    match decode_request(&mut reader) {
        Err(IpcError::Io(err)) => {
            assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        }
        other => assert!(false, "expected Io, got {other:?}"),
    }
}

proptest! {
    /// Random request_id values must roundtrip through the codec.
    #[test]
    fn proptest_lexical_request_id_roundtrips(request_id: u64) {
        let envelope = SearchPlaneIpcRequestEnvelope {
            request_id,
            payload: SearchPlaneIpcRequest::Lexical(SearchPlaneLexicalQueryRequest {
                query: LqQuery {
                    expr: LqExpr::MatchAll,
                    filters: LqFilterSet::default(),
                    options: LqOptionSet::default(),
                    directives: LqDirectiveSet::default(),
                },
                generation: None,
            }),
        };
        let frame = encode_request(&envelope)
            .map_err(|err| TestCaseError::fail(format!("encode failed: {err}")))?;
        let mut cursor = Cursor::new(frame);
        let decoded = decode_request(&mut cursor)
            .map_err(|err| TestCaseError::fail(format!("decode failed: {err}")))?;
        prop_assert_eq!(decoded, envelope);
    }

    /// Random short byte slices (length < 4) must always be reported as
    /// truncated, never as some other error variant.
    #[test]
    fn proptest_short_header_always_truncated(prefix in proptest::collection::vec(any::<u8>(), 0..4)) {
        let mut cursor = Cursor::new(prefix);
        match decode_request(&mut cursor) {
            Err(IpcError::Truncated) => {}
            other => {
                return Err(TestCaseError::fail(format!(
                    "expected Truncated, got {other:?}"
                )));
            }
        }
    }
}
