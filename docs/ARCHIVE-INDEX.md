# Documentation and Ticket Recovery Index

Status: `HISTORICAL RECOVERY INDEX`

Accepted contracts live in [ADRs](adr/README.md); current Index work lives in
[Oct-22 SSOT](plans/oct-22-finalizing/README.md).
Historical commands, counts and receipts do not qualify newer source.
Dated sections describe their then-current paths. The Oct-22 epics own full-product scope;
[Nov-5 bugbash](plans/nov-5-indexing-bugbash/README.md) owns current Native indexing performance RCA and new packets.

## Historical record recovery

### 2026-11-08 November audit ticket closeout

Implemented contracts are consolidated in
[NOV-07-001](adr/NOV-07-001-native-index-execution-and-custody.md).
P01–P03 and P05–P07 were retired from the audit queue on the evidence below.
The [audit ticket index](plans/nov-7-repo-audit-closeout/tickets/README.md)
records the current P04 daemon counts, P08 Sidecar custody and P09 IPC acceptance.

| Closed scope | Source-scoped execution |
| --- | --- |
| P01 Native ANN | Working tree based on `7a16f48a2`. `./scripts/cargow nextest run --locked -p quanta-index-semantic --lib --no-tests fail -E 'test(ann_store) \| test(ann_lance)'` ran 16/16 with 228 filtered out. Nextest run `86fdd938-9fcd-47aa-b177-88ff49480752`. |
| P02 Go trace-file reply | G69 reproduced the duplicate chunk ID across distinct files. G70 passed dispatcher 39/39 and ordinary Hybrid runtime-risk 2/2 on its fixed input. The [execution record](plans/oct-22-finalizing/VALIDATION.md#source-file-identity-repair--2026-11-06) owns the exact source and ranking scope. |
| P03 read sessions | Working tree based on `7a16f48a2`. Kernel 23/23, Registry 9/9 and UDS 2/2 passed, with 713 filtered out. The SQLite witness counted six lease checks in the prior borrowed path and five in the shared first-stage path. Expiry before reservation caused zero reader reservation and query calls while the original pin survived. |
| P05 Native boot, P06 Source member authentication, P07 Map batch | The combined P04–P07 focused run passed 86/86 across six crates. The real Native boot selection, verification and serve case passed 1/1. The two SDK publication and restart cases also passed on the matched daemon, including eight named crash cuts. Those SDK cases cover P04 behavior. |

Exact copies of the six retired untracked tickets and the pre-compaction P04
ticket are in the local Codex artifacts directory
`quanta-index-nov-08-doc-preimages/audit-tickets/`. The original selectors and
implementation plans are recoverable there.

### 2026-11-07 November closeout

Scope: November 1–7 KST, through `7a16f48a20e2af2a37ed16847509bf6208dc8a6b`.

| Changes | Current authority |
| --- | --- |
| `484e796d3` retired 39 superseded Oct-10/13/17 forwarding pages | [Original recovery mapping](#2026-11-05-forwarding-page-retirement). |
| `d7df99efe`, `de16b193b` corrected completed IPC write settlement, hangup cancellation ordering, typed causes and platform inventory counters | [IPC and custody ADR](adr/NOV-07-001-native-index-execution-and-custody.md). |
| `48666a746` integrated Native indexing, catalog installation, Go trace file ranking, SQLite diagnostics and the pinned rusqlite interface | [Native ADR](adr/NOV-07-001-native-index-execution-and-custody.md) for implemented contracts. [Fixed acceptance scope](plans/oct-22-finalizing/DOD.md) and [current E02](plans/oct-22-finalizing/README.md#e02) for execution. |
| `c5370025b`, `7a16f48a2` updated lexical profiling and Native indexing RCA/workplan | [Native bugbash](plans/nov-5-indexing-bugbash/README.md) owns current source, bounded proofs and measurements. The last recorded Large Off attempt `lo12` failed the original 120-second bound. The subsequent diagnostic build and Large Record were not run in that checkpoint. |

The Oct-28 E02 write map, lane assignments and nine dated ticket bodies were
retired. The [remaining E02 routes](plans/oct-28-e02-native-closeout/README.md)
retain open GC/lifecycle, reuse, DELTA, Provider and owning/installed acceptance.
P01 Acked closed on [its selected cohort](plans/oct-22-finalizing/VALIDATION.md#e02-deduplicated-current-work-20261030-root).
The old caller-zero successor diagnosis and the borrowed Provider path as the
only configured route were superseded by the actual DELTA and owned Host paths.
The exact original fourteen files, including the older Provider and verification
instructions, are recoverable from the pre-edit revision:

```sh
git ls-tree -r --name-only 7a16f48a2 docs/plans/oct-28-e02-native-closeout
git show 7a16f48a2:<repository-relative-path>
```

For Native lexical work, the 4,096-byte/128-row occurrence policy and the
4 KiB single Posting Block candidate are retired. The candidate that retired
all block work on any cold-byte increase was also withdrawn. The
[current workplan](plans/nov-5-indexing-bugbash/WORKPLAN.md) retains the original
metric-region and mixed-query bounds. Focused lexical regressions, cached pair
builds and fixed 512 MiB Medium diagnostics have separate scopes from Large,
XL, mixed-load and release acceptance.

#### Nov-7 audit P10

`VERIFIED` on the working tree based on `7a16f48a2`. P10 fixed five links
(benchmark E07 interpretation, three references to the shared Native bugbash
source-availability anchor, and vendored rusqlite SQLCipher license) and four
EOF whitespace findings (three history pages and the process monitor test).
`just lint-doc-paths`, `git diff --check` and
`git diff --check c32cd1915 -- . ':!vendor'` passed. The closed ticket and
dispatch copy were retired from the audit queue. Their exact untracked bodies
were saved in the local Codex artifacts directory
`quanta-index-nov-07-doc-preimages/P10/`. This was a documentation check.

#### Plan and ticket deduplication

At this checkpoint, the nine Nov-7 audit tickets retained their original
paths and acceptance. The ten forwarding documents under
`docs/plans/nov-7-repo-audit-closeout/tickets/packets/` were retired. Their
distinct commands and shared handoff requirements were moved to the tickets
and their index. The exact untracked copies are in the local Codex artifacts
directory `quanta-index-nov-07-doc-preimages/audit-packets/`.

The [current wave plan](plans/oct-22-finalizing/WAVES.md) retains W0–W6, the
fixed [product DoD](plans/oct-22-finalizing/DOD.md), current E02 join order and
the incoming anchors. The dated execution tables, old Provider readiness
assumption and source-specific proof counts are recoverable with
`git show 7a16f48a2:docs/plans/oct-22-finalizing/WAVES.md`.
The [E04 plan](plans/oct-22-finalizing/E04-semantic-ann.md#acceptance-and-rca-routing)
links directly to the original ANN 4K fixed-cohort result and keeps operational
acceptance in E07. The older October failure summaries are recoverable with
`git show 7a16f48a2:docs/plans/oct-22-finalizing/E04-semantic-ann.md`.

### 2026-11-05 Native indexing RCA consolidation

Current RCA, unresolved decisions and implementation/test packets live in
[Nov-5 bugbash](plans/nov-5-indexing-bugbash/README.md).
The prior [E07 body](history/nov-5-indexing-bugbash/E07-performance-operations.md)
and [E07 wave section](history/nov-5-indexing-bugbash/E07-WAVES.md) retain their
dated content with relative links rebased to the archive location.
The E07 contract entry retains Fixed profiles and DoD. Existing wave anchors
resolve to their historical bodies. Source-bound VALIDATION results remain
at their original paths. This migration adds no runtime or performance proof.

### 2026-11-05 forwarding page retirement

The 39 Oct-10/13/17 forwarding pages were removed after their in-tree links
were redirected to current epic owners or the original historical sections.
Their exact pre-removal pages and fragments are recoverable at
`90a0ec340388400828c193809e533de2b1fb9001`:

```sh
git show 90a0ec340:<repository-relative-path>
git ls-tree -r --name-only 90a0ec340 docs/plans
```

The [Oct-22 epics](plans/oct-22-finalizing/README.md) remain the execution owner.
The [Native RFC](rfc/oct-10-native-incremental-vnext.md) and
[R01–R40 DoD](plans/oct-22-finalizing/DOD.md) retain their acceptance scope.

### 2026-10-07 dated packet retirement

The final dated RFC/plan/ticket retirement on 2026-10-07 uses pre-deletion revision
`4f07c3c5ac57f161aa6870a5cc661b0038932993`. It removes 39 Markdown files under
`docs/plans/` plus the redundant `tickets/README.md`; the residual ledger was
retained at that checkpoint and is superseded by the Oct-10 plan below.
The executable handoff schema moved unchanged to
`tools/ci/lane-handoff.schema.json`.

Recover an original body into a separate directory:

```sh
git show 4f07c3c5:<repository-relative-path>
git ls-tree -r --name-only 4f07c3c5 docs/plans tickets
```

| Retired packet | Current contract / remaining owner |
| --- | --- |
| Sep27 code-search RFCs, Sep30 B01–B09 | [Review/corpus/statistics](adr/OCT-05-001-review-admission-and-result-identity.md), [native/update](adr/OCT-05-002-native-capture-clock-and-index-scope.md), [cost/host](adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md); E1/E2/E4/I0 |
| Sep21 residual plan, S21-11/12/13, prompts/handoff usage | [Proof/backup/custody](adr/SEP-27-005-catalog-recovery-supervision-and-proof-custody.md), [installed/pair/actions](adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md#installed-paired-and-operational-acceptance), operator runbooks and executable registry; I0 |
| Jul15 QIT, Sep27 MISC and root ticket index | [Selected regression/platform](adr/SEP-27-005-catalog-recovery-supervision-and-proof-custody.md#selected-regression-and-platform-acceptance), [CI provider](adr/SEP-28-001-circleci-provider-and-credit-boundary.md); test/platform/E1/E2/E4 |
| Jun7 J7Q quality and May25 semantic | [Consumer preview/operator](adr/SEP-27-005-catalog-recovery-supervision-and-proof-custody.md#consumer-preview-and-operator-acceptance), [semantic generation](adr/MAY-31-001-lancedb-semantic-generation-authority.md), cost/review ADRs; semantic/E1/E4/I0 |
| Oct4 owner map / Semantica pointer | [Current ownership](plans/oct-22-finalizing/OWNERSHIP.md#exclusive-boundaries), [consumer acceptance](history/oct-22-finalizing/oct-10-index-closeout/VALIDATION.md#release-and-consumer); external producer fact resolution stays producer-owned |

Retired IDs and their original task bodies are recoverable from the revisions
below. They are not additional active tasks; current validation is in the
[closeout validation plan](plans/oct-22-finalizing/VALIDATION.md).
Completed source-bound cache/lifecycle/F15/CI/Medium/Ready9 checkpoints remain in
those ADRs; deleting a packet does not issue fresh execution or qualification.
Unadmitted numeric hardening/holdout targets remain historical proposals.

## Oct-10 plan retirement

Pre-deletion committed revision: `de279c45d0a24cc6bb87a478dc20cda80b5284d7`.
The user requested replacing the old work documents with a new Index-only plan.
Removed bodies:

- `docs/plans/oct-4-parallel-closure/tickets/INDEX.md`
- `docs/plans/oct-8-incremental-index-hardening/README.md`
- `docs/plans/oct-8-incremental-index-hardening/VALIDATION.md`
- `docs/plans/oct-8-incremental-index-hardening/REFERENCES.md`

Recover committed bodies with `git show de279c45:<repository-relative-path>`.
The latest uncommitted scope-only refinement is incorporated in the new plan;
the pre-deletion revision is not claimed to contain that later edit.
Candidate/proof chronology is historical; accepted ADRs and their source-bound
results remain. The Oct-10 plan took over at that checkpoint; its remaining
work is now routed through the [Oct-22 epics](plans/oct-22-finalizing/README.md)
and [source-scoped validation](plans/oct-22-finalizing/VALIDATION.md).
Deletion does not mark pending code, quality, performance or release accepted.

## Earlier recovery boundaries

- `6a3f6afc8c286176962e722ce75524aefcfa7607`: earlier May–Oct05 documentation
  and packet consolidation, including the original `docs/ARCHIVE-INDEX.md` and
  `docs/plans/ARCHIVE-INDEX.md` mappings.
- `a5e87614bee79bf9d67c8358df01313242548798`: earlier Oct07 completed OCT04
  status detail and B07 execution chronology.

Read those historical recovery indexes with `git show <revision>:<path>` for
exact older source/artifact identities and removed-path sets. Dirty/untracked
Sep27 preimages require the external content backup named there; Git cannot
recover never-committed bytes. Temporary external backups are not durable history.
Missing old native/raw/review roots cannot be recreated from hashes or counters.


## Oct-14 execution SSOT consolidation

Current tasks/owners/order: [Oct-22 SSOT](plans/oct-22-finalizing/README.md).
The Oct10 README/NI queue and Wave2 parent/index/ownership/ticket dated status
passes were consolidated. Ticket IDs, unique scope/acceptance contracts,
completed reuse boundaries and the existing VALIDATION results remain.
RFC requirement/IR contracts and Accepted ADR decisions were not replaced.
No pending implementation, measurement, product or release gate was closed.

Pre-edit committed base: `c32cd191554031b9a7d04207ffa3410e8fc201ba`.
That base does **not** contain all dirty/untracked preimages. Their verified
external custody is:

- Archive: `/Users/songmin/.codex/artifacts/qidx-document-custody-20261014T132216Z/preimages.tar.gz`
- SHA-256: `9980c2b5eaff6ed3d8c7745befe7eaa9b1aa24852363bb4c0b80b0798cd5cc9a`
- File hash manifest: `/Users/songmin/.codex/artifacts/qidx-document-custody-20261014T132216Z/manifest.json`
- Coverage: 65 Markdown preimages, including untracked Wave2/NI documents.

Recover to a separate directory with `tar -xzf <archive> -C <recovery-directory>`.
The archive was read back and every member SHA-256 matched before edits.
Keep this external archive with the repository history until a durable committed
custody transfer exists. It is historical recovery, not another execution queue.

### Retired execution prose

The following historical bodies are now stored as reference documents. Current
paths preserve their original headings and link to the moved text so existing
deep links remain usable. Only the Oct-17 SSOT owns actionable status.

| Original entry | Preserved body |
| --- | --- |
| Oct-10 audit | [Historical audit](history/oct-10-index-closeout/AUDIT.md) |
| Lexical plan sections 7 and 10 | [Execution order and checkpoints](history/oct-10-index-closeout/LEXICAL-CHECKPOINTS.md) |
| RFC initial section 23.55 register and sections 23.75–23.77 | [Delivery/progress/assignment history](history/oct-10-index-closeout/RFC-DELIVERY-CHECKPOINTS.md) |

These moves preserve technical rationale and source-scoped observations. They
do not retire unresolved acceptance or qualify current behavior. Do not append
fresh tasks, percentages, owner assignments or results to the historical bodies.

## Oct-17 residual SSOT consolidation

Current tasks/owners/order live only in [Oct-17 tickets](plans/oct-22-finalizing/README.md). The former19 W2 entries map to7 active residual tickets; the mapping preserves every old ID without reopening completed subfeatures. Existing validation-only gates are consolidated into W2-19, producer outputs into W2-05, and conditional upkeep into W2-09.

The pre-migration dirty/untracked Oct-13 ledger is preserved in [RESIDUAL-LEDGER](history/oct-13-native-incremental-wave2/RESIDUAL-LEDGER.md). At that checkpoint, previous W2 specification and handoff bodies remained at their original paths with historical-reference banners. The Oct-18 retirement below moves those bodies into history and keeps anchor-preserving entrypoints. Their TODO/status statements do not create work. Existing source-bound failures/results remain in VALIDATION; migration is not acceptance.


## Oct-18 final ticket audit and history retirement

Current tasks/status/owners/next gates remain in the seven-row [Oct-22 SSOT](plans/oct-22-finalizing/README.md). Active tickets retain current contracts and acceptance boundaries; they no longer accumulate version-specific commands, candidate suggestions, test counts or next-run chronology. At that checkpoint VALIDATION remained append-only execution history; the Oct-21 compaction below separates its current summary from immutable dated history.

| Retired text | Preserved history | Original path |
| --- | --- | --- |
| Oct-17 ticket pre-cleanup bodies | [Ticket history](history/oct-17-index-closeout/tickets/W2-09-cost-observability.md) and sibling seven-ticket bodies | Current tickets retain current contracts; previous heading anchors remain |
| Oct-13 nineteen ticket specifications and write ownership | [Wave2 history](history/oct-13-native-incremental-wave2/tickets/W2-01-outer-artifact-admission.md) and sibling files | Anchor-preserving link entrypoints only |
| Legacy lexical/query/store and trace plan/packets | [Lexical history](history/oct-10-index-closeout/LEXICAL-QUERY-PLAN.md), [store contract](history/oct-10-index-closeout/LEXICAL-STORE-CONTRACT.md), [trace plan](history/oct-10-index-closeout/TRACE-WATERFALL-PLAN.md), [trace packets](history/oct-10-index-closeout/TRACE-EXECUTION-PACKETS.md) | Anchor-preserving link entrypoints only |

Bodies were preserved from the shared dirty/untracked documents, with relative links relocated to their original targets. This is document maintenance, not a new runtime receipt, source qualification, code change or closure of Native18/4K/producer/operations/release. Prior Native15–17 cost floors and debug/optimized failures remain historical; original4K per-request caps remain the current product condition. Current external S compiler/caller acceptance was not rerun, so its prior accessor failure is not asserted as a fresh failure. Do not update historical TODOs or revive rejected micro-cache/prefix-reuse proposals as new assignments.


## Oct-21 closeout document compaction

Current state/owners/next gates remain only in the seven-row [Oct-22 SSOT](plans/oct-22-finalizing/README.md). Current source-scoped execution results live in the compact [validation summary](plans/oct-22-finalizing/VALIDATION.md). No runtime,benchmark or release was performed for this document change.

| Preserved body | Source path | Recovery |
| --- | --- | --- |
| [Full validation through 2026-10-21](history/oct-10-index-closeout/VALIDATION-2026-10-21.md) | `docs/plans/oct-10-index-closeout/VALIDATION.md` | All existing failure/timeout/repair/pass records and heading anchors preserved; relative links rebased to their original targets |
| [Previous SSOT and dispatch chronology](history/oct-17-index-closeout/tickets/README-2026-10-21.md) | `docs/plans/oct-17-index-closeout/tickets/README.md` | Candidate versions,dated status additions and duplicate dispatch tables are historical,not a second current queue |

### Ownership and caller-summary follow-up

Historical child assignments, old active W/M/I roles and the dated dependency join moved to [ownership history](history/oct-17-index-closeout/tickets/WRITE-OWNERSHIP.md). The active ownership page retains only current packet boundaries and handoff/verification contracts. Its old heading aliases remain valid.

Thirty-three caller/source/run observation blocks not already present in the validation history were preserved in [producer observations](history/oct-10-index-closeout/VALIDATION-2026-10-21.md#producer-caller-observations-2026-10-21). The active producer summary now distinguishes source-connected support from actual producer/publisher and Runtime/product qualification. The former SDK funded-accessor rewrite instructions moved to [SDK handoff history](history/oct-17-index-closeout/tickets/W2-19-integration-acceptance.md#funded-sdk-handoff-before-scope-closure); the current ticket reuses the actual scoped client/RPC loan.

Old Oct-13 tickets and lexical/trace pages remain retired link entrypoints. Accepted ADR requirements and RFC constraints are unchanged; only execution/history routing links were corrected. No new ticket/status ledger, runtime result or qualification was created.

Active lexical/trace/audit entrypoints keep their old fragment aliases and link directly to their existing full histories. Fixed contracts and Accepted ADR/RFC requirements are unchanged. Long future execution records belong in dated history;the active validation summary records exact source/command/scope and links to that history. Do not append a second status ledger below SSOT's seven rows.

Pre-compaction document SHA256: VALIDATION `25ec65ab6b0210c9c6ca396f7643df92c475f7a447eab8092bb44cfc6682a2e9`; SSOT `11928e23af8c7eca7a6e7728e45aa2bde1a6140fe9ff74c1dd4e131cb52ad92c`. These are document recovery identities,not product/source qualification.


### Oct-21 Native RFC checkpoint retirement

The Native RFC section 23 implementation/progress table and dated sections
23.1–23.90 now live in [checkpoint history](history/oct-10-index-closeout/RFC-CHECKPOINTS-2026-10-21.md).
The active RFC retains the original forty R01–R40 requirement/exit-condition rows,
without historical code-state or verification columns, and all previous fragment
IDs. At that pass sections 1–22 were retained; the follow-up relocation below
removes additional dated implementation records without changing their contracts.
The F01–F09 records and nested technical constraints remain in the preserved body;
this move does not change scope, accept a gate or authorize a new implementation.
The source-map page no longer repeats frozen release-run results; VALIDATION
owns those source-specific observations. The active validation summary is updated
in place by its existing writer; prior raw results belong to dated history.

Pre-move RFC SHA256: `b2e14773326c90d7cd73e86c42d05d9d1d3840a572bdd78176a437c4b497b97e`. This identifies the documentation preimage, not a qualified executable source.


### Oct-21 follow-up implementation-history cleanup

The dated section 17.2.3–17.2.4 rechecks and section 22 executable checkpoints
were moved to [implementation history](history/oct-10-index-closeout/RFC-IMPLEMENTATION-CHECKPOINTS-2026-10-21.md).
The active RFC retains their headings and fragment IDs as navigation entries.
The full bodies preserve technical contracts, original test results, failures,
commands and old source observations. Current work/status stays in the existing
seven-row SSOT; the active RFC does not repeat those historical assignments.
The fixed R01–R40 requirement rows and all other RFC sections are unchanged.

Pre-move RFC SHA256: `84bb351abbfdeb230f4867cc6b6fada0dff13d401381b308515df383b4b0eac3`.
This is a document recovery identity, not executable qualification.


<a id="oct-22-finalizing-migration"></a>
## Oct-22 finalizing migration

Current work/status/owners/next conditions live only in [eight epics](plans/oct-22-finalizing/README.md).
[Final DoD](plans/oct-22-finalizing/DOD.md) maps all original R01-R40 exactly once to a primary epic and preserves P2I/UX/AUTO, performance/operations, installed/CI/release and actual IntelliJ acceptance. Prior deferrals are not completion or implicit scope removal.

| Preserved record | Archive | Live destination |
| --- | --- | --- |
| Oct-17 seven tickets, prior SSOT and source/hunk rules | [Original ticket bodies](history/oct-22-finalizing/oct-17-index-closeout/tickets/README.md) and siblings | [Epics](plans/oct-22-finalizing/README.md), [single-writer map](plans/oct-22-finalizing/OWNERSHIP.md) |
| Full validation immediately before migration | [Source/run history](history/oct-22-finalizing/oct-10-index-closeout/VALIDATION.md) | [Compact current validation](plans/oct-22-finalizing/VALIDATION.md) |

At this checkpoint ten former live documents became forwarding pages with
867 fragment aliases. The forwarding pages were later removed as recorded
above. Accepted ADR/RFC constraints and the original forty-requirement
denominator remain unchanged. This migration creates no runtime, benchmark or
release qualification.
