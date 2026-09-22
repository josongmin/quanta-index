# searchd-runtime tests DEEP dive (2026-09-22)

Scope: `crates/quanta-index-searchd-runtime/tests/` — 50 test files + 4
`common/` helpers (54 files). Every finding cites a file body opened during
this dive (search hits alone were not used).

Structural fact that shapes everything below: `Cargo.toml:7` sets
`autotests = false` and declares exactly 5 `[[test]]` targets. The 45+
`e2e_*.rs` / `end_to_end.rs` / `sdk_frontdoor.rs` / `dsl_scenarios.rs` files
are **not** standalone targets — they are pulled into the three suite
binaries via `#[path]` module includes. The partition is disjoint (no file
runs twice), but each suite binary compiles thousands of lines as one unit.

Suite partition (verified by reading all three suite files):

- `runtime_fast_suite.rs:29` — explain_score_trace, hybrid_filters,
  ingest_idempotency, matrix_inventory, read_view, structural_hellgate,
  text_route_hellgate, end_to_end, sdk_frontdoor (+ common e2e_corpus,
  frontdoor_scenarios, searchd_binary_process).
- `runtime_risk_suite.rs:49` — dsl_scenarios, aux_epoch, boot_quarantine,
  dual_syntax_lowering_parity, exact_count_window, filter_execution,
  full_corpus, generation_activation_concurrency, history_relevance,
  keyset_cursors, lexical_full_fidelity, perf_chaos, physical_gc,
  predicate_authority_boolean/lifecycle, restart_replay_determinism,
  semantic_scope_cap, snapshot_registry, top_k_truth_table, explain,
  repo_map (+ common frontdoor_scenarios).
- `runtime_extended_suite.rs:45` — composite_generation_authority_restart,
  ann_incremental_seal, auxiliary_catalog, crash_matrix, history_order,
  history_text_predicate, ingest_preflight, ingest_resource_envelope,
  integrity_scrub, lexical_sealed_overlays, metrics_scrape,
  process_envelope, ranked_pages, semantic_budget_interruption,
  semantic_stream_window, socket_access, umask_hardening,
  unicode_text_semantics, semantic_boot_report (+ common
  searchd_binary_process, searchd_lease_probe).
- `runtime_supervisor_owner_v1.rs:588`, 10 tests — in-process supervisor
  unit/integration tests, standalone target.
- `state_migration_owner_v1.rs:981`, 27 tests — offline
  migrate/backup/restore/verify over tempdirs, standalone target.

## File → behavior map

| File | Tests | Behavior |
|---|---|---|
| `end_to_end.rs` | 18 | Legacy in-process runtime rail (lexical/semantic/history/scope/readiness); 1 ignored OpenAI test |
| `sdk_frontdoor.rs` | 15 | SDK-typed publish/query frontdoor over in-process runtime |
| `dsl_scenarios.rs` | 8 | Sourcegraph/LQ/hybrid DSL determinism over in-process runtime |
| `e2e_filter_execution.rs` | 44 | Per-predicate filter matrix (rev/author/committer/message/meta/topic/description/owner/contributor/path), fresh boot per group |
| `e2e_perf_chaos.rs` | 43 | Boundedness/fail-closed/metric-label rails (regex verify, timeouts, early-stop, closed labels) |
| `e2e_restart_replay_determinism.rs` | 20 | Reopen + fresh-reingest determinism per route |
| `e2e_full_corpus.rs` | 4 | TOML-driven full-corpus rail + 3 fixture-loader unit tests |
| `e2e_dual_syntax_lowering_parity.rs` | 4 | Native-vs-Sourcegraph lowering parity matrix (2 runtimes + reopen, aggregated verdict) |
| `e2e_boot_quarantine.rs` | 6 | Boot quarantine lifecycle (inactive damage, active damage, list/discard, rollback-proof, one-track seal) |
| `e2e_crash_matrix.rs` | 4 | Real-binary crash at every seal/GC point + converge + coverage meta-test |
| `e2e_top_k_truth_table.rs` | 6 | top_k accept/refuse/window-consistency table over 8 routes |
| `e2e_metrics_scrape.rs` | 4 | Exact counter/gauge movement per traffic sent + provider/cache + regex-cache tallies |
| `e2e_physical_gc.rs` | 6 | Retention reclaims bytes on both tracks; reaped generation refused/listed/discarded |
| `e2e_integrity_scrub.rs` | 3 | Cheap doors + quota'd scrub finds tamper; old format refused |
| `e2e_keyset_cursors.rs` | 2 | Keyset cursor walks partition result sets; epoch expiry refused typed |
| `e2e_snapshot_registry.rs` | 5 | Resident-handle proof via delete-on-disk trick; ingest never drops residency |
| `e2e_aux_epoch.rs` | 1 | Epoch-pinned history walk invisible to mid-walk ingest; survives restart |
| `e2e_history_order.rs` | 1 | Recency (not sha) order + cursor walks every match exactly once |
| `e2e_history_relevance.rs` | 1 | BM25 relevance order vs in-test reference + restart + mid-walk ingest |
| `e2e_history_text_predicate.rs` | 1 | Keyword/raw-string predicate parity across both history orders |
| `e2e_hybrid_filters.rs` | 1 | Every DSL filter binds both hybrid lanes (leak = fail) |
| `e2e_exact_count_window.rs` | 4 | count:all/N keeps page, reports exact total; projections report universe |
| `e2e_ranked_pages.rs` | 3 | Tie-ordered pages, cursor continuation, byte-budget cut, generation mismatch |
| `e2e_read_view.rs` | 2 | Per-route read view; absent domains refused with own code |
| `e2e_ann_incremental_seal.rs` | 1 | Delta seal appends to inherited ANN index (inode-fraction oracle) |
| `e2e_auxiliary_catalog.rs` | 2 | Aux rows durable pre-receipt, survive restart, reaped by retention |
| `e2e_ingest_idempotency.rs` | 4 | Canonical digest idempotency: replay applied=false, mismatch typed, 8/32-thread converge, survives restart |
| `e2e_ingest_preflight.rs` | 2 | Ingest fault matrix (raw IPC + SDK side), refusals change nothing |
| `e2e_ingest_resource_envelope.rs` | 2 | Over-envelope batch refused typed, zero bytes changed |
| `e2e_lexical_full_fidelity.rs` | 1 | Table rail with closed-loop ExpectedFailing semantics |
| `e2e_lexical_sealed_overlays.rs` | 2 | Sealed-generation overlay publish refused; torn overlay refused at boot/query |
| `e2e_semantic_budget_interruption.rs` | 1 | In-lane deadline answered from lane, counted, daemon unpoisoned |
| `e2e_semantic_scope_cap.rs` | 3 | scope_top_k caps semantic draw from lexical top-N |
| `e2e_semantic_stream_window.rs` | 1 | 32-row batch streams as 4 windows; all rows serve after restart |
| `e2e_socket_access.rs` | 2 | Shared-vs-private socket modes via stat; metrics gauges match |
| `e2e_structural_hellgate.rs` | 2 | Structural + semantic-source authority hellgate |
| `e2e_text_route_hellgate.rs` | 3 | Text-route authority/predicate hellgate |
| `e2e_unicode_text_semantics.rs` | 2 | Normalizer goldens through daemon (Greek sigma regression) + regex/case refusal |
| `e2e_explain_score_trace.rs` | 1+5 | Score-trace fixture shared by 5 verify fns (page/boost/presence/hybrid/unmatched) |
| `e2e_generation_activation_concurrency.rs` | 2 | Readers racing activation see only complete G1 or G2 |
| `e2e_process_envelope.rs` | 4 | Scripted-probe memory envelope, writer RSS gate, idle sweep, over-ceiling boot refusal |
| `e2e_umask_hardening.rs` | 2 | Real binary under umask 000 → private paths; 0777 root refused typed |
| `e2e_predicate_authority_boolean.rs` | 2 | Boolean composition correlates by (repo, path), not global sets |
| `e2e_predicate_authority_lifecycle.rs` | 1 | Multi-repo authority shards + projections survive reopen |
| `composite_generation_authority_restart.rs` | 9 | Composite identity rollback/restart incl. real child-process restarts + lease |
| `explain.rs` | 3 | Presence-only explain over in-process runtime |
| `repo_map_end_to_end.rs` | 5 | Repo-map publish/query over in-process runtime |
| `semantic_boot_report.rs` | 3 | Boot report populated + payload-free (direct-open vs migration) |
| `e2e_matrix_inventory.rs` | 1 | Harness smoke: write→seal→reopen→query + typed-error surface |
| `runtime_supervisor_owner_v1.rs` | 10 | Supervisor drain/escalation/lease incl. 2-process test |
| `state_migration_owner_v1.rs` | 27 | Offline migrate/backup/restore/verify matrices over tempdirs |
| `common/e2e_corpus.rs` | — | SMOKE_CORPUS fixture rows |
| `common/frontdoor_scenarios.rs` | — | Shared frontdoor scenarios (compiled into fast+risk) |
| `common/searchd_binary_process.rs` | — | Real-process fixture, 30s socket timeout (compiled into fast+extended) |
| `common/searchd_lease_probe.rs` | — | Second-owner lease probe (extended only) |

## Overlap with 01–05 findings (already reported, still applicable here)

- 02 flagged `runtime_supervisor_owner_v1.rs:261` (30s sleeper child) and
  `:486` (3s lease hold) — confirmed above, ranked below.
- 02 flagged `sdk_frontdoor.rs:170`-area 10ms polls and `:1320`/`:1349`
  `wait_for_sdk_*` spins — confirmed (`sdk_frontdoor.rs:249`,
  `:1320`, `:1349`, `:1357`, `:1385`, all `thread::sleep(10ms)`).
- 02 flagged `common/searchd_binary_process.rs:91` (`SOCKET_TIMEOUT`
  30s at `:15`) and `:131` (10ms connect poll) — confirmed.
- 02 flagged `e2e_generation_activation_concurrency.rs:166` (5ms poll)
  and `e2e_integrity_scrub.rs:161` (`SCRUB_WAIT` 120s) — confirmed.
- 03 flagged `runtime_supervisor_owner_v1.rs:161-162,215`-area weak
  drop-guard assertions and `end_to_end.rs:1759-1760` (ignored OpenAI
  test errors when key missing) — confirmed.
- 04 flagged `e2e_filter_execution.rs:39` (`boot_with_history`),
  `repo_map_end_to_end.rs:76-88` / `explain.rs:86-99` /
  `sdk_frontdoor.rs:112-114` (`unique_socket_paths` copies),
  `e2e_process_envelope.rs:352-355` (tempdir boot probe), and the four
  giant files (`sdk_frontdoor.rs:4563`, `end_to_end.rs:3492`,
  `e2e_filter_execution.rs:2582`,
  `e2e_dual_syntax_lowering_parity.rs:2723`) — all confirmed.
- 01 has no searchd-runtime duplication cluster; the suite partition is
  disjoint, so no double-execution overlap to flag.

## Slowest 10 (within this crate)

1. `e2e_filter_execution.rs:39` — ~44 tests each booting a sealed
   runtime (`boot_with_history`, `boot_with_lexical`,
   `boot_with_multi_repo*`, `boot_with_rev_at_time_generations`).
   Symptom: highest boot-per-test ratio in the crate; every predicate row
   pays a full seal+activate. Why bad (H): wall-time scales with row
   count, not behavior count. Fix: share one multi-repo sealed runtime
   across read-only filter tests (re-seal only for the rev_at_time group).
2. `e2e_crash_matrix.rs:240` (`wait_for_exit`) + `:389/:534` — each case
   boots the **real daemon binary**, crashes it, restarts, reseals and
   re-verifies both routes; seal×GC matrix fans out per crash point.
   Symptom: process-spawn-heavy, sequential multi-boot. Why bad (H):
   slowest per-test cost in crate. Fix: keep, but shard cases across
   threads (private state roots already) or gate the GC half behind the
   extended lane only.
3. `e2e_integrity_scrub.rs:55` (`SCRUB_WAIT = 120s`, interval 100ms) —
   Symptom: worst timeout ceiling in crate; a stuck scrub burns 2 min
   before failing. Why bad (H): failure latency, not just pass latency.
   Fix: fail fast on missed progress (no new scrub-step metric in N
   intervals → fail), keep 120s only as a last-resort guard.
4. `sdk_frontdoor.rs:15 tests × in-process boot + wait_for_sdk_*` —
   Symptom: 4563-line binary; every test boots a runtime and spins
   10ms polls (`:249`, `:1320`, `:1349`, `:1357`, `:1385`). Why bad (H):
   poll-per-test + giant compile unit in fast suite. Fix: share one
   booted runtime per test group; split file by area (history/structural/
   symbol) so fast-suite parallelism applies.
5. `end_to_end.rs:18 tests, READINESS_TIMEOUT 15s (:61)` — Symptom: each
   test polls `wait_until(READINESS_TIMEOUT, …)` (`:792`, `:928`,
   `:994`, `:1025`, …) against a freshly booted runtime; a slow host
   multiplies 15s ceilings. Why bad (M): readiness polling per test
   instead of once per boot. Fix: boot once per file (or migrate rows to
   harness-based e2e files), assert readiness once.
6. `e2e_boot_quarantine.rs` (986 lines, 6 tests) — Symptom: every test
   seals 2–3 generations, tampers bytes, restarts, reboots again
   (`quarantine_is_listed_discarded_as_named_and_gone_after_a_reboot`
   at `:347` boots ≥3 times). Why bad (M): restart-heavy. Fix: fold the
   list/discard/reboot assertions into fewer boots (one daemon, staged
   damage).
7. `e2e_full_corpus.rs:2266` — Symptom: parses the whole TOML corpus,
   ingests every row, seals, then executes; fixture-loader unit tests
   (`:2372`, `:2406`, `:2433`) ride the same 2459-line compile unit.
   Why bad (M): failure anywhere re-runs everything; loader tests do not
   need the daemon. Fix: move the 3 loader tests to a unit test in the
   harness or a `#[cfg(test)]` module; keep the rail.
8. `e2e_dual_syntax_lowering_parity.rs:2401` — Symptom: boots **two**
   runtimes (corpus + authority), reopens, then runs the full scenario
   table twice (`run_parity_matrix` at `:2380`). Why bad (M): double
   ingest+seal per run. Fix: ingest both corpora into one runtime (repos
   already distinct) and run the table once.
9. `runtime_supervisor_owner_v1.rs:486`
   (`hold_the_lease` sleeps 3s) + `:261`/`:401` (30s sleeper children,
   bounded by 400ms/abort in practice). Symptom: fixed 3s floor on the
   lease test; 30s threads outlive the tests that spawn them. Why bad
   (M): wall-time floor + thread leak. Fix: signal the holder instead of
   sleeping 3s; join/detach the sleeper children explicitly.
10. `e2e_umask_hardening.rs:27-28` (`SOCKET_TIMEOUT`/`EXIT_TIMEOUT` 30s)
    + `composite_generation_authority_restart.rs:550,586,697` (real
    child-process restarts). Symptom: real-binary boots with 30s
    ceilings each. Why bad (L/M): necessary coverage, but ceilings are
    10–60× the happy path. Fix: shrink timeouts to ~10s (sockets appear
    in ms locally) and keep 30s only under a `SLOW_CI` env override.

## Weakest 10 (within this crate)

1. `runtime_supervisor_owner_v1.rs:442-443` (`p08_lease_child_entry`) —
   Symptom: a `#[test]` that **returns Ok immediately** when
   `QUANTA_INDEX_P08_LEASE_ROLE` is unset, i.e. a green no-op in every
   normal run; the real proof only executes as a spawned child. Why bad
   (H): phantom coverage — counts as a test, proves nothing standalone.
   Fix: gate with `#[ignore]` + runner shim, or assert the parent path
   (`run_two_process_lease_parent`) instead of registering the child
   entry as its own test.
2. `end_to_end.rs:1757-1758` (`openai_semantic_paraphrase_outranks_unrelated_v1`)
   — Symptom: `#[ignore]`, needs a live OpenAI key; without it the file
   exercises nothing neural. Why bad (H): dead in CI by design. Fix:
   keep the ignore but add a hash-embedder paraphrase test that runs
   unignored, so the ranking assertion is proven on every run.
3. `e2e_matrix_inventory.rs:75-76`
   (`harness_smoke_scenarios_share_one_reopened_fixture`) — Symptom:
   asserts candidates non-empty + one path matches + typed error on
   unpinned query. Why bad (M): smoke only; any non-empty wrong set with
   one right path passes. Fix: assert the exact candidate-id set (the
   `id` field on `CorpusRow` in `common/e2e_corpus.rs:18` already exists
   for this).
4. `semantic_boot_report.rs` (3 tests) — Symptom: asserts the boot report
   is populated and payload-free (direct-open vs migration surfaced).
   Why bad (M): proves observability plumbing, not behavior; a report
   that always says "direct-open, 0 seeds" passes. Fix: add one negative
   case (seeded sealed generation → count > 0; forced migration →
   outcome names migration).
5. `e2e_history_relevance.rs:34-35` (`K1 = 1.2`, `B = 0.75` in-test BM25
   reference) — Symptom: the oracle re-implements BM25 with the engine's
   own constants; a shared constant bug passes silently. Why bad (M):
   mirror-of-implementation. Fix: pin a hand-computed ranking for 3–4
   commits (literal expected order) alongside the formula check.
6. `e2e_crash_matrix.rs:582`
   (`the_matrix_has_a_case_for_every_crash_point`) — Symptom: tests the
   test table (coverage of declared points), not the daemon. Why bad
   (L): useful guardrail, weak as behavior proof; counted beside real
   crash cases. Fix: keep but move to a `#[cfg(test)]` unit check on the
   table so the e2e binary holds only behavior tests.
7. `e2e_full_corpus.rs:2372,2406,2433` (fixture-loader rejects tests) —
   Symptom: unit-grade TOML validation tests compiled into the heaviest
   e2e binary; they never touch the daemon. Why bad (L): slow binary for
   fast assertions. Fix: move to harness `#[cfg(test)]` unit tests.
8. `e2e_socket_access.rs` docstring (accept path) — Symptom: the file
   itself admits a single-uid test cannot prove a stranger is refused at
   connect/accept; it proves bind/serve/report only. Why bad (L):
   partial proof by construction. Fix: document the IPC-crate proof link
   in the test name/message so a green here is not read as access-proof.
9. `dsl_scenarios.rs:358`
   (`sourcegraph_repo_path_lang_filters_are_deterministic_across_repeated_runs`)
   — Symptom: runs the same query twice and compares (self-agreement),
   no external oracle. Why bad (L): a deterministically wrong answer
   passes. Fix: assert one literal expected id per scenario (fixtures are
   tiny and static).
10. `e2e_exact_count_window.rs` (4 tests over 5 files) — Symptom: thin
    contract (exact totals on a 5-file fixture); the
    `count:all`-vs-projection boundary is covered but combinatorics
    (count + cursor + projection together) are not. Why bad (L): narrow,
    not wrong. Fix: add one combined test (bounded count + cursor walk
    asserts window totals stay exact across pages).

## Cheesiest 10 (within this crate)

1. `wait_until` triplicated — `sdk_frontdoor.rs:240`,
   `end_to_end.rs:351`, `dsl_scenarios.rs:171`: identical 10ms-poll
   loops. Symptom: 3 copies of the same ~12 lines. Why bad (M): drift
   (timeouts already differ: 15s vs 5s). Fix: one `pub(crate)` helper in
   the harness crate; delete all three copies.
2. `unique_socket_paths` (pid+nanos+counter) copied per file —
   `explain.rs:86-99`, `repo_map_end_to_end.rs:76-88`,
   `sdk_frontdoor.rs:112-114` (04-confirmed) plus same-shape variants in
   `e2e_generation_activation_concurrency.rs`,
   `semantic_boot_report.rs:34-40`, `end_to_end.rs`. Symptom: ~6 copies
   of socket-naming with subtly different prefixes. Why bad (M): AF_UNIX
   length bugs must be fixed N times. Fix: `E2eRuntime`/harness-provided
   socket-path factory; delete copies.
3. `wait_for_sdk_*` 4-way split — `sdk_frontdoor.rs:1313,1331,1341,1363`
   (`wait_for_sdk_ready`, `_observation`, `_with_retry_codes`,
   `_terminal_error`): same loop, different match arms. Symptom: ~90
   lines where one policy enum fits. Why bad (M): every retry-code change
   touches 4 fns. Fix: single `wait_for_sdk(timeout, RetryPolicy,
   Predicate)`; thin wrappers only if call sites need them.
4. Shared `common/` helpers compiled into 2 suite binaries each —
   `frontdoor_scenarios.rs:646` (fast+risk),
   `searchd_binary_process.rs:183` (fast+extended). Symptom: same code
   compiled and monomorphized twice; fixes must land once but build cost
   is paid twice. Why bad (M): build-time cheese. Fix: move both into
   the `quanta-index-searchd-harness` lib crate as real modules.
5. `e2e_lexical_full_fidelity.rs:60-72` — `ExpectedFailing` variant kept
   alive with `#[expect(dead_code)]` while every live row is
   `Candidates`. Symptom: dead-code lint exception checked in for a
   hypothetical. Why bad (L): lint-exception precedent. Fix: delete the
   variant now; re-add in the PR that needs it (the doc-comment already
   describes the protocol).
6. `NEXT_SOCKET_ID` statics per file (`explain.rs:55`,
   `repo_map_end_to_end.rs`, generation-activation file) — Symptom:
   atomic counter duplicated instead of shared. Why bad (L): collapses
   into fix #2. Fix: same harness factory.
7. `expect_*`/`require_*` assertion shims per file
   (`require_no_typed_error` in `e2e_perf_chaos.rs:22`,
   `e2e_restart_replay_determinism.rs:36`, `served` in
   `e2e_aux_epoch.rs:117`/`e2e_read_view.rs`, `expect_eq` in
   `e2e_socket_access.rs:40`) — Symptom: same "bail on typed error"
   shape rewritten per file. Why bad (L): message-format drift hides
   failures. Fix: harness `require_served(result, ctx) -> AnyResult<()>`.
8. `seed_*_fixture` walls in `e2e_perf_chaos.rs:206-566` (~360 lines of
   seed fns before the first `#[test]` at `:589`) — Symptom: fixture
   code outweighs assertions 1:1 in the file. Why bad (L): readers audit
   seeds, not behavior. Fix: move seeds to `common/` or a fixture
   module; keep the 43 one-screen tests.
9. `e2e_process_envelope.rs:352-373`
   (`an_envelope_over_its_ceiling_refuses_boot_typed_before_any_socket`)
   boots a runtime against `std::env::temp_dir()`-joined probe path
   (`:362-364`, 04-flagged). Symptom: shared temp dir + magic
   `declared - 1` ceiling in one body. Why bad (L): collision-prone,
   magic arithmetic. Fix: `private_tempdir()` + named
   `just_below_declared()` constructor.
10. Per-file `repo()`/`revision()`/`generation()` constructors (every
    in-process file, e.g. `explain.rs:61-71`,
    `repo_map_end_to_end.rs:59-69`) — Symptom: same 3 trivial fns with
    different fixture strings in ~10 files. Why bad (L): rename churn ×
    10. Fix: harness `E2eRepo::new("label")` helper or accept the
    3-liners as fixture-local (cheapest: leave, do not "fix" by macro).

## Suggested fix order (value per diff)

1. Share runtimes in `e2e_filter_execution.rs` + `end_to_end.rs`
   (slowest #1/#5) — biggest wall-time win, no coverage loss.
2. Dedupe `wait_until` + socket-path factory into the harness
   (cheesiest #1/#2) — kills ~6 copies and one AF_UNIX bug class.
3. Repair or re-scope the two phantom tests (weakest #1/#2).
4. Fail-fast the scrub wait + shrink 30s binary ceilings (slowest
   #3/#10) — cuts failure latency more than pass latency.
5. Move loader/table-meta tests out of e2e binaries (weakest #6/#7,
   slowest #7) — faster binaries, same assertions.

