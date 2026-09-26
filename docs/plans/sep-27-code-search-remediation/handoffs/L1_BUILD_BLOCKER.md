# L1 build admission blocker

Status: **historical FAILED, compile blocker resolved externally**.
Current native compilation executes tests; primitive-admission regressions remain.
See `L1_PROOF.json` for source-bound results, not the historical zero-test receipt.
No cross-task message sent. No shared Cargo/vendor edit applied by L1.

- HEAD: `98601a66d8cab9c86232b3e62ce490c8b43b71b6`, shared dirty checkout.
- `Cargo.toml` now patches `tantivy-sstable` to `vendor/tantivy-sstable`.
  The initial lock mismatch is resolved by the updated shared `Cargo.lock`.
- Source-stable command:
  `./scripts/cargow test --locked -j 2 -p quanta-index-lexical --test l1_query_domain_window --message-format=json`.
- Receipt: `/tmp/quanta-l1-green.diz44g/native-final-5.receipt.json`.
- Exit 101, **zero tests executed**. Error E0599 at
  `vendor/tantivy-sstable/src/dictionary.rs:64`: `FileSlice::len()` requires
  `tantivy_common::HasLen` to be imported.
- This crate aliases the dependency as `common`; the applicable correction is
  `use common::{BinarySerializable, HasLen, OwnedBytes};` in that module.
- Source manifest SHA-256:
  `5793d4147fb271c2411393e3a7cce6c96ba071b26373bb03ff8788dd2827172e`.
- Raw log SHA-256:
  `e80c5ccc8d413bfc2b1d3e6646083384b0b57cbea475098c7943ade5842f3432`.

The current collector follows Cargo.lock from each selected package and binds
reachable local registry patch manifests/build scripts/src, path dependency
sources, selected fixtures, config, toolchain, wrappers and admission helpers.
It excludes unrelated vendor provenance/doc changes from query behavior inputs.

`native_all` now executes 86 tests, including the final `ManualDocumentView`
refactor. Consult `L1_PROOF.json` for terminal counts and current-source validity.
Compilation recovery does not fix the native Keyword/Content or dispatcher phrase
admission failures described in `L1_PRIMITIVE_ADMISSION_REQUEST.md`.
