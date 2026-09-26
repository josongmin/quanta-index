# SEP-26 Retrieval Active Gap Register

Status: `ACTIVE`

Consolidated: 2026-09-27 from clean pre-documentation snapshot
`5e6addd5814ce8b71af808ff201ddd0b18fbe6c4`.

This file contains only work that can still change a current verification or qualification claim. Accepted design is
owned by the [SEP-26 ADR set](../../../adr/README.md). Historical detail is indexed by
[ARCHIVE-MANIFEST.md](ARCHIVE-MANIFEST.md).

| ID | Scope | Current status | Exit condition |
|---|---|---|---|
| G-01 | RBR-00/02/05/12 current-source integration | `NOT_RUN` | After this documentation/source-closure change, freeze one clean revision and run the canonical Python, Rust and SDK retrieval inventories with exact selected/executed/passed counts, raw terminals and fresh receipts. Do not compose older focused or stale-source runs. |
| G-02 | RBR-01/09 observation overhead and query performance | `NOT_RUN` | Run identical on/off workloads, including tight-deadline behavior, then the declared k/filter/floor matrix on a quiet host. Bind actual planner trace and preserve failures/timeouts in the result. Default floor remains 100 until the qualified decision rule passes. |
| G-03 | RBR-03/04/06/07/09/12 external development and final evaluation | `BLOCKED` on admitted external inputs; execution otherwise `NOT_RUN` | Freeze admitted corpus, model, comparator reference, independent gold, development/holdout split and experiment manifest. Run the finite development matrix, select one combination, then run one fresh final pair/replay. Bounded local parity, ANN and chunking probes do not satisfy this gate. |
| G-04 | RBR-08 canonical symbol text authority and ranking | `NOT_RUN` | Either retain typed refusal, or accept a separate schema/ingest/lifecycle/cursor migration ADR. A rank change additionally needs a source-bound misranking case, development ablation and admitted final holdout. |
| G-05 | RBR-10 ingest performance and resilience | `NOT_RUN` | Bind a current installed SDK/daemon and run fresh, replace and delete workloads with row-set invariants, activation, fault and restart checks, plus quiet-host latency. Transient timing must remain separate from durable receipts. |
| G-06 | RBR-11 platform resource ownership | `NOT_RUN` outside the bounded macOS owner checks | Run owner proof on every supported platform with a clean receipt. Preserve the legacy `ps` PID-start-identity limitation unless the sampler binds a stable process identity. |

## Closure order

1. G-01 after the final documentation revision.
2. G-02 and G-05 on immutable current binaries.
3. G-03 only after external admission inputs exist.
4. G-04 only if product scope chooses the symbol-authority expansion.
5. G-06 per supported platform.

`PAIR_VALID`, `QUALITY_DELTA` and `PERF_QUALIFIED` remain `NOT_RUN` until their own exit conditions pass. A local pass,
artifact presence or summary boolean does not close a row.
