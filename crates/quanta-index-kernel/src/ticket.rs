use std::sync::Arc;
use std::time::Duration;

use quanta_index_core::index::{
    CatalogReadAttempt, CatalogReadSettlementPort, CatalogView, IndexError,
    NativeReadReleaseObservation, NativeReadSettlement, NativeReadSettlementScope,
    ReadSessionLease, TicketRecord, WorkBudget,
    ports::{CatalogDiagnosticPort, CatalogReadPort},
};

// One original Catalog supplies both custody and observation. This is an
// object-safe view of that same owner, not a second reader or pin issuer.
pub(crate) trait TicketCatalog: CatalogReadPort + CatalogDiagnosticPort {}
impl<T: CatalogReadPort + CatalogDiagnosticPort> TicketCatalog for T {}

/// Request-local authority issued by one retained ticket's original Catalog.
/// Only its Registry owner may finalize and expose the private query result.
pub struct TicketReadAttempt<'a> {
    ticket: &'a ReadTicket,
    budget: &'a WorkBudget,
    authority: Box<dyn CatalogReadAttempt + 'a>,
}

impl TicketReadAttempt<'_> {
    pub(crate) fn require_owner(
        &self,
        ticket: &ReadTicket,
        budget: &WorkBudget,
    ) -> Result<(), IndexError> {
        if !std::ptr::eq(self.ticket, ticket)
            || self.authority.ticket() != ticket.record.lease
            || !self.budget.request().same_request_as(budget.request())
            || self.budget.request().deadline() != budget.request().deadline()
        {
            return Err(IndexError::InvalidInput(
                "read attempt belongs to another ticket or request".into(),
            ));
        }
        Ok(())
    }

    pub(crate) fn authority(&mut self) -> &mut dyn CatalogReadAttempt {
        self.authority.as_mut()
    }

    /// Consume the final durable observation before exposing success.
    pub fn finalize(self) -> Result<(), IndexError> {
        self.authority.finalize()?;
        self.budget.checkpoint()
    }

    /// Abandon request authority while retaining the caller's original pin.
    pub fn abort(self) {
        self.authority.abort();
    }
}

/// Exact immutable view lease. The catalog closes its read transaction before
/// returning this ticket; the lease protects roots until release.
pub struct ReadTicket {
    record: TicketRecord,
    released: bool,
    catalog: Arc<dyn TicketCatalog>,
    session: Arc<dyn ReadSessionLease>,
    lease: Duration,
    diagnostics: Option<quanta_index_core::DiagnosticContextV1>,
}

impl ReadTicket {
    pub(crate) fn new(
        record: TicketRecord,
        catalog: Arc<dyn TicketCatalog>,
        session: Arc<dyn ReadSessionLease>,
        lease: Duration,
    ) -> Self {
        Self {
            record,
            released: false,
            catalog,
            session,
            lease,
            diagnostics: None,
        }
    }

    pub(crate) fn with_diagnostics(mut self, budget: &WorkBudget) -> Self {
        self.diagnostics = budget.request().diagnostic_context().cloned();
        self
    }

    #[must_use]
    pub fn view(&self) -> &CatalogView {
        &self.record.view
    }

    #[must_use]
    pub fn record(&self) -> &TicketRecord {
        &self.record
    }

    /// A borrowed query may use only the catalog and process read session
    /// which originally issued this ticket.
    pub(crate) fn belongs_to(
        &self,
        catalog: &Arc<dyn CatalogReadPort>,
        session: &Arc<dyn ReadSessionLease>,
    ) -> bool {
        std::ptr::addr_eq(Arc::as_ptr(&self.catalog), Arc::as_ptr(catalog))
            && Arc::ptr_eq(&self.session, session)
    }

    pub fn validate(&self) -> Result<(), IndexError> {
        if self.released {
            return Err(IndexError::SnapshotUnavailable);
        }
        let _scope = self
            .catalog
            .enter_diagnostic_scope(self.diagnostics.as_ref());
        self.catalog
            .validate_ticket(self.session.as_ref(), self.record.lease)
    }

    pub fn validate_for_request(&self, budget: &WorkBudget) -> Result<(), IndexError> {
        if self.released {
            return Err(IndexError::SnapshotUnavailable);
        }
        self.catalog
            .validate_ticket_for_request(self.session.as_ref(), self.record.lease, budget)
    }

    /// Start one explicit request authority without transferring the retained pin.
    /// The caller must finalize before exposing success; abort/drop keeps custody.
    pub fn begin_read_attempt<'a>(
        &'a self,
        budget: &'a WorkBudget,
    ) -> Result<TicketReadAttempt<'a>, IndexError> {
        if self.released {
            return Err(IndexError::SnapshotUnavailable);
        }
        let authority =
            self.catalog
                .begin_read_attempt(self.session.as_ref(), self.record.lease, budget)?;
        Ok(TicketReadAttempt {
            ticket: self,
            budget,
            authority,
        })
    }

    pub fn renew(&mut self) -> Result<(), IndexError> {
        if self.released {
            return Err(IndexError::SnapshotUnavailable);
        }
        let _scope = self
            .catalog
            .enter_diagnostic_scope(self.diagnostics.as_ref());
        self.record.lease =
            self.catalog
                .renew_ticket(self.session.as_ref(), self.record.lease, self.lease)?;
        Ok(())
    }

    pub fn renew_for_request(&mut self, budget: &WorkBudget) -> Result<(), IndexError> {
        if self.released {
            return Err(IndexError::SnapshotUnavailable);
        }
        self.record.lease = self.catalog.renew_ticket_for_request(
            self.session.as_ref(),
            self.record.lease,
            self.lease,
            budget,
        )?;
        Ok(())
    }

    /// Release without consuming custody on failure. A retained read owner
    /// keeps this guard until Catalog confirms release, then may safely retry
    /// acknowledgement without deleting the same pin twice.
    pub fn try_release(&mut self) -> Result<(), IndexError> {
        if self.released {
            return Ok(());
        }
        let _scope = self
            .catalog
            .enter_diagnostic_scope(self.diagnostics.as_ref());
        let result = self
            .catalog
            .release_ticket(self.session.as_ref(), self.record.lease);
        self.released = result.is_ok();
        result
    }

    /// Settle the original pin through its private catalog session. A failed
    /// settlement leaves this ticket retained for caller-owned reconciliation.
    pub(crate) fn release_held(
        &mut self,
        catalog: &dyn CatalogReadSettlementPort,
        scope: NativeReadSettlementScope,
        budget: &mut WorkBudget,
    ) -> Result<NativeReadReleaseObservation, IndexError> {
        if self.released {
            return Err(IndexError::SnapshotUnavailable);
        }
        let observation =
            catalog.release_held_ticket(self.session.as_ref(), self.record.lease, scope, budget)?;
        // The returned observation certifies a committed deletion. Record it
        // before any caller-side checkpoint can fail or be cancelled.
        self.released = true;
        Ok(observation)
    }

    /// Settlement observation does not confer read authority. It remains
    /// available after release or expiry using the original private session.
    pub(crate) fn observe_settlement(
        &self,
        catalog: &dyn CatalogReadSettlementPort,
        scope: NativeReadSettlementScope,
        budget: &mut WorkBudget,
    ) -> Result<NativeReadSettlement, IndexError> {
        catalog.observe_read_settlement(self.session.as_ref(), scope, budget)
    }

    pub fn release(mut self) -> Result<(), IndexError> {
        self.try_release()
    }

    /// Finish the read before reporting cleanup or post-cleanup interruption.
    /// The result stays live through release, and its first failure wins.
    pub fn finish<R>(
        self,
        result: Result<R, IndexError>,
        budget: &WorkBudget,
    ) -> Result<R, IndexError> {
        let released = self.release();
        match (result, released) {
            (Err(primary), _) => Err(primary),
            (Ok(_), Err(cleanup)) => Err(cleanup),
            (Ok(value), Ok(())) => {
                budget.checkpoint()?;
                Ok(value)
            }
        }
    }
}

impl Drop for ReadTicket {
    fn drop(&mut self) {
        drop(self.try_release());
    }
}

#[cfg(test)]
mod tests;
