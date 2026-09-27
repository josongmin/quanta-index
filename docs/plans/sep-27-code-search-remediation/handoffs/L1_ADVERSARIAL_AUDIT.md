# L1 adversarial RCA follow-up

> Historical report: one-off evidence files were removed from the repository. This report alone is not current verification.

Status: **VERIFIED** for the repaired query-admission boundary. No remaining
reproduced P0–P2 is known in this exercised scope. This is not a claim that the
whole repository or every preventive RFC matrix cell is qualified.

Start HEAD: `5571132655a83824731e7909b0e310951edad52b`, concurrent dirty checkout.
The shared HEAD advanced externally to `681dc3f0c5232b4c756c9fd296d305677560370a`
during verification and later to `2102966246866398f01833bebf71396831377149`. This task did not commit, push, reset, coordinate agents,
or send task messages. Successful executions remain accepted per user direction;
revision/dirty state and source changes are retained as provenance.

## Reproduced and repaired

| Priority | Root cause | Structural repair |
| --- | --- | --- |
| P2 | Hybrid, seed, hybrid explain and semantic lexical scope bypassed the primitive-admission port before language composition. The hybrid preparations were duplicated. | One hybrid lexical planning owner serves all three hybrid entry points. Semantic scope invokes the same original-query domain/primitive admission before composing languages. |
| P2 | Pure scope preflight always selected Tantivy grammar, rejecting valid `index:no` anchors before the manual executor could see them. | Top-level scope admission selects the actual execution grammar and reuses `manual_filter_regex`; indexed predicate subplans retain their existing grammar. |

Both failures have behavioral RED logs. The composite regression checks four
routes × three invalid primitives × unconstrained/contradictory language sets
(24 requests), typed refusal, exactly one primitive admission, and no lexical
open. The native scope test checks anchored and word-boundary membership against
one fixed source row, plus malformed regex refusals in both execution modes.

Native L1 tests: **18/18 passed**. Dispatcher aggregate: **465 passed / 1 failed**;
the single failed control used `repo:^other$` on an indexed stub, a pattern the
real indexed executor rejects. Replacing it with the valid nonmatching `repo:other`
preserves the same exclusion/window/admission assertions; that test subsequently
passed **1/1**. The aggregate was not relabelled as a successful command.

The remaining direct language-composition callers were inspected: Text/explain
Text already preflight the original request (preserving the special revision
selection rule), and Symbol preflights the Symbol endpoint before composition.
Hybrid `count:` is explicitly refused by the existing filter planner; it is not
a new truncation defect. Empty dense candidate batches do not invoke native
candidate admission; the original public lexical request is now admitted first.

## Actual daemon and SDK

`l1_daemon_query_contract` runs against a separately launched real searchd over
a private fresh state root. It publishes and activates three source-bound files
and three symbols through the SDK, using current SourceFileCoverage and source
event contracts. No scripted transport peer or runtime harness substitutes for
the daemon. The daemon uses the explicit `hash-dev` test embedder.

The final run passed **1/1**, with **zero ignored tests** (`--ignored` explicitly
selects this opt-in process test). It covers Native/Sourcegraph × indexed/manual:

- Symbol projection/type conflicts with case controls preserve remote typed errors.
- `count:1` retrieves each of three distinct symbol identities exactly once,
  reports remaining counts 3/2/1, and terminates the cursor after the third page.
- Changed cursor context returns `CURSOR_CONTEXT_MISMATCH`.
- Invalid primitives reject through Text, Symbol, hybrid, seed, semantic scope
  and hybrid explain; positive hybrid rows supply valid explain input.
- Valid language contradiction is exact empty without claiming search execution.
- Manual anchored path search retains the single expected source row.

Daemon SHA-256:
`6392fef5eac7a1f8bc8d8b404ce51f0065b03d0e0228f1acf028270d1ec9bf2c`.
Later SDK test corrections reused that verified immutable binary with a new
temporary state root. Three initial SDK failures were test setup/oracle mistakes:
the cursor's specific error code, remaining rather than original page count, and
a fabricated zero-score hybrid row rejected by serialization. All failed logs
are retained; none is reported as a new production defect or a passing run.

## Lint and diagnostic correction

The first scoped Clippy run stopped at the SDK's existing source-publication
receipt error conversion: `map_err(|_| ...)` discarded its cause despite the
crate's deny policy. The conversion now includes the cause in the diagnostic,
keeping the same `SdkError::Binding` route/axis and refusal behavior. This small
diagnostic change was checked by Clippy and static checks; it is not an additional
query behavior finding. The successful daemon/SDK query proof precedes only this
SDK error-message and test documentation changes, and was not rerun solely for them.
The second lint run also caught a concurrently added native decoder borrow
error (`Value::as_u64(value)`); this task supplied the required `&value` borrow.
A separate concurrent request-probe lifetime error was already repaired when
current source was inspected, then further edits introduced another lifetime error; this task did not change that budget behavior (only a later documentation lint fix). The new SDK
test's environment-variable documentation was marked as code for doc lint.
A supplemental SDK input snapshot was captured while the lint command waited for
resource admission; the final lint snapshot includes all three selected packages.

Shared-tree lint subsequently encountered additional concurrent L2/L3 compile
and style failures, all retained in raw logs. Final scoped lint uses isolated
worktree `/Users/songmin/.codex/worktrees/l1-audit-lint/quanta-index` at
`681dc3f0c5232b4c756c9fd296d305677560370a` plus the owned SDK diagnostic/doc fixes and the lint-only repairs listed below.
The isolated baseline also contained eight pre-existing lint failures. Equivalent
Boolean/closure rewrites, checked usize-to-u64 conversions, an explicit drop
immediately after the last lock-guard use, and shorter first doc paragraphs were
applied in both checkouts. Exact deltas are archived in `audit-lint-hygiene.patch`;
no lint suppression or weakened test assertion was added. A lint cleanup initially used a private module path; it was corrected to the existing public type re-export before the final check.
Its query implementation and test behavior match the already executed code.
It does not qualify later concurrent owner edits or the unrelated `text_docs`
borrow fix. No passing behavior tests were repeated to chase shared-tree drift.

## Scope and residuals

Implementation, owner-local regressions, actual daemon/SDK changed paths, scoped
Clippy, rustfmt and whitespace checks are recorded below and in the JSON receipt.
The ignored SDK test requires an explicitly supplied fresh daemon state root;
ordinary SDK tests do not silently claim this process proof.

Repository-wide CI, clean-commit qualification, learned semantic relevance,
performance, release and deployment remain **NOT_RUN**. The broader RFC's
preventive stored-row fault-injection/property matrix is not newly claimed by
this repair. Existing source corruption checks were not weakened.

## Executed evidence

| Run | Result | Status |
| --- | --- | --- |
| `audit-clippy-1` | compile/lint only; exit 101 | FAILED |
| `audit-clippy-2` | compile/lint only; exit 101 | FAILED |
| `audit-clippy-3` | compile/lint only; exit 101 | FAILED |
| `audit-clippy-isolated-1` | compile/lint only; exit 101 | FAILED |
| `audit-clippy-isolated-2` | compile/lint only; exit 101 | FAILED |
| `audit-clippy-isolated-3` | compile/lint only; exit 0 | VERIFIED |
| `audit-composite-red-1` | 0 passed / 1 failed / 0 ignored; exit 101 | FAILED |
| `audit-dispatcher-green-1` | 465 passed / 1 failed / 0 ignored; exit 101 | FAILED |
| `audit-hybrid-control-green-1` | 1 passed / 0 failed / 0 ignored; exit 0 | VERIFIED |
| `audit-manual-scope-red-1` | 0 passed / 1 failed / 0 ignored; exit 101 | FAILED |
| `audit-native-green-1` | 18 passed / 0 failed / 0 ignored; exit 0 | VERIFIED |
| `audit-public-build-1` | compile/lint only; exit 0 | VERIFIED |
| `audit-public-sdk-1` | 0 passed / 1 failed / 0 ignored; exit 101 | FAILED |
| `audit-public-sdk-2` | 0 passed / 1 failed / 0 ignored; exit 101 | FAILED |
| `audit-public-sdk-3` | 0 passed / 1 failed / 0 ignored; exit 101 | FAILED |
| `audit-public-sdk-4` | 1 passed / 0 failed / 0 ignored; exit 0 | VERIFIED |

Exact commands, raw log hashes, source manifests, binary identities and daemon environment: L1_ADVERSARIAL_AUDIT.json, archived under `l1-proof/adversarial/`.
