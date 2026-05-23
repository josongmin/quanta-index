# Agent Execution Playbook

`AGENT_CORE.md`를 먼저 읽고, 여기서 실제 명령을 고른다.

## Quick Check

- workspace check: `cargo check --workspace`
- one crate check: `cargo check -p <crate>`

## Formatting and Lints

- format: `cargo fmt --all`
- format check: `cargo fmt --all -- --check`
- clippy: `cargo clippy --workspace --all-targets -- -D warnings`
- supply-chain policy: `bash scripts/run-cargo-deny.sh`
- workspace lint inheritance: `python3 scripts/check_workspace_lints.py`
- ban `#[allow]`: `bash scripts/check-rust-allow-attributes.sh`

## Tests

- full workspace tests: `cargo test --workspace`
- one crate tests: `cargo test -p <crate>`
- full Rust closeout: `just verify-rust`

## Structured Agent Output

- schema: `tools/ci/agent/agent_output.schema.json`
- validator: `python3 tools/ci/agent/validate_agent_output.py <output.json>`

## Prompt-Manager

- sync: `python3 tools/prompt-manager/pm.py sync`
- lint: `python3 tools/prompt-manager/pm.py lint`
- preview: `python3 tools/prompt-manager/pm.py preview --target agents`
- tests: `python3 -m pytest tools/prompt-manager/tests/test_pm.py -q`

## Retry Rule

- 같은 명령을 맥락 변화 없이 반복 재실행하지 않는다
- 실패하면 원인 분류 후 가장 좁은 surface부터 고친다
