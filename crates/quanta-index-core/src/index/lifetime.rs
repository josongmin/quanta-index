//! Catalog-bound read custody. Expiry can invalidate a read; expiry alone must
//! never prove a running reader has stopped using immutable objects.

use std::time::Duration;

use super::{IndexError, TicketId};

/// Lease decisions use integral milliseconds in the durable Catalog clock.
/// Validate before creating a service, session or state marker.
pub fn validate_read_ticket_lease(lease: Duration) -> Result<u64, IndexError> {
    let milliseconds =
        u64::try_from(lease.as_millis()).map_err(|_error| IndexError::ResourceLimited)?;
    if !(1..=300_000).contains(&milliseconds) {
        return Err(IndexError::InvalidInput(
            "read ticket lease must be 1..=300000 ms".into(),
        ));
    }
    Ok(milliseconds)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReadSessionRef {
    pub id: u64,
    pub fence: u64,
}

/// Retain the owned session guard for the entire lifetime of every read ticket.
/// The catalog implementation binds this guard to process-death-safe custody.
pub trait ReadSessionLease: std::any::Any + Send + Sync {
    fn binding(&self) -> ReadSessionRef;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReadTicketLease {
    pub id: TicketId,
    pub session: ReadSessionRef,
    pub fence: u64,
    pub expires_at_ms: u64,
}

/// Bounded recovery advances only past fully examined sessions. A dead session
/// with a remaining pin batch is revisited using the same prior cursor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReadSessionRecoveryPage {
    pub after: Option<ReadSessionRef>,
    pub has_more: bool,
    /// Fully retired dead sessions, not elapsed leases or individual pins.
    pub recovered: u32,
    pub pins_released: u32,
    /// Unfinished session creations retired or confirmed absent under custody.
    pub open_intents_reaped: u32,
}
