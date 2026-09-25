# Code-search comparison baseline — 2026-09-25

Status: `candidate corpus frozen / evaluation gold absent / no new paired result`.

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

1. **Candidate/diagnostic (current):** 10 repo manifests and license-source inventories only. License approval, reviewed queries, independent gold, admitted suite, model/lockfile/host freeze, and a new pair are absent. `PAIR_VALID`, `QUALITY_DELTA` and `PERF_QUALIFIED` for this set are `NOT_RUN`.
2. **Five-language local comparison:** the five non-overlap repositories above can form one holdout per language after predeclared query strata and independent adjudicated spans. The five overlapping repositories exercise parity, not independent generalization. Freeze the query/corpus matrix before capture; report confidence intervals and per-stratum failures without a post-hoc win threshold. For a broader five-language generalization claim, add at least one more independent non-overlap repository per language before capture.
3. **Semble-coverage claim:** require independently sourced holdout coverage of all 19 upstream languages and all three query categories, with language-specific file/grammar/fallback coverage and the same path/SHA proof. Until then, call any result a **five-language subset** at most; do not infer 19-language superiority from it.

The existing W0-B admission, T00–T17 checks, and [test plan](TEST-PLAN.md) still govern qualification. This candidate freeze does not alter the historical seven-file diagnostic verdict or issue new receipts.
