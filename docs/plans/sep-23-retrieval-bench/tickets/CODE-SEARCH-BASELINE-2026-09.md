# Code-search comparison baseline — 2026-09-25

Status: `candidate corpus frozen / 10 diagnostic pairs captured / qualified quality and speed absent`.

This is a **local Quanta vs Semble code-search** standard, not an adoption of Semble's published score. The upstream method and repository list were read at Semble `24497845460960db1839c8485319df189a889225` ([method](https://github.com/MinishLab/semble/blob/24497845460960db1839c8485319df189a889225/benchmarks/README.md), [repository pins](https://github.com/MinishLab/semble/blob/24497845460960db1839c8485319df189a889225/benchmarks/repos.json)). Its dataset covers 63 repositories and 19 languages with semantic, architecture and symbol questions. The README says 1,251 total queries, but its displayed category counts (711 + 343 + 204) sum to 1,258; do not derive stratum weights from those inconsistent counts. Its query labels are generated and checked with the same model, so neither those labels nor its reported NDCG/latency are independent local comparison evidence.

## Frozen candidate corpus set

The reproducible [corpus-set spec](/Users/songmin/Documents/code-new/qi-rb-coverage-set-2026-09-25/corpus-set-sep-2026.json) is **outside this repository** (SHA-256 `4f4a6c5515e2265c261edfab270e07f3ef1a645cef6a8e1922bc4a6482d7eeb3`). The repository retains only the generic [`corpus_set.py`](../../../../tools/benchmark/retrieval/corpus_set.py) generator and its fixture tests; the generator rejects spec, checkout or output paths inside the quanta-index checkout. Each repository remains an independent clean external Git checkout and produces its **own** external RB-00 `repository_commit` + sorted path/SHA manifest. The current pair runner accepts one repository per capture; aggregate across paired repository results, never fake one synthetic commit. File selection uses tracked UTF-8 code files under the declared root, a 1 MiB file cap, regular-file/no-symlink checks, and the explicit language extensions in the tool. Every excluded tracked path and reason is retained; test source files inside the root are included as realistic distractors, not silently removed. The Semble adapter must still prove its actual indexed path/SHA universe equals each manifest; a candidate manifest alone does not establish that equality.

Regenerate into a **fresh external** output directory only:

```sh
python3 tools/benchmark/retrieval/corpus_set.py \
  --spec /Users/songmin/Documents/code-new/qi-rb-coverage-set-2026-09-25/corpus-set-sep-2026.json \
  --checkouts /Users/songmin/Documents/code-new/qi-rb-coverage-set-2026-09-25/checkouts \
  --out /Users/songmin/Documents/code-new/qi-rb-coverage-set-2026-09-25/<fresh-output-directory>
```

| Repository | Language | Commit | Code files | Upstream benchmark overlap |
| --- | --- | --- | ---: | --- |
| ripgrep | Rust | `af60c2de9d85e7f3d81c78601669468cf02dabab` | 88 | no |
| tokio | Rust | `5db10f538b683fe88d699dfd11be31d193db011c` | 375 | yes |
| gin | Go | `d3ffc9985281dcf4d3bef604cce4e662b1a327a6` | 99 | yes |
| fastapi | Python | `c3c9dd6b1a08bcda766e7b43eafe72c4c5e9e193` | 46 | yes |
| axios | JavaScript | `c7a76ddbf277db864ee6cfb4ef17b8a08ffbe3f5` | 63 | yes |
| trpc | TypeScript | `c188dab0822caf3615199e4ac95147bc7560d26f` | 107 | yes |
| viper | Go | `528f7416c4b56a4948673984b190bf8713f0c3c4` | 33 | no |
| black | Python | `8d5a2d9f49378d7abe2eb632df1601818de8c24e` | 24 | no |
| eslint | JavaScript | `b14b8bc213ccfc19d3edcfa9ce6af62622661160` | 389 | no |
| vite | TypeScript | `bc598a6a8a6b7d6e157e9f19c16911cff8d2360c` | 256 | no |

Frozen local candidate: 10 repositories, 5 languages, 1,480 files, with one Semble-public overlap and one non-overlap repository per language. External artifact: `/Users/songmin/Documents/code-new/qi-rb-coverage-set-2026-09-25/frozen-v4/corpus-set.json`, SHA-256 `31c248d79ad908052018ee74279630b4b0bd77c5e3cd5ead31d1651b0eb71f33`; per-repository manifests are in its `manifests/` directory. Each summary includes exact excluded paths/reasons and root license-source path/SHA inventory, **not** license approval. This expands the former 88-file Rust candidate. It has a five-language non-overlap *candidate* but no reviewed queries or gold, so it is not a fresh quality proof or population-level result. The overlapping half is useful for parity diagnostics, not as the sole generalization sample.

Focused loader/chunker probe: `./scripts/cargow --lane test-daemon-lane run -q -p quanta-index-retrieval-bench --bin quanta-index-retrieval-bench --locked -- chunk --repo <checkout> --manifest <per-repo-manifest> --strategy fixed_window_strict --out <per-repo-output>` was run once for each of the ten named repositories on the current dirty source. The outputs are `/Users/songmin/Documents/code-new/qi-rb-coverage-set-2026-09-25/chunk-probes/{name}.json`: 1,480 files, 4,191 chunks, zero reported fallback chunks and zero uncovered bytes. This is input/chunk coverage only; it does not run searchd, Semble, queries, mapping proof or a paired verdict. Source-bound qualification receipts are not reissued by this probe.

Raw Semble 0.6.0 file-walker discovery on these original checkouts is **not** the admitted universe: it sees three extra shell-completion files in ripgrep and two empty Python files in fastapi. The common-universe pair must index the exact materialized manifests and recheck the adapter's path/SHA mapping; native discovery is reported separately.

## Actual Quanta–Semble diagnostic pair (2026-09-25)

All ten repositories were run, not extrapolated from the corpus inventory. The immutable input pack is outside the repository at `/Users/songmin/Documents/code-new/qi-rb-coverage-set-2026-09-25/symbol-diagnostic-inputs-04/`; its `diagnostic-inputs.json` (SHA-256 `a5e6879a8ee4d1560bc4ca1db820eeff843838771de714fc27e1ac5a7f9a4567`) binds each suite, query pack and pair spec digest. The per-repository captures and raw reports are at `/Users/songmin/Documents/code-new/qi-rb-coverage-set-2026-09-25/pairs-overlap256/<repo>/`. Each of the ten `verdict.json` files has `PAIR_VALID=pass`, and a fresh `run.py verdict` replay was byte-identical to the captured verdict. An aggregate SHA-256 over lexicographically sorted repository names, each followed by NUL, its verdict byte length as unsigned big-endian 64-bit, then its raw verdict bytes, is `1698dbe145a7feaa8ad7811b61e80e49fa2440c978126b7c4fa8164f78c08884`. The pair validator checked the admitted path/SHA universe and query-protocol binding. The same 20 query strings per repository, source bytes and `top_k=10` were supplied to both systems.

This is a **post-failure-tuned symbol diagnostic**, not the predeclared quality or performance experiment. Queries and exact line-span labels were mechanically derived from 20 unique declarations per repository, without independent annotators; only the `symbol` category was tested. The first strict-window settings failed on history retention, SDK I/O timeout and tokenless punctuation-only chunks. The successful setting is Quanta `hybrid` with `potion-code`, 1,024-byte strict windows and 256-byte overlap versus Semble 0.6.0 `hybrid` with native chunks. Both reference the same Potion Code asset revision, but embedding implementation/model parity is **not proven**. The benchmark-only Quanta daemon history ceiling was raised to 1 GiB per revision/4 GiB total and the frozen pair spec set a 600-second per-request SDK timeout. These settings were selected after observing failures. There was one fresh root, one warmup pass and one measured query per task; the host was not certified quiet. Captures used the same runner binary SHA-256 `35cc5dbfa2847d21c3a2bcc5a96e4d635ce122b54278c1b293c96b72974bded2` and searchd SHA-256 `79a2e4e8790ec28d235969a45a800eb03304d7125d3b0842ef59a15add593227`, but a dirty Quanta source and two observed Git HEADs (`6a6024b2` for eslint, `83de08a4` for the other nine); source-closure digests were absent. `QUALITY_DELTA=not_applicable`, `PERF_QUALIFIED=not_applicable`, and `CONTRACT_GREEN`/`SDK_PATH_GREEN=not_run` on all ten verdicts.

The following are raw per-repository report means, not qualified claims. `NDCG` is this evaluator's size-aware exact-span NDCG@10; `recall` is exact-span Recall@10. Warm query latency is one observation per query, not a stable p95 or throughput measurement. `index` compares Quanta embed/publish/seal/activate with Semble index phase, which are distinct pipeline boundaries. Q = Quanta, S = Semble.

| Repo | Q/S NDCG@10 | Q/S recall@10 | Q/S warm mean ms | Q/S index s |
| --- | ---: | ---: | ---: | ---: |
| ripgrep | .0048 / .0328 | .25 / .60 | 84.82 / 2.71 | 57.92 / 1.85 |
| tokio | .0285 / .0810 | .50 / .75 | 89.11 / 4.46 | 161.56 / 4.51 |
| gin | .0258 / .0776 | .60 / 1.00 | 20.73 / 8.65 | 16.85 / 1.84 |
| fastapi | .0279 / .0695 | .65 / .85 | 33.57 / 1.01 | 17.34 / .69 |
| axios | .0262 / .0826 | .85 / .85 | 12.62 / 4.65 | 3.15 / .33 |
| trpc | .0293 / .0833 | .60 / .90 | 18.08 / 4.15 | 6.11 / .33 |
| viper | .0384 / .0669 | .70 / .85 | 17.58 / 10.76 | 6.58 / .81 |
| black | .0174 / .0325 | .55 / .55 | 37.48 / 1.23 | 19.16 / .51 |
| eslint | .0064 / .0954 | .25 / 1.00 | 88.47 / 72.31 | 95.48 / 7.96 |
| vite | .0265 / .0998 | .55 / 1.00 | 54.04 / 22.68 | 89.58 / 2.21 |
| Equal-repo mean | .0231 / .0721 | .550 / .835 | 45.65 / 13.26 | — |

On this deliberately narrow mechanical task, Semble ranked the labeled span higher in all ten repository means; paired task NDCG outcomes were Quanta 25 wins, Semble 151 wins and 24 ties. Semble's observed warm-query mean was lower in all ten repositories. These observations do **not** establish general code-search quality, same-model superiority, qualified speed, architecture/semantic-query behavior, or deployment suitability. Failure modes remain visible: Quanta's no-overlap strict windows can emit tokenless chunks, and the original 16 MiB benchmark history ceiling/30-second SDK timeout failed on this larger corpus. The successful configuration avoids those failures but does not erase them.

### Route and query-form ablation (2026-09-25)

The preceding 200 questions are **named-symbol lookup tasks phrased as natural-language sentences** (`Find the function definition named X.`), not a semantic-behavior or architecture benchmark. The initial Quanta route was `hybrid`, which invokes native lexical and dense semantic lanes and fuses them by RRF. It did not separately measure either lane. Semble's worker called `SembleIndex.search(query, top_k=10)` without an explicit alpha: Semble 0.6.0's query classifier assigned all 200 sentence queries `alpha=0.5` (semantic/BM25 RRF blend), with its code reranker enabled. Thus the initial table was **hybrid-vs-hybrid on symbol intent**, not semantic-only or lexical-only quality.

We subsequently captured Quanta `lexical` and `semantic` as distinct pairs against the same Semble default-hybrid route, using the same 200 sentence queries, line-span labels, admitted bytes, `top_k=10`, strict 1,024-byte/256-overlap chunks and runner binary SHA-256 `35cc5dbfa2847d21c3a2bcc5a96e4d635ce122b54278c1b293c96b72974bded2`. Raw inputs and captures are outside the repository at `symbol-lexical-inputs-01/`, `symbol-semantic-inputs-01/` and `pairs-sentence-route-ablation/{lexical,semantic}/` under the external corpus root above. All 20 new pairs have `PAIR_VALID=pass`; each `verdict-replay.json` is byte-identical to its captured verdict. A first attempt to capture all three Quanta routes in one pair was rejected by the driver single-capture contract and is **not** included. The shared main moved during this diagnostic; the ablation manifests record HEAD `09ea7b78` and no source-closure digest. Do not merge these into qualified source-bound evidence.

To distinguish lexical retrieval from sentence-query parsing, we also issued the **same 200 selected symbol names as bare identifiers**, keeping their label spans and corpus bytes fixed. The same-binary diagnostic subset uses `pairs-bare-lexical/{ripgrep,tokio,gin,fastapi,axios}/` and `pairs-bare-lexical-frozen/{trpc,viper,black,eslint,vite}/` outside the repository. All ten pairs are valid with byte-identical verdict replays and the same Quanta runner digest as above; the first five manifests record HEAD `09ea7b78`, the last five `83de08a4`. A later re-run under `83de08a4` is retained separately but its runner binary digest changed during the shared build, so it is **excluded** from this grouped row. Semble auto-selected `alpha=0.3` for 187 of these bare identifiers and `alpha=0.5` for 13; it remained hybrid, **not BM25-only**. The bare-identifier result is a different query-form experiment and must not be pooled with sentence results.

The following are equal-repository means across ten 20-task repositories. Q/S means Quanta route / Semble default hybrid. Latency is a single measured query observation per task and is descriptive only; the distinct pair sessions were not a quiet-host or same-order speed qualification.

| Query form / Quanta route | Q/S NDCG@10 | Q/S exact-span Recall@10 | Q/S MRR@10 | Q/S warm mean ms |
| --- | ---: | ---: | ---: | ---: |
| Sentence / lexical | .0000 / .0721 | .000 / .835 | .000 / .595 | 2.72 / 4.56 |
| Sentence / semantic | .0231 / .0721 | .550 / .835 | .313 / .595 | 15.60 / 4.39 |
| Sentence / hybrid (original) | .0231 / .0721 | .550 / .835 | .313 / .595 | 45.65 / 13.26 |
| Bare identifier / lexical | .0418 / .0968 | .980 / .950 | .660 / .894 | 2.90 / 7.05 |

The sentence-form lexical route returned **zero candidates in all 200 tasks**. That does not mean the lexical index cannot retrieve symbols: bare identifiers returned the labeled span within top ten in 196/200 tasks. It identifies a query-form mismatch in this benchmark's use of the native lexical route. Sentence semantic-only and hybrid had identical per-repository Recall@10 in all ten repositories, and identical NDCG@10 in nine; eslint differed by about `0.000032`. The semantic lane therefore supplied nearly all measured hybrid relevance for these sentence tasks, while hybrid's observed mean latency was larger. This is a route ablation, not evidence for semantic-behavior search quality. Bare lexical had slightly higher Recall@10 than Semble hybrid but materially lower NDCG@10 and MRR@10; the hit was usually less well ranked or less context-efficient. The cross-system bare row is not a pure lexical-vs-lexical comparison because Semble still blends BM25 and semantic scores. For all four rows, independent gold, reviewed query categories, same-model implementation proof and qualified speed remain absent; `QUALITY_DELTA` and `PERF_QUALIFIED` are `not_applicable` on their verdicts.

### Follow-up instrumentation (source change; not a new pair)

The runner now emits a separate `retrieval-diagnostic.json` beside each new Quanta record. It is bound to the exact v3 record and projected query pack by SHA-256, and enumerates every returned `(task, route)` window with candidate identity/path/rank/score and hybrid lexical/dense rank/raw-score contributions. The pair driver rejects a missing, malformed, partial or mismatched new diagnostic and binds its file digest in the Quanta run manifest. New pairs declare `retrieval_diagnostic_version=1` in their protocol lock; replay then refuses a missing diagnostic reference. Historical captures without that declaration remain replayable. The v3 scoring record and primary metric are unchanged. This artifact exposes **returned windows only**: absence from it cannot distinguish a candidate that was never retrieved from one truncated before `top_k`. A fresh lexical/semantic/hybrid ablation at an explicitly frozen larger `top_k` is required to locate loss before versus after fusion; its results must remain diagnostic and must not be substituted for the `top_k=10` comparison.

The same diagnostic records runner-clock boot/readiness, the opaque SDK publish-and-activate call, record assembly, corpus reverification and shutdown. The SDK call still combines embedding, indexing, seal and activation; its subphases cannot be attributed by this runner alone. Server-side trace points or an explicitly instrumented SDK/server protocol would be needed before claiming a split of those costs. Existing v1 phase metrics are preserved for verdict compatibility. New protocol locks independently declare `rank_metric_k_policy=declared_top_k_v1`: reports mark `Recall@20` as `not_applicable` for `top_k=10`, rather than silently duplicating `Recall@10`, and an attempted @10 primary comparison with `top_k<10` refuses. Historical reports retain their legacy calculation **only for immutable replay**; do not interpret their capped `Recall@20` as measured recall at 20.

Next decision gate: use the bound diagnostic to classify missed labels as wrong file versus wrong span, and compare lexical, semantic and hybrid lanes on a predeclared holdout. Only if the lexical lane demonstrably loses natural-language symbol queries should a separate natural-language-to-native lexical adapter be proposed; the existing native DSL's semantics are not changed by this benchmark work. Chunker, fetch-depth and fusion changes likewise need a causal ablation first. None of these source edits requalifies the ten prior pairs or supplies independent gold, license approval, same-model proof or quiet-host speed evidence.

## Predeclared evaluation matrix

| Dimension | Primary rule | Required breakdown / refusal |
| --- | --- | --- |
| Surface | Code-only, single-repository ranked chunk retrieval; exact same query text, admitted source bytes, `top_k=10`, and context-token accounting for both systems. | Exclude docs/config/data, multi-repo routing and `find_related` from this primary claim; report them separately if tested. Reject path/SHA or indexed-file mismatch. |
| Quanta chunk route | Use one predeclared language-neutral fixed-window strategy for the five-language primary pair. | The current syntax chunker parses Rust only; Go/Python/JavaScript/TypeScript take an explicit whole-file fallback. Report its fallback rate as an ablation, not as a five-language syntax strategy. |
| Query intent | Semantic behavior, architecture/flow, named symbol lookup; keep train/eval query families disjoint. | Report category × language × repository, and each failure/no-answer task. Do not use upstream generated questions as independent gold. |
| Relevance | Independent two-annotator span and grade judgments plus adjudication **before** viewing either result. Primary metric is the existing size-aware graded NDCG@10 (`rb-rank-context-density-first-coverage`). | Span Recall@10, MRR@10, BCY@2k/4k/8k/16k, file-only recall, same-file-collapse view and genuine no-answer abstention are diagnostic. Missing gold means `QUALITY_DELTA=NOT_RUN`, not zero. |
| Coverage | One sorted tracked-file path/SHA manifest per repository; common-universe pair only after adapter mapping proof. | Separately state native indexed coverage and all excluded files/reasons. A file-count match alone is insufficient. |
| Speed | Local same-host sequential pair: time-to-searchable, warm query p50/p95/p99, errors/timeouts, RSS and index bytes. | Preserve model/config/cache identities and raw observations. Qualified warm speed needs at least 20 distinct tasks, 5 fresh roots and 1,000 valid observations per route; a contended or mismatched-host run is diagnostic only. |
| Aggregation | Pair by identical query within repository, then report per-repository/language/category deltas and uncertainty. | Never pool repeated latency observations as independent relevance samples; never mask a weak language with a micro-averaged all-file score. |

## Claim levels and next coverage boundary

1. **Candidate/diagnostic (current):** 10 repo manifests and license-source inventories plus 10 valid exploratory pairs exist. License approval, reviewed semantic/architecture queries, independent gold, a predeclared untuned quality suite, same-model proof, clean-source receipts and quiet-host repeated speed capture remain absent. The paired protocol is valid; qualified quality and speed are not applicable to this run.
2. **Five-language local comparison:** the five non-overlap repositories above can form one holdout per language after predeclared query strata and independent adjudicated spans. The five overlapping repositories exercise parity, not independent generalization. Freeze the query/corpus matrix before capture; report confidence intervals and per-stratum failures without a post-hoc win threshold. For a broader five-language generalization claim, add at least one more independent non-overlap repository per language before capture.
3. **Semble-coverage claim:** require independently sourced holdout coverage of all 19 upstream languages and all three query categories, with language-specific file/grammar/fallback coverage and the same path/SHA proof. Until then, call any result a **five-language subset** at most; do not infer 19-language superiority from it.

The existing W0-B admission, T00–T17 checks, and [test plan](TEST-PLAN.md) still govern qualification. This diagnostic pair does not alter the historical seven-file verdict or issue new qualification receipts.
