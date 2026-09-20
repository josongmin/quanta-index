# Agent Execution Playbook

`AGENT_CORE.md`를 먼저 읽고, 여기서 실제 명령을 고른다.

## Quick Check

- preferred agent surface: `just rust-profile <name>`
- default local compile: `just rust-profile dev-fast`
- daemon-only compile: `just rust-profile dev-daemon`
- deployable daemon artifact: `just rust-profile release-daemon`
- stale-fingerprint-safe daemon artifact: `just rust-profile release-daemon-fresh`
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
- bounded integration edit loop: `just rust-profile test-integration-fast`
- lexical storage integration slice: `just rust-profile test-integration-storage`
- semantic storage integration slice: `just rust-profile test-integration-semantic`
- complete integration rail: `just rust-profile test-integration`
- fast daemon e2e edit loop: `just rust-profile test-daemon-fast`
- risk-focused daemon e2e rail: `just rust-profile test-daemon`
- exhaustive daemon e2e rail: `just rust-profile test-daemon-all`
- full Rust closeout: `just rust-profile verify-rust`
- history summary: `just rust-profile-history-summary`
- one crate probe when the profile catalog is insufficient: `./scripts/cargow test -p <crate>`

Local integration/CLI/daemon profiles resolve target IDs from
`tools/ci/test-authority.toml` and launch one `cargo nextest` process per scope.
Do not restore one-Cargo-process-per-test recipes. `validate-shared-surface`
uses one shared lane, compiles the selected all-target graph once in the test
profile with nextest `--no-run`, then executes three bounded selections: shared
libraries, bounded integration, and CLI smoke. The selections stay separate
because Cargo's global `--lib` selector would otherwise pull unrelated package
libraries into the integration command. CI keeps its independent workspace-wide
authority rail. Scope-specific `test_threads` caps prevent a scheduler from
oversubscribing daemon and storage tests; composed scopes use the smallest
declared cap.
The 49 searchd-runtime scenario source files are modules of three explicit Cargo
test suites (fast, risk, extended), not 49 separately linked binaries. The test
authority guard verifies every source-to-suite binding. `test-daemon-fast`
executes only the fast runtime suite; the expensive DSL golden-truth matrix
remains in `test-daemon`, `test-daemon-all`, and `rust-bench-dsl-truth`.
The complete integration profile runs fast, lexical-storage, and semantic
slices sequentially in one build lane, retaining their 8/2/4 thread caps. The
edit loop can skip text-authority persistence and Lance/DataFusion when those
storage surfaces did not change. Local `sccache`, when installed, reuses
cacheable clean-rebuild work on a repository-isolated server;
`QUANTA_INDEX_SCCACHE=0` disables it.

Timing and timing-bearing quality rails fail before build/cache mutation when
foreign Cargo or rustc processes are active. Wait for a quiet host. The
`QUANTA_INDEX_ALLOW_CONTENDED_TIMINGS=1` escape hatch permits diagnosis only;
it is not clean performance evidence.

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
