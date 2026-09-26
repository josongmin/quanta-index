# L1_HANDOFF — query/domain/window

State: **partial**. Current post-fix native/dispatcher proof: **NOT_RUN**.
This working handoff is not a completion receipt. L0 owns integrated SDK/daemon,
shared schema cutover, repository qualification and final source freeze.

## Ownership and source

- Baseline inspection began at `66cee47efdda7c5f3886ac58690aa645f44f691f`.
  Shared HEAD subsequently advanced to `106d7abec2dd3fa03f9db5a19a3de41df2f0afad`.
- Shared checkout is dirty across L0–L5 and unrelated benchmark/control-plane
  work. No commit, push, reset or extra agent was created by L1.
- L0 explicitly leased L1 core `lexical/{outbound,query_plan,service}.rs`, base
  `results/query_window.rs`, affected lexical test doubles, and scoped test
  registrations/assertions. Other files retain their original owner.
- `L1_PRE_G0.md` and its source JSON are historical investigation evidence.
  Their initial BLOCKED and NOT_RUN states do not describe subsequent G0 work.

## Implemented boundaries, awaiting integration verification

| Requirement | Implementation | Current proof |
| --- | --- | --- |
| Typed Symbol cannot become Text | Core immutable `ValidatedLexicalPlan` binds endpoint/domain/decoder; adapter preparation consumes it | Behavioral RED reproduced; GREEN NOT_RUN |
| Empty cannot hide invalid count/domain/Symbol text | Pure plan admission precedes language and predicate emptiness; count zero rejects; L0 adds planner preflight before predicate resolution | Behavioral RED reproduced; GREEN NOT_RUN |
| Generic Text Symbol selection | Separate `SymbolAsText` plan kind; nested Symbol authority need does not change Text result domain | Baseline native positive control passed; current NOT_RUN |
| Count facts independent of capped rows | Existing page carrier generalized to Symbol; mandatory constrained Symbol page port; count before truncation and after cursor | Baseline row walk passed; new facts assertions NOT_RUN |
| Truthful window and continuation | Generic window rejects missing producer facts for capped fetch; verifies total against complete fetched prefix before clipping | New negative tests NOT_RUN |
| Logical empty has no execution | Canonical logical-empty proof/provenance and false lane execution; decoder refuses contradictory/missing facts | L0 owner gate passed before later core changes; final snapshot NOT_RUN |
| Cursor binding preserved | Existing context/pin/query/options/route/order/cap checks retained; reject any cursor on new logical-empty plan | New replay/clipping/full-walk tests NOT_RUN |
| Exact-all contract | Bounded count is typed refusal; unbounded/count:all preserve whole-set semantics under execution budget | New native tests NOT_RUN; public structural impact NOT_RUN |

## Behavioral RED receipt

Receipt: `/tmp/quanta-l1-red.MDEdmS/receipt.json`.
SHA-256: `632fb206d4599048dc37f0d2b4c04dafa476d7dd6bcab6de2ed123b59ca426a9`.

- `./scripts/cargow test --locked -j 2 -p quanta-index-lexical --test l1_query_domain_window`
  exited 101: 5 executed, 2 positive controls passed, 3 intended failures.
  Observed Symbol decoder INTERNAL on Text projection, regex rerouting through
  Text projection, and count-zero hidden by empty predicate.
- `./scripts/cargow test --locked -j 2 -p quanta-index-search-plane --lib l1_query_domain_window`
  exited 101: 3 executed, 3 intended failures. Domain/count rejection was hidden
  by contradictory language; logical empty reported an executed lane.
- Initial native RED run was invalidated by a concurrent sealed-generation
  edit. The repeated native run used identical before/after source manifests.
  Dispatcher capture excludes one newly authored, then-unregistered ledger
  module from compiled closure; the receipt explicitly records that exclusion.
- These are dirty-source owner receipts, not exact-commit or installed product
  qualification. The test files have since expanded; the old counts remain the
  historical executed counts only.

## Regression scope now authored

- Native Text-only, Symbol-only and mixed snapshots; hit/zero-hit, case modes,
  indexed/manual, endpoint/type/select conflicts and unsupported Symbol regex.
- count zero against present/absent repo predicate; generic Text Symbol control.
- Independent fixture cardinalities 0/1/3/5/6, count absent/1/3/5/all,
  fetch 1/3/5/8, boundaries and full walks against fixture IDs.
- Exact-all bounded refusal plus unbounded/all positive controls.
- Tokenless phrase rejection before present/absent repo predicates; native fetch
  zero/overflow rejection before zero-count fabrication; SymbolAsText explanation
  retains the Symbol domain on predicate-empty plans.
- Native/Sourcegraph dispatcher conflict admission; language contradiction,
  unsupported Symbol text, no backend invocation, cursor replay, byte clipping
  and full dispatcher cursor walks, including equal paths/IDs across source repos.
- Core plan and base logical-proof construction/decoder negative tests; window
  malformed count facts refused before clipping.

## Active integration dependencies

1. L2 strict Symbol coverage helper: L1 authored the private
   `prepare.rs::validate_symbol_coverage_for_plan` consumer over the pinned handle.
   It prepares scope regexes once and reuses canonical repo/path/language semantics;
   content/name/result filters never shrink coverage. L0 connected the compile hook before predicate planning. The L1 native fixture
   now uses mandatory file coverage and source-event hashes; native activation
   remains NOT_RUN. L0 permits the existing validated language-intersection
   LogicalEmpty case to return without acquiring a view. Every potentially
   nonempty scope requires coverage, including zero result/predicate-empty.
2. L4 selected-row witnesses: L0 owns identity/selected candidate conversion;
   L1 manual path now retains exact DocAddress, ranks/groups identity-only rows,
   then renders selected rows with one request-scoped ledger. No candidate-ID
   relookup and no preview rendering for discarded rows. Returning the output
   memory reservations beyond the local context remains an L0 carrier dependency.
3. L3 resource guards: its `RankedRows` owns collector reservations; grouping
   was generalized to `RankedRowView` for retained manual document addresses.
4. L0 source-aware schema/cursor migration, malformed stored Symbol regression,
   shared export/DTO registrations and public SDK/daemon error mapping.

## Verification still required

- Freeze compiled input closure after shared hooks land; capture source,
  toolchain/config/dependencies, exact commands, binaries and raw logs.
- Execute native L1 integration and dispatcher L1/window tests, then relevant
  existing planner/ranked-page/lexical/cursor controls. Do not widen to repository
  qualification without L0 scheduling and a new frozen source boundary.
- Final handoff must replace this working state with terminal executed counts,
  source/artifact digests, explicit included/excluded scope and residuals.

Latest authored test inventory: native L1 9 tests; dispatcher L1 9 tests, plus
2 window tests; core plan 6 tests, service config 2 tests, base logical proof 3 tests.
These are source counts, not executed test counts. Core/base L1 files are frozen
for L0's integration gate; lexical and dispatcher integration remains mutable.

Source identity migration: candidates now have mandatory actual `source_repo_id`
and separate optional full source revision/preview metadata. L1 uses the source
facet directly for file-owner projection and preserves it through fixture mapping.
Cursor order tag is `score_desc_source_repo_path_line_candidate_v2`; existing
query/options/route/pin/constraints/cap binding is preserved. Exact Symbol
predicates use the canonical argument validator; top-level implicit Symbol and
explicit Symbol domain are admitted, while nested exact predicates in Text reject
the unsupported join before emptiness.
