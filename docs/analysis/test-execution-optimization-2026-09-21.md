# Test execution optimization — 2026-09-21

## Source snapshot

- Audit-start HEAD: `3ad279a08879de35fa96a5495a3382af28f095d0`
- Initial topology-verification HEAD: `a11085b7af1152df40983ac2c69c53befc8266f6`.
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

- `runtime_fast_suite`: 9 source files, 45 runnable tests plus one ignored test;
- `runtime_risk_suite`: 21 source files, 168 tests;
- `runtime_extended_suite`: 19 source files, 48 tests.

`package.autotests = false` prevents Cargo from also discovering the source
files as standalone binaries. Source-level rows remain in
`tools/ci/test-authority.toml`, with `target` pointing to the owning suite. The
authority checker requires exactly one launcher, an explicit manifest `[[test]]`
entry, `autotests = false`, and an unconditional `#[path = ...]` / `mod` pair
for every mapped source. It rejects unsupported launcher syntax rather than
counting commented-out or conditional declarations as coverage. This prevents
consolidation from silently dropping source modules.

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

A second consolidation pass removed ten more identical read-only lifecycles:

- four structural-ready scenarios now share one indexed parse-tree fixture;
- three structural request-refusal scenarios now share one empty runtime;
- text and hybrid repo-metadata filters share one indexed fixture;
- lexical and Sourcegraph publish/dispatch checks share one indexed fixture;
- the two harness smoke checks share one reopened fixture;
- the two sealed-track read-view checks share one fixture;
- two structural file-query checks share one indexed fixture.

The second pass exposed 57 runnable test entry points plus one ignored network
test while retaining the original query cases and assertions.

A third pass folded five more read-only validation scenarios into the existing
empty-runtime refusal group: hybrid and semantic generation-pin mismatch,
hybrid zero `top_k`, structural generation-not-ready, and structural composition
wiring. This exposed 52 runnable entry points plus one ignored network test.

A fourth pass folded three more read-only empty-runtime gates into that group:
history generation-not-ready, hybrid joint-materialization readiness, and
semantic materialization readiness. This exposed 49 runnable entry points plus
one ignored network test.

A fifth pass consolidated five read-only queries over one sealed default
runtime and a non-overlapping corpus: global semantic nearest-hit, unindexed
lexical scope, empty semantic query refusal, search-owned text derivation, and
history producer-unavailable without lexical fallback. The current suite
exposes 45 runnable entry points plus one ignored network test. In total, 34
redundant fixture or daemon lifecycles were removed from the original 79
runnable entry points.

The local scope runner now accepts `QUANTA_INDEX_TEST_THREADS` as an explicit
lower cap, never above the catalog's declared cap. The normal daemon-fast rail
still uses four threads. On a contended development host,
`QUANTA_INDEX_TEST_THREADS=1 just rust-profile test-daemon-fast` executes the
same catalog-selected suite with one concurrent test. This avoids retrying the
known four-way IPC timeout pattern; it does not turn a contended run into clean
performance evidence. Invalid or cap-exceeding overrides fail before Cargo.
The runner flushes the selected scope, binary count, and effective thread cap
before replacing itself with Cargo, so execution logs show the actual mode.

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
- daemon-fast after the second fixture consolidation: 57/57 PASS, one ignored,
  in 32.367s with a 0.94s cached compile. The host was still shared, so this is
  diagnostic rather than a clean performance receipt
- third-pass shared refusal group: 1/1 PASS in 0.843s, covering all eight
  validation/refusal scenarios on one empty runtime
- third-pass full daemon-fast attempt: INFRA RED, not a semantic failure. With
  32 foreign Rust processes and load averages above 400, four unrelated tests
  hit the 30-second IPC read timeout; fail-fast stopped the run after 17/52
  tests. A quiet-host 52/52 rerun remains required
- fourth-pass shared refusal group: 1/1 PASS in 0.780s, covering all eleven
  read-only validation/readiness scenarios on one empty runtime; suite compile
  PASS. A quiet-host 49/49 rerun remains required because host load was still
  above 300 during this pass
- fifth-pass default indexed-query group: clean detached-main compile PASS and
  1/1 PASS in 2.009s, covering all five named query scenarios on one runtime.
  The host still had foreign Rust builds, so this proves correctness of the
  consolidated group but is not clean performance evidence
- fifth-pass full daemon-fast correctness run: clean detached-main 45/45 PASS,
  one ignored, in 102.652s with `--test-threads 1`. This closes the optimized
  selection under contention without the four IPC timeouts seen at concurrency
  four. It is correctness evidence only; a quiet-host four-thread rerun remains
  required for a comparable performance baseline
- local thread-cap override: runner unit tests 10/10 PASS; default daemon-fast
  dry-run retains four threads, explicit `QUANTA_INDEX_TEST_THREADS=1` dry-run
  selects one thread, invalid/above-cap inputs fail before execution, and
  selection output is flushed before Cargo starts; all `tools/ci/tests` 304 PASS
- `QUANTA_INDEX_TEST_THREADS=1 just rust-profile test-daemon-fast`: 45/45 PASS,
  one ignored, in 70.479s nextest time with a 0.77s cached compile. The host
  still had foreign Rust builds, so this is a functional front-door proof and
  not a clean performance measurement
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

## Follow-up verification-topology audit

The next audit started from clean `main` at
`3e5fd07ade8a50a4807b1aaaf44b36f51ae3da4e`. It found three more
unnecessary compile paths and one selector-safety defect:

| Path | Before | After | Retained authority |
|---|---|---|---|
| CI pre-commit | Installs Rust and runs full-workspace Cargo fmt/check, despite dedicated jobs | Skips those two hooks in this job and omits the Rust installation | `rust-fmt` and `rust-clippy` jobs |
| CI clippy | Full all-target/all-feature `check`, then exact-surface clippy in separate cache lanes | Clippy only | Exact-surface clippy; stand-alone check remains in `rust-msrv` |
| Local `verify-rust` | Full check, then exact-surface clippy in separate lanes | Clippy only | `just rust-check` remains an explicit compile-only command |
| Local Git hooks | Rust compile, test, doc, dependency, and bench hooks run even for Python/doc-only changes | Heavy hooks run only when their declared Rust/manifest paths changed | CI whole-workspace jobs still run on every PR/push/merge-group |

The removed checks were redundant within these aggregates, not a removal of
stand-alone compile capability. MSRV, test-profile, bench-profile, docs,
nextest, and policy jobs remain distinct. The `pre-push` hook is now a
changed-file fast gate; it is **not** a complete qualification receipt for
unchanged Rust files. A full verification claim still requires the appropriate
CI/`just` rails.

Two further CI setup costs were identified without changing the validation
commands: `prompt-manager` now installs only the project runtime dependencies,
pytest, and Ruff; its prior `.[dev]` install also pulled Semgrep and pre-commit,
which have separate jobs. `agent-output` now identifies changed JSON files
before setting up Python or installing project dependencies, and skips both
when its validation step is also skipped. Its push comparison now uses the
event's `before` SHA rather than `HEAD~1` (which missed earlier commits in a
multi-commit push); PR and merge-group comparisons use their event base SHA,
and unavailable bases fail closed. Changed paths are passed as NUL-delimited
files rather than interpolated into a shell command through a step output,
preserving unusual filenames. These are dependency/idle-job reductions and a
selection-correctness fix; no end-to-end CI timing delta is claimed while
hosted jobs cannot start.

The local scope runner's cross-product guard previously compared the source
file stem instead of the declared Cargo `target`. A renamed/grouped test target
from another selected package could be executed unintentionally. The guard now
checks the same target name used to construct the Cargo command, with a
regression test for the alias case.

The baseline workflow also had a ShellCheck SC2209 warning in verification
receipt variable assignments, independently reproduced against the prior HEAD;
quoting those literal assignments makes the changed workflow actionlint-clean.

Verification for this follow-up: runner unit tests 11/11 PASS; test-authority
lint PASS; pre-commit config valid; non-Rust file probes skip the Rust and bench
hooks; explicit `SKIP` probe skips CI's two duplicate hooks; `just --dry-run
verify-rust` contains no full check; actionlint, scoped Ruff checks, and all
three sampled local-scope dry runs PASS. The non-DSL CI-tool tests passed
276/276; all seven prompt-manager tests passed. The unfiltered CI-tool run
was interrupted after 85 passes while a DSL test's Cargo subprocess waited
under foreign Rust load. Doc-path lint remains RED on 35 broken paths in
unchanged `docs/bugbash/sep-16` files. Whole-workspace Rust tests and clean
wall-clock savings are not claimed. The timing preflight still found 11
unrelated Cargo/rustc processes on the host.

## Follow-up DSL tooling test separation

The general `pytest tools` rail previously called `main()` in three DSL
checker unit tests. Each call executed two owner dump binaries through Cargo,
so a Python-tooling test run could launch six Rust builds and stall on a shared
Cargo target lock. The unit tests now inject deterministic, schema-shaped owner
dump payloads while still reading the real lowering source and checking the
guarded-widening and verdict-flip rejection paths. A new failure case verifies
that an unavailable owner dump fails closed.

The executable truth check was not removed: the `rust-policy` CI job now
installs the pinned Rust toolchain and executes
`tools/ci/lint/check-dsl-capability-truth.py` once against the real owner
binaries. The local pre-push hook executes that same gate only when its core
predicate, lexical registry/dump, structural lowering/dump, checker, or
capability-document inputs change. Local hooks are a changed-file gate; CI
remains the whole-revision authority.

The focused DSL unit module passed 30/30 in 0.43s without a Cargo invocation.
The full `python3 -m pytest tools -q` suite passed 313/313 in 100.43s while
other Rust builds saturated the host. The real
`python3 tools/ci/lint/check-dsl-capability-truth.py` gate also passed locally
against both executable owner dumps. These are correctness results, not a clean
end-to-end timing comparison. The then-current GitHub Actions account
billing/spending-limit failure prevented CI jobs from starting, so the new
CI step itself remains unqualified.

## Follow-up verification receipt hardening

`tools/ci/write-verification-receipt.py` previously read the same nextest
JSONL twice (once fully into memory for SHA-256, then again as text for event
counting) and accepted a receipt whenever it found any terminal test event.
That admitted ignored-only evidence, failed or timed-out tests, and truncated
evidence with a passing test but no finished suite. A receipt consumer could
misread such an artifact as a completed passing run even though the normal CI
producer also checks the process exit status.

The writer now hashes and validates the bytes in one streaming pass. It
requires at least one passing test, no failed/timed-out test or failed suite,
complete started/finished suite events, and matching suite/test pass counts.
Unknown event types and test outcomes fail closed. Both receipt-producing
workflows pin nextest's `libtest-json-plus` format version `0.1` and record the
full executed command in the receipt instead of omitting its format flags.
The test-authority rail declarations now include those flags. Their binding
check matches a logical shell execution line across continuations, not a
comment or the receipt writer's `--command` metadata argument. This is a
static binding check, not a substitute for executing the workflow.
This follows nextest's documented
`libtest-json-plus` suite and test event shape
(<https://nexte.st/docs/machine-readable/libtest-json/>); that format remains
experimental, so format drift must fail visibly rather than silently produce
a receipt. The receipt schema and rail identity are unchanged.

Focused receipt and test-authority tests passed 21/21. Scoped Ruff, test-authority
lint, wire-inventory lint, and diff checks passed. A new live nextest JSONL
sample was not produced under the current foreign Rust build contention;
GitHub CI still cannot run while the account billing/spending-limit issue
persists. Thus the stronger writer is locally contract-tested but not yet
CI-qualified at this HEAD.

## Grouped-suite coverage guard hardening

The grouped-suite authority check previously searched launcher text for any
`#[path = ...]` substring. A cataloged module could be commented out, put in a
raw string, or left without its `mod` declaration while the guard still found
the path. Because `autotests = false`, Cargo would not discover that source as
a separate test target. This was a static P1 coverage hole, not evidence that
the current launchers had dropped a module.

The guard now accepts only the launchers' deliberately narrow form: a path
attribute immediately followed by an unconditional module declaration. Blank
lines, line comments, the unsafe-code crate attribute, and the harness alias
are allowed; all other syntax fails closed. Contract tests cover commented,
raw-string, detached, and conditional declarations. Existing launchers pass
the real catalog check. The `tools/ci/tests` suite passed 315/315, alongside
scoped Ruff and the real authority lint. This is local static coverage proof,
not a full Rust suite run or CI qualification; GitHub CI is still blocked by
account billing.

## Full-corpus correctness rail repair and PR deduplication

The correctness workflow still invoked `cargo test --test e2e_full_corpus`
after that source became a module of `runtime_risk_suite` with
`autotests = false`. A local `--no-run` probe confirmed Cargo exits 101 with
"no test target named `e2e_full_corpus`" before compiling. The dedicated
workflow job now selects `runtime_risk_suite` with a module filter, the same
test surface as the local `just rust-test-full-corpus` rail. It runs on schedule or explicit
dispatch, not on every PR: PR workspace nextest already executes the same
module, so a second cold runtime build added no PR coverage. The catalog lint
now validates every explicit workflow `--test` selector against cataloged
Cargo target names; regression tests cover both split and equals syntax.
The focused checker tests (13/13), real catalog lint, Ruff, actionlint, and
diff check passed. The corrected CI command has not run at this HEAD: the
host had 70 Cargo/rustc processes during the audit, and jobs on the previous
published HEAD failed before startup on account billing. This is a structural PR job
reduction, not a measured wall-clock speedup.

## CI Python setup and static-gate baseline

Ten CI/correctness jobs previously installed the complete `.[dev]` dependency
set even when they needed only the project runtime dependencies, pytest,
pre-commit, Semgrep, or the Python standard library. Their installs now match
their actual command imports; the prompt-manager job installs runtime deps,
pytest, and Ruff without Semgrep or pre-commit. Two stdlib-only correctness
jobs no longer invoke pip at all. This removes redundant package resolution
and installation without dropping a gate or changing its selected tests.
No hosted-runner time reduction can be measured while GitHub jobs fail before startup.

A clean-environment rehearsal exposed an existing CI pre-commit failure:
generated prompt files and the parity report had extra final blank lines, two
historical docs had trailing spaces, and six Python files did not match the
then-latest Ruff formatter. The prompt-manager and parity generators now emit
one final newline, Ruff is pinned to `0.16.8`, and those files are normalized.
Historical bugbash links to deleted pre-split files now point to verified
commits where the files existed. The same `pre-commit --all-files` command as
CI passed locally with only the two independently owned Cargo hooks skipped;
that is static/local proof, not a hosted CI pass.

Regenerating the parity report also exposed a separate P1 false-negative:
its old string-literal regex read zero canonical predicates from the typed
registry. The generator now binds the lexical registry rows to core enum
names, checks complete/unique typed tables, and fails closed on drift. The
report again lists 10 canonical predicates, six aliases, and the symbol route.
This inventory is static source proof; it does not replace executable query
results or the DSL capability owner's runnable dump.

## Error-authority inventory scan cost

The P00 inventory producer scanned 492 Rust source files (about 8 MB) with
eight regular expressions, including files lacking a literal required by each
pattern. It also reread every file for the source digest and located every
match's line with a linear search. The producer now reads raw bytes once,
preserves universal-newline text semantics, skips regexes when their required
literal is absent, and uses binary search for line lookup. At frozen HEAD
`b78ccd11289f274aa5568881640eb6547008841d` with unchanged Rust source,
the complete serialized inventory SHA-256 remained
`0e80ebd47bc6efac315848ce7d4d0972fc26e066ad865b4bc33c93257fd2ba25`,
with all eight category counts unchanged. Contended local cProfile probes
observed 16.868s before and 3.310s after; these are diagnostic observations,
not controlled latency or hosted-CI speedup claims.

The pre-commit digest-fallibility lint also performed character-by-character
comment stripping and brace-depth analysis on every Rust source file, although
it can report a site only when the file contains both a public declaration and
a literal `[u8; N]` array. A cheap exact prefilter now excludes files where a
site is impossible; the parser and its policy are unchanged for candidate
files. The current 448-file scan still reports six sites and zero violations,
with the complete site/finding-list SHA-256 unchanged at
`646e0e09a6a0df0a34556a8785cb0729e0073bc969d3d80ec7c6e7c6f07346a6`.
Contended local probes observed 35.7s before and 6.815s after; again this is
diagnostic, not a controlled CI timing claim.

## Aggregate receipt validation passes

The P12 aggregate writer performed the full source-bound semantic validation
twice before atomic replacement, then again after publication. It now keeps
one pre-publication validation and the independent post-publication rebind.
The second pre-publication pass was redundant because no authority state is
mutated between those checks. The existing release-ready, diagnostic, and
host-drift aggregate tests passed with the two-pass path. A new source-drift
test proves that the post-publication pass still detects a changed source.
That test also reproduced a pre-existing failure mode: post-publication
rejection deleted a previous aggregate after overwriting it. The writer now
restores the prior bytes atomically on post-publication failure (or removes the
new file when no prior receipt existed). This is a correctness repair as well
as a repeated-validation reduction. Heavy fixture timings remain contended,
so no numeric P12 speedup is claimed.

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
