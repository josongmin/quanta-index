//! Blocking, opt-in native-index UDS client. Async callers must provide their
//! own bounded blocking executor; no synchronous socket I/O occurs in a poll.

use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use quanta_index_contract::{
    index::{
        CatalogView, IndexInstanceId, IndexProfile, IngressRef, MAX_SOURCE_PATH_BYTES, MapRoot,
        ObjectRef, OperationKey, OperationReply, OperationStatus, OwnerMutation, OwnerMutationSet,
        ProjectionCertificate, ProjectionOwnerContribution, SourceInput, SubmitOutcome,
        UpdateRequest, ViewSelection,
    },
    native_protocol::{
        NativeIndexErrorCode, NativeIndexRequest, NativeIndexRequestEnvelope, NativeIndexResponse,
        NativeIndexResponseEnvelope, NativeIndexWireError, NativeLiteralPage, NativePathPage,
    },
};
use quanta_index_ipc::{ClientIoPolicy, RequestBudgetV1, send_request};

#[derive(Debug, thiserror::Error)]
pub enum NativeUdsError {
    #[error("native index response is malformed or mismatched: {0}")]
    Protocol(String),
    #[error("native index refused: {0:?}")]
    Refused(NativeIndexWireError),
    #[error("native index transport failed: {0}")]
    Transport(String),
    #[error("native write outcome unknown; reconcile original key {0:?}")]
    Unknown(OperationKey),
}

pub struct NativeUdsClient {
    socket: PathBuf,
    next_request_id: AtomicU64,
}

impl NativeUdsClient {
    #[must_use]
    pub fn new(socket: PathBuf) -> Self {
        Self {
            socket,
            next_request_id: AtomicU64::new(1),
        }
    }

    pub fn request(
        &self,
        payload: NativeIndexRequest,
        budget: &RequestBudgetV1,
    ) -> Result<NativeIndexResponse, NativeUdsError> {
        if budget.is_cancelled() {
            return Err(NativeUdsError::Refused(NativeIndexWireError {
                code: NativeIndexErrorCode::Cancelled,
                message: "request cancelled before native UDS send".into(),
            }));
        }
        let request_id = self
            .next_request_id
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |id| id.checked_add(1))
            .map_err(|_exhausted_id| NativeUdsError::Protocol("request id exhausted".into()))?;
        let envelope = NativeIndexRequestEnvelope {
            request_id,
            payload,
        };
        let io = ClientIoPolicy::try_with_deadline(budget.deadline())
            .map_err(|error| NativeUdsError::Transport(error.to_string()))?;
        let reply: NativeIndexResponseEnvelope = send_request(&self.socket, &envelope, io)
            .map_err(|error| NativeUdsError::Transport(error.to_string()))?;
        if reply.request_id != request_id {
            return Err(NativeUdsError::Protocol(
                "response request id differs".into(),
            ));
        }
        match reply.payload {
            NativeIndexResponse::Error(error) => Err(NativeUdsError::Refused(error)),
            response @ (NativeIndexResponse::Created(_)
            | NativeIndexResponse::InputOpened(_)
            | NativeIndexResponse::Staged(_)
            | NativeIndexResponse::StagedSources(_)
            | NativeIndexResponse::MutationPageStaged(_)
            | NativeIndexResponse::MutationsStaged(_)
            | NativeIndexResponse::InputDiscarded(_)
            | NativeIndexResponse::LiteralPage(_)
            | NativeIndexResponse::PathPage(_)
            | NativeIndexResponse::Submitted(_)
            | NativeIndexResponse::View(_)
            | NativeIndexResponse::Status(_)
            | NativeIndexResponse::ReadyWork(_)
            | NativeIndexResponse::Claimed(_)
            | NativeIndexResponse::Renewed(_)
            | NativeIndexResponse::WorkStatus(_)) => Ok(response),
        }
    }

    fn write_stage(
        &self,
        key: OperationKey,
        payload: NativeIndexRequest,
        budget: &RequestBudgetV1,
    ) -> Result<NativeIndexResponse, NativeUdsError> {
        match self.request(payload, budget) {
            Ok(response) => Ok(response),
            Err(NativeUdsError::Refused(error))
                if error.code == NativeIndexErrorCode::ResponseTooLarge =>
            {
                Err(NativeUdsError::Unknown(key))
            }
            Err(error @ NativeUdsError::Refused(_)) => Err(error),
            Err(
                NativeUdsError::Transport(_)
                | NativeUdsError::Protocol(_)
                | NativeUdsError::Unknown(_),
            ) => Err(NativeUdsError::Unknown(key)),
        }
    }

    /// Opens a durable operation-scoped input stage. An ambiguous ACK keeps
    /// the same operation identity; callers must not allocate a replacement.
    pub fn begin_input(
        &self,
        key: OperationKey,
        budget: &RequestBudgetV1,
    ) -> Result<IngressRef, NativeUdsError> {
        let response = self.write_stage(key, NativeIndexRequest::BeginInput(key), budget)?;
        let NativeIndexResponse::InputOpened(ingress) = response else {
            return Err(NativeUdsError::Unknown(key));
        };
        if ingress.operation != key {
            return Err(NativeUdsError::Unknown(key));
        }
        Ok(ingress)
    }

    pub fn stage_source(
        &self,
        ingress: IngressRef,
        path: String,
        bytes: &[u8],
        budget: &RequestBudgetV1,
    ) -> Result<ObjectRef, NativeUdsError> {
        staged_reference(
            &self.write_stage(
                ingress.operation,
                NativeIndexRequest::StageSource {
                    ingress,
                    path,
                    bytes: bytes.to_vec(),
                },
                budget,
            )?,
            ingress.operation,
        )
    }

    /// Uploads one bounded child-first source batch without a per-file UDS fallback.
    pub fn stage_sources(
        &self,
        ingress: IngressRef,
        inputs: Vec<SourceInput>,
        budget: &RequestBudgetV1,
    ) -> Result<Vec<ObjectRef>, NativeUdsError> {
        if inputs.is_empty() || inputs.len() > 32 {
            return Err(NativeUdsError::Protocol(
                "source batch count exceeds native bounds".into(),
            ));
        }
        let mut bytes = 0usize;
        for input in &inputs {
            if input.path.len() > MAX_SOURCE_PATH_BYTES {
                return Err(NativeUdsError::Protocol(
                    "source batch path exceeds native bounds".into(),
                ));
            }
            bytes = bytes
                .checked_add(input.path.len())
                .and_then(|total| total.checked_add(input.bytes.len()))
                .and_then(|total| total.checked_add(256))
                .ok_or_else(|| NativeUdsError::Protocol("source batch size overflow".into()))?;
            if bytes > 8 * 1024 * 1024 {
                return Err(NativeUdsError::Protocol(
                    "source batch size exceeds native bounds".into(),
                ));
            }
        }
        let count = inputs.len();
        let response = self.write_stage(
            ingress.operation,
            NativeIndexRequest::StageSources { ingress, inputs },
            budget,
        )?;
        let NativeIndexResponse::StagedSources(references) = response else {
            return Err(NativeUdsError::Unknown(ingress.operation));
        };
        if references.len() != count {
            return Err(NativeUdsError::Unknown(ingress.operation));
        }
        Ok(references)
    }

    pub fn stage_profile(
        &self,
        ingress: IngressRef,
        profile: IndexProfile,
        budget: &RequestBudgetV1,
    ) -> Result<ObjectRef, NativeUdsError> {
        staged_reference(
            &self.write_stage(
                ingress.operation,
                NativeIndexRequest::StageProfile { ingress, profile },
                budget,
            )?,
            ingress.operation,
        )
    }

    pub fn stage_projection_output(
        &self,
        ingress: IngressRef,
        bytes: &[u8],
        budget: &RequestBudgetV1,
    ) -> Result<ObjectRef, NativeUdsError> {
        staged_reference(
            &self.write_stage(
                ingress.operation,
                NativeIndexRequest::StageProjectionOutput {
                    ingress,
                    bytes: bytes.to_vec(),
                },
                budget,
            )?,
            ingress.operation,
        )
    }

    pub fn stage_projection_owner(
        &self,
        ingress: IngressRef,
        contribution: ProjectionOwnerContribution,
        budget: &RequestBudgetV1,
    ) -> Result<ObjectRef, NativeUdsError> {
        staged_reference(
            &self.write_stage(
                ingress.operation,
                NativeIndexRequest::StageProjectionOwner {
                    ingress,
                    contribution,
                },
                budget,
            )?,
            ingress.operation,
        )
    }

    pub fn stage_projection_certificate(
        &self,
        ingress: IngressRef,
        certificate: ProjectionCertificate,
        budget: &RequestBudgetV1,
    ) -> Result<ObjectRef, NativeUdsError> {
        staged_reference(
            &self.write_stage(
                ingress.operation,
                NativeIndexRequest::StageProjectionCertificate {
                    ingress,
                    certificate,
                },
                budget,
            )?,
            ingress.operation,
        )
    }

    /// Streams at most 128 owner mutations per UDS request. The caller owns
    /// source capture and source-body sizing; no corpus-wide vector is built.
    pub fn stage_mutations<I>(
        &self,
        ingress: IngressRef,
        mutations: I,
        budget: &RequestBudgetV1,
    ) -> Result<OwnerMutationSet, NativeUdsError>
    where
        I: IntoIterator<Item = OwnerMutation>,
    {
        let mut input = mutations.into_iter();
        let mut root = MapRoot(None);
        let mut count = 0u64;
        loop {
            let page: Vec<_> = input.by_ref().take(128).collect();
            if page.is_empty() {
                break;
            }
            count = count
                .checked_add(u64::try_from(page.len()).map_err(|_error| {
                    NativeUdsError::Protocol("mutation page length overflow".into())
                })?)
                .ok_or_else(|| NativeUdsError::Protocol("mutation count overflow".into()))?;
            let response = self.write_stage(
                ingress.operation,
                NativeIndexRequest::StageMutationPage {
                    ingress,
                    base: root,
                    mutations: page,
                },
                budget,
            )?;
            let NativeIndexResponse::MutationPageStaged(next) = response else {
                return Err(NativeUdsError::Unknown(ingress.operation));
            };
            root = next;
        }
        let response = self.write_stage(
            ingress.operation,
            NativeIndexRequest::SealMutations {
                ingress,
                root,
                count,
            },
            budget,
        )?;
        let NativeIndexResponse::MutationsStaged(set) = response else {
            return Err(NativeUdsError::Unknown(ingress.operation));
        };
        if set.root != root || set.count != count {
            return Err(NativeUdsError::Unknown(ingress.operation));
        }
        Ok(set)
    }

    pub fn discard_input(
        &self,
        ingress: IngressRef,
        budget: &RequestBudgetV1,
    ) -> Result<(), NativeUdsError> {
        let response = self.write_stage(
            ingress.operation,
            NativeIndexRequest::DiscardInput(ingress),
            budget,
        )?;
        let NativeIndexResponse::InputDiscarded(ack) = response else {
            return Err(NativeUdsError::Unknown(ingress.operation));
        };
        if ack != ingress {
            return Err(NativeUdsError::Unknown(ingress.operation));
        }
        Ok(())
    }

    /// On transport loss the original operation may already be admitted.
    /// The caller must reconcile this exact key with `status`.
    pub fn submit(
        &self,
        request: UpdateRequest,
        budget: &RequestBudgetV1,
    ) -> Result<SubmitOutcome, NativeUdsError> {
        let key = request.operation;
        self.submit_like(NativeIndexRequest::Submit(request), key, budget)
    }

    /// Replays only the admitted immutable request associated with this key.
    /// An uncertain response must be reconciled with `status` or retried with
    /// the same key; constructing a new operation changes its identity.
    pub fn resume(
        &self,
        key: OperationKey,
        budget: &RequestBudgetV1,
    ) -> Result<SubmitOutcome, NativeUdsError> {
        self.submit_like(NativeIndexRequest::Resume(key), key, budget)
    }

    fn submit_like(
        &self,
        request: NativeIndexRequest,
        key: OperationKey,
        budget: &RequestBudgetV1,
    ) -> Result<SubmitOutcome, NativeUdsError> {
        match self.request(request, budget) {
            Ok(NativeIndexResponse::Submitted(reply)) => validate_submit_reply(key, reply),
            Err(NativeUdsError::Refused(error))
                if error.code == NativeIndexErrorCode::ResponseTooLarge =>
            {
                Err(NativeUdsError::Unknown(key))
            }
            Err(NativeUdsError::Refused(error)) => Err(NativeUdsError::Refused(error)),
            Ok(_) | Err(_) => Err(NativeUdsError::Unknown(key)),
        }
    }

    pub fn status(
        &self,
        key: OperationKey,
        budget: &RequestBudgetV1,
    ) -> Result<OperationStatus, NativeUdsError> {
        match self.request(NativeIndexRequest::Status(key), budget)? {
            NativeIndexResponse::Status(reply) => validate_status_reply(key, reply),
            NativeIndexResponse::Created(_)
            | NativeIndexResponse::InputOpened(_)
            | NativeIndexResponse::Staged(_)
            | NativeIndexResponse::StagedSources(_)
            | NativeIndexResponse::MutationPageStaged(_)
            | NativeIndexResponse::MutationsStaged(_)
            | NativeIndexResponse::InputDiscarded(_)
            | NativeIndexResponse::LiteralPage(_)
            | NativeIndexResponse::PathPage(_)
            | NativeIndexResponse::Submitted(_)
            | NativeIndexResponse::View(_)
            | NativeIndexResponse::ReadyWork(_)
            | NativeIndexResponse::Claimed(_)
            | NativeIndexResponse::Renewed(_)
            | NativeIndexResponse::WorkStatus(_)
            | NativeIndexResponse::Error(_) => {
                Err(NativeUdsError::Protocol("unexpected status reply".into()))
            }
        }
    }

    pub fn read_view(
        &self,
        selection: ViewSelection,
        budget: &RequestBudgetV1,
    ) -> Result<CatalogView, NativeUdsError> {
        let NativeIndexResponse::View(view) =
            self.request(NativeIndexRequest::ReadView(selection), budget)?
        else {
            return Err(NativeUdsError::Protocol("unexpected view reply".into()));
        };
        let digest = view
            .identity_digest()
            .map_err(|error| NativeUdsError::Protocol(format!("view digest: {error}")))?;
        if view.identity.digest != digest {
            return Err(NativeUdsError::Protocol(
                "view identity digest mismatch".into(),
            ));
        }
        let selected = match selection {
            ViewSelection::Exact(reference) => view.identity == reference,
            ViewSelection::Latest { workspace, epoch } => {
                view.identity.workspace == workspace && view.identity.epoch == epoch
            }
        };
        if !selected {
            return Err(NativeUdsError::Protocol("view selection mismatch".into()));
        }
        Ok(view)
    }

    pub fn search_literal(
        &self,
        selection: ViewSelection,
        instance: IndexInstanceId,
        literal: &[u8],
        after: Option<&[u8]>,
        limit: u32,
        budget: &RequestBudgetV1,
    ) -> Result<NativeLiteralPage, NativeUdsError> {
        if literal.is_empty()
            || literal.len() > 4096
            || after.is_some_and(|cursor| cursor.len() != 32)
            || !(1..=128).contains(&limit)
        {
            return Err(NativeUdsError::Protocol(
                "literal query exceeds native request bounds".into(),
            ));
        }
        let page_limit = usize::try_from(limit)
            .map_err(|_conversion_error| NativeUdsError::Protocol("page limit overflow".into()))?;
        let NativeIndexResponse::LiteralPage(page) = self.request(
            NativeIndexRequest::SearchLiteral {
                selection,
                instance,
                literal: literal.to_vec(),
                after: after.map(<[u8]>::to_vec),
                limit,
            },
            budget,
        )?
        else {
            return Err(NativeUdsError::Protocol(
                "unexpected literal query reply".into(),
            ));
        };
        if !selection_matches(selection, page.view) || page.hits.len() > page_limit {
            return Err(NativeUdsError::Protocol(
                "literal query page binding or limit mismatch".into(),
            ));
        }
        Ok(page)
    }

    pub fn query_paths(
        &self,
        selection: ViewSelection,
        instance: IndexInstanceId,
        prefix: &str,
        after: Option<&str>,
        limit: u32,
        budget: &RequestBudgetV1,
    ) -> Result<NativePathPage, NativeUdsError> {
        if prefix.len() > MAX_SOURCE_PATH_BYTES
            || after.is_some_and(|cursor| cursor.len() > MAX_SOURCE_PATH_BYTES)
            || !(1..=128).contains(&limit)
        {
            return Err(NativeUdsError::Protocol(
                "path query exceeds native request bounds".into(),
            ));
        }
        let page_limit = usize::try_from(limit)
            .map_err(|_conversion_error| NativeUdsError::Protocol("page limit overflow".into()))?;
        let NativeIndexResponse::PathPage(page) = self.request(
            NativeIndexRequest::QueryPaths {
                selection,
                instance,
                prefix: prefix.into(),
                after: after.map(str::to_owned),
                limit,
            },
            budget,
        )?
        else {
            return Err(NativeUdsError::Protocol(
                "unexpected path query reply".into(),
            ));
        };
        if !selection_matches(selection, page.view) || page.entries.len() > page_limit {
            return Err(NativeUdsError::Protocol(
                "path query page binding or limit mismatch".into(),
            ));
        }
        Ok(page)
    }
}

fn staged_reference(
    response: &NativeIndexResponse,
    key: OperationKey,
) -> Result<ObjectRef, NativeUdsError> {
    let NativeIndexResponse::Staged(reference) = response else {
        return Err(NativeUdsError::Unknown(key));
    };
    Ok(*reference)
}

fn selection_matches(
    selection: ViewSelection,
    view: quanta_index_contract::index::ViewRef,
) -> bool {
    match selection {
        ViewSelection::Exact(expected) => expected == view,
        ViewSelection::Latest { workspace, epoch } => {
            view.workspace == workspace && view.epoch == epoch
        }
    }
}

fn validate_submit_reply(
    key: OperationKey,
    reply: OperationReply<SubmitOutcome>,
) -> Result<SubmitOutcome, NativeUdsError> {
    if reply.operation != key {
        return Err(NativeUdsError::Unknown(key));
    }
    let outcome = reply.payload;
    let bound = match outcome {
        SubmitOutcome::Accepted(attempt) => attempt.operation == key,
        SubmitOutcome::Unknown(operation) => operation == key,
        SubmitOutcome::Terminal(OperationStatus::Published(view)) => {
            view.workspace == key.workspace && view.epoch == key.epoch
        }
        SubmitOutcome::Terminal(OperationStatus::Refused(_)) => true,
        SubmitOutcome::Terminal(OperationStatus::Pending(_) | OperationStatus::Unknown) => false,
    };
    if bound {
        Ok(outcome)
    } else {
        Err(NativeUdsError::Unknown(key))
    }
}

fn validate_status_reply(
    key: OperationKey,
    reply: OperationReply<OperationStatus>,
) -> Result<OperationStatus, NativeUdsError> {
    if reply.operation != key {
        return Err(NativeUdsError::Protocol("status operation mismatch".into()));
    }
    let status = reply.payload;
    let bound = match status {
        OperationStatus::Pending(attempt) => attempt.operation == key,
        OperationStatus::Published(view) => {
            view.workspace == key.workspace && view.epoch == key.epoch
        }
        OperationStatus::Unknown | OperationStatus::Refused(_) => true,
    };
    if bound {
        Ok(status)
    } else {
        Err(NativeUdsError::Protocol("status operation mismatch".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_contract::index::{
        CollectionId, Digest, ProducerId, SlotId, StreamId, TenantId, WorkspaceKey, WorktreeId,
    };
    use std::{
        io::Write,
        os::unix::net::UnixListener,
        time::{Duration, Instant},
    };

    fn scripted_reply<T>(
        response: NativeIndexResponse,
        call: impl FnOnce(&NativeUdsClient, &RequestBudgetV1) -> T,
    ) -> T {
        let directory = tempfile::tempdir().expect("socket directory");
        let socket = directory.path().join("native.sock");
        let listener = UnixListener::bind(&socket).expect("bind scripted native socket");
        let peer = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("native client connection");
            let request: NativeIndexRequestEnvelope =
                quanta_index_ipc::decode_request(&mut stream).expect("native request");
            let reply = NativeIndexResponseEnvelope {
                request_id: request.request_id,
                payload: response,
            };
            stream
                .write_all(&quanta_index_ipc::encode_response(&reply).expect("encode reply"))
                .expect("send native reply");
        });
        let client = NativeUdsClient::new(socket);
        let budget = RequestBudgetV1::until(Instant::now() + Duration::from_secs(2));
        let result = call(&client, &budget);
        peer.join().expect("scripted native peer");
        result
    }

    fn key(sequence: u64) -> OperationKey {
        OperationKey {
            workspace: WorkspaceKey {
                tenant: TenantId([1; 16]),
                collection: CollectionId([2; 16]),
                worktree: WorktreeId([3; 16]),
                slot: SlotId([4; 16]),
            },
            epoch: 1,
            producer: ProducerId([5; 16]),
            stream: StreamId([6; 16]),
            stream_epoch: 1,
            sequence,
        }
    }

    #[test]
    fn original_operation_binds_even_refused_and_unknown_replies() {
        assert!(matches!(
            validate_submit_reply(key(1), OperationReply {
                operation: key(2), payload: SubmitOutcome::Unknown(key(1)),
            }),
            Err(NativeUdsError::Unknown(operation)) if operation == key(1)
        ));
        assert!(matches!(
            validate_status_reply(
                key(1),
                OperationReply {
                    operation: key(2),
                    payload: OperationStatus::Unknown,
                }
            ),
            Err(NativeUdsError::Protocol(_))
        ));
    }

    #[test]
    fn cancelled_before_send_is_a_typed_pre_admission_refusal() {
        let client = NativeUdsClient::new(PathBuf::from("/nonexistent/native-index.sock"));
        let budget =
            RequestBudgetV1::until(std::time::Instant::now() + std::time::Duration::from_secs(1));
        budget.cancel_handle().cancel();
        assert!(matches!(
            client.resume(key(1), &budget),
            Err(NativeUdsError::Refused(NativeIndexWireError {
                code: NativeIndexErrorCode::Cancelled,
                ..
            }))
        ));
    }

    #[test]
    fn stage_replies_keep_original_operation_on_wrong_ack_or_response_kind() {
        let original = key(1);
        let ingress = IngressRef {
            operation: original,
            fence: 7,
        };
        let wrong_operation = IngressRef {
            operation: key(2),
            fence: 7,
        };
        assert!(matches!(
            scripted_reply(NativeIndexResponse::InputOpened(wrong_operation), |client, budget| client.begin_input(original, budget)),
            Err(NativeUdsError::Unknown(operation)) if operation == original
        ));
        assert!(matches!(
            scripted_reply(NativeIndexResponse::Staged(ObjectRef(Digest([1; 32]))), |client, budget| client.begin_input(original, budget)),
            Err(NativeUdsError::Unknown(operation)) if operation == original
        ));
        assert!(matches!(
            scripted_reply(NativeIndexResponse::InputDiscarded(IngressRef { fence: 8, ..ingress }), |client, budget| client.discard_input(ingress, budget)),
            Err(NativeUdsError::Unknown(operation)) if operation == original
        ));
    }

    #[test]
    fn batch_cardinality_and_seal_root_count_reconcile_original_operation() {
        let original = key(3);
        let ingress = IngressRef {
            operation: original,
            fence: 9,
        };
        assert!(matches!(
            scripted_reply(NativeIndexResponse::StagedSources(Vec::new()), |client, budget| client.stage_sources(
                ingress,
                vec![SourceInput { path: "a.rs".into(), bytes: b"source".to_vec() }],
                budget,
            )),
            Err(NativeUdsError::Unknown(operation)) if operation == original
        ));
        for invalid in [
            OwnerMutationSet {
                root: MapRoot(Some(ObjectRef(Digest([2; 32])))),
                count: 0,
                digest: Digest([3; 32]),
            },
            OwnerMutationSet {
                root: MapRoot(None),
                count: 1,
                digest: Digest([3; 32]),
            },
        ] {
            assert!(matches!(
                scripted_reply(NativeIndexResponse::MutationsStaged(invalid), |client, budget| client.stage_mutations(ingress, std::iter::empty(), budget)),
                Err(NativeUdsError::Unknown(operation)) if operation == original
            ));
        }
    }
}
