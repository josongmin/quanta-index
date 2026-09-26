# Benchmark Migration — Active Gaps

Status: `PARTIAL`. This file lists only unfinished implementation and
verification. Completed architecture is in
[SEP-27-002](../../../adr/SEP-27-002-single-benchmark-orchestrator-and-typed-evidence.md);
the [ticket index](INDEX.md) maps owners. This is not a receipt or a benchmark
result.

The former 939-line chronological audit is retained in Git at
`git show 84c9331f:docs/plans/sep-26-bench-migration/tickets/CURRENT-AUDIT.md`.
Its raw paths, digests and outcomes remain bound to their original sources;
they do not establish current checkout status. In particular, the earlier
runtime Criterion full-profile build timed out rather than producing a complete
measurement. Do not infer current failure or success from that old run.

## Remaining work

| Owner | Unfinished condition | Exit evidence |
|---|---|---|
| BM-02/07 | Current-source evidence-store custody and scale | Re-run the complete control-plane suite and fresh-process replay after the staged-output no-follow fix and any streaming/GC changes. Distinguish local path controls from hostile concurrent rename or remote attestation. |
| BM-03/07 | Registered families versus executable current profiles | For each declared active profile, require a real or explicitly fixed-fixture producer, complete immutable capture, fresh validate/replay and CI policy parity. Do not promote registration or compile-only checks. |
| BM-04 | Micro and system profiles | Complete both registered Criterion targets and every declared case through the CLI; fresh validate and raw-derived replay. Capture exact build/binary/toolchain identity. Separately prove capture-time host lease and generator health before speed/capacity qualification. |
| BM-05 | Live retrieval and lexical comparison | Execute a view-bound two-repository pilot using the same frozen release and query pack; bind actual product index universe, raw responses, query transformations, rank semantics and timeouts. Run the current-source SDK/contract profile and replay. Recorded five-product scoring is not live search. |
| BM-06 | Authenticated agent outcome | The implemented unauthenticated import/replay is diagnostic only. Real trajectories and independent task/test receipts are needed before an authenticated outcome claim; do not silently relabel recorded fixtures. |
| BM-07 | Final cutover and CI | Demonstrate representative clean-source profile execution, raw parity, source/evidence refusal controls and hosted CI on the same frozen inputs. Keep baseline admission separate from a fixture pass. |

External license approval, independent gold, model assets and quiet-host
capacity are qualification inputs, not substitutes for the implementation
work above. `PAIR_VALID`, `QUALITY_DELTA`, speedup and capacity have no passing
claim here. The [test plan](TEST-PLAN.md) owns the required independent oracles;
[SEP-26-003](../../../adr/SEP-26-003-retrieval-evidence-custody-and-qualification.md)
owns retrieval proof promotion.
