#![expect(
    clippy::redundant_pub_crate,
    reason = "crate-private transports are shared through the SDK root and tests only"
)]

use std::path::{Path, PathBuf};

use quanta_index_contract::{
    DiagnosticAttemptFieldV1, DiagnosticAttemptOutcomeV1, DiagnosticAttemptTokenV1,
    SearchPlaneControlIpcRequestEnvelope, SearchPlaneControlIpcResponseEnvelope,
    SearchPlaneIngestIpcRequestEnvelope, SearchPlaneIngestIpcResponseEnvelope,
    SearchPlaneQueryIpcRequestEnvelope, SearchPlaneQueryIpcResponseEnvelope,
};
use quanta_index_ipc::{
    ClientIoPolicy, ClientIpcTimingV1, RequestSizeDecisionV1, request_size_decision_v1,
    send_request, send_request_negotiated, send_request_observed, send_request_observed_negotiated,
};

use crate::SdkError;

/// Owned attempt scope returned by an explicitly injected diagnostic source.
/// Dropping without `finish` represents an interrupted attempt.
pub trait DiagnosticAttemptGuardV1: std::fmt::Debug + Send {
    fn token(&self) -> DiagnosticAttemptTokenV1;
    fn finish(self: Box<Self>, outcome: DiagnosticAttemptOutcomeV1);
}

/// Creates a fresh correlation token at each actual transport send boundary.
/// Implementations own trace IDs and clocks; SDK callers do not synthesize IDs.
pub trait DiagnosticAttemptSourceV1: std::fmt::Debug + Send + Sync {
    fn begin_attempt(&self) -> Option<Box<dyn DiagnosticAttemptGuardV1>>;
}

pub(crate) trait QueryTransport: Send + Sync {
    fn send(
        &self,
        request: SearchPlaneQueryIpcRequestEnvelope,
    ) -> Result<SearchPlaneQueryIpcResponseEnvelope, SdkError>;

    fn send_observed(
        &self,
        _request: SearchPlaneQueryIpcRequestEnvelope,
    ) -> Result<(SearchPlaneQueryIpcResponseEnvelope, ClientIpcTimingV1), SdkError> {
        Err(SdkError::Protocol(
            "query transport does not support request-local client observation".to_string(),
        ))
    }

    fn send_traced(
        &self,
        request: SearchPlaneQueryIpcRequestEnvelope,
        source: Option<&dyn DiagnosticAttemptSourceV1>,
    ) -> Result<SearchPlaneQueryIpcResponseEnvelope, SdkError> {
        send_with_attempt(request, source, false, |request| self.send(request))
    }

    fn send_observed_traced(
        &self,
        request: SearchPlaneQueryIpcRequestEnvelope,
        source: Option<&dyn DiagnosticAttemptSourceV1>,
    ) -> Result<(SearchPlaneQueryIpcResponseEnvelope, ClientIpcTimingV1), SdkError> {
        send_with_attempt(request, source, false, |request| {
            self.send_observed(request)
        })
    }
}

pub(crate) trait ControlTransport: Send + Sync {
    fn send(
        &self,
        request: SearchPlaneControlIpcRequestEnvelope,
    ) -> Result<SearchPlaneControlIpcResponseEnvelope, SdkError>;

    fn send_traced(
        &self,
        request: SearchPlaneControlIpcRequestEnvelope,
        source: Option<&dyn DiagnosticAttemptSourceV1>,
    ) -> Result<SearchPlaneControlIpcResponseEnvelope, SdkError> {
        send_with_attempt(request, source, false, |request| self.send(request))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct UdsQueryTransport {
    inner: UdsTransport,
}

impl UdsQueryTransport {
    #[must_use]
    pub(crate) fn new(socket_path: impl Into<PathBuf>, io_policy: ClientIoPolicy) -> Self {
        Self {
            inner: UdsTransport::new(socket_path, io_policy),
        }
    }
}

impl QueryTransport for UdsQueryTransport {
    fn send(
        &self,
        request: SearchPlaneQueryIpcRequestEnvelope,
    ) -> Result<SearchPlaneQueryIpcResponseEnvelope, SdkError> {
        self.inner.send(&request)
    }

    fn send_observed(
        &self,
        request: SearchPlaneQueryIpcRequestEnvelope,
    ) -> Result<(SearchPlaneQueryIpcResponseEnvelope, ClientIpcTimingV1), SdkError> {
        self.inner.send_observed(&request)
    }

    fn send_traced(
        &self,
        request: SearchPlaneQueryIpcRequestEnvelope,
        source: Option<&dyn DiagnosticAttemptSourceV1>,
    ) -> Result<SearchPlaneQueryIpcResponseEnvelope, SdkError> {
        send_with_uds_attempt(&request, source, |plain, linked| {
            self.inner.send_negotiated(plain, linked)
        })
    }

    fn send_observed_traced(
        &self,
        request: SearchPlaneQueryIpcRequestEnvelope,
        source: Option<&dyn DiagnosticAttemptSourceV1>,
    ) -> Result<(SearchPlaneQueryIpcResponseEnvelope, ClientIpcTimingV1), SdkError> {
        send_with_uds_attempt(&request, source, |plain, linked| {
            self.inner.send_observed_negotiated(plain, linked)
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct UdsControlTransport {
    inner: UdsTransport,
}

impl UdsControlTransport {
    #[must_use]
    pub(crate) fn new(socket_path: impl Into<PathBuf>, io_policy: ClientIoPolicy) -> Self {
        Self {
            inner: UdsTransport::new(socket_path, io_policy),
        }
    }
}

impl ControlTransport for UdsControlTransport {
    fn send(
        &self,
        request: SearchPlaneControlIpcRequestEnvelope,
    ) -> Result<SearchPlaneControlIpcResponseEnvelope, SdkError> {
        self.inner.send(&request)
    }

    fn send_traced(
        &self,
        request: SearchPlaneControlIpcRequestEnvelope,
        source: Option<&dyn DiagnosticAttemptSourceV1>,
    ) -> Result<SearchPlaneControlIpcResponseEnvelope, SdkError> {
        send_with_uds_attempt(&request, source, |plain, linked| {
            self.inner.send_negotiated(plain, linked)
        })
    }
}

/// QI-SDK-01: typed ingest transport.
///
/// The SDK's `publish()` paths route through this trait so the producer never
/// opens legacy channel publishers directly. `quanta-index-channel` is
/// intentionally absent from the SDK's dependency surface.
pub(crate) trait IngestTransport: Send + Sync {
    fn send(
        &self,
        request: SearchPlaneIngestIpcRequestEnvelope,
    ) -> Result<SearchPlaneIngestIpcResponseEnvelope, SdkError>;

    fn send_traced(
        &self,
        request: SearchPlaneIngestIpcRequestEnvelope,
        source: Option<&dyn DiagnosticAttemptSourceV1>,
    ) -> Result<SearchPlaneIngestIpcResponseEnvelope, SdkError> {
        send_with_attempt(request, source, false, |request| self.send(request))
    }
}

trait DiagnosticAttemptEnvelope: serde::Serialize {
    fn diagnostic_attempt(&mut self) -> &mut DiagnosticAttemptFieldV1;

    type Payload: serde::Serialize;
    fn request_id(&self) -> u64;
    fn payload(&self) -> &Self::Payload;

    fn linked_view(&self, token: DiagnosticAttemptTokenV1) -> LinkedEnvelope<'_, Self::Payload> {
        LinkedEnvelope {
            request_id: self.request_id(),
            payload: self.payload(),
            token,
        }
    }
}

struct LinkedEnvelope<'a, Payload> {
    request_id: u64,
    payload: &'a Payload,
    token: DiagnosticAttemptTokenV1,
}

impl<Payload: serde::Serialize> serde::Serialize for LinkedEnvelope<'_, Payload> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut fields = serializer.serialize_struct("SearchPlaneIpcRequestEnvelope", 3)?;
        fields.serialize_field("request_id", &self.request_id)?;
        fields.serialize_field("payload", self.payload)?;
        fields.serialize_field("diagnostic_attempt", &self.token)?;
        fields.end()
    }
}

impl DiagnosticAttemptEnvelope for SearchPlaneQueryIpcRequestEnvelope {
    type Payload = quanta_index_contract::SearchPlaneQueryIpcRequest;

    fn diagnostic_attempt(&mut self) -> &mut DiagnosticAttemptFieldV1 {
        &mut self.diagnostic_attempt
    }

    fn request_id(&self) -> u64 {
        self.request_id
    }
    fn payload(&self) -> &Self::Payload {
        &self.payload
    }
}

impl DiagnosticAttemptEnvelope for SearchPlaneControlIpcRequestEnvelope {
    type Payload = quanta_index_contract::SearchPlaneControlIpcRequest;

    fn diagnostic_attempt(&mut self) -> &mut DiagnosticAttemptFieldV1 {
        &mut self.diagnostic_attempt
    }

    fn request_id(&self) -> u64 {
        self.request_id
    }
    fn payload(&self) -> &Self::Payload {
        &self.payload
    }
}

impl DiagnosticAttemptEnvelope for SearchPlaneIngestIpcRequestEnvelope {
    type Payload = quanta_index_contract::SearchPlaneIngestIpcRequest;

    fn diagnostic_attempt(&mut self) -> &mut DiagnosticAttemptFieldV1 {
        &mut self.diagnostic_attempt
    }

    fn request_id(&self) -> u64 {
        self.request_id
    }
    fn payload(&self) -> &Self::Payload {
        &self.payload
    }
}

fn send_with_attempt<Envelope, Response>(
    mut request: Envelope,
    source: Option<&dyn DiagnosticAttemptSourceV1>,
    wire_link_allowed: bool,
    send: impl FnOnce(Envelope) -> Result<Response, SdkError>,
) -> Result<Response, SdkError>
where
    Envelope: DiagnosticAttemptEnvelope,
{
    let attempt = source.and_then(DiagnosticAttemptSourceV1::begin_attempt);
    let result = (|| {
        // Public SDK socket options do not prove the peer was built with this
        // diagnostic field. A client attempt is still useful locally, but an
        // unverified peer must receive the original request envelope shape.
        if let Some(guard) = attempt.as_ref().filter(|_| wire_link_allowed) {
            *request.diagnostic_attempt() = DiagnosticAttemptFieldV1::Linked(guard.token());
            // Only a typed decoded-body limit permits dropping diagnostic metadata.
            // All serialization, compression, connection and product errors retain
            // their original failure path.
            if matches!(
                request_size_decision_v1(&request).map_err(SdkError::Transport)?,
                RequestSizeDecisionV1::ExceedsDecodedCap { .. }
            ) {
                *request.diagnostic_attempt() = DiagnosticAttemptFieldV1::Absent;
                if matches!(
                    request_size_decision_v1(&request).map_err(SdkError::Transport)?,
                    RequestSizeDecisionV1::ExceedsDecodedCap { .. }
                ) {
                    *request.diagnostic_attempt() = DiagnosticAttemptFieldV1::Linked(guard.token());
                }
            }
        }
        send(request)
    })();
    if let Some(guard) = attempt {
        guard.finish(match &result {
            Ok(_) => DiagnosticAttemptOutcomeV1::Ok,
            Err(SdkError::Transport(
                quanta_index_ipc::IpcError::Timeout { .. }
                | quanta_index_ipc::IpcError::ClientIoDeadlineElapsed,
            )) => DiagnosticAttemptOutcomeV1::Cancelled,
            Err(_) => DiagnosticAttemptOutcomeV1::Error,
        });
    }
    result
}

fn send_with_uds_attempt<Envelope, Response>(
    request: &Envelope,
    source: Option<&dyn DiagnosticAttemptSourceV1>,
    send: impl FnOnce(
        &Envelope,
        Option<(&LinkedEnvelope<'_, Envelope::Payload>, [u8; 16])>,
    ) -> Result<Response, SdkError>,
) -> Result<Response, SdkError>
where
    Envelope: DiagnosticAttemptEnvelope,
{
    let attempt = source.and_then(DiagnosticAttemptSourceV1::begin_attempt);
    let result = (|| {
        let Some(guard) = attempt.as_ref() else {
            return send(request, None);
        };
        let token = guard.token();
        let linked = request.linked_view(token);
        if matches!(
            request_size_decision_v1(&linked).map_err(SdkError::Transport)?,
            RequestSizeDecisionV1::ExceedsDecodedCap { .. }
        ) && matches!(
            request_size_decision_v1(request).map_err(SdkError::Transport)?,
            RequestSizeDecisionV1::Fits { .. }
        ) {
            return send(request, None);
        }
        send(request, Some((&linked, token.capture_id())))
    })();
    if let Some(guard) = attempt {
        guard.finish(match &result {
            Ok(_) => DiagnosticAttemptOutcomeV1::Ok,
            Err(SdkError::Transport(
                quanta_index_ipc::IpcError::Timeout { .. }
                | quanta_index_ipc::IpcError::ClientIoDeadlineElapsed,
            )) => DiagnosticAttemptOutcomeV1::Cancelled,
            Err(_) => DiagnosticAttemptOutcomeV1::Error,
        });
    }
    result
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct UdsIngestTransport {
    inner: UdsTransport,
}

impl UdsIngestTransport {
    #[must_use]
    pub(crate) fn new(socket_path: impl Into<PathBuf>, io_policy: ClientIoPolicy) -> Self {
        Self {
            inner: UdsTransport::new(socket_path, io_policy),
        }
    }
}

impl IngestTransport for UdsIngestTransport {
    fn send(
        &self,
        request: SearchPlaneIngestIpcRequestEnvelope,
    ) -> Result<SearchPlaneIngestIpcResponseEnvelope, SdkError> {
        self.inner.send(&request)
    }

    fn send_traced(
        &self,
        request: SearchPlaneIngestIpcRequestEnvelope,
        source: Option<&dyn DiagnosticAttemptSourceV1>,
    ) -> Result<SearchPlaneIngestIpcResponseEnvelope, SdkError> {
        send_with_uds_attempt(&request, source, |plain, linked| {
            self.inner.send_negotiated(plain, linked)
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct UdsTransport {
    socket_path: PathBuf,
    io_policy: ClientIoPolicy,
}

impl UdsTransport {
    fn new(socket_path: impl Into<PathBuf>, io_policy: ClientIoPolicy) -> Self {
        Self {
            socket_path: socket_path.into(),
            io_policy,
        }
    }

    fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    fn send<Request, Response>(&self, request: &Request) -> Result<Response, SdkError>
    where
        Request: serde::Serialize,
        Response: serde::de::DeserializeOwned,
    {
        send_request(self.socket_path(), request, self.io_policy).map_err(SdkError::Transport)
    }

    fn send_observed<Request, Response>(
        &self,
        request: &Request,
    ) -> Result<(Response, ClientIpcTimingV1), SdkError>
    where
        Request: serde::Serialize,
        Response: serde::de::DeserializeOwned,
    {
        send_request_observed(self.socket_path(), request, self.io_policy)
            .map_err(SdkError::Transport)
    }

    fn send_negotiated<Request, Linked, Response>(
        &self,
        plain: &Request,
        linked: Option<(&Linked, [u8; 16])>,
    ) -> Result<Response, SdkError>
    where
        Request: serde::Serialize,
        Linked: serde::Serialize,
        Response: serde::de::DeserializeOwned,
    {
        match linked {
            Some((linked, capture_id)) => send_request_negotiated(
                self.socket_path(),
                plain,
                linked,
                capture_id,
                self.io_policy,
            ),
            None => send_request(self.socket_path(), plain, self.io_policy),
        }
        .map_err(SdkError::Transport)
    }

    fn send_observed_negotiated<Request, Linked, Response>(
        &self,
        plain: &Request,
        linked: Option<(&Linked, [u8; 16])>,
    ) -> Result<(Response, ClientIpcTimingV1), SdkError>
    where
        Request: serde::Serialize,
        Linked: serde::Serialize,
        Response: serde::de::DeserializeOwned,
    {
        match linked {
            Some((linked, capture_id)) => send_request_observed_negotiated(
                self.socket_path(),
                plain,
                linked,
                capture_id,
                self.io_policy,
            ),
            None => send_request_observed(self.socket_path(), plain, self.io_policy),
        }
        .map_err(SdkError::Transport)
    }
}

#[cfg(test)]
mod diagnostic_attempt_tests {
    use std::io::{Read, Write};
    use std::num::NonZeroU64;
    use std::os::unix::net::UnixListener;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    use quanta_index_contract::{DiagnosticAttemptOutcomeV1, DiagnosticAttemptTokenV1};
    use serde::ser::{SerializeSeq, SerializeStruct};

    use super::{
        DiagnosticAttemptEnvelope, DiagnosticAttemptFieldV1, DiagnosticAttemptGuardV1,
        DiagnosticAttemptSourceV1, RequestSizeDecisionV1, SdkError, UdsTransport,
        request_size_decision_v1, send_with_attempt, send_with_uds_attempt,
    };

    #[test]
    fn uds_attempt_token_requires_same_stream_positive_ack() {
        for ack in [Some(true), Some(false), None] {
            let dir = tempfile::tempdir().expect("socket directory");
            let socket = dir.path().join("attempt.sock");
            let listener = UnixListener::bind(&socket).expect("bind test socket");
            let peer = std::thread::spawn(move || {
                let (mut first, _) = listener.accept().expect("first connection");
                let mut preface = [0_u8; 24];
                first.read_exact(&mut preface).expect("preface");
                assert_eq!(&preface[..8], b"QIPCDG01");
                assert_eq!(&preface[8..], &[3; 16]);
                let mut stream = if let Some(accepted) = ack {
                    let mut response = [0_u8; 9];
                    response[..8].copy_from_slice(b"QIPCDG01");
                    response[8] = u8::from(accepted);
                    first.write_all(&response).expect("ack");
                    first
                } else {
                    drop(first);
                    listener.accept().expect("legacy retry").0
                };
                let request: ciborium::value::Value =
                    quanta_index_ipc::decode_request(&mut stream).expect("one product request");
                stream
                    .write_all(&quanta_index_ipc::encode_response(&1_u64).expect("response"))
                    .expect("response write");
                let ciborium::value::Value::Map(fields) = request else {
                    panic!("request must be a map");
                };
                fields.iter().any(|(key, _)| {
                    *key == ciborium::value::Value::Text("diagnostic_attempt".into())
                })
            });
            let source = Source {
                next: AtomicU64::new(0),
                outcomes: Arc::new(Mutex::new(Vec::new())),
            };
            let transport = UdsTransport::new(socket, quanta_index_ipc::ClientIoPolicy::default());
            let result: u64 = send_with_uds_attempt(
                &Request {
                    diagnostic_attempt: DiagnosticAttemptFieldV1::Absent,
                    virtual_payload: None,
                },
                Some(&source),
                |plain, linked| transport.send_negotiated(plain, linked),
            )
            .expect("request outcome");
            assert_eq!(result, 1);
            assert_eq!(peer.join().expect("peer"), ack == Some(true));
            assert_eq!(source.next.load(Ordering::Relaxed), 1);
            assert_eq!(source.outcomes.lock().expect("outcomes").len(), 1);
        }
    }

    #[derive(Debug)]
    struct Source {
        next: AtomicU64,
        outcomes: Arc<Mutex<Vec<(u64, DiagnosticAttemptOutcomeV1)>>>,
    }

    #[derive(Debug)]
    struct Guard {
        token: DiagnosticAttemptTokenV1,
        outcomes: Arc<Mutex<Vec<(u64, DiagnosticAttemptOutcomeV1)>>>,
    }

    impl DiagnosticAttemptSourceV1 for Source {
        fn begin_attempt(&self) -> Option<Box<dyn DiagnosticAttemptGuardV1>> {
            let id = match self
                .next
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |next| {
                    next.checked_add(1)
                }) {
                Ok(previous) => previous.checked_add(1)?,
                Err(_exhausted_attempt_id) => return None,
            };
            let token = DiagnosticAttemptTokenV1::new([3; 16], NonZeroU64::new(id)?);
            Some(Box::new(Guard {
                token,
                outcomes: Arc::clone(&self.outcomes),
            }))
        }
    }

    impl DiagnosticAttemptGuardV1 for Guard {
        fn token(&self) -> DiagnosticAttemptTokenV1 {
            self.token
        }

        fn finish(self: Box<Self>, outcome: DiagnosticAttemptOutcomeV1) {
            self.outcomes
                .lock()
                .expect("test outcome lock")
                .push((self.token.attempt_span_id().get(), outcome));
        }
    }

    #[derive(Debug)]
    struct Request {
        diagnostic_attempt: DiagnosticAttemptFieldV1,
        virtual_payload: Option<VirtualPayload>,
    }

    #[derive(Debug)]
    struct VirtualPayload {
        chunk: Vec<u8>,
        tail: Vec<u8>,
    }

    struct ByteView<'a>(&'a [u8]);

    impl serde::Serialize for ByteView<'_> {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: serde::Serializer,
        {
            serializer.serialize_bytes(self.0)
        }
    }

    impl serde::Serialize for VirtualPayload {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: serde::Serializer,
        {
            let mut parts = serializer.serialize_seq(Some(128))?;
            for _ in 0..127 {
                parts.serialize_element(&ByteView(&self.chunk))?;
            }
            parts.serialize_element(&ByteView(&self.tail))?;
            parts.end()
        }
    }

    impl serde::Serialize for Request {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: serde::Serializer,
        {
            let token = self.diagnostic_attempt.token();
            let mut fields =
                serializer.serialize_struct("Request", if token.is_some() { 2 } else { 1 })?;
            if let Some(payload) = &self.virtual_payload {
                fields.serialize_field("payload", payload)?;
            } else {
                fields.serialize_field("payload", &1_u8)?;
            }
            if let Some(token) = token {
                fields.serialize_field("diagnostic_attempt", &token)?;
            }
            fields.end()
        }
    }

    impl DiagnosticAttemptEnvelope for Request {
        type Payload = Option<VirtualPayload>;

        fn diagnostic_attempt(&mut self) -> &mut DiagnosticAttemptFieldV1 {
            &mut self.diagnostic_attempt
        }

        fn request_id(&self) -> u64 {
            1
        }

        fn payload(&self) -> &Self::Payload {
            &self.virtual_payload
        }
    }

    #[test]
    fn each_send_uses_a_fresh_attempt_and_records_its_result() {
        let source = Source {
            next: AtomicU64::new(0),
            outcomes: Arc::new(Mutex::new(Vec::new())),
        };
        let first = send_with_attempt(
            Request {
                diagnostic_attempt: DiagnosticAttemptFieldV1::Absent,
                virtual_payload: None,
            },
            Some(&source),
            true,
            |request| Ok::<_, SdkError>(request.diagnostic_attempt.token()),
        )
        .expect("first send");
        let second = send_with_attempt(
            Request {
                diagnostic_attempt: DiagnosticAttemptFieldV1::Absent,
                virtual_payload: None,
            },
            Some(&source),
            true,
            |request| {
                Err::<Option<DiagnosticAttemptTokenV1>, _>(SdkError::Protocol(format!(
                    "attempt {} failed",
                    request
                        .diagnostic_attempt
                        .token()
                        .expect("token")
                        .attempt_span_id()
                )))
            },
        );
        assert_eq!(first.expect("first token").attempt_span_id().get(), 1);
        assert!(matches!(second, Err(SdkError::Protocol(_))));
        assert_eq!(source.next.load(Ordering::Relaxed), 2);
        assert_eq!(
            *source.outcomes.lock().expect("test outcome lock"),
            vec![
                (1, DiagnosticAttemptOutcomeV1::Ok),
                (2, DiagnosticAttemptOutcomeV1::Error),
            ]
        );
    }

    #[test]
    fn unverified_peer_receives_original_envelope_but_attempt_finishes() {
        let source = Source {
            next: AtomicU64::new(0),
            outcomes: Arc::new(Mutex::new(Vec::new())),
        };
        let request = Request {
            diagnostic_attempt: DiagnosticAttemptFieldV1::Absent,
            virtual_payload: None,
        };
        let observed = send_with_attempt(request, Some(&source), false, |request| {
            Ok::<_, SdkError>(request.diagnostic_attempt)
        })
        .expect("unverified send");
        assert_eq!(observed, DiagnosticAttemptFieldV1::Absent);
        assert_eq!(source.next.load(Ordering::Relaxed), 1);
        assert_eq!(
            *source.outcomes.lock().expect("test outcome lock"),
            vec![(1, DiagnosticAttemptOutcomeV1::Ok)]
        );
    }

    #[test]
    fn token_is_omitted_only_when_it_crosses_the_decoded_request_cap() {
        let source = Source {
            next: AtomicU64::new(0),
            outcomes: Arc::new(Mutex::new(Vec::new())),
        };
        let mut request = Request {
            diagnostic_attempt: DiagnosticAttemptFieldV1::Absent,
            virtual_payload: Some(VirtualPayload {
                chunk: vec![0; 1024 * 1024],
                tail: Vec::new(),
            }),
        };
        let base_bytes = match request_size_decision_v1(&request).expect("base length") {
            RequestSizeDecisionV1::Fits { decoded_bytes } => decoded_bytes,
            RequestSizeDecisionV1::ExceedsDecodedCap { .. } => panic!("base exceeds cap"),
        };
        let cap_bytes = 128_u64 * 1024 * 1024;
        // A CBOR byte-string header grows by four bytes at this tail length.
        let tail_len = usize::try_from(cap_bytes - base_bytes - 44).expect("bounded tail");
        request.virtual_payload.as_mut().expect("payload").tail = vec![0; tail_len];
        assert!(matches!(
            request_size_decision_v1(&request).expect("without token"),
            RequestSizeDecisionV1::Fits { .. }
        ));
        request.diagnostic_attempt = DiagnosticAttemptFieldV1::Linked(
            DiagnosticAttemptTokenV1::new([3; 16], NonZeroU64::new(1).expect("span ID")),
        );
        assert!(matches!(
            request_size_decision_v1(&request).expect("with token"),
            RequestSizeDecisionV1::ExceedsDecodedCap { .. }
        ));
        request.diagnostic_attempt = DiagnosticAttemptFieldV1::Absent;
        let observed = send_with_attempt(request, Some(&source), true, |request| {
            Ok::<_, SdkError>(request.diagnostic_attempt)
        })
        .expect("metadata-only overflow is omitted");
        assert_eq!(observed, DiagnosticAttemptFieldV1::Absent);
        assert_eq!(
            *source.outcomes.lock().expect("test outcome lock"),
            vec![(1, DiagnosticAttemptOutcomeV1::Ok)]
        );
    }
}
