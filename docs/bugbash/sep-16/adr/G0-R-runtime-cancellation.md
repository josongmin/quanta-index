# G0-R — runtime cancellation and scheduling baseline

**Status: BASELINE PINNED — hard cancellation is not available and is not
claimed.** W5 starts from a serial, uncancellable transport; the plan's
"cooperative deadline / flight-owned cancellation" work is confirmed as net-new
rather than a refinement of something that exists.

- Decided: 2026-09-16
- Gate owner: W0, informs W5 (QI-BB-002, QI-BB-016) and W4 single-flight (QI-BB-001)
- Probe: [`crates/quanta-index-ipc/tests/g0r_runtime_cancellation_probe.rs`](../../../../crates/quanta-index-ipc/tests/g0r_runtime_cancellation_probe.rs)
- Command: `just rust-w0-storage-gates` (recipe name kept; it now runs all three W0 probes), or
  `./scripts/cargow --lane test-integration-lane test --all-features --locked -p quanta-index-ipc --test g0r_runtime_cancellation_probe -- --nocapture`
- Surface probed: the real `UdsServer::run` accept loop with a barrier-gated dispatcher. Every ordering claim below is enforced by a barrier or channel handshake; nothing sleeps and assumes.

## Questions and evidence

Raw probe output (`G0R-EVIDENCE` lines, 2026-09-16):

```
G0R-EVIDENCE head_of_line blocked_client_outcome=err:Timeout_{_operation:_Read,_timeout:_150ms_} blocked_client_waited_ms=151 holder_outcome=ok:ProbeResponse_{_request_id:_1,_payload:_1001_} served_after_release=ok:ProbeResponse_{_request_id:_3,_payload:_6_} dispatch_completions=2
G0R-EVIDENCE disconnect_no_cancel abandoned_client_outcome=err:Timeout_{_operation:_Read,_timeout:_150ms_} completions_while_abandoned=0 completions_after_release=2 follow_up_outcome=ok:ProbeResponse_{_request_id:_9,_payload:_2_}
G0R-EVIDENCE shutdown_drain holder_outcome=ok:ProbeResponse_{_request_id:_1,_payload:_1001_} completion_stamps=[0] loop_exit_stamp=1 run_outcome=Ok(())
```

| Question | Result |
| --- | --- |
| Does one in-flight dispatch block every other client? | **Yes.** With client A parked inside the dispatcher on a barrier the test holds, client B — a separate connection carrying a request the dispatcher would answer instantly — timed out on read. Once A was released, the identical request was served. This is QI-BB-002 reproduced under handshake control, not inferred from timing. |
| Does a disconnected peer's dispatch get cancelled? | **No.** Client A abandoned its request (read budget expired) while its dispatch was parked. The dispatcher observed nothing: `completions_while_abandoned=0`. After release it ran to completion and produced a response for a peer that was gone (`completions_after_release=2`: the orphan plus the follow-up). There is no cancellation point between request decode and response write. |
| Does shutdown drain in-flight work? | **Yes.** Shutdown was triggered while A was parked. The accept loop exited (`loop_exit_stamp=1`) only after A's dispatch completed (`completion_stamps=[0]`), and A received its response. The serial loop makes drain a structural property: `run()` cannot return mid-dispatch because it *is* the dispatch. |
| Where are the cancellation points inside the longest native operations? | **Not probed here; from source.** `handle_connection` calls `dispatcher.dispatch(request)` synchronously with no cancellation token. Tantivy `Searcher::search` with a `TopDocs` collector runs to completion. LanceDB queries are async futures driven to completion behind a sync seam. None of these observe peer state. |
| Frame decode peak? | **From source, not measured.** `codec::decode_frame` (line 183) allocates `vec![0u8; body_len]` for the *declared* length — up to `MAX_FRAME_BODY_BYTES` = 16 MiB — before reading any body byte. A peer that sends a 4-byte header and stalls pins that allocation for `CONNECTION_IO_TIMEOUT` (30 s). Measuring it needs a counting `#[global_allocator]`, which this workspace's `#![forbid(unsafe_code)]` rules out; the code path is unambiguous. |
| Single-flight waiter survival? | **Not applicable today.** There is no single-flight (every query reopens, QI-BB-001), so "does cancelling the first waiter strand the others" has no current subject. It becomes W4's obligation the moment a `SnapshotRegistry` exists; DA-10's design answer (flight-owned cancel) stands. |

## Decision

1. **Hard cancellation of native work is not claimed.** W5 must design for cooperative cancellation: a deadline the dispatcher checks at its own boundaries (before native search, between candidate batches, before response encode) plus peer-liveness checks at the same points. Any claim that a query "was cancelled" must name the checkpoint that observed the cancel, not the fact that the client left.
2. **Process isolation is not chosen.** The probe found nothing that a bounded worker pool with cooperative checkpoints cannot address; the cost of isolation is not justified by the evidence. M1's `G0-R BLOCK → cooperative-only contract` branch is the outcome, taken deliberately rather than by default.
3. **Head-of-line blocking is the first W5 deliverable**, ahead of cancellation: the probe shows a client can be starved by an unrelated request with zero concurrency, which is a bigger availability defect than orphaned work.
4. **Frame allocation must move after a bounded read**, or reserve against a process-wide byte budget before allocating (QI-BB-016). The current shape lets one peer reserve 16 MiB per connection with four bytes.
5. **Shutdown drain is already correct** for the serial loop and must stay correct once work is pooled: a pooled design needs an explicit drain, because the property will no longer fall out of the loop structure.

## Rejected alternatives

- **`spawn_blocking` + abort as "cancellation".** Aborting the task does not stop the thread executing a native search; it only detaches the awaiter. The probe's second result (work completes for nobody) is exactly what that would look like from outside, so it would not change the evidence.
- **Relying on client-side deadlines as the cancellation mechanism.** The probe's second result shows the server cannot see them.

## Limitations

- The dispatcher under probe is a barrier stub, not a Tantivy or Lance search. The probe establishes the transport's properties; native-operation interruptibility is stated from source, as noted.
- Timings in the evidence (`151ms`) are the client's own budget elapsing, not a server latency measurement. They are ordering witnesses, not performance numbers.
