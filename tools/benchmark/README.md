# Benchmark tooling

For code-search execution, start with the
[operator runbook](CODE_SEARCH_RUNBOOK.md). It separates live Quanta--Semble
capture, recorded five-product lexical scoring, and correctness-only proof.

The registered evidence CLI is `python3 tools/benchmark/benchctl.py`. Its sole
registration/producer/validator/scorer/baseline control plane is
`tools/benchmark/registry.toml` (validated by `tools/benchmark/registry.py`);
producers remain the referenced Just recipes, cargo bench targets and
allowlisted Python modules. `tools/benchmark/manifest.json` was removed — the
artifact checker and quality summary read the registry through the read-only
`manifest.py` projection, so there is one data authority.

Python `benchctl` is the **single current benchmark orchestrator**. The typed
common evidence contract (`BenchmarkEvidenceV1`) is defined by the Rust crate
`benchmarks/bench-protocol` and written with identical canonical bytes by
`tools/benchmark/evidence.py`. The Rust/Python go/no-go decision is recorded in
[`SEP-27-002`](../../docs/adr/SEP-27-002-single-benchmark-orchestrator-and-typed-evidence.md).

```sh
python3 tools/benchmark/benchctl.py list                    # profiles + registry digest
python3 tools/benchmark/benchctl.py plan dsl-authority      # digest-bound, no mutation
python3 tools/benchmark/benchctl.py run systems --evidence-root /external/bench
python3 tools/benchmark/benchctl.py validate systems --evidence-root /external/bench
python3 tools/benchmark/benchctl.py replay <run-id> --evidence-root /external/bench
python3 tools/benchmark/benchctl.py summarize systems        # observed artifacts only
python3 tools/ci/lint/check-benchmark-policy.py              # registry/dependency/CI policy
```

`run` executes a profile's producers serially and then requires their
current-HEAD artifacts. `validate` checks existing artifacts without rerunning
producers. `plan` is deterministic on the same frozen inputs and never mutates.
`replay` re-validates one immutable run from its captured raw bytes in a fresh
process and re-derives the verdict through the independent artifact checker; a
changed input is refused. `summarize` is read-only and labels observed files
`present_unvalidated`; it is deliberately not a qualification command.
`benchctl run` and `benchctl validate` require the target checkout to be clean;
a HEAD-matching artifact captured before local source edits cannot be
requalified as evidence for the dirty tree.

## Capture capabilities and qualification boundary

Registration is not execution support. `list`/`plan` expose all registered
families; the current end-to-end native capture path is narrower:

| Profiles | Current execution/evidence path |
| --- | --- |
| `dsl-authority`, `quality-core`, `quality-full`, `systems`, `semantic-ab` | Native Just producers, artifact checks, baseline comparison where declared, immutable promotion and raw-derived replay. Requires clean source and the declared host/inputs. |
| `micro`, `dsl-diagnostic` | Registered Criterion owners execute with an explicit external evidence root. Fresh output directories, binary listing + correctness smoke, exact sample/estimate retention, per-case immutable runs and complete-profile publication. Diagnostic wall time only; no performance qualification. |
| `retrieval-contract` | Executes both existing SDK/contract proof owners, retains raw terminal inventories and immutable binary copies, publishes a complete profile and replays with the existing owner validator. Typed test proof only, not relevance or speed. |
| `retrieval-diagnostic` | Executes the existing paired Quanta/Semble owner from an external `--pair-spec`; retains frozen inputs, executable identities, native tree and a self-contained Git corpus bundle. Publishes separate file/context/span cases only after native verdict re-computation and complete inventory validation. Diagnostic only; no quality/performance admission. |
| `lexical-diagnostic` | Executes the existing scorer over nine frozen external observation inputs; publishes five product-specific file-recall runs as one complete capture, with raw-derived replay. Recorded diagnostic only, not live product search or qualified speed. |
| `recorded` | Explicit external A/B/C JSONL and scan-native JSON imports; existing agent evaluator, per-family immutable runs, complete profile publication and raw-derived replay. Submitted recordings remain unauthenticated diagnostics; authenticated claims are refused. |

Unsupported profiles do not execute a supported subset and then claim a full
capture. `summarize` reports `registered_not_captured` with an unknown (null)
measurement count; `preflight` explicitly refuses rather than raising a lookup
exception. These are implementation gaps, not missing license/gold/quiet-host
inputs. See the [execution SSOT](../../docs/plans/sep-27-misc/tickets/INDEX.md).

## Immutable runs and typed evidence

### Native executable custody

Retrieval proof execution contexts are **v2**. Both SDK and contract rails
derive a mandatory compiled-test executable map from retained raw nextest
collection (`binary-id`, canonical absolute `binary-path`, selected testcases).
Roles are `nextest-<sha256(full binary-id)>`; SDK also binds `runner` and
`searchd`. Missing roles, duplicate paths, collection/path contradictions and
changed executable bytes/epochs refuse. The producer pins these files before
nextest execution; common capture retains all executable bytes and replay
validates them without the original target cache. Common build flags retain
the test selector's `--all-features` and `--locked`; individual raw commands
remain the authority for each executable's actual feature/build arguments.

Existing v1 contexts are not silently requalified or upcast. Runner record v5,
diagnostic v6, run manifest v2 and required Rust test identities are unchanged.
This is local file custody, not compiler-dependency, compromised-UID or remote
producer attestation. SDK/contract success still requires terminal actual tests
and fresh raw replay at the same frozen source.

### Shared external corpus releases

```sh
uv run --frozen --extra dev python tools/benchmark/benchctl.py corpus create \
  --spec /external/corpus-recipe.json --checkouts /external/checkouts \
  --release /external/corpus-releases/release-name
uv run --frozen --extra dev python tools/benchmark/benchctl.py corpus validate \
  --release /external/corpus-releases/release-name
```

`corpus_release.py` is the common release/view authority; `corpus_set.py` exports
legacy **candidate** code manifests, not admitted releases. Existing `frozen-v4`
is not overwritten or relabelled. Creation requires clean source and clean,
exact-commit source checkouts. Existing release destinations are never replaced.

```
<external-release>/
  release.json                         # complete tracked inventory, policy, identities
  recipe.json                          # canonical declared source recipe
  bundles/<repo>.bundle                # self-contained original Git commit/history
  blobs/<sha256>                       # deduplicated admitted canonical Git blob bytes
  manifests/<repo>/<view>.json          # native RB-00 corpus manifest
  views/<repo>/<view>/<tracked-path>    # independently materialized read-only files
```

Both `code_only` and `developer_search` views span the complete repository.
The old recipe's `benchmark_root` is retained as candidate provenance, **not**
silently reused as the new view filter. Code view extensions and generated/vendor
directory components are explicit in the release policy; developer search also
includes admitted documentation/configuration text. Symlink, submodule, LFS
pointer, empty/oversize, binary/encoding and exotic-line-break exclusions are
recorded per tracked path. NFC/casefold aliases, including parent directories,
refuse rather than depending on host filesystem casing.

All Git blobs are streamed and hash-checked against their actual Git object IDs;
only one bounded view-size file is retained in memory. Git network protocols are
limited to local files and unrelated service credentials are not forwarded.
Batch readers and subprocess groups have bounded cleanup; this is local custody,
not an OS sandbox or same-UID/remote producer attestation. Validation restores
the retained Git bundles and re-derives inventory, manifests, exclusions, view
bytes and executable/read-only modes without the original mutable checkouts.

Use a chosen view manifest and **re-freeze its corresponding query pack/suite**
for Quanta/Semble; external lexical products index that view directory and keep
the original tracked-path namespace. View directories are not synthetic Git
checkouts and must not claim the original commit as their own invented HEAD.
Every comparator must bind the same manifest/file universe; a valid release
does not itself prove its live searchable index universe. Release file count is
not independent qrels, license approval, query performance or product quality.
The release status remains `frozen_not_admitted`.

Passing `--evidence-root <external-root>` (or setting
`QUANTA_BENCH_EVIDENCE_ROOT`) makes `run` capture each family's **native**
artifact verbatim into an immutable run:

```
<external-root>/runs/<run-id>/{evidence.json,raw/<native artifact>}
<external-root>/latest                 # advisory pointer, never a baseline
<external-root>/baselines/<family>.json
<external-root>/captures/<capture-id>.json  # immutable complete case inventory
<external-root>/profiles/<profile>.json    # complete-capture commit pointer
```

Criterion example (defaults: 100 samples, 3 s warmup, 5 s measurement per case):

```sh
python3 tools/benchmark/benchctl.py run micro --evidence-root /external/bench
python3 tools/benchmark/benchctl.py validate micro --evidence-root /external/bench
python3 tools/benchmark/benchctl.py summarize micro --evidence-root /external/bench
```

`--criterion-samples`, `--criterion-warmup`, `--criterion-measurement`,
`--criterion-resamples` and `--producer-timeout` configure explicit diagnostic
captures. Fewer than ten samples, non-finite durations, missing/duplicate cases,
interrupted producers, estimates disagreeing with raw samples and changed
source/binaries are refused. LQ requires all twelve stage/size cases. Runtime
DSL uses the executable's complete `--list` inventory. Each run retains native
metadata, samples, estimates, listing, Cargo messages, rustc identity and actual
execution arguments. Replay recomputes the native mean; validation also requires
the complete profile and current source/registry/lockfile identities.

The common POSIX producer executor bounds failure cleanup to ten seconds.
Nested owned sessions use a parent-liveness descriptor so controller death
terminates their groups. A direct child's exit does not release group custody:
the controller drains output while awaiting a private terminal record, kills
the group before reaping its leader, and only then interprets the actual child
exit status. Same-group background descendants with closed output pipes are
also terminated on normal, nonzero and signalled child exit. Missing/malformed
records or an unproved guard termination cannot establish completion.
External/ignored SIGCHLD handlers are refused before spawning: the executor
must own direct-child reaping and keep the group leader's PID pinned.
Both pipe watchers use the OS default selector rather than a fixed-fd-range
`select()` implementation, including high-numbered descriptors.
Pipe-drain/reap failure remains an explicit
`incomplete cleanup`, never completed evidence. This is not sandbox containment
of an arbitrary subprocess that deliberately escapes its owned group.
Portable retrieval proof commands use this same owner: thirty seconds for Git
and tool identity, 7200 seconds per proof/build command, plus bounded cleanup.

Only the complete capture pointer publishes a profile. Individually promoted
runs left by an I/O failure are not a complete profile. Python store GC pins
all immutable captures (including history); malformed custody refuses deletion.
Publication and Python GC share a POSIX process/thread custody lock; the current
capture rail supports Linux/macOS. Unsupported hosts refuse before production.
Rust store GC refuses orchestration-owned capture roots instead of deleting
their referenced runs. These diagnostic captures do not prove quiet-host
latency, peak RSS, capacity, instruction cost or repository-wide qualification.

- The run id is immutable; promoting it twice is refused. A crash before
  promotion leaves only a `.staging/` directory, which is never admissible.
- `evidence.json` is a sealed `BenchmarkEvidenceV1`: protocol version, family/
  profile/case ids, source closure, build/toolchain/lockfile/binary digests,
  input and output digests, host policy with an explicit lease observation, the
  exact command with exit/timeout/interruption, the measurement boundary, typed
  payload, referenced raw files with SHA-256 and length, and the verdict scope.
- Every referenced raw file is re-hashed at load and at replay. Missing, extra,
  reordered, truncated or tampered bytes are refused, as is a symlinked or
  path-escaping reference.
- `latest` is a pointer, not a baseline. A baseline names an immutable run id
  and digest, and a run that backs an admitted baseline (with its raw inputs)
  can never be garbage-collected.
- The evidence root must stay outside the checkout; artifact data is external.

Native latency, load and freshness payloads are re-derived from every raw
artifact during replay. Concurrency requires all 1/8/32 **fast-client** cases;
the native envelope includes the additional slow client (1/9/33 total clients).
Each case retains its own config digest. Fast/slow aggregates are projected
once, not re-summed from their route subsets or from all copies of `detail`.
Open-loop counts reconcile every offered request with served/errors/timeouts/
drops; scheduler-late drops, not SUT queue backpressure, flag generator
saturation. Freshness retains every sample's transition phases and an explicit
producer-observed stale-hit count; older captures without that count are not
silently upgraded.

Promoted native runs currently have diagnostic scope, shared-host lease
observations and no measured binary digest inventory. Their profile elapsed
time includes build/recipe work and is **not query latency**. The command clock
is measured; a hard timeout is applied to each producer recipe. Baseline
admission and `--evidence-root` capture are separate actions. These runs cannot
be promoted into a performance qualification claim merely because replay passes.

Typed payloads keep measurement kinds apart: a `micro` payload whose
instrumentation is `instructions` cannot carry `ms`; a retrieval `span` metric
space cannot be built from mechanically labelled file data; an `unjudged` or
`timeout` retrieval row cannot carry a score; closed-loop throughput cannot be
reported as an offered rate; a recorded experiment cannot claim qualification.
See `benchmarks/bench-protocol/fixtures/` for the cross-language canonical
vectors both implementations must reproduce byte-for-byte.

Retrieval test proofs use a separate `proof` payload: terminal selected,
executed, passed and failed counts plus source/context digests. Incomplete
execution, failed tests presented as passing, wrong source and relevance or
performance verdict scopes are refused. Proof counts are never retrieval scores.

```sh
uv run --frozen --extra dev python tools/benchmark/benchctl.py run retrieval-contract \
  --evidence-root /external/bench --producer-timeout 7200
uv run --frozen --extra dev python tools/benchmark/benchctl.py validate retrieval-contract \
  --evidence-root /external/bench
uv run --frozen --extra dev python tools/benchmark/benchctl.py replay \
  --family retrieval-sdk --evidence-root /external/bench
```

The capture retains original absolute execution paths without rewriting native
receipts. SDK runner/searchd bytes are frozen for replay independently of the
mutable build cache. Outer recipe wall time is not search latency. `compare`
refuses test proofs because they have no relevance/performance baseline.

### Paired diagnostic execution

```sh
uv run --frozen --extra dev python tools/benchmark/benchctl.py run retrieval-diagnostic \
  --pair-spec /external/pair-spec.json --evidence-root /external/bench \
  --producer-timeout 7200
uv run --frozen --extra dev python tools/benchmark/benchctl.py validate retrieval-diagnostic \
  --evidence-root /external/bench
uv run --frozen --extra dev python tools/benchmark/benchctl.py replay \
  --family retrieval-pair --evidence-root /external/bench
```

The spec is the existing `tools.benchmark.retrieval.run pair` contract, not a
second query/scorer schema. Declare `manifest`, `suite`, `query_pack`,
`host_profile`, `semble_lockfile` and `semble_python` explicitly. Spec, corpus,
input and native output paths must be external to the source checkout; evidence,
native output and original corpus roots must be mutually disjoint. Only
`scope: exploratory` is admitted by this diagnostic profile. Qualified native
admission remains a separate rail; `compare` refuses a qualified baseline.

`pair_capture.py` re-runs the native `verdict` owner instead of duplicating its
scorer. Original command/path bytes remain unchanged. Raw custody includes the
native output as a sorted regular-file archive, a Git bundle restoring the real
commit/tree/executable modes, all five frozen inputs and all four executed-file
identities (driver Python, runner, searchd, Semble Python). Missing, duplicate,
reordered, unsafe, compressed, encrypted or corrupt archive entries are refused.
Original mutable corpus/output paths are not needed for replay. Current scorer
identity remains required; a historical run is not upgraded after owner edits.

Each strategy/route has independent file, context and indexed-span cases.
Unsupported span metrics carry no score; no-answer, unsupported and timeout are
not converted to zero relevance. Native metrics/timing remain in the raw tree,
and common execution wall time is not search latency. This bridge does not attest
an exhaustive searchable universe, independent gold or a quiet host. Full Git
history and archive bytes are currently retained per case; large-corpus storage
deduplication/streaming is not established by the fixture contract tests.

### Lexical diagnostic scorer

`retrieval-diagnostic` and `lexical-diagnostic` are separate profiles. The former
owns paired execution. The standalone native scorer consumes a nine-role spec
(`schema_version: 1`): `suite`, `query_pack`, `pair_report`, `pair_lock`,
`semble_native`, `pair_verdict`, `sourcegraph_rows`, `opengrok_rows`, `cs_rows`.
Paths must be absolute; omitted, unknown or mixed inputs are refused.
The common `--lexical-spec` is instead a closed schema v2 envelope containing
`corpus` and `inputs`. `inputs` holds those same nine roles; `corpus` requires
absolute `release_path`, `release_digest`, `repository`, and `view`
(`code_only` or `developer_search`). The suite/query pack must exactly bind
the selected Git-derived view's commit, ordered complete file universe and
digest. Capture retains the recipe/release/Git bundles and binding; replay
reconstructs the view without original mutable paths. This is input binding,
not proof of a product's indexed universe. Capsules above 256 MiB refuse.
For fresh external rows, `benchctl code-search external --spec ...`
issues Sourcegraph/OpenGrok HTTP requests and cs processes against the selected
release view. It retains native responses and emits the three row files; it is
an exploratory producer composed by `benchctl code-search run --spec ...` with
the live SDK pair, existing lexical scorer, validation and replay. The
[operator runbook](CODE_SEARCH_RUNBOOK.md#b-score-five-recorded-lexical-products)
gives the spec example and execution sequence.

```sh
uv run --frozen --extra dev python -m tools.benchmark.retrieval.lexical_file_comparison \
  --spec /external/lexical-inputs.json --out /external/fresh-lexical-report.json
uv run --frozen --extra dev python tools/benchmark/benchctl.py run lexical-diagnostic \
  --lexical-spec /external/lexical-corpus-bound-spec-v2.json --evidence-root /external/bench
uv run --frozen --extra dev python tools/benchmark/benchctl.py validate lexical-diagnostic \
  --evidence-root /external/bench
```

`file_hit_rate_at_10` is the fraction of queries with at least one gold file.
`file_recall_at_10` is query-macro coverage of all gold files; these differ when
a query has multiple answers. The paired report must carry per-query
`file_recall_at_10` and hit observations; the scorer cross-checks their aggregate
instead of deriving hit count from recall. Old reports without those facts
must be re-scored from native records, never relabelled or defaulted.
The scorer requires one bare identifier per query and rejects noncanonical
or out-of-view result paths against the frozen suite/pack universe.
The scorer retains per-query observations and derives descriptive latency from
each product's recorded timing layer. Raw rows remain diagnostic; index-universe
attestation, independent judgments, qualified speed and raw HTTP/process
authenticity are not inferred from a summary flag.

The common adapter freezes exact input bytes before running the owner module,
retains the original spec and original execution paths, and independently
re-scores captured raw observations. Products are separate case IDs and every
run retains the full native report (hit rate, latency layers, exclusions and
per-query rows); the typed payload is file recall only. All five product cases
must validate before the complete profile pointer changes. Timeout/unsupported
paired rows carry no typed score; missing/unrepresentable terminal states refuse.
Re-scoring succeeds without the original mutable recording paths, but requires
the original scorer identity. Publication uses the same immutable store and GC
custody as other profiles. No live HTTP request or product binary execution is
claimed by `run lexical-diagnostic`; a fresh search pilot is a separate input
producer requirement, and `compare` refuses a qualified baseline claim.

`benchctl run` requires declared baselines before starting a comparison run and
requires a clean host preflight receipt; contention overrides are diagnostic.
The preflight also refuses missing load data or a one-minute load average at or
above half the logical CPU count. This is a conservative overload guard, not
proof that the host stayed isolated throughout a run. Baseline admission and
profile execution independently recompute the load guard instead of trusting
the receipt's `clean` status string. Old receipts without load evidence cannot
be used for new baseline admission.
`benchctl run` also freezes the initial Git HEAD and rechecks HEAD plus the
clean worktree after preflight, every producer, validation, and comparison.
An in-flight source edit or commit refuses the run before its artifacts can be
admitted under the starting preflight.
The integration summary independently validates its required artifacts before
writing a green aggregate.
`just rust-verify-quality-all` delegates its producer order to `benchctl run
quality-full`, so it has the same clean-worktree admission before a producer
can write timing-bearing evidence.
`just rust-bench-dsl-refresh <samples>` likewise delegates to `benchctl run
dsl-authority`; authority comparison accepts no fewer than 20 cold samples.

`just benchmark-policy-local` runs the machine-checkable policy: registry
validity and reachability, every Cargo bench target registered, no production
`crates/*` normal dependency on a `benchmarks/*` package, and no CI step that
calls a registered producer or comparator directly instead of the CLI.

For local PREP after benchmark-control-plane changes, run
`just benchmark-prep-local`. It validates the registry, the evidence contract
(Rust tests plus Python conformance vectors), the benchmark-control-plane Python
suites, benchmark-harness Rust formatting and library tests, and producer-binary
compilation in the warm `test-daemon-lane`. The Python capture/custody subset has one
shared local/CI entrypoint: `uv run --frozen --extra dev just
benchmark-control-contract-local`. It includes Criterion, system, recorded,
retrieval-proof and lexical adapters and their rejection cases; it does not run
live products or qualify performance. CI runs that exact entrypoint, not a
second independently maintained test-file list. Retrieval evaluator and chunking
contracts are intentionally separate: run `just retrieval-contract-local` for a
dirty-checkout edit loop, or `just retrieval-contract-proof <fresh-output-root>`
once on a clean source to run them and emit source-bound receipts. Running prep
followed by proof does not repeat retrieval contract tests. PREP does not create
a benchmark artifact, invoke a timing preflight, or claim current-source
qualification.
Run a real profile only after the checkout is clean; DSL authority additionally
requires the quiet canonical Linux host.
The `dsl-authority` profile is canonical-Linux-only: it writes an
`unsupported_host` preflight receipt and refuses before producer execution on
any other OS. Local macOS runs remain available only for diagnostic families.
The open-loop qualification default uses seeded-Poisson arrivals; the prior
deterministic periodic schedule remains an explicit diagnostic mode only. The
correctness verdict requires a healthy first offered-load point
and no malformed response or unexpected typed error. Timeout, drop and socket
refusal above saturation are recorded as capacity loss with error-kind counts,
not hidden or interpreted as a passing latency SLO. A capacity threshold needs
reviewed measurements on the pinned Linux host.
The recorded retrieval and agent-outcome evaluators have separate CLIs and
strict input contracts in [retrieval/README.md](retrieval/README.md) and
[agent_outcome/README.md](agent_outcome/README.md); they do not invent runner
results when recordings are absent.

### Recorded import

```sh
uv run --frozen --extra dev python tools/benchmark/benchctl.py run recorded \
  --evidence-root /external/bench \
  --agent-recording /external/recordings/agent.jsonl \
  --scan-recording /external/recordings/scan.json \
  --recorded-authenticity recorded_unauthenticated
uv run --frozen --extra dev python tools/benchmark/benchctl.py validate recorded \
  --evidence-root /external/bench
```

`scan.json` contains exactly `{"artifacts": [<native BenchArtifactV1>, ...]}`.
Each scale must be distinct and measured, with the same native source revision;
every row's p50/p95/p99 and error/timeout counters are retained. The raw native
documents are preserved verbatim. They contain index-query measurements, not
`rg`/`grep` timing samples; no scan timing is reconstructed from Markdown.

The recorded envelope binds the **importer** source, locked Python dependency
identity and importer host. The original producer source/host remain in the raw
input; import does not authenticate them or infer current-product performance.
A/B/C denominators and conditional first-useful-evidence timing are recomputed
by the existing evaluator. Missing useful evidence has coverage zero and no
conditional timing value, not a zero-ms event. No coding agents or arbitrary
shell commands are launched. Missing/partial input cannot publish a complete
profile. `--recorded-authenticity authenticated` is refused until an underlying
receipt authenticator exists; it never silently downgrades the request.

The sections below document the DSL Layer-3 latency gate.

The architecture and claim boundaries are defined in
[`JUN-08-001`](../../docs/adr/JUN-08-001-verification-hellgate-and-benchmark-separation.md).
The current capture, artifact, comparison and baseline-admission contract is
documented below. The superseded Jun-2 RFC is recoverable through
[`docs/plans/ARCHIVE-INDEX.md`](../../docs/plans/ARCHIVE-INDEX.md).
This directory holds the **Layer-3 (query latency)** tooling: capturing and
gating per-scenario warm and cold query latencies for the DSL query matrix.

Nothing here is "verified" until baselines are actually captured (Phase A)
on a quiet host at the head under test. There are no committed baselines
right now: the previous schema-1 baselines (short `git_rev`, no corpus /
config digest, host or resources, 200+ commits stale) were removed because
the gate below refuses them, and a stale baseline cannot be migrated into an
attributed one. `just rust-bench-dsl-compare` fails typed until a guarded
`python3 tools/benchmark/benchctl.py run dsl-authority --admit-baseline`
captures and admits both at `HEAD`.

Standalone `compare_dsl_bench.py --update-baseline` is refused: an externally
supplied receipt cannot prove it belongs to an existing artifact. Guarded
admission requires a clean source, canonical Linux preflight, fresh warm/cold
artifacts from the same run, matching measured host, and complete latency rows.
It cannot turn an `early_stop_reason` fixture gap into a committed reference.
The two baseline files are published under a durable
`tools/benchmark/baselines/.dsl-admission-pending` marker. A crash or failed
rollback leaves that marker in place; the runner, comparator, and artifact
checker refuse the pair until an owner inspects both files, restores or
recaptures them, and removes the marker. Ordinary second-file write failures
restore the prior pair and remove the marker.

## Artifact schema (the native producer format): `BenchArtifactV1`

`BenchArtifactV1` is the **native** format the rail binaries write. It is no
longer the common benchmark envelope: it is captured verbatim as a run's raw
evidence and wrapped in the typed `BenchmarkEvidenceV1` contract described
above. A `BenchArtifactV1` artifact stays valid, and the artifact checker below
still enforces its shape and attribution, but a common latency-row envelope is
not a correct payload for retrieval judgments, microbenchmark instruction
counts or recorded agent outcomes.

Every benchmark and relevance artifact — the DSL warm/cold matrices, the
  ambiguity, snippet, scale, tail, ANN, concurrency, freshness, open-loop,
  ops, and UI rails, the relevance rail and its OpenAI A/B capture, and the
  scan-vs-index experiment — is one `BenchArtifactV1`
envelope, written by exactly one writer
(`crates/quanta-index-searchd-harness/src/artifact.rs`, QI-BB-010). A
measurement that cannot say which source, corpus, configuration, model and
host it came from is not evidence, so the envelope is:

```json
{
  "schema_version": 2,
  "dimension": "dsl-warm",
  "mode": "warm",
  "concurrency": 1,
  "provenance": {
    "git_head": "0123456789abcdef0123456789abcdef01234567",
    "corpus_digest": "sha256:…",
    "config_digest": "sha256:…",
    "model_revision": "search-owned-hash-text-v1@fnv1a64-slots-l2unit-v1:d16"
  },
  "host": {
    "os": "linux", "arch": "x86_64", "cpu_count": 8, "mem_bytes": 17179869184,
    "hostname_hash": "sha256:…"
  },
  "resources": { "peak_rss_bytes": 123456789 },
  "phases": { "build_ms": null, "update_ms": null, "gc_ms": null },
  "disk_amplification": null,
  "rows": [
    {
      "scenario_id": "lexical.keyword.native",
      "route_family": "lexical",
      "syntax": "native",
      "result_shape": "candidates",
      "latency": { "p50_ms": 0.42, "p95_ms": 0.55, "p99_ms": 0.61, "samples": 200 },
      "qps": null,
      "error_count": 0,
      "timeout_count": 0,
      "result_count": 3,
      "typed_error_code": null,
      "engine_touched": ["lexical"],
      "early_stop_reason": null
    }
  ],
  "detail": {}
}
```

- `provenance.git_head` is the exact 40-character `git rev-parse HEAD` of a
  **clean** worktree, resolved by the rail binary itself. A dirty tree, a
  short SHA or an unresolvable head is a typed refusal
  (`BENCH_WORKTREE_DIRTY`, `BENCH_GIT_HEAD_NOT_FULL`, `BENCH_GIT_UNAVAILABLE`);
  there is no `"unknown"` and no env-supplied stamp.
- `corpus_digest` is a framed sha256 over the exact bytes the rail ingested;
  `config_digest` over the rail's parameters; `model_revision` names the
  embedder the fixture was built under (`null` only when no embedding model
  was exercised).
- `host` records the host the number came from (the hostname is hashed);
  `resources.peak_rss_bytes` is `getrusage(RUSAGE_SELF)` of the harness
  process, which drives the daemon in-process.
- `phases` carry build / one-file update / reclaim durations where the rail
  has that phase (the scale rail); `null` is "no such phase", never "not
  timed". `disk_amplification` is bytes written over changed bytes for a
  rail that wrote an index.
- `rows` carry p50/p95/p99, `qps` (the concurrency rail), and error / timeout
  counts. A row with `early_stop_reason` set was **not measured**: its
  `latency` is null. A baseline containing one is refused and a current one
  fails the comparison; absent measurement is never a zero-regression result.
- `detail` is the dimension's own shape (tier manifest, per-route budgets,
  judged queries, per-client-count tallies).

Route labels are semantic ownership labels, not result-shape aliases:
`lexical`, `semantic`, `hybrid`, `symbol`, `repomap`, `structural`, `history`,
and `runtime_catalog` remain distinct. A route with no qualified tail budget is
emitted without inheriting an unrelated lexical threshold.

### Stale-artifact gate

`python3 tools/ci/lint/check-bench-artifacts.py` (part of `just rust-policy`
and the CI policy job) walks every artifact family above and refuses one
that is not schema 2, whose head is not 40 lowercase hex, or — for a fresh
artifact under `artifacts/` — whose head is not the checkout's `HEAD`.
Committed baselines are held to the shape and a full head, not to head
equality. `compare_dsl_bench.py` additionally refuses a comparison whose
current side is not at `HEAD` or whose corpus, configuration, model revision,
or host identity differs from the baseline's. Absence is reported, not refused; `--require` (the Linux perf
evidence gate) fails when a family has no artifact.

## Producers

- **`warm-matrix.json`** is produced by the dedicated runner
  `dsl_warm_matrix` (bench-profile binary; each isolated pass boots a fresh
  runtime, the runner interleaves passes round-robin across scenarios, and the
  final row pools raw samples across those isolated passes).
- **`cold-matrix.json`** is produced by `run_dsl_cold_matrix.py` (true
  cold-start: a fresh OS process per sample, using a bench-profile prebuilt
  `dsl_cold_matrix` binary).
- warm measurements include the real daemon UDS front door. Because clients are
  one-shot, daemon accept-loop cadence is still visible in the tail metrics.
  The current adopted contract is
  `crates/quanta-index-searchd/src/app/searchd.rs`: query accept idle `1ms`,
  control/ingest accept idle `5ms`. Older artifacts captured under a uniform
  `50ms` accept poll are not comparable as-if they measured the same
  steady-state path.

The scenario authority lives in
`crates/quanta-index-searchd-harness/src/scenarios.rs` and currently covers all
four shipped families end-to-end plus an adversarial family (33 scenarios):
**lexical** (keyword / phrase / regex / `file.contains` / `repo:has.file`),
**history** (`since.time` / `since.commit` / `after` / `until` /
`diff.added|removed|touched`), **runtime catalog** (`dirty` / `changed` /
`stale` / `snapshot` / `meta.*` / `affected` / `invalidated_by`), **structural**
(boolean `OR` / `NOT` plus a genuine `match { … }` tree pattern), and
**adversarial** (malformed / unterminated / oversized-past-16 KiB /
nesting-past-depth-32 — exercising the *typed-error path latency*; fail-closed
must be fast). Several lexical/structural surfaces also have a sourcegraph twin
for native↔sourcegraph parity. Each is seeded by a deterministic fixture and
served through the real runtime — no mocked latencies.

Every scenario row also carries golden behavior truth:

- `expected_shape`
- `expected_count`
- `expected_typed_error_code`

The bench runners validate that truth before emitting latency artifacts. A
latency run that drifts in behavior now fails instead of quietly publishing
numbers for the wrong result shape.

## Convenience recipes

```
just rust-bench-dsl-truth       # small golden-truth smoke over the bench scenario table
just rust-verify-hellgate-fast  # fast correctness hellgate (bench truth + text + structural + guards)
just rust-verify-hellgate-broad # broad daemon lifecycle sweep
just rust-verify-hellgate-all   # fast + broad + warm/cold compare
just rust-bench-dsl-warm        # dedicated warm authority runner -> warm-matrix.json
just rust-bench-dsl-warm-criterion /external/bench  # common diagnostic Criterion capture
just rust-bench-dsl-cold 20     # cold matrix (20 samples/scenario) -> cold-matrix.json
just rust-bench-dsl-refresh 20  # warm -> cold -> compare, serialized authority run
just rust-bench-dsl-compare     # gate both matrices against tools/benchmark/baselines/
python3 tools/ci/lint/check-bench-artifacts.py --profile dsl-authority --require --skip-baselines
python3 tools/benchmark/benchctl.py list  # registered profiles + registry digest
python3 tools/benchmark/benchctl.py plan dsl-authority  # digest-bound plan; no mutation
python3 tools/benchmark/benchctl.py preflight dsl-authority --receipt artifacts/benchmark-receipts/dsl-authority/preflight.json
python3 tools/benchmark/benchctl.py run dsl-authority  # clean-host preflight, serial producer, validate, compare
python3 tools/benchmark/benchctl.py compare dsl-authority  # validate then run declared baseline comparators
python3 tools/benchmark/benchctl.py replay <run-id> --evidence-root /external/bench
python3 tools/benchmark/benchctl.py summarize systems  # observed artifacts only; never a pass claim
python3 tools/ci/lint/check-benchmark-policy.py  # registry / dependency direction / CI bypass policy
just rust-verify-quality-concurrency  # 1/8/32 clients + slow client -> concurrency/latest/summary-c*.json
```

The individual `just rust-<producer>` recipes remain the producer owners and
are invoked by `benchctl`; they must not be called directly from a CI timing
step (the policy guard refuses it) because that bypasses source freeze,
preflight, validation and immutable-run promotion.

The warm authority runner honours `$DSL_BENCH_WARM_SAMPLES` (default 100).
The dedicated warm runner also honours `$DSL_BENCH_WARM_REPEATS`
(default 5), `$DSL_BENCH_WARM_COOLDOWN_MS` (default 10),
`$DSL_BENCH_WARM_PASS_SETTLE_MS` (default 5), and
`$DSL_BENCH_WARM_PRIME_QUERIES` (default 5).
Authority artifacts must be produced **serially**. Do not run warm and cold
producers in parallel on the same machine and then treat the results as gate
authority; shared CPU/package-cache contention can distort warm tail advisories
and cold first-query latency.

## Golden-truth smoke rail

`just rust-bench-dsl-truth` runs the same bench `SCENARIOS` table without any
timing assertions:

- `warm_matrix_scenarios_match_golden_truth`
- `cold_matrix_scenarios_match_golden_truth`

This is the correctness companion to the latency tooling. Use it when the full
runtime E2E rails are too broad and you want a smaller fail-fast proof that the
bench scenario authority still executes the shipped behavior exactly.

## Hellgate split

Verification now uses four separate lanes:

- fast correctness
  - `just rust-verify-hellgate-fast`
  - bench-owned truth + small text-route + small structural-route + inventory
    guards
- broad daemon lifecycle
  - `just rust-verify-hellgate-broad`
  - real daemon boot, front-door, replay, restart, fail-closed, corpus sweep
- cross-repo ingress
  - `just rust-verify-hellgate-cross-repo`
  - external producer publish + ingress live roundtrip
- perf compare
  - `just rust-bench-dsl-compare`

Do not collapse them into one verdict. A green perf compare is not correctness.
A green fast hellgate is not restart/replay proof.

## Scripts

### `compare_dsl_bench.py` — regression gate

```
python3 tools/benchmark/compare_dsl_bench.py <baseline.json> <current.json> \
    [--rel-threshold F] [--abs-threshold-ms F] \
    [--p95-rel-threshold F] [--p95-abs-threshold-ms F]
```

- Both artifacts must be schema-2 `BenchArtifactV1` with a full `git_head`;
  the current artifact's head must be the checkout's `HEAD` and both must share
  the corpus/config digest, model revision and host identity. Any of these
  refuses the comparison with exit 2, as does a missing baseline (capture
  both with `benchctl run dsl-authority --admit-baseline`).
- Both artifacts must share the same top-level `mode`; a mismatch exits 2.
- Matches scenarios by `scenario_id`.
- Blocking metrics on both warm and cold artifacts:
  - **p50**: steady-state / first-query central tendency
  - **p95**: agent-loop tail; retrieval calls compound inside one turn
- Only `p99` remains an `ADVISORY` line.
- New scenarios (in current, not baseline) fail until a reviewed baseline update.
- Scenarios missing from current fail.
- `--update-baseline` is a legacy option that exits 2 without writing.
- Exit codes: `0` ok, `1` regression / missing-fail, `2` usage / mode-mismatch.

### `run_dsl_cold_matrix.py` — cold-matrix orchestrator

```
python3 tools/benchmark/run_dsl_cold_matrix.py --samples K \
    --out artifacts/dsl-bench/cold-matrix.json [--bin-cmd "..."]
```

Builds `dsl_cold_matrix` once on the `bench-lane`, then invokes the resulting
binary directly once per `(scenario, sample)` in a fresh process for a genuine
cold-start, and hands every sample to the same binary's `--assemble`, which
aggregates p50/p95/p99 (nearest-rank percentile) per scenario and writes the
`BenchArtifactV1` — the orchestrator never writes an artifact or stamps a
head. Default sample count is `20`; passing fewer samples is allowed for
ad-hoc local inspection, but the comparator refuses cold artifacts with fewer
than `20` measured samples per row. Warm artifacts require `200` samples per
row; the default warm runner pools `100` samples across `5` passes.

## Baseline admission and regression gate

The scheduled Linux job requires a pinned `self-hosted, linux, quanta-bench`
runner and is a blocking authority gate. Until its
reviewed canonical baselines are committed it fails typed; it is never silently
report-only. Run `python3 tools/benchmark/benchctl.py run dsl-authority
--admit-baseline` on a quiet canonical Linux host. It captures and validates
both artifacts in one guarded invocation, then writes both baseline candidates.
Review their metrics and scenario contract before committing them. A deliberate
scenario or semantic change requires the same review, not an automatic PR-side
update.

## Ratchet rule (exact)

A scenario regresses iff **both** legs are exceeded on the mode's blocking metric:

- **warm:** `p50 rel > +10%` **AND** `abs > +1.0 ms`
- **cold:** `p50 rel > +10%` **AND** `abs > +5.0 ms`
- **warm:** `p95 rel > +20%` **AND** `abs > +5.0 ms`
- **cold:** `p95 rel > +20%` **AND** `abs > +10.0 ms`

`--rel-threshold` / `--abs-threshold-ms` override p50 defaults;
`--p95-rel-threshold` / `--p95-abs-threshold-ms` override p95 defaults.

## Appendix: scan-vs-index scaling experiment (NOT a gate)

`run_scan_vs_index.py` + the `scan_vs_index` binary are an **exploratory
experiment**, deliberately separate from the 3-layer model above. They exist
only to make the *scaling* argument concrete, because the benchmark separation
decision forbids reporting DSL latency against a text-only engine as a
benchmark: a daemon IPC round-trip and a `grep` process answer different
questions, and at toy corpus sizes the
plumbing (IPC vs process spawn) dominates, which inverts the real picture.

The experiment removes that confound: it measures the lexical index query
**in-process** (no daemon, no IPC) and times `rg` / `grep` over the identical
corpus bytes, across corpus sizes. The result it demonstrates: index query
latency is ~flat in corpus size while a full scan is linear, so there is a
crossover beyond which the index wins per query (the index's one-time build cost
is reported separately and amortizes over many queries).

```
python3 tools/benchmark/run_scan_vs_index.py --scales 2000,20000,100000
```

Output goes to `artifacts/experiments/scan-vs-index.md` plus one
`BenchArtifactV1` per scale under `artifacts/experiments/scan-vs-index/`
(gitignored). The binary resolves the head itself and refuses a dirty tree;
the runner keeps the artifacts verbatim and adds only the scan timings. This
is never compared, gated, or written to the committed baselines. Only the
lexical keyword surface is even comparable to grep; history /
runtime-catalog / structural-tree queries have no text-engine equivalent.
