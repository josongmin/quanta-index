# SEP-27-005 — Catalog Recovery, Supervision and Proof Custody

Status: `Accepted`

Decided: 2026-09-27

Consolidates completed SEP-21 repair decisions from the execution chronology.
Extends SEP-21-002/003/004 and SEP-27-004. Historical runs do not qualify the
current source; remaining release, producer and operational work stays in the
[SEP-21 residual ledger](../plans/sep-21-search-plane-sota-hardening/tickets/FINAL-RESIDUAL-EXECUTION-PLAN.md).

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
  are implemented proof seams. Quantitative targets from the retired Jul-15
  plan remain proposals in its residual board until admitted and implemented.

### Implemented lifecycle tests and remaining coverage

The [semantic lifecycle model](../../crates/quanta-index-semantic/tests/semantic_generation_lifecycle_model.rs)
uses an independent owner-to-record map and the real durable adapter. Its
deterministic case and six generated seed/base traces cover replacement, delta,
tombstones, unsealed-open refusal, selection/rollback and restart recovery.
Sealing is carried by Build; selection/rollback changes the model's selected
generation, not the process-wide active-head CAS. The current command enum has
no independent Append, Clear or QueryPinned operation. This is a limit of the
generated model, not missing production APIs or an absence of individual tests:

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

The remaining model extension combines corpus/surface-aware clear, repeated
unsealed append/replacement, historical pinned observations and process-wide
CAS outcomes against an independent reference state. Reuse the existing
catalog/SDK operations and individual regressions; do not recreate them.

The [runtime concurrency test](../../crates/quanta-index-searchd-runtime/tests/e2e_generation_activation_concurrency.rs)
uses the SDK over UDS with an in-process `E2eRuntime`. A querying thread overlaps
one G1-to-G2 activation and checks complete, unmixed predicate-authority result
sets. It is not an independent operation-history linearizability checker or a
duplicate/reorder/delay/rollback/restart schedule matrix.
Individual concurrent-CAS, sync-delay, source-event refusal and child restart/
rollback regressions above cover parts of that schedule. The missing checker
must consume operation invocation/completion history, enforce real-time order
and find a legal sequence in an independent model across combined schedules;
per-response complete-result membership alone cannot issue that claim.
The [dispatcher selector regressions](../../crates/quanta-index-search-plane/src/query_dispatcher/tests/semantic.rs)
already reject resolved-selector A-to-B-to-A ABA and mismatched explicit pins;
retain those controls while extending the operation-history oracle.

The lifecycle model and persisted scenarios are registered integration targets
in `tools/ci/test-authority.toml`. Runtime concurrency uses `runtime_risk_suite`,
composite restart uses `runtime_extended_suite`, and active selection is the
standalone `active_selection_process_v1` target. Semantic build and activation
catalog regressions are library tests. Their implementation is enrolled already;
this source audit did not run Rust or native race detection.

The [daemon crash matrix](../../crates/quanta-index-searchd-runtime/tests/e2e_crash_matrix.rs)
starts and restarts real child daemons. It requires a case for every declared
[seal/GC crash point](../../crates/quanta-index-search-plane/src/crash_point.rs),
currently eight, and checks convergence/retention. Those track-level points do
not inject failure at each inner F15 write, object link, file/directory sync,
root rename or cleanup operation. The narrower publication matrix is now
implemented in main `8599f2e8`: 32 I/O and 32 actual SIGKILL cases with independent
raw-object/source oracles. Its completed owner runs are recorded in
[OCT-05-004](OCT-05-004-cost-capacity-and-qualification-boundaries.md#completed-source-bound-checkpoints).
[O4-E4-02](../plans/oct-4-parallel-closure/tickets/INDEX.md#o4-e4-02) owns only
the follow-up recovered-query/daemon checks; QIT-03 retains broader selected
storage-boundary acceptance. The F15 matrix is not missing implementation.

These are source/test-coverage statements, not fresh Rust execution results.
Full SDK-only recovery, native race detection, long mutation/fuzz and
release/platform execution retain their independent oracles and scope.

The old QIT progress snapshot/scaffolding is retired. Its unfulfilled acceptance
is preserved in [the residual board](../plans/jul-15-sota-test-hardening/tickets/00-ticket-status-board.md);
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
in [MISC-05/06](../plans/sep-27-misc/tickets/INDEX.md).

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
