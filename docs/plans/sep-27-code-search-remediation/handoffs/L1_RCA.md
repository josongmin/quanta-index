# L1 primitive admission RCA

Status: **VERIFIED** for the repaired primitive-admission boundary. Start HEAD
`98601a66d8cab9c86232b3e62ce490c8b43b71b6`; closeout HEAD
`5571132655a83824731e7909b0e310951edad52b`, shared checkout. HEAD advanced externally during the lint queue;
this task did not create a commit. Observed passes remain accepted.

- Native L1: **17/17 passed**.
- Other native integration controls: **71 passed**, plus **3/3** migrated
  pre-interruption tests in a subsequent targeted run.
- Direct BudgetProbe/collector controls: **4/4 passed**.
- Dispatcher: **462 passed** in the aggregate; the new real-adapter test
  passed after correcting its input syntax, in a separate **1/1** run.
- Earlier aggregate failures remain recorded below. These totals combine
  successful executions and do not claim a single green aggregate run.
- Lexical library and affected integration-target lint: **VERIFIED**.
  Full repository lint remains NOT_RUN.


## Cause

`ValidatedLexicalPlan` establishes domain, projection and request shape, but did
not establish that every text primitive is executable. Keyword token admission
and `content:` leaf compilation were deferred to native execution. Predicate
planning could return `force_empty` before those checks. At the dispatcher,
language intersection could produce LogicalEmpty without opening a lexical
handle, and the opener exposed no pure primitive validator. Invalid requests
therefore depended on data and language scope.

A phrase-only check at the dispatcher would leave Keyword/Content/predicate
arguments and the native shortcut defective. Opening a generation merely to
validate input would break the intentional no-read LogicalEmpty path.

## Change

- Mandatory `LexicalIndexOpenPort::preflight_query_primitives(plan, budget)`;
  no default-success implementation. All eight implementations are updated.
- One lexical primitive-admission owner invokes the existing tokenizer,
  phrase/raw/regex planners, predicate registry and scalar lowering. It visits
  all Boolean branches and Content filter leaves before result simplification.
- Effective Regexp options apply to Keyword and RawString. The adapter supplies
  its actual regex policy. Content predicate preludes retain Standard options;
  repo-content evaluation retains caller options. Metadata regex and timeref
  inputs use their existing owners. Producer availability remains view-bound.
- Native common preparation calls admission before predicate planning. Text and
  explain dispatch call the opener before language intersection and history
  selection; Symbol dispatch calls it before cursor/pin/view acquisition.
- Search-plane uses the concrete lexical implementation only in dev-dependencies
  for tests. Production depends on the injected core port. The existing runtime
  composition already supplies LexicalAdapter, so no runtime flag is required.

## Regression design

Historical RED: native tokenless Keyword/Content after an empty repo predicate;
dispatcher tokenless Phrase after language contradiction. Existing raw receipts
are preserved in L1_PROOF.json and L1_CODE_AUDIT.json.

Added owner tests cover indexed/manual modes, Boolean order/OR/NOT, expression
and Content filter locations, nested predicate content, effective regex options,
canonical token limits and adapter regex policy. Positive raw punctuation and
metadata-availability controls distinguish primitive validity from producer
capability. Real LexicalAdapter + dispatcher tests use a nonexistent generation:
invalid input must return a literal error, while valid language contradiction
must remain empty with no executed engine and no created index.

## Test setup corrections

Two new test assumptions were corrected after reading the owning contract:
`RegexPolicy::max_nfa_states` is documented as diagnostic-only today, so the
policy propagation regression uses the active `require_literal` policy instead.
Native `content:` requires a bare filter value; the dispatcher negative uses
`content:!!!` rather than the syntactically rejected `content:"!!!"`.
Neither change relaxes the expected production error for an admitted literal.
All failed attempts remain in the raw record. Post-test lint edits add a missing
semicolon and replace two panicking assertions with the same comparisons
returning errors; their expected values and behavior are unchanged.

## Cancellation contract

The new admission phase observes pre-cancelled/expired budgets before native
collect, regex verification or manual scans. Three older integration tests
required a later checkpoint name despite cancelling before the call. Their
fixed-row controls and typed cancellation/deadline assertions are preserved;
the expected first checkpoint and names/docs now describe admission. Direct
BudgetProbe and collector tests separately exercise interruption inside walks.
This is an intentional earlier refusal, not a lost cancellation signal.

## Acceptance and scope

Per explicit user direction, a successful execution is accepted; unrelated
concurrent edits do not trigger reruns or invalidate observed test passes.
Source, dirty state, exact commands and raw log hashes are retained as provenance.
This task does not claim repository-wide CI, daemon transport/SDK E2E, deployment
or performance qualification. No cross-task inspection, outbound messages,
subagents, commit, push or reset are used.

## Executed evidence

| Run | Terminal result | Status |
| --- | --- | --- |
| `rca-cancellation-1` | 7 passed / 0 failed; exit 0 | VERIFIED |
| `rca-dispatcher-all-1` | 462 passed / 1 failed; exit 101 | FAILED |
| `rca-dispatcher-regression-1` | 1 passed / 0 failed; exit 0 | VERIFIED |
| `rca-lexical-clippy-1` | No test terminal (compile/lint only); exit 101 | FAILED |
| `rca-lexical-clippy-2` | No test terminal (compile/lint only); exit 101 | FAILED |
| `rca-lexical-clippy-3` | No test terminal (compile/lint only); exit 0 | VERIFIED |
| `rca-native-1` | No test terminal (compile/lint only); exit 101 | FAILED |
| `rca-native-2` | 16 passed / 1 failed; exit 101 | FAILED |
| `rca-native-controls-1` | 88 passed / 3 failed; exit 101 | FAILED |

Exact commands, compiler artifact identities, source manifests and raw/compressed log
hashes: `L1_RCA.json` and `l1-proof/rca/`.
