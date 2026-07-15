# Source Truth Map

This map identifies current owners, not desired abstractions. `Implemented`
means present in the committed baseline; `in-flight` means visible in the shared
working tree and still requires validation/commit; `planned` has no sufficient
owner-local proof yet.

## Test and CI authority

- `tools/ci/test-authority.toml` — integration/fuzz inventory and current
  invariant proof rows. Baseline has target inventory and P0/P1 rows;
  universe/tier/role-strengthening changes are `in-flight`.
- `tools/ci/lint/check-test-authority.py` — filesystem, fuzz manifest, catalog,
  and invariant consistency guard. Execution-count validation remains `planned`.
- `.github/workflows/ci.yml` — PR/push CI; `merge_group` and nextest receipt
  changes are `in-flight`.
- `.github/workflows/correctness.yml` — scheduled/manual heavy correctness
  rails, Miri, careful, TSan, ASan, mutants, fuzz, and changed-line coverage.
  It is daily scheduled today; weekly/release tier remains `planned`.
- `tools/ci/write-verification-receipt.py` and
  `tools/ci/verification-receipt.schema.json` — receipt machinery, `in-flight`.
- `tools/ci/lint/check-ignored-test-policy.py` and
  `tools/ci/ignored-test-policy.toml` — ignored-test ownership policy,
  `in-flight`.

## Product proof seams

- `crates/quanta-index-contract/tests/ipc_query_result_v2_contract.rs` and
  `crates/quanta-index-ipc/tests/wire_historical_cbor.rs` — current contract
  and historical decode evidence; full version matrix is `planned`.
- `crates/quanta-index-semantic/tests/semantic_generation_lifecycle_model.rs`
  — lifecycle trace/model seam; generated command sequences are `in-flight`.
- `crates/quanta-index-semantic/src/build.rs` — durable sidecar/promotion path,
  atomic rename/fsync implementation, and two-boundary subprocess crash test.
  Full durability-boundary kill matrix is `planned`.
- `crates/quanta-index-semantic/tests/persisted_semantic.rs` and
  `scv2_persisted_scenarios.rs` — persistence and owner identity proof.
- `crates/quanta-index-searchd-runtime/tests/e2e_restart_replay_determinism.rs`
  — restart/replay consumer evidence.
- `crates/quanta-index-searchd-runtime/tests/sdk_frontdoor.rs` — SDK boundary
  consumer test; full SDK-only lifecycle/crash conformance is `planned`.
- `crates/quanta-index-searchd-runtime/tests/end_to_end.rs` and
  `e2e_full_corpus.rs` — broad daemon and corpus coverage; neither replaces an
  owner-local oracle.
- `crates/quanta-index-lexical/tests/tantivy_smoke.rs`, semantic persistence,
  and core policy tests — current engine evidence. Independent exhaustive
  lexical/ANN/RRF/filter oracles are `planned`.

## Known boundaries

- TSan is a broad race detector in `correctness.yml`; there is no direct
  owner-local Loom linearization model yet.
- Mutation currently targets `quanta-index-core`; score/survivor gates and
  critical semantic targets are not yet authoritative.
- Fuzz covers four decode/parser targets. Corpus retention, changed-target
  selection, and long-run weekly fuzz are not yet authoritative.
- Linux CI evidence is not macOS production-performance evidence. Cross-repo
  ingress and live-provider results require separate receipts.
