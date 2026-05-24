## CI Gates

- Rust format: `cargo fmt --all -- --check`
- Rust lint: `cargo clippy --workspace --all-targets -- -D warnings`
- Rust tests: `cargo test --workspace`
- Rust policy: `python3 scripts/check_workspace_lints.py` and `bash scripts/check-rust-allow-attributes.sh`
- Rust derive allowlist: `python3 tools/ci/lint/check-rust-derive-allowlist.py`
- Rust Cargo.toml hygiene: `python3 tools/ci/lint/check-cargo-toml-hygiene.py`
- Rust module discipline: `python3 tools/ci/lint/check-module-discipline.py`
- Rust error shape: `python3 tools/ci/lint/check-error-shape.py`
- Rust supply chain: `bash scripts/run-cargo-deny.sh`
- Semgrep: `bash scripts/run-semgrep.sh` (silent-fallback / serde-derive / unwrap / vendor-import rules)
- Prompt drift: `python3 tools/prompt-manager/pm.py lint`
- Tooling tests: `python3 -m pytest tools -q`
- Agent output (PR-changed only): `python3 tools/ci/agent/validate_agent_output.py <file> --skip-rust-gates`

### Heavy rail (correctness.yml, nightly + workflow_dispatch)

- Miri, cargo-careful, TSan, ASan, cargo-mutants, cargo-udeps (existing)
- Monomorphization budget: `python3 tools/ci/lint/check-llvm-lines.py`
- Contract surface diff: `python3 tools/ci/lint/check-public-api.py`
- Module-tree snapshot: `python3 tools/ci/lint/check-cargo-modules-snapshot.py`
- IPC decoder fuzz smoke (60s each): `cd crates/quanta-index-contract/fuzz && cargo +nightly fuzz run <target>`
