# Sep 30 benchmark trust — ticket index

## Current state (2026-10-04)

Current audit baseline is main `e43cda8c` plus the shared overlay. B07 implements
diagnostic 9 / protocol lock 7 / phase 4, completed-response validation, SDK RPC
attribution and detailed ingest clocks. Two frozen release SDK proofs passed
25/25 each. Its 20-capture scanner A/B passed independent output/work-counter
equivalence but showed mixed whole-request timing; quiet-host performance and
new release scale runs remain open. See [B07](S30-B07-performance-and-indexing.md)
for source-bound evidence and exact residuals. B08 diagnostic work is separate
from its qualified decision; follow its owner ticket rather than the historical
September execution labels below. Existing collectors, scorers and registries
remain the implementation owners.

The current B08 owner inventory records an actual supplemental-review failure:
the external batch request builder omitted `answerability_min_grade`. Its
preserved bat stderr and follow-on terminal confirm failure before new label
issuance. Threshold propagation and actual-request preflight are repaired below;
unresolved adjudication, supplemental qrels/admission and final all-five scoring
remain. Per-repository terminal failures are now isolated from other ready cells
in the repaired controller. This is external orchestration and
unfinished label coverage, not evidence of a Quanta search defect. See
[B08's latest inventory](S30-B08-fresh-multirepo-holdout.md); its original captures
and failed review namespace remain immutable.

### Supplemental execution defect repair (2026-10-04)

At main `8ee2f1ea` plus the owned overlay, the canonical review adapter now
binds supplemental task thresholds from the fully validated frozen suite.
Explicit conflicting thresholds, changed queries/source, duplicate or already
judged pairs and supplied decisions are refused. The existing execution-batch
module now drains ready and failed repository admissions before polling pending
ones; known repository review failures are terminal without blocking siblings.
Both are connected to fresh copies of the external review/capture controllers.

Focused owner verification passed 111 tests (including 31 new controls).
Actual bat/cli/lo preflights constructed 14 reviewer request/model-input/schema
payloads with threshold 2 and zero model calls.
The actual external request preflight also refused threshold/query/grade/source
text mutations in four separately bound negative controls.
The actual capture-controller preflight drained seven ready repositories plus
failed typeorm and tailscale,
leaving django/sqlalchemy/zellij pending at observation. It validated ready
admission input-byte bindings and made zero product calls. Sources, commands
and outputs are under `/private/tmp/qi-bench-defect-fix-20261004-46iq_iok`;
old scripts, captured responses and labels were not overwritten.

These checks close request preparation and queue readiness defects. Actual new
review completion, unresolved adjudication, revised qrels/admission and final
five-product scoring remain `NOT_RUN` here. Canonical preparation and preflight
are not relevance decisions or benchmark qualification.

## Historical execution snapshots

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
The subsequent admission/reporting patch versioned the NOC content-absence
oracle and added a source-bound Quanta/Semble robustness breakdown; the
four scored exact misses were independently located beyond top 10 by cursor.
The versioned NOC suite's 99 admitted probes were then rerun separately for
Quanta and Semble with current evaluator replay; both remain diagnostic.
These repairs do not close human labels, external index attestation, matched
five-product semantics, controlled performance or the fresh holdout.

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
| P1 | [S30-B09](S30-B09-external-robustness-adoption.md) | B03/B05/B08 and CS-BENCH-01/03 | Pinned external source and input contracts | External qrel intake and source-defined robustness strata; product qualification remains separate |

Parallel preparation: B02 and B03 may run independently after B01; B06 release
validation can start while B04 captures exact gin. B05 depends on admitted raw
evidence. B07 must not share the timing host with concurrent builds/indexers.
B08 is a later generalization boundary, not a prerequisite for diagnostic
results from B01–B06.
