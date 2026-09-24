# Real-repository retrieval and context-yield benchmark

The Python evaluator scores recorded runner output; it does not query an
index, generate candidates, or infer missing evidence. The benchmark-only
Rust CLI in `benchmarks/retrieval` loads a pinned repository, chunks it,
publishes through the public SDK to a real daemon, and records route results.
The Rust CLI is the Quanta runner; the separate Python `semble.py` adapter
and `run.py` driver implement paired capture and comparison.
Python 3.9 and the standard library suffice for the evaluator; focused tests
use pytest. Runtime validation checks Git, file contents, hashes, split
isolation and complete route coverage.

## Run

Use a clean checkout pinned at the suite's `repository_commit`. Keep the suite,
query pack, runner record and report **outside** that checkout so they do not
change its Git status.

```sh
python3 -m tools.benchmark.retrieval freeze \
  --repo /absolute/clean/repository --suite /absolute/suite.json \
  --output /absolute/query-pack.json

# freeze prints the canonical query_pack_sha256 for the runner record.

# Build searchd and the benchmark CLI through scripts/cargow, then run the
# benchmark CLI against the blind pack. The manifest is JSON with exactly
# repository_commit and files [{path, file_sha256}] for the admitted universe.
# The state root and output must be outside the clean source checkout.
./scripts/cargow --lane bench-lane build -p quanta-index-searchd-runtime \
  --bin quanta-index-searchd --locked
./scripts/cargow --lane bench-lane run -p quanta-index-retrieval-bench \
  --bin quanta-index-retrieval-bench --locked -- run \
  --repo /absolute/clean/repository --manifest /absolute/manifest.json \
  --query-pack /absolute/query-pack.json --strategy brace_heuristic \
  --routes lexical,semantic,hybrid --top-k 20 \
  --repo-id benchmark-repo --revision-id pinned-revision --generation 1 \
  --state-root /absolute/fresh-state-root \
  --searchd-bin /absolute/quanta-index-searchd \
  --searchd-expected-sha256 <lowercase-sha256-of-searchd> \
  --runner-name quanta-sdk --runner-revision pinned-revision --run-id run-1 \
  --blinding attested --isolation-method query-pack-only \
  --access-block-log runner-was-not-given-gold \
  --out /absolute/recorded-run.json

python3 -m tools.benchmark.retrieval evaluate \
  --repo /absolute/clean/repository --suite /absolute/suite.json \
  --runner /absolute/recorded-run.json \
  --baseline-route lexical --candidate-route hybrid \
  --output /absolute/report.json
```

`run` defaults to the pinned `potion-code`/Model2Vec provider.
`--embedder hash-dev` is an explicit development-only correctness control. The CLI derives
model ID and revision from provider-owned constants; caller-supplied model
labels are not accepted. Other provider profiles need a separately verified
provenance path before they can emit benchmark records. The daemon verifies
the local PotionCode model assets on boot; missing or changed assets fail the
run. The CLI refuses dirty or wrong-HEAD repositories, untracked admitted
files, empty query-pack universes, stale non-empty state roots and existing
output files. A direct CLI invocation is normally `attested`. The paired
driver can invoke the CLI as `isolated` inside a verified macOS Seatbelt or
Linux Landlock boundary; the record then carries the driver-generated proof digest.
For that path, the driver first materializes a Git-free directory containing
exactly the manifest-admitted files, denies the entire original checkout and
both suite roots, and passes only the materialized corpus to both runners.
The verdict re-enumerates every materialized path/SHA and refuses extra files,
symlinks, Git metadata, or a corpus-proof mismatch.
The record label alone is never authority: `verdict` requires the frozen
policy/probe artifact and matching process-resource bindings.

The runner computes `query_pack_sha256` as SHA-256 of UTF-8 JSON serialized
with sorted keys, no whitespace, and `ensure_ascii=False`. A producer can use
the same implementation as `evaluator.digest(evaluator.canonical(pack))`.
The evaluator re-derives the pack from the frozen suite and rejects a different
hash. The pack includes suite commitment, repository commit, tokenizer, route
names and only eval task IDs, query text and query hashes. It contains no labels,
answerability bit or train tasks. `freeze` is the only query-pack producer.

## Current artifact contract

Only `schema_version: 3` suite, blind query pack, and runner records are
accepted. The stamp identifies the exact stored JSON shape; it does not select
an internal IR or a compatibility reader. Older and unknown stamps fail before
scoring. Re-capture old benchmark outputs from the pinned source rather than
converting them in the evaluator.

The suite requires a `comparison_contract` with `top_k`, `tokenizer`,
`tokenizer_budget_version`, `output_unit_policy`, and the byte-span unit.
Gold spans may carry grades 1–3 only. A grade-0 (irrelevant) judgment is not
a gold span: including it would mark a task answerable and award Recall/BCY
credit despite zero NDCG gain. No-answer tasks have an empty `gold` list.
The blind pack echoes this contract without train rows, labels, grades, or
answerability bits. The runner record echoes the same contract, binds each
route to a capture, and binds captures to binary, generation, receipt,
activation, and model identities. Candidate credit uses proven byte spans;
line spans are checked projections. All-answerable external suites are valid;
an absent no-answer stratum reports `not_applicable` rather than invented
tasks. Invalid provenance, hashes, fields, or version stamps are refused.

`gold_access: false` is a runner attestation. The evaluator can verify that
the supplied pack is blind and that the record contains no gold fields; it
cannot independently prove the runner did not open the suite outside this
protocol. Use isolated runner credentials or process sandboxing when that
assurance is needed.

## Metrics and comparison

For 2k, 4k, 8k and 16k benchmark tokens, candidates are packed in rank order.
Packing stops at the first candidate that exceeds the remaining budget.
`BCY@budget` is the fraction of answerable eval tasks whose **every** gold
block is fully covered by packed candidates. `file_recall` and `block_recall`
are task-macro averages. `no_gold_abstention` is the fraction of no-gold tasks
with explicit abstention, or `not_applicable` without a real no-answer
stratum. `mean_context_tokens` is averaged across all eval tasks. The report
gives these metrics for each route and the selected route pair's deltas and
paired win/loss/tie counts (where success means BCY for answerable tasks and
abstention for no-gold tasks). A candidate matching a gold file but missing
its gold byte span cannot earn block credit.

The current scorer reports span-aware Recall@1/5/10/20, MRR@10, graded NDCG@10 (only when every
eval answerable gold label carries a reviewed grade, else `not_applicable`),
file-only recall as a secondary view, and both chunk-level and deterministic
same-file-collapsed rankings (best chunk per file). NDCG credits each gold span
only on its first covering candidate: overlapping chunks cannot earn the same
gain twice or inflate NDCG above 1. The report labels this scoring contract
`rb-rank-context-density-first-coverage`; reports with a different scoring identity are
not comparable on overlapping-chunk suites. Duplicate candidate byte spans are
rejected. Distinct overlapping chunks consume rank and context budget but
earn no second relevance gain. Primary metric is
`ndcg_at_10` when graded, else `recall_at_10`. Reports include per-query rows,
per-route status counts and mean latency, sample counts, and a deterministic
stratified-bootstrap 95% interval on paired deltas when the sample reaches 20, else an
explicit insufficient-sample marker. Re-scoring immutable records is
deterministic under row order; scores depend only on recorded spans and
statuses, never on runner identity strings.

W0-A relevance rubric: a grade-3 gold span is independently judged sufficient
answer evidence; grade 2 is substantial but incomplete evidence; grade 1 is
weak supporting evidence. Grade 0 is excluded from `gold`. Only full byte-span
containment earns rank gain; partial overlap earns zero. Each gold span is
credited on its first covering candidate, and a candidate uses the highest
newly covered grade. Its gain is `(2**grade - 1) * U/C`, where `U` is the union
byte length of newly covered gold spans and `C` is the candidate byte length.
The ideal ranking is one exact candidate per gold span in descending grade
order. Thus an exact span gets full credit, while a whole-file hit containing
10 relevant bytes in 1 MB gets only 10/1,000,000 of that gain. Rank ties are
resolved by the recorded unique rank; the report publishes both raw chunks
and deterministic best-chunk-per-file collapse. Token-budget BCY remains a
separate, primary context-coverage diagnostic. Byte density is an explicit
precision policy, not a claim that shorter context is always semantically
better; report language/category strata and BCY alongside NDCG.

The formula alone does not qualify a quality claim. `QUALITY_DELTA` still
requires W0-B's independently adjudicated graded gold, verified isolation,
model/source/receipt admission and paired uncertainty. No real admitted pair
has supplied that evidence yet.

The report is evidence only for the supplied frozen suite, pinned repository
and runner record. No arbitrary pass threshold or claim of production retrieval
quality is inferred.

## Runners and paired orchestration

`semble.py` runs a pinned Semble install (an outside-the-checkout virtualenv)
against the admitted manifest and a projected query pack, then normalizes
native hits into a v3 record with proven spans. It always emits the
path-mapping proof (path map plus both-side path+SHA diff); any skipped or
extra file fails the common-universe pair with a typed reason. `run.py`
drives the Rust SDK runner per chunking strategy (`quanta`), runs both
systems sequentially from a pinned spec (`pair`), deterministically merges
per-system records (`merge`), re-scores immutable records into the TEST-PLAN
§8 verdict artifact (`verdict`), records the host check (`host-probe`), and
freezes a canonical machine/power fingerprint (`host-profile`).

Each system consumes a projected pack (same tasks/universe/commit, narrowed
routes, rebound suite commitment); the merge re-derives and re-validates
every projection with this evaluator before scoring. Records, corpora,
caches and reports stay under an explicit output root outside the checkout.
See `docs/plans/sep-23-retrieval-bench/tickets/` for the protocol and the
`just retrieval-*` recipes for the registered entry points.

## Pair spec schema

`run.py pair --spec SPEC` (and `run.py quanta`) take a JSON spec. Required
keys: `repo`, `manifest`, `suite`, `query_pack`, `top_k`, `output_root`,
`runner_binary`. Optional keys and defaults:

| Key | Default | Meaning |
| --- | --- | --- |
| `routes` | `["lexical","semantic","hybrid"]` | Quanta routes (must be suite routes) |
| `strategies` | required for `quanta`/`pair` | e.g. `[{"name":"whole_file"},{"name":"brace_heuristic"}]` |
| `searchd_binary` | required | explicit daemon pin; unpinned capture is refused |
| `embedder` | `potion-code` | Rust runner embedder profile (`hash-dev` is an explicit diagnostic control) |
| `repo_id`/`revision_id`/`generation` | `bench-repo`/`bench-rev`/`7` | batch identity |
| `runner_name`/`run_id` | `quanta-sdk-runner`/`run` | runner identity; `runner_revision` is derived from the binary SHA-256 |
| `blinding` | `attested` | `isolated` requires the enforced Seatbelt (macOS) or Landlock (Linux) path; unsupported or unavailable backends fail closed |
| `suite_secret_root` | none | required for `isolated`; external evaluator-only root containing the suite and no runner-readable input |
| `isolation_method`/`access_block_log` | `attested-only…` | supplied for attested runs; driver-generated and proof-bound for isolated runs |
| `semble_python` | required for `pair` | pinned Semble venv interpreter |
| `semble_lockfile` | required for `pair` | external hash-pinned lockfile path (frozen into the stage) |
| `semble_lockfile_sha256` | required for `pair` | SHA-256 of the external lockfile; env must carry every locked line plus `semble==0.6.0` |
| `semble_route` | `semble-hybrid` | Semble record route name |
| `semble_cache_root` | `<out>/semble-cache` | Semble + HF caches (outside checkout) |
| `query_repetitions_per_root`/`query_warmup_passes` | `1`/`1` | one driver-generated, digest-bound randomized schedule consumed by both runners; qualified speed requires warmup >= 1 and at least 1,000 warm observations per route across roots |
| `semble_model_revision` | observed | pinned HF revision (drift fails) |
| `quanta_model_dir` | none | explicit local model directory; required for a `potion-code` speed claim and counted separately from index storage |
| `repetitions` | `1` | external reps on fresh state; qualified speed requires at least 5 |
| `alternate_order` | `true` | alternate system order per rep; qualified speed rejects `false` |
| `order` | `["quanta","semble"]` | base system order |
| `baseline_route` | Semble route | report baseline |
| `scope` | `exploratory` | `exploratory` or `qualified` |
| `admission` | required for `qualified` | paths to the W0-B manifest, license receipt, two independent annotation receipts, and adjudication receipt |
| `host_profile` | required for `pair` | path to a generated host-profile JSON; the file is frozen, digest-bound, and matched against both host probes |
| `claims` | all `false` | `{quality,speed,same_model,incremental}` |
| `receipts` | omitted | paths to contract/SDK summaries, receipts, raw JUnit/nextest JSONL, actual-runner record and Python/Rust/SDK collection inventories; all bytes are frozen and raw evidence is reparsed by the verdict |
| `timeout_secs` | `1800` | per-capture timeout |

Before a paired run, author a hash-pinned external lockfile for the exact
Semble virtualenv, put its path in `semble_lockfile` and its SHA-256 in
`semble_lockfile_sha256`, then preflight the env (the observed `pip freeze`
must carry every locked line; the recorded `installed_distribution`
RECORD/direct_url digests identify the installed bytes):

```sh
python3 -m tools.benchmark.retrieval.semble check \
  --python /absolute/semble-venv/bin/python
```

The external lockfile and installed-distribution checks pin the declared
environment; they do not by themselves prove which model
weights Semble loaded. The adapter sets Semble's documented
`SEMBLE_MODEL_NAME` to the requested model and verifies the worker-reported
setting. It also requires an observed Hugging Face
cache revision and rejects a supplied revision that disagrees with it.
`same_model` remains an external claim needing its own evidence. An
`attested` pair cannot qualify isolated-blind quality. The paired driver can
attempt `isolated` capture under Seatbelt on macOS or Landlock on Linux; the
verdict still requires the frozen tagged proof v2, re-probes the boundary after
stage relocation, and checks Linux child attestation. Landlock availability
must be probed on the actual host. This proves a filesystem-path read boundary,
not IPC or mount topology isolation. Neither mode alone qualifies speed.

Qualified capture runs the canonical retrieval source-closure check before staging;
dirty relevant source is a hard refusal. The closure is frozen into the run,
reverified after capture, cross-bound to all contract/SDK receipt closures, and
bound by both the protocol lock and run manifest. Isolated capture executes stage-local,
SHA-bound copies of the Semble adapter/evaluator rather than reading the checkout.
Qualified quality also requires an estimable paired category-stratified bootstrap CI.

Generate the host profile on the measurement host before authoring the pair
spec. `PERF_QUALIFIED` requires the frozen fingerprint, normalized active-source
power digest, clean thermal state, directly observed frequency bounds, and no
competing benchmark/build process at both start and end. Missing Apple Silicon
frequency telemetry is `unavailable`, not inferred from a power plan:

```sh
python3 tools/benchmark/retrieval/run.py host-profile \
  --profile-id macbook-m4-ac-power --out /absolute/host-profile.json
```

On Linux, host-profile v2 requires an operator-selected CPU thermal zone and
explicit limits; mere sensor presence is not speed evidence. The selected zone
name and sensor type, per-CPU maximum frequencies, and the performance governor
set are pinned. Per-CPU scaling min/max, driver, and an exposed disabled
boost/turbo control are also pinned; hosts without these controls cannot qualify speed.
Both start/end probes must observe every CPU at or above the
configured percentage of its pinned maximum and the selected thermal zone at
or below the configured ceiling. The ceiling cannot exceed 85,000 millidegrees
and the frequency floor cannot be below 80 percent. Unsupported or partial
telemetry makes speed unavailable; the operator must establish that the selected
zone actually represents CPU/package temperature. These endpoint checks do not
prove absence of transient throttling during the run.

```sh
python3 tools/benchmark/retrieval/run.py host-probe
python3 tools/benchmark/retrieval/run.py host-profile \
  --profile-id linux-perf-host \
  --linux-thermal-zone thermal_zone0 \
  --linux-max-thermal-millidegrees 80000 \
  --linux-min-frequency-percent 90 \
  --out /absolute/host-profile.json
```

Resource evidence is schema-closed: aggregate and per-process peak RSS/CPU,
index/model/parser/embedding-cache bytes, discovered file count, indexed chunk
count, disk-vs-memory ownership, and the measurement method are mandatory.
Semble in-memory index bytes are a worker-observed peak-resident delta (Unix
peak RSS; Windows peak working set), not an exact allocation count. They must
be positive and byte-equal in native/resource evidence. The Windows path is
implemented but not native-host qualified. For each fresh root the
driver emits one SHA-256-bound query protocol containing a cold probe, randomized
warmup permutation(s), and randomized measurement permutations. Quanta and
Semble must echo that exact protocol and raw per-route/per-task warm latency
arrays in phase evidence. The verdict rejects protocol drift, incomplete
permutations, count mismatches, first-sample/record disagreement, or cold samples
entering the warm matrix. Qualified speed additionally requires exactly one
Quanta route, at least 20 tasks, at least 5 fresh roots, and at least 1,000 warm
observations per route. Cold-query latency remains separate and never contributes
to the qualified matrix. A legacy cold-only manifest cannot be upgraded by
editing its claim fields.

The validator regenerates the complete protocol from the frozen seed, task order,
warmup count and measurement count. A different permutation with a recomputed,
internally valid SHA-256 is still rejected.

The 2026-09-24 adversarial audit found qualified replay gaps in an earlier
source. The current implementation enforces the same independent 20-task,
one-Quanta-route, five-root, warmup and 1,000-observation requirements at
capture and replay; repetition cannot replace missing tasks. Raw serial call
durations must fit their enclosing monotonic phase windows within a declared
clock/serialization tolerance. The protocol lock rejects unknown keys and
re-derives every retained pin. The lock's retained keys are
`suite_digest`, `query_pack_digest`, `corpus_manifest_digest`, `top_k`,
`strategies`, `searchd_expected_sha256`, `semble_lockfile_sha256`,
`host_profile_digest`, `admission_digest`, `driver_source_closure_digest`,
`repetitions`, ordered `system_orders`, `base_seed`, `query_warmup_passes`,
`query_repetitions_per_root`, and ordered `query_protocol_sha256s`. Do not
retain an unbound `spec_digest` without freezing its canonical source artifact.
Qualified speed replay checks the recorded order alternates across roots; this
is a driver-attested ordering claim, not an independent wall-clock chronology.
At the audited baseline, a two-task fixture passed speed replay and impossible
raw durations passed phase validation. Focused regressions now reject both;
real qualified performance evidence remains pending.

Notes: the first Semble index includes the model download (later runs reuse
the cache; `index_stats` and `semble_index_ms` always record what ran).
Partial output is never resumed — rerun from a fresh output root. To re-score
a frozen pair, use `just retrieval-verdict <repo> <suite> <run-manifest> <out>`.
The verdict resolves and revalidates record/report paths from the manifest;
there are no separate records, route, or baseline CLI arguments.

Successful atomic promotion must preserve canonical merged record, report and
verdict identities. At the audited baseline, absolute input paths in merged
provenance made unchanged stage files fail after relocation. The merger now
uses relative capture provenance, and a focused regression checks promoted
replay in a fresh process. A final admitted pair still requires the public
verdict at its promoted path with independently re-derived identities.

## T00–T17 evidence map

The IDs map to a test target, command and artifact. A listed command describes
an evidence producer, not a current qualification result. `NOT_RUN` means a
required rail has no current-source evidence; T15 and T16 are
`NOT_APPLICABLE` when their respective claims are absent.

| ID | Target | Command / artifact |
| --- | --- | --- |
| T00 | `benchmarks/retrieval` corpus loader + `semble.py` mapping proof | `just retrieval-contract-proof <fresh-output-root>`; `mapping-proof.json` (path map + both-side path+SHA diff) |
| T01 | `tools/ci/tests/test_retrieval_benchmark.py` (runner independence) | `just retrieval-contract-proof <fresh-output-root>`; tampered-record mutants must fail |
| T02 | split-leakage custody: duplicate/near-duplicate queries, query families and answer-span overlap | `just retrieval-contract-proof <fresh-output-root>`; explicit both-sides allowlist mutants |
| T03 | same (schema rejection: fields/routes/timings/status and complete protocol-lock pins) | `just retrieval-contract-proof <fresh-output-root>`; missing/mutated/unknown lock fields must fail |
| T04 | byte-span Recall/MRR/NDCG/BCY scoring and deterministic same-file collapse | `just retrieval-contract-proof <fresh-output-root>`; hand-calculated coverage/budget mutants |
| T05 | `sdk_roundtrip.rs` process/frontdoor | `just retrieval-sdk-proof <fresh-output-root>`; pinned actual runner + separate daemon, readiness and empty-state checks |
| T06 | same, SDK write authority | sealed receipt + exact composite activation ACK; direct IPC/fixture helpers refused by static guard |
| T07 | same, SDK read authority | lexical/semantic/hybrid SDK reads, generation pin and typed failure behavior |
| T08 | `benchmarks/retrieval/tests/chunking_contract.rs` | `just retrieval-contract-proof <fresh-output-root>` (chunking contract, no daemon); `just retrieval-contract-local` for diagnostic edits |
| T09 | same (oracle cases + fallback accounting) | `just retrieval-contract-proof <fresh-output-root>` |
| T10 | per-strategy generation/capture/model binding plus deterministic replay | `just retrieval-sdk-proof <fresh-output-root>`; `just retrieval-contract-proof <fresh-output-root>`; real ablation evidence remains unrun |
| T11 | `semble.py` mapping proof + adapter tests | `mapping-proof.json`; `just retrieval-contract-proof <fresh-output-root>` |
| T12 | `run.py pair` (same universe/host, full lock and speed eligibility) + `host.json` | `just retrieval-pair <spec>`; two-task and pin refusal regressions implemented; real frozen pilot `NOT_RUN` |
| T13 | `run.py verdict` (deterministic re-score and phase/sample consistency) | fresh-process promoted replay regression implemented; admitted final-path `verdict.json` pending |
| T14 | registered commands and external artifact root | `just benchmark-prep-local`, `just retrieval-contract-local`, `just retrieval-contract-proof <fresh-output-root>` and `just retrieval-sdk-proof <fresh-output-root>`; no implicit model download; real-pair promotion/replay pending |
| T15 | model parity (conditional on a same-model claim) | `NOT_APPLICABLE` without that claim; `NOT_RUN` only if claimed without proof |
| T16 | incremental capture (conditional on an incremental claim) | `NOT_APPLICABLE` without that claim; `NOT_RUN` only if claimed without proof |
| T17 | W0-B qualification admission | `admission.schema.json` plus license, two annotation, adjudication, model, host, lockfile and exact contract/SDK receipt digests; exploratory runs are never promoted |

At `a634b90a`, clean-source proof executed all 173 Python contract,
51 Rust contract and 12 SDK required identities without skips and issued
three receipts against one source closure. Those receipts qualify only their
recorded revision and bound inputs. `pair-spec.schema.json` names the
three inventory inputs; `run-manifest.schema.json` binds their paths and
SHA-256 claims. Contract/SDK collection inventories and raw terminal evidence
must match the required identities; the verdict reads the authority Git blob
at the receipt revision and compares its digest with the receipt's source
closure. At the audited baseline, a fixture with one pass and 159 skips
certified contract proof; focused regressions now reject that case. Any later
bound code, test or normative-document edit requires new contract/SDK receipts
from one v2 source closure;
the generic workspace rail must record a post-change frozen-source result;
and OS-specific isolation and phase/process-tree RSS still need a real admitted
quiet-host pair meeting their proof and sample floors. The commands above
prove code paths, not W0-B, `PAIR_VALID`, `PERF_QUALIFIED` or `QUALITY_DELTA`. No tracked
real-pair `run-manifest.json` or `verdict.json` is a benchmark result here;
external evidence must be supplied and revalidated before a quality claim.
