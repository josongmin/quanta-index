# SEP-21 Search Plane SOTA Hardening — File-Level Action List

Status: implementation checklist; no item is complete from this document alone.

Copy/paste execution prompts: [prompt runbook](prompts/README.md)

Execution rule: respect the wave order in [INDEX.md](INDEX.md). Parallel work is allowed only where the table says
so and only after shared DTO/schema owners are frozen. A checked item requires code plus the listed DoD evidence.

## W0 — authority and proof freeze

### S21-00

- [x] Purpose: remove semantic drift before code changes.
- [x] Files: `docs/adr/`, `tools/ci/inventory/wire-surface.toml`, proof authority, this packet.
- [x] Logic: freeze canonical identity, catalog state machines, receipt versions, refusal codes, migration and
  shutdown/cursor/provider decisions.
- [x] DoD: no ownerless/TBD decision; old/new matrices and fixture names exist; no compatibility shim design.

### S21-13A

- [x] Purpose: make required evidence absent/stale/zero-execution fail before implementation starts.
- [x] Files: `tools/ci/proof-authority.toml`, proof schema/validator, CI/pre-commit wiring,
  `tools/ci/lint/check-test-authority.py`, receipt schema/writer, CI workflow skeleton.
- [x] Logic: strict manifest validation for full SHA, dirty digest, binary, host, feature/toolchain, counts and
  artifact digests; register every mandatory family.
- [x] DoD: missing artifact, short SHA, dirty mismatch, selected/executed 0, ignored-only, wrong binary and
  unregistered proof all fail locally and in blocking CI.

Closed M0 boundary: blocking PR CI produces and validates the registered P00 receipt against current source;
`--require-all --bind-source` is a separate explicit release gate and refuses any absent future receipt. The source
digest covers staged, unstaged and Git-visible untracked bytes while excluding proof output. Exact-pair receipts bind
the normalized paired repository identity, its current Git state and registered lockfile. Future proof nodes still
must add their owner-local `test-authority` targets and evidence in the owning lane; this checkbox does not qualify
those unimplemented product surfaces.

## W1a — canonical identity/layout

### S21-01

- [ ] Purpose: eliminate tuple aliasing and product-identity-by-filename.
- [ ] Files/symbols: `contract-base/src/macros.rs::string_newtype`, `contract-base/src/ids.rs::{RepoId,RevisionId}`,
  `repomap/src/persistence.rs::{snapshot_file_name_for,activation_file_name,encode_component}`,
  `repomap/src/store.rs::RepoMapStoreKeyV1`, `repomap/src/model.rs::RepoMapSnapshot`.
- [ ] Logic: one canonical tuple encoder; domain-separated content address; separate logical key/content digest;
  payload/address verification; append-only quarantine incidents.
- [ ] DoD: injectivity/property/golden vectors; Unicode/case/path-length/symlink/hardlink negative fixtures;
  runtime legacy dual-read/write 0.

## W1b — parallel compiler and journal

### S21-03 — lane A

- [ ] Purpose: make malformed/unbounded RepoMap input unsealable.
- [ ] Files: `contract/src/repomap.rs`, `core/src/domains/repomap/`,
  `repomap/src/{materializer,model,query}.rs`, `repomap/tests/`.
- [ ] Logic: `RepoMapGraphCompiler::compile -> CompiledRepoMapCandidateV1`; typed node keys; whole-bundle
  referential validation; deterministic canonicalization; byte/work budgets; Unicode tokenizer.
- [ ] DoD: refusal before durable mutation; no silent `continue`/last-write-wins; order-independent commitment;
  worst-case allocation/work remains within configured envelope.

### S21-04 — lane B

- [ ] Purpose: make mutation replay, ownership, refusal and sequence durable and deterministic.
- [ ] Files/symbols: `core/src/domains/idempotency.rs`, `catalog/src/{idempotency,open,auxiliary}.rs`,
  `search-plane/src/ingest_dispatcher/dispatcher.rs`, operation status contract/SDK.
- [ ] Logic: `inspect → prepare → record_refused|claim_prepared → apply → commit/recover`; owner lease/fence;
  terminal receipt/row digest; positive unique sequence; replay floor.
- [ ] DoD: replay precedes mutable preflight; stale worker cannot commit; restore high-water reconciles;
  terminal replay does not call storage/provider; effective SQLite durability settings are read back.

Parallel boundary: S21-03 owns RepoMap compiler DTO/output. S21-04 owns operation protocol/schema. Shared contract
files are assigned to P02I; the lanes do not both edit the same DTO or baseline concurrently. P02I must integrate both
checkpoint commits, rerun both registered proofs on the same clean HEAD and emit the source-bound integration handoff
before S21-02 starts.

## W2 — sole durable candidate/activation authority

### S21-02

- [ ] Purpose: make publish/activate/recover content-bound with one authority.
- [ ] Files: `repomap/src/{materializer,model,persistence,store}.rs`, catalog candidate/activation schema,
  `searchd/src/app/runtime.rs`, search-plane control/ingest owners, contract/SDK RepoMap surfaces.
- [ ] Logic: seal compiled candidate object; catalog candidate+activation CAS; publish registry after commit;
  durable invalidation; exact content-bound ACK/replay; delete activation-file runtime authority.
- [ ] DoD: publish never activates; same generation/different content conflicts; crash converges to one committed
  activation; missing/corrupt object invalidates and cannot resurrect; runtime activation file readers/writers 0.

## W3 — query truth and consumer binding

### S21-05

- [ ] Purpose: bind a request to actual immutable resources for its full lifetime.
- [ ] Files/symbols: `core/src/domains/repomap/inbound.rs::RepoMapQueryPort`,
  `core/src/domains/read_view/{identity,domain,errors}.rs`,
  `query_dispatcher/read_view/{view,snapshots}.rs`, `query_dispatcher/routes/repo_map.rs`,
  `repomap/src/{store,reader}.rs`.
- [ ] Logic: acquire active identity+`Arc` atomically; store `DomainReadEvidenceV2`; execute only via view;
  RAII pin release; GC respects pins.
- [ ] DoD: one evidence per declared domain; ambient store/ledger lookup after acquisition 0; barrier-based
  activate/retire/GC tests; cancel/panic reference count returns to baseline.

### S21-06

- [ ] Purpose: report pagination, coverage, ranking and availability without inference from row count.
- [ ] Files: `contract-base/src/results/query_window.rs`, core semantic/hybrid domains,
  `query_dispatcher/{dense_admission,window,hybrid,hybrid_seed,semantic_query}.rs`, cursor/explain/harness owners.
- [ ] Logic: `ExecutionOutcomeV2`; route capability matrix; canonical continuation digest; typed candidate identity;
  compact post-dedup rank shared by RRF/provenance; execution vs contribution metrics.
- [ ] DoD: no capped/partial-to-exact promotion; `has_more=false` requires exhaustion proof; independent RRF/window
  oracle; all cursor tamper dimensions rejected before execution.

### S21-07

- [ ] Purpose: reject syntactically valid responses that do not answer the original request/authority.
- [ ] Files: `sdk/src/client.rs`, query/control/ingest SDK modules, `contract/src/results/query_responses.rs`,
  `contract/src/repomap.rs`, cursor validators, `sdk/src/config.rs`.
- [ ] Logic: closed expected-response enums built before payload move; intrinsic then contextual validation;
  active selector resolution proof; query-only endpoint profile.
- [ ] DoD: every public method exhaustively mapped; wrong-but-same-variant matrix rejected; active epoch mismatch
  rejected; query-only client does not require control/ingest sockets.

W3 sequencing: freeze S21-06 outcome/cursor schema first. S21-05 resource work and S21-07 validator plumbing may
then proceed on disjoint files, but the final merge unit closes all three together.

## W4 — provider, process and control boundary

### S21-08

- [ ] Purpose: refuse locally decidable semantic work before egress/cost and globally bound admitted work.
- [ ] Files: `core/src/domains/semantic/`, `search-plane/src/{query_embedder,semantic_derive}.rs`, semantic/hybrid
  routes, `embed/src/openai.rs`, provider config/telemetry.
- [ ] Logic: common input/model/policy gate; source and query egress classification; global reservation;
  cancellable supervised executor; observed model/dimension/finite/norm/usage validation; redacted audit.
- [ ] DoD: local refusal provider calls 0; source/query leakage scan 0; cancellation/retry reservations settle;
  task/FD/thread/request/cost stay under global caps.

### S21-09

- [ ] Purpose: make every process resource owned, observable and bounded through shutdown.
- [ ] Files: `searchd-runtime/src/lib.rs`, `searchd/src/app/{searchd,runtime,maintenance}.rs`,
  `ipc/src/{server,counters}.rs`, provider executor, process tests.
- [ ] Logic: remove `SearchdRuntime` partial destructure; supervisor owns `RuntimeGuards` and child registry;
  signal root; all-or-rollback startup; RAII connection accounting; cooperative/hard drain and exit semantics.
- [ ] DoD: state-root lease lives across entire serving interval; real two-process exclusion; required child death
  lowers readiness and exits non-zero; signal/spawn-failure/panic/slowloris leave no unjoined resource or residue.

### S21-10

- [ ] Purpose: enforce least privilege per operation and report process health truthfully.
- [ ] Files: `ipc/src/{socket_access,server}.rs`, `search-plane/src/control_dispatcher.rs`,
  `searchd/src/app/{socket_access,runtime}.rs`, config, contract/SDK/searchctl observability surfaces.
- [ ] Logic: kernel credential → principal → exhaustive capability; `DispatchContextV1`; separate process readiness
  and generation status; bounded diagnostic correlation; executed/contributed counters.
- [ ] DoD: unauthorized mutation 0; unknown principal default deny; kill each required component makes global
  ready false; request correlation contains no high-cardinality metric labels or secrets.

W4 parallel boundary: S21-08 owns admission/reservation interfaces, S21-09 owns executor lifetime, S21-10 owns
principal/capability/readiness DTOs. Provider enrollment and readiness integration close atomically.

## W5 — state and producer cutover

### S21-11

- [ ] Purpose: migrate/backup/restore a whole state root without live fallback or inconsistent snapshots.
- [ ] Files: searchd CLI `command.rs`, runtime `StateRootLease`, `legacy_semantic_migration.rs`, `semantic_boot.rs`,
  catalog connection/open/idempotency/auxiliary, persisted adapters, wire inventory, operator docs.
- [ ] Logic: offline exclusive lease; SQLite backup API; immutable object inventory; staging deep scrub;
  manifest-last fsync; same-filesystem atomic cutover; forward-only recovery after first new-format mutation.
- [ ] DoD: source root unchanged; partial staging never opens ready; old/new pairs explicitly refuse; restored
  identities/receipts/replay floor/high-water match manifest; production boot legacy migration/decoder 0.

### S21-12

- [ ] Purpose: prove producer payload, journal result, candidate, activation and release binary are one chain.
- [ ] Files: quanta contract/SDK/ingest/control/RepoMap APIs, Semantica handoff/receipt owners, cross-repo hellgate,
  release receipt schema.
- [ ] Logic: mandatory domain-separated commitments from producer source/payload to operation/candidate/activation;
  exact dependency roots and binary provenance; breaking compatibility refusal.
- [ ] DoD: identity-only ACK cannot close; old/new matrix rejects before mutation; ACK replay does no repeated work;
  clean exact repo pair and actual daemon binary hash are in the aggregate receipt.

## W6 — final qualification

### S21-13B

- [ ] Purpose: convert completed code into source-bound evidence without conflating deploy/activation.
- [ ] Files: proof authority, test authority/lints, Justfile profiles, CI workflows, benchmark/quality tooling,
  receipt writer, checklist and closeout report.
- [ ] Logic: aggregate static, owner, adapter, UDS, process, fault/concurrency, quality/ANN/performance, provider,
  migration and cross-repo families from one final source and attested binary.
- [ ] DoD: all mandatory nodes pass; no `NOT_RUN`/`BLOCKED` P0/P1; Linux production-like host and real-provider
  opt-in proof present; verdicts separately state code qualified, deployed, activated and rollback proven.

## Stop and re-plan triggers

- shared contract/schema decision changes after a dependent lane begins;
- implementation introduces dual-read, dual-write, fallback decoder or second activation authority;
- proof requires a different daemon binary/source than the candidate being qualified;
- runtime cannot terminate a worker but still proposes graceful success or releases the state-root lease;
- migration needs live source mutation or raw SQLite/WAL copying;
- a closing test derives expected output from the system under test rather than an independent oracle.

Any trigger blocks the affected merge unit. It is not handled by adding an optional field, compatibility wrapper,
route-local correction or post-hoc metric.
