# SEP-26 Retrieval Active Gap Register

Status: `ACTIVE`

The accepted decisions are in the [SEP-26 ADR set](../../../adr/README.md).
Only open work is listed here. The completed RBR packets, old audit ledger and
source-bound receipt details are recoverable with
`git show eff53181:<path>`; their verdicts do not transfer to this checkout.
This consolidation changes the retrieval source closure. Final current-main
verification is `NOT_RUN` until a fresh exact-source receipt terminates.

The completed pre-deletion frozen snapshot (`3c6bc0ad`, tree `d23d32db`) has
source-bound G-01 `VERIFIED` results: SDK18, Python319, Rust108, separate
CI1648 with 15 dynamic subtests and 12 gates. Its external closeout is
`/private/tmp/qi-rbr-symbol-boundary-final.x3ddOxQY/final-closeout.json`
(SHA-256 `a44548cfb01004ac70034912e2f6e8c6a12dff3d1afe1740c4a1ae4605b36de7`).
That receipt excludes the changed main documentation tree, hosted CI,
qualified external pair/performance, full Rust workspace and deployment.

| ID | Scope and current boundary | Exit condition |
|---|---|---|
| G-01 | Native input admission, symbol-owner classification and no-follow evidence reading are implemented. Focused/owner-local proofs exist on earlier identified inputs; current-main integration `NOT_RUN`. Hosted CI was last observed `BLOCKED` by billing (14 jobs/0 steps). | Freeze the final source and run canonical Python, Rust, SDK, full CI, gates and evidence replay. Bind selected/executed/passed identities, dependencies, binaries, environment and raw terminals. Do not compose old owner, partial or frozen-snapshot receipts with the changed documentation tree. |
| G-02 | Observation overhead and query performance `NOT_RUN`; hybrid floor default is 100. | Run identical observation on/off workloads, tight deadlines and the declared k/filter/floor matrix on a quiet host. Bind actual planner traces and retain failures/timeouts. Change the default only under the ADR decision rule. |
| G-03 | Broad external quality and conditional model/incremental qualification `NOT_RUN`. Bounded SEARCH3, ANN and native5/85 probes are historical development evidence, not a full admitted pair. | Obtain independent corpus/gold and quiet-host inputs, freeze development and one holdout combination, run the actual pair/replay and declared breadth/filter/churn matrix. Do not reopen bounded implementation probes as missing code. |
| G-04 | Canonical symbol-text authority expansion `NOT_APPLICABLE` without explicit new product scope. No source-bound ranking defect has been established. | Preserve typed refusal. A new scope requires schema, ingress, lifecycle and cursor migration; ranking changes independently require a demonstrated misranking case and evaluation. |
| G-05 | Ingest performance, fault and restart qualification `NOT_RUN`; transient timings are not durable receipts. | On a current installed SDK/daemon, run fresh, replace and delete workloads with row-set, activation, fault and restart invariants plus quiet-host latency. |
| G-06 | Supported-platform resource qualification remains incomplete: macOS bounded owner checks exist; Linux delegated-cgroup/Landlock positive proof `NOT_RUN`. Native Windows pair is `NOT_APPLICABLE` absent new scope. | Bind platform-specific owner/resource custody on supported macOS/Linux. Fake-owner/process-group diagnostics do not qualify Linux cgroup behavior; preserve the macOS legacy `ps` PID-identity exclusion. |

Closure order: G-01 after the final documentation revision; G-02 and G-05 on
immutable binaries; G-03 only after the user-owned admission inputs exist;
G-06 per supported platform. G-04 is not active engineering work without an
explicit product decision. The [test plan](TEST-PLAN.md) owns required oracles
and commands. A local pass, artifact path or summary boolean closes no row.
