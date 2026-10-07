# SEP-27-005 — Catalog Recovery, Supervision and Proof Custody

Status: `Accepted`

Decided: 2026-09-27

Consolidates completed SEP-21 repair decisions from the execution chronology.
Extends SEP-21-002/003/004 and SEP-27-004. Historical runs do not qualify the
current source; remaining release, producer and operational work stays in the
[SEP-21 residual ledger](../plans/oct-4-parallel-closure/tickets/INDEX.md#release-and-proof).

## Decision

### One transactional catalog and event authority

- Classify freshness under the startup `BEGIN IMMEDIATE` lock: only a database
  with no tables is pristine. Unknown, auxiliary-only or partially installed
  tables cannot authorize a zero seed. Schema creation/verification, allocator
  initialization, sequence reconciliation, unfinished-operation abort, stale
  lease cleanup and final integrity checks commit or roll back together.
- One crate-private installed-schema verifier compares canonical table/index
  DDL, normalizing only whitespace and creation-time `IF NOT EXISTS`.
  Incompatible installed constraints refuse before recovery/serving; no silent
  migration or equivalent-looking schema substitution.
- The append-only global event ledger has contiguous sequences and bidirectional
  domain/event references. Canonical decoders bind event kind, sequence, logical
  identity, payload/commitment, row digest and state. Candidate quarantine follows
  seal; activation/invalidation references and prior commitment are bound to the
  actual candidate and activation state. At most one latest active head exists
  for a repository/revision, including targeted reads after catalog open.
- A replay validates the complete surviving terminal journal row, receipt bytes,
  operation identity and its exact ledger event before returning original bytes.
  An internally self-consistent foreign receipt cannot substitute for that
  operation. Legitimately removed terminal rows need exact invalidation lineage.
- Retry supersession (event kind 7) and generation-GC invalidation (kind 12) bind
  the immediately preceding terminal commitment or an explicit no-terminal
  marker. Attribution is one-to-one. GC writes the self-digested
  `operation_gc_floor_v1` row in the same transaction; floor and event are checked
  both ways. Removing/recasting one cannot turn a retired retry into a fresh one.
- All claim/prepare/lease writers use the one self-digested `catalog_fence_v1`
  allocator in their existing transaction. Fences are positive, root-global,
  monotonic and explicitly exhaust at `i64::MAX`; rollback does not advance
  them. Verify schema/digest/high-water before recovery. No truncated-clock
  fallback. Expired same/foreign-owner takeover writes supersession lineage
  before replacing terminal state; unexpired claims remain fenced.
- Quarantine discard state and sequence are an exact relation. Catalog-first
  tombstoning, retry/boot reclaim and live payload references prevent false
  discard success and unlinking shared live bytes. There is one visibility
  authority, not a filesystem fallback.

Old event/floor/fence/schema roots require inventory, backup and an explicit
offline rebuild or separately designed conversion. Self-digests do not detect
coordinated rewriting of every authority or restoration of an older consistent
root; external trust anchors and root/daemon fencing are separate obligations.

### Pinned handles, failure settlement and retirement

- SEP-21-003 owns `QueryReadViewV2`: one evidence per declared domain, handles
  per actual resource group, immutable lifetime through completion and no
  ambient latest lookup afterward. RepoMap executes through its acquired handle.
- `catch_open` settles failed flights and removes opening reservations, then
  resumes the original panic for supervision. Poison remains failure; wake
  ordering cannot discard cancellation or strand waiters.
- Shared weak `LiveHandleTracker` custody includes resident, evicted, oversized
  and detached snapshot/history-text handles. Track before exposure and sweep
  dead weak references without retaining native resources. Retirement checks
  all live incarnations and opening flights, including repeated calls and
  generation-wide retirement; return `StillReferenced` without waiting for an
  opener. GC, repair and quarantine defer physical deletion on that result.

### Supervised terminal truth and backend observations

- Observe every registered finished child, including `reports_exit` children.
  Join completion and queued reports are separate facts: preserve Failed,
  classify missing required reports as Failed and actual join panic as Panicked.
  Serving, drain and rollback share classification; reporting cannot substitute
  for termination or release runtime/state-root custody early.
- Drain, required-child loss and rollback fix their hard deadlines at phase
  entry. Charge stop callbacks and clip cooperative waiting against that same
  budget; arithmetic overflow exhausts. This does not bound arbitrary blocking
  callbacks or establish actual daemon signal/credential/release behavior.
- Maintenance owns lightweight lexical/semantic sealed-identity probes bound
  to the exact active generation and activation token. Readiness refuses missing,
  failed, wrong-identity or stale observations; the default staleness horizon is
  three maintenance cadences. Identity/marker reads are capped at 4096 bytes.
  Fresh physical admission seeds a new activation; zero-active catalogs need
  no track roots. Root/identity liveness is distinct from deep content proof.
  The bounded authorized IPC projection and independent disk-meter cadence are
  implemented under [OCT-05-003](OCT-05-003-active-query-and-runtime-lifecycle.md).
  Final selected installed/release process proof retains its own source/host scope.

### Original backup authority and current-format cutover

- `backup-state`, `verify-state` and `restore-state` are the current offline
  workflow under an exclusive lease. `migrate-state` and boot-time legacy
  import are retired. A historical materialized RepoMap snapshot cannot recreate
  the original source graph/authority; retained legacy data needs an explicit
  producer rebuild and retention decision. This supersedes legacy-import steps
  in the original SEP-21-004 workflow, without adding a conversion implementation.
- Pin the original backup manifest in read-only custody. Verify its complete
  inventory and catalog digest/row count before staging; verify copied bytes
  against that same original authority before deliberate incarnation rotation.
  Reserved malformed/dangling/nonregular entries are not absent. Keep only the
  bounded catalog-directory SQLite sidecar exception; other advertised bytes
  and directories remain exact. Drift or missing authority refuses, discards
  staging and cannot publish a partial root. Restore-forward is the supported
  post-mutation rollback boundary; actual target-root/release proof is separate.

### One proof interpretation and custody owner

- Registry-derived atomic manifest production and immutable source-pair/archive
  identity remain SEP-21-004-owned. Required proof DAG/verdict expectations have
  an independent checker oracle; editing the registry cannot redefine success.
  Every new owner target must enter executable authority before proof issuance;
  empty future selections cannot inherit the foundation gate. Composite scopes
  enforce the strictest included thread/resource cap. Detached/upstream-less
  sources and registered nonbinary modes remain explicit, never missing defaults.
- PR CI's current P00 foundation proof and the explicit all-proof release-bundle
  dispatch have different scope. P12A infrastructure can qualify independently
  of P11; the final aggregate still needs the complete current-source release
  graph and separate operational verdicts.
- `proof_json.py` rejects duplicate keys at every depth, nonfinite constants and
  floating overflow before schemas across terminal/archive/aggregate/handoff
  readers. One `junit_events.py` owns supported XML placement, counters, outcomes
  and known attributes; unsupported wrappers and failure/error/skipped outcomes
  cannot become passed tests. Inventory binds unique complete-file selectors and
  exact represented cases. A smaller self-forged inventory is not attestation.
- `NextestInventory` admits only known explicit filter outcomes, preserves
  selected identities and precise ignored exclusions, and is immutable.
  Inventory-bound reporter fragments reconcile starts/outcomes/footers and the
  complete unique selected set. Excluded ignored events are never selected
  successes; unknown exclusions, missing outcomes, duplicates and inconsistent
  counters refuse. Unbound transcripts keep strict suite accounting.
- Capture evidence/inventory once through the no-follow descriptor owner,
  verify declared digests and interpret those exact bytes. No pathname reopen
  between hash and parser. Private executable copies pin paired-daemon bytes
  through every subprocess and final check; build/supplied aliases cannot reduce
  custody to comparing a file with itself. Same-UID hostility remains excluded.
- Proof DAG validation uses invocation-local content/full-authority-bound reuse.
  Every incoming edge retains custody, identity/ancestry and digest checks;
  hits and final checks rehash relevant artifacts/binaries. Wrong source,
  changed authority, cycles and cross-invocation substitution refuse. Cache
  reuse is not aggregate speedup or a shared cross-root cache claim.
- Rail names and full argv share one literal Bash-word decoder. Unknown
  expansions, executable globs, prefix assignments and nonexecution/partial
  Python selection cannot establish a completed command. Selected absolute
  tools and executable epochs govern SDK/contract and conditional source/build/
  reference/native work; reject inherited exported functions and startup/test
  selection overrides. Command frames/environment/epochs and execution bytes
  are replay inputs, not PATH-only assumptions. This is not compiler, Python
  package, system-library or remote-producer attestation.
- Use the shared process owner in SEP-27-004. Failure drain/reap share a bounded
  ten-second cleanup deadline. Nested session lifelines survive controller death
  through EOF cleanup. After direct exit, retain unreaped group identity, drain
  output, validate the bounded private status, kill remaining owned group work
  before reaping, then interpret completion. Refuse external/ignored SIGCHLD
  handlers before spawn; default-selector watchers support high descriptors.
  No arbitrary escaped-session containment is claimed.
- Conditional final results publish only after terminal custody succeeds,
  through exclusive fsynced pending bytes and atomic hard-link publication.
  Pending output and mere binary hashes cannot establish successful execution.

### Executable invariant authority and tier receipts

- `tools/ci/test-authority.toml` owns target/rail/scope and declared invariant
  registration. Its guard compares filesystem/Cargo/fuzz inventories, explicit
  workflow event/job/step commands and selectors; P0/P1 rows name owner-local
  positive/negative plus recovery/consumer roles. Unknown, missing or substituted
  targets and disabled/nonexecuting bindings refuse. Catalog registration does
  not prove that the declared invariant universe is semantically complete.
- `tools/ci/inventory/wire-surface.toml` and its guard bind current IPC/format reproduction
  classes and fixtures. Current-contract breaking cuts refuse retired shapes;
  historical fixtures are rejection oracles, not permission for a second decoder.
- Current PR/merge/main and scheduled nightly workflow paths bind raw nextest
  inventory/terminal evidence through the existing receipt writer/schema. Exact
  selected/executed outcomes, nonzero complete execution, command/tier/source and
  raw identity govern admission. An enabled workflow is not a hosted passing run,
  and a valid lower-scope receipt cannot promote itself into release qualification.
- Ignored-test ownership and actual excluded identities remain explicit. A
  skipped/empty selection, duplicated outcome, forged smaller inventory or stale
  receipt cannot establish completed proof. Keep the existing guard/parser owner,
  not a second test list or generic boolean promotion API.
- Adapter lifecycle generation-selection models and independent query fixtures
  are implemented proof seams. Unadmitted quantitative targets from the retired
  Jul-15 plan remain historical proposals; executable authority owns selected gates.

### Implemented lifecycle tests and remaining coverage

The [semantic lifecycle model](../../crates/quanta-index-semantic/tests/semantic_generation_lifecycle_model.rs)
uses an independent corpus/kind/owner-to-record map and the real durable adapter. Its
deterministic case and six generated seed/base traces cover replacement, delta,
tombstones, unsealed-open refusal, selection/rollback and restart recovery.
Sealing is carried by Build; selection/rollback changes the model's selected
generation, not the process-wide active-head CAS. Repeated unsealed replacement
and append already occur through Build, including retry and restart in the six
generated traces; a separately named Append enum was never a missing API.
Clear and QueryPinned now have explicit model operations. The mixed-corpus
trace shares owner ID/path across SymbolCard, ModuleCard and raw Symbol/Callsite/
Module/Chunk rows, repeats unsealed delivery, selectively clears Symbol/Module
and checks fixed historical membership after seal, selection, rollback and
restart. Existing individual tests remain controls:

- `build/tests.rs::clear_symbol_surface_removes_exact_and_fallback_rows_only_v1`
  independently checks that clearing Symbol removes exact/fallback rows while
  preserving Module; append-failure tests preserve prior unsealed rows.
- `persisted_semantic.rs::generation_pin_isolates_results` checks distinct
  persisted-generation result sets. SCV2 owner replace/tombstone/restart and
  vector-index append/delete/retrain cases also exist.
- `readiness/tests/activation.rs` already covers simultaneous CAS contenders,
  stale activation/rollback tokens and old-pair visibility during delayed parent
  sync. `composite_generation_authority_restart.rs` has real child-process
  active/pinned queries before and after restart/rollback, including cross-repo
  retention. `active_selection_process_v1.rs` gates a selected G1 query across
  G2/G3 activation and physical retirement, requiring refusal before old-view open.

The [SDK lifecycle history rail](../../crates/quanta-index-searchd-runtime/tests/e2e_lifecycle_history.rs)
combines sealed delta append, coverage-coherent source-file and semantic-scope
deletion to an empty corpus, exact old source-event replay, historical
lexical/semantic pins, duplicate CAS contenders and a stale-head contender,
out-of-order publication refusal with exact retry, caller-delayed query,
rollback, real daemon restart and stale-token ABA refusal.
Its [reference model](../../crates/quanta-index-searchd-runtime/tests/lifecycle_history/model.rs)
uses generation-to-row sets and generation/incarnation/sequence heads; expected
rows come from fixed fixture inputs, not production query output. Each SDK call
has invocation/response events. The checker searches legal serial orders while
enforcing completion-before-invocation precedence. It preserves duplicate
results and rejects empty, missing, duplicate/orphan terminal events and search
exhaustion. The bounds are 64 operations and 100,000 search nodes, not 100,000
executed lifecycle transitions or nightly qualification.
Fixed [oracle counterexamples](../../crates/quanta-index-searchd-runtime/tests/lifecycle_history/oracle_tests.rs)
reject stale reads after completed activation, double CAS winners, missing clear,
duplicated append results, foreign pins, ABA token reuse, sequence/incarnation
drift, restart head loss and sealing past an older staged source publication. A
publication-order refusal is valid only while a predecessor remains unaccepted;
fixed controls reject unnecessary refusal after accepting it and changed-payload
retry of the refused event. A separate control retains already-accepted source
activation refusal after rollback. An old read overlapping activation remains legal
even if its response arrives later. Caller gates guarantee overlapping CAS
invocations; they do not claim a gate inside the server's storage transaction.
The public trace publishes and activates each delta predecessor on one producer
stream before the next event. The physical delta base must carry that same
stream and its declared parent event. Independent streams start full replacements
but cannot bypass the pair-wide history-retention refusal to seal past an older
unaccepted source publication. The trace observes typed `NOT_READY` for early G5
publication while G4 is staged, then retries that exact batch/event after G4
activates. It races two duplicate G4 requests against the complete G3 head and a
third G4 request against the stale complete G1 head; exactly one matching caller
may win. It does not manufacture two simultaneously sealed unresolved candidates.
Coverage-bound SDK generations forbid independent Chunk/Symbol clear. The
empty-corpus step uses the existing `tombstone_scope` and
`tombstone_semantic_scope` APIs together, preserving coverage/source ownership;
it does not weaken that refusal or introduce a new clear API. Exact event replay
is also checked. The existing adapter
model and its mixed-corpus extension cover repeated unsealed delivery and
surface-selective clear. Canonical SDK/catalog operations remain the production
owners; there is no new activation or publication API.

The [runtime concurrency test](../../crates/quanta-index-searchd-runtime/tests/e2e_generation_activation_concurrency.rs)
uses the SDK over UDS with an in-process `E2eRuntime`. A querying thread overlaps
one G1-to-G2 activation and checks complete, unmixed predicate-authority result
sets. That single-race test remains distinct from the independent bounded
operation-history checker above.
Individual concurrent-CAS, sync-delay, source-event refusal and child restart/
rollback regressions above cover parts of that schedule. The new history rail
combines those operation kinds through actual public front doors; its bounded
trace does not replace declared generated-case/repeat inventories or native
race-detector execution.
The [dispatcher selector regressions](../../crates/quanta-index-search-plane/src/query_dispatcher/tests/semantic.rs)
already reject resolved-selector A-to-B-to-A ABA and mismatched explicit pins;
retain those controls alongside the operation-history oracle.

The lifecycle model and persisted scenarios are registered integration targets
in `tools/ci/test-authority.toml`. Runtime concurrency uses `runtime_risk_suite`,
composite restart uses `runtime_extended_suite`, and active selection is the
standalone `active_selection_process_v1` target. Semantic build and activation
catalog regressions are library tests. Their implementation is enrolled already;
the SDK lifecycle history target is also enrolled in `runtime_extended_suite`
and the local daemon scope. `VERIFIED` on 2026-10-07: `./scripts/cargow --lane test-scale-f15-lane nextest run -p quanta-index-semantic --test semantic_generation_lifecycle_model --all-features --locked --no-tests fail --test-threads 1 --no-fail-fast`
passed all three tests (deterministic, six generated traces and the mixed-corpus
trace; 6.966 seconds execution), integrated as main `50a30b09`. Its strict target Clippy also passed:
`./scripts/cargow --lane test-scale-f15-lane clippy -p quanta-index-semantic --test semantic_generation_lifecycle_model --all-features --locked -- -D warnings`.
The corrected public trace and all seven SDK checker controls passed on
2026-10-07 (8/8, 9.681 seconds execution; real daemon trace 9.461 seconds):
`./scripts/cargow --lane test-scale-f15-lane nextest run -p quanta-index-searchd-runtime --test runtime_extended_suite --all-features --locked --no-tests fail --test-threads 1 --no-fail-fast -E 'test(/^e2e_lifecycle_history::/)'`.
Earlier failed fixtures violated source staging, delta-parent, coverage-bound
clear or history-retention contracts; they are historical refused inputs, not
evidence of new production defects. The clear-membership counterexample is now
paired with a valid empty-result history so an unrelated publication-order
refusal cannot satisfy its negative assertion. That final control refinement
is included in the 8/8 result. Its affected integration strict check also passed:
`./scripts/cargow --lane test-daemon-lane clippy -p quanta-index-searchd-runtime --test runtime_extended_suite --all-features --locked -- -D warnings`.
Runtime strict Clippy with `--lib --tests --all-features` passed after fixing
cache-test lints; the current history integration target is verified separately.
The focused commands reused existing Cargo targets with
`QUANTA_INDEX_PRESERVE_CARGO_TARGET_DIR=1`, explicit `CARGO_TARGET_DIR`,
`QUANTA_INDEX_TARGET_GC=0`, `QUANTA_INDEX_RESOURCE_WAIT_SECONDS=7200` and
`CARGO_BUILD_JOBS=2`; the lane still uses canonical resource admission.
Static registration, formatting and document-link checks passed. Native race
detection, declared long generated/repeat scopes and full-suite qualification
remain `NOT_RUN`; 87 other integration cases were excluded by the history filter.

The [daemon crash matrix](../../crates/quanta-index-searchd-runtime/tests/e2e_crash_matrix.rs)
starts and restarts real child daemons. It requires a case for every declared
[seal/GC crash point](../../crates/quanta-index-search-plane/src/crash_point.rs),
currently eight, and checks convergence/retention. Those track-level points do
not inject failure at each inner F15 write, object link, file/directory sync,
root rename or cleanup operation. The narrower publication matrix is now
implemented in main `8599f2e8`: 32 I/O and 32 actual SIGKILL cases with independent
raw-object/source oracles. The follow-up `b09c4aa7` adds fresh recovered-query
assertions; `70521514` repairs interrupted delta cloning before writer admission
and expands the matrix to 36 I/O and 36 SIGKILL cuts. The owner reports those
cuts, the corrected partial-clone regression, 73 storage mutation/seal/cost
regressions and strict lexical Clippy passing. `eb97e7c2` pins release-daemon
bytes across process restarts; its Medium restart/delete and binary-custody
regressions passed. These focused owner results are not current hosted or
Large/XL qualification. Original completed owner runs are recorded in
[OCT-05-004](OCT-05-004-cost-capacity-and-qualification-boundaries.md#completed-source-bound-checkpoints).
Current Large/XL cost remains [E4](../plans/oct-4-parallel-closure/tickets/INDEX.md#e4);
[test/platform](../plans/oct-4-parallel-closure/tickets/INDEX.md#test-and-platform)
retains broader selected storage-boundary acceptance. The F15 matrix and recovered-query assertions are not missing
implementation.

Source coverage and explicitly reported focused executions above have separate
scopes; they are not repository-wide qualification.
Full SDK-only recovery, native race detection, long mutation/fuzz and
release/platform execution retain their independent oracles and scope.

The old QIT progress snapshot/scaffolding is retired. Its unfulfilled acceptance
is preserved in [the residual board](../plans/oct-4-parallel-closure/tickets/INDEX.md#test-and-platform);
this consolidation asserts implemented authorities, not SOTA/test qualification.

### Test fixture and wait invariants

The completed MISC test-optimization contracts retain these independent oracles.
Executable test authority and actual collection select the current owner checks;
this inventory asserts no fresh terminal result or quantitative speedup.

| ID | Current owner surface | Independent invariant |
| --- | --- | --- |
| D1 | searchd runtime end-to-end/repomap fixtures and searchd harness | Long-path configuration covers all three sockets, including ingest. |
| D2 | searchd runtime `tests/common/searchd_binary_process.rs` | Already-exited child cleanup cannot signal an unrelated reused process. |
| R1 | runtime lifecycle/lease fixtures | Explicit acknowledged release; no mandatory three-second happy-path sleep. |
| R2 | embed OpenAI retry/sleeper seam | Injected test delay; production retry bounds/jitter unchanged. |
| R3 | embed concurrency fixtures | Structural barrier proves four-way overlap, not a 25 ms timing guess. |
| R4 | IPC `PeerWatch` | Explicit wake/disarm/join; cancellation needs no compensating sleeps. |
| R5 | runtime ingest-resource envelope and lower core owners | Boundary assertions conserved below E2E; one wiring proof retained; daemon boots counted. |
| WA-1 | lexical trigram property tests | Unexpected error makes the property fail, never a skipped success. |
| WA-2 | runtime matrix smoke | Exact expected candidate identity/set, not any in-corpus row. |
| WA-3 | runtime state migration | Manifest/object/digest completeness; missing/corrupt data fails. |
| TH-1 | SDK-frontdoor observation waits | Never-ready input returns typed timeout, not stale `Ok`. |
| TH-2 | runtime process-envelope scrape waits | Never-true predicate returns typed timeout. |
| TH-3 | SDK binding fixtures | RAII temporary path custody; no pid-only persistent socket directory. |
| TH-4 | runtime filter-execution cases | Immutable family fixture reuse with per-case context and no shared mutable daemon. |
| PO-1 | core `timeref.rs` | Injected exact clock and fixed boundary matrix; convenience edge samples once. |
| PO-2 | SDK `config.rs` | Injected environment precedence/errors; no process-global mutation seam. |
| PO-3 | catalog connection/idempotency | One clock sample per transition; less/equal/greater deadline matrix. |
| PO-4 | search-plane `single_flight.rs` | Outcome-or-cancellation wake; no correctness dependence on 20 ms polling. |

Do not shorten sleeps, disable production jitter, weaken errors, share mutable
global fixtures or delete lower-layer assertions to make timing look better.
Reproduce a current regression before reopening implementation. Final selected
functional/installed/platform execution and paired test-cost measurements remain
in [MISC-05/06](../plans/oct-4-parallel-closure/tickets/INDEX.md#test-and-platform).

### Semantic mutation output admission

Preflight exact escaped UTF-8 native delete SQL before allocation or mutation:
the private output ceiling is 32 MiB, including quotes, separators and groups.
Borrow owner tuples; oversize is typed invalid contract/request, not predicate
splitting or broader deletion. Preflight header tombstones and the first leased
window before generation preparation; admit subsequent windows before their
writes, with only one leased window at a time. Earlier staging cannot be promoted
after later refusal. Charge first-window admission to streaming, not storage
preparation. The SQL ceiling is not total-input or aggregate heap admission and
does not change public owner/vector limits or wire schema.

### Selected regression and platform acceptance

QIT-00–09 and MISC-03/04/05 consolidate into the current residual ledger, with
these standing limits rather than another implementation queue:

- Test authority/catalog wiring does not prove semantic independence. Public wire
  golden/refusal/re-encode, independent lexical/ANN/fusion/filter/metamorphic oracles,
  generated lifecycle history, declared repeats and native race detection retain
  their actual inventory and platform. Existing lifecycle/SDK fixtures remain done.
- Selected storage/marker/CAS crash cuts keep old-or-complete-new and unchanged-source
  truth. Process kill does not establish power loss. Real ingress/provider and actual
  installed SDK/CLI/daemon lifecycle are separate from scripted peers and local tests.
- Mutation/fuzz/coverage select actual risk owners and admitted thresholds; record
  survivor dispositions/expiry and minimized inputs. Historical percentage, repeat,
  fuzz-duration and CI-budget aspirations are not newly enforced requirements.
- Every selected adapter's prepare/execute/publish/load/replay validates large success
  and failed stdout/stderr, many-entry metadata and bounded archive/JSONL interruption
  through the common process/I/O owner. One fake adapter cannot qualify all adapters.
- Final-source production and relocated replay use the canonical registered command,
  current complete nonzero inventory and independent terminal parsing. Refuse sticky
  profile/GC and ambient selection overrides; retain cooperative process limits.
  SDK/runtime, Linux cgroup/Landlock, native race detector, model and hosted CI results
  are separate scopes. Unselected Miri/ASan/LLVM/udeps diagnostics are conditional.
- Handoff schema now lives at tools/ci/lane-handoff.schema.json. Strict
  lane-handoff-check binds current-source checkpoint/proof custody; historical mode
  and lane-handoff-chain-check audit original ancestry/order/P02 joins only. Missing
  historical bytes cannot be reconstructed from current manifests or become release
  prerequisites by an old lane name.

### Consumer preview and operator acceptance

J7Q-01/02 retains independently judged route/hard-negative quality and overlapping
native lexical comparison, not arbitrary generated/test-file ranking penalties.
Actual SDK/CLI wire proof covers phrase/regex/repeated/long-line/symbol previews,
UTF-8/CRLF spans, bounded deterministic truncation and empty/unavailable output.
Explain reconciles actual planner/engine/contribution/score/provenance. Doctor,
readiness, generation-status and metrics agree under missing/divergent/failed
states; unsupported/ambiguous/wrong-route repairs remain typed without silently
rewriting intent. UI/confidence fields and the deferred regex allocation cap need
their explicit consumer/measurement trigger.

## Consequences and verification boundary

Retain independent regressions for event/domain substitutions, active cardinality,
foreign receipts, invalidation/floor/fence tamper and exhaustion, transactional
startup rollback, repeated live-handle retirement, report loss/deadlines, original
backup replacement, parser/selection mutants and actual owned-process cleanup.
Source/test identities and executable selectors own inventory; no historical
numerical test count becomes permanent acceptance.

The original daemon gate failure and later parser replay of unchanged raw bytes
are distinct historical claims. A source-specific exporter, focused owner tests
or a copied clean snapshot are not final combined-source qualification. Remaining
trusted host/producer/complete-command, real provider, semantic omission oracle,
exact-pair typed resolver/build receipts, release and operational acceptance stay
in the active ledger. No release, deployment or activation is asserted here.

Historical bodies are recoverable from
`0b4839a4a8b4cf99e870b4251395b6e3df8f4a21`; see
[the plan archive](../ARCHIVE-INDEX.md#historical-record-recovery).
