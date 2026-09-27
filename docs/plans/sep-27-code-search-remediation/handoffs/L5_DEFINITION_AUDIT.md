# L5 named-definition code audit

Status: **VERIFIED for the bounded producer/consumer scope below**. Five additional
inventory/ownership regressions were reproduced on the original source and repaired.

Final source: `681dc3f0c5232b4c756c9fd296d305677560370a` plus dirty overlay, frozen at
`/Users/songmin/.codex/worktrees/l5-definition-final/quanta-index`. The shared checkout's three edited source/inventory files match this
snapshot at closeout. A concurrent commit included the initial fixes; this task
performed no commit, push or reset. Other shared-checkout drift, if any, is listed
in the machine report and is not covered by this snapshot's qualification.

- Rust: **138 passed** (87 library, 3 binary, 25 chunking, 23 parser); the exact
  **135** registered owner test identities match executed tests.
- Python preflight/receipt compatibility: **16 passed**, 323 deselected.
- Fresh Vite CLI: **256** files retained, **255 Complete**, one intentional
  ParseFailed; **2241** symbols. Allow-incomplete exits 0 and
  strict exits 2 after emitting the same complete census. Source/grammar/lockfile
  policy commitments are independently recomputed by the Python consumer.
- Changed Rust files pass rustfmt; changed files pass whitespace checks.
- All final receipts have identical source manifests and unchanged before/after inputs.

| Finding | Fixed behavior |
| --- | --- |
| L5-DEF-01 / P2 | Go `type Alias = ...` is emitted as type_alias. Local types retain `Function.Local` and `Receiver.Method.Local` ownership. |
| L5-DEF-02 / P2 | Named Go interfaces and interface aliases emit their own method contracts, including local interfaces. Two existing tests now select the concrete receiver by qualified name rather than accepting another same-name method. |
| L5-DEF-03 / P2 | Explicitly named JS/TS class/function/generator expressions emit a definition and qualify their descendants. Anonymous expressions receive no invented binding name. |
| L5-DEF-04 / P2 | TS function declarations without bodies, interface/type-alias methods, abstract methods and overload signatures retain their own spans and owners. |
| L5-DEF-05 / P2 | TS enum and module/namespace definitions are emitted together with their members. |

The handwritten regression inventories assert exact names, kinds, source slices,
owner fields, multiplicity and Complete counts across Go, JS/JSX and TS/TSX.
The original-source run failed all five new tests with 18 controls passing. The TS
type-alias method case was added during the subsequent completeness review.

A first fixed-source run still returned the original integration behavior. Copied
input mtimes predated the earlier library build, consistent with stale artifact
reuse; the new regressions caught the mismatch. That failure
and the interrupted follow-up remain in the evidence. Final verification uses a
fresh checkout-scoped Cargo target, fresh copied mtimes, and a source-policy check
against the actual CLI binary. No failed, interrupted or stale run is promoted.

Commands, source/environment identities, raw outputs and digests:
[L5_DEFINITION_AUDIT.json](L5_DEFINITION_AUDIT.json), SHA-256 `76b6f610bbb9f9009af22319589fd5f95d808b9d6714ece27793aa62928adb5b`.
Raw logs are retained as `.raw.txt` files under
[l5-proof/definition-audit-20260927](l5-proof/definition-audit-20260927).

This proves the supported named-definition forms exercised here, not a complete
language semantic symbol table. Variable/field/enum-member inventories, computed
property resolution and semantic name normalization are outside this change.
Whole-repository CI, fresh daemon/SDK query execution and release/deployment are
**NOT_RUN** on this snapshot. Historical audit and process receipts remain bound
to their original sources; their counts are not combined with this proof.
