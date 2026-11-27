//! Ticket-bound query adapter for native text matches.
//!
//! The reader binds the selected text member exactly as the ranked text
//! reader does and runs the match engine in `text_match_scan` over it. It
//! shares the ranked reader's index configuration and reservation; see
//! [`NativeTextReader::match_reader`](crate::native::NativeTextReader::match_reader).

use std::ops::Deref;

use super::text::{BoundTextMember, NativeTextIndex, TextReadContext};
use super::text_match_scan::{ScannedTextMatches, scan_text_matches};
use quanta_index_contract::native_protocol::{
    NativeTextMatchCompletion, NativeTextMatchPage, NativeTextMatchesRequest,
};
use quanta_index_core::index::{
    IndexError, IndexQueryPort, InstanceSpec, QueryOutcome, QueryReadContext, effort::ResourceNeed,
    ports::IndexBuildPort as _,
};

/// One match page and the grants that admitted its rows.
pub struct NativeTextMatchResult(ScannedTextMatches);

impl Deref for NativeTextMatchResult {
    type Target = NativeTextMatchPage;

    fn deref(&self) -> &Self::Target {
        &self.0.page
    }
}

/// Serves [`NativeTextMatchesRequest`] over the selected native text member.
/// Clients cannot choose the recipe or the reservation.
pub struct NativeTextMatchReader {
    index: NativeTextIndex,
    need: ResourceNeed,
}

impl NativeTextMatchReader {
    pub(super) const fn new(index: NativeTextIndex, need: ResourceNeed) -> Self {
        Self { index, need }
    }
}

impl IndexQueryPort<NativeTextMatchesRequest, NativeTextMatchResult> for NativeTextMatchReader {
    fn validate_instance(&self, instance: &InstanceSpec) -> Result<(), IndexError> {
        self.index.validate_instance(instance)
    }

    fn reservation(&self, request: &NativeTextMatchesRequest) -> Result<ResourceNeed, IndexError> {
        request.validate().map_err(IndexError::InvalidInput)?;
        Ok(self.need)
    }

    fn query(
        &self,
        request: &NativeTextMatchesRequest,
        context: &mut QueryReadContext<'_>,
    ) -> Result<QueryOutcome<NativeTextMatchResult>, IndexError> {
        self.validate_instance(context.instance)?;
        let _need = self.reservation(request)?;
        let bound = BoundTextMember::from_context(context)?;
        let scanned = scan_text_matches(
            &self.index,
            &bound,
            request,
            &mut TextReadContext {
                ticket: context.ticket,
                objects: context.objects,
                maps: context.maps,
                budget: &mut *context.budget,
            },
        )?;
        let exhaustive = scanned.page.completion == NativeTextMatchCompletion::Exhausted;
        QueryOutcome::core_scanned(
            context.ticket,
            context.instance,
            exhaustive,
            NativeTextMatchResult(scanned),
        )
    }
}
