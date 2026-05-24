## Rule Catalog

### Safety

- no silent failure
- no silent fallback
- fail-closed by default
- no error swallowing or error-to-default replacement on production paths
- no heuristic authority when the real authority is absent
- no unverifiable completion claim
- no `ok` status when required inputs, correctness-affecting assumptions, failed checks, or tool errors remain

### Architecture

- shared contract crate is the only producer/search-plane integration surface
- core crate must not import `rusqlite`, `tantivy`, `lancedb`, or raw filesystem layout
- storage/query vendor choices belong to adapters only

### Build hygiene

- no proc-macro derives for serialization: `#[derive(serde::Serialize)]`, `#[derive(serde::Deserialize)]`, `#[derive(Serialize)]`, `#[derive(Deserialize)]` are banned. Write manual `impl serde::Serialize` / `impl serde::Deserialize` instead. Reason: proc-macro expansion is the dominant build-time cost in serde-heavy crates; manual impls keep cold-build seconds bounded and make wire shape auditable.
- the ban applies workspace-wide (contract, core, adapters, searchd, tests, benches). Enforced by semgrep rule `rust-no-serde-derive`.
- derive allowlist (script `tools/ci/lint/check-rust-derive-allowlist.py`): only `Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd, Error` (thiserror) are permitted. Any other `#[derive(...)]` arrival — `strum::EnumIter`, `clap::Parser`, `Deserialize_repr`, future proc-macros — must measure cost (cargo llvm-lines / --timings) and extend the allowlist explicitly. Reason: flips the serde rule from denylist to allowlist so new proc-macro deps cannot silently bloat cold-build time.
- monomorphization budget: `tools/ci/lint/check-llvm-lines.py` snapshots LLVM IR line counts for contract + core and fails PRs that exceed the baseline by >15% (and >5k absolute lines). Reason: derive allowlist enforces the rule; this guards the *outcome* — generic blowup or new proc-macros that slip past the allowlist still trip this gate.

### Contract surface

- public API of `quanta-index-contract` is snapshotted by `tools/ci/lint/check-public-api.py` against `tools/ci/lint/baselines/public-api/quanta-index-contract.txt`. Any drift fails CI; updating the baseline must be an intentional commit in the same PR as the breaking change. Reason: contract crate is the only producer/search-plane integration surface — silent shape drift is impossible to review after the fact.

### Fail-closed deserialization

- IPC request/response decoders are fuzzed via cargo-fuzz under `crates/quanta-index-contract/fuzz/` in the heavy correctness rail (`just rust-fuzz-smoke`, `.github/workflows/correctness.yml` job `rust-fuzz-smoke`, 60s/target). Any panic, infinite loop, or non-`Err` exit on malformed bytes is treated as a fail-closed violation.

### Silent-fallback guards (semgrep)

- `rust-no-silent-or-else-ok` blocks `.or_else(|_| Ok(...))` shaped error-to-success conversions.
- `rust-no-err-arm-default` blocks `Err(_) => Default::default()` / `Vec::new()` / `None` / etc. in production crate src/ trees.
- `rust-no-debug-assertions-divergence` blocks `cfg!(debug_assertions)` and `#[cfg(debug_assertions)]` in contract/core production paths so release behavior cannot silently diverge from debug.
- `rust-no-result-to-option-discard` blocks `.err().is_some()` / `.err().is_none()` which throw away the error payload.
- *deliberately not enforced via semgrep:* `if let Ok(x) = ... { ... }` with no else. Semgrep's Rust grammar does not handle multi-statement block patterns reliably, and the idiom is too common in legitimate best-effort paths (metrics, logging) to lint without high false-positive rate. Manual code review covers it for now.

### Verification

- compile claims require a real `cargo` run
- prompt-manager claims require `pm.py lint`
- behavior changes require tests
- structured agent outputs must validate against `tools/ci/agent/agent_output.schema.json`

### Documentation

- generated docs are artifacts
- edit `tools/prompt-manager/sources/` only
- run `pm.py sync` then `pm.py lint`
