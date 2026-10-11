use std::sync::Arc;

use quanta_index_core::index::{CatalogView, IndexError, TicketRecord, ports::CatalogReadPort};

/// Exact immutable view lease. The catalog closes its read transaction before
/// returning this ticket; the lease protects roots until release.
pub struct ReadTicket {
    record: TicketRecord,
    released: bool,
    catalog: Arc<dyn CatalogReadPort>,
}

impl ReadTicket {
    pub(crate) fn new(record: TicketRecord, catalog: Arc<dyn CatalogReadPort>) -> Self {
        Self {
            record,
            released: false,
            catalog,
        }
    }

    #[must_use]
    pub fn view(&self) -> &CatalogView {
        &self.record.view
    }

    #[must_use]
    pub fn record(&self) -> &TicketRecord {
        &self.record
    }

    pub fn release(mut self) -> Result<(), IndexError> {
        let result = self.catalog.release_ticket(self.record.id);
        self.released = result.is_ok();
        result
    }
}

impl Drop for ReadTicket {
    fn drop(&mut self) {
        if !self.released {
            drop(self.catalog.release_ticket(self.record.id));
        }
    }
}
