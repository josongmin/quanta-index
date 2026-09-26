# SEP-21 residual execution plan

This is the current work map. The previous dated dirty-checkout overlays and
conditional importer plan are retained in Git history, not as executable
instructions. Reinspect code and proof registry at the source revision used
for each run; this document is not a receipt.

## Contract already present in source

- The CLI exposes `backup-state`, `restore-state`, and `verify-state`.
  `migrate-state` is not a command. Boot refuses legacy state instead of
  converting it. See [the operator runbook](../../../operator/state-cutover-runbook.md).
- `tools/ci/proof-aggregate.schema.json`,
  `tools/ci/write-proof-aggregate.py`,
  `tools/ci/lint/check-proof-authority.py`, and
  `tools/ci/lint/handoff_validation.py` implement the aggregate schema,
  producer, validator, and handoff DAG checks. `Justfile` has the P12A owner
  recipe and final producer recipe. Code presence is not a P12A manifest.
- `tools/ci/proof-authority.toml` is the operational registry. The checker
  independently fixes the expected proof dependency DAG and verdict inputs.
  Keep these distinct so editing the registry cannot redefine its own oracle.
- PR CI generates a fresh P00 receipt. The all-proof release gate in
  `.github/workflows/correctness.yml` is an explicit proof-bundle dispatch,
  not an ordinary PR gate.

## Adversarial current-source audit (2026-09-24)

Read-only snapshot: Quanta clean `28c20fabfdc9d57b0d7d94794d59bcf78ea7cd14`;
Semantica `f5e8ff63642ce815cafed296f2f5556f6e8e4048` had 13 dirty paths.
These identities expire when either checkout changes. No Rust/build/release
command was run for this audit. Registry-only lint is not execution evidence.

| Finding | Current-source evidence | Decision |
|---|---|---|
| Test result parsing is present; execution provenance remains open | `check-proof-authority.py::check_manifest` uses `proof_execution_result.py::derive_test_result` to recompute passed test counts and registered target coverage from archived Nextest or pytest results and collection inventories. `terminal.environment.host.profile` remains caller-supplied, and the result does not attest every recipe subcommand or CI bundle producer. | Retain the raw-result check. Bind host class, complete command outcome, and trusted CI producer before release qualification. A log digest alone is custody, not runner or host attestation. |
| Exact pair does not close path dependencies | `paired_source_snapshot` binds Semantica Git state and `Cargo.lock`, while Semantica's `quanta-runtime-retrieval-kernel/Cargo.toml` resolves `quanta-index-{contract,ipc,sdk}` through relative paths. `verify-repomap-cross-repo.sh` does not prove those canonical paths resolve to this Quanta checkout. | Add a resolved dependency-root check before building or accepting an exact-pair manifest. No extra RepoMap IR or compatibility reader. |
| P09 has two distinct gaps | `RuntimeReadiness` sets `required_backend: true` after boot; the transport event ring is only read in tests. Control wire/dispatcher/SDK/searchctl have no diagnostic opcode. | Replace the constant with live required-backend authority and expose one bounded, authorized projection of the existing ring. |
| P11 operational commands are declarations only | `proof-authority.toml` registers `proof-p11-deployment`, `proof-p11-activation`, `proof-p11-rollback`; `Justfile` contains no such recipes. All three use `execution_mode=test-authority` with empty target lists. | Model them as operational actions with independently observed before/after state, not fake test counts; add one real recipe per stage only after host/root authority is fixed. |
| P03-P08 are not blanket rewrite tasks | Pinned RepoMap/read-view, V2 query window, SDK response binding, provider admission, and supervisor owners exist; release nodes remain `staged`. | Audit each registered release target against its acceptance counterexamples. Change source only for a demonstrated missing behavior or target. |
| Semantic no-op is intentionally ambiguous | `SearchCorpusIngestBatch` carries semantic mutation vectors but no independent expected-scope authority; `semantic_derive.rs` accepts empty typed sources as a legitimate no-op. | Do not add a self-asserted `complete=true` field. First locate an independent producer source-plan/manifest oracle that distinguishes unchanged scopes from omitted required scopes. If none exists, narrow the claim and prove producer behavior separately. |
| Historical custody is absent | `lane-handoff-chain-check` requires P00/P01/P02A/P02B/P02I/P11; those files are absent from `artifacts/sep-21/handoffs`. | Recover authentic records or explicitly revise acceptance. Never synthesize historical handoffs from current manifests. |

## Code work in dependency order

### R0 — Establish the proof-result authority before release claims

Owners: `tools/ci/proof-authority.toml`, `tools/ci/proof-manifest.schema.json`,
`tools/ci/write-proof-manifest.py`, `tools/ci/lint/check-proof-authority.py`,
`tools/ci/tests/{test_write_proof_manifest,test_check_proof_authority}.py`,
`.github/workflows/correctness.yml`, and the canonical `Justfile` recipes.

- Define one versioned execution-result input per registered mode. For tests,
  derive selected/executed/passed/failed/ignored and exit status from an
  allowlisted machine-readable runner result; for operational actions, define
  typed observed pre/post state and outcome. Reject a `passed` terminal JSON
  whose result cannot be reproduced from the archived raw evidence. Keep
  evidence custody and result interpretation separate.
- Derive OS/arch from the issuing runner as today; do not accept a free-form
  `linux-production-like` claim as host qualification. Bind the host class to
  a trusted runner/host inventory, exact source, command, binary and artifact
  identity. The CI bundle consumer must bind the bundle to its trusted
  producing run, not merely download an arbitrary run ID and hash its files.
- Adversarial tests: forged counts, wrong executable, skipped/partial/timeout,
  reordered or stale result, wrong host class, tampered log, mismatched binary,
  wrong source and wrong CI run must all refuse. DoD: a hand-written terminal
  input plus an arbitrary log cannot produce an authoritative `passed` proof.

R0 is a release-evidence prerequisite, not a reason to stop independent P09
code development. Do not reissue owner receipts while source is still moving;
each commit invalidates earlier exact-source receipts.

2026-09-24 implementation note: the manifest checker now recomputes passed
owner-test counts from archived Nextest JSONL plus collection inventory or
pytest JUnit plus collection inventory. It rejects missing, malformed, skipped,
duplicate, partial and count-mismatched runner evidence. P00 passed counts are
fixed to the single inventory invariant, and a source-bound P00 validation
recomputes the archived discovery inventory. This closes the arbitrary-log/count
path for executable owner-test manifests. `QUANTA_PROOF_RAW_DIR` makes local
Nextest scopes emit collection inventories and JSONL events. P00 collects a
pytest inventory when `QUANTA_PROOF_RAW_DIR` is set; P12A collects an inventory
and emits matching JUnit through `proof_execution_result.py run-p12a` in that
mode. Both proof recipes reject `PYTEST_ADDOPTS` and `PYTEST_PLUGINS` because
they can change test selection. Default recipe runs do not emit these artifacts,
`--run-ignored all` scopes have no capture mode, and issuance does not bind
every recipe subcommand to its exit result. No new owner `passed` manifest
should be issued from human-readable logs. Trusted CI-run provenance, Linux host
inventory, executable/command binding, operational pre/post result modes and
the release recipe producers remain open R0 work. The parser is evidence
interpretation, not runner attestation or release qualification.

Historical P01 attempts exposed two source issues: the `quanta-index-contract`
module baseline omitted public active-head control DTOs, and the runtime E2E
fixture sent lexical chunks without the typed semantic sources required by its
semantic query expectations. Both were corrected in source. On 2026-09-25,
`just proof-p01-canonical-identity` completed on clean Quanta source
`623e80cea2cc8e6a62c5c77304c7c0addebb7e90`. The local log is
`/tmp/qi-p01-clean-623e80ce.log` (SHA-256
`ca187d90e44d72bd4cdd10bc41c4d62948aeb80f208d84da79a17bb2b6e06a76`):
canonical identity 15/15, shared surface 783/783, integration-fast 210/210,
CLI smoke 25/25, and daemon 204/204 with one ignored real-provider test;
structural, public API, wire, and fuzz rails also completed. This is a
source-specific recipe result, not a P01 proof manifest or release qualification.
The log lacks the archived machine-readable runner evidence required for an
authoritative manifest, and later commits make it stale for the current HEAD.

### R1 — Close the P03-P08 release counterexample inventory, not the owners again

Use `tools/ci/proof-authority.toml` and `tools/ci/test-authority.toml` as the
target inventory, but retain independent acceptance oracles from the tickets.
For each staged node, record target path, exact negative scenario, expected
oracle, release-binary/host requirement, and result parser before promoting
`authority_state`. Candidate owners are:

| Node | Existing owner to inspect; edit only on a proved gap | Required release distinction |
|---|---|---|
| P03 | `crates/quanta-index-repomap/src/{store,pinned}.rs`, `crates/quanta-index-searchd-runtime/tests/e2e_generation_activation_concurrency.rs` | Live activation/recovery, ACK replay and no publish-only activation. |
| P04 | `crates/quanta-index-search-plane/src/query_dispatcher/read_view/view.rs`, `crates/quanta-index-searchd-runtime/tests/e2e_read_view.rs` | Pinned handle across activate/retire/GC, not just an owner-local view. |
| P05 | `crates/quanta-index-contract-base/src/results/query_window.rs`, `crates/quanta-index-searchd-runtime/tests/e2e_keyset_cursors.rs`, quality oracle | Independent order/completeness oracle; a quality summary cannot replace it. |
| P06 | `crates/quanta-index-sdk/src/binding.rs`, `crates/quanta-index-sdk/tests/sdk_binding_owner_v1.rs` | Real daemon same-variant/wrong-identity refusal. |
| P07 | `crates/quanta-index-search-plane/src/query_embedder.rs`, provider owner test | Approved real-provider identity/budget/cancellation evidence, separate from spy tests. |
| P08 | `crates/quanta-index-searchd/src/app/{supervisor,searchd}.rs`, runtime supervisor owner test | Release binary signal/child-loss/FD/lease residue, separate from in-process supervisor tests. |

DoD per node: a concrete registered target and independent negative oracle;
no staged-to-executable flip based only on a broad `test-daemon` recipe or a
test name. If the existing code and target already cover the case, no product
code edit is authorized: only bind the executable rail and raw result.

### R2 — P09 process truth and one operator diagnostic path

Owners: `crates/quanta-index-searchd/src/app/readiness.rs`,
`crates/quanta-index-searchd/src/app/runtime.rs`,
`crates/quanta-index-ipc/src/counters.rs`,
`crates/quanta-index-contract/src/ipc/{control,split}.rs`,
`crates/quanta-index-search-plane/src/control_dispatcher.rs`,
`crates/quanta-index-sdk/src/{binding,observability}.rs`,
`crates/quanta-index-searchctl/src/lib.rs`, and
`crates/quanta-index-searchd-runtime/tests/e2e_process_readiness.rs`.

- Replace `required_backend: true` with an owned, live required-backend health
  observation that fails closed. Specify which opened backend handles and
  catalog/scrub invalidations it covers; do not perform an unbounded full
  repository scan on every readiness call or mistake an initial boot proof for
  ongoing health.
- Root-loss counterexample (confirmed on pre-fix source): both lexical and
  semantic `inventory_*generations` return an empty successful inventory when
  their track root is missing; both `track_disk_bytes` ports return successful
  zero bytes. A scrub that discovers candidates only from those inventories
  can therefore go idle without incrementing its error epoch, leaving the
  cached active proof and constant backend Boolean apparently healthy after
  an active track root disappears. The live signal must compare durable active
  expectations with observed backend state, or invalidate/re-prove that exact
  active set through a bounded owner. Do not equate empty inventory/zero bytes
  with health when active generations are named; missing roots remain valid
  before any generation has been published. Define and test a maximum
  root-loss detection interval, including recovery after an authorized
  restore, without a full physical re-open on every readiness poll.
- The local implementation in progress probes each active sealed identity on
  the maintenance timer and binds the observation to the activation token;
  the default readiness staleness horizon is three five-second maintenance
  ticks. This closes the missing-root/marker liveness case in a dirty-overlay
  runtime test, not the full P09 event/provenance scope or clean-HEAD proof.
- Define a bounded control DTO and adapter projection from the *existing*
  `IpcServerCounters` ring. Bound event count and encoded bytes below the
  transport's 16 MiB frame cap; carry process instance, plane, sequence gap
  and drop totals. Never put request ID or payload in metric labels.
- Reuse the existing `Admin` owner/root admission for operator-only
  diagnostics unless a concrete distinct principal policy requires a new
  `Operate` capability. Authorization must precede ring read. Do not create a
  parallel ring, self-asserted principal, or a metrics-snapshot backdoor.
- DoD: observer denial with zero ring disclosure; operator success; invalid
  limit/oversize refusal; wrap/drop/instance restart; real daemon request ID
  queue→backend/provider→terminal correlation; supervised child, maintenance
  and backend loss independently make global readiness false. A post-boot
  rename/removal of either active lexical or semantic track root must make
  readiness false within the declared detection interval even when the
  inventory returns `Ok(empty)` and disk usage returns `Ok(0)`; the same
  missing root with zero active generations must not be misreported as loss.

### R3 — Semantic omission oracle before changing the ingest wire

Owners to inspect: `crates/quanta-index-contract/src/ipc/ingest.rs`,
`crates/quanta-index-search-plane/src/semantic_derive.rs`, and Semantica's
`search_plane_handoff_dispatch/{lexical_batch,semantic_state}.rs` plus the
source-plan/manifest that precedes them.

Prove whether that upstream source-plan independently enumerates required
semantic scope changes. If yes, bind its digest and exact scope partition to
the existing batch/terminal receipt and refuse omitted or duplicated scopes
before mutation. If not, do not invent a producer-authored completeness flag;
keep the documented lexical-only/no-op behavior and require a producer-side
oracle over source inputs. DoD distinguishes a legitimate unchanged semantic
delta from an erroneous missing scope without relying on the value being
checked to assert its own completeness.

### R4 — P10 current-format state custody

Owners: `crates/quanta-index-searchd-runtime/src/state_migration.rs`,
`crates/quanta-index-searchd-runtime/tests/state_migration_owner_v1.rs`,
`crates/quanta-index-searchd/src/cli/command.rs`, and the operator runbook.
The backup/restore/verify CLI and legacy typed refusal already exist. Add
release-process counterexamples only where the existing owner suite does not
cover them; inventory actual state roots and retention obligations before any
cutover. DoD: stopped daemon + exclusive lease, current-format backup,
verify, restore-forward, activation-incarnation rotation and wrong-format
refusal on one disposable and one authorized target root. No legacy importer.

### R5 — P11 exact source pair, resolved dependency roots and build result

Owners: `scripts/verify-repomap-cross-repo.sh`, Semantica's
`quanta-runtime-retrieval-kernel/Cargo.toml` and RepoMap producer/validator,
`tools/ci/lint/check-proof-authority.py::paired_source_snapshot`, the common
R0 result authority, and the P11 negative tests.

Before the fresh build, resolve every Semantica `quanta-index-*` path package
through the canonical build graph and require it to lie under the frozen
Quanta checkout at the expected package root; bind that mapping and lock
digest into the paired receipt. Then run the already selected live V2
publish/activate/restart/query and forged-receipt tests through QBC. Archive
the actual build/test result, not just the release binary bytes. DoD: a clean
Semantica checkout using a second Quanta checkout, a wrong binary, wrong
bundle, wrong transition, or ACK replay divergence is refused.

### R6 — P11 operational actions, then P12 custody

First obtain exact authorized Linux host, deployment path, config, state root,
retention and rollback window. Then add separate `Justfile` recipes and typed
result producers for `proof-p11-{deployment,activation,rollback}`. Extend the
registry/checker with an operational-action mode instead of pretending these
are test-authority selections. Deployment binds installed binary/config/root;
activation observes the serving identity and query path; rollback drills the
P10-allowed restore-forward boundary. DoD: each stage has a distinct external
pre/post observation and independently issuable manifest; deployment alone
cannot yield activated or rollback-proven.

Audit available historical P00-P11 handoffs with `handoff_validation.py`;
do not fabricate missing records. These records are not release prerequisites.
On the final frozen clean pair, reissue all current
owner/release proofs, P12A, and aggregate. `--require-all --bind-source`
must accept the exact pair and raw evidence; deployment, activation and
rollback verdicts remain separate. Missing, staged or stale evidence is not
production readiness.

Use [EXECUTION-PROGRESS.md](EXECUTION-PROGRESS.md) for the live result. This
plan is an action map, not a passed receipt.
