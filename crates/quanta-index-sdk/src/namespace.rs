//! Generic namespace registry (QI-NS-01).
//!
//! Producer-facing extension point. Adding a new namespace (e.g. a derived
//! index family that lives alongside lexical / semantic / repomap) requires
//! exactly three things:
//!
//! 1. **One typed batch DTO + ingest variant in `quanta-index-contract`** —
//!    extend [`SearchPlaneIngestIpcRequest`] / `Response` with one new
//!    variant. (Symmetric on the query side if the namespace exposes a
//!    typed query path.)
//!
//! 2. **One marker type + trait impl in this crate (or downstream)** —
//!    `pub struct MyNs;` plus `impl NamespaceIngest for MyNs { ... }` and/or
//!    `impl NamespaceQuery for MyNs { ... }`.
//!
//! 3. **No edit to SDK core** — `QuantaIndex::ns::<N>()` is generic over the
//!    marker. The composition root, dispatcher, and transports do not
//!    learn about new namespaces. This satisfies the CLAUDE.md
//!    Open/Closed rule for variant edges ("adding a new IPC variant must
//!    not force a sprawling edit across encode + decode + match + factory +
//!    test all at once").
//!
//! Built-in namespaces ([`LexicalNs`], [`SemanticNs`], [`RepoMapNs`]) are
//! defined via the same trait surface; the existing `client.lexical()` etc.
//! sugar methods delegate to the marker-impl paths so behavior is uniform.

use std::marker::PhantomData;

use crate::{QuantaIndex, SdkError};

/// Marker capability: a namespace exposes a typed publish path.
///
/// The marker `Self` is a zero-sized type used purely for type-level
/// dispatch (e.g. [`LexicalNs`]). `publish` consumes a reference to the
/// SDK-side batch DTO and returns the namespace's receipt shape.
///
/// Implementations route through [`QuantaIndex::dispatch_ingest`] and are
/// responsible for:
///
/// - mapping the SDK batch into the namespace's ingest IPC request variant,
/// - extracting the namespace's receipt variant from the response, and
/// - rejecting cross-namespace responses with a typed
///   [`SdkError::Protocol`] (no silent fallback per CLAUDE.md safety
///   rules).
pub trait NamespaceIngest {
    /// SDK-side batch shape. Typically an idiomatic builder type
    /// (`LexicalBatch`, `SemanticBatch`, etc.).
    type Batch;
    /// Receipt shape returned on a successful publish. Typically
    /// [`quanta_index_contract::BatchPublishReceipt`] for stream
    /// namespaces or a typed ack for one-shot bundle ingests.
    type Receipt;

    /// Publish a batch through the SDK's ingest transport.
    fn publish(
        client: &QuantaIndex,
        batch: &Self::Batch,
    ) -> Result<Self::Receipt, SdkError>;
}

/// Marker capability: a namespace exposes a typed query builder.
///
/// The builder carries the namespace's query-specific options (syntax,
/// generation selection, top_k, etc.) and produces the typed response
/// when executed. The associated `QueryBuilder<'a>` GAT ties the
/// builder's lifetime to the SDK client reference.
pub trait NamespaceQuery {
    /// Per-namespace builder type. Each namespace defines its own; there
    /// is no fat shared trait (ISP). The builder is responsible for
    /// dispatching through [`QuantaIndex::dispatch_query`] when its
    /// `execute()` method is called.
    type QueryBuilder<'a>
    where
        Self: 'a;

    /// Construct a fresh query builder bound to `client`.
    fn query<'a>(client: &'a QuantaIndex) -> Self::QueryBuilder<'a>;
}

/// Handle returned by [`QuantaIndex::ns`]. Thin wrapper that gates
/// `.publish` and `.query` on which capabilities the marker `N`
/// implements; calling `.publish` on a namespace that does not implement
/// [`NamespaceIngest`] is a compile-time error (no runtime fallback).
pub struct NamespaceHandle<'a, N>
where
    N: ?Sized,
{
    client: &'a QuantaIndex,
    // PhantomData over `fn() -> N` so the handle is invariant in the
    // marker but does not borrow `N` itself (markers are zero-sized).
    _marker: PhantomData<fn() -> N>,
}

impl<'a, N> NamespaceHandle<'a, N>
where
    N: ?Sized,
{
    pub(crate) const fn new(client: &'a QuantaIndex) -> Self {
        Self {
            client,
            _marker: PhantomData,
        }
    }

    /// Underlying SDK client. Exposed for callers that want to drop back
    /// to the `lexical()` / `semantic()` sugar within the same scope.
    #[must_use]
    pub const fn client(&self) -> &'a QuantaIndex {
        self.client
    }
}

impl<'a, N> NamespaceHandle<'a, N>
where
    N: NamespaceIngest + ?Sized,
{
    /// Publish a typed batch through the namespace's ingest path. Errors
    /// from the wire surface as [`SdkError::Remote`]; cross-namespace
    /// responses surface as [`SdkError::Protocol`].
    pub fn publish(&self, batch: &N::Batch) -> Result<N::Receipt, SdkError> {
        N::publish(self.client, batch)
    }
}

impl<'a, N> NamespaceHandle<'a, N>
where
    N: NamespaceQuery + ?Sized + 'a,
{
    /// Construct a fresh query builder for the namespace. Identical to the
    /// built-in `client.lexical().query()` / `client.semantic().query()`
    /// sugar — the sugar layers delegate to this path under the hood.
    pub fn query(&self) -> N::QueryBuilder<'a> {
        N::query(self.client)
    }
}

#[cfg(test)]
mod tests {
    //! Custom-namespace round-trip. Verifies that a downstream marker
    //! can be defined entirely outside the SDK core, route through
    //! `client.ns::<MyNs>()`, and that the SDK core / dispatcher need no
    //! edits.

    use std::sync::{Arc, Mutex};

    use quanta_index_contract::{
        BatchPublishReceipt, ChannelSeq, ChunkId, ManifestGeneration, RepoId, RevisionId,
        SearchPlaneControlIpcRequestEnvelope, SearchPlaneControlIpcResponseEnvelope,
        SearchPlaneIngestIpcRequest, SearchPlaneIngestIpcRequestEnvelope,
        SearchPlaneIngestIpcResponse, SearchPlaneIngestIpcResponseEnvelope,
        SearchPlaneQueryIpcRequestEnvelope, SearchPlaneQueryIpcResponseEnvelope,
    };

    use super::*;
    use crate::{
        BatchReceipt, ControlTransport, IngestTransport, LexicalBatch, QueryTransport,
    };

    /// Downstream marker. Demonstrates that wiring is decoupled from the
    /// SDK core: this struct exists only in the test module, but it
    /// composes through `client.ns::<DownstreamLexicalNs>()` without any
    /// edit to the namespace module, client, or transports.
    struct DownstreamLexicalNs;

    impl NamespaceIngest for DownstreamLexicalNs {
        type Batch = LexicalBatch;
        type Receipt = BatchReceipt;

        fn publish(
            client: &QuantaIndex,
            batch: &LexicalBatch,
        ) -> Result<BatchReceipt, SdkError> {
            // Reuse the LexicalNs implementation so the wire path is
            // exercised exactly once. A real downstream namespace would
            // construct its own typed ingest variant.
            <crate::LexicalNs as NamespaceIngest>::publish(client, batch)
        }
    }

    // --- Stub transports (trimmed copies of the ones in `tests.rs`) ----

    struct StubIngestTransport {
        requests: Mutex<Vec<SearchPlaneIngestIpcRequestEnvelope>>,
        response: Mutex<Option<SearchPlaneIngestIpcResponse>>,
    }

    impl IngestTransport for StubIngestTransport {
        fn send(
            &self,
            request: SearchPlaneIngestIpcRequestEnvelope,
        ) -> Result<SearchPlaneIngestIpcResponseEnvelope, SdkError> {
            self.requests.lock().expect("ingest mutex").push(request.clone());
            let payload = self
                .response
                .lock()
                .expect("ingest response mutex")
                .take()
                .expect("ingest stub response");
            Ok(SearchPlaneIngestIpcResponseEnvelope {
                request_id: request.request_id,
                payload,
            })
        }
    }

    struct StubQueryTransport;

    impl QueryTransport for StubQueryTransport {
        fn send(
            &self,
            _request: SearchPlaneQueryIpcRequestEnvelope,
        ) -> Result<SearchPlaneQueryIpcResponseEnvelope, SdkError> {
            Err(SdkError::Protocol("query transport unused in test".into()))
        }
    }

    struct StubControlTransport;

    impl ControlTransport for StubControlTransport {
        fn send(
            &self,
            _request: SearchPlaneControlIpcRequestEnvelope,
        ) -> Result<SearchPlaneControlIpcResponseEnvelope, SdkError> {
            Err(SdkError::Protocol("control transport unused in test".into()))
        }
    }

    fn make_client(ingest: Arc<StubIngestTransport>) -> QuantaIndex {
        QuantaIndex::from_transports(
            Arc::new(StubQueryTransport),
            Arc::new(StubControlTransport),
            ingest,
        )
    }

    fn fixture_receipt() -> BatchPublishReceipt {
        BatchPublishReceipt {
            first_seq: Some(ChannelSeq::new(0)),
            last_seq: Some(ChannelSeq::new(2)),
            sealed: true,
        }
    }

    fn fixture_batch() -> LexicalBatch {
        LexicalBatch::replace_generation(
            RepoId::new("repo"),
            RevisionId::new("rev"),
            ManifestGeneration::new(1),
        )
        .chunk_upsert(
            ChunkId::new("c1"),
            quanta_index_contract::ChunkRecord {
                repo_relative_path: quanta_index_contract::RepoRelativePath::new("src/lib.rs"),
                language: "rust".to_string().into_boxed_str(),
                start_line: 1,
                end_line: 2,
                snippet: "fn main() {}".to_string().into_boxed_str(),
            },
        )
    }

    #[test]
    fn downstream_marker_routes_through_ns_handle() {
        let ingest = Arc::new(StubIngestTransport {
            requests: Mutex::new(Vec::new()),
            response: Mutex::new(Some(SearchPlaneIngestIpcResponse::LexicalReceipt(
                fixture_receipt(),
            ))),
        });
        let client = make_client(Arc::clone(&ingest));
        let receipt = client
            .ns::<DownstreamLexicalNs>()
            .publish(&fixture_batch())
            .expect("downstream publish");
        assert_eq!(receipt.first_seq, Some(ChannelSeq::new(0)));
        let captured = ingest.requests.lock().expect("captured");
        assert_eq!(captured.len(), 1);
        assert!(matches!(
            captured[0].payload,
            SearchPlaneIngestIpcRequest::PublishLexicalBatch(_)
        ));
    }

    #[test]
    fn sugar_lexical_and_ns_lexical_produce_equivalent_wire() {
        // QI-NS-01: `client.lexical().publish(batch)` is sugar for
        // `client.ns::<LexicalNs>().publish(batch)`. Wire output must be
        // identical apart from request_id allocation.
        let batch = fixture_batch();

        let sugar_ingest = Arc::new(StubIngestTransport {
            requests: Mutex::new(Vec::new()),
            response: Mutex::new(Some(SearchPlaneIngestIpcResponse::LexicalReceipt(
                fixture_receipt(),
            ))),
        });
        let sugar_client = make_client(Arc::clone(&sugar_ingest));
        let _sugar_receipt = sugar_client
            .lexical()
            .publish(&batch)
            .expect("sugar publish");

        let ns_ingest = Arc::new(StubIngestTransport {
            requests: Mutex::new(Vec::new()),
            response: Mutex::new(Some(SearchPlaneIngestIpcResponse::LexicalReceipt(
                fixture_receipt(),
            ))),
        });
        let ns_client = make_client(Arc::clone(&ns_ingest));
        let _ns_receipt = ns_client
            .ns::<crate::LexicalNs>()
            .publish(&batch)
            .expect("ns publish");

        let sugar_captured = sugar_ingest.requests.lock().expect("sugar captured");
        let ns_captured = ns_ingest.requests.lock().expect("ns captured");
        assert_eq!(sugar_captured.len(), 1);
        assert_eq!(ns_captured.len(), 1);
        assert_eq!(sugar_captured[0].payload, ns_captured[0].payload);
    }

    #[test]
    fn ns_query_returns_namespace_specific_builder() {
        // QI-NS-01: type-level dispatch picks the right query builder.
        // The fact that this compiles is the test — `client.ns::<N>()
        // .query()` returns `N::QueryBuilder<'_>`. The chained
        // `.native(...)` call anchors the type to `LexicalQueryBuilder`
        // because that method exists only on the lexical builder.
        let client = make_client(Arc::new(StubIngestTransport {
            requests: Mutex::new(Vec::new()),
            response: Mutex::new(None),
        }));
        let _builder = client.ns::<crate::LexicalNs>().query().native("anchor").top_k(1);
    }
}
