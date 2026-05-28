# Agent Execution Playbook

`AGENT_CORE.md`를 먼저 읽고, 여기서 실제 명령을 고른다.

## Quick Check

- preferred agent surface: `just rust-profile <name>`
- default local compile: `just rust-profile dev-fast`
- daemon-only compile: `just rust-profile dev-daemon`
- widest compile rail: `just rust-profile dev-all-targets`
- shared-surface validation: `just rust-profile validate-shared-surface`
- one crate probe when the profile catalog is insufficient: `./scripts/cargow check -p <crate>`

## Formatting and Lints

- format: `just fmt`
- format check: `just fmt-check`
- clippy: `just rust-clippy`
- supply-chain policy: `just rust-deny`
- workspace lint inheritance: `just rust-workspace-lints`
- hexagonal boundaries: `just rust-hexagonal`
- ban `#[allow]`: `just rust-no-allow`

## Tests

- default local tests: `just rust-profile test-fast`
- integration rail: `just rust-profile test-integration`
- daemon e2e rail: `just rust-profile test-daemon`
- full Rust closeout: `just rust-profile verify-rust`
- history summary: `just rust-profile-history-summary`
- one crate probe when the profile catalog is insufficient: `./scripts/cargow test -p <crate>`

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

## Cargo Wrapper Rule

- bare `cargo`보다 `./scripts/cargow`를 우선한다
- bare `cargo`가 필요하면 먼저 `scripts/quanta-index-env.sh`를 source하거나, 이미 그 작업을 캡슐화한 `just` target을 쓴다

## Mandatory Escalation By Change Surface

- `quanta-index-contract` / `quanta-index-sdk` public surface 변경: `just rust-public-api`
- IPC decode, wire DTO, error envelope 변경: `just rust-fuzz-smoke`
- crate/module boundary, facade/export surface 변경: `just rust-hexagonal` + `just rust-cargo-modules`
- activation/generation resolution/query pin/state-root/shared-ingress 변경: `just rust-profile test-daemon` plus the owning `U/E/C/H-SP` scenario proof
- prompt-manager source 변경: `python3 tools/prompt-manager/pm.py sync`, `python3 tools/prompt-manager/pm.py lint`, `python3 -m pytest tools/prompt-manager/tests/test_pm.py -q`
