//! One original request's read authority. No connection or transaction may
//! remain held while the caller performs admission, adapter or reader work.

use super::{IndexError, ReadTicketLease};

/// Bound by its issuer to the exact Catalog, session guard, ticket fence and
/// original request budget. Construction performs the entry observation.
///
/// Every successful observation authenticates the live session and exact pin,
/// checks expiry and the durable clock floor, and waits for any new clock
/// sample to become durable before returning. Concurrent observations may
/// share a bounded commit; an uncommitted sample never grants read authority.
/// Earlier successful observations remain durable after failure, cancellation,
/// abort, drop or restart. Neither abort nor drop releases the retained pin.
///
/// A failed observation terminates this attempt. Finalization performs one last
/// live observation and consumes the attempt. Public success requires successful
/// finalization followed by the caller's original request checkpoint. If a
/// commit succeeds as cancellation arrives, its floor remains durable, but
/// cancellation still prevents public success. No retry of ambiguous commit
/// outcomes is authorized by this interface.
pub trait CatalogReadAttempt: Send + Sync {
    fn ticket(&self) -> ReadTicketLease;

    fn observe(&mut self) -> Result<(), IndexError>;

    fn finalize(self: Box<Self>) -> Result<(), IndexError>;

    /// Discard request-local authority. All successful observations are already
    /// durable, so abort and drop require no deferred database work.
    fn abort(self: Box<Self>);
}
