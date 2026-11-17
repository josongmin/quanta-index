//! Ordinary SDK Native reads. Session authority is explicit and never renewed implicitly.

use quanta_index_contract::{
    SemanticQueryRequest, TextQueryRequest,
    index::{IndexInstanceId, InstanceSpec, ViewSelection},
    native_protocol::{
        NativeGraphQuery, NativeGraphResult, NativeHybridFileResult, NativeReadSessionProfile,
        NativeReadSessionSettlement, NativeReadSessionState, NativeReadSessionStatus,
        NativeRepoMapQuery, NativeRepoMapResult, NativeSemanticResult, NativeTextCursor,
        NativeTextPage, NativeTextResult,
    },
};
use quanta_index_ipc::{ClientIoPolicy, MemoryScope, RequestBudgetV1};

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

    pub fn repomap(
        &self,
        instance: IndexInstanceId,
        query: NativeRepoMapQuery,
        budget: &RequestBudgetV1,
    ) -> Result<NativeRepoMapResult, NativeQueryError> {
        Ok(self
            .client
            .query_repomap(self.selection, instance, query, &self.budget(budget)?)?)
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
