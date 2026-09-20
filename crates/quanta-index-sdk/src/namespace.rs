#![expect(
    clippy::redundant_pub_crate,
    reason = "crate-private namespace infrastructure is shared across sibling SDK modules"
)]

//! Internal namespace capability traits (QI-NS-01).
//!
//! This module is the crate-private owner for the SDK's typed namespace
//! dispatch traits:
//!
//! - [`NamespaceIngest`] for publish-style namespace operations
//! - [`NamespaceQuery`] for query-builder namespace operations
//!
//! Built-in namespaces such as lexical query, search-corpus ingest, semantic,
//! and repo-map implement these traits directly. Production code uses typed
//! namespace surfaces such as `client.lexical()`, `client.search_corpus()`,
//! and siblings. The generic [`NamespaceHandle`] below is test-only and exists
//! only to prove that the trait split remains open for internal namespace
//! conformance tests.

#[cfg(test)]
use std::marker::PhantomData;

use crate::{QuantaIndex, SdkError};

/// Marker capability: a namespace exposes a typed publish path.
///
/// The marker `Self` is a zero-sized type used purely for type-level
/// dispatch (e.g. [`crate::lexical::SearchCorpusNs`]). `publish` consumes a
/// reference to the SDK-side batch DTO and returns the namespace's receipt
/// shape.
///
/// Implementations route through `QuantaIndex::dispatch_ingest` and are
/// responsible for:
///
/// - mapping the SDK batch into the namespace's ingest IPC request variant,
/// - extracting the namespace's receipt variant from the response, and
/// - rejecting cross-namespace responses with a typed
///   [`SdkError::Protocol`] (no silent fallback per CLAUDE.md safety
///   rules).
pub(crate) trait NamespaceIngest {
    /// SDK-side batch shape. Typically an idiomatic builder type
    /// (`SearchCorpusBatch`, `HistoryBatch`, etc.).
    type Batch;
    /// Receipt shape returned on a successful publish. Typically
    /// [`quanta_index_contract::BatchPublishReceipt`] for stream
    /// namespaces or a typed ack for one-shot bundle ingests.
    type Receipt;

    /// Publish a batch through the SDK's ingest transport.
    fn publish(client: &QuantaIndex, batch: &Self::Batch) -> Result<Self::Receipt, SdkError>;
}

/// Marker capability: a namespace exposes a typed query builder.
///
/// The builder carries the namespace's query-specific options.
///
/// Examples include syntax, generation selection, `top_k`, and the typed
/// response returned by `execute()`. The associated `QueryBuilder<'a>` GAT
/// ties the builder's lifetime to the SDK client reference.
pub(crate) trait NamespaceQuery {
    /// Per-namespace builder type. Each namespace defines its own; there
    /// is no fat shared trait (ISP). The builder is responsible for
    /// dispatching through `QuantaIndex::dispatch_query` when its
    /// `execute()` method is called.
    type QueryBuilder<'a>
    where
        Self: 'a;

    /// Construct a fresh query builder bound to `client`.
    fn query(client: &QuantaIndex) -> Self::QueryBuilder<'_>;
}

/// Test-only handle returned by [`QuantaIndex::ns`].
///
/// This wrapper gates `.publish` and `.query` on which capabilities the marker
/// `N` implements. Calling `.publish` on a namespace that does not implement
/// [`NamespaceIngest`] is a compile-time error.
#[cfg(test)]
pub(crate) struct NamespaceHandle<'a, N>
where
    N: ?Sized,
{
    client: &'a QuantaIndex,
    // PhantomData over `fn() -> N` so the handle is invariant in the
    // marker but does not borrow `N` itself (markers are zero-sized).
    _marker: PhantomData<fn() -> N>,
}

#[cfg(test)]
impl<'a, N> NamespaceHandle<'a, N>
where
    N: ?Sized,
{
    pub(crate) const fn new(client: &'a QuantaIndex) -> NamespaceHandle<'a, N> {
        NamespaceHandle {
            client,
            _marker: PhantomData,
        }
    }
}

#[cfg(test)]
impl<N> NamespaceHandle<'_, N>
where
    N: NamespaceIngest + ?Sized,
{
    /// Publish a typed batch through the namespace's ingest path. Errors
    /// from the wire surface as [`SdkError::Remote`]; cross-namespace
    /// responses surface as [`SdkError::Protocol`].
    pub(crate) fn publish(&self, batch: &N::Batch) -> Result<N::Receipt, SdkError> {
        N::publish(self.client, batch)
    }
}

#[cfg(test)]
impl<'a, N> NamespaceHandle<'a, N>
where
    N: NamespaceQuery + ?Sized,
{
    /// Construct a fresh query builder for the namespace.
    ///
    /// This is identical to the built-in `client.lexical().query()` or
    /// `client.semantic().query()` sugar. Those sugar layers delegate here.
    ///
    /// The returned builder is bound to the client lifetime `'a` (the
    /// lifetime carried by the handle), not to `&self`. Sibling sugar
    /// methods on [`crate::LexicalNamespace`] / [`crate::SemanticNamespace`] /
    /// etc. already return `QueryBuilder<'a>`; this method matches them so
    /// callers can hold the builder past the handle scope.
    #[must_use]
    pub(crate) fn query(&self) -> N::QueryBuilder<'a> {
        N::query(self.client)
    }
}

#[cfg(test)]
mod tests {
    //! Namespace trait conformance tests for the test-only generic handle.

    use std::sync::{Arc, Mutex};

    use quanta_index_contract::lex::LanguageCode;
    use quanta_index_contract::{
        BatchPublishReceipt, ChunkId, ManifestGeneration, RepoId, RevisionId,
        SearchPlaneControlIpcRequestEnvelope, SearchPlaneControlIpcResponseEnvelope,
        SearchPlaneIngestIpcRequest, SearchPlaneIngestIpcRequestEnvelope,
        SearchPlaneIngestIpcResponse, SearchPlaneIngestIpcResponseEnvelope,
        SearchPlaneQueryIpcRequestEnvelope, SearchPlaneQueryIpcResponseEnvelope,
    };

    use super::*;
    use crate::{
        BatchReceipt, ControlTransport, IngestTransport, QueryTransport, SearchCorpusBatch,
    };

    type TestRes = Result<(), String>;

    /// Test-local marker used only in this module.
    struct DownstreamSearchCorpusNs;

    impl NamespaceIngest for DownstreamSearchCorpusNs {
        type Batch = SearchCorpusBatch;
        type Receipt = BatchReceipt;

        fn publish(
            client: &QuantaIndex,
            batch: &SearchCorpusBatch,
        ) -> Result<BatchReceipt, SdkError> {
            // Reuse the SearchCorpusNs implementation so the wire path is
            // exercised exactly once.
            <crate::lexical::SearchCorpusNs as NamespaceIngest>::publish(client, batch)
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
            {
                let mut requests = self
                    .requests
                    .lock()
                    .map_err(|err| SdkError::Protocol(format!("ingest mutex poisoned: {err}")))?;
                requests.push(request.clone());
            }
            let payload = {
                let mut response = self.response.lock().map_err(|err| {
                    SdkError::Protocol(format!("ingest response mutex poisoned: {err}"))
                })?;
                response
                    .take()
                    .ok_or_else(|| SdkError::Protocol("missing ingest stub response".to_string()))?
            };
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
            Err(SdkError::Protocol(
                "control transport unused in test".into(),
            ))
        }
    }

    fn make_client(ingest: Arc<StubIngestTransport>) -> QuantaIndex {
        QuantaIndex::from_transports(
            Arc::new(StubQueryTransport),
            Arc::new(StubControlTransport),
            ingest,
        )
    }

    /// The receipt the search plane would answer `fixture_batch` with: it
    /// names the batch's canonical digest.
    fn fixture_receipt() -> Result<BatchPublishReceipt, String> {
        Ok(BatchPublishReceipt {
            generation: ManifestGeneration::new(1),
            manifest_digest: Some("manifest:digest".to_string()),
            batch_digest: fixture_batch()?
                .batch_digest()
                .map_err(|err| format!("fixture batch digest: {err}"))?,
            applied: true,
            durable_sequence: 7,
            semantic_content: None,
            accepted_clear_surfaces: 0,
            accepted_replace_scopes: 1,
            accepted_tombstone_scopes: 0,
            accepted_semantic_replace_scopes: 0,
            accepted_semantic_tombstone_scopes: 0,
            sealed: true,
        })
    }

    fn assert_test_ok(result: &TestRes) {
        assert!(result.is_ok(), "{result:?}");
    }

    fn fixture_batch() -> Result<SearchCorpusBatch, String> {
        let language = LanguageCode::new("rust")
            .map_err(|err| format!("valid language code fixture required: {err}"))?;
        Ok(SearchCorpusBatch::replace_generation(
            RepoId::new("repo"),
            RevisionId::new("rev"),
            ManifestGeneration::new(1),
            "manifest:digest",
        )
        .replace_scope(
            quanta_index_contract::SearchScopeKey {
                doc_surface: quanta_index_contract::SearchScopeSurface::File,
                repo_relative_path: quanta_index_contract::RepoRelativePath::new("src/lib.rs"),
            },
            "scope:digest",
            vec![quanta_index_contract::ChunkRecord {
                chunk_id: ChunkId::new("c1"),
                repo_relative_path: quanta_index_contract::RepoRelativePath::new("src/lib.rs"),
                language,
                start_byte: 0,
                end_byte: 12,
                start_line: 1,
                end_line: 2,
                text: "fn main() {}".to_string().into_boxed_str(),
                structural: None,
                parent_chunk_id: None,
                source_repo_id: None,
            }],
            Vec::new(),
        ))
    }

    fn only_ingest_request(
        ingest: &StubIngestTransport,
    ) -> Result<SearchPlaneIngestIpcRequestEnvelope, String> {
        let captured = ingest
            .requests
            .lock()
            .map_err(|err| format!("captured requests must not be poisoned: {err}"))?;
        let len = captured.len();
        if len != 1 {
            return Err(format!("expected one captured request, got {len}"));
        }
        captured
            .first()
            .cloned()
            .ok_or_else(|| "expected one captured request".to_string())
    }

    #[test]
    fn test_local_marker_routes_through_ns_handle() {
        let result = (|| -> TestRes {
            let ingest = Arc::new(StubIngestTransport {
                requests: Mutex::new(Vec::new()),
                response: Mutex::new(Some(SearchPlaneIngestIpcResponse::SearchCorpusReceipt(
                    fixture_receipt()?,
                ))),
            });
            let client = make_client(Arc::clone(&ingest));
            let batch = fixture_batch()?;
            let receipt = client
                .ns::<DownstreamSearchCorpusNs>()
                .publish(&batch)
                .map_err(|err| format!("test-local publish must succeed: {err}"))?;
            assert_eq!(receipt.generation, ManifestGeneration::new(1));
            let first = only_ingest_request(ingest.as_ref())?;
            assert!(matches!(
                first.payload,
                SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(_)
            ));
            Ok(())
        })();
        assert_test_ok(&result);
    }

    #[test]
    fn sugar_search_corpus_and_ns_search_corpus_produce_equivalent_wire() {
        let result = (|| -> TestRes {
            // QI-NS-01: `client.search_corpus().publish(batch)` is sugar for
            // `client.ns::<SearchCorpusNs>().publish(batch)`. Wire output must
            // be identical apart from request_id allocation.
            let batch = fixture_batch()?;

            let sugar_ingest = Arc::new(StubIngestTransport {
                requests: Mutex::new(Vec::new()),
                response: Mutex::new(Some(SearchPlaneIngestIpcResponse::SearchCorpusReceipt(
                    fixture_receipt()?,
                ))),
            });
            let sugar_client = make_client(Arc::clone(&sugar_ingest));
            let _sugar_receipt = sugar_client
                .search_corpus()
                .publish(&batch)
                .map_err(|err| format!("sugar publish must succeed: {err}"))?;

            let ns_ingest = Arc::new(StubIngestTransport {
                requests: Mutex::new(Vec::new()),
                response: Mutex::new(Some(SearchPlaneIngestIpcResponse::SearchCorpusReceipt(
                    fixture_receipt()?,
                ))),
            });
            let ns_client = make_client(Arc::clone(&ns_ingest));
            let _ns_receipt = ns_client
                .ns::<crate::lexical::SearchCorpusNs>()
                .publish(&batch)
                .map_err(|err| format!("ns publish must succeed: {err}"))?;

            let sugar_payload = only_ingest_request(sugar_ingest.as_ref())?.payload;
            let ns_payload = only_ingest_request(ns_ingest.as_ref())?.payload;
            assert_eq!(sugar_payload, ns_payload);
            Ok(())
        })();
        assert_test_ok(&result);
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
        let _builder = client
            .ns::<crate::lexical::LexicalNs>()
            .query()
            .native("anchor")
            .top_k(1);
    }
}
