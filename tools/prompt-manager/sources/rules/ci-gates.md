## CI Gates

- Rust format: `just fmt-check`
- Rust lint: `just rust-clippy`
- Rust tests: `just rust-test`
- Rust policy: `just rust-workspace-lints`, `just rust-hexagonal`, `just rust-no-allow`, `just rust-derive-allowlist`, `just rust-cargo-toml-hygiene`, `just rust-module-discipline`, `just rust-module-cycles`, `just rust-error-shape`, `just rust-digest-fallibility`, `just rust-wire-inventory`, `just rust-deny`
- Rust derive allowlist: `python3 tools/ci/lint/check-rust-derive-allowlist.py`
- Rust Cargo.toml hygiene: `python3 tools/ci/lint/check-cargo-toml-hygiene.py`
- Rust module discipline: `python3 tools/ci/lint/check-module-discipline.py`
- Rust module cycles: `python3 tools/ci/lint/check-module-cycles.py`
- Rust error shape: `python3 tools/ci/lint/check-error-shape.py`
- Rust digest fallibility: `python3 tools/ci/lint/check-digest-fallibility.py`
- Wire-surface inventory: `python3 tools/ci/lint/check-wire-inventory.py`
- Rust supply chain: `bash scripts/run-cargo-deny.sh`
- Semgrep: `just semgrep` (silent-fallback / serde-derive / unwrap / vendor-import rules)
- Prompt drift: `python3 tools/prompt-manager/pm.py lint`
- Tooling tests: `python3 -m pytest tools -q`
- Agent output envelope and evidence binding (PR-changed only): `python3 tools/ci/agent/validate_agent_output.py <file>`

### Heavy rail (correctness.yml, nightly + workflow_dispatch)

- Miri, cargo-careful, TSan, ASan, cargo-mutants, cargo-udeps (existing)
- real-engine full corpus: `just rust-test-full-corpus`
- Monomorphization budget: `python3 tools/ci/lint/check-llvm-lines.py`
- Contract surface diff: `python3 tools/ci/lint/check-public-api.py`
- Module-tree snapshot: `python3 tools/ci/lint/check-cargo-modules-snapshot.py`
- IPC decoder fuzz build: `just rust-fuzz-build`
- IPC decoder fuzz smoke (60s each): `just rust-fuzz-smoke`
