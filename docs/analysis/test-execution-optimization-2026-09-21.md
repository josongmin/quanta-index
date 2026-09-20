# Test execution optimization — 2026-09-21

## Source snapshot

- Audit-start HEAD: `3ad279a08879de35fa96a5495a3382af28f095d0`
- Final verification HEAD: `a11085b7af1152df40983ac2c69c53befc8266f6`.
  The shared checkout advanced through `49204a8` and `a11085b` while this work
  was running; validation was repeated after that advance.
- The checkout already contained user-owned contract, SDK, search-plane, and
  public-API baseline edits. They are not part of this optimization.
- The host was not quiet: the timing preflight observed 23 foreign Cargo/rustc
  processes while validation was attempted.

## Confirmed structural causes

1. `validate-shared-surface` selected three target-dir lanes. A cold run built
   substantially overlapping dependency graphs in `all-targets-lane`,
   `test-integration-lane`, and `test-cli-smoke-lane`.
2. `rust-test-integration` launched Cargo 21 times.
3. `rust-test-cli-smoke` launched Cargo twice.
4. `rust-test-e2e` launched Cargo 25 times, serializing test-binary discovery
   and process setup outside Cargo's scheduler.
5. The daemon list was hand-maintained and omitted six risk-bearing targets:
   explain score trace, history relevance, auxiliary epoch, keyset cursors,
   read view, and hybrid filters.
6. Timing rails admitted measurements while unrelated Rust builds saturated
   the same host, so wall-clock comparisons could be invalid without any
   machine-readable refusal.

## Implemented topology

| Scope | Before | After | Selection authority | Test concurrency |
|---|---:|---:|---|---:|
| integration aggregate | 21 Cargo processes | 3 nextest processes, one shared lane | 21 target IDs | per slice |
| integration fast | absent | 1 nextest process | 15 bounded target IDs | 8 |
| integration storage | absent | 1 nextest process | 1 text-authority target ID | 2 |
| integration semantic | absent | 1 nextest process | 5 semantic target IDs | 4 |
| CLI smoke | 2 Cargo processes | 1 nextest process | 2 target IDs | 2 |
| daemon fast | absent | 1 nextest process, 1 binary | 9 runtime scenario sources | 4 |
| daemon risk scope | 25 Cargo processes | 1 nextest process, 3 binaries | 30 runtime sources + DSL truth | 4 |
| daemon exhaustive | absent | 1 nextest process, 4 binaries | all 49 runtime sources + DSL truth | 4 |

The target IDs and owner/path mappings live in
`tools/ci/test-authority.toml`. `tools/ci/run-local-test-scope.py` expands one
or more scopes into one command and refuses selector cross-product leaks. The
existing workspace-wide PR/merge/nightly nextest rails remain authoritative.

`validate-shared-surface` now uses `shared-validation-lane` for one selected
all-target test-profile compile (`nextest --no-run`) and all selected tests.
This replaces a dev-profile `cargo check` followed by a second test-profile
build of the same graph. Library, integration, and CLI selectors use three
nextest processes in that same lane. Combining them into one process was tested
and rejected: Cargo's global `--lib` selector expanded across the package union,
running 1,089 tests instead of the intended bounded selections. Three schedulers
retain compile reuse while preventing that selector cross-product.

The shared-surface compile is now restricted to contract/core/SDK/search-plane/
IPC instead of the workspace-wide all-target graph. Its selected integration
tests use the bounded slice, so ordinary shared-surface validation no
longer pulls text-authority persistence or the semantic/Lance/DataFusion graph.

## Runtime binary consolidation

`quanta-index-searchd-runtime` previously exposed 49 top-level integration-test
files as 49 Cargo test binaries. Each binary linked nearly the complete daemon
graph and measured about 330 MiB in the current lane. The sources now compile as
modules of three explicit targets:

- `runtime_fast_suite`: 9 source files, 67 runnable tests plus one ignored test;
- `runtime_risk_suite`: 21 source files, 168 tests;
- `runtime_extended_suite`: 19 source files, 48 tests.

`package.autotests = false` prevents Cargo from also discovering the source
files as standalone binaries. Source-level rows remain in
`tools/ci/test-authority.toml`, with `target` pointing to the owning suite. The
authority checker requires exactly one launcher, an explicit manifest `[[test]]`
entry, `autotests = false`, and a matching `#[path = ...]` module declaration
for every mapped source. This prevents consolidation from silently dropping
coverage.

The three suite binaries are 337 MiB, 340 MiB, and 336 MiB in the observed
lane. The old fast prototype had three surviving standalone binaries totaling
995 MiB; one 337 MiB suite replaced those three and also absorbed six additional
source targets. For the full runtime surface, the structural change removes 46
duplicate links. Existing old artifacts remain in the dirty external cache, so
the current 13 GiB cache directory is not a clean post-change footprint.

Two runtime tests repeated subsets of the harness cold-matrix using the same
fixture, query, and golden-truth oracle. Those duplicate subset loops were
removed; the authoritative harness still executes every `SCENARIOS` row. The
cold matrix was split into four deterministic modulo shards so the declared
four-thread nextest cap can schedule it. `test-daemon-fast` now excludes this
expensive benchmark-truth target; it remains in `test-daemon`,
`test-daemon-all`, `rust-bench-dsl-truth`, and the hellgate rail.

Nextest output now emits failure and final summaries instead of one PASS line
per test. This reduces log rendering and makes slow/failing cases visible
without changing selection.

## Runtime fixture and shutdown optimization

A per-test timing trace exposed a fixed shutdown cost in direct-runtime tests.
`MaintenanceTimer` slept for the full maintenance cadence and `Drop` joined the
sleeping worker. The production default cadence is five seconds, so a short
test could spend most of its lifetime waiting for an idle maintenance thread.
The timer now waits on an interruptible channel timeout: a timeout performs the
maintenance tick, while a stop signal or sender disconnect exits immediately.
A 30-second-cadence regression test requires drop to finish within one second.

Three fast-suite source files also rebuilt an identical indexed fixture for
each read-only scenario. Their 15 test entry points are now three test entry
points with named scenario helpers:

- text-route authorities: five fixture boots reduced to one;
- hybrid filter authorities: five fixture boots reduced to one;
- explain score traces: five fixture boots reduced to one.

All query cases and assertions remain. Helper failures are wrapped with the
scenario name, so consolidation does not erase the failing semantic oracle.
The suite's process-visible test count falls from 79 to 67 because 12 redundant
fixture/daemon lifecycles were removed, not because scenarios were deleted.

Local `sccache` is enabled only when installed and uses a repository-derived
server port. It caches non-incremental compilations after a lane is cleaned or
recreated while preserving Cargo incremental compilation for workspace crates.
It is not counted as cross-lane reuse: an explicit two-lane probe produced zero
hits because lane-specific dependency paths remain part of the compiler input.
An identical-lane rebuild after `clean` produced 27 hits. The feature is
observable with `just rust-sccache-stats` and can be disabled with
`QUANTA_INDEX_SCCACHE=0`.

## Timing integrity

`tools/ci/timing/check_host_contention.py` now guards:

- all Cargo timing capture/check/baseline-update recipes;
- scale, tail, ANN, and concurrency quality rails.

It reports the foreign process IDs and fails before `clean` or build mutation.
`QUANTA_INDEX_ALLOW_CONTENDED_TIMINGS=1` is an explicit diagnostic override;
an overridden run is not clean performance evidence.

## Verification status

- test-authority catalog and local scopes: PASS
- new runner/preflight/checker tests: PASS
- all `tools/ci/tests`: 257 PASS
- prompt-manager sync/lint/tests: PASS
- Just recipe parse and dry-run expansion: PASS
- new CLI scope: 25 PASS across two binaries; warm nextest execution 1.101s,
  total command 5.309s while still waiting on a contended package-cache lock
- pre-split non-semantic integration scope: 204 PASS across 16 binaries; 25.42s
  compile plus 49.824s nextest execution under foreign Rust load
- final bounded integration scope: 198 PASS across 15 binaries; 0.31s
  no-op compile plus 10.898s nextest execution under the same non-authoritative
  host conditions
- rejected combined shared-surface probe: 1,089 PASS across 28 binaries in
  100.556s, including an unintended 76.216s lexical library test
- bounded shared-library scope: 662 PASS across five binaries in 4.131s;
  excludes lexical, repomap, catalog, and CLI library tests
- selected shared-surface test-profile compile: 8.53s Cargo-reported time;
  the immediately following 662-test library selection required 0.26s compile
  and 3.136s nextest execution
- all three runtime suites: compile PASS; 298 discovered tests across three
  binaries before removal of the two duplicate golden-subset tests
- DSL truth after four-way cold sharding: 5/5 PASS in 58.421s nextest time
  (93.58s command time including a contended compile/cache-lock interval)
- daemon-fast after consolidation and DSL-truth separation: 79/79 PASS, one
  ignored, one binary; 1.24s cached compile and 131.949s nextest execution
  under heavy foreign Rust load
- per-test baseline before shutdown/fixture optimization: 79/79 PASS, one
  ignored; 132.545s wall time and 497.798s cumulative test execution while 23
  foreign Rust processes were active
- direct-runtime shutdown probe: `publish_dispatch_query_lexical_roundtrip`
  fell from 5.336s to 2.142s after the interruptible maintenance timer change
- consolidated fixture probes: 3/3 PASS in 6.676s; the same three source groups
  previously performed 15 indexed-fixture/daemon lifecycles
- daemon-fast after both optimizations: 67/67 PASS, one ignored, in 50.509s;
  a repeated run under worsening host load completed in 74.413s. Both are
  diagnostic only, because the host still had 18 foreign Rust processes and a
  load average near 80 on 16 logical CPUs
- daemon-fast before DSL-truth separation: 84/84 PASS, one ignored, two
  binaries; 193.157s nextest execution under the same non-authoritative class
  of host contention
- test-profile `debug=0` probe rejected: the semantic test binary shrank only
  from 297,912,096 to 297,259,056 bytes (about 0.2%), insufficient to justify
  changing diagnostic quality
- sccache same-lane clean-rebuild probe: 27 hits; cross-lane probe: 0 hits
- Python lint: PASS
- repository-wide Python format check: PRE-EXISTING RED on seven unrelated
  files; all files changed by this optimization pass their scoped format check
- clean wall-clock comparison: BLOCKED by 23 foreign Rust processes
- new integration execution: INTERRUPTED after 11 minutes while the foreign
  load held the `lance` compile near 12% CPU; no correctness claim is made from
  that run, and it is excluded from performance evidence

## Remaining measurement

On a quiet host, run:

```text
just rust-profile test-integration
just rust-profile test-integration
just rust-profile test-daemon
```

The first invocation records the invalidation/cold-ish cost; the second records
the warm local loop. Compare those profile records with the historical
multi-process run only when source, toolchain, features, and host contention
state match.
