# L3_HANDOFF

State: implementation in progress; native compile/tests NOT_RUN by L3.
Snapshot claims below are not engine qualification. Source manifest is refreshed
only at a verification boundary and may lag ongoing edits.

## Ownership and shared decisions

Initial HEAD `66cee47efdda7c5f3886ac58690aa645f44f691f`; shared dirty checkout now
`106d7abec2dd3fa03f9db5a19a3de41df2f0afad`. Preserve other lanes' work.
L0 coordinator: `01a0dea8-aa8e-7d73-857f-b174f7be64fe`. Rust runs serialized by L0.
L3 did not commit, push, reset, spawn agents, or run a Rust build.

L0 expanded L3's lease from ranked_page.rs, budgeted_search.rs, paging.rs,
symbol.rs and helpers/tests to:

- core/domains/lexical/collection_budget.rs (new)
- core/domains/read_view/predicate.rs
- contract-base/results/candidates.rs
- contract-base/query/lexical_cursor.rs
- contract/results/query_responses.rs

Canonical contracts agreed with L0:

- Exact `symbol.local_name.exact(keyword)` and
  `symbol.qualified_name.exact(keyword)`: whole STRING field equality, NFC then
  existing Unicode case policy. One nonempty keyword <=4096 bytes. No broad
  keyword rewrite, prefix/fuzzy fallback, or ranking-weight change.
- Candidates retain containing repo/revision/generation, mandatory
  `source_repo_id: RepoId`, `source: Option<SourceFileRevision>` and
  `preview: Option<PreviewMetadata>`. Optional values still require wire keys.
  The source facet is stored authority even without revision/hash proof.
- Source revision, when present, must agree with candidate owner/path. Available
  preview source must agree; unavailable previews can omit source, but any
  source they carry must agree. Missing/duplicate/unknown wire fields fail.
- Ranked order: score descending; source repo, path, start line, end line,
  candidate ID ascending. Cursor includes mandatory source repo. L1 bumped
  continuation order binding to `score_desc_source_repo_path_line_candidate_v2`.
- `select:repo` means source repository; file/path group means (source repo,path).
- One shared work/byte ledger, separate from examined-candidate cap and the
  existing RequestBudget cancellation/deadline owner. Ordinary ranked/count
  traversal uses resource quotas without materialized-candidate admission;
  whole-set/grouped traversal uses both.
- L0 approved retaining native TermQuery BlockWAND and using a budgeted scorer
  for other weights. Public Tantivy 0.22.1 callbacks cannot prove the Boolean
  generic scorer stops on MAX threshold. Boolean TermUnion block skipping is
  therefore a performance tradeoff, not a preserved performance claim.

## Implemented

- Canonical core exact predicate enum/name/domain/family/accessor and argument
  policy tests. L1 owns pure request plan/inference and nested Text rejection.
- Adapter exact helper using dedicated local/qualified sensitive/folded fields;
  broad SymbolHasName returns None. L0 owns compiler/planner/schema/document
  producer integration and original spelling/signature/definition preservation.
- Source-aware strict candidate/cursor serde and shared comparator, used by
  plain, grouped and manual rank keys. L0/L1/L4 migrate external constructors.
- Existing native grouped collector now uses per-segment (repo ordinal,path
  ordinal), global (source repo,path), and the unchanged best-hit rule. No
  score sums or overfetch approximation. Manual grouping uses the same identity.
- Shared atomic checked work/resident/peak ledger with sticky typed refusal and
  noncloneable Drop reservations. Five owner tests cover concurrency, overflow,
  release and refusal. No mutex/poison fallback.
- Native visits charged before scorer initialization/advance/seek; global
  collection refusal stops all segments. Group admission precedes key/map work.
  Every representative/merge row charged. Partial fruits discarded on refusal.
- Segment-fruit, heap, row/guard buffers, conservative pinned BTreeMap nodes,
  cloned grouping keys and stable-sort scratch reserved before allocation;
  guards live across heap/segment/global/page moves and iterators.
- rows_to_candidates accepts L4 SelectedPreviewContext across final selected
  rows and compares source-aware stored identity with fast ranking columns.

## Remaining implementation boundaries

- Full ranked-key preallocation bound awaits L0 canonical maximum byte limits
  plus ingest and sealed-open enforcement. ord_to_str currently may allocate
  before decoded length is known; do not claim full key-memory closure.
- L0 must finish compiler/planner exact hook and raw symbol facts preservation.
- FileOwnerProjectionRow source facet contract is raised to L0; candidate IDs
  alone cannot join ambiguous source owners.
- Page/transport allocations and optional previews have separate owners; this
  collector ledger alone is not end-to-end resident-memory admission.
- Query-weight construction, dictionary decompression and native engine caches
  are outside the collection retained-byte ledger. Peak logical reservations
  are not RSS measurements.

## Verification state

- Scoped rustfmt and git diff --check run during editing; they are syntax/style
  diagnostics, not semantic proof. Final current-source receipt pending.
- L3 native tests authored: early candidate/work/byte stop, one/many groups,
  cross-segment admission, exact-cap tie/global merge, corrupt-key abort,
  cancellation/deadline, merge refusal, partial-harvest refusal, retained-buffer
  lifetime, empty index, generic pruning work bound, streaming count control,
  cross-source native/manual grouping and cursor walks. Exact tests cover same
  local/different qualified names, overload, nested/container-only, case/NFC,
  whole-string rejection, zero matches and broad-name control.
- Native execution: NOT_RUN by L3. L4 diagnostic shared compiles hit contract/core
  migration errors before tests; no native behavioral result is inferred.
- Original failing behavior source analysis is recorded in engine-audit E05/E06/
  E07. A before-fix runtime RED has not been executed for this lane.
- Core ledger owner snapshot: L0's earlier gate ran 5 ledger tests successfully,
  but subsequent shared contract edits mean it is not current-tree proof.
  Raw `/private/tmp/qi-l0-g0.d9edNekW/g0-final-owner.log`, SHA256
  `a30df516257bef4bca1f6c9a26f04285811728878f2b6b336a7b29627fa5e40a`;
  before/after manifests both SHA256
  `7f9e142a14ebcfd7d173f67a855aa561325cb6620bb7a9b70ff5087e93a0a605`;
  ledger source SHA256
  `01b0fa55ef873246e2e67027d74d13d50b65f561bebe67605dec021566dcc224`.
  Command: `CARGO_BUILD_JOBS=2 ./scripts/cargow --lane test-fast-lane test -p quanta-index-contract-base -p quanta-index-contract -p quanta-index-core --lib --locked`.
- Public SDK/installed daemon, complete repository qualification, quality
  holdout/ablation, performance and peak RSS: NOT_RUN.

Next native gate once L0 grants source freeze and Rust slot:
`CARGO_BUILD_JOBS=2 ./scripts/cargow --lane test-fast-lane test -p quanta-index-lexical --lib l3_ --locked`.
Then baseline budgeted_search and execution_budget/ranked_pages regressions.
Re-freeze relevant source/config/dependencies before accepting any receipt.
