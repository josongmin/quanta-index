# Sep 30 benchmark trust — ticket index

[Parent plan](../README.md). Rows were `PLANNED`; each ticket now carries its 2026-09-30 execution receipt
(B01–B03, B05 diagnostic; B04, B06 partial; B07, B08 `NOT_RUN`). A 2026-10-01 v2
rerun from clean `quanta-index@f318e832` added the `keyword_file`/`substring_file`
arms, all seven Semble robustness pairs and the ARB adapter arm (88/88):
[qi-s30-v2-f318e832/RESULTS.md](/Users/songmin/Documents/code-new/qi-s30-v2-f318e832/RESULTS.md). v1 results:
[qi-s30-bench-trust-20260930-0d21914e/RESULTS.md](/Users/songmin/Documents/code-new/qi-s30-bench-trust-20260930-0d21914e/RESULTS.md).
The [Sep 27 benchmark RFCs](../../sep-27-code-search-remediation/readme.md)
retain ownership. These tickets describe the concrete data and execution work;
do not count the two ledgers as separate implementations.

2026-10-01 follow-up at clean `quanta-index@5d345a52`: Quanta scored
`keyword_file` and Semble scored `lexical-file` each completed all 1,196 exact
queries against the 99-file gin universe. Their independent source-file hits
were 1,192 and 1,190, respectively. The [archived raw evidence and audit](/Users/songmin/Documents/code-new/qi-s30-file-modes-20261001-ExtGj6hb/README.md)
remain diagnostic; B02 human qrels, B04 external indexed universes and a
matched five-product cohort, B07 controlled performance, and B08 fresh
multi-repository holdout remain open.
The subsequent [robustness file-mode run](/Users/songmin/Documents/code-new/qi-s30-robust-filemodes-20261001-xh1F3Xoh/README.md)
also completed all six Semble lanes and the five Quanta `keyword_file` lanes
supported by its query grammar; it remains a source-exposed native-mode
diagnostic with no fresh external capture.

| Order | Ticket | Existing authority | Prerequisite | Deliverable |
| --- | --- | --- | --- | --- |
| P0 | [S30-B01](S30-B01-exact-baseline-and-input-admission.md) | CS-BENCH-01/02 | Pinned gin checkout, suite and original capture | 1,196-task source/pack/row audit and immutable run admission |
| P0 | [S30-B02](S30-B02-semble-gin-20-qrels.md) | CS-BENCH-01/03 | B01 source universe | Reviewed semantic/architecture/symbol qrels and explicit review grade |
| P0 | [S30-B03](S30-B03-identifier-robustness.md) | CS-BENCH-01/03 | B01 and frozen sampling protocol | Reproducible variants, exhaustive supported gold and ambiguity/no-answer labels |
| P1 | [S30-B04](S30-B04-five-product-capture.md) | CS-BENCH-02/04 | B01 for exact; B02/B03 for their lanes | Native-bound five-product captures and indexed-universe evidence |
| P1 | [S30-B05](S30-B05-scoring-statistics-and-report.md) | CS-BENCH-03 | B02/B03 judgments, B04 raw evidence | Independent per-lane scorecard, exclusions and uncertainty |
| P1 | [S30-B06](S30-B06-arb-gin-workflow.md) | CS-BENCH-01/03 | ARB release and B04 adapter contract | ARB gin 88 positive cases on base-commit snapshots |
| P1 | [S30-B07](S30-B07-performance-and-indexing.md) | CS-BENCH-04, MISC-06 | B04 correctness, quiet host | Equal-boundary query and separately bounded index/build timing |
| P2 | [S30-B08](S30-B08-fresh-multirepo-holdout.md) | CS-BENCH-01/03/04, CS-INT-01 | Frozen new repositories and policy | Unseen cross-repository holdout and qualified product decision |

Parallel preparation: B02 and B03 may run independently after B01; B06 release
validation can start while B04 captures exact gin. B05 depends on admitted raw
evidence. B07 must not share the timing host with concurrent builds/indexers.
B08 is a later generalization boundary, not a prerequisite for diagnostic
results from B01–B06.
