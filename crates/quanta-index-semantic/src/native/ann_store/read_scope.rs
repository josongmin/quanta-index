//! Cooperative stop and completion tracking for private ANN store reads.

use std::{
    future::Future,
    sync::atomic::{AtomicUsize, Ordering},
};

use quanta_index_core::index::IndexError;
use tokio::sync::Notify;

use super::object_error;

const STOPPED: usize = usize::MAX ^ (usize::MAX >> 1);
const ACTIVE_MASK: usize = !STOPPED;

/// Tracks read futures started by one private ANN store.
///
/// A stopped scope refuses new reads and wakes active reads. The caller must
/// keep the original execution grant alive through `wait_quiescent`.
pub(super) struct AnnReadScope {
    // The stop bit and active count share one atomic transition. Once stopped,
    // no later registration can race past a completed quiescence wait.
    state: AtomicUsize,
    stop_notify: Notify,
    quiescent_notify: Notify,
}

impl AnnReadScope {
    pub(super) fn new() -> Self {
        Self {
            state: AtomicUsize::new(0),
            stop_notify: Notify::new(),
            quiescent_notify: Notify::new(),
        }
    }

    pub(super) fn stop(&self) {
        let _prior = self.state.fetch_or(STOPPED, Ordering::AcqRel);
        self.stop_notify.notify_waiters();
    }

    pub(super) fn is_stopped(&self) -> bool {
        self.state.load(Ordering::Acquire) & STOPPED != 0
    }

    /// Call after `stop`. Every read registered before the stop has returned
    /// or dropped when this completes.
    pub(super) async fn wait_quiescent(&self) {
        loop {
            let notified = self.quiescent_notify.notified();
            tokio::pin!(notified);
            let _registered = notified.as_mut().enable();
            if self.state.load(Ordering::Acquire) & ACTIVE_MASK == 0 {
                return;
            }
            notified.await;
        }
    }

    pub(super) async fn run_read<T>(
        &self,
        work: impl Future<Output = object_store::Result<T>>,
    ) -> object_store::Result<T> {
        match self
            .state
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |state| {
                if state & STOPPED != 0 || state & ACTIVE_MASK == ACTIVE_MASK {
                    None
                } else {
                    state.checked_add(1)
                }
            }) {
            Ok(_previous) => {}
            Err(state) if state & STOPPED != 0 => return Err(stopped()),
            Err(_state) => {
                return Err(object_error(
                    IndexError::ResourceLimited,
                    "ANN read count overflow",
                ));
            }
        }
        let _guard = ActiveRead { scope: self };

        // notify_waiters does not retain a permit. Enable before checking the
        // latch so a concurrent stop cannot fall between registration and wait.
        let notified = self.stop_notify.notified();
        tokio::pin!(notified);
        let _registered = notified.as_mut().enable();
        if self.is_stopped() {
            return Err(stopped());
        }
        tokio::select! {
            biased;
            _ = &mut notified => Err(stopped()),
            result = work => result,
        }
    }
}

fn stopped() -> object_store::Error {
    object_error(IndexError::Cancelled, "ANN read stopped")
}

struct ActiveRead<'a> {
    scope: &'a AnnReadScope,
}

impl Drop for ActiveRead<'_> {
    fn drop(&mut self) {
        if self.scope.state.fetch_sub(1, Ordering::AcqRel) & ACTIVE_MASK == 1 {
            self.scope.quiescent_notify.notify_waiters();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{future::pending, sync::Arc, time::Duration};

    use tokio::sync::oneshot;

    use super::*;

    #[tokio::test]
    async fn stopped_read_wakes_and_quiesces() -> Result<(), Box<dyn std::error::Error>> {
        let scope = Arc::new(AnnReadScope::new());
        let read_scope = Arc::clone(&scope);
        let (entered, started) = oneshot::channel();
        let read = tokio::spawn(async move {
            read_scope
                .run_read(async {
                    let _sent = entered.send(());
                    pending::<object_store::Result<()>>().await
                })
                .await
        });
        tokio::time::timeout(Duration::from_secs(2), started).await??;
        scope.stop();
        tokio::time::timeout(Duration::from_secs(2), scope.wait_quiescent()).await?;
        if !matches!(
            read.await?.map_err(super::super::index_error),
            Err(IndexError::Cancelled)
        ) {
            return Err("stopped read succeeded".into());
        }
        Ok(())
    }

    #[tokio::test]
    async fn quiescence_waits_for_the_last_registered_read() -> Result<(), IndexError> {
        let scope = AnnReadScope::new();
        let first = scope.run_read(pending::<object_store::Result<()>>());
        let second = scope.run_read(pending::<object_store::Result<()>>());
        tokio::pin!(first, second);
        if !futures::poll!(first.as_mut()).is_pending()
            || !futures::poll!(second.as_mut()).is_pending()
        {
            return Err(IndexError::BackendViolation(
                "ANN fixture did not park both reads".into(),
            ));
        }
        scope.stop();
        let settled = scope.wait_quiescent();
        tokio::pin!(settled);
        if !futures::poll!(settled.as_mut()).is_pending() {
            return Err(IndexError::BackendViolation(
                "ANN reported quiescence while both reads were live".into(),
            ));
        }
        if !matches!(
            futures::poll!(first.as_mut()).map(|result| result.map_err(super::super::index_error)),
            std::task::Poll::Ready(Err(IndexError::Cancelled))
        ) || !futures::poll!(settled.as_mut()).is_pending()
        {
            return Err(IndexError::BackendViolation(
                "ANN reported quiescence before the second read ended".into(),
            ));
        }
        if !matches!(
            futures::poll!(second.as_mut()).map(|result| result.map_err(super::super::index_error)),
            std::task::Poll::Ready(Err(IndexError::Cancelled))
        ) || !futures::poll!(settled.as_mut()).is_ready()
        {
            return Err(IndexError::BackendViolation(
                "ANN did not quiesce after both reads ended".into(),
            ));
        }
        Ok(())
    }

    #[tokio::test]
    async fn ready_read_stop_and_observation_orders_preserve_the_first_outcome()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::task::Poll;

        #[derive(Clone, Copy, Debug)]
        enum Event {
            Ready,
            Stop,
            Observe,
        }
        use Event::{Observe, Ready, Stop};
        for schedule in [
            [Ready, Stop, Observe],
            [Ready, Observe, Stop],
            [Stop, Ready, Observe],
            [Stop, Observe, Ready],
            [Observe, Ready, Stop],
            [Observe, Stop, Ready],
        ] {
            let scope = AnnReadScope::new();
            let (send, receive) = oneshot::channel();
            let read = scope.run_read(async {
                receive.await.map_err(|_error| {
                    object_error(
                        IndexError::BackendViolation("fixture completion sender dropped".into()),
                        "ANN read fixture",
                    )
                })
            });
            tokio::pin!(read);
            if !futures::poll!(read.as_mut()).is_pending() {
                return Err("ANN read did not start pending".into());
            }

            let mut send = Some(send);
            let mut made_ready = false;
            let mut stopped = false;
            let mut finished = false;
            for event in schedule {
                match event {
                    Ready => {
                        let sender = send.take().ok_or("duplicate read completion")?;
                        let _sent = sender.send(17);
                        made_ready = true;
                    }
                    Stop => {
                        scope.stop();
                        stopped = true;
                    }
                    Observe => {
                        let expected = match (stopped, made_ready) {
                            (true, _) => Poll::Ready(Err(IndexError::Cancelled)),
                            (false, true) => Poll::Ready(Ok(17)),
                            (false, false) => Poll::Pending,
                        };
                        let observed = futures::poll!(read.as_mut())
                            .map(|result| result.map_err(super::super::index_error));
                        if observed != expected {
                            return Err(format!(
                                "{schedule:?} produced {observed:?}, expected {expected:?}"
                            )
                            .into());
                        }
                        finished = observed.is_ready();
                    }
                }
            }
            if !finished
                && !matches!(
                    futures::poll!(read.as_mut())
                        .map(|result| result.map_err(super::super::index_error)),
                    Poll::Ready(Err(IndexError::Cancelled))
                )
            {
                return Err(format!("{schedule:?} accepted a result after stop").into());
            }
            let stopped = scope.wait_quiescent();
            tokio::pin!(stopped);
            if !futures::poll!(stopped.as_mut()).is_ready() {
                return Err(format!("{schedule:?} retained a completed read").into());
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn stop_before_registration_refuses_read_without_waiting()
    -> Result<(), Box<dyn std::error::Error>> {
        let scope = AnnReadScope::new();
        scope.stop();
        let result = tokio::time::timeout(
            Duration::from_secs(1),
            scope.run_read(pending::<object_store::Result<()>>()),
        )
        .await?;
        if result.is_ok() {
            return Err("stopped read succeeded".into());
        }
        scope.wait_quiescent().await;
        if scope.state.load(Ordering::Acquire) & ACTIVE_MASK != 0 {
            return Err("stopped read retained an active slot".into());
        }
        Ok(())
    }
}
