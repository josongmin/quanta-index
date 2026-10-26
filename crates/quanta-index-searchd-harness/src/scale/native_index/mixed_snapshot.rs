//! Query snapshot custody spans publication and the last offered-query borrower.

use std::sync::{Arc, Mutex};

use anyhow::{Context as _, Result, ensure};
use quanta_index_contract::native_protocol::{NativeReadSessionStatus, NativeTextResult};
use quanta_index_core::RequestBudgetV1;
use quanta_index_sdk::native_uds::NativeUdsClient;

use super::{MixedStart, text_query, text_spec};

pub(super) struct QuerySnapshot {
    pub(super) state: MixedStart,
    pub(super) session: NativeReadSessionStatus,
}

impl QuerySnapshot {
    pub(super) fn search(
        &self,
        client: &NativeUdsClient,
        token: &str,
        top_k: u32,
        budget: &RequestBudgetV1,
    ) -> Result<NativeTextResult> {
        Ok(client.search_text_in_read_session(
            &self.session,
            text_spec().instance,
            text_query(token, top_k),
            budget,
        )?)
    }
}

#[derive(Default)]
pub(super) struct SnapshotReads {
    current: Option<Arc<QuerySnapshot>>,
    retired: Vec<Arc<QuerySnapshot>>,
}

impl SnapshotReads {
    pub(super) fn install(&mut self, state: MixedStart, session: NativeReadSessionStatus) {
        let next = Arc::new(QuerySnapshot { state, session });
        if let Some(previous) = self.current.replace(next) {
            self.retired.push(previous);
        }
    }

    pub(super) fn capture(&self) -> Result<Arc<QuerySnapshot>> {
        self.current.clone().context("mixed query snapshot absent")
    }

    pub(super) fn reap_shared<F>(shared: &Mutex<Self>, release: F) -> Result<()>
    where
        F: FnMut(&NativeReadSessionStatus) -> Result<()>,
    {
        let mut detached = {
            let mut bank = shared
                .lock()
                .map_err(|error| anyhow::anyhow!("mixed snapshot poisoned: {error}"))?;
            Self {
                current: None,
                retired: std::mem::take(&mut bank.retired),
            }
        };
        // RPC latency must not prevent queries from borrowing the current view.
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| detached.reap(release)));
        let poisoned = match shared.lock() {
            Ok(mut bank) => {
                bank.retired.append(&mut detached.retired);
                false
            }
            Err(error) => {
                // Preserve failed handles even when another caller poisoned the bank.
                error.into_inner().retired.append(&mut detached.retired);
                true
            }
        };
        let result = match result {
            Ok(result) => result,
            Err(panic) => std::panic::resume_unwind(panic),
        };
        if poisoned {
            result.context("mixed snapshot poisoned during release restoration")?;
            anyhow::bail!("mixed snapshot poisoned during release restoration");
        }
        result
    }

    pub(super) fn drain_owned<F>(owned: Mutex<Self>, release: F) -> Result<()>
    where
        F: FnMut(&NativeReadSessionStatus) -> Result<()>,
    {
        let (mut bank, poisoned) = match owned.into_inner() {
            Ok(bank) => (bank, false),
            Err(error) => (error.into_inner(), true),
        };
        let result = bank.drain(release);
        if poisoned {
            result.context("mixed snapshot poisoned before final drain")?;
            anyhow::bail!("mixed snapshot poisoned before final drain");
        }
        result
    }

    pub(super) fn reap<F>(&mut self, mut release: F) -> Result<()>
    where
        F: FnMut(&NativeReadSessionStatus) -> Result<()>,
    {
        let mut first_error = None;
        self.retired.retain(|snapshot| {
            // Retired snapshots cannot be captured from the bank. A live
            // borrower can clone one, but then this count cannot be one.
            if Arc::strong_count(snapshot) != 1 {
                return true;
            }
            match release(&snapshot.session) {
                Ok(()) => false,
                Err(error) => {
                    let _first = first_error.get_or_insert(error);
                    true
                }
            }
        });
        first_error.map_or_else(|| Ok(()), Err)
    }

    pub(super) fn drain<F>(&mut self, mut release: F) -> Result<()>
    where
        F: FnMut(&NativeReadSessionStatus) -> Result<()>,
    {
        let retired = self.reap(&mut release);
        let current = (|| -> Result<()> {
            if let Some(snapshot) = self.current.as_ref() {
                ensure!(
                    Arc::strong_count(snapshot) == 1,
                    "mixed current reader remains at drain"
                );
                release(&snapshot.session)?;
                self.current = None;
            }
            ensure!(
                self.retired.is_empty(),
                "mixed retired reader remains at drain"
            );
            Ok(())
        })();
        match (retired, current) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
            (Err(error), Err(cleanup)) => {
                Err(error.context(format!("mixed current cleanup also failed: {cleanup:#}")))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_contract::{
        index::{Digest, ViewRef},
        native_protocol::{NativeReadSessionId, NativeReadSessionState},
    };

    fn fixed_cleanup_panic() -> ! {
        panic!("fixed cleanup panic");
    }

    fn snapshot(sequence: u64, id: u8) -> (MixedStart, NativeReadSessionStatus) {
        let view = ViewRef {
            workspace: super::super::operation(1).workspace,
            epoch: 1,
            sequence,
            digest: Digest([id; 32]),
        };
        let state = MixedStart {
            view,
            first_ref: None,
            first_token: None,
            next_operation_sequence: 20,
            next_event_sequence: 19,
            transition_step: 0,
        };
        let session = NativeReadSessionStatus {
            session: NativeReadSessionId([id; 32]),
            view,
            expires_at_ms: 300_000,
            revision: 1,
            state: NativeReadSessionState::Active,
        };
        (state, session)
    }

    #[test]
    fn publication_keeps_original_snapshot_until_last_query_borrower_finishes() -> Result<()> {
        let mut bank = SnapshotReads::default();
        let (initial, initial_session) = snapshot(5, 1);
        bank.install(initial, initial_session);
        let first = bank.capture()?;
        let second = Arc::clone(&first);
        let (next, next_session) = snapshot(6, 2);
        bank.install(next, next_session);
        bank.reap(|_status| anyhow::bail!("released while two query borrowers remain"))?;
        ensure!(first.state.view.sequence == 5 && first.session == initial_session);
        ensure!(bank.capture()?.state.view.sequence == 6);
        drop(first);
        bank.reap(|_status| anyhow::bail!("released before the last query borrower finished"))?;
        ensure!(second.session == initial_session);
        drop(second);
        let mut released = Vec::new();
        let mut release = |status: &NativeReadSessionStatus| {
            released.push(status.session);
            Ok(())
        };
        bank.reap(&mut release)?;
        bank.drain(&mut release)?;
        ensure!(released == vec![NativeReadSessionId([1; 32]), NativeReadSessionId([2; 32])]);
        ensure!(bank.capture().is_err());
        Ok(())
    }

    #[test]
    fn in_flight_uds_query_keeps_captured_session_across_snapshot_replacement() -> Result<()> {
        use quanta_index_contract::{
            index::ObjectRef,
            native_protocol::{
                NativeIndexRequest, NativeIndexRequestEnvelope, NativeIndexResponse,
                NativeIndexResponseEnvelope, NativeReadSessionRequest, NativeTextCoverage,
            },
        };
        use std::{
            io::Write as _,
            os::unix::net::UnixListener,
            sync::mpsc,
            thread,
            time::{Duration, Instant},
        };
        let directory = tempfile::tempdir()?;
        let socket = directory.path().join("query.sock");
        let listener = UnixListener::bind(&socket)?;
        listener.set_nonblocking(true)?;
        let (initial, initial_session) = snapshot(5, 9);
        let mut bank = SnapshotReads::default();
        bank.install(initial, initial_session);
        let captured = bank.capture()?;
        let (arrived, receive_arrived) = mpsc::sync_channel(1);
        let (reply, receive_reply) = mpsc::sync_channel(1);
        let peer = thread::spawn(move || -> Result<()> {
            let deadline = Instant::now()
                .checked_add(Duration::from_secs(2))
                .context("test peer deadline overflow")?;
            let (mut stream, _) = loop {
                match listener.accept() {
                    Ok(connection) => break connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        ensure!(Instant::now() < deadline, "test query never connected");
                        thread::sleep(Duration::from_millis(1));
                    }
                    Err(error) => return Err(error.into()),
                }
            };
            stream.set_read_timeout(Some(Duration::from_secs(2)))?;
            stream.set_write_timeout(Some(Duration::from_secs(2)))?;
            let request: NativeIndexRequestEnvelope =
                quanta_index_ipc::decode_request(&mut stream)?;
            let NativeIndexRequest::ReadSession(NativeReadSessionRequest::SearchText {
                session,
                instance,
                query,
            }) = request.payload
            else {
                anyhow::bail!("offered query tried to acquire a fresh pin");
            };
            ensure!(session == NativeReadSessionId([9; 32]));
            ensure!(instance == text_spec().instance && query.query_text == "fixedmissing");
            arrived.send(())?;
            receive_reply.recv_timeout(Duration::from_secs(2))?;
            let response = NativeIndexResponseEnvelope {
                request_id: request.request_id,
                payload: NativeIndexResponse::TextRanked(NativeTextResult {
                    view: initial_session.view,
                    instance: text_spec().instance,
                    coverage: NativeTextCoverage::CoreRequired,
                    artifact: ObjectRef(Digest([31; 32])),
                    matched: 0,
                    exhaustive: true,
                    hits: Vec::new(),
                }),
            };
            stream.write_all(&quanta_index_ipc::encode_response(&response)?)?;
            Ok(())
        });
        let query = thread::spawn(move || {
            let client = NativeUdsClient::new(socket);
            captured.search(
                &client,
                "fixedmissing",
                1,
                &RequestBudgetV1::for_duration(Duration::from_secs(2)),
            )
        });
        let replacement = (|| -> Result<()> {
            receive_arrived.recv_timeout(Duration::from_secs(2))?;
            let (next, next_session) = snapshot(6, 10);
            bank.install(next, next_session);
            bank.reap(|_status| anyhow::bail!("released during the captured UDS query"))?;
            reply.send(())?;
            Ok(())
        })();
        // Join both threads on every outcome before reporting the assertion.
        let completed = query
            .join()
            .map_err(|panic| anyhow::anyhow!("query panicked: {panic:?}"));
        let served = peer
            .join()
            .map_err(|panic| anyhow::anyhow!("peer panicked: {panic:?}"));
        replacement?;
        let page = completed??;
        served??;
        ensure!(page.view.sequence == 5 && page.matched == 0);
        let mut released = Vec::new();
        bank.drain(|status| {
            released.push(status.session);
            Ok(())
        })?;
        ensure!(released == vec![NativeReadSessionId([9; 32]), NativeReadSessionId([10; 32])]);
        Ok(())
    }

    #[test]
    fn failed_release_retains_original_handle_and_drain_keeps_active_borrower() -> Result<()> {
        let mut bank = SnapshotReads::default();
        let (initial, initial_session) = snapshot(5, 3);
        bank.install(initial, initial_session);
        let (next, next_session) = snapshot(6, 4);
        bank.install(next, next_session);
        let failed = bank.reap(|_status| anyhow::bail!("fixed transport failure"));
        ensure!(failed.is_err());
        let borrower = bank.capture()?;
        let mut released = Vec::new();
        let mut release = |status: &NativeReadSessionStatus| {
            released.push(status.session);
            Ok(())
        };
        ensure!(bank.drain(&mut release).is_err());
        ensure!(borrower.session == next_session);
        drop(borrower);
        bank.drain(&mut release)?;
        ensure!(released == vec![NativeReadSessionId([3; 32]), NativeReadSessionId([4; 32])]);
        Ok(())
    }

    #[test]
    fn release_rpc_leaves_current_snapshot_unlocked_and_preserves_failed_handle() -> Result<()> {
        let mut bank = SnapshotReads::default();
        let (initial, initial_session) = snapshot(5, 11);
        bank.install(initial, initial_session);
        let (next, next_session) = snapshot(6, 12);
        bank.install(next, next_session);
        let shared = Mutex::new(bank);
        let failed = SnapshotReads::reap_shared(&shared, |status| {
            let current = shared
                .try_lock()
                .map_err(|error| anyhow::anyhow!("release held the snapshot lock: {error}"))?;
            ensure!(current.capture()?.session == next_session);
            drop(current);
            ensure!(status == &initial_session);
            anyhow::bail!("fixed release transport failure")
        });
        ensure!(failed.is_err());
        let mut released = Vec::new();
        SnapshotReads::reap_shared(&shared, |status| {
            let current = shared
                .try_lock()
                .map_err(|error| anyhow::anyhow!("retry held the snapshot lock: {error}"))?;
            ensure!(current.capture()?.session == next_session);
            drop(current);
            released.push(status.session);
            Ok(())
        })?;
        shared
            .into_inner()
            .map_err(|error| anyhow::anyhow!("test snapshot poisoned: {error}"))?
            .drain(|status| {
                released.push(status.session);
                Ok(())
            })?;
        ensure!(released == vec![NativeReadSessionId([11; 32]), NativeReadSessionId([12; 32])]);
        Ok(())
    }

    #[test]
    fn release_panic_restores_unacknowledged_handles_before_unwinding() -> Result<()> {
        let mut bank = SnapshotReads::default();
        let (initial, initial_session) = snapshot(5, 13);
        bank.install(initial, initial_session);
        let (next, next_session) = snapshot(6, 14);
        bank.install(next, next_session);
        let shared = Mutex::new(bank);
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            SnapshotReads::reap_shared(&shared, |_status| fixed_cleanup_panic())
        }));
        ensure!(panic.is_err());
        ensure!(
            shared
                .lock()
                .map_err(|error| anyhow::anyhow!("test bank poisoned: {error}"))?
                .capture()?
                .session
                == next_session
        );
        let mut released = Vec::new();
        SnapshotReads::drain_owned(shared, |status| {
            released.push(status.session);
            Ok(())
        })?;
        ensure!(released == vec![NativeReadSessionId([13; 32]), NativeReadSessionId([14; 32])]);
        Ok(())
    }

    #[test]
    fn poisoned_bank_drains_all_handles_and_preserves_poison_failure() -> Result<()> {
        let mut bank = SnapshotReads::default();
        let (initial, initial_session) = snapshot(5, 15);
        bank.install(initial, initial_session);
        let (next, next_session) = snapshot(6, 16);
        bank.install(next, next_session);
        let shared = Mutex::new(bank);
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = shared.lock();
            fixed_cleanup_panic();
        }));
        ensure!(panic.is_err());
        let mut released = Vec::new();
        let cleanup = SnapshotReads::drain_owned(shared, |status| {
            released.push(status.session);
            Ok(())
        });
        ensure!(cleanup.is_err());
        ensure!(released == vec![NativeReadSessionId([15; 32]), NativeReadSessionId([16; 32])]);
        Ok(())
    }
}
