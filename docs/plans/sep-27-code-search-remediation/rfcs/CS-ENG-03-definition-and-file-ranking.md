# CS-ENG-03 — Definition and distinct-file ranking

Status: exact-name/source-identity and collection-budget repairs **IMPLEMENTED**;
historical owner execution is scoped by [L3 completion](../handoffs/L3_SS_AUDIT.md).
Final combined-source execution and ranking-quality/holdout promotion: **NOT_RUN**.
Category: engine relevance. Findings: F02/F03; part of F04.
Depends on ENG-01/02; tuning requires BENCH-01/03 development contracts.

Final audit: E05/E06/E07 in [engine-audit.md](../engine-audit.md). Native grouped
file collection already exists; reuse and repair its identified boundaries.

## Current disposition and remaining acceptance

At `b42a9b5d` plus dirty overlay, explicit local/qualified-name predicates,
federated source identity and pre-materialization request accounting are present.
The merge replaces private SSTable ordinal decoding with committed format-8
ranked-key tables. It changes seal/open, resident accounting, ranking comparisons
and fixture layouts; the older L3 receipt does not qualify that replacement.

- **INT-R1, NOT_RUN:** execute ranked/grouped exact-reference tests, same-path
  federation, complete cursor walks, resource/cancellation precedence and storage
  tamper/reopen regressions on the combined source. Check migration fixtures and
  registered authority, API/module snapshots, compile and Clippy.
- **Ranking experiment, NOT_RUN:** exact-case preference/weights, development
  ablation, independent holdout and comparative relevance admission. No default
  ranking-weight change or quality improvement follows from the L3 repair.

No new L3 behavioral defect was reproduced in this audit. The original checklist
below includes experimental requirements beyond the completed L3 A/B/C scope.
See [CS-INT-01](CS-INT-01-integration-and-qualification.md#current-remaining-work-audit--2026-09-27).

## Purpose and RCA

Content matching, finding a named declaration, and returning ten distinct files
are different objectives. Keep them separate rather than replacing lexical
search globally with symbol search.

[Observed diagnostics](../evidence.md): `Default`, `generate_unique_id`, and
`anything` have gold lexical chunk ranks 14/31/34 but native symbol rank 1.
`select:file` changes those ranks to 7/3/18; case-sensitive matching plus file
selection gives 3/3/18. These are bounded counterfactuals, not a new default win.
Repeated chunks occupy 393 of 710 returned slots beyond the first chunk per file.

Other inspected examples distinguish ranker mechanisms:

- `Default`: folded term frequency favors text containing repeated lowercase
  `default`; nine of the first ten chunks lack capitalized `Default`.
- `get_typed_signature`: a 69-token reference chunk scores 6.215331 versus a
  declaration's 89-token chunk at 5.575418, both with one occurrence. This is
  consistent with length normalization, not proof of a BM25 arithmetic defect.
- `normalize_fstring_quotes`: a reference and definition tie at
  5.592399597167969; deterministic path order places the reference first.

Identifiers were not split into missing underscore/camel components in these
cases. Global BM25-statistics decomposition remains NOT_RUN. Do not diagnose a
tokenizer or path-only scorer bug from these examples.

## Decision: explicit policies over the existing engine

Keep Tantivy/trigram/predicate candidate correctness. Add code-aware ranking only
where a declared policy requires it; do not replace the retrieval substrate.

| Intent | Candidate authority | Ranking/output policy |
| --- | --- | --- |
| Content/literal | Exact verified content matches in the requested scope | Content relevance or explicitly declared deterministic order |
| File locator | Matching content/path candidates in scope | Distinct-file top-k, one file identity per slot |
| Definition locator | Source-bound complete declaration facts | Exact local/qualified name, requested case and symbol kind |

The producer's `SymbolRecord` contains local/qualified name, signature and
definition byte span. The implemented lexical adapter retains original and
normalized exact-name fields and source facts in addition to legacy broad keyword
text. Explicit exact lookup uses those fields; it does not create another producer.

`symbol.has.name(utils)` currently returns symbols located in `utils.py`, and
`symbol.has.name(DefaultPlaceholder)` also returns its methods. The current
predicate contract does not independently promise exact local-name equality, so
classify this as broad keyword authority and an exact-name capability gap, not a
proven violation of an already promised exact-name API. Name the new explicit
lookup policy and its interaction with legacy keyword search unambiguously.

Use explicit identifier fields for raw local name, qualified name and normalized
lookup where supported. Preserve original spelling. Exact case is a hard match
constraint when requested; otherwise a soft exact-case signal is an experiment,
not an undeclared reinterpretation of case-insensitive search.

Definition policy first applies repository/path/kind constraints. Exact name and
qualified-name matches can outrank references because the domain consists of
declarations, not because a benchmark name was memorized. Multiple valid
definitions stay multiple results. An exact qualified-name request cannot be
satisfied or outranked by an unqualified near-match. Prefix/fuzzy expansion requires an explicit
capability/policy; do not silently add it to exact lookup.

## Existing distinct-file collector: retain correctness, repair boundaries

`port.rs` already dispatches `select:file` to `collect_projection`; the
[GroupedPageCollector](../../../../crates/quanta-index-lexical/src/ranked_page.rs)
selects each path's best scored row per segment, merges representatives globally,
sorts deterministically, then applies cursor and limit. No fixed-100 overfetch is
used. Best-hit file scoring is the existing baseline, not a new feature to claim.

Group by source-file identity within the selected snapshot **before the returned
top-k boundary**. A file's baseline score is its best admissible hit;
overlapping chunk count is not an implicit popularity boost. Retain a bounded,
ordered set of supporting hits only when the response contract exposes it.

Preserve exact top-k over the selected ranking policy or report
bounded/partial results honestly. Fetching a fixed 100 chunks and deduplicating
does not prove the best ten distinct files. Use a collector/query plan that can
maintain group scores with valid termination bounds, or exhaust the bounded
candidate universe; measure the cost. No global content-hash dedup across paths.

### Federated identity

`ChunkRecord.source_repo_id` is supported inside one generation pin. The repaired
collector groups files by source repository and path, and candidates preserve
source identity separately from the containing pin. Same-path federation fixtures
cover distinct owners. Keep snapshot ownership and source identity separate.

The proposed contract preserves federated origin on the result and groups files
by `(source_repo, canonical_path)` within the pin, with source revision/hash bound
by ENG-02. Repository projection must explicitly name whether it selects source
repositories or containing index repositories. For source discovery, use source
identity. Do not silently reinterpret the existing `repo_id` pin field: coordinate
the breaking result/schema/cursor cutover. If source identity is unprovable,
refuse the ambiguous input/profile rather than guessing a revision.

### Enforce resource limits during collection

The repaired `collect_projection` uses the shared request work/byte ledger before
candidate/group allocation. Native traversal, harvest/merge/sort and retained
output observe the canonical interruption/resource owner. Preserve accounting
across segments and in-flight values. Logical reservations and resident estimates
are not process-wide allocator/RSS proof; the format-8 paths need INT-R1 execution.

On cap exhaustion, stop the native walk and return the existing typed budget
refusal. Do not let the internal stop appear as successful exhaustion. Partial
ranked output is not part of this fix unless separately contracted. Do not assume
Tantivy's ordinary TopDocs pruning proves grouped top-k with unseen groups.

Define stable tie-breaking explicitly. A cursor binds snapshot, group identity,
ranking policy and query. It cannot continue a chunk ranking as a file ranking.
Keep evaluator-side post-hoc collapsed diagnostics separate from this native
engine output: the existing evaluator view does not prove native file top-k.

## Ownership and experiments

Start with [symbol query](../../../../crates/quanta-index-lexical/src/symbol.rs),
[compile](../../../../crates/quanta-index-lexical/src/searcher/compile.rs),
[candidate collection](../../../../crates/quanta-index-lexical/src/searcher/candidates.rs),
[paging](../../../../crates/quanta-index-lexical/src/searcher/paging.rs) and
[response contract](../../../../crates/quanta-index-contract/src/results/query_responses.rs).
ENG-01 owns shared plan validity; ENG-04 owns preview bytes, not rank policy.

Predeclare a finite development ablation: current chunk and existing native-file baselines;
exact-name fields; optional exact-case preference; definition-specific policy.
Within those policies, identifier-boundary, proximity, kind and scope features
are candidates for a bounded ablation, not assumed improvements. Inspect term
frequency, document frequency, field length and existing score components first.
Measure combinations only as declared and retain per-query wins/losses/ties,
candidate counts, group cardinality, duplicate-slot ratio and score components.
Tracing is sampled/bounded and disabled or equally enabled for timing comparisons.

BENCH-01 repairs labels before tuning. Freeze chosen policy, weights, budgets and
schema before opening the fresh holdout. Failed holdout admission preserves the
current default; a diagnostic gain may remain an explicit experimental profile.

## Tests and DoD

- [ ] Independent fixtures cover declaration versus reference, exact/lowercase,
  qualified-name ambiguity, overloads and same name in different repositories.
- [ ] Grouped top-k agrees with an exhaustive small-index reference, including
  more than 100 leading chunks from one file and ties across many files.
- [ ] Same-path chunks from different source repos, repo-filtered queries and
  source-repo projection preserve identity and do not collapse distinct files.
- [ ] Tiny examined/group-byte caps halt before unbounded accumulation, including
  multi-segment merges; an instrumented work counter and allocation accounting
  prove the bound. No OOM or peak-RSS result is claimed from source review alone.
- [ ] Case, path/repo filters, missing symbols and zero matches preserve semantics.
- [ ] Reindex/insertion order leaves the same snapshot ranking deterministic;
  renamed files follow the declared path/tie policy without stale identities.
- [ ] Queries/matches crossing chunk boundaries, short literals without a usable
  trigram and long identifiers agree with the advertised matching contract; new
  rank/group collection cannot discard valid fallback or boundary candidates.
- [ ] Pagination has no duplicates/omissions relative to the declared snapshot;
  changed policy/query/source cursors reject rather than silently restart.
- [ ] Repeated overlapping chunks cannot multiply a file's relevance by accident.
- [ ] Development ablation and independent holdout report quality and resource
  tradeoffs under BENCH-03/04; no default change without admission.
- [ ] Public result-unit/ranking semantics and affected ADRs are updated together.

Rejected: global symbol routing, special cases for the three failed names,
unbounded candidate overfetch, and learned reranking before a valid baseline.
Code-aware signals have production precedent in [S01/S03](../references.md);
their Quanta weights and benefits require measurement.
