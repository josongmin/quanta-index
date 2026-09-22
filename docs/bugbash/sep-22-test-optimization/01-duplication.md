# Duplication Audit — retained actions only (2026-09-22)

Audited snapshot: `23bd3d7fa7af1122f904e59f1f514935bd5ffe7e`.

Scope: Rust tests under `crates/`, with source bodies and owning APIs opened at
the cited lines. Repeated test names or assertion shapes are not findings by
themselves. An item remains only when duplicated ownership has already produced
a reachable platform failure or a concrete lifecycle race.

## D1 — Direct-runtime fixture copies omit the ingest socket override

- Severity: **M**
- Evidence:
  - `crates/quanta-index-searchd-runtime/tests/end_to_end.rs:307-333` creates
    flat temporary paths only for query/control. Its own comment at `:329-331`
    says a socket below the temp state root can exceed macOS's AF_UNIX path
    limit, but ingest remains at the state-root default.
  - `crates/quanta-index-searchd-runtime/tests/repo_map_end_to_end.rs:76-102`
    repeats the same two-socket builder and also leaves ingest at the default.
  - `crates/quanta-index-searchd/src/app/config.rs:806-821` explicitly defines
    `with_socket_overrides` as query/control-only and requires
    `with_ingest_socket_override` for ingest.
  - The other copies already use three flat paths:
    `crates/quanta-index-searchd-runtime/tests/dsl_scenarios.rs:130-155`,
    `crates/quanta-index-searchd-runtime/tests/explain.rs:86-114`, and
    `crates/quanta-index-searchd-runtime/tests/sdk_frontdoor.rs:105-132`.
    The harness owner does the same at
    `crates/quanta-index-searchd-harness/src/harness.rs:3428-3446`.
- Impact: on a sufficiently long temporary state-root path, query/control bind
  to short flat paths while ingest can fail to bind at
  `state_root/search-plane/ingest.sock`. The direct-runtime suites therefore
  retain a platform/path-length failure already avoided by the other fixtures.
- Owner: `crates/quanta-index-searchd-harness` for test runtime configuration;
  `quanta-index-searchd-runtime` should consume it rather than own more socket
  builders.
- Fix direction: expose one harness-owned direct-runtime config/driver builder
  that always allocates and overrides query, control, and ingest sockets. Keep
  per-suite fixture data and special embedder settings as explicit parameters.
  Delete the five local socket/config/poll copies after migration; do not add a
  sixth macro or `#[path]` helper with another default set.
- Verification:
  - static: every direct-runtime config either calls the harness owner or has
    all three explicit overrides;
  - focused runtime: `just rust-profile test-daemon-fast` (includes
    `end_to_end`) and `just rust-profile test-daemon` (includes the risk-suite
    `repo_map_end_to_end` surface);
  - macOS: run with a deliberately long state-root and prove all three sockets
    bind and are cleaned up.

## D2 — Umask fixture forks the shared process lifecycle and retains a kill race

- Severity: **M**
- Evidence:
  - `crates/quanta-index-searchd-runtime/tests/e2e_umask_hardening.rs:64-95`
    locally copies socket readiness, termination, and polling.
  - The shared owner at
    `crates/quanta-index-searchd-runtime/tests/common/searchd_binary_process.rs:63-128`
    implements the same lifecycle, but its `terminate_child` accepts
    `InvalidInput`/`NotFound` when the child exits between `try_wait` and
    `kill` (`:101-114`). The local `terminate` at `e2e_umask_hardening.rs:70-75`
    turns that race into a test failure and can mask the scenario's real result.
  - Both modules also duplicate the four required retention environment values
    at `e2e_umask_hardening.rs:41-54` and
    `common/searchd_binary_process.rs:130-153`.
- Impact: a normal child-exit race can surface as cleanup failure; future
  changes to required daemon test environment can update the shared process
  fixture while leaving the umask launch stale.
- Owner:
  `crates/quanta-index-searchd-runtime/tests/common/searchd_binary_process.rs`
  for process lifecycle and required environment; the umask test owns only the
  shell/umask wrapper and permission oracle.
- Fix direction: expose/reuse `wait_for_sockets` and `terminate_child`, and
  extract a shared `configure_searchd_test_env(&mut Command, max_generations)`
  used by both direct and shell-wrapped commands. Preserve the umask-specific
  command construction.
- Verification: focused `runtime_extended_suite` execution for
  `e2e_umask_hardening`; add a deterministic termination test with an already
  exited child and require successful cleanup.
