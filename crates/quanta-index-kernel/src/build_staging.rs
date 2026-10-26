//! Live backing custody for coordinator base records and erased build futures.

use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use quanta_index_core::index::{
    AttemptContext, BaseMember, IndexError, MemberBinding, MemoryLease, MemoryLifetime, WorkBudget,
    ports::IndexFuture,
};

pub(crate) struct PreparedBaseMember {
    base: BaseMember,
    // A transferred Box is consumed by build before this owner is released.
    _memory: Option<MemoryLease>,
}

impl PreparedBaseMember {
    pub(crate) fn new(
        binding: Option<MemberBinding>,
        budget: &WorkBudget,
    ) -> Result<Self, IndexError> {
        budget.checkpoint()?;
        let (base, memory) = if let Some(binding) = binding {
            let bytes = u64::try_from(std::mem::size_of::<MemberBinding>())
                .map_err(|_error| IndexError::ResourceLimited)?;
            let memory = budget.reserve_memory(bytes, MemoryLifetime::Temporary)?;
            (BaseMember::Present(Box::new(binding)), memory)
        } else {
            (BaseMember::Absent, None)
        };
        let prepared = Self {
            base,
            _memory: memory,
        };
        budget.checkpoint()?;
        Ok(prepared)
    }

    pub(crate) fn take(&mut self) -> BaseMember {
        std::mem::replace(&mut self.base, BaseMember::Absent)
    }
}

/// The owner is returned by value; only the actual concrete future is boxed.
/// `Pin<Box<_>>` is `Unpin`, allowing safe polling without projecting an inline `F`.
pub(crate) struct BuildFuture<'a, T> {
    future: IndexFuture<'a, T>,
    // The future's captured data is dropped before its backing grant.
    _memory: Option<MemoryLease>,
}

impl<T> Future for BuildFuture<'_, T> {
    type Output = Result<T, IndexError>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        self.get_mut().future.as_mut().poll(context)
    }
}

pub(crate) fn future<'a, 'context, F, T>(
    attempt: &'a mut AttemptContext<'context>,
    make: impl FnOnce(&'a mut AttemptContext<'context>) -> F,
) -> Result<BuildFuture<'a, T>, IndexError>
where
    F: Future<Output = Result<T, IndexError>> + Send + 'a,
    T: 'a,
{
    // F's monomorphized layout is known before invoking the factory. Borrow
    // the original budget before the future captures mutable AttemptContext.
    attempt.budget.checkpoint()?;
    let bytes = std::mem::size_of::<F>();
    let memory = if bytes == 0 {
        None
    } else {
        attempt.budget.reserve_memory(
            u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?
    };
    Ok(BuildFuture {
        future: Box::pin(make(attempt)),
        _memory: memory,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::tests::{NoIo, demand_test_context};
    use crate::staging_payload::tests::{Ledger, require, scoped_budget};
    use quanta_index_core::index::{
        Digest, IndexInstanceId, InputBinding, MapRoot, ObjectRef, SchemaId, StorageFormatId,
    };
    use std::{
        cell::Cell,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
    };

    fn binding() -> MemberBinding {
        MemberBinding {
            instance: IndexInstanceId([1; 16]),
            format: StorageFormatId([1; 16]),
            input: InputBinding {
                root: MapRoot(Some(ObjectRef(Digest([2; 32])))),
                schema: SchemaId([1; 16]),
                policy: Digest([3; 32]),
                context: Digest([4; 32]),
            },
            artifact: ObjectRef(Digest([5; 32])),
            logical_commitment: Digest([6; 32]),
        }
    }

    #[test]
    fn base_box_floor_refusal_transfer_and_refund_preserve_binding() -> Result<(), IndexError> {
        let (denied, ledger) = scoped_budget(0)?;
        require(matches!(
            PreparedBaseMember::new(Some(binding()), &denied),
            Err(IndexError::ResourceLimited)
        ))?;
        require(ledger.live() == 0)?;
        let mut absent = PreparedBaseMember::new(None, &denied)?;
        require(matches!(absent.take(), BaseMember::Absent) && ledger.live() == 0)?;
        let floor = u64::try_from(std::mem::size_of::<MemberBinding>())
            .map_err(|_error| IndexError::ResourceLimited)?;
        let (budget, ledger) = scoped_budget(floor)?;
        let mut prepared = PreparedBaseMember::new(Some(binding()), &budget)?;
        require(ledger.live() == floor)?;
        let owned = prepared.take();
        let BaseMember::Present(record) = &owned else {
            return Err(IndexError::Conflict);
        };
        require(**record == binding() && ledger.live() == floor)?;
        drop(owned);
        require(ledger.live() == floor)?;
        drop(prepared);
        require(ledger.live() == 0)
    }

    struct FrameProbe {
        data: [u8; 64],
        ledger: Arc<Ledger>,
        dropped_with_credit: Arc<AtomicBool>,
    }

    impl Future for FrameProbe {
        type Output = Result<MemberBinding, IndexError>;

        fn poll(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<Self::Output> {
            Poll::Pending
        }
    }

    impl Drop for FrameProbe {
        fn drop(&mut self) {
            self.dropped_with_credit.store(
                self.ledger.live() > 0 && self.data == [7; 64],
                Ordering::SeqCst,
            );
        }
    }

    #[test]
    fn future_floor_precedes_factory_and_frame_drop_precedes_refund() -> Result<(), IndexError> {
        let io = NoIo;
        let called = Cell::new(false);
        let drop_seen = Arc::new(AtomicBool::new(false));
        let (mut denied, ledger) = scoped_budget(0)?;
        let mut context = demand_test_context(&io, &mut denied);
        require(matches!(
            future(&mut context, |_attempt| {
                called.set(true);
                FrameProbe {
                    data: [7; 64],
                    ledger: ledger.clone(),
                    dropped_with_credit: drop_seen.clone(),
                }
            }),
            Err(IndexError::ResourceLimited)
        ))?;
        require(!called.get() && !drop_seen.load(Ordering::SeqCst) && ledger.live() == 0)?;
        let floor = u64::try_from(std::mem::size_of::<FrameProbe>())
            .map_err(|_error| IndexError::ResourceLimited)?;
        let (mut budget, ledger) = scoped_budget(floor)?;
        let mut context = demand_test_context(&io, &mut budget);
        let pending = future(&mut context, |_attempt| {
            called.set(true);
            FrameProbe {
                data: [7; 64],
                ledger: ledger.clone(),
                dropped_with_credit: drop_seen.clone(),
            }
        })?;
        require(called.get() && ledger.live() == floor)?;
        // Drop before the first poll is an abandoned/cancelled build path.
        drop(pending);
        require(drop_seen.load(Ordering::SeqCst) && ledger.live() == 0)?;
        let (mut cancelled, ledger) = scoped_budget(0)?;
        cancelled.request().cancel_handle().cancel();
        let mut context = demand_test_context(&io, &mut cancelled);
        called.set(false);
        require(matches!(
            future(&mut context, |_attempt| {
                called.set(true);
                FrameProbe {
                    data: [7; 64],
                    ledger: ledger.clone(),
                    dropped_with_credit: drop_seen.clone(),
                }
            }),
            Err(IndexError::Cancelled)
        ))?;
        require(!called.get() && ledger.live() == 0)
    }
}
