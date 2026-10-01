# Retrieval benchmark usage

Run from the repository root. Start with the [code-search runbook](../CODE_SEARCH_RUNBOOK.md)
for live five-product capture or a registered Quanta–Semble pair. The commands
below operate on external frozen inputs and fresh output roots.

## Native Quanta capture and recorded evaluation


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

Use `--embedder hash-dev` only for development diagnostics. The default
`potion-code` uses the historical effective 512-token V1 encoder with verified
local assets. `potion-code-full-v2` removes that tokenizer cap, admits at most
16 KiB of UTF-8 per text and 4 MiB per 1,024-text model batch, and requires a
fresh vector generation; pair captures under it are exploratory with no quality,
speed, or same-model claim. A dirty/wrong-HEAD
corpus, stale state root or existing output is refused. `freeze` creates a blind
pack from an authored suite; it does not generate gold labels.

For a **declaration-name** diagnostic, keep bare ASCII names in a separate
suite with `routes: ["symbol"]` and run with `--routes symbol
--query-input-policy exact_symbol_name`. The runner plans each name as
`symbol.local_name.exact(name) case:yes`, binds that effective request in the
record, and refuses DSL text or any other route. This is a different query
intent from bare lexical content search. Mechanically generated declaration
labels still need independent review before quality qualification; this
standalone profile is not a five-product file-rank comparison.

New exact-symbol records explicitly bind `rank_unit: symbol` and retain distinct
published IDs and indexed declaration spans even when two declarations share
one returned context span. Their diagnostic projection uses `symbol-unit-v1`;
ordinary content captures retain `first-source-span-v1`. Replay validates each
projection against its bound record. Historical exact-symbol records may omit
the explicit rank field and continue to use their original span contract, but
they are ineligible for declaration-rank judgment metrics: policy alone does
not prove that same-line declarations retained independent ranks.
Native symbol keyword captures retain the original first-source-span context
projection; distinct declaration ranking requires the exact-symbol profile.

For a **distinct-file lexical** diagnostic, keep the bare source queries in a
separate suite with `routes: ["lexical"]` and run with `--routes lexical
--query-input-policy literal_file`. The runner emits `select:file` over a
quoted, escaped content phrase and binds the effective request hash to the
original query. Its result unit is a distinct repository-relative file, even
though each file's source evidence remains the representative published chunk.
A quoted phrase is a match-only (constant-score) content restriction, so the
files come back in repository-path order, not relevance order: its top 10 is an
observed, path-ordered prefix of the matching set, and a `capped` result is
truncated alphabetically. Hit@10 on it is an observed-prefix measure, not a
scored ranking quality. A bare keyword `select:file name` is a different,
scored request over content and path tokens and is not this policy.

Two further file-projection policies measure the other request shapes
explicitly; each has its own profile, policy-config identity and request
golden, re-derived independently by `query_plan.py`:

- `keyword_file` (`quanta-keyword-file-v1`): one bare ASCII identifier of at
  most 256 bytes (not `AND`/`OR`/`NOT`) becomes `select:file case:yes <name>`,
  a scored, case-sensitive keyword over content **and** path tokens. Files come
  back by descending score (`ordering: score_desc_path_tiebreak`), so its file NDCG
  (reported by `evaluate-diagnostic`) is a ranking number. Historical records
  lacked scores and rely on the policy and pinned runner binary for their order;
  new records preserve finite SDK scores per file and verify score/path order.
  A file whose path alone contains the name can
  match; this is not the content-only phrase contract.
- `substring_file` (`quanta-substring-file-v1`): one fragment of 3–256 bytes
  without a single quote or control character becomes
  `select:file case:yes '<fragment>'`, a case-sensitive raw-substring
  restriction (trigram candidates, byte verification) over content. It is
  match-only, so files return in path order (`ordering:
  path_order_constant_score`), like `literal_file`.

Every file-projection result records `rank_unit: distinct_file` (the unit) and
`ordering` (how the units are ordered, derived from the policy). The evaluator
refuses a relabeled ordering, a path-ordered result that is not in path order,
an ordering on a chunk result, and a missing ordering on the two new policies;
historical `literal_file` records without the field keep their derived path
order. `evaluate-diagnostic` reports file `hit_at_10` and `recall_at_10` next
to `ndcg_at_10`, with `rank_metric_interpretation` `scored_ranking` or
`observed_path_order_prefix`. A plan refusal (for example a digit-leading
fragment under `keyword_file`) refuses the whole run, so run such tasks in a
subset suite and report them as unsupported query forms.
New `keyword_file` records also carry `score_evidence: native_sdk_score_v1`
and each candidate's finite SDK score. The recorder and evaluator reject
ascending scores and out-of-order path ties. Historical v5 records without
these fields remain replayable, but their diagnostic route reports
`score_evidence: not_recorded`; their score order is supported by the policy
and fixture only, not by per-row captured scores. Do not promote those older
rows to a score-order-qualified claim.
Semble's separate `lexical-file` profile requests all indexed chunks from
the pinned BM25 lane, keeps that positive-score native list in `native.json`,
and records each file at its first source rank until ten distinct files are
selected. It emits `rank_unit: distinct_file`,
`ordering: score_desc_native_tiebreak`, per-file BM25 scores and collection
counts. Upstream ties retain native order; they are not path tie breaks.
`lexical-only` continues to return ten chunks. The two profiles must not be
combined in a single score or latency comparison.
The evaluator requires the recorded `rank_unit: distinct_file` and rejects
repeated file paths for this policy. Source-reviewed file judgments and their
metrics are opt-in; the original 300-query native capture stays on its frozen
policy and report contract.

The benchmark runner refuses native `select:` and `type:path`/`type:repo`
projections that change the ranked result unit. Use a file-projection policy (`literal_file`, `keyword_file` or
`substring_file`) for a distinct-file benchmark. A quoted `"select:file"` remains ordinary content text
under `native`.

After freezing the reviewed suite and recording its single-route run, score
independent file or declaration judgments with:

```sh
uv run --frozen --extra dev python tools/benchmark/retrieval/evaluator.py evaluate-diagnostic \
  --repo /absolute/corpus --suite /absolute/suite.json \
  --runner /absolute/record.json --output /absolute/diagnostic.json
```

This report exposes eligible task IDs, exclusions, coverage, operational and
conditional means. It is `diagnostic_unqualified`; it does not enter the
paired `QUALITY_DELTA` gate or alter the original 300-query scores.
Use `judgment_policy: complete_ranked_pool_v1` for newly reviewed file or
declaration diagnostics. Each returned top-10 file or published declaration
must have an explicit source-bound grade, including grade 0 for irrelevant
results. A missing judgment excludes that task with `unjudged_ranked_file` or
`unjudged_ranked_declaration`; it is not silently scored as irrelevant. The
historical `unjudged_zero_v1` policy remains available for exploratory reports
and retains its original behavior. Neither policy turns a post-result review
into a pre-result qualified holdout.

For objective lexical checks, a task may instead declare `source_oracle` with
`contract: go_exact_local_name_v3` and `unit: symbol` or `distinct_file`, or
`contract: ascii_identifier_word_v1` and `unit: distinct_file`. Set
`query_intent: bare_symbol`, `judgment_policy: source_oracle_complete_v1`, and
provide exactly the matching judgment kind. The evaluator reparses every Go
file or scans ASCII identifier words across the frozen file universe, then
requires an exact match with all submitted positive grade-3 judgments. An
absent judgment is therefore an exhaustive source-oracle negative, not a
human relevance decision. The declaration contract covers the Go symbol
producer's functions, methods, type specs, type aliases, and interface methods
declared directly under a named type, by exact case-sensitive local name.
Anonymous interface methods outside named types are excluded. Symbol judgments
use the exact indexed declaration byte span; the matched local-name bytes select
the declaration but do not stand in for its published symbol span. The word contract
counts identifier words anywhere in file bytes, including comments and tests.
`label_review` is forbidden on these tasks, and qualified annotation receipts
reject them. Reports remain `diagnostic_unqualified`.

Identifier-robustness variants use four more contracts over the same indexed Go
declaration set: `go_declaration_name_prefix_v1` and
`go_declaration_name_infix_v1` (case-sensitive name text, at least three
characters), `go_declaration_name_osa1_v1` (every other name at optimal
string alignment distance one; an edited query that is itself a declaration
name is an exact-name collision, not a typo), and
`go_declaration_name_components_v1` (space-separated lowercase components
matched as a contiguous run of `camel-snake-v1` components; non-ASCII names
have no components). `identifier_robustness_suite.py` builds one suite per
lane from an unannotated frozen suite and a seed, with a census of strata,
ambiguity classes, shortfalls and no-answer content presence:

```sh
python3 tools/benchmark/retrieval/identifier_robustness_suite.py \
  --repo /absolute/clean/repository --baseline-suite /absolute/baseline.json \
  --output-root /absolute/fresh-output --seed 20260930
```

Its base names come from an exposed suite, so its output is a source-exposed
diagnostic, never an unseen holdout.
The generated `no-answer-content-v2` suite uses
`ascii_content_absent_casefold_v1` with `unit: distinct_file`. Replay admission
checks every frozen source file for the query under the same UTF-8 replacement
and casefold rule used by the builder. The separate `no-answer` lane continues
to mean only that no matching declaration exists. Archived NOC suites retain
their older declaration-only oracle and must be labeled as legacy diagnostics.

For a validated Quanta or Semble distinct-file diagnostic, join the existing
evaluator report to the frozen robustness census without rescoring candidates:

```sh
uv run --frozen --extra dev python -m tools.benchmark.retrieval.identifier_robustness_report \
  --repo /absolute/corpus --suite /absolute/suite.json \
  --record /absolute/record.json --diagnostic /absolute/diagnostic.json \
  --census /absolute/census.json \
  --generation-manifest /absolute/manifest.json --lane prefix \
  --output /absolute/new-external-root/report.json
```

The output records generator admission, unsupported request forms, execution
status, eligible unique/ambiguous Hit@10 and no-answer abstention separately.
The generation manifest must bind the exact census and lane suite bytes; its
SHA-256 is included in the report so a later review can identify the admitted
population. A supplied manifest is producer provenance, not an independent
human label or qualification authority.
It refuses an existing output path and is always `diagnostic_unqualified`.
Sourcegraph, cs and OpenGrok have a different native capture contract and are
not admitted through this report command.

Create reproducible, runnable single-route suites from an unannotated frozen
source suite with a new external output root:

```sh
uv run --frozen --extra dev python -m tools.benchmark.retrieval.source_oracle_suite \
  --repo /absolute/clean/corpus \
  --baseline-suite /absolute/frozen-suite.json \
  --output-root /absolute/new-oracle-output
```

The generator writes three suites and blind packs: identifier-word/file and
Go declaration/file use the `lexical` route; Go declaration/symbol uses the
`symbol` route. Each suite has exactly one route for `evaluate-diagnostic`.
Each mode recomputes `answerable` and replaces the baseline's authored gold
with the line containing the first source match by path and byte offset.
These mechanical gold lines are diagnostic evidence, not human relevance
labels. A mode without a source match retains the task as unanswerable with
empty gold. The evaluator still refuses cross-split source-label leakage.
Source-oracle admission rejects more than 4,096 files, 2,000 queries, or
512 MiB of source bytes before materializing an over-limit file. This is a
corpus input limit, not a peak-RSS guarantee.
The output manifest binds the input suite, source commit, tool file bytes, and
all outputs; exact tool sources are copied under `tool-sources/`. These are new
diagnostic inputs; their blind-pack digests differ
from historical captures, so historical runner records cannot be reused.
For preserved Sourcegraph, OpenGrok, and cs file rows, rescore the original
300-query capture against complete source-derived file judgments without
rewriting its rows or mixing native chunk records from another execution:

```sh
uv run --frozen --extra dev python -m tools.benchmark.retrieval.lexical_external_oracle \
  --repo /absolute/clean/corpus \
  --suite /absolute/original-suite.json \
  --query-pack /absolute/original-blind-pack.json \
  --capture-manifest /absolute/native-external/capture.json \
  --sourcegraph-rows /absolute/native-external/sourcegraph_rows.jsonl \
  --opengrok-rows /absolute/native-external/opengrok_rows.jsonl \
  --cs-rows /absolute/native-external/cs_rows.jsonl \
  --out /absolute/new-output-root/result.json
```

This verifies the capture manifest's suite, pack, row, and raw response file
digests, then revalidates all row queries, paths, statuses, and original hit
flags. File recall and binary file NDCG use `file_judgments`, which list every
source match. The suite's `gold` is a representative first source line and
must not replace those complete file judgments. Both contracts remain
mechanical diagnostics. Backend indexed-universe equivalence and human
relevance are still unproved. Digest verification does not replay native
response parsing at the capture's original source revision. The result is not
a five-product quality rank.
When the original paired native capture is retained as `native-tree.zip`,
rescore all five from the same frozen suite, pack, and corpus:

```sh
uv run --frozen --extra dev python -m tools.benchmark.retrieval.lexical_five_product_oracle \
  --repo /absolute/clean/corpus \
  --suite /absolute/original-suite.json \
  --query-pack /absolute/original-blind-pack.json \
  --capture-manifest /absolute/native-external/capture.json \
  --sourcegraph-rows /absolute/native-external/sourcegraph_rows.jsonl \
  --opengrok-rows /absolute/native-external/opengrok_rows.jsonl \
  --cs-rows /absolute/native-external/cs_rows.jsonl \
  --native-evidence /absolute/pair-evidence/evidence.json \
  --native-archive /absolute/pair-evidence/raw/native-tree.zip \
  --out /absolute/new-output-root/result.json
```

The evidence binds the original native archive and the suite/pack bytes. The
pair report, verdict, query identities, result ranks, file hashes, and original
hit flags are checked before new judgments are scored. Native rows still end
at 10 **chunks**, so their file NDCG is explicitly an observed-prefix score
after first-occurrence file deduplication. External rows end at 10 distinct
files. The two NDCG columns and hit counts remain separate diagnostic units.
The previous `go_exact_local_name_v1` covered fewer Go declaration kinds.
`go_exact_local_name_v2` covered the same declaration kinds but used local-name
token spans as symbol judgments. Published symbols use definition spans, so v2
symbol scores were invalid. Replay archived v1/v2 suites with their snapshotted
tool sources; the current validator accepts only v3 for new Go diagnostics.

Before review or search, check a new query proposal pool against every
previously searched suite and proposal pool:

```sh
uv run --frozen --extra dev python -m tools.benchmark.retrieval.query_pool_guard \
  --repo /absolute/clean/corpus \
  --reference-suite /absolute/previous-suite.json \
  --reference-proposals /absolute/previous-proposals.jsonl \
  --candidate-proposals /absolute/new-proposals.jsonl \
  --output /absolute/new-pool-check.json
```

The guard validates the reference suite against source, binds input SHA-256s,
and reports every normalized or shingle near-duplicate at the evaluator's
threshold (up to 10,000 conflict rows). Repeat `--reference-suite` and
`--reference-proposals` for additional searched inputs. It exits 2 on a
conflict or malformed input and never overwrites an existing report. This
early check does not replace frozen experiment custody or independent gold
review. Candidate JSONL rows must have `proposal_id`, `query`, `stratum`, and
`status: "unreviewed_query_proposal"`; the guard refuses candidates explicitly
marked reviewed or searched. This status is an authored claim, not proof that
review or search has not already occurred. Preserve a separately controlled
proposal freeze and reviewer custody record for that ordering claim.

The historical five-product bare-symbol diagnostic records the native top-10 rank unit
per product: Quanta and Semble return chunks, while Sourcegraph, OpenGrok and
cs return distinct files. Its common evidence uses separate metric names for
these two units. The mechanically generated gold and unverified external
indexed universes keep the result `diagnostic_unqualified`.
The standalone lexical scorer checks frozen rows and paired report/verdict
digests; an actual-execution claim additionally requires the paired capture
replay and the external raw HTTP/process capture replay through the workflow.

Input formats:

- [Suite schema](suite.schema.json)
- Blind query packs: generated by `freeze` from the suite
- [Runner-record schema](runner.schema.json)
- [Pair spec schema](pair-spec.schema.json)
- [Exploratory pair example](examples/pair-spec.exploratory.json)

## Paired capture and replay

Replace the example's placeholder paths and digests, pin both binaries and the
Semble environment, and generate a host profile before running:

```sh
python3 -m tools.benchmark.retrieval.semble check --python /absolute/semble-venv/bin/python
python3 tools/benchmark/retrieval/run.py host-profile \
  --profile-id measurement-host --out /absolute/host-profile.json
python3 tools/benchmark/retrieval/run.py pair --spec /absolute/pair-spec.json
```

`pair` runs both systems sequentially. Keep original corpus, native output and
evidence roots disjoint. Use the registered `retrieval-diagnostic` profile from
the runbook for immutable common capture, validation and raw replay.

Required pair keys: `repo`, `manifest`, `suite`, `query_pack`, `top_k`,
`output_root`, `runner_binary`, `searchd_binary`, `searchd_expected_sha256`,
`strategies`, `host_profile`, `semble_python`, `semble_lockfile` and its SHA-256.
The JSON schema is the complete field authority; frequently used options:

| Key | Default | Meaning |
| --- | --- | --- |
| `routes` | `["lexical","semantic","hybrid"]` | Quanta routes (must be suite routes) |
| `strategies` | required for `quanta`/`pair` | e.g. `[{"name":"whole_file"},{"name":"brace_heuristic"}]` |
| `searchd_binary` | required | explicit daemon pin; unpinned capture is refused |
| `embedder` | `potion-code` | Rust runner embedder profile (`hash-dev` is an explicit diagnostic control) |
| `query_stage_observation` | `enabled` | Exact `enabled`/`disabled` server query-stage policy; optional in the spec, explicit in daemon env and protocol4/diagnostic6 config SHA |
| `experimental_hybrid_fetch_floor` | `100` | Exact string `25`/`50`/`100`; default `100`; diagnostic experiment only |
| `repo_id`/`revision_id`/`generation` | `bench-repo`/`bench-rev`/`7` | batch identity |
| `runner_name`/`run_id` | `quanta-sdk-runner`/`run` | runner identity; `runner_revision` is derived from the binary SHA-256 |
| `blinding` | `attested` | `isolated` requires the enforced Seatbelt (macOS) or Landlock (Linux) path; unsupported or unavailable backends fail closed |
| `suite_secret_root` | none | required for `isolated`; external evaluator-only root containing the suite and no runner-readable input |
| `isolation_method`/`access_block_log` | `attested-only…` | supplied for attested runs; driver-generated and proof-bound for isolated runs |
| `semble_python` | required for `pair` | pinned Semble venv interpreter |
| `semble_lockfile` | required for `pair` | external exact-environment freeze path (frozen into the stage) |
| `semble_lockfile_sha256` | required for `pair` | SHA-256 of the external freeze; installed distributions must match every pinned line with no extras, including `semble==0.6.0` |
| `semble_route` | derived from Semble profile | `lexical-only` -> `semble-lexical-only`, `lexical-file` -> `semble-lexical-file`, `semantic-only` -> `semble-semantic-only`, and `native-default`/`hybrid-no-rerank` -> `semble-hybrid`; explicit mismatches are refused |
| `semble_cache_root` | `<out>/semble-cache` | Semble + HF caches (outside checkout) |
| `query_repetitions_per_root`/`query_warmup_passes` | `1`/`1` | one driver-generated, digest-bound randomized schedule consumed by both runners; qualified speed requires warmup >= 1 and at least 1,000 warm observations per route across roots |
| `semble_model_revision` | observed | pinned HF revision (drift fails) |
| `quanta_model_dir` | none | explicit local model directory; required for a `potion-code` speed claim and counted separately from index storage |
| `repetitions` | `1` | external reps on fresh state; qualified speed requires at least 5 |
| `alternate_order` | `true` | alternate system order per rep; qualified speed rejects `false` |
| `order` | `["quanta","semble"]` | base system order |
| `baseline_route` | Semble route | Must match `semble_route`; the spec loader refuses a different baseline label |
| `scope` | `exploratory` | `exploratory` or `qualified` |
| `admission` | required for `qualified` | v2 W0-B manifest plus frozen development suite, experiment-custody manifest, license receipt, two independent annotation receipts, and adjudication receipt; the verdict revalidates both suites and their source-bound cross-suite leakage boundary |
| `host_profile` | required for `pair` | path to a generated host-profile JSON; the file is frozen, digest-bound, and matched against both host probes |
| `linux_cgroup_parent` | none | required for qualified native Linux: an explicitly delegated cgroup v2 parent, frozen by path/device/inode and rechecked with the resource owner |
| `claims` | all `false` | `{quality,speed,same_model,incremental}` |
| `receipts` | omitted | paths to contract/SDK summaries, receipts, raw JUnit/nextest JSONL, actual-runner record and Python/Rust/SDK collection inventories; all bytes are frozen and raw evidence is reparsed by the verdict |
| `timeout_secs` | `1800` | per-capture timeout |

For `qualified`, the license receipt must be JSON with exactly
`schema_version: 1`, `reviewer_id`, `decision: approved`, `repository_commit`,
`corpus_manifest_sha256`, and a nonempty `rationale`. The reviewer and corpus
identity must match the admission manifest. This verifies the recorded decision
and scope, not the legal correctness of the review.

Each annotation receipt must be JSON with exactly
`schema_version: 1`, `reviewer_id`, `suite_sha256`, and `reviews`. `reviews`
must contain one row per suite task in suite order. Each row has exactly
`task_id`, `query_sha256`, `labels`, and a nonempty `rationale`. `labels`
contains `answerable` and `gold`, plus the same optional scoring-label keys
present on that suite task: `query_intent`, `judgment_policy`, `file_judgments`,
and `declaration_judgments`. The adjudication receipt has the same shape plus
`annotation_receipt_sha256`, the ordered hashes of both annotation receipts;
its labels must equal the final suite labels. Each annotation's proposed labels
are independently validated against the pinned source. The manifest must name
three distinct reviewer IDs. Hashes and IDs establish content and claimed
custody, but cannot establish that three humans actually reviewed independently;
that remains an external qualification check.

Use a short native output path: Unix socket path limits are 103 bytes on macOS
and 107 on Linux. The composed code-search workflow reserves a short runtime
path automatically and retains the native tree in its permanent capture.
Native Windows paired SDK capture is unsupported; WSL uses the Linux path.

### Symbol preflight

`symbol_coverage_policy` defaults to `require-complete`. For diagnostic text
search over unsupported languages use explicit `allow-incomplete`; inspect
`symbol-preflight.json` and the resulting incomplete symbol coverage. Fatal
parse/time/resource failures refuse under either policy. Use the native runner's
`preflight` subcommand to obtain the census without starting a daemon.
Symbol phrase/regex/raw-string queries currently return
`LEX_PLANNER_UNSUPPORTED_FILTER_COMBO`; literal query policy is not a substitute
for native symbol keyword search.

### Qualified runs

Use `scope: qualified` only with the schema's required admission, experiment,
independent annotation/adjudication and contract/SDK proof inputs. An attested
run cannot qualify isolated-blind quality. Linux qualified capture also requires
an explicitly delegated `linux_cgroup_parent`. Missing controls refuse the
corresponding claim; inspect verdict fields rather than command exit alone.

`QUALITY_DELTA=pass` validates comparative evidence; it does not select a
product default. Before qualified capture, put the SHA-256 of a decision policy
in the admission manifest as `decision_policy_sha256`. The policy JSON declares
`schema_version: 1`, one `comparison` (`strategy`, `baseline_route`,
`candidate_route`, `primary_metric`), a positive `min_useful_delta`, a
nonnegative `min_cluster_lower_95`,
`confidence_method: paired_query_family_cluster_bootstrap_percentile_v1`, a
nonempty list of `critical_strata` (`axis`, `name`, `min_delta`), and
`resource_limits` (`max_query_p95_ms`, `max_peak_rss_bytes`,
`max_index_bytes`). No threshold has a default. After capture and replay, run
`python -m tools.benchmark.retrieval.decision --repo REPO --suite SUITE
--run-manifest RUN_MANIFEST --policy POLICY --out DECISION_JSON`. Exit 0 admits,
1 records a threshold refusal, and 2 refuses malformed or missing proof.

For a Linux performance host, select the actual CPU thermal zone and limits:

```sh
python3 tools/benchmark/retrieval/run.py host-probe
python3 tools/benchmark/retrieval/run.py host-profile \
  --profile-id linux-perf-host --linux-thermal-zone thermal_zone0 \
  --linux-max-thermal-millidegrees 80000 --linux-min-frequency-percent 90 \
  --out /absolute/host-profile.json
```

Speed inputs need one Quanta route, at least 20 tasks and five fresh roots,
alternating system order, warmup and at least 1,000 warm observations per route.
Increase `query_repetitions_per_root` and `repetitions` for more observations;
add distinct tasks to the external suite and re-freeze its pack for more queries.
Repeating a 20-task suite does not create 1,000 distinct queries.
Within each fresh root, `query_repetitions_per_root` reuses the loaded Quanta
and Semble indexes for all measured passes; it does not rebuild an index per
query. `repetitions` creates new roots and rebuilds both indexes. A later
invocation also rebuilds them: the current runner refuses a nonempty state
root, and Semble's index exists only in the worker process. For a descriptive
warm-query diagnostic on 100 tasks, set `repetitions: 1`,
`query_warmup_passes: 1` and `query_repetitions_per_root: 10` before capture;
report the single setup/index cost separately from the 1,000 measured calls.
See [qualification and scoring policy](../../../docs/adr/SEP-26-003-retrieval-evidence-custody-and-qualification.md)
for acceptance rules.

## Read outputs

`run-manifest.json`, `protocol-lock.json`, native records, phase/resource
observations and `verdict.json` live under the selected native output root.
Inspect the verdict's validity, quality, performance and conditional-claim
fields separately. `file_recall_at_10` measures coverage of all gold files;
`file_hit_rate_at_10` measures whether any gold file was found. Context/span
scores use different units. Mechanical labels and descriptive timing are
recorded diagnostics. Server stage timings may overlap; do not sum them into
query wall time.

The Quanta window counts native published units. The scored record keeps the
first hit for each source byte span, so overlapping chunks can reduce its
candidate count. Diagnostic v6 carries a `first-source-span-v1` projection for
such responses: every native unit maps to an exact scored span and its rank.
Replay rejects missing proofs, substituted spans, duplicate unit IDs and
changes to first-hit order; it does not treat the scored count as exhaustion.

Use `run.py verdict --help` for replay arguments. Immutable common captures
support `benchctl replay --family retrieval-pair --evidence-root ROOT`.
A changed source/input/binary or missing raw requires recapture, not relabeling.

## Conditional model/incremental proof


`PYTHONPATH=. uv run --frozen --extra dev python -m tools.benchmark.retrieval.conditional_proof`
produces schema 2 bundles in a new external output directory. Common arguments:
`--kind model_vectors|incremental_rows --suite SUITE --corpus CORPUS --records RECORD... --semble-lockfile LOCK --out NEW_DIRECTORY`.
The producer requires a clean retrieval source closure and builds the owner binary
through `./scripts/cargow --lane test-daemon-lane ... --locked`.

- T15 also requires `--model-dir PINNED_MODEL --reference-python PYTHON_313`.
  The separate reference environment must contain model2vec 0.9.0. All 256
  components, reversed batch order, norms and pairwise cosines are replayed
  against the pinned reference for the adversarial inputs and frozen suite queries.
- T16 requires `--plan PLAN`. Schema 1 plans have sorted `cases`, each with
  `case_id`, `fresh`, `before`, and `delta` canonical `SemanticIngestBatch` objects.
  Required cases are append, clear_surface, membership_replace, replace, tombstone.
  Fresh/before are sealed ReplaceGeneration batches; delta is sealed Delta with
  its before generation as base. Repo, revision and model contracts must agree.
  The owner exports every semantic and membership column from the sealed tables.
  Replay checks the independent fresh input oracle, mutation coverage, receipts,
  complete row sets, payload/vector changes and membership order/content digest.


A conditional bundle is required only when the corresponding claim is enabled.
It does not replace pair admission or holdout qualification.

## Query stage clock cost diagnostic


Capture identical frozen inputs with `--query-stage-observation enabled` and
`disabled`, separate fresh state roots, identical query protocols, and at least
two measured repetitions. Replay with:

```sh
PYTHONPATH=. uv run --frozen --extra dev python -m tools.benchmark.retrieval.query_timing_overhead \
  --on-record ON_RECORD --off-record OFF_RECORD \
  --on-phases ON_PHASES --off-phases OFF_PHASES \
  --on-diagnostic ON_DIAGNOSTIC --off-diagnostic OFF_DIAGNOSTIC \
  --pack PROJECTED_QUERY_PACK --out NEW_RESULT
```


The output is `diagnostic_unqualified`; inspect per-route/task deltas and sample
coverage. Capture the two modes with identical frozen inputs and fresh state roots.

## Edit-loop checks

```sh
just retrieval-contract-local
just retrieval-contract-proof /absolute/fresh-contract-proof
just retrieval-sdk-proof /absolute/fresh-sdk-proof
```

The local command works during edits. Proof commands require clean source and
bind exact selected/executed inventories. [Cross-language fixtures](fixtures/README.md)
provide the evaluator/runner test inputs. Design decisions and historical
implementation records are indexed in [ADRs](../../../docs/adr/README.md);
open acceptance work is in the [execution ledger](../../../docs/plans/sep-27-misc/tickets/INDEX.md).
