//! Root observations do not grant publication, EX or Original authority.
use super::*;
use quanta_index_contract::{RepoId, RevisionId};

/// One exact admitted reply and its same-connection kernel peer observation.
/// Both raw and decoded grants remain alive throughout every scoped borrow.
/// The installed provider must independently authenticate this peer process
/// image and expected root; returned nonce/device/inode alone are not proof.
#[derive(Debug)]
pub struct AdmittedNativeRootScopedViewReplyV1 {
    // Retire process/FD custody before the reply's raw and decoded grants.
    peer: quanta_index_ipc::KernelConnectedPeerProcessV1,
    reply: AdmittedNativePublicationReply,
}

impl AdmittedNativeRootScopedViewReplyV1 {
    #[must_use]
    pub fn response(&self) -> &NativeIndexResponse {
        self.reply.response()
    }

    #[must_use]
    pub fn peer_credentials(&self) -> &quanta_index_ipc::PeerCredentials {
        self.peer.credentials_v1()
    }

    #[must_use]
    pub fn peer_process_v1(&self) -> &quanta_index_ipc::KernelConnectedPeerProcessV1 {
        &self.peer
    }
}

impl<P: AsRef<Path>> NativeUdsClient<P> {
    /// Resolve a preserved RepoMap pair to the exact pinned view, member and
    /// instance selected by the installed BOOT route.
    pub fn resolve_protected_repomap_head_v1(
        &self,
        repo_id: RepoId,
        revision_id: RevisionId,
        budget: &RequestBudgetV1,
    ) -> Result<
        quanta_index_contract::native_protocol::NativeProtectedRepoMapResolutionV1,
        NativeUdsError,
    > {
        read_session::admitted_budget_checkpoint(budget)?;
        let NativeIndexResponse::ProtectedRepoMapHead(resolved) = self.request(
            NativeIndexRequest::ResolveProtectedRepoMapHead {
                repo_id,
                revision_id,
            },
            budget,
        )?
        else {
            return Err(NativeUdsError::Protocol(
                "unexpected protected RepoMap head reply".into(),
            ));
        };
        validate_repomap_reply(&resolved)?;
        read_session::admitted_budget_checkpoint(budget)?;
        Ok(resolved)
    }

    /// Resolve a selected, protected Source revision to its exact view. The
    /// server derives BOOT route custody; callers cannot supply an object ref.
    pub fn resolve_protected_source_head_v1(
        &self,
        repo_id: RepoId,
        revision_id: RevisionId,
        budget: &RequestBudgetV1,
    ) -> Result<NativeRootScopedViewV1, NativeUdsError> {
        read_session::admitted_budget_checkpoint(budget)?;
        let NativeIndexResponse::RootScopedView(observed) = self.request(
            NativeIndexRequest::ResolveProtectedSourceHead {
                repo_id,
                revision_id,
            },
            budget,
        )?
        else {
            return Err(NativeUdsError::Protocol(
                "unexpected protected Source head reply".into(),
            ));
        };
        validate_reply(ViewSelection::Exact(observed.view.identity), &observed)?;
        read_session::admitted_budget_checkpoint(budget)?;
        Ok(observed)
    }

    /// Pin the connected Q process on an empty root before any publication
    /// side effect. No workspace request or response is sent on this socket.
    /// The installed provider must authenticate its executable and root.
    pub fn capture_peer_process_admitted_v1(
        &self,
        budget: &RequestBudgetV1,
        memory: &MemoryScope,
    ) -> Result<quanta_index_ipc::KernelConnectedPeerProcessV1, NativeUdsError> {
        quanta_index_ipc::capture_connected_peer_process_with_budget_v1(
            self.socket.as_ref(),
            budget,
            memory,
        )
        .map_err(|error| match error {
            quanta_index_ipc::IpcError::PeerBindingRefused(cause) => {
                NativeUdsError::PeerBindingRefused(cause)
            }
            _ if budget.is_cancelled() => cancelled_error(),
            error => NativeUdsError::Transport(error.to_string()),
        })
    }

    /// Observe metadata only. This convenience result has no authenticated peer
    /// custody and cannot authorize an installed-provider or EX transition.
    pub fn observe_root_view(
        &self,
        selection: ViewSelection,
        budget: &RequestBudgetV1,
    ) -> Result<NativeRootScopedViewV1, NativeUdsError> {
        read_session::admitted_budget_checkpoint(budget)?;
        let NativeIndexResponse::RootScopedView(observed) =
            self.request(NativeIndexRequest::ObserveRootView(selection), budget)?
        else {
            return Err(NativeUdsError::Protocol(
                "unexpected root-scoped view reply".into(),
            ));
        };
        validate_reply(selection, &observed)?;
        read_session::admitted_budget_checkpoint(budget)?;
        Ok(observed)
    }

    /// Observe the exact reply and kernel UID/GID/PID on one admitted connection.
    /// PID-unavailable platforms refuse before writing; no self-reported PID or
    /// path reconnect can substitute the actual connected peer.
    pub fn observe_root_view_admitted(
        &self,
        selection: ViewSelection,
        budget: &RequestBudgetV1,
        memory: &MemoryScope,
    ) -> Result<AdmittedNativeRootScopedViewReplyV1, NativeUdsError> {
        if self.observed_query_peer_v1.is_some() {
            return Err(NativeUdsError::PeerBindingRefused(
                quanta_index_ipc::PeerBindingRefusalV1::UnsupportedRoute,
            ));
        }
        read_session::admitted_budget_checkpoint(budget)?;
        let request_id = allocate_request_id(&self.next_request_id)?;
        let envelope = NativeIndexRequestEnvelope {
            request_id,
            payload: NativeIndexRequest::ObserveRootView(selection),
        };
        let (body, peer) = quanta_index_ipc::send_request_body_with_budget_admitted_peer_v1(
            self.socket.as_ref(),
            &envelope,
            budget,
            memory,
        )
        .map_err(|error| match error {
            quanta_index_ipc::IpcError::PeerBindingRefused(cause) => {
                NativeUdsError::PeerBindingRefused(cause)
            }
            _ if budget.is_cancelled() => cancelled_error(),
            error => NativeUdsError::Transport(error.to_string()),
        })?;
        let reply =
            decode_admitted_publication_reply_v1(request_id, body, budget, memory, false, false)?;
        if !publication_response_matches_request(&envelope.payload, reply.response()) {
            return Err(NativeUdsError::Protocol(
                "unexpected root-scoped view reply".into(),
            ));
        }
        read_session::admitted_budget_checkpoint(budget)?;
        Ok(AdmittedNativeRootScopedViewReplyV1 { reply, peer })
    }
}

pub(super) fn validate_reply(
    selection: ViewSelection,
    observed: &NativeRootScopedViewV1,
) -> Result<(), NativeUdsError> {
    validate_catalog_view_selection(selection, &observed.view)?;
    if observed.root_nonce == Digest([0; 32]) {
        return Err(NativeUdsError::Protocol(
            "root-scoped observation nonce is zero".into(),
        ));
    }
    // Device/inode are observed DATA, not portable authorization heuristics.
    // Matching them against the installed provider remains that owner's work.
    Ok(())
}

pub(super) fn validate_repomap_reply(
    resolved: &quanta_index_contract::native_protocol::NativeProtectedRepoMapResolutionV1,
) -> Result<(), NativeUdsError> {
    validate_reply(
        ViewSelection::Exact(resolved.root.view.identity),
        &resolved.root,
    )?;
    if resolved.member_binding.0 == Digest([0; 32]) {
        return Err(NativeUdsError::Protocol(
            "protected RepoMap member is zero".into(),
        ));
    }
    Ok(())
}
