# L1 adversarial code audit

Status: VERIFIED for the two corrections below. The full L1 remediation remains
incomplete because primitive admission is still missing. HEAD is
`98601a66d8cab9c86232b3e62ce490c8b43b71b6` with concurrent dirty work.

## Confirmed native-port defects and repairs

1. **P2 — exact-all omits canonical Symbol name rewriting.**
   `search_all(symbol.has.name(needle))` refused with `LEX_PREDICATE_UNIMPLEMENTED`
   while the ordinary Text port returned the five fixture-authored Symbol IDs.
   All eight indexed/manual × absent/all count × implicit/explicit Symbol cases
   failed. `port.rs` now calls the same `rewrite_symbol_name_predicate_query`
   before immutable domain admission as the other Text/Symbol/explain/admission
   entry points. No tokenizer, predicate registry or public API was duplicated.
2. **P2 — indexed repo projection loses other source repositories.**
   `search_all(select:repo needle)` / `type:repo` retained one row for the entire
   containing generation. The fixture independently contains source-a and
   source-b; indexed absent/all count cases lost source-b while manual controls
   retained both. The port now uses the existing `collect_projection(Repo)`
   authority. Two source-a files still collapse to its best row; source-b remains.
   Empty-result, ordinary-port and manual controls are included.

Production edit: `crates/quanta-index-lexical/src/searcher/port.rs`.
Permanent regressions: `crates/quanta-index-lexical/tests/l1_query_domain_window.rs`.
The existing fixture builder was factored to accept explicitly authored scopes.

## Reachability and claim boundary

These failures are reproduced through the real public `LexicalSearcher` native
port on sealed source-bound fixtures. They are not claimed as current daemon/SDK
failures: the sole production exact-all caller is structural routing, which
routes a top-level `symbol.has.name` to `search_symbols_all` and refuses repo
projection filters before execution. The native contract/parity defects still
exist independently of that public-route restriction.

The audit read the immutable domain plan, native page/all/explain/admission
entry points, manual matching/rendering, dispatcher lexical/Symbol routes,
cursor binding, authoritative count/window and byte-clipping callers. No new
reachable defect was established in cursor authentication or count clipping.
That is an audit result, not an exhaustive correctness proof. L2/L3/L4/L5
production files and shared API owners were not modified.

## Verification

| Run | Terminal result | Evidence state |
| --- | --- | --- |
| `audit-native-red-1` | No test terminal counts; see compiler diagnostics; exit 101 | FAILED |
| `audit-native-red-2` | 0 passed / 2 failed; exit 101 | FAILED |
| `audit-native-red-3` | 0 passed / 2 failed; exit 101 | Source-stable behavioral RED (pre-fix source) |
| `audit-native-green-1` | 2 passed / 0 failed; exit 0 | VERIFIED |
| `audit-native-controls-1` | 87 passed / 1 failed; exit 101 | FAILED |
| `audit-native-final-1` | 13 passed / 1 failed; exit 101 | FAILED |
| `audit-native-final-2` | 13 passed / 1 failed; exit 101 | FAILED |

Commands, inputs/toolchain, before/after/current source closures, executable hashes
and raw compressed/uncompressed log digests are in `L1_CODE_AUDIT.json` and
`l1-proof/audit/`. The RED oracle is fixed IDs/cardinality authored by the fixture;
the ordinary query is a positive control, not the oracle used to derive the set.

The first attempt executed no tests because the concurrent shared ingest
observation cutover temporarily duplicated `validate_for` and mismatched
`validate_identity`. That failure was resolved externally. Per explicit user acceptance, the successful executions establish the two
corrections for the exercised cases. Concurrent source changes are retained as
provenance; they do not invalidate these observed passes or trigger further
reruns. This acceptance does not claim unexecuted repository or SDK gates.

Formatting and whitespace: `rustfmt --check --edition 2024 --config
skip_children=true` and scoped `git diff --check` both exited 0 for the two
audit-edited files. Exact arguments and hashes are in the static receipt.

## Residual and corrected earlier scope

- **FAILED / remaining:** native Keyword/Content and dispatcher phrase can be
  hidden by predicate/language empty shortcuts. The shared primitive-admission
  API is still absent. See `L1_PRIMITIVE_ADMISSION_REQUEST.md`.
- **NOT_RUN:** actual daemon/SDK assembly, repository-wide CI, release/deployment
  and performance or memory qualification. Full lint was not rerun during this audit.
- The previous statement that no further safe L1 work existed was too broad:
  this audit found the two additional owned-port defects above. The prior goal
  blocker record is historical; it must not be treated as an exhaustive audit.
- Prior handoff receipts remain historical. Current audit execution results
  are recorded above; no unchanged-input RED/GREEN pairing is claimed.

No outbound task messages, task inspection/polling, extra agents, commit, push
or reset were used.
