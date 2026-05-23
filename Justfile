set shell := ["/bin/zsh", "-lc"]

cargo := "./scripts/cargow"

default:
    @just --list

install-dev:
    python3 -m pip install -e '.[dev]'

install-hooks:
    python3 -m pre_commit install --install-hooks --hook-type pre-commit --hook-type pre-push

cache-root:
    @bash scripts/quanta-index-env.sh

fmt:
    {{cargo}} fmt --all

fmt-check:
    {{cargo}} fmt --all -- --check

rust-check:
    {{cargo}} check --workspace --all-targets --all-features --locked

rust-clippy:
    {{cargo}} clippy --workspace --all-targets --all-features --locked -- -D warnings

rust-test:
    {{cargo}} test --workspace --all-features --locked

rust-test-unit:
    {{cargo}} test --workspace --lib --bins --all-features --locked

rust-test-integration:
    {{cargo}} test -p quanta-index-core --test policy_contracts --all-features --locked
    {{cargo}} test -p quanta-index-control-sqlite --test control_plane --all-features --locked
    {{cargo}} test -p quanta-index-searchd --test bootstrap --all-features --locked

rust-test-e2e:
    {{cargo}} test -p quanta-index-searchd --test serve_smoke --all-features --locked

rust-test-pyramid:
    @just rust-test-unit
    @just rust-test-integration
    @just rust-test-e2e

rust-doc:
    RUSTDOCFLAGS="-D warnings" {{cargo}} doc --workspace --all-features --no-deps --locked

rust-deny:
    bash scripts/run-cargo-deny.sh

rust-workspace-lints:
    python3 scripts/check_workspace_lints.py

rust-no-allow:
    bash scripts/check-rust-allow-attributes.sh

rust-policy:
    @just rust-workspace-lints
    @just rust-no-allow
    @just rust-deny

verify-rust:
    @just fmt-check
    @just rust-check
    @just rust-clippy
    @just rust-policy
    @just rust-test-pyramid
    @just rust-doc

semgrep:
    bash scripts/run-semgrep.sh

actionlint:
    bash scripts/run-actionlint.sh

shell-lint:
    bash scripts/run-shellcheck.sh

python-lint:
    source scripts/quanta-index-env.sh && python3 -m ruff check .

python-format-check:
    source scripts/quanta-index-env.sh && python3 -m ruff format --check .

python-test:
    source scripts/quanta-index-env.sh && python3 -m pytest tools -q -o cache_dir="$PYTEST_CACHE_DIR"

agent-output-validate output:
    source scripts/quanta-index-env.sh && python3 tools/ci/agent/validate_agent_output.py {{output}}

lint-root-hygiene:
    bash tools/ci/lint/lint-root-hygiene.sh

lint-doc-paths:
    python3 tools/ci/lint/lint-doc-paths.py

check-lock-freshness:
    bash scripts/check-lock-freshness.sh

verify:
    @just lint-root-hygiene
    @just lint-doc-paths
    @just check-lock-freshness
    @just verify-rust
    @just actionlint
    @just shell-lint
    @just semgrep
    @just python-lint
    @just python-format-check
    @just python-test
    @just lint-prompt-drift

sync-prompts:
    python3 tools/prompt-manager/pm.py sync

lint-prompt-drift:
    python3 tools/prompt-manager/pm.py lint

test-prompt-manager:
    source scripts/quanta-index-env.sh && python3 -m pytest tools/prompt-manager/tests/test_pm.py -q -o cache_dir="$PYTEST_CACHE_DIR"

verify-prompts:
    @just sync-prompts
    @just lint-prompt-drift
    @just test-prompt-manager

precommit-run:
    source scripts/quanta-index-env.sh && python3 -m pre_commit run --all-files --show-diff-on-failure
