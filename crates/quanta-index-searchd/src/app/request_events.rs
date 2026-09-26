//! Operator diagnostic projection from the three runtime-owned IPC rings.

use std::sync::Arc;

use quanta_index_contract::{
    ProcessRequestEventPlaneV1, ProcessRequestEventStageV1, ProcessRequestEventV1,
    ProcessRequestEventsRequestV1, ProcessRequestEventsV1,
};
use quanta_index_core::CoreError;
use quanta_index_ipc::{IpcServerCounters, RequestEventStageV1, RequestEventWindowV1};
use quanta_index_search_plane::ProcessRequestEventsPort;

pub(crate) struct RuntimeRequestEvents {
    query: Arc<IpcServerCounters>,
    control: Arc<IpcServerCounters>,
    ingest: Arc<IpcServerCounters>,
}

impl RuntimeRequestEvents {
    pub(crate) fn new(
        query: Arc<IpcServerCounters>,
        control: Arc<IpcServerCounters>,
        ingest: Arc<IpcServerCounters>,
    ) -> Self {
        Self {
            query,
            control,
            ingest,
        }
    }
}

impl ProcessRequestEventsPort for RuntimeRequestEvents {
    fn request_events(
        &self,
        request: &ProcessRequestEventsRequestV1,
    ) -> Result<ProcessRequestEventsV1, CoreError> {
        request
            .validate_v1()
            .map_err(|error| CoreError::InvalidContract(error.to_owned()))?;
        let counters = match request.plane {
            ProcessRequestEventPlaneV1::Query => &self.query,
            ProcessRequestEventPlaneV1::Control => &self.control,
            ProcessRequestEventPlaneV1::Ingest => &self.ingest,
        };
        let window = counters.request_event_window_v1(usize::from(request.limit))?;
        Ok(project_window(request.plane, window))
    }
}

fn project_window(
    plane: ProcessRequestEventPlaneV1,
    window: RequestEventWindowV1,
) -> ProcessRequestEventsV1 {
    ProcessRequestEventsV1 {
        process_instance: format!("{:032x}", window.process_instance.get()),
        plane,
        events: window
            .events
            .into_iter()
            .map(|item| {
                let (stage, route, error, ticket_id, window_ordinal) = match item.event.stage {
                    RequestEventStageV1::Validated => (
                        ProcessRequestEventStageV1::Validated,
                        None,
                        None,
                        None,
                        None,
                    ),
                    RequestEventStageV1::ShuttingDown => (
                        ProcessRequestEventStageV1::ShuttingDown,
                        None,
                        None,
                        None,
                        None,
                    ),
                    RequestEventStageV1::QueueAdmitted => (
                        ProcessRequestEventStageV1::QueueAdmitted,
                        None,
                        None,
                        None,
                        None,
                    ),
                    RequestEventStageV1::QueueRefusedGlobal => (
                        ProcessRequestEventStageV1::QueueRefusedGlobal,
                        None,
                        None,
                        None,
                        None,
                    ),
                    RequestEventStageV1::QueueRefusedRepository => (
                        ProcessRequestEventStageV1::QueueRefusedRepository,
                        None,
                        None,
                        None,
                        None,
                    ),
                    RequestEventStageV1::DispatchStarted => (
                        ProcessRequestEventStageV1::DispatchStarted,
                        None,
                        None,
                        None,
                        None,
                    ),
                    RequestEventStageV1::BackendStarted => (
                        ProcessRequestEventStageV1::BackendStarted,
                        None,
                        None,
                        None,
                        None,
                    ),
                    RequestEventStageV1::BackendReturned => (
                        ProcessRequestEventStageV1::BackendReturned,
                        None,
                        None,
                        None,
                        None,
                    ),
                    RequestEventStageV1::BackendOutcome { route, error } => (
                        ProcessRequestEventStageV1::BackendOutcome,
                        Some(route.to_owned()),
                        error,
                        None,
                        None,
                    ),
                    RequestEventStageV1::ProviderStarted { ticket_id } => (
                        ProcessRequestEventStageV1::ProviderStarted,
                        None,
                        None,
                        Some(ticket_id),
                        None,
                    ),
                    RequestEventStageV1::ProviderReturned { ticket_id } => (
                        ProcessRequestEventStageV1::ProviderReturned,
                        None,
                        None,
                        Some(ticket_id),
                        None,
                    ),
                    RequestEventStageV1::IngestWindowStarted { window_ordinal } => (
                        ProcessRequestEventStageV1::IngestWindowStarted,
                        None,
                        None,
                        None,
                        Some(window_ordinal),
                    ),
                    RequestEventStageV1::IngestWindowReturned { window_ordinal } => (
                        ProcessRequestEventStageV1::IngestWindowReturned,
                        None,
                        None,
                        None,
                        Some(window_ordinal),
                    ),
                    RequestEventStageV1::DispatchReturned => (
                        ProcessRequestEventStageV1::DispatchReturned,
                        None,
                        None,
                        None,
                        None,
                    ),
                    RequestEventStageV1::PeerWatchFailed => (
                        ProcessRequestEventStageV1::PeerWatchFailed,
                        None,
                        None,
                        None,
                        None,
                    ),
                    RequestEventStageV1::PeerCancelled => (
                        ProcessRequestEventStageV1::PeerCancelled,
                        None,
                        None,
                        None,
                        None,
                    ),
                    RequestEventStageV1::ResponseEncodeFailed => (
                        ProcessRequestEventStageV1::ResponseEncodeFailed,
                        None,
                        None,
                        None,
                        None,
                    ),
                    RequestEventStageV1::ResponseWriteFailed => (
                        ProcessRequestEventStageV1::ResponseWriteFailed,
                        None,
                        None,
                        None,
                        None,
                    ),
                    RequestEventStageV1::ResponseWritten => (
                        ProcessRequestEventStageV1::ResponseWritten,
                        None,
                        None,
                        None,
                        None,
                    ),
                    RequestEventStageV1::Aborted => {
                        (ProcessRequestEventStageV1::Aborted, None, None, None, None)
                    }
                    RequestEventStageV1::Panicked => {
                        (ProcessRequestEventStageV1::Panicked, None, None, None, None)
                    }
                };
                ProcessRequestEventV1 {
                    sequence: item.sequence,
                    request_id: item.event.request_id,
                    connection_id: item.event.connection_id,
                    stage,
                    elapsed_micros: item.event.elapsed_micros,
                    route,
                    error,
                    ticket_id,
                    window_ordinal,
                }
            })
            .collect(),
        oldest_retained_sequence: window.oldest_retained_sequence,
        next_sequence: window.next_sequence,
        dropped_before: window.dropped_before,
        dropped_after: window.dropped_after,
        omitted_before_window: window.omitted_before_window,
        sequence_exhausted: window.sequence_exhausted,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::num::{NonZeroU64, NonZeroU128};

    use quanta_index_ipc::{RequestEventSinkV1, RequestEventV1};

    #[test]
    fn one_runtime_port_projects_the_existing_ring_without_a_second_store() {
        let instance = NonZeroU128::new(41).expect("fixture process instance");
        let query = Arc::new(IpcServerCounters::for_plane_with_instance(
            "query", instance,
        ));
        let control = Arc::new(IpcServerCounters::for_plane_with_instance(
            "control", instance,
        ));
        let ingest = Arc::new(IpcServerCounters::for_plane_with_instance(
            "ingest", instance,
        ));
        query.record_request_event_v1(RequestEventV1 {
            request_id: NonZeroU64::new(7).expect("fixture request ID"),
            connection_id: 3,
            stage: RequestEventStageV1::BackendOutcome {
                route: "query.text",
                error: None,
            },
            elapsed_micros: 11,
        });
        let port = RuntimeRequestEvents::new(query, control, ingest);
        let response = port
            .request_events(&ProcessRequestEventsRequestV1 {
                plane: ProcessRequestEventPlaneV1::Query,
                limit: 1,
            })
            .expect("operator query window");
        assert_eq!(
            response.process_instance,
            "00000000000000000000000000000029"
        );
        assert_eq!(response.events.len(), 1);
        let first = response.events.first().expect("one event fixture");
        assert_eq!(first.request_id.get(), 7);
        assert_eq!(first.route.as_deref(), Some("query.text"));
    }

    #[test]
    fn projected_tail_preserves_wrap_loss_and_process_restart_identity() {
        let instance = NonZeroU128::new(41).expect("fixture process instance");
        let query = Arc::new(IpcServerCounters::for_plane_with_instance(
            "query", instance,
        ));
        for id in 1..=1026_u64 {
            query.record_request_event_v1(RequestEventV1 {
                request_id: NonZeroU64::new(id).expect("positive request ID"),
                connection_id: 3,
                stage: RequestEventStageV1::Validated,
                elapsed_micros: 0,
            });
        }
        let port = RuntimeRequestEvents::new(
            query,
            Arc::new(IpcServerCounters::for_plane_with_instance(
                "control", instance,
            )),
            Arc::new(IpcServerCounters::for_plane_with_instance(
                "ingest", instance,
            )),
        );
        let request = ProcessRequestEventsRequestV1 {
            plane: ProcessRequestEventPlaneV1::Query,
            limit: 2,
        };
        let wrapped = port.request_events(&request).expect("wrapped tail");
        wrapped.validate_encoded_size_v1().expect("bounded tail");
        assert_eq!(wrapped.events.len(), 2);
        assert_eq!(wrapped.oldest_retained_sequence, Some(3));
        assert_eq!(
            wrapped
                .events
                .first()
                .expect("wrapped tail fixture")
                .sequence,
            1025
        );
        assert_eq!(wrapped.next_sequence, 1027);
        assert_eq!(wrapped.dropped_before, 2);
        assert_eq!(wrapped.dropped_after, 2);
        assert!(wrapped.omitted_before_window);

        let restarted = NonZeroU128::new(42).expect("restart process instance");
        let restarted_port = RuntimeRequestEvents::new(
            Arc::new(IpcServerCounters::for_plane_with_instance(
                "query", restarted,
            )),
            Arc::new(IpcServerCounters::for_plane_with_instance(
                "control", restarted,
            )),
            Arc::new(IpcServerCounters::for_plane_with_instance(
                "ingest", restarted,
            )),
        );
        let fresh = restarted_port.request_events(&request).expect("fresh tail");
        assert_ne!(fresh.process_instance, wrapped.process_instance);
        assert!(fresh.events.is_empty());
        assert_eq!(fresh.next_sequence, 1);
    }
}
