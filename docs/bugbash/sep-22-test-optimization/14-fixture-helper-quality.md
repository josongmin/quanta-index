# 14 — Fixture / Helper Quality Audit (tests/common, helpers, builders)

Scope: `crates/quanta-index-searchd-runtime/tests/` (+ `common/`), `crates/quanta-index-searchd-harness/src/`,
`crates/quanta-index-search-plane/src/query_dispatcher/tests/support/common.rs`,
`crates/quanta-index-semantic/src/semantic_ingest_fixtures_v1.rs`.
Every file cited below was opened and read; search-hit-only claims were excluded.

## Summary

The harness itself (`E2eRuntime` in `searchd-harness`) is good: one owned builder with
`boot_*` variants. The problem is everything around it — each e2e file re-implements the
same 4 helpers (`expect_eq`, `require_no_typed_error`, `mode_of`, `wait_*`) and the same
runtime-boot scaffolding (`unique_socket_paths` / `build_config` / `start_runtime`), while
the biggest fixtures (4563-line `sdk_frontdoor.rs`, 3492-line `end_to_end.rs`, 2459-line
`e2e_full_corpus.rs`) inline hundreds of lines of batch builders per file. One shared
`tests/common/` with real `mod` ownership (not `#[path]` includes) removes ~400 duplicated
lines and gives every magic constant one home.

## Findings

### F1 — Triplicated byte-identical `expect_eq` (duplicated setup fn)
- Files/lines: `crates/quanta-index-searchd-runtime/tests/e2e_metrics_scrape.rs:99`,
  `crates/quanta-index-searchd-runtime/tests/e2e_process_envelope.rs:79`,
  `crates/quanta-index-searchd-runtime/tests/e2e_socket_access.rs:33`
- Symptom: the same 7-line generic `fn expect_eq<T: PartialEq + Debug>(what, observed, expected)`
  copy-pasted into three files.
- Why bad: a message-format fix must land in 3 places; drift is invisible because each
  compiles independently.
- Severity: M
- Fix: move one copy to `tests/common/assert.rs` as `pub(super) fn expect_eq`; delete the
  three locals; `#[path]`-include (or better, convert suites to `mod common;`) once.

### F2 — Duplicated `require_no_typed_error` (duplicated setup fn)
- Files/lines: `crates/quanta-index-searchd-runtime/tests/e2e_perf_chaos.rs:22`,
  `crates/quanta-index-searchd-runtime/tests/e2e_restart_replay_determinism.rs:40`
- Symptom: byte-identical 10-line `fn require_no_typed_error(error: Option<E2eTypedError>, context)`
  in both files.
- Why bad: same as F1; this is the most-called guard in e2e tests so it belongs in one place.
- Severity: M
- Fix: same `common/assert.rs` home as F1: `pub(super) fn require_no_typed_error`.

### F3 — Duplicated `mode_of` + socket-probe scaffolding
- Files/lines: `crates/quanta-index-searchd-runtime/tests/e2e_socket_access.rs:41` vs
  `crates/quanta-index-searchd-runtime/tests/e2e_umask_hardening.rs:60`
  (`fn mode_of` identical); `e2e_umask_hardening.rs:64` (`socket_accepts_connection`) and
  `:70` (`terminate`) vs `crates/quanta-index-searchd-runtime/tests/common/searchd_binary_process.rs:65,101,117`
  (`wait_for_sockets`, `terminate_child`, `socket_accepts_connection`).
- Symptom: umask test re-implements the shared real-process fixture locally instead of reusing it.
- Why bad: two socket-readiness definitions can disagree (e.g. one checks file-type, one
  doesn't); bug fixes to reconnect logic miss the copy.
- Severity: H (readiness logic is correctness-critical for real-process tests)
- Fix: `e2e_umask_hardening.rs` imports `searchd_binary_process::{terminate_child,
  socket_accepts_connection}` (expose as `pub(super)`); keep only its umask-specific
  `searchd_under_umask` constructor. Move `mode_of` to `common/socket.rs`.

### F4 — Duplicated runtime-boot trio (`unique_socket_paths` / `build_config` / `start_runtime`)
- Files/lines: `crates/quanta-index-searchd-runtime/tests/dsl_scenarios.rs:130,143,185` vs
  `crates/quanta-index-searchd-runtime/tests/sdk_frontdoor.rs:105,119,133,170`
  (plus `repo()/revision()/generation()` at `dsl_scenarios.rs:70-80` vs `sdk_frontdoor.rs:70-98`;
  `wait_until` at `dsl_scenarios.rs:171` vs `sdk_frontdoor.rs:240`).
- Symptom: two ~80-line in-process `build_runtime` harnesses doing the same thing with
  different socket prefixes (`qi-dsl-*` vs `qi-sdk-*`).
- Why bad: retention/socket policy drift between the two boots means DSL and SDK tests run
  against different daemon configs silently; every socket-policy change needs 2+ edits.
- Severity: H
- Fix: single `common/runtime.rs` owning `unique_socket_paths(prefix)`, `build_config`,
  `start_runtime`, `wait_until`, `repo()/revision()/generation()`; both files call it with
  their prefix. Long term, migrate both onto `E2eRuntime` from `searchd-harness`.

### F5 — Magic retention tuple + env block in ≥3 places (magic strings)
- Files/lines: `crates/quanta-index-searchd-runtime/tests/common/searchd_binary_process.rs:136-152`
  vs `crates/quanta-index-searchd-runtime/tests/e2e_umask_hardening.rs:41-54`
  vs `crates/quanta-index-searchd-runtime/tests/dsl_scenarios.rs:143-151`
  (and `sdk_frontdoor.rs:119-131`).
- Symptom: `QUANTA_INDEX_EMBEDDER=hash-dev` + 4 `QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_*`
  vars (`8`, `16*1024*1024`, `128`, `256*1024*1024`) spelled out literally in each file.
- Why bad: bumping a retention default requires finding every literal; a missed file tests
  a different GC window.
- Severity: M
- Fix: `common/runtime.rs` owns `pub const DEFAULT_RETENTION: (usize,u64,u64,u64) = (8, 16MiB, 128, 256MiB)`
  and `pub fn apply_default_env(cmd)`. `searchd_binary_process::searchd_command` becomes the
  only real-process constructor; umask file calls it (or shares the env fn).

### F6 — Inconsistent `SOCKET_TIMEOUT` (magic number, 3 values)
- Files/lines: `crates/quanta-index-searchd-runtime/tests/common/searchd_binary_process.rs:15`
  (`30s`) vs `crates/quanta-index-searchd-runtime/tests/sdk_frontdoor.rs:68` (`5s`) vs
  `crates/quanta-index-searchd-runtime/tests/e2e_umask_hardening.rs:27-28` (`30s` + `EXIT_TIMEOUT`).
- Symptom: no shared timeout; SDK tests fail 6x faster than real-process tests under load.
- Why bad: flake reports can't distinguish "daemon slow" from "timeout too short" across files.
- Severity: M
- Fix: one `pub const SOCKET_TIMEOUT` in `common/runtime.rs`; per-test overrides pass an
  explicit arg instead of a private const.

### F7 — Socket-path literals in 3+ files (magic paths)
- Files/lines: `crates/quanta-index-searchd-runtime/tests/common/searchd_binary_process.rs:67-69,158-160`
  (`search-plane/{query,control,ingest}.sock`) duplicated inside the same file (wait vs
  cleanup) and re-listed in `crates/quanta-index-sdk/src/config.rs` and
  `crates/quanta-index-searchd/src/app/config.rs`.
- Symptom: the triple is spelled out twice in one file and again in src.
- Why bad: adding a fourth socket (or renaming) misses a cleanup site → leaked sockets,
  cross-test interference.
- Severity: M
- Fix: `pub fn search_plane_sockets(state_root) -> [PathBuf; 3]` in one place
  (SDK config re-exported for tests); both `wait_for_sockets` and `remove_socket_files`
  iterate it.

### F8 — 200+ line inline fixtures: `sdk_frontdoor.rs` (4563 lines)
- File/lines: `crates/quanta-index-searchd-runtime/tests/sdk_frontdoor.rs:1-4563`
  (batch builders `history_batch:263`, `lexical_batch:323`, `lexical_frontdoor_matrix_batch:387`,
  `repo_map_bundle:838`, `structural_batch:995`, … ~30 `fn`s; grep `^fn ` lists ~40 helpers).
- Symptom: the test file IS the fixture library; nothing else can reuse
  `lexical_frontdoor_matrix_batch` without copying it.
- Why bad: unreadable diffs, 4.5k-line compile unit, builders untestable in isolation,
  inevitable copy-paste into the next frontdoor test.
- Severity: H
- Fix: extract `common/frontdoor_fixtures.rs` (lexical/history/repo-map/structural batch
  builders) + keep scenario tables in `common/frontdoor_scenarios.rs` (already extracted —
  the good pattern). Target: `sdk_frontdoor.rs` < 1500 lines of test logic only.

### F9 — 200+ line inline fixtures: `end_to_end.rs` (3492), `e2e_full_corpus.rs` (2459),
  `e2e_dual_syntax_lowering_parity.rs` (2723), `e2e_filter_execution.rs` (2582)
- Files/lines: sizes from `wc -l`; e.g. `e2e_full_corpus.rs:288-403` hand-rolls a TOML
  fixture parser (`require_string/optional_string/require_bool/require_u64/require_u32/
  require_string_array`) instead of `#[derive(Deserialize)]`.
- Symptom: each mega-file carries its own corpus + parser + assertions.
- Why bad: same fixture rows redefined per file with different ids (`repo-e2e` vs `repo-dsl`
  vs `repo-metrics`, 193 grep hits for fixture ids/needles); parser helpers are untested
  stringly-typed code.
- Severity: H
- Fix: (a) derive Deserialize for the TOML corpus spec, delete the 6 `require_*` fns;
  (b) move shared rows to data files under `tests/fixtures/` (only `lexical_corpus/`
  exists today) loaded by one loader; (c) split `end_to_end.rs` per route.

### F10 — Helpers hiding assertions: `wait_for_scrape` resolves timeout as success
- File/line: `crates/quanta-index-searchd-runtime/tests/e2e_process_envelope.rs:108-119`
  (`if condition(&scrape) || started.elapsed() > bound { return Ok(scrape); }`;
  same shape at `e2e_integrity_scrub.rs:143`).
- Symptom: on timeout the helper returns the last (failing) scrape as `Ok`; the test then
  asserts on stale data and reports "gauge wrong" instead of "condition never became true".
- Why bad: hides the real failure (timeout vs wrong value), wastes triage time; classic
  helper-hides-assertion smell.
- Severity: H
- Fix: return `Err(timeout with last scrape dumped)` when the bound expires; keep the
  comment contract "assertion is on what the scrape says" for the success path only.

### F11 — Helpers hiding assertions: `E2eRoutePage::served()` + `check_page` family
- Files/lines: `crates/quanta-index-searchd-harness/src/harness.rs:390`
  (`served(self, what)` turns refusal into `Err`); `e2e_keyset_cursors.rs:124`
  (`check_page`), `e2e_full_corpus.rs:1658,1676,1694` (`assert_expected_paths/snippets/bindings`),
  `sdk_frontdoor.rs:1276,1392` (`assert_structural_single_binding`, `assert_single_symbol_candidate`).
- Symptom: assertion logic (which refusal codes are OK, single-candidate shape) lives inside
  helpers taking a `&str what` context param; failure messages point at the helper, not the test.
- Why bad (mild here): the `what`-context convention actually preserves diagnosability —
  this is the one "hiding" pattern that is acceptable, but it is undocumented so new helpers
  (F10) don't follow it.
- Severity: L
- Fix: document the convention in harness.rs ("helpers take `what: &str`, never assert,
  return TestResult") and make F10's helper follow it. No code change to `served()`.

### F12 — Fragile cross-module import in `common/e2e_corpus.rs` + blanket `expect(dead_code)`
- Files/lines: `crates/quanta-index-searchd-runtime/tests/common/e2e_corpus.rs:11`
  (`use super::e2e_harness::E2eRuntime;` — resolves only because `runtime_fast_suite.rs:3`
  aliases `quanta_index_searchd_harness as e2e_harness` in the parent module);
  `crates/quanta-index-searchd-runtime/tests/common/frontdoor_scenarios.rs:2-5`
  (`#![expect(dead_code, reason = "consumed selectively")]`).
- Symptom: `e2e_corpus.rs` compiles only inside `runtime_fast_suite.rs`, not
  `runtime_extended_suite.rs`/`runtime_risk_suite.rs`; the blanket `expect` hides genuinely
  dead scenarios from the compiler.
- Why bad: moving a `#[path]` include to another suite breaks the build with a confusing
  error; dead scenarios accumulate silently.
- Severity: M
- Fix: `e2e_corpus.rs` uses `quanta_index_searchd_harness::E2eRuntime` directly (like every
  other test file); replace blanket `#![expect(dead_code)]` with per-item allows or,
  better, a registry test that iterates `SDK/IPC/DSL_FRONTDOOR_SCENARIOS` so all are consumed.

### F13 — `#[path]`-include composition has no ownership (structural)
- Files/lines: `crates/quanta-index-searchd-runtime/tests/runtime_fast_suite.rs:5-10`,
  `runtime_extended_suite.rs:3-6`, `runtime_risk_suite.rs:5-6` — each suite re-declares
  `#[path = "common/..."] mod ...;` for a different subset.
- Symptom: no `common/mod.rs`; which suite gets which helper is decided at each include site
  (e.g. `e2e_corpus` only in fast, `searchd_lease_probe` only in extended).
- Why bad: adding a helper means editing N suite files; a helper used by two suites gets
  compiled twice with possibly different cfg; ownership unclear.
- Severity: M
- Fix: add `tests/common/mod.rs` declaring all submodules once; suites do
  `#[path = "common/mod.rs"] mod common;` and refer to `common::…`. This is the file-ownership
  change that makes F1–F7 stick.

### Counter-examples (checked, NOT findings)
- `crates/quanta-index-search-plane/src/query_dispatcher/tests/support/common.rs:25-80` —
  small named builders (`build_probe_query`, `corpus_generation`, `test_activation_catalog`);
  the right size for shared helpers. No change.
- `crates/quanta-index-semantic/src/semantic_ingest_fixtures_v1.rs:23-60` — centralized
  `embedding_record_v1`/`legacy_chunk_embedding_record_v1` used by integration + owner tests;
  the pattern F8/F9 should copy. No change.
- `harness.rs:683-689` `clippy::panic` expect on daemon-restart abort — justified, documented.
  No change.

## Proposed shared builder / fixture layout with file ownership

New `crates/quanta-index-searchd-runtime/tests/common/` (owned by searchd-runtime test owners):

| File | Owns | Absorbs |
|---|---|---|
| `common/mod.rs` (new) | single include point for all suites | F13 |
| `common/assert.rs` (new) | `expect_eq`, `require_no_typed_error`, `expect_remote_code` | F1, F2 |
| `common/socket.rs` (new) | `mode_of`, `socket_accepts_connection`, `search_plane_sockets`, `SOCKET_TIMEOUT` | F3, F6, F7 |
| `common/runtime.rs` (new) | `DEFAULT_RETENTION`, `apply_default_env`, `unique_socket_paths`, `build_config`, `start_runtime`, `wait_until` | F4, F5, F6 |
| `common/frontdoor_fixtures.rs` (new) | batch builders from `sdk_frontdoor.rs` | F8 |
| `common/frontdoor_scenarios.rs` (keep) | scenario tables (already good) | — |
| `common/searchd_binary_process.rs` (keep, slim) | real-process fixture only | F3, F5 |
| `common/searchd_lease_probe.rs` (merge into above or keep) | lease probe | — |
| `common/e2e_corpus.rs` (fix import) | tiny smoke corpus | F12 |
| `tests/fixtures/*.toml` (extend) | machine-readable corpora | F9 |

Rules: helpers return `TestResult`/`AnyResult`, never assert; every helper takes effect
from explicit args (no hidden 30s/5s/8-generations defaults — defaults live as named consts
in the owning file). Good models already in-repo: `semantic_ingest_fixtures_v1.rs` for
fixture centralization, `frontdoor_scenarios.rs` tables for data/logic separation.

## Suggested order
1. F13 (`mod.rs`) + F12 (direct import) — unlocks everything else.
2. F1, F2, F3 (`assert.rs`, `socket.rs`) — pure moves, no behavior change.
3. F5, F6, F7 (consts/env/paths) — kills magic strings.
4. F10 (timeout Err) — fixes a real diagnostic bug.
5. F4, F8, F9 (boot + fixture extraction) — the large but mechanical splits.
