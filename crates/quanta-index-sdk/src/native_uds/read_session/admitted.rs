//! Retained read replies keep their original raw and decoded grants through
//! every borrow. The funded caller owns request DTO backing before entry.

use super::super::{AdmittedNativePublicationReply, MemoryScope, validate_text_result_with_memory};
use super::*;
use quanta_index_contract::{SemanticQueryRequest, native_protocol::NativeTextCursor};

impl<P: AsRef<Path>> NativeUdsClient<P> {
    /// Copy ACK fields only; keep the returned owner alive for full DATA borrows.
    pub fn open_read_session_admitted(
        &self,
        selection: ViewSelection,
        open_key: [u8; 32],
        budget: &RequestBudgetV1,
        memory: &MemoryScope,
    ) -> Result<AdmittedNativePublicationReply, NativeUdsError> {
        admitted_budget_checkpoint(budget)?;
        let request = NativeIndexRequest::ReadSession(NativeReadSessionRequest::Open {
            selection,
            open_key,
        });
        let reply = self.request_publication_admitted_ref(&request, budget, memory)?;
        match reply.response() {
            NativeIndexResponse::ReadSession(NativeReadSessionReply::Status(status)) => {
                validate_open(selection, status)?
            }
            NativeIndexResponse::Error(_) => {}
            _ => {
                return Err(NativeUdsError::Protocol(
                    "unexpected read session open reply".into(),
                ));
            }
        }
        admitted_budget_checkpoint(budget)?;
        Ok(reply)
    }

    pub fn read_session_profile_admitted(
        &self,
        expected: &NativeReadSessionStatus,
        budget: &RequestBudgetV1,
        memory: &MemoryScope,
    ) -> Result<AdmittedNativePublicationReply, NativeUdsError> {
        admitted_budget_checkpoint(budget)?;
        profile::validate_profile_request(expected)?;
        let request = NativeIndexRequest::ReadSession(NativeReadSessionRequest::ReadProfile {
            session: expected.session,
        });
        let reply = self.request_publication_admitted_ref(&request, budget, memory)?;
        match reply.response() {
            NativeIndexResponse::ReadSessionProfile(result) => {
                profile::validate_profile_with_memory(expected, result, Some(memory))?
            }
            NativeIndexResponse::Error(_) => {}
            _ => {
                return Err(NativeUdsError::Protocol(
                    "unexpected retained profile reply".into(),
                ));
            }
        }
        admitted_budget_checkpoint(budget)?;
        Ok(reply)
    }

    pub fn search_text_in_read_session_admitted(
        &self,
        expected: &NativeReadSessionStatus,
        instance: IndexInstanceId,
        query: TextQueryRequest,
        budget: &RequestBudgetV1,
        memory: &MemoryScope,
    ) -> Result<AdmittedNativePublicationReply, NativeUdsError> {
        admitted_budget_checkpoint(budget)?;
        let limit = prepare_text(expected, &query, Some(memory))?;
        let request = NativeIndexRequest::ReadSession(NativeReadSessionRequest::SearchText {
            session: expected.session,
            instance,
            query,
        });
        let reply = self.request_publication_admitted_ref(&request, budget, memory)?;
        let NativeIndexRequest::ReadSession(NativeReadSessionRequest::SearchText { query, .. }) =
            &request
        else {
            unreachable!()
        };
        match reply.response() {
            NativeIndexResponse::TextRanked(result) => validate_text_result_with_memory(
                ViewSelection::Exact(expected.view),
                instance,
                limit,
                &query.constraints,
                result,
                Some(memory),
            )?,
            NativeIndexResponse::Error(_) => {}
            _ => {
                return Err(NativeUdsError::Protocol(
                    "unexpected retained text reply".into(),
                ));
            }
        }
        admitted_budget_checkpoint(budget)?;
        Ok(reply)
    }

    pub fn search_text_page_in_read_session_admitted(
        &self,
        expected: &NativeReadSessionStatus,
        instance: IndexInstanceId,
        query: TextQueryRequest,
        after: Option<NativeTextCursor>,
        budget: &RequestBudgetV1,
        memory: &MemoryScope,
    ) -> Result<AdmittedNativePublicationReply, NativeUdsError> {
        admitted_budget_checkpoint(budget)?;
        let digest = page::prepare_page(expected, instance, &query, after, Some(memory))?;
        let request = NativeIndexRequest::ReadSession(NativeReadSessionRequest::SearchTextPage {
            session: expected.session,
            instance,
            query,
            after,
        });
        let reply = self.request_publication_admitted_ref(&request, budget, memory)?;
        let NativeIndexRequest::ReadSession(NativeReadSessionRequest::SearchTextPage {
            query, ..
        }) = &request
        else {
            unreachable!()
        };
        match reply.response() {
            NativeIndexResponse::TextRankedPage(result) => page::validate_page_with_memory(
                expected,
                instance,
                query.top_k,
                digest,
                after,
                &query.constraints,
                result,
                Some(memory),
            )?,
            NativeIndexResponse::Error(_) => {}
            _ => {
                return Err(NativeUdsError::Protocol(
                    "unexpected retained text page reply".into(),
                ));
            }
        }
        admitted_budget_checkpoint(budget)?;
        Ok(reply)
    }

    pub fn search_hybrid_file_in_read_session_admitted(
        &self,
        expected: &NativeReadSessionStatus,
        text_instance: IndexInstanceId,
        semantic_instance: IndexInstanceId,
        text_query: TextQueryRequest,
        semantic_query: SemanticQueryRequest,
        file_top_k: u32,
        budget: &RequestBudgetV1,
        memory: &MemoryScope,
    ) -> Result<AdmittedNativePublicationReply, NativeUdsError> {
        admitted_budget_checkpoint(budget)?;
        let lexical_limit = hybrid::prepare_hybrid(
            expected,
            &text_query,
            &semantic_query,
            file_top_k,
            Some(memory),
        )?;
        let request = NativeIndexRequest::ReadSession(NativeReadSessionRequest::SearchHybridFile {
            session: expected.session,
            text_instance,
            semantic_instance,
            text_query,
            semantic_query,
            file_top_k,
        });
        let reply = self.request_publication_admitted_ref(&request, budget, memory)?;
        let NativeIndexRequest::ReadSession(NativeReadSessionRequest::SearchHybridFile {
            text_query,
            semantic_query,
            ..
        }) = &request
        else {
            unreachable!()
        };
        match reply.response() {
            NativeIndexResponse::HybridFileRanked(result) => {
                hybrid::validate_hybrid_reply_with_memory(
                    expected,
                    text_instance,
                    semantic_instance,
                    lexical_limit,
                    semantic_query.top_k,
                    file_top_k,
                    &text_query.constraints,
                    result,
                    Some(memory),
                )?
            }
            NativeIndexResponse::Error(_) => {}
            _ => {
                return Err(NativeUdsError::Protocol(
                    "unexpected retained Hybrid file reply".into(),
                ));
            }
        }
        admitted_budget_checkpoint(budget)?;
        Ok(reply)
    }

    /// Release the original acknowledged session; no Drop I/O or reopened Latest.
    pub fn release_read_session_admitted(
        &self,
        expected: &NativeReadSessionStatus,
        budget: &RequestBudgetV1,
        memory: &MemoryScope,
    ) -> Result<AdmittedNativePublicationReply, NativeUdsError> {
        admitted_budget_checkpoint(budget)?;
        let request =
            NativeIndexRequest::ReadSession(NativeReadSessionRequest::Release(expected.session));
        let reply = self.request_publication_admitted_ref(&request, budget, memory)?;
        match reply.response() {
            NativeIndexResponse::ReadSession(NativeReadSessionReply::Released {
                status, ..
            }) => validate_release(expected, status)?,
            NativeIndexResponse::Error(_) => {}
            _ => {
                return Err(NativeUdsError::Protocol(
                    "unexpected read session release reply".into(),
                ));
            }
        }
        admitted_budget_checkpoint(budget)?;
        Ok(reply)
    }
}

#[cfg(test)]
mod tests;
