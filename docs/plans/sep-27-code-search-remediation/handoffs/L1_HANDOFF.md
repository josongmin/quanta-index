# L1_HANDOFF — query/domain/window

Latest audit: **VERIFIED** for two additional native exact-all corrections.
Primitive admission remains **FAILED**, so the full L1 remediation is incomplete.
The earlier statement that no further safe L1 work existed was too broad: this
audit repaired two owned-port defects without changing a shared public API.
See `L1_CODE_AUDIT.md` and `L1_CODE_AUDIT.json` for the current audit results.

The user explicitly accepts a successful execution even when concurrent sources
change. Those observed passes are accepted; source changes remain provenance and
do not trigger further reruns. `L1_GOAL_BLOCKED_AUDIT.json` and `L1_PROOF.json`
record earlier work and are historical, not an exhaustive audit or a prerequisite
to accepting the new regressions. The former vendor compilation blocker was
repaired externally. This remains owner-local evidence; repository and actual
daemon/SDK gates have not been executed for this audit.

## Latest adversarial code audit

- Exact-all Text search now applies the canonical Symbol-name predicate rewrite.
  Eight indexed/manual, count and domain combinations previously refused valid
  queries; the fixed fixture-authored five-Symbol result is now retained.
- Indexed exact-all repo projection now uses the existing repo grouping collector.
  It retains one representative for each source repo instead of truncating the
  entire generation to one result. Manual, empty and ordinary-query controls are
  included.
- New regressions: **2 passed / 0 failed**. Expanded native suite: **87 passed /
  1 failed**; the remaining failure is the existing Keyword/Content primitive
  admission regression. Latest full L1 target: **13 passed / 1 failed**.
- `rustfmt --check` and scoped `git diff --check`: **VERIFIED** for the two audit
  files. Full lint and repository/SDK gates: **NOT_RUN** in this audit.
- Both new defects are native port contract/parity defects. Current structural
  routing uses a separate Symbol-name path and rejects repo projection filters;
  no new daemon/SDK failure is claimed.

## Source and ownership

- HEAD: `98601a66d8cab9c86232b3e62ce490c8b43b71b6`, shared dirty checkout.
  Exact commands, source closures, dependency/config/toolchain identity, dirty
  inventory, executable hashes and compressed/raw log digests:
  `L1_CODE_AUDIT.json` for this audit; `L1_PROOF.json` for earlier work.
- No L1 commit/push/reset/extra agent. User prohibited cross-task messages and
  inspection. Integration requests are files only; no task messaging continues.
- Previously leased shared production files:
  core `domains/lexical/{outbound,query_plan,service}.rs` and contract-base
  `results/query_window.rs`. Their public contracts remain frozen for L0.
  A new primitive API has not been applied or given a default-success fallback.
- Rust commands run sequentially through `./scripts/cargow --locked -j 2` and its
  canonical `resource_admission.py` lock. No lock/cache bypass.

## Behavior and contract impact

| Requirement | Implemented behavior / remaining failure | Evidence boundary |
| --- | --- | --- |
| 1: Domain/decoder | Immutable validated plan rejects Text projections/types on Symbol and preserves generic Text select/type Symbol | Native indexed/manual fixtures, core plan tests, instrumented dispatcher |
| 2: Validate before empty | Domain/count/Symbol text/predicate/phrase/explicit-regex checks run before emptiness; **native Keyword/Content and dispatcher phrase gaps remain** | Two failing regressions; no complete validation claim |
| 3: Unsupported Symbol regex | Projection cannot reroute an unsupported Symbol request to Text execution | Native indexed/manual and native/Sourcegraph dispatcher controls |
| 4: Execution facts | Valid language contradiction has LogicalEmpty proof with no executed lane | Instrumented ports plus strict proof decoder negatives |
| 5: Count/window | Exact count is authoritative and separate from capped rows; invalid/missing facts refuse; cursor binds source identity | Independent cardinality fixtures, cap/top-k and byte clipping, complete cursor walks |
| Exact-all | Bounded count refuses with `LEX_FILTER_INVALID_COUNT`; absent/all preserve full set within budget | Real native ports; structural caller with instrumented ports |

`L1_REQUIREMENT_AUDIT.json` maps every requested item to terminal individual tests.
Passing rows are limited to those exercised cases. They do not erase the failed
primitive-admission requirement or establish installed product behavior.

Concrete repairs:

- Symbol cursor query lowering clears the token only from the nonpaged query
  representation. The original token still follows authentication and
  query/options/route/pin/constraints/order/cap validation.
- Native preflight preserves registry `LEX_PREDICATE_UNIMPLEMENTED` and regex
  `LEX_REGEX_DIALECT_*` through existing mappers. Earlier native RED reproduced
  a regex parse error incorrectly mapped to `INVALID_REQUEST`.
- Manual matching shares a borrowed `ManualDocumentView`; selected-row preview
  retains document addresses and request-owned output retention. Identity
  ranking/grouping precedes rendering; L3 remains the resource-budget owner.
- The stale fixed page-test byte budget now derives from an independently returned
  one-row continued response, public CBOR size and fixed observation reserve.
  It separately proves the full two-row response exceeds that budget. Production
  budget policy is unchanged.
- L2 coverage gating uses the pinned read view before predicate result emptiness.
  L1 adds no separate coverage, witness, tokenizer, schema or IR authority.

## Files

L1 production owners:

- `crates/quanta-index-lexical/src/searcher/{prepare,port,manual_scan,planner_errors}.rs`
- `crates/quanta-index-search-plane/src/query_dispatcher/{planning,window,continuation}.rs`
- `crates/quanta-index-search-plane/src/query_dispatcher/routes/lexical.rs`

Tests and leased fixtures:

- Native and dispatcher `l1_query_domain_window.rs`.
- Dispatcher `tests/support/lexical.rs` and `tests/lexical_pages.rs`.
- Lexical targets: `tantivy_smoke`, `execution_budget`, `ranked_pages`,
  `planner_authority`, `explain_candidate`, `regex_literal_alternation`,
  `regex_cache_bounds`, `unicode_normalization_goldens`, `cancellation_inside_search`.
- Lexical `tests/support/{source_fixture,op_fixture}.rs`.

The shared dirty inventory includes other owners' changes and is not an L1 diff.

## Fixtures and real callers

The nine migrated lexical targets execute 74 tests. Synthetic source-file units
carry raw SHA, exact chunk/name slice checks, unit-set hash, explicit Symbol
coverage/count and a real source-event payload hash before production writes.
These tests do not prove parser completeness. The adapter bypasses IPC digest
recomputation; its 64-zero transport token is not public transport evidence.

Smoke controls check sealed admission/replay, reject an independent Chunk clear
without creating a target, and apply a canonical file tombstone while retaining
another file's Symbols and the old immutable generation. Unicode goldens pin
format 7 / nine fields and reject format 6. See `L1_FIXTURE_MIGRATION.json`.

`routes/structural/lexical_leaves.rs::LexicalSubexprEvaluator::evaluate` is the
only production exact-all caller found by the repeated Rust census. Both Text
and Symbol preserve parent count. Six instrumented structural-route cases cover
Text/Symbol × absent/all/1. See `L1_ALL_RESULTS_CALLERS.json`; real native plus
structural producer plus daemon/SDK assembly remains **NOT_RUN**.

## Earlier verification (historical)

| Rail | Terminal result | Source-bound status |
| --- | --- | --- |
| `native-all-resume-3` | 85 passed / 1 failed; exit 101 | FAILED |
| `dispatcher-all-resume-2` | 52 passed / 1 failed; exit 101 | FAILED |
| `core-controls-resume-1` | 16 passed / 0 failed; exit 0 | VERIFIED |
| `base-window-resume-2` | 3 passed / 0 failed; exit 0 | VERIFIED |
| rustfmt + scoped diff check (23 files) | exit 0 / 0 | VERIFIED (format/whitespace only) |

The lexical combined command runs all ten selected targets with `--no-fail-fast`;
the expected native failure does not prevent the 74 controls executing. Dispatcher
combines the L1, lexical, lexical_pages and structural selections. Core/base
receipts are separate executions. Commands use JSON compiler artifacts; exact
argument arrays and before/after/current source manifests are in `L1_PROOF.json`.

The collector binds path dependencies and follows Cargo.lock to include reachable
local registry patches. It captures patch manifests/build scripts/src while
excluding unrelated vendor documentation/provenance from query behavior input.
Older conservative snapshots remain historical; they are not silently promoted.

Latest package clippy attempts failed on external-owner diagnostics: Symbol
wildcard matching and ingest/readiness test lints. Raw failed receipts remain
archived. Subsequent source changes mean complete final-source lint is **NOT_RUN**.
No suppression or ignored failure was promoted to success.

`L1_PRE_G0.*`, initial RED, zero-test build failures and source-drift runs remain
historical evidence. Initial RED receipt SHA-256:
`632fb206d4599048dc37f0d2b4c04dafa476d7dd6bcab6de2ed123b59ca426a9`.

## Remaining integration and exclusions

1. **FAILED:** Tokenless Keyword and Content filters after an empty repo-content
   predicate incorrectly return `[]` in indexed/manual search/search_all. The
   paired nonempty controls return `LEX_TEXT_QUERY_NO_TOKENS`.
2. **FAILED:** Text `lang:python "!!!"` plus typed Rust returns LogicalEmpty
   instead of `LEX_TEXT_QUERY_NO_TOKENS`. L0 owns the shared primitive-admission
   boundary. Exact proposed signature, eight implementations and L1 call sites
   are in `L1_PRIMITIVE_ADMISSION_REQUEST.md`. Both native and dispatcher must
   consume one canonical pure owner; metadata/coverage remain view-dependent.
3. **NOT_RUN:** Final-source full lint, clean integration/repository gates and
   assembled daemon/SDK error mapping, malformed stored Symbol/public transport,
   producer parser publication and witness lifetime proof. L0 must freeze the
   integrated source and run the relevant gates. No ranking quality, memory,
   performance, release or deployment claim.
