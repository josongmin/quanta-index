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
unchanged. Fresh Quanta/Semble 99-query diagnostics under this new suite are
recorded in B04. Archived scores retain their old provenance.

## Full Gin OSA1 operation generation (2026-10-03)

**VERIFIED, generation only:** clean `quanta-index@7fc77bc1` generated paired
diagnostic suites from the frozen 1,196-task Gin exact suite and
`gin@d3ffc998` (`code_only`, 99 files, universe digest
`d4e1ea025c067f344af640568b5bfd0835ed09c2bfbcf9668b8e9782bc68bd67`).
The first attempt on clean `9d38b69d` exposed a bounded oracle bug: the
validator registered both the intended exact names and the submitted typo
queries in its 2,000-query word index, although the typo collision check uses
the separate exhaustive folded-token index. Commit `7fc77bc1` limits that
word index to intended names; the focused paired-generation, query-limit and
Ruff checks passed.

| Lane | Admitted | Ineligible out of 1,196 |
| --- | ---: | ---: |
| Insertion | 1,192 | 4 |
| Deletion | 1,178 | 18 |
| Substitution | 1,192 | 4 |
| Transposition | 1,192 | 4 |
| Keyboard stress | 1,192 | 4 |
| Boundary stress | 1,056 | 140 |

The four core operations contain 4,754 admitted task rows from 1,196 query
families; they are not 4,754 independent samples. The census also marks 118
exact-name overcorrection candidates as unjudged and 1,192 two-substitution
probes as unjudged/unscored. The generated artifacts are under
`/private/tmp/qi-gin-full-osa1-7fc77bc1-20261003` with manifest SHA-256
`3bd6f31c09e4c6248b481fc9bf63dc63a8c9706ef7c6427d79a7089ee2091b8c`.
A second fresh generation under the adjacent `-repeat-20261003` root produced
38 byte-identical files, including the manifest. All 37 manifest artifact
digests were independently recomputed. No archived 363-task score is combined
with these suites.

The independent reporter rederived all four admitted operation censuses from
the frozen Gin bytes (insertion 2.467s, deletion 2.228s, substitution 2.204s,
transposition 3.162s). Their admitted rows contain 23, 56, 27 and 24 cases,
respectively, with another declaration name within OSA1 distance. These 130
rows retain the intended original-name files and all near-name files as
separate source facts; they require intent review before navigation relevance
claims. The reporter's projected-clean-suite binding was repaired in
`72436e5e` and checked against the real generated manifest; 36 report tests
passed.

## Full Gin OSA1 Quanta diagnostic execution (2026-10-03)

**VERIFIED, diagnostic only:** clean `quanta-index@7fc77bc1` release binaries
ran the paired clean suite and all four core OSA1 operation suites against
`gin@d3ffc998`. All 5 native records and evaluator replays completed; the
operation reporter verified each generated census and paired family binding.
The fixed reporter at `0f41f71f` also accepts equivalent suite JSON
serialization only after verifying the original generated artifact digest;
37 focused reporter tests passed. The source, binary, corpus, input and report
bindings and per-lane times are in
`/private/tmp/qi-gin-full-product-7fc-20261003/gin-full-summary.json`
(SHA-256 `90289ab3c096c0c2607082e4063292196ad390c28af5f1da0b5c7b166bd9c890`).

| Lane | File Hit@10 | Query-call sum | Query-call p50 / p95 | Runner wall |
| --- | ---: | ---: | ---: | ---: |
| Paired clean | 1,187 / 1,196 | 4.299s | 3.255 / 5.837ms | 11.484s |
| Insertion | 1,181 / 1,192 | 4.826s | 3.695 / 6.546ms | 10.330s |
| Deletion | 1,161 / 1,178 | 4.456s | 3.438 / 5.838ms | 10.869s |
| Substitution | 1,174 / 1,192 | 4.382s | 3.333 / 5.694ms | 8.773s |
| Transposition | 1,180 / 1,192 | 6.524s | 4.616 / 10.502ms | 10.533s |

The call boundary is request construction through normalized response. The
host was contended, each lane was run once, and the ~17-minute release build
is separate from query and runner time. These times do not qualify a speed
comparison. The four operation rows share 1,196 original-name families and
are not independent samples. The report status is `diagnostic_unqualified`;
human relevance review, five-product matching and a fresh holdout remain
**NOT_RUN** for this Gin operation set.

## Full Gin ordinary-input typo projections (2026-10-03)

**VERIFIED, diagnostic generation:** `main@5121711c` added four paired
`default-typo-{insertion,deletion,substitution,transposition}` suites. Each
has exactly the corresponding explicit OSA1 suite's typo query, intended-name
gold, family ID and source partition; only the request contract changes to
`default_file_search` and routes become Quanta `lexical` plus Semble
`lexical-file`. The census references its original operation rows instead of
duplicating them, and the reporter rejects a changed source/count or wrong
default request contract. The four suite sizes are 1,192/1,178/1,192/1,192.

The generator produced 45 bound artifacts under
`/private/tmp/qi-gin-full-default-5121711c-20261003` (manifest SHA-256
`341f3e6c6b840c0dde37bea0b7c93fdfe369d2ceab334602b6129f93d4684e99`).
A second fresh output root produced 46 byte-identical files including the
manifest; all artifact hashes, four suite/census source replays and the
query/gold equality to the explicit suites passed. The affected source-oracle
and reporter tests passed 72/72, followed by Ruff check/format and
`git diff --check`. This extends the diagnostic inputs, not the qualified
five-product comparison.

## Full Gin ordinary-input fallback diagnostic (2026-10-03)

**VERIFIED, diagnostic only:** the clean `12fe7d9f` source was built with
`--release --all-features`. The paired runner completed all five frozen Gin
ordinary-input suites and every verdict reported `PAIR_VALID=pass` with no
execution failures. An external checker independently joined suite file gold
`(path, file_sha256)` to both native distinct-file records, checked all task
IDs, hashes and report scores, and verified that all preexisting Quanta
nonempty results retained their status and complete top-ten candidates.
The checker output is
`/private/tmp/qi-default-auto-12fe-20261003/gin-default-summary.json`
(SHA-256 `74c8b9fd0c551beab556683a98b3edb2a97e232fbd9dbc83672cc7d6410b70e2`).

| Lane | Quanta file Hit@10 | Semble file Hit@10 | Paired runner wall |
| --- | ---: | ---: | ---: |
| Clean | 1,187 / 1,196 | 1,190 / 1,196 | 380.307s |
| Insertion | 1,176 / 1,192 | 1,036 / 1,192 | 362.509s |
| Deletion | 1,158 / 1,178 | 1,004 / 1,178 | 532.454s |
| Substitution | 1,174 / 1,192 | 1,034 / 1,192 | 782.490s |
| Transposition | 1,171 / 1,192 | 967 / 1,192 | 756.744s |

The five runner walls sum to 2,814.504 seconds and include repeated setup and
indexing under concurrent builds. They are not comparable query-latency
measurements. The four typo lanes share the original 1,196 query families;
their rows are not independent samples. This mechanical intended-declaration
file gold is not independently reviewed general-file relevance.

The 84 Quanta misses across five lanes were inspected in the native rows:
9 clean, 16 insertion, 20 deletion, 18 substitution and 21 transposition.
Among the 75 typo-lane misses, 36 had a preexisting literal result, so the
empty-result fallback was not entered; all such results were identical to the
older ordinary-input records. The other 39 arose after the new fallback and
returned ten files with `capped` status. A separate audit records every query,
gold path, top-ten path and status in
`/private/tmp/qi-default-auto-12fe-20261003/gin-miss-audit.json`
(SHA-256 `c445ca2246549258b3c20bad9fe0f3d8130efc14aae4089d1e633e36fd1986e9`).
An earlier explicit-OSA1 source sometimes hit misses with preexisting literal
results; that is feasibility evidence from a different binary, not a causal
effect estimate for merging candidate sets. Native ranks beyond ten remain
unobserved.

A separate source-checked Gin absence suite contained 99 casefold-substring
absent queries. The new default route abstained on all 99 without error;
the receipt is
`/private/tmp/qi-default-auto-12fe-20261003/gin-absence-receipt.json`
(SHA-256 `45b200ac6f8e9abc87841d6f324776ba65b184de137539a37a106eb27534a262`).
Five-product matching, human relevance review, fresh holdout and qualified
performance remain **NOT_RUN** for this diagnostic.
