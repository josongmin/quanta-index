# S30-B03 — gin identifier robustness suite

Status: `EXECUTED_DIAGNOSTIC` (2026-09-30); see receipt below. Priority: P0. Depends on S30-B01.
Parent: [Sep 30 plan](../README.md). Contract owners:
[CS-BENCH-01](../../sep-27-code-search-remediation/rfcs/CS-BENCH-01-corpus-gold-and-holdout.md)
and [CS-BENCH-03](../../sep-27-code-search-remediation/rfcs/CS-BENCH-03-tracks-metrics-and-statistics.md).

## Sampling contract, frozen before results

Use the 1,196 exact local names as a **known-source development population**.
Preregister a deterministic 300-family random sample with its seed and the
complete 78-family multi-file stress stratum; report overlap and the observed
population proportions. Keep test/generated, short/long and camel/snake
sub-strata visible. This engineering sample size is not a statistical power
guarantee. Do not replace a low-scoring or ineligible family after seeing
product results. Any unavailable stratum is reported as a shortfall.

For each eligible base name, derive at most one deterministic query per
transformation:

| Lane | Query contract | Independent label rule |
| --- | --- | --- |
| Prefix | Proper prefix of at least three characters | All case-policy-matching declaration names with that prefix in the selected source universe |
| Infix | Internal substring of at least three characters, neither full name nor prefix | All declarations whose names contain it under declared case policy |
| Camel/snake split | Existing component boundaries only; no invented synonym | All declarations satisfying the frozen component-matching rule; rule and tokenizer version recorded |
| One-edit typo | Deterministic single insertion/deletion/substitution/transposition with recorded edit policy | If no exact-name collision exists, all names at the allowed minimum positive distance; unique, ambiguous and no-candidate cases separate |
| No-answer controls | 100 deterministic absent-declaration probes, selected before search | Exhaustive declaration census finds no valid answer; parser failure is not absence |

Select semantic rules before generation; then independently enumerate **all**
valid answers. An edited query that is itself another declaration name is an
exact-name collision, not a uniquely recoverable typo. A variant derived from
`FooBar` is not entitled to `FooBar` as its only gold when `FooBaz` also
satisfies the query. Mark collisions and
multi-file labels; report unique/ambiguous/no-answer strata separately. The
current `go_exact_local_name_v3` oracle cannot be relabeled as a prefix or
fuzzy oracle. Add a versioned independent oracle only for semantics that can
be exhaustively specified, with hand-written positive/negative Go fixtures.
No-answer applies to the declared declaration intent; a bare content search
may legitimately return files containing the same bytes and is scored in a
separate content lane.

Group original name and variants under one `query_family_id`. Hold out variants
from the runner pack and any tuning decision until the preregistered release
is frozen, but call this gin set a **source-exposed diagnostic** because all
base names were already in the public 1,196 suite. S30-B08 supplies a fresh
cross-repository holdout.

## Verification and deliverable

- Fixed seed and source produce byte-identical query proposals, family IDs,
  stratum census and gold. All proposal overlaps against previously searched
  suites are reported by [query_pool_guard](../../../../tools/benchmark/retrieval/query_pool_guard.py).
- Hand-authored fixtures cover case, short name, multiple nearest names,
  generated/test files, no-answer, parser error, split boundary and UTF-8
  refusal/semantics where applicable.
- The report compares each variant with its exact base query by family, gives
  per-transformation Recall/MRR and no-answer metrics, and shows ambiguity and
  coverage denominators. No aggregate robustness score hides a weak lane.
- Substring failure is evidence for a substring candidate/verification path;
  typo failure is assessed separately. Neither alone mandates an engine design.

Use [COBE](https://repositorio.pucrs.br/dspace/bitstream/10923/25612/2/COBE_A_Natural_Language_Code_Search_Robustness_Benchmark.pdf)
for perturbation-vs-quality methodology, [Memtrace](https://github.com/syncable-dev/memtrace-public/blob/main/benchmarks/README.md)
as a vendor method example, and [Zoekt's trigram design](https://github.com/sourcegraph/zoekt/blob/main/doc/design.md)
for the substring/typo mechanism distinction. None supplies this suite's gold.

## Execution receipt (2026-09-30)

Release v2 (post-audit): variant contracts, byte-identical build, 81 v3 multi-file families (ticket's 78 is v1), components 278 after an acronym-boundary guard. Built from pre-commit source in the shared checkout (later committed as 59249da8); pool guard fails for prefix/infix/typo. Producer: pre-commit source in the shared checkout (later committed as 59249da8) (RESULTS custody);
results, digests and residuals: [qi-s30-bench-trust-20260930-0d21914e/RESULTS.md](/Users/songmin/Documents/code-new/qi-s30-bench-trust-20260930-0d21914e/RESULTS.md).

Release v3 (pre-commit build adding the `no-answer-content` lane, 99 probes) was regenerated byte-identically from clean `f318e832` (except the evaluator digest) and used by v2: [qi-s30-v2-f318e832/RESULTS.md](/Users/songmin/Documents/code-new/qi-s30-v2-f318e832/RESULTS.md).

2026-10-01 file-mode follow-up: the same frozen prefix (352), infix (339),
components (278), typo (363), no-answer (100), and no-answer-content (99)
task/query/gold identities were rechecked against the source and run with
Semble's new `lexical-file` profile. Quanta's scored `keyword_file` ran the
supported five lanes, with the original exclusion denominator retained. The
[independent checker](/Users/songmin/Documents/code-new/qi-s30-robust-filemodes-20261001-xh1F3Xoh/audit.json)
verifies native-to-file projection and current Quanta path/status parity with
v2. This source-exposed diagnostic is not a fresh holdout.

2026-10-01 admission repair: the legacy `no-answer-content` generator checked
content absence when building its 99 probes, but replay admission checked only
the empty declaration oracle. A comment-only match could therefore be admitted
after a query/hash mutation. Newly generated NOC suites use the distinct
`ascii_content_absent_casefold_v1` source oracle and a `no-answer-content-v2`
suite ID. The evaluator rechecks all frozen file bytes under the declared
casefold rule; the old NOA declaration contract and archived NOC diagnostics
remain readable. Focused fixtures reject content-positive and case-variant
mutations and wrong units. The original 99 NOC query/gold identities are
unchanged. New product captures under the v2 suite are still required before
claiming a newly executed result; archived scores retain their old provenance.
