//! Shared test-only foreground publisher lease for daemon tests.
//!
//! Overlay, save, stub and effort requests only validate the interaction
//! publisher lease; they never extend it. An editor keeps its lease live with
//! `InteractionRenew` (or `InteractionActivity` for a real input event), so a
//! scripted daemon session must do the same between protocol steps instead of
//! assuming that wall-clock time stays below the lease window under load.
//!
//! A lapsed lease is refused with `Conflict`, the same code many request
//! refusals use. A renewal succeeds only while the lease is live, so
//! [`LivePublisher::refused`] renews right after an expected refusal to prove
//! the refusal came from the request rather than from an expired lease.

use std::{cell::Cell, error::Error, fmt::Debug, time::Duration};

use quanta_index_contract::native_protocol::{NativeIndexErrorCode, NativeInteractionPublisherId};
use quanta_index_core::RequestBudgetV1;
use quanta_index_sdk::native_uds::{NativeUdsClient, NativeUdsError};

type LeaseResult<T = ()> = Result<T, Box<dyn Error>>;

fn lease_budget() -> RequestBudgetV1 {
    RequestBudgetV1::for_duration(Duration::from_secs(30))
}

/// One live foreground publisher with its strictly increasing command
/// sequence. Every lease command goes through this owner.
pub(super) struct LivePublisher<'a> {
    client: &'a NativeUdsClient,
    id: NativeInteractionPublisherId,
    sequence: Cell<u64>,
}

impl<'a> LivePublisher<'a> {
    pub(super) fn begin(client: &'a NativeUdsClient, window: Duration) -> LeaseResult<Self> {
        Ok(Self {
            client,
            id: client.begin_interaction_publisher(window, &lease_budget())?,
            sequence: Cell::new(0),
        })
    }

    pub(super) const fn id(&self) -> NativeInteractionPublisherId {
        self.id
    }

    fn next_sequence(&self) -> LeaseResult<u64> {
        let next = self
            .sequence
            .get()
            .checked_add(1)
            .ok_or("publisher sequence overflow")?;
        self.sequence.set(next);
        Ok(next)
    }

    /// Report a foreground interaction, which also extends the lease.
    pub(super) fn activity(&self) -> LeaseResult {
        Ok(self.client.report_interaction_activity(
            self.id,
            self.next_sequence()?,
            &lease_budget(),
        )?)
    }

    /// Extend the lease without reporting an interaction. It succeeds only
    /// while the lease is live.
    pub(super) fn renew(&self) -> LeaseResult {
        self.client
            .renew_interaction_publisher(self.id, self.next_sequence()?, &lease_budget())
            .map_err(|error| format!("publisher lease was not live: {error:?}").into())
    }

    /// Fence the publisher with an explicit protocol event.
    pub(super) fn end(&self) -> LeaseResult {
        Ok(self.client.end_interaction_publisher(
            self.id,
            self.next_sequence()?,
            &lease_budget(),
        )?)
    }

    /// Expect `code` from the request itself, attributed by a successful
    /// renewal right afterwards: an expired lease cannot be renewed.
    pub(super) fn refused<T: Debug>(
        &self,
        result: Result<T, NativeUdsError>,
        code: NativeIndexErrorCode,
    ) -> LeaseResult {
        match result {
            Err(NativeUdsError::Refused(error)) if error.code == code => self.renew(),
            other => Err(format!("expected a {code:?} refusal; got {other:?}").into()),
        }
    }
}
