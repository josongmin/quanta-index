use super::*;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use ciborium::de::{FiniteParserAllocationAdmissionV3, FiniteParserAllocationRefusalV3};
use quanta_index_contract::{
    DecodeAttemptStateV1, DecodeBackingAdmissionV1, DecodeBackingRefusalV1, DecodeFieldKindV1,
    DecodeFieldTokenV1, HistoryIngestBatch, ParserTextInputErrorV1, SemanticContentRootsV1,
    SourceIdentityDecodeAdmissionV1,
};
use quanta_index_ipc::{
    IndexError, MemoryLease, MemoryLeaseOwner, MemoryLifetime, MemoryScope, MemoryScopePort,
    RequestBudgetV1,
};
use serde::Deserialize;

// Dispatch/lifetime fixture only; physical Original admission is exercised by S.
#[derive(Clone, Default)]
struct DecodeOwner {
    state: Arc<Mutex<DecodeAttemptStateV1>>,
    text_rows: Arc<Mutex<Vec<(usize, usize)>>>,
    aborted: Arc<AtomicBool>,
}

impl FiniteParserAllocationAdmissionV3 for DecodeOwner {
    fn begin_preflight_v3(&mut self) -> Result<(), FiniteParserAllocationRefusalV3> {
        self.state
            .lock()
            .map_err(|_cause| FiniteParserAllocationRefusalV3)?
            .begin_preflight_v1()
            .map_err(|_cause| FiniteParserAllocationRefusalV3)
    }
    fn complete_preflight_v3(&mut self) -> Result<(), FiniteParserAllocationRefusalV3> {
        self.state
            .lock()
            .map_err(|_cause| FiniteParserAllocationRefusalV3)?
            .complete_preflight_v1()
            .map_err(|_cause| FiniteParserAllocationRefusalV3)
    }
    fn begin_decode_v3(&mut self) -> Result<(), FiniteParserAllocationRefusalV3> {
        self.state
            .lock()
            .map_err(|_cause| FiniteParserAllocationRefusalV3)?
            .begin_decode_v1()
            .map_err(|_cause| FiniteParserAllocationRefusalV3)
    }
    fn complete_decode_v3(&mut self) -> Result<(), FiniteParserAllocationRefusalV3> {
        self.state
            .lock()
            .map_err(|_cause| FiniteParserAllocationRefusalV3)?
            .complete_decode_v1()
            .map_err(|_cause| FiniteParserAllocationRefusalV3)
    }
    fn abort_decode_v3(&mut self) {
        self.abort_decode();
    }
    fn check_text_length_v3(
        &mut self,
        length: Option<usize>,
    ) -> Result<(), FiniteParserAllocationRefusalV3> {
        self.state
            .lock()
            .map_err(|_cause| FiniteParserAllocationRefusalV3)?
            .check_length_v1(DecodeFieldKindV1::Text, length, 4096)
            .map_err(|_cause| FiniteParserAllocationRefusalV3)
    }
    fn check_bytes_length_v3(
        &mut self,
        length: Option<usize>,
    ) -> Result<(), FiniteParserAllocationRefusalV3> {
        self.state
            .lock()
            .map_err(|_cause| FiniteParserAllocationRefusalV3)?
            .check_length_v1(DecodeFieldKindV1::Bytes, length, 4096)
            .map_err(|_cause| FiniteParserAllocationRefusalV3)
    }
    fn check_sequence_length_v3(
        &mut self,
        length: Option<usize>,
    ) -> Result<(), FiniteParserAllocationRefusalV3> {
        self.state
            .lock()
            .map_err(|_cause| FiniteParserAllocationRefusalV3)?
            .check_sequence_header_v1(length, 4096)
            .map_err(|_cause| FiniteParserAllocationRefusalV3)
    }
    fn reserve_text_v3(
        &mut self,
        destination: &mut String,
        additional: usize,
    ) -> Result<(), FiniteParserAllocationRefusalV3> {
        let length = destination
            .len()
            .checked_add(additional)
            .ok_or(FiniteParserAllocationRefusalV3)?;
        self.check_text_length_v3(Some(length))?;
        destination
            .try_reserve_exact(additional)
            .map_err(|_cause| FiniteParserAllocationRefusalV3)?;
        self.text_rows
            .lock()
            .map_err(|_cause| FiniteParserAllocationRefusalV3)?
            .push((destination.as_ptr().addr(), destination.capacity()));
        Ok(())
    }
    fn reserve_bytes_v3(
        &mut self,
        destination: &mut Vec<u8>,
        additional: usize,
    ) -> Result<(), FiniteParserAllocationRefusalV3> {
        let length = destination
            .len()
            .checked_add(additional)
            .ok_or(FiniteParserAllocationRefusalV3)?;
        self.check_bytes_length_v3(Some(length))?;
        destination
            .try_reserve_exact(additional)
            .map_err(|_cause| FiniteParserAllocationRefusalV3)
    }
}

impl DecodeBackingAdmissionV1 for DecodeOwner {
    fn abort_decode(&self) {
        self.aborted.store(true, Ordering::SeqCst);
        if let Ok(mut state) = self.state.lock() {
            state.abort_v1();
        }
    }
    fn enter_decode_field_v1(
        &self,
        kind: DecodeFieldKindV1,
        maximum: usize,
    ) -> Result<DecodeFieldTokenV1, DecodeBackingRefusalV1> {
        self.state
            .lock()
            .map_err(|_cause| DecodeBackingRefusalV1)?
            .enter_field_v1(kind, maximum)
    }
    fn leave_decode_field_v1(
        &self,
        token: DecodeFieldTokenV1,
    ) -> Result<(), DecodeBackingRefusalV1> {
        self.state
            .lock()
            .map_err(|_cause| DecodeBackingRefusalV1)?
            .leave_field_v1(token)
    }
    fn reserve_text(
        &self,
        destination: &mut String,
        additional: usize,
    ) -> Result<(), DecodeBackingRefusalV1> {
        if self.aborted.load(Ordering::SeqCst) {
            return Err(DecodeBackingRefusalV1);
        }
        destination
            .try_reserve_exact(additional)
            .map_err(|_cause| DecodeBackingRefusalV1)
    }
    fn reserve_bytes(
        &self,
        destination: &mut Vec<u8>,
        additional: usize,
    ) -> Result<(), DecodeBackingRefusalV1> {
        if self.aborted.load(Ordering::SeqCst) {
            return Err(DecodeBackingRefusalV1);
        }
        destination
            .try_reserve_exact(additional)
            .map_err(|_cause| DecodeBackingRefusalV1)
    }
    fn reserve_elements<T>(
        &self,
        destination: &mut Vec<T>,
        additional: usize,
    ) -> Result<(), DecodeBackingRefusalV1> {
        if self.aborted.load(Ordering::SeqCst) {
            return Err(DecodeBackingRefusalV1);
        }
        destination
            .try_reserve_exact(additional)
            .map_err(|_cause| DecodeBackingRefusalV1)
    }
    fn copy_boxed_text(&self, source: &str) -> Result<Box<str>, DecodeBackingRefusalV1> {
        if self.aborted.load(Ordering::SeqCst) {
            return Err(DecodeBackingRefusalV1);
        }
        Ok(source.into())
    }
    fn adopt_parser_text<E>(
        &self,
        source: &mut Option<String>,
        validate: impl FnOnce(&str) -> Result<(), E>,
    ) -> Result<String, ParserTextInputErrorV1<E>> {
        self.state
            .lock()
            .map_err(|_cause| DecodeBackingRefusalV1)?
            .begin_owned_input_v1()?;
        let result = (|| {
            let value = source.as_ref().ok_or(DecodeBackingRefusalV1)?;
            if self.aborted.load(Ordering::SeqCst)
                || !self
                    .text_rows
                    .lock()
                    .map_err(|_cause| DecodeBackingRefusalV1)?
                    .contains(&(value.as_ptr().addr(), value.capacity()))
            {
                return Err(ParserTextInputErrorV1::Admission(DecodeBackingRefusalV1));
            }
            validate(value).map_err(ParserTextInputErrorV1::Validation)?;
            source.take().ok_or_else(|| DecodeBackingRefusalV1.into())
        })();
        if let Ok(mut state) = self.state.lock() {
            state.finish_owned_input_v1(result.is_ok());
        }
        result
    }
    fn copy_boxed_parser_text<E>(
        &self,
        source: &mut Option<String>,
        validate: impl FnOnce(&str) -> Result<(), E>,
    ) -> Result<Box<str>, ParserTextInputErrorV1<E>> {
        self.state
            .lock()
            .map_err(|_cause| DecodeBackingRefusalV1)?
            .begin_owned_input_v1()?;
        let result = (|| {
            let value = source.as_ref().ok_or(DecodeBackingRefusalV1)?;
            if self.aborted.load(Ordering::SeqCst)
                || !self
                    .text_rows
                    .lock()
                    .map_err(|_cause| DecodeBackingRefusalV1)?
                    .contains(&(value.as_ptr().addr(), value.capacity()))
            {
                return Err(ParserTextInputErrorV1::Admission(DecodeBackingRefusalV1));
            }
            validate(value).map_err(ParserTextInputErrorV1::Validation)?;
            let boxed: Box<str> = value.as_str().into();
            let _consumed = source.take().ok_or(DecodeBackingRefusalV1)?;
            Ok(boxed)
        })();
        if let Ok(mut state) = self.state.lock() {
            state.finish_owned_input_v1(result.is_ok());
        }
        result
    }
    fn verify_parser_owned_bytes(
        &self,
        _source: &[u8],
        _capacity: usize,
    ) -> Result<(), DecodeBackingRefusalV1> {
        Err(DecodeBackingRefusalV1)
    }
}

// This fixture tests SDK dispatch and owner retention, not Original identity
// allocation. S exercises the production admitted identity seed.
impl SourceIdentityDecodeAdmissionV1 for DecodeOwner {
    fn decode_repo_id_v1<'de, D: serde::Deserializer<'de>>(
        &self,
        decoder: D,
    ) -> Result<quanta_index_contract::RepoId, D::Error> {
        quanta_index_contract::RepoId::deserialize(decoder)
    }

    fn decode_revision_id_v1<'de, D: serde::Deserializer<'de>>(
        &self,
        decoder: D,
    ) -> Result<quanta_index_contract::RevisionId, D::Error> {
        quanta_index_contract::RevisionId::deserialize(decoder)
    }
}

struct LeaseOwner(Arc<AtomicU64>);
impl MemoryLeaseOwner for LeaseOwner {
    fn release(&self, bytes: u64) {
        let _previous = self.0.fetch_sub(bytes, Ordering::SeqCst);
    }
}
struct Scope(Arc<AtomicU64>);
impl MemoryScopePort for Scope {
    fn reserve(&self, bytes: u64, lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
        if lifetime != MemoryLifetime::Retained {
            return Err(IndexError::ResourceLimited);
        }
        let _previous = self.0.fetch_add(bytes, Ordering::SeqCst);
        Ok(MemoryLease::new(
            bytes,
            Arc::new(LeaseOwner(Arc::clone(&self.0))),
        ))
    }
}

fn history_batch() -> HistoryIngestBatch {
    let mut batch = HistoryIngestBatch {
        repo_id: repo_id(),
        revision_id: revision_id(),
        generation: ManifestGeneration::new(9),
        manifest_digest: None,
        batch_digest: String::new(),
        commits: Vec::new(),
        refs: Vec::new(),
        tags: Vec::new(),
        diff_hunks: Vec::new(),
    };
    crate::stamp_batch_digest_v1(&mut batch).expect("stamp history fixture");
    batch
}

#[test]
fn repomap_borrowed_admitted_uds_refuses_before_send_and_retains_both_receipts() {
    use quanta_index_contract::{
        BorrowedRepoMapPublishBundleRequestV3, QueryErrorRepair, RepairClass,
        RepoMapExactnessSummary, RepoMapGraphCoverage, RepoMapGraphCoverageClass,
        RepoMapItemIndexAvailability, RepoMapMutationAck, RepoMapMutationPhaseV2,
        RepoMapRedactionState, RepoMapSourceBundle, RepoMapTerminalReceiptV2,
        SearchPlaneControlIpcRequestEnvelope, SearchPlaneControlIpcResponse,
        SearchPlaneControlIpcResponseEnvelope, SearchPlaneErrorCodeV2,
        SearchPlaneIngestIpcRequestEnvelope, SearchPlaneIngestIpcResponse,
        SearchPlaneIngestIpcResponseEnvelope, SearchPlaneIpcError,
    };
    use std::io::Write;
    use std::os::unix::net::UnixListener;

    let bundle = RepoMapSourceBundle::new(
        repo_id(),
        revision_id(),
        ManifestGeneration::new(13),
        "manifest-repomap-13",
        "snapshot-repomap-13",
        2,
        "authority-repomap-13",
        RepoMapGraphCoverage {
            item_index_availability: RepoMapItemIndexAvailability::Available,
            graph_coverage_class: RepoMapGraphCoverageClass::Complete,
        },
        RepoMapExactnessSummary::Exact,
        RepoMapRedactionState::Unredacted,
    );
    let activation = quanta_index_contract::RepoMapActivateGenerationRequestV2::for_bundle(&bundle)
        .expect("source commitment");
    let request = BorrowedRepoMapPublishBundleRequestV3 {
        source_bundle_digest: &activation.source_bundle_digest,
        bundle: &bundle,
    };
    let directory = tempfile::tempdir().expect("socket dir");
    let ingest_socket = directory.path().join("repomap-ingest.sock");
    let control_socket = directory.path().join("repomap-control.sock");
    let ingest_listener = UnixListener::bind(&ingest_socket).expect("bind ingest");
    let control_listener = UnixListener::bind(&control_socket).expect("bind control");
    let make_receipt = |phase| RepoMapTerminalReceiptV2 {
        phase,
        mutation: RepoMapMutationAck {
            prior_candidate_commitment: None,
            new_candidate_commitment: format!("sha256:{}", "ab".repeat(32)),
            activation_epoch: 1,
            terminal_sequence: 1,
            replayed: false,
            repo_id: repo_id(),
            revision_id: revision_id(),
            manifest_generation: ManifestGeneration::new(13),
        },
        manifest_digest: bundle.manifest_digest.clone(),
        snapshot_id: bundle.snapshot_id.clone(),
        projection_version: bundle.projection_version,
        authority_digest: bundle.authority_digest.clone(),
        source_bundle_digest: activation.source_bundle_digest.clone(),
    };
    let publish_receipt = make_receipt(RepoMapMutationPhaseV2::Publish);
    let activation_receipt = make_receipt(RepoMapMutationPhaseV2::Activate);
    let ingest_peer = std::thread::spawn(move || {
        for case in 0..2 {
            let (mut stream, _) = ingest_listener.accept().expect("accept publish");
            let request: SearchPlaneIngestIpcRequestEnvelope =
                quanta_index_ipc::decode_request(&mut stream).expect("decode publish");
            assert!(matches!(
                request.payload,
                SearchPlaneIngestIpcRequest::PublishRepoMapBundleV2(_)
            ));
            let mut receipt = publish_receipt.clone();
            if case == 1 {
                receipt.manifest_digest = "foreign-manifest".to_string();
            }
            let reply = SearchPlaneIngestIpcResponseEnvelope {
                request_id: request.request_id,
                payload: SearchPlaneIngestIpcResponse::RepoMapTerminalReceiptV2(receipt),
            };
            stream
                .write_all(&quanta_index_ipc::encode_response(&reply).expect("encode publish"))
                .expect("write publish");
        }
    });
    let control_peer = std::thread::spawn(move || {
        for case in 0..7 {
            let (mut stream, _) = control_listener.accept().expect("accept activation");
            let request: SearchPlaneControlIpcRequestEnvelope =
                quanta_index_ipc::decode_request(&mut stream).expect("decode activation");
            assert!(matches!(
                request.payload,
                quanta_index_contract::SearchPlaneControlIpcRequest::RepoMapActivateV2(_)
            ));
            let mut receipt = activation_receipt.clone();
            if case == 2 {
                receipt.manifest_digest = "foreign-manifest".to_string();
            }
            if case == 3 {
                receipt.mutation.activation_epoch = 2;
            }
            let payload = if case == 6 {
                SearchPlaneControlIpcResponse::Error(SearchPlaneIpcError {
                    code: SearchPlaneErrorCodeV2::InvalidRequest,
                    message: "scripted RepoMap refusal".to_string(),
                    repair: Some(QueryErrorRepair {
                        class: RepairClass::Unsupported,
                        supported_alternatives: vec!["repomap".to_string()],
                        docs_anchor: Some("docs/repomap".to_string()),
                    }),
                })
            } else {
                SearchPlaneControlIpcResponse::RepoMapTerminalReceiptV2(receipt)
            };
            let reply = SearchPlaneControlIpcResponseEnvelope {
                request_id: request.request_id + u64::from(case == 1),
                payload,
            };
            let mut wire = quanta_index_ipc::encode_response(&reply).expect("encode activation");
            if case == 4 {
                let _last_byte = wire.pop().expect("nonempty response frame");
            } else if case == 5 {
                // Four-byte frame header stays intact; the CBOR body starts invalid.
                *wire.get_mut(4).expect("framed CBOR body") = 0xff;
            }
            stream.write_all(&wire).expect("write activation");
        }
    });
    let client = crate::QuantaIndexClientPayloadV1::from_socket_paths_v1(
        ingest_socket.clone(),
        Some(control_socket),
        Some(ingest_socket),
        quanta_index_ipc::ClientIoPolicy::default(),
    );
    let active = Arc::new(AtomicU64::new(0));
    let memory = MemoryScope::new(Arc::new(Scope(Arc::clone(&active))));
    let budget = RequestBudgetV1::for_duration(Duration::from_secs(2));
    let wrong = client.repomap().publish_borrowed_admitted_v3(
        BorrowedRepoMapPublishBundleRequestV3 {
            source_bundle_digest: "sha256:wrong",
            bundle: &bundle,
        },
        &budget,
        &memory,
        DecodeOwner::default(),
        &mut |_| Ok::<(), ()>(()),
    );
    assert!(matches!(
        wrong,
        Err(crate::RepoMapAdmittedPublishErrorV3::DigestMismatch)
    ));
    let refused = client.repomap().publish_borrowed_admitted_v3(
        request,
        &budget,
        &memory,
        DecodeOwner::default(),
        &mut |_| Err::<(), _>("work-refused"),
    );
    assert!(matches!(
        refused,
        Err(crate::RepoMapAdmittedPublishErrorV3::BeforeSendProgress(
            "work-refused"
        ))
    ));
    assert_eq!(active.load(Ordering::SeqCst), 0);
    let published = client
        .repomap()
        .publish_borrowed_admitted_v3(
            request,
            &budget,
            &memory,
            DecodeOwner::default(),
            &mut |_| Ok::<(), ()>(()),
        )
        .expect("publish admitted");
    assert_eq!(
        published
            .receipt()
            .expect("admitted RepoMap publish receipt")
            .phase,
        RepoMapMutationPhaseV2::Publish
    );
    let activated = client
        .repomap()
        .activate_borrowed_admitted_v3(
            &activation,
            &budget,
            &memory,
            DecodeOwner::default(),
            &mut |_| Ok::<(), ()>(()),
        )
        .expect("activate admitted");
    assert_eq!(
        activated
            .receipt()
            .expect("admitted RepoMap activation receipt")
            .phase,
        RepoMapMutationPhaseV2::Activate
    );
    assert!(active.load(Ordering::SeqCst) > 0);
    drop(published);
    assert!(active.load(Ordering::SeqCst) > 0);
    drop(activated);
    assert_eq!(active.load(Ordering::SeqCst), 0);
    for case in 1..7 {
        let outcome = client.repomap().activate_borrowed_admitted_v3(
            &activation,
            &budget,
            &memory,
            DecodeOwner::default(),
            &mut |_| Ok::<(), &'static str>(()),
        );
        match (case, outcome) {
            (1, Err(crate::RepoMapAdmittedPublishErrorV3::ResponseRequestIdMismatch))
            | (2..=4, Err(crate::RepoMapAdmittedPublishErrorV3::Sdk(_)))
            | (5, Err(crate::RepoMapAdmittedPublishErrorV3::Decode(_))) => {}
            (6, Err(error)) => {
                let crate::RepoMapAdmittedPublishErrorV3::RemoteActivate { decoded, .. } = &error
                else {
                    panic!("remote response must retain its Error variant");
                };
                let SearchPlaneControlIpcResponse::Error(remote) = &decoded.value().payload else {
                    panic!("remote response must retain its Error payload");
                };
                assert_eq!(remote.code, SearchPlaneErrorCodeV2::InvalidRequest);
                assert_eq!(remote.message, "scripted RepoMap refusal");
                assert_eq!(
                    remote
                        .repair
                        .as_ref()
                        .and_then(|repair| repair.docs_anchor.as_deref()),
                    Some("docs/repomap")
                );
                let displayed = error.to_string();
                assert!(displayed.contains("scripted RepoMap refusal"));
                assert!(displayed.contains("docs/repomap"));
                assert!(active.load(Ordering::SeqCst) > 0);
            }
            (_, _) => panic!("activation case {case} accepted an invalid response"),
        }
        assert_eq!(active.load(Ordering::SeqCst), 0);
    }
    let wrong_axes = client.repomap().publish_borrowed_admitted_v3(
        request,
        &budget,
        &memory,
        DecodeOwner::default(),
        &mut |_| Ok::<(), ()>(()),
    );
    assert!(matches!(
        wrong_axes,
        Err(crate::RepoMapAdmittedPublishErrorV3::Sdk(_))
    ));
    assert_eq!(active.load(Ordering::SeqCst), 0);
    ingest_peer.join().expect("publish peer");
    control_peer.join().expect("activation peer");
}

#[test]
fn admitted_auxiliary_public_sdk_uds_preserves_roots_and_refuses_wrong_replies() {
    use quanta_index_contract::{
        QueryErrorRepair, RepairClass, SearchPlaneIngestIpcResponseEnvelope, SearchPlaneIpcError,
    };
    use std::io::Write;
    use std::os::unix::net::UnixListener;

    let batch = history_batch();
    let directory = tempfile::tempdir().expect("socket dir");
    let socket = directory.path().join("auxiliary.sock");
    let listener = UnixListener::bind(&socket).expect("bind ingest");
    let peer = std::thread::spawn(move || {
        for case in 0..5 {
            let (mut stream, _) = listener.accept().expect("accept request");
            let request: SearchPlaneIngestIpcRequestEnvelope =
                quanta_index_ipc::decode_request(&mut stream).expect("decode request");
            let SearchPlaneIngestIpcRequest::PublishHistoryBatch(batch) = request.payload else {
                panic!("unexpected auxiliary request");
            };
            let mut receipt =
                BatchPublishReceipt::empty_for(batch.generation, None, batch.batch_digest);
            receipt.semantic_content = Some(SemanticContentRootsV1 {
                row_root_digest: format!("sha256:{}", "a".repeat(64)),
                membership_root_digest: format!("sha256:{}", "b".repeat(64)),
            });
            if case == 4 {
                receipt.generation = ManifestGeneration::new(99);
            }
            let payload = match case {
                2 => SearchPlaneIngestIpcResponse::DirtyReceipt(receipt),
                3 => SearchPlaneIngestIpcResponse::Error(SearchPlaneIpcError {
                    code: SearchPlaneErrorCodeV2::InvalidRequest,
                    message: "scripted refusal".to_string(),
                    repair: Some(QueryErrorRepair {
                        class: RepairClass::Unsupported,
                        supported_alternatives: vec!["history".to_string()],
                        docs_anchor: Some("docs/history".to_string()),
                    }),
                }),
                _ => SearchPlaneIngestIpcResponse::HistoryReceipt(receipt),
            };
            let reply = SearchPlaneIngestIpcResponseEnvelope {
                request_id: request.request_id + u64::from(case == 1),
                payload,
            };
            stream
                .write_all(&quanta_index_ipc::encode_response(&reply).expect("encode response"))
                .expect("write response");
        }
    });
    let client = crate::QuantaIndexClientPayloadV1::from_socket_paths_v1(
        socket.clone(),
        None,
        Some(socket),
        quanta_index_ipc::ClientIoPolicy::default(),
    );
    let active = Arc::new(AtomicU64::new(0));
    let memory = MemoryScope::new(Arc::new(Scope(Arc::clone(&active))));
    let budget = RequestBudgetV1::for_duration(Duration::from_secs(2));
    let owner = DecodeOwner::default();
    let mut work_total = 0_usize;
    let success = client
        .history()
        .publish_history_borrowed_admitted_v1(&batch, &budget, &memory, owner, &mut |bytes| {
            work_total += bytes;
            Ok::<(), ()>(())
        })
        .expect("admitted public History publish");
    assert!(success.receipt().semantic_content.is_some());
    assert!(active.load(Ordering::SeqCst) > 0);
    assert!(work_total > 0);
    drop(success);
    assert_eq!(active.load(Ordering::SeqCst), 0);
    for case in 1..5 {
        let result = client.history().publish_history_borrowed_admitted_v1(
            &batch,
            &budget,
            &memory,
            DecodeOwner::default(),
            &mut |_bytes| Ok::<(), ()>(()),
        );
        match (case, result) {
            (1, Err(crate::AuxiliaryAdmittedPublishErrorV1::ResponseRequestIdMismatch))
            | (2, Err(crate::AuxiliaryAdmittedPublishErrorV1::ResponseVariantMismatch))
            | (4, Err(crate::AuxiliaryAdmittedPublishErrorV1::ResponseCommitmentMismatch)) => {}
            (3, Err(crate::AuxiliaryAdmittedPublishErrorV1::Remote(remote))) => {
                assert_eq!(remote.code(), SearchPlaneErrorCodeV2::InvalidRequest);
                assert_eq!(remote.message(), "scripted refusal");
                assert_eq!(
                    remote
                        .repair()
                        .expect("retained repair")
                        .docs_anchor
                        .as_deref(),
                    Some("docs/history")
                );
                assert!(active.load(Ordering::SeqCst) > 0);
            }
            (_, _other) => panic!("case {case} accepted wrong reply"),
        }
        assert_eq!(active.load(Ordering::SeqCst), 0);
    }
    peer.join().expect("scripted auxiliary peer");
}

#[test]
fn all_auxiliary_admitted_public_routes_use_canonical_uds_receipts() {
    use quanta_index_contract::{
        BatchIngestMode, DirtyIngestBatch, FileContributorIngestBatch,
        RepoCommitRecencyIngestBatch, SearchPlaneIngestIpcResponseEnvelope, StructuralIngestBatch,
    };
    use std::io::Write;
    use std::os::unix::net::UnixListener;

    let history = history_batch();
    let generation = ManifestGeneration::new(9);
    let mut recency = RepoCommitRecencyIngestBatch {
        repo_id: repo_id(),
        revision_id: revision_id(),
        generation,
        batch_digest: String::new(),
        entries: Vec::new(),
    };
    let mut contributor = FileContributorIngestBatch {
        repo_id: repo_id(),
        revision_id: revision_id(),
        generation,
        batch_digest: String::new(),
        entries: Vec::new(),
    };
    let mut dirty = DirtyIngestBatch {
        repo_id: repo_id(),
        revision_id: revision_id(),
        generation,
        overlay_epoch_ms: 17,
        batch_digest: String::new(),
        entries: Vec::new(),
    };
    let mut structural = StructuralIngestBatch {
        repo_id: repo_id(),
        revision_id: revision_id(),
        generation,
        base_generation: None,
        manifest_digest: "manifest:structural".to_string(),
        batch_digest: String::new(),
        mode: BatchIngestMode::ReplaceGeneration,
        replace_scopes: Vec::new(),
        tombstone_scopes: Vec::new(),
        seal: true,
    };
    for stamp in [
        crate::stamp_batch_digest_v1(&mut recency),
        crate::stamp_batch_digest_v1(&mut contributor),
        crate::stamp_batch_digest_v1(&mut dirty),
        crate::stamp_batch_digest_v1(&mut structural),
    ] {
        stamp.expect("stamp auxiliary fixture");
    }
    let directory = tempfile::tempdir().expect("socket dir");
    let socket = directory.path().join("five-routes.sock");
    let listener = UnixListener::bind(&socket).expect("bind ingest");
    let peer = std::thread::spawn(move || {
        for expected_route in 0..5 {
            let (mut stream, _) = listener.accept().expect("accept auxiliary request");
            let request: SearchPlaneIngestIpcRequestEnvelope =
                quanta_index_ipc::decode_request(&mut stream).expect("decode canonical request");
            let (route, generation, digest) = match request.payload {
                SearchPlaneIngestIpcRequest::PublishHistoryBatch(batch) => {
                    (0, batch.generation, batch.batch_digest)
                }
                SearchPlaneIngestIpcRequest::PublishRepoCommitRecencyBatch(batch) => {
                    (1, batch.generation, batch.batch_digest)
                }
                SearchPlaneIngestIpcRequest::PublishFileContributorBatch(batch) => {
                    (2, batch.generation, batch.batch_digest)
                }
                SearchPlaneIngestIpcRequest::PublishDirtyBatch(batch) => {
                    (3, batch.generation, batch.batch_digest)
                }
                SearchPlaneIngestIpcRequest::PublishStructuralBatch(batch) => {
                    (4, batch.generation, batch.batch_digest)
                }
                other @ (SearchPlaneIngestIpcRequest::StageSourcePublication(_)
                | SearchPlaneIngestIpcRequest::PublishStagedSourcePublication(_)
                | SearchPlaneIngestIpcRequest::DiscardSourcePublicationUpload(_)
                | SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(_)
                | SearchPlaneIngestIpcRequest::PublishRepoTopicBatch(_)
                | SearchPlaneIngestIpcRequest::PublishFileOwnershipBatch(_)
                | SearchPlaneIngestIpcRequest::PublishRuntimeCatalogBatch(_)
                | SearchPlaneIngestIpcRequest::PublishRepoMapBundleV2(_)
                | SearchPlaneIngestIpcRequest::PublishRepoMetaBatch(_)
                | SearchPlaneIngestIpcRequest::PublishRepoDescriptionBatch(_)) => {
                    panic!("unexpected auxiliary request: {other:?}")
                }
            };
            assert_eq!(route, expected_route);
            let receipt = BatchPublishReceipt::empty_for(generation, None, digest);
            let payload = match route {
                0 => SearchPlaneIngestIpcResponse::HistoryReceipt(receipt),
                1 => SearchPlaneIngestIpcResponse::RepoCommitRecencyReceipt(receipt),
                2 => SearchPlaneIngestIpcResponse::FileContributorReceipt(receipt),
                3 => SearchPlaneIngestIpcResponse::DirtyReceipt(receipt),
                4 => SearchPlaneIngestIpcResponse::StructuralReceipt(receipt),
                other => panic!("unexpected auxiliary route: {other}"),
            };
            let reply = SearchPlaneIngestIpcResponseEnvelope {
                request_id: request.request_id,
                payload,
            };
            stream
                .write_all(&quanta_index_ipc::encode_response(&reply).expect("encode receipt"))
                .expect("write receipt");
        }
    });
    let client = crate::QuantaIndexClientPayloadV1::from_socket_paths_v1(
        socket.clone(),
        None,
        Some(socket),
        quanta_index_ipc::ClientIoPolicy::default(),
    );
    let active = Arc::new(AtomicU64::new(0));
    let memory = MemoryScope::new(Arc::new(Scope(Arc::clone(&active))));
    let budget = RequestBudgetV1::for_duration(Duration::from_secs(5));
    let mut work = |_bytes| Ok::<(), ()>(());
    let receipts = [
        client.history().publish_history_borrowed_admitted_v1(
            &history,
            &budget,
            &memory,
            DecodeOwner::default(),
            &mut work,
        ),
        client
            .history()
            .publish_repo_commit_recency_borrowed_admitted_v1(
                &recency,
                &budget,
                &memory,
                DecodeOwner::default(),
                &mut work,
            ),
        client
            .history()
            .publish_file_contributor_borrowed_admitted_v1(
                &contributor,
                &budget,
                &memory,
                DecodeOwner::default(),
                &mut work,
            ),
        client.runtime().publish_dirty_borrowed_admitted_v1(
            &dirty,
            &budget,
            &memory,
            DecodeOwner::default(),
            &mut work,
        ),
        client.structural().publish_structural_borrowed_admitted_v1(
            &structural,
            &budget,
            &memory,
            DecodeOwner::default(),
            &mut work,
        ),
    ];
    for (route, result) in receipts.into_iter().enumerate() {
        let receipt = match result {
            Ok(receipt) => receipt,
            Err(error) => panic!("route {route} failed: {error:?}"),
        };
        assert_eq!(receipt.receipt().generation, generation);
        assert!(active.load(Ordering::SeqCst) > 0);
        drop(receipt);
    }
    assert_eq!(active.load(Ordering::SeqCst), 0);
    peer.join().expect("scripted five-route peer");
}
