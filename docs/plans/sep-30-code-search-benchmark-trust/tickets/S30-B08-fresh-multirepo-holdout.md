# S30-B08 — Fresh multi-repository holdout and product decision

Status: `ACTIVE_RESIDUAL`; qualified multi-repository decision `NOT_RUN`.
Parent: [benchmark plan](../README.md). Acceptance owners:
[CS-BENCH-01](../../sep-27-code-search-remediation/rfcs/CS-BENCH-01-corpus-gold-and-holdout.md),
[CS-BENCH-03](../../sep-27-code-search-remediation/rfcs/CS-BENCH-03-tracks-metrics-and-statistics.md),
[CS-BENCH-04](../../sep-27-code-search-remediation/rfcs/CS-BENCH-04-comparators-performance-and-incremental.md)
and [CS-INT-01](../../sep-27-code-search-remediation/rfcs/CS-INT-01-integration-and-qualification.md).
Completed review/admission, native capture and cost contracts live in
[OCT-05-001](../../../adr/OCT-05-001-review-admission-and-result-identity.md),
[002](../../../adr/OCT-05-002-native-capture-clock-and-index-scope.md) and
[004](../../../adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md).

## Current execution owner

The [OCT-04 residual ledger](../../oct-4-parallel-closure/tickets/INDEX.md)
owns review/admission/capture state once: E1-01/02 remaining actual judgments,
E1-03 final admissions and stale C5 suites, E1-04 name recovery, E1-05 unused
holdout, E1-06 final reporting, E2 native cells, E4 admitted measurement and I0
integration. Its source-specific reuse and unresolved populations remain binding.
Prepared forms, source splits, diagnostic captures and source release counts do
not establish admitted relevance or an unseen holdout.

The exposed Gin 1,196 names, Semble Gin 20, ARB and existing development/C3/B09
cohorts remain regression or external diagnostic inputs. A corpus/family used
for tuning cannot regain holdout status by renaming or reshuffling.

## Corpus and sampling acceptance

1. Select at least twelve additional pinned repositories and 1,200 fresh cases
   under BENCH-01's local engineering target. Target three repositories each in
   Go, Rust, Python and JS/TS, with small/medium/large admitted-file strata.
   These counts are sampling targets, not population proportions or a power
   guarantee. Prespecify selection, reserves and every rejection before queries.
2. Audit overlap against development, Semble public repositories and adopted
   external snapshots: forks/ancestry, exact blobs, copied fixtures and near-copy
   source. Public source may appear in model pretraining; the promise is unseen
   to this tuning process, not to every model.
3. Freeze upstream URL/commit/license approval, complete tracked inventory/Git
   blob hashes, language/encoding, symlink/submodule/vendor/generated policy,
   view/size exclusions and materialized identity. `corpus_set.py` candidates
   are not admitted `corpus_release.py` releases. Large inputs must satisfy the
   oracle's actual bounded admission; two large-repository scale/resource cases
   keep a separate denominator rather than inflating primary quality counts.
4. Provisional per-repository quota: 20 exact-content + 20 named-definition +
   30 identifier variants + 10 no-answer/wrong-repository + 20 NL/workflow tasks
   (480/360/120/240 across twelve repositories). Before capture, enumerate the
   eligible population and freeze actual lane counts, seed, family IDs,
   inclusion probabilities and underfill reasons. Keep base/variants together;
   never backfill after observing scores. Existing 1,000+ mechanical-family
   targets require actual eligible cases or a declared shortfall, not copies.
5. Each task binds intent, grammar, case/Unicode, path/repository scope,
   answerability, native result unit, completeness and admissible products.
   Prefix/infix gold contains every valid alternative; typo collisions/equal
   distances are explicitly ambiguous/excluded or fully labeled. Separate
   unique/ambiguous/no-candidate/natural-no-answer populations. Wrong-repository
   controls bind the negative corpus. Definition, occurrence, file and required
   context remain distinct; edited files do not enumerate all useful context.

## Independent gold and review acceptance

- Exact-content truth uses frozen-byte reference scanning and hand-authored
  edge fixtures. Definition truth uses an independent pinned parser/compiler
  census and verified byte spans. Check every admitted language/syntax, duplicate
  names, same-line declarations, UTF-8/case, long files, generated/test files and
  chunk boundaries. Unsupported parsing is not empty gold. Re-enumerate all
  mechanical alternatives independently of product hits.
- NL/workflow labels bind pre-fix source state, query provenance and a written
  rubric. Pool varied retrieval candidates plus source-derived alternatives and
  random negative controls. Two actual independent blind reviewers and an
  adjudicator retain per-pair rationale, provenance and unresolved/unjudged sets.
  Product identity/rank/score is hidden. A missing pooled judgment is not zero.
- Use the prespecified stratified source audit for mechanical gold; claimed human
  qualification requires an actual human sample and assessors. Automated review
  remains AI diagnostic evidence. Freeze label decisions and label-free packs;
  required-context delivery needs its own required-block contract and metric.

## C0–C5 completion gates

| Stage | Existing owner and required result | Refusal / completion boundary |
| --- | --- | --- |
| C0 — input design | `corpus_set.py`, `corpus_release.py`: approved roster/strata/licenses, exclusions and complete source freeze | Wrong commit, dirty source, hash/path collision or unsupported source inventory refuses. Candidates remain unadmitted. |
| C1 — split | `corpus_binding.py`, `retrieval/gold_oracle.py`: globally bound repository/family split and release identity | Wrong repo/commit, repeated family, source overlap, missing repo and stale release refuse. A single-repository custody check cannot prove a global split. |
| C2 — mechanical gold | `source_oracle.py`, `gold_oracle.py`, `identifier_robustness_suite.py`: independent source/name/span/case/ambiguity authority for every admitted lane | Missing language coverage and failed parse remain explicit. Gold cannot be derived from product hits. |
| C3 — subjective review | `holdout_review.py`, existing suite/evaluator: actual issued decisions and adjudication under final source/rubric | Incomplete roles, mismatched source/threshold, unjudged result or invented human provenance refuses. |
| C4 — blind native matrix | `corpus_binding.py`, `code_search_matrix.py` and native adapters: repository × lane × mode inventory and source/request/unit-bound outcomes | Apply the selected actual query profile; do not truncate or reuse old limits. Missing/unsupported cells remain explicit. Ten chunks are not ten distinct files. |
| C5 — product decision | `evaluator.py`, `decision.py`: replayed eligible report and frozen decision policy | A failed/omitted repository, unjudged pool, unmatched index universe or negative/insufficient effect cannot issue a product-default decision. B09 global12 cannot silently fill B08 C5 cells. |

Order: C0 → C1 → C2/C3 → C4 → C5. Reuse the existing release → binding →
suite/pack → capture → replay/scorer path. Source/labels/parser/oracle/query/split
changes produce new bound identities and recheck affected proof. A new parser
generation requires a fresh producer process. Old bytes remain immutable.

## Reporting and decision

- Before holdout access, freeze primary task/metric, no-answer metric, useful
  effect, critical-stratum regression bounds, resource ceilings, uncertainty and
  finite ablations. Choose them from baseline variance/product needs, not winners.
- Report task/family/repository macro results and repository-cluster uncertainty;
  requested/eligible/attempted/completed/unsupported/error/unjudged populations,
  index coverage and every excluded ID/reason. A single-repository interval is
  not cross-repository confidence. Preserve source/output/timing unit differences.
- Independently replay native rows and source gold. `QUALITY_DELTA=pass` admits
  evidence; only the eligible report plus frozen policy may decide a default.
  Exact conformance, relevance, context, scale, latency and incremental costs keep
  separate reports. Retire holdout independence once used to select policy.
- Public external data/evaluator versions stay pinned and separate. ARB full
  workflow/no-gold tracks, installed release, CI, activation and deployment retain
  their own acceptance; B08 does not close them.

Exact earlier C0–C5 commands, source pins, diagnostics and terminal failures are
recoverable through [the history index](../../../ARCHIVE-INDEX.md#historical-record-recovery).
This compaction issues no new labels, native run or qualification.
