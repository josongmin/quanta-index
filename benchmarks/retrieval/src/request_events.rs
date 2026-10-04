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
    pub resolve_request_id: u64,
    pub text_request_id: u64,
}

/// Join measured client intervals to the lossless server IDs. All arithmetic
/// uses durations from the client clock; server-local clocks are never subtracted.
pub(crate) fn client_observation_value(
    pair: RequestPair,
    observation: &ClientLexicalQueryObservationV1,
    sdk_execute_ns: u64,
) -> BenchResult<serde_json::Value> {
    if observation.rpcs.len() != 2 {
        return Err(BenchError::Protocol(
            "client query observation requires exactly resolve-active and text RPCs".to_string(),
        ));
    }
    let mut total_ns = 0_u64;
    let mut rpcs = Vec::with_capacity(2);
    for (rpc, (kind, request_id)) in observation.rpcs.iter().zip([
        (
            ClientQueryRpcKindV1::ResolveActiveGeneration,
            pair.resolve_request_id,
        ),
        (ClientQueryRpcKindV1::Text, pair.text_request_id),
    ]) {
        if request_id == 0 || rpc.kind != kind || rpc.request_id != request_id {
            return Err(BenchError::Protocol(
                "client query observation differs from server request identity or order"
                    .to_string(),
            ));
        }
        let timing = rpc.ipc;
        let children_ns = timing
            .encode_ns
            .checked_add(timing.connect_ns)
            .and_then(|sum| sum.checked_add(timing.write_ns))
            .and_then(|sum| sum.checked_add(timing.decode_call_ns))
            .ok_or_else(|| BenchError::Protocol("client RPC child clocks overflow".to_string()))?;
        if children_ns > timing.total_ns || timing.read_io_ns > timing.decode_call_ns {
            return Err(BenchError::Protocol(
                "client RPC child clocks exceed their observed parent".to_string(),
            ));
        }
        total_ns = total_ns
            .checked_add(timing.total_ns)
            .ok_or_else(|| BenchError::Protocol("client query RPC clocks overflow".to_string()))?;
        rpcs.push(serde_json::json!({
            "route": kind.as_str(), "request_id": request_id,
            "total_ns": timing.total_ns, "encode_ns": timing.encode_ns,
            "connect_ns": timing.connect_ns, "write_ns": timing.write_ns,
            "decode_call_ns": timing.decode_call_ns, "read_io_ns": timing.read_io_ns,
            "decode_non_read_ns": timing.decode_call_ns.saturating_sub(timing.read_io_ns),
            "unallocated_ns": timing.total_ns.saturating_sub(children_ns),
        }));
    }
    if pair.resolve_request_id == pair.text_request_id || total_ns > sdk_execute_ns {
        return Err(BenchError::Protocol(
            "client query RPC clocks or identity contradict SDK execution".to_string(),
        ));
    }
    Ok(serde_json::json!({
        "clock": "client_monotonic_duration_ns",
        "timing_boundary": "successful_sdk_query_rpc_calls",
        "sdk_execute_ns": sdk_execute_ns, "rpc_total_ns": total_ns,
        "sdk_unallocated_ns": sdk_execute_ns.saturating_sub(total_ns), "rpcs": rpcs,
        "read_io_accounting": "nested_inside_decode_call",
        "decode_non_read_accounting": "elapsed_local_work_not_cpu_time",
    }))
}

/// Retain only a complete, lossless, exactly serial two-RPC trace per query.
/// Each event clock is request-local and begins after frame decoding; this
/// function does not label any SDK/server residual as IPC overhead.
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
        || new_events.len() != expected_text_ids.len() * SUCCESS_STAGES.len() * 2
    {
        return Err(BenchError::Protocol(
            "query event tail has a missing, extra or incomplete request".to_string(),
        ));
    }
    let mut seen_request_ids = BTreeSet::new();
    let mut request_pairs = Vec::with_capacity(expected_text_ids.len());
    for (query_index, expected_text_id) in expected_text_ids.iter().copied().enumerate() {
        let mut pair = [0; 2];
        for (rpc_index, expected_route) in ["query.resolve_active", "query.text"]
            .into_iter()
            .enumerate()
        {
            let start = (query_index * 2 + rpc_index) * SUCCESS_STAGES.len();
            let group = &new_events[start..start + SUCCESS_STAGES.len()];
            let request_id = group[0].request_id.get();
            let connection_id = group[0].connection_id;
            if !seen_request_ids.insert(request_id)
                || group.iter().enumerate().any(|(index, event)| {
                    event.request_id.get() != request_id
                        || event.connection_id != connection_id
                        || event.stage != SUCCESS_STAGES[index]
                        || (index == 4
                            && (event.route.as_deref() != Some(expected_route)
                                || event.error.is_some()))
                        || (index > 0 && event.elapsed_micros < group[index - 1].elapsed_micros)
                })
            {
                return Err(BenchError::Protocol(format!(
                    "query event RPC {query_index}/{rpc_index} is foreign, reordered or incomplete"
                )));
            }
            pair[rpc_index] = request_id;
        }
        if pair[1] != expected_text_id {
            return Err(BenchError::Protocol(format!(
                "query event text request ID differs from response for query {query_index}"
            )));
        }
        request_pairs.push(RequestPair {
            resolve_request_id: pair[0],
            text_request_id: pair[1],
        });
    }
    Ok(request_pairs)
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU64;

    use quanta_index_contract::ProcessRequestEventV1;
    use quanta_index_sdk::{ClientIpcTimingV1, ClientQueryRpcObservationV1};

    use super::*;

    fn client_fixture() -> (RequestPair, ClientLexicalQueryObservationV1) {
        let pair = RequestPair {
            resolve_request_id: 7,
            text_request_id: 9,
        };
        let timing = ClientIpcTimingV1 {
            total_ns: 100,
            encode_ns: 5,
            connect_ns: 10,
            write_ns: 15,
            decode_call_ns: 60,
            read_io_ns: 40,
        };
        (
            pair,
            ClientLexicalQueryObservationV1 {
                rpcs: vec![
                    ClientQueryRpcObservationV1 {
                        kind: ClientQueryRpcKindV1::ResolveActiveGeneration,
                        request_id: 7,
                        ipc: timing,
                    },
                    ClientQueryRpcObservationV1 {
                        kind: ClientQueryRpcKindV1::Text,
                        request_id: 9,
                        ipc: timing,
                    },
                ],
            },
        )
    }

    #[test]
    fn client_join_uses_nested_read_clocks_and_fixed_request_ids() {
        let (pair, observation) = client_fixture();
        let value = client_observation_value(pair, &observation, 250).expect("fixed valid trace");
        assert_eq!(value["rpc_total_ns"], 200);
        assert_eq!(value["sdk_unallocated_ns"], 50);
        assert_eq!(value["rpcs"][0]["request_id"], 7);
        assert_eq!(value["rpcs"][1]["request_id"], 9);
        assert_eq!(value["rpcs"][1]["read_io_ns"], 40);
        assert_eq!(value["rpcs"][1]["decode_non_read_ns"], 20);
        assert_eq!(value["rpcs"][1]["unallocated_ns"], 10);
        assert_eq!(value["read_io_accounting"], "nested_inside_decode_call");
    }

    #[test]
    fn client_join_refuses_foreign_incomplete_reordered_and_overflowing_traces() {
        let (pair, fixture) = client_fixture();
        let mut mutants = Vec::new();
        let mut row = fixture.clone();
        row.rpcs.swap(0, 1);
        mutants.push(row);
        let mut row = fixture.clone();
        row.rpcs[1].request_id = 11;
        mutants.push(row);
        let mut row = fixture.clone();
        let _removed = row.rpcs.pop();
        mutants.push(row);
        let mut row = fixture.clone();
        row.rpcs.push(row.rpcs[0]);
        mutants.push(row);
        let mut row = fixture.clone();
        row.rpcs[1].ipc.read_io_ns = 61;
        mutants.push(row);
        let mut row = fixture.clone();
        row.rpcs[1].ipc.total_ns = 89;
        mutants.push(row);
        let mut row = fixture.clone();
        row.rpcs[0].ipc.encode_ns = u64::MAX;
        mutants.push(row);
        let mut row = fixture.clone();
        for rpc in &mut row.rpcs {
            rpc.ipc.total_ns = u64::MAX;
        }
        mutants.push(row);
        for mutant in mutants {
            assert!(client_observation_value(pair, &mutant, u64::MAX).is_err());
        }
        assert!(client_observation_value(pair, &fixture, 199).is_err());
        assert!(
            client_observation_value(
                RequestPair {
                    resolve_request_id: 0,
                    text_request_id: 9
                },
                &fixture,
                250,
            )
            .is_err()
        );
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
                    route: (stage == ProcessRequestEventStageV1::BackendOutcome).then(|| {
                        if rpc % 2 == 0 {
                            "query.resolve_active".to_string()
                        } else {
                            "query.text".to_string()
                        }
                    }),
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
    fn serial_two_query_trace_has_fixed_request_pairs() {
        let before = window(0);
        let after = window(4);
        assert_eq!(
            check_serial_windows(&before, &after, &[2, 4]).expect("complete trace"),
            vec![
                RequestPair {
                    resolve_request_id: 1,
                    text_request_id: 2
                },
                RequestPair {
                    resolve_request_id: 3,
                    text_request_id: 4
                }
            ]
        );
        assert_eq!(
            check_serial_windows(&window(2), &after, &[4]).expect("baseline excludes old RPCs"),
            vec![RequestPair {
                resolve_request_id: 3,
                text_request_id: 4
            }]
        );
    }

    #[test]
    fn serial_trace_rejects_loss_foreign_rpc_and_wrong_response() {
        let before = window(0);
        let mut after = window(2);
        assert!(check_serial_windows(&before, &after, &[3]).is_err());
        after.dropped_after = 1;
        assert!(check_serial_windows(&before, &after, &[2]).is_err());
        after.dropped_after = 0;
        after.events[12].route = Some("query.symbol".to_string());
        assert!(check_serial_windows(&before, &after, &[2]).is_err());
        after = window(2);
        after.process_instance = "0000000000000000000000000000002b".into();
        assert!(check_serial_windows(&before, &after, &[2]).is_err());
        after = window(2);
        let _removed = after.events.remove(0);
        assert!(check_serial_windows(&before, &after, &[2]).is_err());
        assert!(check_serial_windows(&before, &window(4), &[2]).is_err());
    }
}
