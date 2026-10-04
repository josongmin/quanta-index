//! Bounded, non-scoring query-plane event capture for serial IPC diagnosis.

use std::collections::BTreeSet;

use quanta_index_contract::{
    ProcessRequestEventPlaneV1, ProcessRequestEventStageV1, ProcessRequestEventsV1,
};
use quanta_index_sdk::{ClientLexicalQueryObservationV1, ClientQueryRpcKindV1};
use serde::Serialize;

use quanta_index_retrieval_bench::{BenchError, BenchResult};

const SUCCESS_STAGES: [ProcessRequestEventStageV1; 8] = [
    ProcessRequestEventStageV1::Validated,
    ProcessRequestEventStageV1::QueueAdmitted,
    ProcessRequestEventStageV1::DispatchStarted,
    ProcessRequestEventStageV1::BackendStarted,
    ProcessRequestEventStageV1::BackendOutcome,
    ProcessRequestEventStageV1::BackendReturned,
    ProcessRequestEventStageV1::DispatchReturned,
    ProcessRequestEventStageV1::ResponseWritten,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct RequestPair {
    pub text_request_id: u64,
}

/// Join the one Active query RPC to its lossless server request ID.
pub(crate) fn client_observation_value(
    pair: RequestPair,
    observation: &ClientLexicalQueryObservationV1,
    sdk_execute_ns: u64,
) -> BenchResult<serde_json::Value> {
    let [rpc] = observation.rpcs.as_slice() else {
        return Err(BenchError::Protocol(
            "client query observation requires exactly one text RPC".to_string(),
        ));
    };
    if pair.text_request_id == 0
        || rpc.kind != ClientQueryRpcKindV1::Text
        || rpc.request_id != pair.text_request_id
    {
        return Err(BenchError::Protocol(
            "client query observation differs from server request identity".to_string(),
        ));
    }
    let timing = rpc.ipc;
    let children_ns = timing
        .encode
        .checked_add(timing.connect)
        .and_then(|sum| sum.checked_add(timing.write))
        .and_then(|sum| sum.checked_add(timing.decode_call))
        .ok_or_else(|| BenchError::Protocol("client RPC child clocks overflow".to_string()))?;
    if children_ns > timing.total
        || timing.read_io > timing.decode_call
        || timing.total > sdk_execute_ns
    {
        return Err(BenchError::Protocol(
            "client RPC clocks contradict SDK execution".to_string(),
        ));
    }
    Ok(serde_json::json!({
        "clock": "client_monotonic_duration_ns",
        "timing_boundary": "successful_sdk_query_rpc_calls",
        "sdk_execute_ns": sdk_execute_ns, "rpc_total_ns": timing.total,
        "sdk_unallocated_ns": sdk_execute_ns.saturating_sub(timing.total),
        "rpcs": [{
            "route": rpc.kind.as_str(), "request_id": rpc.request_id,
            "total_ns": timing.total, "encode_ns": timing.encode,
            "connect_ns": timing.connect, "write_ns": timing.write,
            "decode_call_ns": timing.decode_call, "read_io_ns": timing.read_io,
            "decode_non_read_ns": timing.decode_call.saturating_sub(timing.read_io),
            "unallocated_ns": timing.total.saturating_sub(children_ns),
        }],
        "read_io_accounting": "nested_inside_decode_call",
        "decode_non_read_accounting": "elapsed_local_work_not_cpu_time",
    }))
}

/// Admit only a complete, lossless, serial one-RPC trace per query.
pub(crate) fn check_serial_windows(
    before: &ProcessRequestEventsV1,
    after: &ProcessRequestEventsV1,
    expected_text_ids: &[u64],
) -> BenchResult<Vec<RequestPair>> {
    for (label, window) in [("before", before), ("after", after)] {
        window.validate_v1().map_err(|error| {
            BenchError::Protocol(format!("{label} query event window is invalid: {error}"))
        })?;
        if window.plane != ProcessRequestEventPlaneV1::Query
            || window.dropped_before != 0
            || window.dropped_after != 0
            || window.omitted_before_window
            || window.sequence_exhausted
        {
            return Err(BenchError::Protocol(format!(
                "{label} query event window is not complete and zero-loss"
            )));
        }
    }
    if before.process_instance != after.process_instance
        || before.next_sequence > after.next_sequence
    {
        return Err(BenchError::Protocol(
            "query event process identity or sequence changed".to_string(),
        ));
    }
    if !(1..=2).contains(&expected_text_ids.len())
        || expected_text_ids.contains(&0)
        || expected_text_ids
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            .len()
            != expected_text_ids.len()
    {
        return Err(BenchError::Protocol(
            "serial query event probe requires one or two distinct text request IDs".to_string(),
        ));
    }
    let new_events = after
        .events
        .iter()
        .filter(|event| event.sequence >= before.next_sequence)
        .collect::<Vec<_>>();
    if new_events.first().map(|event| event.sequence) != Some(before.next_sequence)
        || new_events
            .last()
            .and_then(|event| event.sequence.checked_add(1))
            != Some(after.next_sequence)
        || new_events.len() != expected_text_ids.len() * SUCCESS_STAGES.len()
    {
        return Err(BenchError::Protocol(
            "query event tail has a missing, extra or incomplete request".to_string(),
        ));
    }
    let mut pairs = Vec::with_capacity(expected_text_ids.len());
    for (index, expected_id) in expected_text_ids.iter().copied().enumerate() {
        let group = &new_events[index * SUCCESS_STAGES.len()..(index + 1) * SUCCESS_STAGES.len()];
        let request_id = group[0].request_id.get();
        let connection_id = group[0].connection_id;
        if request_id != expected_id
            || group.iter().enumerate().any(|(position, event)| {
                event.request_id.get() != request_id
                    || event.connection_id != connection_id
                    || event.stage != SUCCESS_STAGES[position]
                    || (position == 4
                        && (event.route.as_deref() != Some("query.text") || event.error.is_some()))
                    || (position > 0 && event.elapsed_micros < group[position - 1].elapsed_micros)
            })
        {
            return Err(BenchError::Protocol(format!(
                "query event RPC {index} is foreign, reordered or incomplete"
            )));
        }
        pairs.push(RequestPair {
            text_request_id: request_id,
        });
    }
    Ok(pairs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_contract::ProcessRequestEventV1;
    use quanta_index_sdk::{ClientIpcTimingV1, ClientQueryRpcObservationV1};
    use std::num::NonZeroU64;

    fn fixture() -> (RequestPair, ClientLexicalQueryObservationV1) {
        (
            RequestPair { text_request_id: 9 },
            ClientLexicalQueryObservationV1 {
                rpcs: vec![ClientQueryRpcObservationV1 {
                    kind: ClientQueryRpcKindV1::Text,
                    request_id: 9,
                    ipc: ClientIpcTimingV1 {
                        total: 100,
                        encode: 5,
                        connect: 10,
                        write: 15,
                        decode_call: 60,
                        read_io: 40,
                    },
                }],
            },
        )
    }

    #[test]
    fn client_join_binds_the_only_text_request_and_nested_read_clock() {
        let (pair, observation) = fixture();
        let value = client_observation_value(pair, &observation, 150).expect("valid one-RPC trace");
        assert_eq!(value["rpc_total_ns"], 100);
        assert_eq!(value["sdk_unallocated_ns"], 50);
        assert_eq!(value["rpcs"][0]["request_id"], 9);
        assert_eq!(value["rpcs"][0]["decode_non_read_ns"], 20);
        for mutate in 0..6 {
            let mut bad = observation.clone();
            match mutate {
                0 => bad.rpcs.push(bad.rpcs[0]),
                1 => bad.rpcs[0].request_id = 11,
                2 => bad.rpcs[0].kind = ClientQueryRpcKindV1::ResolveActiveGeneration,
                3 => bad.rpcs[0].ipc.read_io = 61,
                4 => bad.rpcs[0].ipc.total = 89,
                _ => bad.rpcs[0].ipc.encode = u64::MAX,
            }
            assert!(client_observation_value(pair, &bad, 150).is_err());
        }
        assert!(client_observation_value(pair, &observation, 99).is_err());
    }

    fn window(request_count: usize) -> ProcessRequestEventsV1 {
        let mut events = Vec::new();
        for rpc in 0..request_count {
            let request_id =
                NonZeroU64::new(u64::try_from(rpc + 1).expect("small ID")).expect("nonzero ID");
            for (index, stage) in SUCCESS_STAGES.iter().copied().enumerate() {
                events.push(ProcessRequestEventV1 {
                    sequence: u64::try_from(events.len() + 1).expect("small sequence"),
                    request_id,
                    connection_id: u64::try_from(rpc + 1).expect("small connection"),
                    stage,
                    elapsed_micros: u64::try_from(index).expect("small clock"),
                    route: (stage == ProcessRequestEventStageV1::BackendOutcome)
                        .then(|| "query.text".to_string()),
                    error: None,
                    ticket_id: None,
                    window_ordinal: None,
                });
            }
        }
        ProcessRequestEventsV1 {
            process_instance: "0000000000000000000000000000002a".to_string(),
            plane: ProcessRequestEventPlaneV1::Query,
            next_sequence: u64::try_from(events.len() + 1).expect("small tail"),
            oldest_retained_sequence: events.first().map(|event| event.sequence),
            events,
            dropped_before: 0,
            dropped_after: 0,
            omitted_before_window: false,
            sequence_exhausted: false,
        }
    }

    #[test]
    fn serial_one_rpc_windows_bind_response_ids_and_reject_loss() {
        let before = window(0);
        let after = window(2);
        assert_eq!(
            check_serial_windows(&before, &after, &[1, 2]).expect("complete trace"),
            vec![
                RequestPair { text_request_id: 1 },
                RequestPair { text_request_id: 2 }
            ]
        );
        assert_eq!(
            check_serial_windows(&window(1), &after, &[2]).expect("one new request"),
            vec![RequestPair { text_request_id: 2 }]
        );
        assert!(check_serial_windows(&before, &after, &[2, 1]).is_err());
        let mut lost = after.clone();
        lost.dropped_after = 1;
        assert!(check_serial_windows(&before, &lost, &[1, 2]).is_err());
        let mut foreign = after.clone();
        foreign.events[4].route = Some("query.symbol".to_string());
        assert!(check_serial_windows(&before, &foreign, &[1, 2]).is_err());
        let mut restarted = after.clone();
        restarted.process_instance = "0000000000000000000000000000002b".into();
        assert!(check_serial_windows(&before, &restarted, &[1, 2]).is_err());
    }
}
