//! Ordinary SDK Native reads. Session authority is explicit and never renewed implicitly.

use quanta_index_contract::{
    SemanticQueryRequest, TextQueryRequest,
    index::{IndexInstanceId, InstanceSpec, ViewRef, ViewSelection},
    native_protocol::{
        NativeDslKindV1, NativeDslPageV1, NativeDslQueryV1, NativeDslScopeV1, NativeGraphQuery,
        NativeGraphResult, NativeHybridFileResult, NativeIndexRequest, NativeIndexResponse,
        NativeProjectionRead, NativeReadSessionProfile,
        NativeReadSessionSettlement, NativeReadSessionState, NativeReadSessionStatus,
        NativeSemanticResult, NativeTextCursor,
        NativeTextPage, NativeTextResult, NativeTextPathCursorV1, NativeTextPathPageV1,
        NativeTextPathResultV1,
    },
};
use quanta_index_ipc::{ClientIoPolicy, MemoryScope, RequestBudgetV1};
use std::path::Path;

use crate::native_uds::{AdmittedNativePublicationReply, NativeUdsClient, NativeUdsError};

#[derive(Debug, thiserror::Error)]
pub enum NativeQueryError {
    #[error(transparent)]
    Transport(#[from] NativeUdsError),
    #[error("Native query needs an explicit nonzero caller open key")]
    MissingOpenKey,
    #[error("Native query needs the captured session profile")]
    ProfileRequired,
    #[error("Native instance does not exactly match the captured profile")]
    InstanceMismatch,
    #[error("Native captured profile differs from the acknowledged session/view")]
    ProfileBindingMismatch,
    #[error("Native instance is ambiguous in the captured profile")]
    AmbiguousInstance,
    #[error("Native read session is not active")]
    InactiveSession,
    #[error("Native request deadline overflow")]
    DeadlineOverflow,
}

/// A borrowed query capability. It exposes no raw Native write/control sender.
#[derive(Clone, Copy)]
pub struct NativeQueryReader<'a> {
    pub(super) client: &'a NativeUdsClient,
    pub(super) selection: ViewSelection,
    pub(super) io_policy: ClientIoPolicy,
}

impl<'a> NativeQueryReader<'a> {
    fn budget(&self, original: &RequestBudgetV1) -> Result<RequestBudgetV1, NativeQueryError> {
        let deadline = match self.io_policy.absolute_deadline() {
            Some(deadline) => deadline,
            None => std::time::Instant::now()
                .checked_add(self.io_policy.request_timeout())
                .ok_or(NativeQueryError::DeadlineOverflow)?,
        };
        // Preserve the caller's cancellation, diagnostics and work ledgers.
        Ok(original.capped_at(deadline))
    }

    /// Opening and loading a profile are separate operations. If profile I/O
    /// fails, the caller still owns the acknowledged session for reconciliation.
    pub fn open(
        self,
        open_key: [u8; 32],
        budget: &RequestBudgetV1,
    ) -> Result<NativeQuerySession<'a>, NativeQueryError> {
        if open_key == [0; 32] {
            return Err(NativeQueryError::MissingOpenKey);
        }
        let status =
            self.client
                .open_read_session(self.selection, open_key, &self.budget(budget)?)?;
        Ok(NativeQuerySession {
            reader: self,
            status,
            profile: None,
        })
    }

    /// Read a DSL projection from the same exact view the caller verified.
    pub fn dsl_projection_exact_admitted_v1(
        &self,
        view: quanta_index_contract::index::ViewRef,
        scope: &NativeDslScopeV1,
        kind: NativeDslKindV1,
        budget: &RequestBudgetV1,
        memory: &MemoryScope,
    ) -> Result<NativeProjectionRead, NativeQueryError> {
        let reader = self.exact_reader_in_scope_v1(view)?;
        let reply = reader.client.request_publication_admitted(
            NativeIndexRequest::ReadProjection {
                selection: reader.selection,
                projection: scope.projection(kind),
            },
            &reader.budget(budget)?,
            memory,
        )?;
        match reply.response() {
            NativeIndexResponse::Projection(observed) => Ok(*observed),
            NativeIndexResponse::Error(error) => Err(NativeUdsError::Refused(error.clone()).into()),
            _ => Err(NativeUdsError::Protocol("missing Native DSL projection response".into()).into()),
        }
    }

    pub fn open_exact_in_scope_v1(
        self,
        view: quanta_index_contract::index::ViewRef,
        open_key: [u8; 32],
        budget: &RequestBudgetV1,
    ) -> Result<NativeQuerySession<'a>, NativeQueryError> {
        self.exact_reader_in_scope_v1(view)?.open(open_key, budget)
    }

    fn exact_reader_in_scope_v1(
        self,
        view: quanta_index_contract::index::ViewRef,
    ) -> Result<Self, NativeQueryError> {
        let same = match self.selection {
            ViewSelection::Exact(expected) => view == expected,
            ViewSelection::Latest { workspace, epoch } => view.workspace == workspace && view.epoch == epoch,
        };
        if !same {
            return Err(NativeQueryError::ProfileBindingMismatch);
        }
        Ok(Self { selection: ViewSelection::Exact(view), ..self })
    }

    pub fn graph(
        &self,
        instance: IndexInstanceId,
        query: NativeGraphQuery,
        budget: &RequestBudgetV1,
    ) -> Result<NativeGraphResult, NativeQueryError> {
        Ok(self
            .client
            .query_graph(self.selection, instance, query, &self.budget(budget)?)?)
    }

    /// Standalone vector/projection query, preserving its original Native DTO
    /// and selection semantics. This does not claim retained-session authority.
    pub fn semantic(
        &self,
        instance: IndexInstanceId,
        query: SemanticQueryRequest,
        budget: &RequestBudgetV1,
    ) -> Result<NativeSemanticResult, NativeQueryError> {
        Ok(self
            .client
            .search_semantic(self.selection, instance, query, &self.budget(budget)?)?)
    }
}

/// Owns one last acknowledged lease and its authenticated captured profile.
/// Drop performs no I/O. Call `release` explicitly, including after a failed
/// profile/query call; retry uncertain Open only with the original open key.
pub struct NativeQuerySession<'a> {
    reader: NativeQueryReader<'a>,
    status: NativeReadSessionStatus,
    profile: Option<NativeReadSessionProfile>,
}

impl NativeQuerySession<'_> {
    #[must_use]
    pub const fn status(&self) -> &NativeReadSessionStatus {
        &self.status
    }

    #[must_use]
    pub const fn profile(&self) -> Option<&NativeReadSessionProfile> {
        self.profile.as_ref()
    }

    pub fn load_profile(
        &mut self,
        budget: &RequestBudgetV1,
    ) -> Result<&NativeReadSessionProfile, NativeQueryError> {
        self.active()?;
        let profile = self
            .reader
            .client
            .read_session_profile(&self.status, &self.reader.budget(budget)?)?;
        self.profile = Some(profile);
        self.profile
            .as_ref()
            .ok_or(NativeQueryError::ProfileRequired)
    }

    fn active(&self) -> Result<(), NativeQueryError> {
        if self.status.state != NativeReadSessionState::Active {
            return Err(NativeQueryError::InactiveSession);
        }
        Ok(())
    }

    /// The caller supplies an actual configured instance, not a guessed kind
    /// ordinal. Every descriptor/policy/group/input field must match exactly.
    fn instance(&self, expected: &InstanceSpec) -> Result<IndexInstanceId, NativeQueryError> {
        self.active()?;
        let profile = self
            .profile
            .as_ref()
            .ok_or(NativeQueryError::ProfileRequired)?;
        captured_instance(&self.status, profile, expected)
    }

    /// Query a published Native DSL projection against the session's exact view.
    pub fn dsl_v1(
        &self,
        query: NativeDslQueryV1,
        budget: &RequestBudgetV1,
    ) -> Result<NativeDslPageV1, NativeQueryError> {
        self.active()?;
        Ok(self.reader.client.search_dsl_in_read_session_v1(
            &self.status,
            query,
            &self.reader.budget(budget)?,
        )?)
    }

    pub fn text(
        &self,
        instance: &InstanceSpec,
        query: TextQueryRequest,
        budget: &RequestBudgetV1,
    ) -> Result<NativeTextResult, NativeQueryError> {
        Ok(self.reader.client.search_text_in_read_session(
            &self.status,
            self.instance(instance)?,
            query,
            &self.reader.budget(budget)?,
        )?)
    }

    /// Preserve repository identities for an unpaged retained `select:path` query.
    pub fn text_paths(
        &self,
        instance: &InstanceSpec,
        query: TextQueryRequest,
        budget: &RequestBudgetV1,
    ) -> Result<NativeTextPathResultV1, NativeQueryError> {
        Ok(self.reader.client.search_text_paths_in_read_session(
            &self.status,
            self.instance(instance)?,
            query,
            &self.reader.budget(budget)?,
        )?)
    }

    pub fn text_page(
        &self,
        instance: &InstanceSpec,
        query: TextQueryRequest,
        after: Option<NativeTextCursor>,
        budget: &RequestBudgetV1,
    ) -> Result<NativeTextPage, NativeQueryError> {
        Ok(self.reader.client.search_text_page_in_read_session(
            &self.status,
            self.instance(instance)?,
            query,
            after,
            &self.reader.budget(budget)?,
        )?)
    }

    pub fn text_path_page(
        &self,
        instance: &InstanceSpec,
        query: TextQueryRequest,
        after: Option<NativeTextPathCursorV1>,
        budget: &RequestBudgetV1,
    ) -> Result<NativeTextPathPageV1, NativeQueryError> {
        Ok(self.reader.client.search_text_path_page_in_read_session(
            &self.status,
            self.instance(instance)?,
            query,
            after,
            &self.reader.budget(budget)?,
        )?)
    }

    pub fn hybrid_file(
        &self,
        text: &InstanceSpec,
        semantic: &InstanceSpec,
        text_query: TextQueryRequest,
        semantic_query: SemanticQueryRequest,
        file_top_k: u32,
        budget: &RequestBudgetV1,
    ) -> Result<NativeHybridFileResult, NativeQueryError> {
        Ok(self.reader.client.search_hybrid_file_in_read_session(
            &self.status,
            self.instance(text)?,
            self.instance(semantic)?,
            text_query,
            semantic_query,
            file_top_k,
            &self.reader.budget(budget)?,
        )?)
    }

    /// Query History on this captured view using its exact configured instance.
    pub fn history(
        &self,
        instance: &InstanceSpec,
        query: quanta_index_contract::HistoryQueryRequest,
        after: Option<quanta_index_contract::native_protocol::NativeHistoryCursor>,
        budget: &RequestBudgetV1,
    ) -> Result<quanta_index_contract::native_protocol::NativeHistoryPage, NativeQueryError> {
        Ok(self.reader.client.search_history_in_read_session(
            &self.status,
            self.instance(instance)?,
            query,
            after,
            &self.reader.budget(budget)?,
        )?)
    }

    /// Read the durable cut of the configured History member on this same
    /// captured view. The call does not reopen Latest or run a History query.
    pub fn history_cut(
        &self,
        instance: &InstanceSpec,
        budget: &RequestBudgetV1,
    ) -> Result<quanta_index_contract::native_protocol::NativeReadSessionHistoryCutV1, NativeQueryError> {
        Ok(self.reader.client.read_history_cut_in_read_session(
            &self.status,
            self.instance(instance)?,
            &self.reader.budget(budget)?,
        )?)
    }

    pub fn renew(&mut self, budget: &RequestBudgetV1) -> Result<(), NativeQueryError> {
        self.active()?;
        self.status = self
            .reader
            .client
            .renew_read_session(&self.status, &self.reader.budget(budget)?)?;
        self.active()
    }

    pub fn refresh_status(&mut self, budget: &RequestBudgetV1) -> Result<(), NativeQueryError> {
        self.status = self
            .reader
            .client
            .read_session_status(&self.status, &self.reader.budget(budget)?)?;
        Ok(())
    }

    pub fn release(&mut self, budget: &RequestBudgetV1) -> Result<bool, NativeQueryError> {
        let (status, has_remaining_target_pins) = self
            .reader
            .client
            .release_read_session(&self.status, &self.reader.budget(budget)?)?;
        self.status = status;
        Ok(has_remaining_target_pins)
    }

    pub fn settlement(
        &self,
        budget: &RequestBudgetV1,
    ) -> Result<NativeReadSessionSettlement, NativeQueryError> {
        Ok(self
            .reader
            .client
            .read_session_settlement(&self.status, &self.reader.budget(budget)?)?)
    }
}

fn captured_instance(
    status: &NativeReadSessionStatus,
    profile: &NativeReadSessionProfile,
    expected: &InstanceSpec,
) -> Result<IndexInstanceId, NativeQueryError> {
    if status.state != NativeReadSessionState::Active {
        return Err(NativeQueryError::InactiveSession);
    }
    if profile.session != status.session || profile.view.identity != status.view {
        return Err(NativeQueryError::ProfileBindingMismatch);
    }
    let mut matches = profile
        .profile
        .instances
        .iter()
        .filter(|spec| spec.instance == expected.instance);
    let actual = matches.next().ok_or(NativeQueryError::InstanceMismatch)?;
    if matches.next().is_some() {
        return Err(NativeQueryError::AmbiguousInstance);
    }
    if actual != expected {
        return Err(NativeQueryError::InstanceMismatch);
    }
    Ok(actual.instance)
}

impl NativeQueryReader<'_> {
    /// Lend one exact-view admitted reader through the installed live peer.
    /// The caller authenticates that peer against its signed installation;
    /// reply bytes cannot create or replace this process lease.
    pub fn with_installed_exact_admitted_v1<R>(
        socket: &Path,
        peer: &quanta_index_ipc::KernelConnectedPeerProcessLeaseV1,
        view: ViewRef,
        consume: impl for<'loan> FnOnce(NativeQueryReader<'loan>) -> R,
    ) -> R {
        let client = NativeUdsClient::new(socket.to_owned()).with_bound_peer_process_v1(peer);
        consume(NativeQueryReader {
            client: &client,
            selection: ViewSelection::Exact(view),
            io_policy: ClientIoPolicy::default(),
        })
    }

    pub fn open_exact_admitted_in_scope_v1(
        &self,
        view: quanta_index_contract::index::ViewRef,
        open_key: [u8; 32],
        budget: &RequestBudgetV1,
        memory: &MemoryScope,
    ) -> Result<AdmittedNativePublicationReply, NativeQueryError> {
        self.exact_reader_in_scope_v1(view)?.open_admitted(open_key, budget, memory)
    }

    pub fn dsl_admitted_v1(
        &self,
        expected: &NativeReadSessionStatus,
        query: NativeDslQueryV1,
        budget: &RequestBudgetV1,
        memory: &MemoryScope,
    ) -> Result<AdmittedNativePublicationReply, NativeQueryError> {
        let scoped = self.exact_reader_in_scope_v1(expected.view)?;
        Ok(scoped.client.search_dsl_in_read_session_admitted_v1(
            expected,
            query,
            &scoped.budget(budget)?,
            memory,
        )?)
    }

    /// The response owns its original raw/decoded grants, including Error.
    /// Borrow response() only while this owner remains in caller DATA.
    pub fn open_admitted(
        &self,
        open_key: [u8; 32],
        budget: &RequestBudgetV1,
        memory: &MemoryScope,
    ) -> Result<AdmittedNativePublicationReply, NativeQueryError> {
        if open_key == [0; 32] {
            return Err(NativeQueryError::MissingOpenKey);
        }
        Ok(self.client.open_read_session_admitted(
            self.selection,
            open_key,
            &self.budget(budget)?,
            memory,
        )?)
    }

    pub fn profile_admitted(
        &self,
        expected: &NativeReadSessionStatus,
        budget: &RequestBudgetV1,
        memory: &MemoryScope,
    ) -> Result<AdmittedNativePublicationReply, NativeQueryError> {
        Ok(self
            .client
            .read_session_profile_admitted(expected, &self.budget(budget)?, memory)?)
    }

    fn admitted_instance(
        &self,
        expected: &NativeReadSessionStatus,
        profile: &AdmittedNativePublicationReply,
        instance: &InstanceSpec,
    ) -> Result<IndexInstanceId, NativeQueryError> {
        let quanta_index_contract::native_protocol::NativeIndexResponse::ReadSessionProfile(
            profile,
        ) = profile.response()
        else {
            return Err(NativeQueryError::ProfileRequired);
        };
        captured_instance(expected, profile, instance)
    }

    pub fn text_admitted(
        &self,
        expected: &NativeReadSessionStatus,
        profile: &AdmittedNativePublicationReply,
        instance: &InstanceSpec,
        query: TextQueryRequest,
        budget: &RequestBudgetV1,
        memory: &MemoryScope,
    ) -> Result<AdmittedNativePublicationReply, NativeQueryError> {
        Ok(self.client.search_text_in_read_session_admitted(
            expected,
            self.admitted_instance(expected, profile, instance)?,
            query,
            &self.budget(budget)?,
            memory,
        )?)
    }

    pub fn text_page_admitted(
        &self,
        expected: &NativeReadSessionStatus,
        profile: &AdmittedNativePublicationReply,
        instance: &InstanceSpec,
        query: TextQueryRequest,
        after: Option<NativeTextCursor>,
        budget: &RequestBudgetV1,
        memory: &MemoryScope,
    ) -> Result<AdmittedNativePublicationReply, NativeQueryError> {
        Ok(self.client.search_text_page_in_read_session_admitted(
            expected,
            self.admitted_instance(expected, profile, instance)?,
            query,
            after,
            &self.budget(budget)?,
            memory,
        )?)
    }

    pub fn text_path_page_admitted(
        &self,
        expected: &NativeReadSessionStatus,
        profile: &AdmittedNativePublicationReply,
        instance: &InstanceSpec,
        query: TextQueryRequest,
        after: Option<NativeTextPathCursorV1>,
        budget: &RequestBudgetV1,
        memory: &MemoryScope,
    ) -> Result<AdmittedNativePublicationReply, NativeQueryError> {
        Ok(self.client.search_text_path_page_in_read_session_admitted(
            expected,
            self.admitted_instance(expected, profile, instance)?,
            query,
            after,
            &self.budget(budget)?,
            memory,
        )?)
    }

    pub fn hybrid_file_admitted(
        &self,
        expected: &NativeReadSessionStatus,
        profile: &AdmittedNativePublicationReply,
        text: &InstanceSpec,
        semantic: &InstanceSpec,
        text_query: TextQueryRequest,
        semantic_query: SemanticQueryRequest,
        file_top_k: u32,
        budget: &RequestBudgetV1,
        memory: &MemoryScope,
    ) -> Result<AdmittedNativePublicationReply, NativeQueryError> {
        Ok(self.client.search_hybrid_file_in_read_session_admitted(
            expected,
            self.admitted_instance(expected, profile, text)?,
            self.admitted_instance(expected, profile, semantic)?,
            text_query,
            semantic_query,
            file_top_k,
            &self.budget(budget)?,
            memory,
        )?)
    }

    pub fn release_admitted(
        &self,
        expected: &NativeReadSessionStatus,
        budget: &RequestBudgetV1,
        memory: &MemoryScope,
    ) -> Result<AdmittedNativePublicationReply, NativeQueryError> {
        Ok(self
            .client
            .release_read_session_admitted(expected, &self.budget(budget)?, memory)?)
    }
}
