# References and applicability

Reviewed for the final recommendation on 2026-09-27. These are primary public
sources inspected during the preceding research. Live documentation and main
branches are not immutable dependency pins. Implementation must freeze relevant
versions and schemas; no RFC treats a web page as executable acceptance proof.

## Established implementation precedents

| ID | Primary source | Applicable lesson | Limit |
| --- | --- | --- | --- |
| S01 | [GitHub code-search syntax](https://docs.github.com/en/search-github/github-code-search/understanding-github-code-search-syntax) | Symbol definitions, content and path constraints have explicit meanings | Not all language/symbol forms supported; does not specify Quanta weights |
| S02 | [Blackbird architecture, 2023](https://github.blog/engineering/the-technology-behind-githubs-new-code-search/) | Ngram retrieval, content-addressed incremental ingestion and commit-consistent results | GitHub-scale deployment is not a Quanta requirement |
| S03 | [Zoekt design](https://github.com/sourcegraph/zoekt/blob/main/doc/design.md#ranking) | Code-aware boundary/proximity/symbol signals; alternative BM25 scoring | Candidate signals require local ablation; no universal best ranker implied |
| S04 | [Sourcegraph navigation](https://sourcegraph.com/docs/code-navigation) and [precise navigation](https://sourcegraph.com/docs/code-navigation/precise-code-navigation) | Search/syntax-based and compiler-informed navigation have different guarantees | Syntax lookup is not precise reference resolution |
| S05 | [SCIP schema](https://github.com/scip-code/scip/blob/main/scip.proto) | Source occurrences, symbol roles, definitions and position encodings are explicit | Optional producer interchange; not a searchd compiler dependency |
| S06 | [Tree-sitter ERROR/MISSING nodes](https://tree-sitter.github.io/tree-sitter/using-parsers/queries/1-syntax.html) | Recovery and missing tokens must be observed | A recovered tree does not prove complete or correct symbol facts |
| S07 | [TREC judgments](https://trec.nist.gov/data/reljudge_eng.html) and [trec_eval](https://github.com/usnistgov/trec_eval) | Collection-bound qrels, pooling and independent standard metric implementations | Unjudged items and incomplete pools can bias ranking comparisons |
| S08 | [ripgrep guide](https://github.com/BurntSushi/ripgrep/blob/master/GUIDE.md) | Explicit match/filter/encoding options for a scan reference | Defaults can exclude hidden, ignored or binary files |
| S09 | [GitHub CLI code search](https://cli.github.com/manual/gh_search_code) | CLI search documents use of the legacy backend | It cannot be used to claim a Blackbird comparison |
| S10 | [Russ Cox: trigram-indexed regex, 2012](https://swtch.com/~rsc/regexp/regexp4.html) | Ngram candidates still require exact regex verification; short literals may give an unselective filter | Established mechanism, not new 2026 research or a Quanta proof |
| S11 | [Zoekt native API schema](https://github.com/sourcegraph/zoekt/blob/main/grpc/protos/zoekt/webserver/v1/webserver.proto) | File identity includes repository/version; match ranges, context, display caps and stable streaming progress are distinct | Maintained implementation contract, not proof that Quanta must copy all fields |

S01–S05 and S10–S11 were rechecked during the engine re-audit on 2026-09-27.
The important common pattern is explicit match/source/result authority, not a
particular learned ranker or an instruction to replace the current index.

## 2026 production updates checked in the engine re-audit

| ID | Primary source/date | Relevance to this plan |
| --- | --- | --- |
| P26-01 | [Sourcegraph Query Assist, 2026-03-30](https://sourcegraph.com/changelog/2026-03-30) | Natural-language assistance translates into an explicit search query; an optional planner layer, not a repair for invalid engine plans |
| P26-02 | [Sourcegraph Search Jobs API, 2026-07-27](https://sourcegraph.com/changelog/2026-07-27) | Asynchronous exhaustive searches and completed JSONL results are distinct from interactive top-k response limits |
| P26-03 | [Sourcegraph Code Finder revision selection, 2026-08-31](https://sourcegraph.com/changelog/2026-08-31) | Branch/tag/commit selection reinforces explicit source binding; it does not establish Quanta's ranking quality |

These are dated vendor implementation announcements. They show available product
capabilities, not independent comparative benchmarks. No claim of an exhaustive
survey or a universal "2026 SOTA" ranking follows. The directly applicable engine
choices still use established indexing, exact verification and snapshot practices.

## Research available by September 2026

| ID | Primary source | Reusable evaluation idea | Scope limitation |
| --- | --- | --- | --- |
| R01 | [CoIR, ACL2025 repository](https://github.com/coir-team/coir) | Multiple code-retrieval task types | Does not certify literal/regex engine behavior or this corpus |
| R02 | [CoREB v2, May8 2026](https://arxiv.org/html/2605.04615v2) | Time-separated releases, graded relevance, retrieval/reranking evaluation | Competitive-programming origin, generated queries, five languages; no broad industry-adoption claim |
| R03 | [CORE-Bench v3, August24 2026](https://arxiv.org/abs/2606.11864v3) | Code understanding, edit localization and broader context are separate tasks | Agentic context track; not an exact-match conformance replacement |
| R04 | [Agent Retrieval Bench v1, July27 2026](https://arxiv.org/abs/2607.24882v1) | Frozen repository states, workflow tasks, natural no-gold and wrong-repository controls | Research evidence with task-specific outcomes, not a universal winner |

The proposed minimum 12 repositories/1,200 fresh cases, specific policy choices,
parallel lanes and acceptance thresholds are Quanta engineering decisions. They
are not standardized SOTA settings. Fresh research can inform additional tracks;
core lexical correctness remains independently measurable.
