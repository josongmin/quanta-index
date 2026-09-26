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

The post-consolidation `20676778` attempt was controlled-stopped when the
shared store's staged-raw output could write through a linked ancestor. Its
SDK18 partial result and CI944/1648 partial result are not qualification;
stop receipt `/private/tmp/qi-rbr-current-final.n3mN6s/final-closeout.json`
SHA-256 `4fdd909ea0f4b4dc6be480641726dbe49c6098b5af74b7b506b306c3036fa297`.
The exact main preimage also wrote 39 bytes outside the requested store in a
bounded reproducer. The corrected writer uses no-follow directory descriptors,
exclusive raw leaf creation and refuses duplicate staging; the existing
reader/pointer guards remain. Five added negative controls failed on the old
source, while the corrected owner-local four-file suite passed 130/130 with
stable selected inputs (raw `/private/tmp/qi-rbr-sdk-symbol-final.TEINH2/final-store-output-main-green.log`,
SHA-256 `e751294fc83ca599986cf2f5acf931bfca246fe9d87a69e1cac3c2129de3b29c`).
This local proof does not qualify a full source or a hostile concurrent
directory-rename race.

The nFF exact-source snapshot closeout is
`/private/tmp/qi-rbr-store-final.nFFdMH/final-closeout.json` (SHA-256
`2c01663472c38030fc8b8eeba671ea6872f49e7934be59adb6307ca0768ab763`).
On that immutable source, native SDK18/Python319/Rust108, fresh validate/two
replays, both actual consumers with eight metadata refusals and five sealed
binary copies, separate CI1654 with 15 dynamic subtests and 12 gates, and
independent source/runtime POST verification terminated successfully. The
snapshot predates later concurrent main ADR/BM/registry edits and the G-02
comparator correction below: these results are snapshot-bound, not current-main
proof. The external receipt and raw artifacts own exact counts, dependencies,
commands, digests and exclusions. Do not edit a frozen source closure merely
to insert a passing status.

| ID | Scope and current boundary | Exit condition |
|---|---|---|
| G-01 | Native input admission, symbol-owner classification, no-follow evidence reading and staged-output custody are implemented. The nFF immutable snapshot has source-bound native, CI and replay `VERIFIED`; the later changed current main remains `NOT_RUN`. Hosted CI was last observed `BLOCKED` by billing (14 jobs/0 steps). | After concurrent main edits stabilize, freeze the new exact source and rerun affected native Python/Rust/SDK, full CI, gates and evidence replay with all bound identities. Do not compose old owner, partial or frozen-snapshot receipts with the changed source tree. |
| G-02 | The on/off diagnostic comparator now checks observable page/continuation, planner/lane, candidate and row-order parity while normalizing only per-run request IDs and stage timings. Its preimage failed the new owning test; corrected main passed focused 1/1 and owning module 319/319. Full observation overhead and query performance remain `NOT_RUN`. Hybrid floor default is 100. | Run identical observation on/off workloads, tight deadlines and the declared k/filter/floor matrix on a quiet host. Bind actual planner traces and retain failures/timeouts. Requalify the changed source closure; change the default only under the ADR decision rule. |
| G-03 | Broad external quality and conditional model/incremental qualification `NOT_RUN`. Bounded SEARCH3, ANN and native5/85 probes are historical development evidence, not a full admitted pair. | Obtain independent corpus/gold and quiet-host inputs, freeze development and one holdout combination, run the actual pair/replay and declared breadth/filter/churn matrix. Do not reopen bounded implementation probes as missing code. |
| G-04 | Canonical symbol-text authority expansion `NOT_APPLICABLE` without explicit new product scope. No source-bound ranking defect has been established. | Preserve typed refusal. A new scope requires schema, ingress, lifecycle and cursor migration; ranking changes independently require a demonstrated misranking case and evaluation. |
| G-05 | Ingest performance, fault and restart qualification `NOT_RUN`; transient timings are not durable receipts. | On a current installed SDK/daemon, run fresh, replace and delete workloads with row-set, activation, fault and restart invariants plus quiet-host latency. |
| G-06 | Supported-platform resource qualification remains incomplete: macOS bounded owner checks exist; Linux delegated-cgroup/Landlock positive proof `NOT_RUN`. Native Windows pair is `NOT_APPLICABLE` absent new scope. | Bind platform-specific owner/resource custody on supported macOS/Linux. Fake-owner/process-group diagnostics do not qualify Linux cgroup behavior; preserve the macOS legacy `ps` PID-identity exclusion. |

Closure order: G-01 after the final documentation revision; G-02 and G-05 on
immutable binaries; G-03 only after the user-owned admission inputs exist;
G-06 per supported platform. G-04 is not active engineering work without an
explicit product decision. The [test plan](TEST-PLAN.md) owns required oracles
and commands. A local pass, artifact path or summary boolean closes no row.
