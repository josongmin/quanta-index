# Agent Execution Playbook

`AGENT_CORE.md`를 먼저 읽고, 여기서 실제 명령을 고른다.

## Quick Check

- preferred agent surface: `just rust-profile <name>`
- default local compile: `just rust-profile dev-fast`
- daemon-only compile: `just rust-profile dev-daemon`
- widest compile rail: `just rust-profile dev-all-targets`
- shared-surface validation: `just rust-profile validate-shared-surface`
- one crate probe when the profile catalog is insufficient: `cargo check -p <crate>`

## Formatting and Lints

- format: `cargo fmt --all`
- format check: `cargo fmt --all -- --check`
- clippy: `cargo clippy --workspace --all-targets -- -D warnings`
- supply-chain policy: `bash scripts/run-cargo-deny.sh`
- workspace lint inheritance: `python3 scripts/check_workspace_lints.py`
- ban `#[allow]`: `bash scripts/check-rust-allow-attributes.sh`

## Tests

- default local tests: `just rust-profile test-fast`
- integration rail: `just rust-profile test-integration`
- daemon e2e rail: `just rust-profile test-daemon`
- full Rust closeout: `just rust-profile verify-rust`
- history summary: `just rust-profile-history-summary`
- one crate probe when the profile catalog is insufficient: `cargo test -p <crate>`

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
