## CI Gates

- Rust format: `cargo fmt --all -- --check`
- Rust lint: `cargo clippy --workspace --all-targets -- -D warnings`
- Rust tests: `cargo test --workspace`
- Rust policy: `python3 scripts/check_workspace_lints.py` and `bash scripts/check-rust-allow-attributes.sh`
- Rust supply chain: `bash scripts/run-cargo-deny.sh`
- Prompt drift: `python3 tools/prompt-manager/pm.py lint`
- Tooling tests: `python3 -m pytest tools -q`
