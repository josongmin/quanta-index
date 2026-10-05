# Retrieval benchmark usage

Run from the repository root with frozen external inputs and fresh output roots.
[Code-search runbook](../CODE_SEARCH_RUNBOOK.md) owns live matrix commands;
[pair schema](pair-spec.schema.json), [suite schema](suite.schema.json) and
[runner schema](runner.schema.json) own input fields.

Permanent scoring, custody and qualification rules live in
[SEP-26-003](../../../docs/adr/SEP-26-003-retrieval-evidence-custody-and-qualification.md),
[review/unit contracts](../../../docs/adr/OCT-05-001-review-admission-and-result-identity.md#review-completion-and-diagnostic-units)
and [response verification](../../../docs/adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md#completed-response-verification).
Open work is in [OCT-04](../../../docs/plans/oct-4-parallel-closure/tickets/INDEX.md),
[B01–B09](../../../docs/plans/sep-30-code-search-benchmark-trust/tickets/INDEX.md)
and [MISC](../../../docs/plans/sep-27-misc/tickets/INDEX.md). This guide declares no
current capture, quality, speed or default-policy result.

## Native Quanta capture and recorded evaluation

Use a clean corpus checkout pinned at the suite's `repository_commit`. Keep
suite, query pack, manifest, state and output outside it. Existing output,
nonempty state, dirty/wrong-HEAD source and changed input are refused. `freeze`
blinds an authored suite; it does not generate labels.

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

| Option | Selection / limit |
| --- | --- |
| `--embedder hash-dev` | Development diagnostic only. |
| `--embedder potion-code` | Pinned local V1 assets, effective 512-token encoder. |
| `--embedder potion-code-full-v2` | Fresh vector generation; uncapped tokenizer with 16 KiB/text, 4 MiB/1,024-text batch admission. Exploratory, no quality/speed/same-model claim. |
| `symbol_total_timeout_ms` in spec | Forwards `--symbol-total-timeout-ms`; default 120,000 ms for complete preflight/source validation. Replay binds overrides; per-file and producer refusals remain. |
| `symbol_coverage_policy` | Default `require-complete`; explicit `allow-incomplete` is diagnostic text coverage. Parse/time/resource failures refuse under either policy. Inspect `symbol-preflight.json`; native `preflight` starts no daemon. |

Symbol phrase/regex/raw-string refuses with `LEX_PLANNER_UNSUPPORTED_FILTER_COMBO`.
A literal policy does not turn it into symbol keyword search.

## Native scale capacity profile

`scale_matrix` keeps the fixed small/medium/large/xlarge shapes (16/256/4,096/32,768
files). Its default `scale-supported-v1` profile uses a 600-second client timeout,
two retained generations, 1 GiB per repo/revision pair, 2 GiB total history,
100,000 ingest records, 128 MiB source/text, 256 MiB vectors, a 512 MiB staged
body limit and an explicit 4 GiB process memory ceiling.
These inputs appear in the artifact and config digest; explicit timeout/history
overrides remain separate diagnostic inputs. The ordinary harness unit fixture
keeps its 16 MiB retention limit. A profile or limit change never requalifies an
older result.

```sh
./scripts/cargow --lane bench-scale-lane build --release \
  -p quanta-index-searchd-harness --bin scale_matrix --locked
# Run the matching binary from a clean source checkout, with fresh external roots.
/absolute/matching/scale_matrix --tier large --out-dir /absolute/new-large-root
/absolute/matching/scale_matrix --tier xlarge --out-dir /absolute/new-xlarge-root
```

SDK and harness publications above 64 MiB use 1 MiB IPC parts followed by one
commit. Parts are stored under the private state-root `source-publication-uploads`
directory. Admission permits at most eight staged bodies and 1 GiB of staged
bytes. The default daemon body limit is 128 MiB; the scale profile explicitly
sets it to the absolute 512 MiB transport maximum. Completed uploads are removed, explicit
discard is idempotent, and later stage admissions reclaim bodies untouched for
24 hours. A producer
retry sends the same parts and original publication: matching durable prefixes
are accepted, changed bytes and gaps are refused. Uploading reserves no source
event and changes no generation. Commit verifies the complete length/hash,
CBOR allocation bounds and original binding, then uses the existing publication
journal and paired activation contract.

The ordinary 128 MiB IPC decoded-request limit remains in force. Commit reads
the bounded complete batch DTO from disk; this transport does not provide a
constant-memory decoder for arbitrarily large individual publications.
Searchd's ordinary ingest text default remains 64 MiB. A standalone daemon
using this scale capacity must set `QUANTA_INDEX_INGEST_MAX_TEXT_BYTES=134217728`,
`QUANTA_INDEX_SOURCE_PUBLICATION_MAX_BYTES=536870912` and
`QUANTA_INDEX_PROCESS_MEMORY_CEILING_BYTES=4294967296`,
configure the matching history bounds and use an explicit client timeout.
The process memory ceiling validates the declared resident policies; it is not
an OS RSS limit. The hash embedder and same-process scale rail are diagnostic; they do not prove
model quality, quiet-host latency, multi-owner concurrency or OS-process crash
recovery.

## Select the query policy and result unit

Each row is a separate intent/profile; use separate suites/output roots. The
independent planner binds the effective request and current versioned profile.
Files, declarations and chunks have separate gold and denominators.

| `query_input_policy` | Input / request | Native unit and ordering |
| --- | --- | --- |
| `native` | Admitted Native DSL; unit-changing `select:`/`type:path`/`type:repo` refused | Published chunks; quoted `"select:file"` remains content text |
| `exact_symbol_name` | Bare ASCII names; `--routes symbol`; `symbol.local_name.exact(name) case:yes` | New records: published symbol ID/indexed declaration span, `rank_unit: symbol`, `symbol-unit-v1` |
| `literal_file` | Quoted/escaped content phrase under `select:file` | Distinct files, `path_order_constant_score`; observed prefix, not relevance order |
| `keyword_file` | One bare ASCII identifier, <=256 bytes, not AND/OR/NOT; `select:file case:yes` | Scored content/path files, `score_desc_path_tiebreak` |
| `substring_file` | 3–256 byte fragment, no single quote/control; case-sensitive raw content substring | Distinct files, `path_order_constant_score`; capped prefix is alphabetical |
| `code_search_file` | 1–32 bare ASCII identifier atoms, <=256 bytes each, public `.code_search(raw)`; other ASCII controls refused | Folded content/path scored files; binds `syntax: code_search`, not Native |
| `natural_language_file` | Pinned `natural_language` token-OR under Native `select:file` | Scored files with representative published chunk witness; reviewed `semantic_intent`/`natural_language_file_search`, diagnostic |
| `code_search_typo_file` | One ASCII identifier, 3–64 bytes; public `typo:<identifier>` | Content-identifier folded OSA1 scored files; separate from default bare search |
| `code_search_components_file` | 2–32 canonical lowercase ASCII components; `components:"word word"` | Adjacent components in one indexed local symbol name; scored files; incomplete coverage refusal remains an execution failure |

Use the current profile's exact identity; do not rewrite historical record units,
ordering, scores or projection. `code_search_exact_content_file` also remains a
separate diagnostic profile. New scored file records retain finite SDK scores;
Semble `lexical-file` retains BM25 native first-file order/ties and collection
counts, while `lexical-only` returns ten chunks. These profiles cannot share a
score or latency denominator. Historical symbol records without explicit unit
are ineligible for declaration-rank metrics.

Reissue reviewed natural-language tasks with a new suite identity:

```sh
python -m tools.benchmark.retrieval.holdout_review \
  --repo /absolute/frozen-checkout --suite /absolute/original-suite.json \
  --suite-id new-nl-file-diagnostic --output /absolute/new-external-root
```

The reissue validates the entire original suite; selected query/label/review
identities stay fixed and the changed request gets new source/input lineage.
AI provenance stays AI, and old split/review receipts keep their old commitment.
After capture, evaluate one source-bound route:

```sh
uv run --frozen --extra dev python tools/benchmark/retrieval/evaluator.py evaluate-diagnostic \
  --repo /absolute/corpus --suite /absolute/suite.json \
  --runner /absolute/record.json --output /absolute/diagnostic.json
```

Read eligible/excluded tasks, execution coverage, conditional and operational
means separately. This report is `diagnostic_unqualified`, outside the paired
quality gate. `complete_ranked_pool_v1` requires explicit grade 0–3 for every
ranked top-10 unit; missing judgments exclude, rather than imply irrelevant.
Set `answerability_min_grade: 2` in both context and task for sufficient-answer
rubrics. Thresholds, missing-pool behavior and reviewed receipt shapes are
[ADR-owned](../../../docs/adr/OCT-05-001-review-admission-and-result-identity.md#review-completion-and-diagnostic-units).

## Review preparation and repository scheduling

Use existing [review owner](holdout_review.py) functions; none creates actual
review execution or human provenance:

| Function | Required input / result |
| --- | --- |
| `capture_review_pool(checkout, suite_path, record_path, pool_id=...)` | Validated single-route file capture; preserves abstentions, paths/hashes and source/pack binding; no chunk collapse |
| `prepare` / `write` | Frozen diverse pools and context; both outputs remain unjudged |
| `finalize_file_review_labels` | Two completed forms, slot-1 adjudication with third identity, `natural_language_file_search`; validates full frozen labels/threshold and emits file witnesses, not declaration/context spans |
| `bind_supplemental_review_tasks(checkout, suite_bytes, tasks)` | Bind before actual request/schema preflight; refuses judged/duplicate pairs, source/threshold drift and supplied decisions; subset cannot issue overall answerability |

Changed labels require a new suite validated by `evaluator.validate_suite`,
new blind commitment and capture. Never rebind an old record to new qrels.
Adjudicator overrides retain ambiguity. Keep actual request/result custody with
the owner; no one-off finalizer may silently drop the answerability threshold.
`execution_batch.iter_repository_admissions` drains ready/failed cells with
upstream liveness, poll/deadline and known failures. Consumers validate bindings;
aggregate failures remain visible and an unresolved earlier repo cannot stall
later ready ones.

## Source oracles and identifier robustness

Set `query_intent: bare_symbol`, `judgment_policy: source_oracle_complete_v1`
and exactly the matching judgment kind. `label_review` and qualified annotation
receipts cannot turn these mechanical labels into reviewed relevance.

| Oracle | Independent source scope |
| --- | --- |
| `go_exact_local_name_v3` (`symbol`/`distinct_file`) | Exact case-sensitive Go functions/methods/types/aliases/direct named-interface methods; indexed declaration spans, not local-name token spans; anonymous-interface methods excluded |
| `ascii_identifier_word_v1` (`distinct_file`) | ASCII identifier words anywhere in frozen bytes, including tests/comments |
| `<language>_exact_local_name_v1`, `<language>_declaration_name_{prefix,infix,components,osa1}_v1` | Rust/Python/TypeScript/JavaScript declaration census; named items/functions/classes/methods/interfaces/aliases/enums, no variables/fields/namespaces/anonymous expressions |
| Go prefix/infix | `go_declaration_name_{prefix,infix}_v1`, case-sensitive, >=3 characters |
| Go typo | `go_declaration_name_osa1_casefold_v1`, ASCII folded OSA distance 1, folded exact-name collisions excluded; historical `osa1_v1` stays case-sensitive |
| Go components | `go_declaration_name_components_v1`, contiguous lowercase `camel-snake-v1` run; non-ASCII names have no components |

Build one suite per lane; exposed baseline names produce exposed diagnostics:

```sh
python3 tools/benchmark/retrieval/identifier_robustness_suite.py \
  --repo /absolute/clean/repository --baseline-suite /absolute/baseline.json \
  --output-root /absolute/fresh-output --seed 20260930
```

`--language` requires exact independent per-file name/start-byte census parity
(CPython `ast`, pinned `syn`, TypeScript compiler or `go/ast`). Refusal/disagreement
means unsupported, never empty gold. TypeScript/TSX use the producer's vendored
grammar; Darwin/Linux requires `cc` and source/platform-keyed external cache
(`QUANTA_CENSUS_PARSER_CACHE`). Bind actual C/header/factory bytes plus exact
`pyproject.toml`/`uv.lock` parser/tokenizer pins. Changed loaded grammar or source
requires a fresh producer/capsule; immutable old identities are not rewritten.

```sh
uv run --frozen --extra dev python -m tools.benchmark.retrieval.declaration_census_audit \
  --release /absolute/release --repository NAME --language rust \
  --output /absolute/new-external-root/census-audit.json
```

`holdout_sampling.py` freezes label-free quotas/inclusion probabilities/underfill,
schema-v2 single-split recipes and a corpus-wide manifest. `corpus_binding.capture_gold`
validates development/holdout releases, assignments and exact/near copies before
labels. NL lanes stay underfilled until reviewed qrels exist.

| Negative lane | What absence proves |
| --- | --- |
| `no-answer` | Declaration absence only |
| `no-answer-content-v2` | `ascii_content_absent_casefold_v1`, all frozen content under UTF-8 replacement/casefold; default search additionally needs path absence |
| `typo-content-absence` | Casefold bytes absent from all content **and paths**; default hard negative, separate from declaration recovery |
| `typo-osa1-absence` | `ascii_identifier_osa1_absent_casefold_v1`, no exact/one-edit ASCII content identifier for explicit `typo:`; no path-absence claim |

```sh
uv run --frozen --extra dev python -m tools.benchmark.retrieval.identifier_osa1_absence_suite \
  --repo /absolute/corpus --source-suite /absolute/no-answer-content-suite.json \
  --output-root /absolute/new-external-root
```

The derived OSA1-absence suite has lexical only and binds suite/pack/census/
manifest. Older declaration-only NOC oracles retain their original diagnostic
scope. Reports expose `evaluation_intent`, `negative_reference_scope` and neutral
`no_answer.nonempty_results`; a file-hit label is not whole-file relevance or
declaration localization. `literal_relation` and `surviving_components` describe
frozen name/query bytes under the oracle tokenizer, not the engine's cause.
Join a validated Quanta/Semble file report without rescoring:

```sh
uv run --frozen --extra dev python -m tools.benchmark.retrieval.identifier_robustness_report \
  --repo /absolute/corpus --suite /absolute/suite.json \
  --record /absolute/record.json --diagnostic /absolute/diagnostic.json \
  --census /absolute/census.json \
  --generation-manifest /absolute/manifest.json --lane prefix \
  --output /absolute/new-external-root/report.json
```

The exact generation manifest binds census/lane bytes. Report unique/ambiguous
Hit@10, unsupported forms, status, no-answer and admission populations separately.
Sourcegraph/cs/OpenGrok use another capture contract. Create single-route suites:

```sh
uv run --frozen --extra dev python -m tools.benchmark.retrieval.source_oracle_suite \
  --repo /absolute/clean/corpus \
  --baseline-suite /absolute/frozen-suite.json \
  --output-root /absolute/new-oracle-output
```

The generator emits identifier-word/file, Go declaration/file and Go symbol
suites/packs; it recomputes answerability and representative first-match lines.
Admission caps: 4,096 files, 2,000 queries, 512 MiB source bytes, checked before
oversized materialization; these are not RSS bounds. Tool snapshots/output
manifest bind new inputs; old runner commitments cannot score a changed pack.

## External annotation and snippet inputs

Download the exact [CodeSearchNet CSV revision](https://github.com/github/CodeSearchNet/blob/106e827405c968597da938f6b373d30183918869/resources/annotationStore.csv)
outside the checkout:

```sh
uv run --frozen --extra dev python -m tools.benchmark.retrieval.codesearchnet_qrels \
  --csv /absolute/external/annotationStore.csv \
  --output /absolute/existing-external-directory/new-review-seed.json
```

The intake checks the pinned digest and preserves repeated/fractional grades,
notes and unknowns. It refuses normalization collisions, mutable URLs and
existing/checkout-local outputs. It is a review seed, not an executable suite.
Source/licensing/scoring admission remains [B09 work](../../../docs/plans/sep-30-code-search-benchmark-trust/tickets/S30-B09-external-robustness-adoption.md).
Materialize without dropping unavailable-source tasks or inventing snippets:

```sh
uv run --frozen --extra dev python -m tools.benchmark.retrieval.codesearchnet_materialize \
  --csv /absolute/pinned/annotationStore.csv \
  --output-root /absolute/new-materialization-root
```

`clarc_adapter.py` admits pinned CLARC original/neutral pairs;
`external_snippet_benchmark.prepare_external_lanes()` writes separate synthetic
corpora/schema-3 packs by variant/language. Scorer gold stays in its bound
sidecar; distinct paths/modes alone do not prove enforced isolation. Use actual
manifest populations, submissions, source-blocked and profile-refused counts;
old diagnostic totals are archived.

Keep Python driver source, runner build source and daemon build source separate
with both executable hashes. `--nl-max-tokens` is NL-only, 1–64 (default 32);
nondefault budgets remain exploratory and digest-bound. An admitted source over
1 MiB needs explicit recorded `--max-file-bytes`, not source exclusion.
Use `natural_language_file`/Semble `lexical-file` for lexical diagnostics only.
Synthetic snippet files are not full upstream files. Full native record replay
precedes scoring. CLARC positive targets support Hit/MRR, not fabricated exhaustive
NDCG/precision; CSN retains fractional/pool-estimated versus official judged-only
metrics. Zero-positive pools do not prove no answer. Execution/source/conditional
quality denominators, licensing and missing upstream universe remain separate.

## Recorded external rescoring and query-pool guard

Use original immutable suite/pack/capture/raw rows; do not rewrite native rank:

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

The three-product oracle checks capture/row/raw digests, query/path/status/hit
bindings and complete source-derived file judgments. A representative first
line cannot replace the full file judgment set. Digests alone do not replay
native parsing or prove external indexed-universe equivalence.
For original paired `native-tree.zip`:

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

The pair report/verdict/archive and original rank/source/hit flags are checked.
Native ten-chunk first-file collapse and external ten-file units keep separate
NDCG columns/denominators. Archived Go v1/v2 gold stays with its tool snapshots;
v2 local-name-token symbol scores were invalid, and new Go oracles require v3.
Before review/search, compare proposals with every previously exposed pool:

```sh
uv run --frozen --extra dev python -m tools.benchmark.retrieval.query_pool_guard \
  --repo /absolute/clean/corpus \
  --reference-suite /absolute/previous-suite.json \
  --reference-proposals /absolute/previous-proposals.jsonl \
  --candidate-proposals /absolute/new-proposals.jsonl \
  --output /absolute/new-pool-check.json
```

Repeat references as needed. Candidates require `proposal_id`, `query`, `stratum`,
`status: unreviewed_query_proposal`; conflicts/malformed inputs exit 2, with at
most 10,000 conflict rows and no overwrite. Authored status does not prove
unseen custody; retain a separately controlled proposal freeze/review sequence.

## Paired capture and replay

Replace [example](examples/pair-spec.exploratory.json) paths/digests, pin binaries
and exact Semble environment, generate host profile, then capture sequentially:

```sh
python3 -m tools.benchmark.retrieval.semble check --python /absolute/semble-venv/bin/python
python3 tools/benchmark/retrieval/run.py host-profile \
  --profile-id measurement-host --out /absolute/host-profile.json
python3 tools/benchmark/retrieval/run.py pair --spec /absolute/pair-spec.json
```

Keep corpus/native/evidence roots disjoint. Use registered `retrieval-diagnostic`
for immutable common capture, validation and raw replay. Required keys:
`repo`, `manifest`, `suite`, `query_pack`, `top_k`, `output_root`, `runner_binary`,
`searchd_binary`, `searchd_expected_sha256`, `strategies`, `host_profile`,
`semble_python`, `semble_lockfile`, `semble_lockfile_sha256`.
The schema owns all fields; common options:

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
| `admission` | required for `qualified` | v2 within-repository or v3 repository-disjoint manifest, with the applicable frozen source, license and independent annotation/adjudication receipts; the verdict revalidates source and label custody |
| `host_profile` | required for `pair` | path to a generated host-profile JSON; the file is frozen, digest-bound, and matched against both host probes |
| `linux_cgroup_parent` | none | required for qualified native Linux: an explicitly delegated cgroup v2 parent, frozen by path/device/inode and rechecked with the resource owner |
| `claims` | all `false` | `{quality,speed,same_model,incremental}` |
| `receipts` | omitted | paths to contract/SDK summaries, receipts, raw JUnit/nextest JSONL, actual-runner record and Python/Rust/SDK collection inventories; all bytes are frozen and raw evidence is reparsed by the verdict |
| `timeout_secs` | `1800` | per-capture timeout |

Direct `quanta` defaults to zero warmups/one measured traversal. Explicit warmup/
repetition controls bind actual phase receipts; direct capture has one root,
while `pair.repetitions` creates fresh roots/indexes. A later invocation rebuilds;
retained captures are not live indexes. For 100-task descriptive warm timing,
choose one root, one warmup and ten measured passes before capture; this gives
1,000 calls, not 1,000 distinct queries.

Qualified scope requires complete admission/experiment/independent labels,
proof inputs and actual isolation/host controls. Attested quality is not isolated
quality; Linux also requires explicitly delegated `linux_cgroup_parent`. Use
[ADR receipt fields](../../../docs/adr/OCT-05-001-review-admission-and-result-identity.md#qualified-review-receipt-shapes).
`code_search_file` pairs exactly lexical with Semble `lexical-file`; qualified
file scope needs v3 repository-disjoint admission, complete source-bound reviewed
file labels and scored order. Other file-intent profiles remain diagnostic.
A verdict does not select defaults. Freeze `decision_policy_sha256` before capture;
execute `python -m tools.benchmark.retrieval.decision --repo REPO --suite SUITE
--run-manifest RUN_MANIFEST --policy POLICY --out DECISION_JSON` afterwards.
Exit 0 admits, 1 refuses thresholds, 2 refuses malformed/missing proof.
[Statistics/decision owner](../../../docs/plans/sep-27-code-search-remediation/rfcs/CS-BENCH-03-tracks-metrics-and-statistics.md)
and `decision.py` own closed policy fields and inference; single-repository
policy does not establish a multi-repository default. C5 context-policy v2/
scored-file-policy v3 replay without a decision is `replayed_no_default_decision`.
For Linux host controls, choose the actual thermal zone/limits:

```sh
python3 tools/benchmark/retrieval/run.py host-probe
python3 tools/benchmark/retrieval/run.py host-profile \
  --profile-id linux-perf-host --linux-thermal-zone thermal_zone0 \
  --linux-max-thermal-millidegrees 80000 --linux-min-frequency-percent 90 \
  --out /absolute/host-profile.json
```

Qualified speed needs one Quanta route, >=20 tasks, >=5 fresh roots, alternating
order, warmup and >=1,000 warm observations/route plus all admitted host inputs.
Short socket runtime paths are automatic in the composed workflow (103 bytes
macOS, 107 Linux); native Windows pairs are unsupported, WSL uses Linux.

## Read outputs and diagnostic clocks

Inspect `run-manifest.json`, `protocol-lock.json`, native records, phase/resource
observations and `verdict.json` under the native root. Validity/quality/speed/
conditional claims are separate. File recall is all-gold-file coverage; file
hit-rate is any-gold-file recovery. Chunk/span/context metrics keep their units.
`first-source-span-v1` records every published native unit's scored span/rank;
replay refuses missing/substituted/reordered proofs, and dedup count is not
exhaustion. Server stage clocks may overlap; do not sum them into wall time.
Every timed cold/warmup/measured output binds the
[completed-response contract](../../../docs/adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md#completed-response-verification).

Use `run.py verdict --help` or common `benchctl replay --family retrieval-pair
--evidence-root ROOT`. Changed source/input/binary or missing raw requires fresh
capture. Capture identical enabled/disabled query-stage observations with separate
roots/protocol and >=2 measured repetitions, then replay:

```sh
PYTHONPATH=. uv run --frozen --extra dev python -m tools.benchmark.retrieval.query_timing_overhead \
  --on-record ON_RECORD --off-record OFF_RECORD \
  --on-phases ON_PHASES --off-phases OFF_PHASES \
  --on-diagnostic ON_DIAGNOSTIC --off-diagnostic OFF_DIAGNOSTIC \
  --pack PROJECTED_QUERY_PACK --out NEW_RESULT
```

This `diagnostic_unqualified` v2 measures plane/response trace observation;
backend clocks run in both modes. It does not establish total instrumentation
or IPC cost. Historical v1 retains its scope.

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

## Edit-loop checks

```sh
just retrieval-contract-local
just retrieval-contract-proof /absolute/fresh-contract-proof
just retrieval-sdk-proof /absolute/fresh-sdk-proof
```

Local checks work during edits. Formal proof binds clean source and actual
selected/executed inventories; [cross-language fixtures](fixtures/README.md)
supply evaluator/runner inputs. Current commands and runtime inputs remain here;
older implementation/results are recoverable through
[history](../../../docs/ARCHIVE-INDEX.md#oct-05-repository-wide-history-cleanup).
