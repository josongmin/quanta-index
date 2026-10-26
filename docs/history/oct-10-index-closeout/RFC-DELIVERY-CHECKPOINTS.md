> HISTORICAL ONLY. 당시 소스의 설계·실행 기록이며 현재 작업 지시가 아니다. 현재 작업·상태·담당은 [단일 SSOT](../../plans/oct-13-native-incremental-wave2/tickets/README.md), 실행 판정은 [VALIDATION](../../plans/oct-10-index-closeout/VALIDATION.md)을 따른다. 이 본문에 상태를 추가하지 않는다.

### 23.55 — Current-source final action register (2026-10-12)

This is the remaining-work entrypoint after the read-only parallel audit and
current-source reconciliation. It refines execution slices within section 14;
it adds no new dependency between independent Lexical, Semantic or IDE lanes.
Earlier sections retain their executed source and verdicts. Their words
"current", "final" and "pending" describe their checkpoint, not this register.
The subsequent implementation checkpoint in section 23.56 supersedes the
audit-time F02 service-call gap without promoting its pending acceptance.
Section 23.57 records its focused runtime acceptance; sections 23.58 and 17.2.4
supersede the F03 composition gap below. This register keeps the complete scope;
dated gap prose is not authority to repeat already implemented code.

Audit basis: local `main`, HEAD
`c32cd191554031b9a7d04207ffa3410e8fc201ba`, plus the inspected shared dirty
working tree on 2026-10-12. HEAD alone does not reproduce these edits. Source
inspection and other tasks' progress identify active ownership; neither proves
test completion. This documentation reconciliation runs no Rust test or benchmark.
No current Semantica source was audited in this reconciliation; cross-repository
requirements below are carried from the contract, not declared implemented.

#### Current facts and important corrections

- At this audit checkpoint, R10 physical reclaim and bounded automatic queue drainage exist. The earlier
  header saying physical reclaim was unimplemented is superseded. Existing
  `CatalogPort::advance_exact_history_floor`, `retire_exact_history_page`,
  `release_terminal_custody` and `retire_terminal_work_page` are not called by
  the inspected runtime. Queue drainage alone cannot establish a sustained
  history/object disk bound.
- R11 is in active implementation, not an untouched proposal. The canonical
  [memory IR](../../../crates/quanta-index-core/src/index/memory.rs),
  [shared ledger](../../../crates/quanta-index-searchd-runtime/src/native_resources.rs),
  [object reader](../../../crates/quanta-index-storage/src/object.rs) and
  [profile loader](../../../crates/quanta-index-kernel/src/profile.rs) already
  contain lease/reservation coupling. SQL BLOB scratch reads, decoded cursor
  pages, hit/path copies and transport buffers still need lifetime coverage.
  WorkUsage bytes are cumulative work, not a resident-memory charge. Explicit
  native aggregate spill/WAL policy and physical operating evidence remain open;
  absence of such policy is not proof that SQLite has no default checkpointing.
- At the 2026-10-12 register inspection,
  [native product composition](../../../crates/quanta-index-searchd-runtime/src/native_product.rs)
  registered Literal/Text/FileMap, an empty Semantic registry and `pump: None`,
  without configure_effort. Sections 23.58/17.2.4 supersede that code gap with
  configured Semantic/pump/effort and selected real-process acceptance. Missing
  IDE activity and full Auto/UX/resource acceptance remain open.
  [Launcher selection](../../../tools/deployment/launcher.py) still invokes `serve`.
- NativeText updates changed owner/term COW paths; the existing fixed-live
  32-revision test checks logical query/ranking/pin behavior. It does not measure
  total physical update/upkeep cost. Legacy File runs have different maintenance
  state. A generic run compactor for NativeText is not justified by those runs.
- Legacy production `RuntimeParts.publication_admission` remains `None` in the
  inspected [composition root](../../../crates/quanta-index-searchd-runtime/src/lib.rs).
  Its boot inventory/mutable writer/FD authority work is separate from native
  MemoryLease implementation and remains required until qualified retirement.

#### Remaining implementation and acceptance slices

Identifiers F01–F09 are execution slices, not additional R-family requirements.
Every slice names its producer/consumer boundary and a decisive acceptance scope.
No task may declare another chat's active source available for editing merely
because it appears in the following file map.

| Slice / R families | What exists; remaining change | Authoritative files / interfaces | Required acceptance |
|---|---|---|---|
| F01 resources — R11/R12 | MemoryScope/MemoryLease, StoredObject/LoadedProfile, result/frame/provider, inspected map cursor, canonical SourceRecord and RepoMap preparation have implementation/checkpoints. Keep remaining request/work/projection/Catalog metadata with its existing owner, then selected physical disk/WAL/secondary-file enforcement and vendor/RSS acceptance. The latest remediation recheck separates implemented paths and their own acceptance from remaining gaps. | Core `index/memory.rs`, `budget.rs`, `records.rs`, `mutations.rs`; storage object/map/scratch; Kernel source/work producers; Catalog/scratch SQLite and Runtime assembly. One shared authority, no second scheduler. | Reserve before allocation/growth; last shared clone releases one charge; deep copy requires new charge; escaping data retains memory but no CPU/IO/provider slot; real decoded source/prior+next refusal/lifetime golds. Preserve completed result/cursor tests rather than reassigning their implementation. Qualify selected extents and independently measure RSS/cache/spill/WAL; logical leases do not advertise physical caps. |
| F02 retention composition — R10/R32 | Exact floor, bounded history/work retirement, custody release and fenced unlink are implemented. Section 23.56 adds explicit sequence-window policy, indexed durable terminal-ready enumeration and service-owned calls before queued GC. Section 23.57 verifies the focused 56-test coupled checkpoint, including pinned physical source custody and post-release GC; sustained operating acceptance remains NOT_RUN. | Core `index/ports.rs`; Catalog `authority/retention.rs`, `terminal_custody.rs`, `terminal_work.rs`, `reclaim.rs`; runtime `native_retention.rs`, `native_reclaim_driver.rs` and maintenance composition. Reuse MaintenanceControl and original round cancellation/deadline. | Replay/floor/current-view/work/pin protections remain exact; protected rows cannot starve later eligible rows; queue progresses through restart; retained old view survives; partial unlink/sync failure is not a refund. Long pin, pressure and sustained ingress/retirement/drain establish the admitted disk envelope. Policy never guesses a safe floor from time alone. |
| F03 explicit native product — R30/R32/R34/R35 | Registered Semantic/provider, owned pump and configured effort are connected. Registered foreground publisher API/fencing/TTL, shared fresh Auto observations, Native HTTP buffer admission and redirect refusal are implemented in section 23.59, with 144/144 selected coupled tests and affected strict Clippy. Actual IDE publisher, complete Auto/UX and provider/resource operating acceptance remain. | Runtime `native_product.rs`, `native.rs`, `native_interaction.rs`, `native_interaction_transport.rs`, `native_effort_ports.rs`, `native_effort.rs`, `native_pump.rs`; existing contract/IPC/SDK codecs and CLI. | Real process: source event to Core query, asynchronous searchable coverage, edit/delete fences, Fast while Deep blocked, same-view terminal/cancel/deadline, unknown/stale signals and protected class admission. Invalid config refuses before creating state. No unsupported capability advertised. |
| F04 indexing cost — R15/R16/R17/R39 | Native changed-owner COW, bootstrap and logical churn oracles exist. Section 23.55's object-install observation adds optional Write-return byte and sync-operation counters to the existing 1/64-owner cold/delta fixture; its current terminal verdict is recorded there. Complete whole-pipeline cold build, fixed delta with growing corpus N/history H/fanout, deletion and mandatory upkeep measurement. Add a specific optimization only for an observed cost or source-established defect. | Kernel `tests/native_noop_cost.rs`, `native_bootstrap.rs`, `native_lifecycle.rs`; lexical native text/literal/spool and storage COW/object paths; existing harness/diagnostic owners. Legacy prepared-run merge remains its own lane. | Independent query/rebuild gold plus physical writes/bytes/fsync, retained space, GC and necessary drain; initial throughput, reopen/catch-up, freshness through searchable coverage, mixed query/update tails. Object-call counters exclude Catalog/WAL/GC/device IO and cannot qualify those costs. Large/XL correctness and matched performance are separate. No generic NativeText compactor, full-corpus validation workaround or raised timeout as a speedup. |
| F05 legacy resource closure — legacy LQ5 | Admission interfaces and subset quotas exist; boot installation, complete inventory/estimator/writer/FD coupling remain incomplete. Complete the existing shared authority rather than just replacing None with Some. | Existing publication/pool admission/capacity ports, runtime `publication_inventory.rs` and composition; ingress and mutable Lexical/Semantic writer owners. Exact boundaries in closeout plan sections 10.35–10.37. | Pre-create/pre-growth refusal; pending adoption at boot; same inode/hardlink one charge; unlink with live FD/pin retains charge; sync plus last lifetime releases once; actual WAL/vendor growth cannot bypass the bound. Existing staged hard-scan owner completes its repaired fixture and selected daemon acceptance. |
| F06 deployment/diagnostics — R13/R36 | Existing legacy backup/restore, launcher and trace tools are not new-native migration qualification. Deployment owner is repairing pre-existing staging deletion safety. Finish that repair; specify and implement native backup/restore or explicit rebuild cutover with exact state/receipt identity, then launcher selection. | Searchd `app/state_migration.rs`; runtime `state_migration.rs`, native format/product; `tools/deployment/launcher.py`; existing trace capture/profile/lifecycle files. See operator runbook. | Pre-existing directory, symlink, dangling link and source alias refuse without deleting foreign/source bytes; source/destination custody and crash boundary preserved. Native root/receipt/pin/replay identity survives supported restore/reopen. Failure-prefix diagnostics survive timeout and observer cost is reported separately. Legacy restore PASS does not qualify native restore. |
| F07 actual producer integration — R37/R38 | Index native ports/SDK exist; actual Semantica affected-domain/outbox/side-output contract is not closed by fake or fixed providers. | Actual Semantica producer owners plus Index contract, SDK and conformance boundaries. Coordinate the coupled source pair before edits. | Execute P2I01–P2I08 through actual producer/consumer calls with independent output gold: replacement/retraction, dependency impact, ACK loss/replay, save/delete/late work and original identity. Missing runnable producer inputs leave that acceptance BLOCKED, not fake-provider VERIFIED. |
| F08 full promised capabilities — R15/R21/R22/R24–R28 | Native exact small-vector, file/path and partial query features are not full ANN, provenance graph or IDE behavior. Complete the remaining domain parity, chunk/ANN feasibility, graph RepoMap, overlays/save handoff, required Stub and coordinate/action fencing. | Existing typed adapters and Core ports; Semantic ANN/chunk owners, graph RepoMap owner, session/editor producer and query owners. Preserve section 14's parallel W2 lanes. | Capability-specific independent query/ranking/recall gold, old-root/delete/late-job/coverage checks; IDE save/overlay/isolation/coordinate/action tests. ANN is no prerequisite to foundation/Lexical work, but remains required for the promised large-Semantic/full-product profile. No silent scope reduction. |
| F09 integration/release — R39/R40/R35/R36 | Scoped passes exist on different sources. Accept the coherent integrated candidate after affected modules settle, then default switch only under section 14.2. | Existing Justfile/cargow/test-authority/CI and product/launcher owners. | Selected regression names actually execute; full applicable suite, physical fault/operations and pinned performance/UX01–UX12/AUTO01–AUTO10 gates. Remote CI, release and Git publication are separate statuses. No addition of overlapping test counts or old-binary promotion to current source. |

#### Observed ownership and permitted parallelism

The following names preserve assignments observed at the original 2026-10-12
register checkpoint. They are not a current live-chat inventory, messages sent
to those tasks or a claim that all residual files have an owner. Later completed
checkpoints supersede their corresponding pending statements. Confirm the exact
current write set before dispatch; the recheck above owns residual priorities:

- **인덱싱 레이어링 어딧**: active F01 Core/Storage/Kernel object/profile/ledger
  integration and affected fixtures. Its progress explicitly leaves decoded
  cursor/hit/path buffers and physical spill/WAL unresolved. Do not start a
  second writer on those coupled files; require an explicit residual handoff.
- **Review Oct 10 index closeout**: active staged hard-scan restart fixture,
  daemon acceptance and legacy resource boundaries. The repaired G36 full
  replacement and pending-authority fence are its next narrow oracle; do not
  duplicate its build or the planned `test-daemon-all` selection.
- **증분 (2)**: File run merge/shared source-buffer repairs have scoped tests
  and strict lint acceptance; it is auditing subsequent indexing-cost work.
  Treat its next exact source write set as active, not automatically available.
- **Review Oct 10 index closeout --보완**: deployment/restore staging safety
  and trace-tool audit. Its reported deletion defect is an active repair;
  acceptance is pending, not a newly assigned duplicate implementation.
- **증분 (this register)**: documentation reconciliation/final native integration.
  F02/F03 and unowned F01/F08 residuals need exact file ownership before code
  dispatch. Existing task membership does not authorize this register to
  change those tasks' scope or send them messages.

F02 retention-policy design, F03 explicit-product configuration and the
independent W2 domain lanes may proceed in parallel after interfaces and exact
write sets are assigned. F04 can extend existing reusable oracles independently;
its physical performance run waits for the relevant integrated implementation.
F01/F02/F03 intersect Core/runtime facades and resource configuration: serialize
their shared hunks and final integration. Heavy Cargo/test/indexing runs use the
existing resource gate; do not launch competing copies for the same hypothesis.
F05 is not a new prerequisite to native foundation work and is not cancelled
by unfinished native progress. Default cutover remains behind section 14.2.

#### Evidence reconciliation and the next checkpoints

| Evidence | Correct disposition |
|---|---|
| Native 468/468 plus later runtime 80/80 | Historical scoped VERIFIED; overlapping selections. They do not include all subsequent MemoryLease/profile/interface changes. |
| Legacy Large 4,096-source PASS, 335.946 seconds | Functional pass on its compiled source; not a native Large or post-change performance qualification. |
| Later lexical 21/21, corrected producer/merge 7/7 and 3/3 | Scoped VERIFIED; three cases overlap. The earlier `producer::tests::` union branch matched zero; only the corrected producer selection proves those cases. |
| Closeout 85/86 batch, corrected panic/shutdown 3/3, Semantic recovery 21/21 and SDK 6/6 | Keep original FAILED batch and each source-specific subsequent VERIFIED scope. Do not label the whole latest workspace green. |
| Active object/profile/ledger and repaired staged G36 | Current acceptance pending at audit; previous compile/test failures remain recorded by their owner. Presence of repairs/tests does not prove execution. |
| Native Large/XL, sustained churn, aggregate RSS/spill/WAL, complete physical faults, actual producer, matched UX/performance and release | NOT_RUN for current integrated acceptance unless a later exact-scope result supersedes this row. |

1. Finish the active F01 coupling and staged/deployment owner checkpoints.
   Record actual terminal outcomes and source; first repair compilation or the
   narrow failing hypothesis. Do not reopen accepted unrelated matrices.
2. Integrate F02/F03 and the selected remaining domain/resource seams. Run a
   single affected contract/adapter/runtime/process batch with checked selection
   names. One-off raw execution output stays outside the checkout.
3. Run current-candidate Large/reopen and XL where required, then fixed-delta
   N/H/fanout and sustained mixed-load/space tests. Count foreground and mandatory
   maintenance together; preserve refusal/coverage semantics under overload.
4. Execute actual producer and selected-profile UX/ANN gates, native cutover
   rehearsal and full applicable suite/CI. Publish only after the explicit
   cutover contract is satisfied; source documentation is not that approval.

There is no justified completion percentage or wall-time guarantee for the
whole plan at this checkpoint. The structural delta path, correctness,
sustainable cost and measured product SLO remain separate acceptance claims.


### 23.75 Current source progress and remaining delivery (2026-10-13)

This source/document reconciliation supersedes earlier pending SDK-run statements,
not earlier source-bound test receipts. Quanta HEAD is
`c32cd191554031b9a7d04207ffa3410e8fc201ba` with346 dirty paths at inspection.
Semantica is concurrently integrating on main (observed `c139fec3be5f9661d7aa6ec2164ac612f72441a5`, then
`4a649cf5bc6e...`); its current source is not the old captured test source.
No fresh integrated qualification or percentage-complete claim is made.

Contract746/746, Foundation208/208 and Kernel ingress127/127 remain VERIFIED
at `accee39d3a925cab08ac8f9923a6e04c27cf528ef959fdd913747c291b159102`.
The SDK owning retry finished FAILED(exit101), tests0, source
`642142617eb2756bc90df8b1701340a5d3da6fcd5401d62aa479cf6b6b01e6fd`.
The moved build-context import passed that compile boundary; four parser type
inference diagnostics stopped execution. Current parser source now names
`JavaWorkspaceResolutionSyntaxErrorV1::from` and the retained-tree visitor's
`RetainedTreeSyntaxPathIndexErrorV1` result type. Their repaired source has no
new owning receipt; do not relabel the failed capture as success.

The current first-FULL API, reader activation, bound Dispatching resume, Original
error preservation and exact immutable-byte read are present in source.
Their coupled Runtime/SDK behavior remains NOT_RUN. Native recovery still lacks
allocation admission through CBOR decode/materialization and repeated member
readback. Ciborium allocates owned strings/bytes before the serde Visitor; a
seed-only facade or post-allocation charge cannot fix this. Implement a controlled
CBOR materializer/decoder owner, schema admission and retained leases together.
Preparing/Prepared/unbound Dispatching and Native terminal Refused retirement
remain distinct liveness work. Twelve current PTA/Runtime files still reference
the deleted generic Original fixture helper; genuine producer migration remains
required and must not be replaced with synthetic authority.

Quanta-index SourceAdvance/SourceResync are already implemented with regression
sources; the gap is their complete Semantica affected-set/deletion/event producer
join. Keep Native mutation guards until that join is real. At this historical
checkpoint NativeService constructed scratch with `open_managed`, Catalog with `open`, and objects with
`open_managed_with_scratch` in `native.rs`, without the newer common physical
IO owner at construction. The writer/read/reclaim modules do not establish boot
installation or aggregate disk/WAL enforcement. Legacy composition still carries
`publication_admission: None`; these are static production gaps, not reproduced
space failures. The later Native physical profile and public boot checkpoint
supersedes the Native constructor gap and records six passing affected tests;
it does not close operating or legacy composition acceptance. Launcher now selects `serve-native` when native_config is present
and validates the immutable boot receipt: earlier always-serve inventory is
superseded in source, while Native restore/cutover acceptance is not closed.

The remaining delivery order uses existing owners, not new journals/schedulers:

1. Freeze the repaired dependency/Runtime/SDK closure and run its decisive owning
   tests, including actual immutable-file and Original refusal laws.
2. Complete controlled CBOR allocation/read custody, remove duplicate unscoped
   decode/read, then resume exact earlier V9 phases without recollection.
3. Execute actual first FULL and ACK-loss/restart/search visibility; complete
   Semantica Delta/delete/Resync changed-set and event authority with independent
   output gold.
4. Install common physical authority before Native constructors mutate state,
   then qualify WAL/object/scratch/read/pin/reclaim/fault and sustained operating
   acceptance.
5. Preserve independent F03/F08 capability and IDE/Auto acceptance; qualify Native
   Large/XL, applicable integrated suite, deployment/restore, remote CI and release.

Steps1–3 do not retire F01–F09. Historical legacy XL900-second timeout remains
a valid failure for its source, but subsequent 07f0430 legacy XL lifecycle passed
at777.167seconds; neither result qualifies Native XL or formal performance. Scoped
old greens, successful main commits and current source presence are independent
from end-to-end performance and release acceptance.


### 23.76 Original scope progress accounting and remaining work (2026-10-13)

The denominator already exists in section23: the original full promised scope
is R01–R40. Its established rule counts each requirement once and credits
PARTIAL as zero for completion. F01–F09 are execution partitions, not additional
requirements. Do not use test totals, changed files, commits or newly split
tasks as the progress denominator. This reconciles §23.75: an effort-weighted
percentage is unavailable, but an equal-count implementation percentage is
well-defined by the original contract.

**Implementation requirement completion:18/40=45%.**
**Started, including partial:30/40=75%; this is not completion.**

| State | Count | Requirement IDs |
| --- | ---: | --- |
| IMPLEMENTED | 18 | R01, R02, R03, R04, R05, R06, R07, R08, R09, R14, R18, R19, R20, R23, R29, R31, R33, R34 |
| PARTIAL | 12 | R10, R11, R12, R13, R15, R16, R28, R30, R32, R35, R37, R39 |
| MISSING complete deliverable | 10 | R17, R21, R22, R24, R25, R26, R27, R36, R38, R40 |

MISSING means the required complete deliverable is absent, not that no related
code exists: R17 has COW/logical churn tests; R36 has legacy restore and a Native
launcher branch; R40 has scoped test/strict passes. None completes that family's
exit condition. Equal requirement weights say nothing about remaining engineering
hours: ANN/IDE/actual producer/physical admission may cost far more than one
foundation requirement.45% is not a runtime qualification, performance guarantee,
release-complete percentage or predicted delivery time. Current-row acceptance
is not upgraded from older component receipts.

| Original family | Implemented / total | Partial | Missing | Implementation fulfillment |
| --- | ---: | ---: | ---: | ---: |
| Canonical IR/adapters/source/COW/CAS/replay/read lifetime, R01–R09 | 9/9 | 0 | 0 | 100% |
| Retention/resources/scratch/diagnostics, R10–R13 | 0/4 | 4 | 0 | 0% |
| Lexical parity/build/sustainable cost, R14–R17 | 1/4 | 2 | 1 | 25% |
| Semantic ownership/reuse/exact/ANN, R18–R22 | 3/5 | 0 | 2 | 60% |
| RepoMap/editor/Stub/action fencing, R23–R28 | 1/6 | 1 | 4 | 16.7% |
| Auto/query/worker/provider/product service, R29–R35 | 4/7 | 3 | 0 | 57.1% |
| Cutover/actual producer/P2I/operations/release, R36–R40 | 0/5 | 2 | 3 | 0% |
| Total | 18/40 | 12 | 10 | 45% |

The foundation family's100% describes code exit conditions only. It does not
claim current integrated correctness or physical-resource closure.

Original legacy A/D/C5 repairs are separately VERIFIED(3/3) within their original
acceptance scope. They are not undone by this larger replacement plan and are
not counted again in R01–R40. Legacy XL product/lifecycle later passed on
`07f0430959cbaa7f2d72f7cbb5656cd5aa802670` at 777.166937750 seconds, producer exit0,
900-second cap unchanged (VALIDATION legacy checkpoint). Quiet-host performance
was BLOCKED and LQ5 sustained operating acceptance NOT_RUN. Earlier statements
calling the legacy XL timeout wholly unresolved were incorrect: retain the
historical failed source and later functional pass separately. Native XL and
formal performance remain NOT_RUN.

#### Remaining promised work, kept with original owners

| Packet | Original requirements | Concrete remaining deliverable | Decisive acceptance |
| --- | --- | --- | --- |
| Physical resource/recovery admission | R10–R12, R37 | Controlled CBOR raw/decode/reencode allocation and retained leases; no repeated unscoped member read/clone; Native constructor-time common authority for Catalog/WAL/object/scratch; final metadata/provider/vendor boundaries | Pre-birth denial, refund/lifetime/first-cause laws; actual boot inventory, WAL growth, reader pin/unlink, crash/space faults, long-pin sustained drain |
| Indexing algorithm and total cost | R13,R15–R17,R39 | Complete token/BM25/regex parity; cold build/reopen/catch-up; changed-size vs corpus/history/fanout costs; sustainable upkeep and write amplification; failure-prefix/observer cost | Independent query/ranking gold, bounded cost counters plus mandatory upkeep; matched initial/delta/delete/mixed-load Native Large/XL and Off/Record |
| Semantic and graph domains | R21,R22,R24 | Chunk/ANN production owner, incremental membership/filter/history recall; provenance graph RepoMap | Recall/ranking/query/deletion/old-root/late-job gold; ANN operating bounds; no exact-small-vector or file-map substitution |
| IDE and dynamic effort | R25–R28,R30,R32,R35,R40 | Overlay/isolation, save ACK/session handoff, Stub, coordinates/action fencing; actual IDE activity driving replaceable Auto/Fast/Deep/provider service | UX01–UX12 and AUTO01–AUTO10; typing/cancel/workspace switch/save/delete/late-result correctness and latency/resource isolation |
| Actual Semantica producer and recovery | R37,R38 | Owning compile closure; actual first FULL searchable; fresh Original source and exact V9 ACK recovery; Preparing/Prepared/unbound dispatch; Refused retirement/replacement; Delta/delete/Resync complete affected-set/event authority | P2I01–P2I08 through actual SDK/Native daemon; independent output gold; crash/ACK-loss/restart/no-recollection laws |
| Cutover and integrated qualification | R35,R36,R39,R40 | Native backup/restore or explicit supported rebuild; qualified launcher/default routing; retire competing legacy owners; current coherent suite, faults, performance, CI/release | Source-bound owning/product receipts, restore/crash/rollback, matched performance, complete applicable suite and external qualification |

Latest owning SDK result remains FAILED(exit101), tests0, parser E0282/E0283.
Current source has those type fixes; post-fix owning execution is NOT_RUN.
Contract746/746, Foundation208/208 and Kernel ingress127/127 retain their exact
`accee39d…` source. No new runtime/benchmark was run for this progress accounting.
The live source recheck retained Semantica Native cold guards and earlier-phase
Pending states; Quanta source resync, literal/text/semantic/file-map adapters,
Native service raw constructor paths and launcher selection were inspected.
No partial row was promoted to IMPLEMENTED in this reconciliation.

Execution order: owning dependency/Runtime/SDK checkpoint; controlled allocation
and exact recovery; real producer FULL/Delta/delete/Resync; physical boot/operating
acceptance; remaining promised ANN/graph/IDE domains and coherent performance/
release. Independent domain work can proceed against canonical ports, but shared
IR/constructors and each coherent verification candidate require serialized
integration. Do not reopen accepted A/D/C5 or create a second status ledger.


### 23.77 Parallel delivery plan and exclusive write sets (2026-10-13)

#### Current-source code-first dispatch revision

This revision is the ready queue for the latest request. It supersedes stale
implementation gaps in the historical dispatch below, without promoting any
requirement to accepted. HEAD at inspection was c32cd191554031b9a7d04207ffa3410e8fc201ba;
the shared checkout is dirty and has active writers. The three audits were
read-only; planned new modules are not delivered implementations.

Already present: controlled parser and complete corpus seeds (§23.87–23.89),
Native Text COW/BM25 and bounded batching, semantic whole-file exact/reuse,
file/path RepoMap, replaceable effort policy/estimator/signal ports, common boot
capacity parent, retention/reclaim pumps, and selected boot-manifest custody.
The current boot manifest retains the full CatalogView and protects the request,
profile and three source roots. Do not redispatch these implementations as absent.

Follow-up source recheck: existing cost work already includes opt-in Catalog
statement profiling, prepared-statement reuse, managed-attempt reserve preflight
reuse and validated registered-metadata reuse before edge verification. Keep
these changes; the total-cost packet extends capture/upkeep/reopen coverage
rather than rebuilding profiling. The reused owning command finished with127/127
tests passing across3 binaries,172 skipped in402.385s: Catalog authority and
capture-format laws, Native text/profile gold and the existing ignored compound
64/4096-owner Shared/Distinct scale case. This is not Native XL, actual Semantica
producer E2E, a full suite or a matched performance comparison.

Physical IO work also already has NativeObjectReadIo/NativeObjectReclaimIo over
the common capacity owner. The current lexical_gc.rs retains pending delete
custody and settles it before epoch replacement or cancellation. The active
closeout writer is coupling lexical reads/reuse/scrub and GC/store propagation;
do not assign a second worker to those files. These source changes are not
qualified by the Catalog/Kernel127-test command above. The active layering
writer separately reported199/199 on its earlier boot candidate; newer full-view
metadata changes still require that owner's final affected checkpoint.

Actual producer wiring remains separate: indexing_api.rs defines the explicit
first-FULL method and native_source_publish_v1.rs defines the checked Delta
stager, but the inspected repository has no production caller for these methods.
The synchronous rescan path refuses an absent asynchronous staged-target receipt.
Complete source proof still merges before/after closure iterators in
runtime_mutation_target_build_v1.rs; this is O(N) producer work even when the
Native index edits only changed owners. Sparse proof needs authenticated indexed
transition custody from the existing source owner, not a trusted-by-convention
changed-file list. This cost belongs in the same total-cost packet below.

| Code packet | Remaining implementation and existing owner | Write boundary and dependency | Observable completion |
| --- | --- | --- | --- |
| P0 producer and recovery | Existing producer writer: connect actual product FULL/Delta/delete/Resync callers; remove Native ordinary DTO re-decode, legacy SDK materialization and unadmitted validator scratch using the retained admitted DTO/wire | Semantica existing indexing_api/mutation_api and native_source prepare/publish owners; shared Contract/Base/IPC exports integrated serially. §23.89 defines the coupled recovery changes | Actual producer output reaches Native search; exact ACK/retry/restart and complete changed-set laws; values retain Original grants through terminal dispatch |
| P1 total indexing cost | Root cost owner: extend existing Native cost/profile harness to source staging/capture, cold build, publish, mandatory upkeep and reopen; repair only source-established or measured bottlenecks | Catalog authority/managed_install and shared Kernel cost tests remain root-owned. No generic removal of confirmation checks: registration writes invalidate reserve-time readback assumptions | Independent query gold and failure-prefix laws remain; counters expose N/H/fanout, commits, reads, writes/bytes, retained space and mandatory upkeep; matched timings are a separate gate |
| P1 lexical execution | Independent lexical worker: implement the promised query subset currently refused by native text_query (nonpositive/capped clauses and unsupported planner/filter options) using the existing planner/reader | native/text_query.rs and related text reader/spool; literal.rs remains its existing writer's scope. Root owns changes to the common Kernel test file | Fixed independent token/ranking/query gold, delete/noop/old-view laws and bounded traversal; explicit capability refusals for excluded query forms |
| P1 chunk/ANN | Independent semantic worker: chunk identity/contribution and incremental ANN membership, filtered query, delete and historical-view lifetime | Existing semantic native owner plus new private chunk/ANN modules; root serializes capability/profile/export integration. Existing LanceDB ANN is a reuse candidate after capacity/lifetime feasibility review | Independent exact recall/filter oracle; deletion, stale jobs, old pins and bounded update/query/space; whole-file exact is not ANN completion |
| P1 provenance graph | Independent RepoMap worker: owner-local node/edge contributions, replacement/retraction and graph queries | New private native graph modules under existing RepoMap adapter/Catalog view; root owns domain ports. Reuse legacy validation/serialization only where applicable, not its whole-bundle compiler as a delta engine | Full-versus-delta graph gold, provenance/retraction, pinned roots, no-op and fanout costs |
| P1 boot/deployment | Existing layering writer closes boot custody. Then reconcile deployment write ownership and implement real verify-native-state, exact serve identity arguments and supported Native restore or explicit new-root rebuild | Catalog boot/native.rs/native_product.rs stay with their current writer. CLI command, launcher and state_migration require a single reconciled deployment writer | Actual Rust verify/select/serve; wrong root/config/source/operation rejection before mutation; later update/restart; supported restore/rebuild and rollback |
| P2 IDE and Auto inputs | Editor worker after a slot frees: unsaved overlay/isolation, save ACK/session frontier, Stub and coordinate/action fencing; real activity and quality signals through existing ports | Session/editor adapter modules; SourceCAS/save joins serialized with producer; native_product config hunks integrated by its existing writer | UX01–UX12/AUTO01–AUTO10, cancellation/workspace switch/late-result laws; stale/unknown signals remain typed. quality_confidence_bps is currently Unsupported |

Concurrency is root plus three workers, not all rows at once. Existing other-chat
producer/boot/physical writers continue without duplicate assignments. Within
this chat the first independent implementation wave is lexical execution,
semantic chunk/ANN, and provenance graph; root owns total-cost work and shared
integration. Before edits, confirm the exact current file/hunk ownership. No
worker edits Core/Contract/config exports, native_lifecycle.rs or this RFC;
workers supply focused behavior tests in their owned modules and the integrator
couples shared callsites and integration gold. Avoid a replacement scheduler,
second publication head, parallel IR or benchmark ledger.

Long-pin, WAL/scratch/space competition, reader unlink, sustained drain, Native
XL, quiet matched performance, full suite and release are verification gates,
not known missing algorithms. Conservative cleanup WorkLimits and logical-byte
inventory do not establish universal cleanup headroom, device-block, RSS or FD
guarantees. A discriminating failure may create a new owner-local repair task.
Do not make a new resource architecture solely because acceptance is unrun.

Checkpoint order: integrate interfaces/callers and owner-local unit laws; batch
the affected strict/unit checks; run actual FULL→Delta→delete/restart integration;
then batch domain/operating gates and Native Large/XL. Reuse live verification
sessions instead of starting duplicate builds. This plan does not change the
last original requirement tally of18/40; a new exhaustive recount is separate.

#### Live ownership correction before dispatch (2026-10-13)

Both actual implementation checkouts are on main. Quanta has346 dirty paths at
inspection; most new work has not been committed. Semantica has multiple live
writers and its main HEAD advances through a separate commit owner. Detached
proof/publication worktrees also exist; their results are not automatically
main integration or origin/main publication.

The app's current thread records and Semantica shared-main status were inspected
read-only. They supersede a proposed fresh assignment where the work already
has an active writer. No other desktop chat was messaged or reassigned.

| Observed thread title | Current observed scope | Consequence for this plan |
| --- | --- | --- |
| 증분 | Semantica Native producer/CBOR/recovery integration planning and source-bound proof, current progress/RFC sections | Add A and non-overlapping B only after exact claim; shared Contract/export/config and SDK callers need integration custody |
| 증분 (2) | Native Literal posting identity/child/cancellation repair in literal.rs, coupled regressions/strict; previous map custody repair | Do not launch a second Literal/map repair owner under D; remaining Native cost/domain work needs an explicit separate write set |
| 인덱싱 레이어링 어딧 | Managed constructor/namespace-lock lifetime, native_scratch_io/storage namespace/Core ports, native_object_write_io and actual census; Native boot installation next | C already has an observed writer; remove C from fresh worker dispatch and consume that owner's actual handoff |
| Review Oct 10 index closeout --보완 | Common publication capacity/VFS/Lance/sidecar/read-copy admission, vendor adapters, native_object_write_io test/lint and metadata; source-specific strict/affected runs | Do not rebuild ledger/VFS/sidecar; common IO/test file edits need hunk-level reconciliation with layering writer |
| 증분 테스트보완 | Genuine Original/PTA/query output producer migration and SDK source authority; active shared-main claims | B must not directly mutate the claimed indexing_api.rs/mutation_api.rs until existing owner coordination is recorded |
| 커밋하고 main에 푸시 | Separate Quanta publication task, currently idle; observed main dirty work is pending | main checkout is not proof that new changes are committed/pushed |

Confirmed shared file is native_object_write_io.rs between the physical and
layering chats. Recent physical changes include test assertion/lint repair;
layering owns the admitted constructor/real census law. Shared documentation
RFC/README/VALIDATION is also edited by several chats. This establishes overlap
requiring ownership, not a reproduced lost edit or same-hunk conflict.

Semantica status showed17 live claims and no integration barrier. Active
PTA-CALLABLE-CONSUMER-A-20261013, owned by the 증분 테스트보완 chat, includes
quanta-sdk-runtime-executor/src/client/indexing_api.rs and mutation_api.rs.
The checked canonical_cbor/admitted_decode/writer readback and Kernel
native_source_validation/native_source_publish paths had no matching live
claim in that snapshot; absence is not mutation authorization or assurance
that no unregistered writer exists. Acquire current exact claims before writes.

Corrected ready queue: A decoder; B non-overlapping Kernel/source-proof pieces;
existing C and D physical/Literal writers continue under their current tasks.
SDK B caller/recovery integration waits for documented same-file reconciliation
with the existing claim; default source presence or broad planning is not that
reconciliation. New independent ANN/graph/IDE slots still require the same live
ownership check. Shared status docs use narrow section hunks; one final coherent
integration owner updates the summary, captures proof and serializes publication.

This is a statically audited implementation plan, not implementation acceptance.
§23.76's 18/40(45%) code tally is unchanged. The available concurrency is one
integrator plus three workers. Eight lanes below are a ready queue, not eight
simultaneous agents. Keep original R01–R40, UX01–UX12/AUTO01–AUTO10 and P2I01–P2I08.
No independent desktop chat is messaged or assigned by this plan; active dirty
hunks must be reconciled with their actual writer before dispatch.

#### Stage0: freeze interfaces and real write ownership

The integrator owns shared Contract/IR decisions, config/schema additions, common
crate exports, cross-owner callsite hunks and checkpoint integration. Each actual
file/hunk has one writer; planned modules are not assumed to already exist.

| Seam | Existing authority to keep | Contract to freeze before worker edits |
| --- | --- | --- |
| Decode/read allocation | OriginalNativeMemoryScopeV1, MemoryScope/MemoryLease, ReplayVectorAdmissionV3, canonical CBOR owner | Reserve before parser/visitor buffer growth, count temporary plus retained live allocations, returned value keeps Original leases; typed first refusal and canonical wire/golden parity. No DeserializeOwned wrapper or wire-size multiplier claims admission |
| Native changed-set | Existing SourceCommitManifestV1, admitted source transition/closure, NativeSourceTargetV1, NativeSourcePreparationIntentV1 and NativeSourceBoundUpdateRequestV1 | Checked borrowed source delta binds complete added/modified/removed set to exact source base/target, policy and Lexical replacements/tombstones. Freeze accessors and lifetime, not a twin IR/journal |
| Physical service boot | PublicationBootInventoryLimits, seed_publication_capacity, PublicationFileCapacity and existing IO ports | Explicit byte/file/pending/reader ceilings and inventory bounds; common owner present before selected writers mutate state. Logical file length is distinct from device allocated blocks/free space and global FD/RSS |
| Adapter/domain capability | Existing IndexOwnerBuildPort/query registry and Catalog source/work/publication owners | Domain identity, coverage, old-root/delete/late-job, bounded preparation and query result custody. Share instance/capability validation; no separate backend lifecycle owner |

Run one current dependency/SDK owning checkpoint against a captured source after
known compiler repairs; do not require unrelated full-suite success before all
independent implementation. Reuse the existing lane/front door. Separate source
capture from ongoing LIVE edits; do not repair an immutable captured tree.

#### First dispatch: A / B / C in parallel

| Lane | Exclusive production write set | Implementation sequence | Required first acceptance |
| --- | --- | --- | --- |
| A — controlled CBOR and source read custody, R11/R12/R37 | Semantica `packages/core/ir/foundation/src/canonical_cbor*`, `quanta-contract-retrieval/src/indexing_writer_ports/admitted_decode_v3*`, artifact/receipt serde hunks; Quanta `quanta-index-contract/src/ipc/ingest/corpus_wire.rs` and its existing bytes visitor | Extend the same canonical decode owner with seed/before-growth hooks or controlled borrowed materializer; fund parser-owned strings/bytes and visitor containers before birth; propagate genuine retained leases through artifact/receipt and Lexical batch. Existing Ciborium allocates before Visitor, so choose real decoder-owner integration before a seed facade | Oversized/truncated/nested/unknown/duplicate CBOR, pre-allocation denial, grow/refund/live-peak laws, exact V3–V5 gold, typed cancellation; whole-file bytes and returned batch lifetime covered |
| B — actual Semantica producer, R37/R38 | `quanta-sdk-runtime-executor/src/client/{indexing_api.rs,mutation_api.rs}`, private cold retry; `quanta-runtime-retrieval-kernel/src/index_sdk_ingress/native_source_{validation,publish}_v1.rs`; original source-commit issuer/composition hunks | First FULL eligibility and exact target retry; borrowed complete changed-set proof; Add/Modify/Remove staging using exact old owners and changed bytes; durable prep→seal→write-once binding→Submit→typed ACK; existing SourceCAS/journal; then Resync and Refused retirement | Genuine Original+real daemon first FULL/current search, exact same-target retry, old-view preservation, modify/delete/rename/noop, missing/extra changed rows, foreign closure, CAS/ACK-loss/restart; keep Native guards until the real join passes |
| C — common physical owner at Native boot, R10–R12/R35 | `quanta-index-searchd-runtime/src/native.rs` constructor/resources; approved config hunks in `native_product.rs`/searchd config; existing `publication_inventory`, file capacity and IO modules only where actual gaps require repair | Component76-test, public boot6-test and partial-batch37-test checkpoints predate reserve changes. Cold Catalog/object/Kernel-session creation now uses the held parent; empty-lock failed settlement records real retirement without refund. Reserve runtime7 passes103/103, including unchanged-cap recovery and ordinary admission. Finish latest strict, supported cleanup-round headroom and operating acceptance; preserve root/marker mutation boundaries | Public boot census/reopen and over-cap preservation; invalid config before mutation; same byte/file cap refusal→discard→close→reopen→GC→ordinary admission; actual WAL/scratch/object competition, old reader/pin unlink, release-once, bounded cleanup resources and failure prefixes; no raw constructor fallback |

A and B must not both edit Runtime writer/recovery assembly. The integrator alone
combines `prepared_commit_artifact_v2.rs`, `prepared_artifact_readback_v3.rs`,
`prepared_member_revalidation_v3.rs`, `terminal_aggregate_readback_v2.rs`,
`prepared_aggregate_owner_pipeline_v1.rs` and shared hunks in
`ordinary_aggregate_prepare_v2.rs`. Workers deliver typed read/dispatch inputs;
this integration removes repeated unscoped decode/member read and retains leases
with custody. Preparing/Prepared/Ready/Committed/unbound Dispatching use the same
V9 owner, not recollection. Retained terminal Refused needs explicit replacement
/retirement authority before another operation can be issued.

B's current complete source proof uses exact before/after closure comparison
(`quanta-core-contract/src/ports/artifact_registry_port/incremental_manifest_source_commit.rs`
and composition `runtime_mutation_target_build_v1.rs`). This O(N) producer cost
is a separate obligation: instrument it, then derive sparse completeness from
existing source owner indexed transition/COW custody if supported. Never replace
complete proof with an untrusted changed-file list. Index-native incremental
behavior alone cannot certify end-to-end changed-size work.

#### Next available slots: D / E / F / G / H

As a worker finishes or reaches a genuine dependency wait, release that slot to
the highest ready lane below. Do not wait for all A/B/C to finish before starting
independent domains; do not keep an agent polling another lane.

| Lane | Existing owners / allowed new modules | Dependencies and concrete work | Acceptance |
| --- | --- | --- | --- |
| D — Lexical parity and total indexing cost, R13/R15–R17/R39 | lexical `native/{text,text_query,text_reader,text_spool}.rs`, existing planner/normalizer; existing scale/scale_matrix, causal_cost_capture and native_noop_cost owners | Preserve existing BM25 in text_query; map actual regex/DSL/token gaps to fixed gold before repair. Add Native mode/cost observation to existing harness; no second runner/ledger. Separate producer closure proof, Catalog/WAL/object/scratch, publish/upkeep/GC, cold/reopen/freshness. Optimize only an observed or source-established cost | Fixed independent rank/query gold; FULL/one-file/noop/delete across N/H/fanout, total mandatory upkeep, writes/bytes/fsync/retained space and query tails; no legacy scale promotion |
| E — chunk/ANN domain, R21/R22 | semantic `native/embedding.rs` and planned native chunk/ANN adapter modules; existing provider and registry | Keep whole-file exact route where supported; build actual chunk identity/membership/ANN behind existing typed adapter, Catalog and physical/read custody. Vendor feasibility/controlled growth must precede large-Semantic enablement | Independent recall/filter/history/deletion/old-pin gold, stale jobs and coverage, bounded update and query cost; exact-small-vector is not ANN acceptance |
| F — provenance graph RepoMap, R24 | native file_map remains its own adapter; planned graph delta/read module; inspect legacy materializer compiler for suitable reuse | Graph owner/reference/provenance delta under one Catalog view; no file-map relabel or second publication head. Integration waits only for frozen domain ports | Full-vs-delta graph/query gold, replacement/retraction, old roots, large fanout and bounded preparation |
| G — editor/Stub/Auto, R25–R28/R30/R32/R35/R40 | existing session/query/effort contracts; planned actual editor producer/Stub owner; native_interaction/native_auto_signals and provider composition | Reuse current EffortPolicy/estimator/governor and session mechanics. Implement unsaved overlay/isolation, save ACK/frontier, Stub and action/coordinate fencing; real IDE activity/quality signals. Share SourceCAS/save interface with B, config/constructor with integrator | UX01–UX12/AUTO01–AUTO10; edits/cancel/switch/save/delete/late results, quality Unknown/stale, policy replacement, provider fairness and same-intent budget |
| H — Native verifier/restore/default cutover, R35/R36/R39/R40 | searchd `cli/command.rs`, existing launcher and state_migration owners; planned Native restore/rebuild owner only if required | After C boot identity and B exact recovery, couple launcher verifier arguments to actual Rust command, qualify Native backup/restore or explicit supported rebuild, default routing/rollback and legacy retirement | Verify/select/serve actual process, receipt/config/root drift, symlink/foreign-source refusal, supported restore crash/rollback; then coherent CI/release |

Observed H seam: launcher invokes `verify-native-state`, but current
`searchd/src/cli/command.rs` has serve-native only and no matching verifier branch.
The launcher also passes root/manifest/source identity flags to both commands;
the current parser rejects those unknown flags even for serve-native. H must
couple both exact argument sets and real identity checks, not just add the
verifier's command name. Shell fixture acceptance is not real daemon acceptance.
Source presence of launcher selection is not product qualification. C inventory
records regular-file logical length only; do not advertise physical block reserve
or all-process FD/RSS enforcement. Existing `scale_matrix` follows legacy
E2eRuntime and needs a genuine Native mode before any Native cost claim.

H's implementation order is a contract dependency, not a parser-only patch:

1. Freeze each receipt field's issuer and persisted source with the existing
   Catalog/publication owner. The later v13 checkpoint below adds a once-issued
   root nonce to the existing capture-format row and its bounded getter; it does
   not yet issue `manifest_operation`, `manifest_object` or `source_cut_digest`
   matching the launcher. The actual Catalog already persists operation receipts,
   source/view bindings and a physical namespace binding. Bind the selected
   receipt to these owners; if a durable incarnation field is required, place it
   in the existing owning record. Do not add a second root registry or treat
   marker bytes, caller JSON or an inode-only lock as an issued boot receipt.
   Selected boot-manifest custody must also survive normal retirement: current
   terminal-custody release can delete `native_operation_object_v1` references
   and enqueue their objects independently of retaining the logical receipt.
   Protect the exact selected manifest and required source/profile closure through
   the existing Catalog lifetime authority until explicit reselection/retirement.
   A readable operation row, root nonce or view pin alone is insufficient evidence
   that the original request object will remain authenticatable after later updates.
2. Implement a coherent read-only verifier for an existing root under exclusive
   root custody. Check config/schema/format, selected original operation and
   immutable object/source bindings against one consistent Catalog snapshot;
   authenticate the referenced immutable objects. A wrong or missing binding
   must fail before DDL, marker creation, namespace rebinding or socket binding.
   Current component APIs are `verify_native_boot_catalog` (Catalog schema,
   nonce, selected publication/custody and saved namespace only) and
   `FileObjectReader::open_existing`/`get`/`verify_closure` (existing namespace,
   canonical immutable bytes and bounded child/leaf wire closure only).
   Closure traversal pre-admits its explicit pending-frontier bound; the same
   original budget bounds all visits/bytes and cancellation. Shared children
   may be reread, without an unbounded visited set. These physical readers do
   not replace Kernel request/profile/domain validation. They reuse the existing publication VFS and object
   decoder; do not implement either reader again. The Catalog scope prohibits
   source creation/writes/delete/truncate/SHM, uses exclusive SQLite locking
   with a memory WAL index and disables checkpoint-on-close before schema/WAL access.
   Native WAL reads carry the original cancellation/deadline. The main FD needs
   write permission for POSIX locks; a read-only filesystem is allowed to refuse.
   Native main xOpen acquires the actual SQLite exclusive lock before its header
   read. Only an absent/zero-byte WAL permits a main-only immutable URI, avoiding
   empty-WAL birth; nonempty WAL must never be hidden. Check WAL presence and
   main/WAL identity again after close. The two component outputs are not a complete
   issued receipt: the existing boot/deployment owner must authenticate the exact
   selected request/profile/SourceCut closure and config, check the install-time
   Catalog content digest, and retain the same root custody into serving.
   Component behavior/strict status belongs to VALIDATION, not this contract.
3. Couple both exact launcher argv sets to this verifier. The install-time
   whole-Catalog digest is checked during selection; it is not a perpetual
   startup equality condition after legitimate updates. Serve still validates
   the durable root and original receipt before mutating boot. Preserve typed
   identity failure and reject missing, duplicate, malformed or mixed-mode args.
4. Qualify with the actual Rust daemon and launcher: valid verify/select/serve,
   wrong root/config/operation/object/source, successful later update/restart,
   live-owner refusal and unsupported restore/rebinding. Current plain
   serve-native publish/query/reopen6-test acceptance does not cover this seam.

These steps refine lane H's existing R35/R36/R39/R40 obligations. They do not
authorize a new scheduler, backend framework, perpetual dual format or a stub
that accepts identity flags without checking their authoritative values.

#### Checkpoints and delivery discipline

- CP0: interface/ownership freeze and compiler dependency baseline.
- CP1: A parser/Contract/batch allocation laws and C constructor boot laws; B
  changed-set/source authority laws. Capture one coherent candidate per proof
  closure, then run the owning tests through existing front doors.
- CP2: coupled actual first FULL→search, one-file Delta, delete, rename/noop and
  replay/restart/old-view/freshness. Earlier-phase V9 and SourceCAS fault cuts
  included; a fake provider or fixture grant cannot close P2I.
- CP3: ready domain capability golds and physical failure/long-pin/sustained
  operating matrices. ANN/graph/IDE remain part of the full promise.
- CP4: Native Large/XL and matched cost/Off-Record, coherent applicable full suite,
  supported Native deployment/restore, remote CI and release qualification.

Implementation and static review proceed in parallel. Workers author necessary
independent regression tests with their behavior changes; the integrator batches
affected Rust/strict/runtime rails at these checkpoints. Test filters do not
narrow lib(test) compilation. Heavy host-sensitive benchmark/QBC custody runs
are serialized; this is not a blanket compiler jobs1 rule. New failing closure
is repaired as a batch, unrelated old greens are not rerun merely due HEAD drift.
No 100% code or release claim until original exit conditions are met. Update the
existing RFC requirement ledger/VALIDATION/README; no repeated planning-only wave
without an explicit contract blocker or new material evidence.


