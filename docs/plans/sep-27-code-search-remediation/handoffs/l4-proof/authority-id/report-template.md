# L4 adversarial audit — strict stored authority IDs

**Fixed: one reproduced P2 defect.** A stored text document containing two
`text_authority_doc_id` values was accepted by selecting the first. The same
ambiguous row survived actual Tantivy serialization and full authority rebuild.
This is a malformed stored-authority boundary failure, not a demonstrated bypass
of sealed artifact digests or a remotely reachable ingestion exploit.

## Change and regression oracle

- `text_docs::stored_text_authority_doc_id` now requires exactly one numeric ID
  in the writer's allocated domain, `1..=u32::MAX`. Duplicate equal IDs, conflicting
  IDs in either order, zero, overflow and malformed values are refused. Missing
  legacy IDs retain the existing generation-format error.
- Source-bound preview conversion maps invalid stored IDs to the existing
  `SEARCH_PREVIEW_INTEGRITY` error before optional preview budget refusal. Authority
  rebuild and file replacement consume the same strict decoder. Query truth,
  ranking, normalizer behavior and wire DTOs are unchanged.
- Three permanent tests exercise ambiguity/domain refusal, independent valid
  boundary values and duplicate IDs read back from a real stored index. The
  **RED** run executed three tests: two failed on duplicate `[1, 2]` acceptance,
  one passed. The remaining invalid-domain cases are covered by GREEN; the
  short-circuiting RED does not independently demonstrate each of them.
- Two verification repairs preserve behavior: name the rejected `OnceLock` probe
  explicitly, and use checked fixture access in an existing L2 integration test.
  They are not two additional production defect findings.

A further source pass covered original/NFC/folded coordinates, Boolean witness
rollback, regex range semantics, focus windows, selected source ownership,
optional refusal, request output retention and authority consumers. No further
reproducible P0–P2 was identified in that inspected scope. This is not a proof
that the entire repository contains no P0–P2.

## Current proof

[Evidence index](l4-proof/authority-id/receipt.json) binds raw output, exact
commands, file manifests, toolchain/environment, test binary hashes and live
source comparison. Receipt SHA-256: `RECEIPT_DIGEST`.

Final frozen source SHA-256:
`SOURCE_DIGEST`.

| Action | Status | Executed scope |
| --- | --- | --- |
| `./scripts/cargow test -p quanta-index-lexical --lib --test l4_match_anchored_preview --test l2_file_mutation --locked` | VERIFIED | 182 library + 23 file mutation + 9 preview = 214 passed; zero failed, ignored or filtered |
| `./scripts/cargow clippy -p quanta-index-lexical --lib --test l4_match_anchored_preview --test l2_file_mutation --locked --no-deps` | VERIFIED | Selected library and integration targets |
| Canonical-env `rustfmt --edition 2024 --check` | VERIFIED | Four changed Rust source/test files |
| Archive plus recorded patch reconstruction | VERIFIED | All 1,167 final bound files reproduced byte-for-byte |

All heavy checks use `CARGO_BUILD_JOBS=2`, the canonical checkout-scoped cache and
shared resource-admission lock. No cross-snapshot target override is used. All
35 named authority-ID/L4/L2 regressions must appear as executed successfully;
raw successful test lines are counted independently of summary totals. Earlier
runs are retained but their test counts are not added to the final 214.

The source is a filesystem composition originating from dirty
`5571132655a83824731e7909b0e310951edad52b`, not a clean Git revision. Concurrent
owner changes fixed a transient cancellation-code compile failure before both
RED and GREEN. Four later selected-source changes were imported for final
verification; the subsequent two lint repairs are also recorded. Failed
compile/lint attempts are retained as failures, not test success.

The archive includes current Cargo configuration and lockfile. The external
SSTable source is pinned to Git revision
`22b3c0e3b554faa8de7a55f917f90c90f2af7050`, separately hashed and archived. To
reconstruct final workspace inputs, extract `red-source.tar.gz`, then apply
`fix.patch`, `selected-refresh.patch`, and `lint-fix.patch` in that order.
The reconstruction receipt checks every resulting file against the final manifest.

At closeout, shared HEAD was `LIVE_HEAD`, with dirty concurrent work.
LIVE_COMPARISON
Exact paths and hashes are in [live-closeout.json](l4-proof/authority-id/live-closeout.json).
This audit performed no task coordination, task messages/reads/polling,
delegation, commit, push or reset.

## Remaining scope

- **BLOCKED:** aggregate regex compiler/cache heap upper-bound proof remains
  unresolved. A logical executor reservation is not an aggregate heap/RSS proof;
  no aggregate runtime overrun was reproduced in this audit.
- **NOT_RUN:** whole-workspace CI, SDK/fresh-process daemon E2E, performance/RSS,
  release, deployment and activation. The current private decoder change does
  not change a public API or wire envelope; public-API/fuzz gates were not rerun.
  Earlier gate receipts are historical and are not promoted here.
- Whole shared-tree qualification is not claimed. This receipt covers the
  selected lexical/native scope and its recorded dependency/configuration inputs.
