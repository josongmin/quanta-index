//! Compose native IPC delivery with orderly shutdown of its owners.

use std::time::Duration;

use anyhow::Result;
use quanta_index_contract::native_protocol::{NativeIndexRequest, NativeIndexRequestEnvelope};
use quanta_index_core::index::IndexError;
use quanta_index_ipc::IpcPlane;
use quanta_index_searchd::app::runtime::RuntimeRetirementPort;

use super::{NativeDispatcher, NativeIndexService, finish_native_service};
use crate::native_read_session_transport;
use crate::native_reply::{NativeReply, NativeReplyEnvelope};

impl NativeIndexService {
    pub fn run(mut self, accept_idle: Duration) -> Result<()> {
        let accept_idle = if self.pump.is_some() {
            accept_idle.min(Duration::from_millis(100))
        } else {
            accept_idle
        };
        let server_result = self.server.run::<
            NativeIndexRequestEnvelope,
            NativeIndexRequest,
            NativeReplyEnvelope,
            NativeReply,
            NativeDispatcher,
        >(&self.dispatcher, IpcPlane::Control, accept_idle);
        let read_sessions_result = native_read_session_transport::drain(&self.dispatcher);
        let pump_result = self.pump.take().map_or(Ok(()), |mut pump| pump.stop_join());
        // The worker joins after the pump releases its receiver. Physical
        // provider attempts retain their grant and root independently; logical
        // worker quiescence is not a claim that transport IO has completed.
        let worker_result = self.dispatcher.worker.stop_join();
        let provider_result = self.provider_attempt_pool.take().map_or(Ok(()), |pool| {
            // This is shutdown grace, never a renewed execution deadline.
            // An unfinished attempt still owns the root through its lease.
            let report = pool
                .drain_and_handoff(Duration::from_secs(5))
                .map_err(|error| IndexError::Storage(error.to_string()))?;
            if report.unfinished.is_empty() && report.panicked.is_empty() {
                Ok(())
            } else {
                Err(IndexError::Storage(format!(
                    "native provider drain unfinished: [{}], panicked: [{}]",
                    report.unfinished.join(","),
                    report.panicked.join(",")
                )))
            }
        });
        let retirement = self.catalog_retirement.clone();
        drop(self);
        let retirement_result = retirement.retire();
        finish_native_service(
            server_result.map_err(Into::into),
            read_sessions_result,
            pump_result,
            worker_result,
            provider_result,
            retirement_result,
        )
    }
}
