//! A vendor write keeps its input custody until terminal completion.

use std::future::Future;

use quanta_index_core::{RequestBudgetV1, index::IndexError};

fn checkpoint(request: &RequestBudgetV1) -> Result<(), IndexError> {
    if request.is_cancelled() {
        Err(IndexError::Cancelled)
    } else if request.remaining().is_zero() {
        Err(IndexError::ResourceLimited)
    } else {
        Ok(())
    }
}

pub(super) async fn settle_write<T>(
    request: &RequestBudgetV1,
    future: impl Future<Output = Result<T, IndexError>>,
) -> Result<T, IndexError> {
    checkpoint(request)?;
    // Lance moves its input stream into a spawned task. Dropping the outer
    // write on cancellation detaches that task. Keep the write and original
    // input grant alive, then refuse publication under the original request.
    let result = future.await;
    checkpoint(request)?;
    result
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        time::Duration,
    };

    use super::*;

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "the test independently asserts that a cancelled write cannot finish before its spawned input owner"
    )]
    fn cancelled_write_settles_spawned_input_before_returning_refusal()
    -> Result<(), Box<dyn std::error::Error>> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        crate::run_blocking(&runtime, async {
            let request = RequestBudgetV1::for_duration(Duration::from_secs(30));
            let (started_tx, started_rx) = tokio::sync::oneshot::channel();
            let (release_tx, release_rx) = tokio::sync::oneshot::channel();
            let finished = Arc::new(AtomicBool::new(false));
            let child_finished = Arc::clone(&finished);
            let mut write = Box::pin(settle_write(&request, async move {
                let child = tokio::spawn(async move {
                    started_tx.send(()).map_err(|_closed| {
                        IndexError::BackendViolation("write fixture start receiver closed".into())
                    })?;
                    release_rx.await.map_err(|_closed| {
                        IndexError::BackendViolation("write fixture release sender closed".into())
                    })?;
                    child_finished.store(true, Ordering::SeqCst);
                    Ok(())
                });
                child
                    .await
                    .map_err(|error| IndexError::Storage(format!("write task: {error}")))?
            }));
            assert!(futures::poll!(write.as_mut()).is_pending());
            started_rx.await?;
            request.cancel_handle().cancel();
            // Pass the old drop-on-cancel wrapper's polling interval before
            // explicitly polling. The child still cannot finish until released.
            tokio::time::sleep(Duration::from_millis(50)).await;
            assert!(futures::poll!(write.as_mut()).is_pending());
            assert!(!finished.load(Ordering::SeqCst));
            release_tx
                .send(())
                .map_err(|_closed| "write child ended early")?;
            assert_eq!(write.await, Err(IndexError::Cancelled));
            assert!(finished.load(Ordering::SeqCst));
            Ok(())
        })
    }
}
