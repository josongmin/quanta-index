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

# Local edit/compile loop without integration tests, examples, or benches.
rust-check-fast:
    {{cargo}} check --workspace --lib --bins --all-features --locked --exclude quanta-index-searchd-runtime

rust-check-daemon:
    {{cargo}} check -p quanta-index-searchd-runtime --all-features --locked

# Local build loop without integration tests or benches.
rust-build-fast:
    {{cargo}} build --workspace --lib --bins --all-features --locked --exclude quanta-index-searchd-runtime

rust-build-daemon:
    {{cargo}} build -p quanta-index-searchd-runtime --all-features --locked

# Full compile rail for every Rust target, including integration tests and benches.
rust-build-all-targets:
    {{cargo}} build --workspace --all-targets --all-features --locked

rust-timings-fast:
    {{cargo}} build --workspace --lib --bins --all-features --locked --exclude quanta-index-searchd-runtime --timings
    source scripts/quanta-index-env.sh && python3 tools/ci/timing/summarize_cargo_timings.py "$CARGO_TARGET_DIR/cargo-timings/cargo-timing.html"

rust-timings-daemon:
    {{cargo}} build -p quanta-index-searchd-runtime --all-features --locked --timings
    source scripts/quanta-index-env.sh && python3 tools/ci/timing/summarize_cargo_timings.py "$CARGO_TARGET_DIR/cargo-timings/cargo-timing.html"

rust-timings-all-targets:
    {{cargo}} build --workspace --all-targets --all-features --locked --timings
    source scripts/quanta-index-env.sh && python3 tools/ci/timing/summarize_cargo_timings.py "$CARGO_TARGET_DIR/cargo-timings/cargo-timing.html"

rust-clippy:
    {{cargo}} clippy --workspace --all-targets --all-features --locked -- -D warnings

rust-test:
    {{cargo}} test --workspace --all-features --locked

# Local test loop that keeps unit and binary coverage but skips integration/E2E.
rust-test-fast:
    {{cargo}} test --workspace --lib --bins --all-features --locked --exclude quanta-index-searchd-runtime

rust-test-unit:
    {{cargo}} test --workspace --lib --bins --all-features --locked --exclude quanta-index-searchd-runtime

rust-test-integration:
    {{cargo}} test -p quanta-index-contract --test ipc_query_result_v2_contract --all-features --locked
    {{cargo}} test -p quanta-index-contract --test lex_scaffold --all-features --locked
    {{cargo}} test -p quanta-index-core --test hybrid_policy --all-features --locked
    {{cargo}} test -p quanta-index-core --test semantic_policy --all-features --locked
    {{cargo}} test -p quanta-index-channel --test hellgate --all-features --locked
    {{cargo}} test -p quanta-index-channel --test wal_roundtrip --all-features --locked
    {{cargo}} test -p quanta-index-lexical --test tantivy_smoke --all-features --locked
    {{cargo}} test -p quanta-index-repomap --test bootstrap_owner_flow --all-features --locked
    {{cargo}} test -p quanta-index-repomap --test owner_surface --all-features --locked

rust-test-cli-smoke:
    {{cargo}} test -p quanta-index-searchctl --test cli_smoke --all-features --locked
    {{cargo}} test -p quanta-index-conformance --test cli_smoke --all-features --locked

rust-test-e2e:
    {{cargo}} test -p quanta-index-searchd-runtime --test dsl_scenarios --all-features --locked
    {{cargo}} test -p quanta-index-searchd-runtime --test end_to_end --all-features --locked
    {{cargo}} test -p quanta-index-searchd-runtime --test explain --all-features --locked
    {{cargo}} test -p quanta-index-searchd-runtime --test repo_map_end_to_end --all-features --locked

rust-test-pyramid:
    @just rust-test-unit
    @just rust-test-integration
    @just rust-test-cli-smoke
    @just rust-test-e2e

rust-doc:
    RUSTDOCFLAGS="-D warnings" {{cargo}} doc --workspace --all-features --no-deps --locked

rust-msrv:
    cargo +1.92.0 check --workspace --all-targets --all-features --locked
    cargo +1.92.0 test --workspace --all-features --locked --no-run

rust-bench:
    {{cargo}} bench --workspace --all-features --locked

rust-bench-build:
    {{cargo}} bench --workspace --all-features --locked --no-run

rust-machete:
    cargo machete --with-metadata

rust-miri:
    RUSTUP_TOOLCHAIN=nightly cargo miri setup
    RUSTUP_TOOLCHAIN=nightly MIRIFLAGS="-Zmiri-strict-provenance" \
        cargo miri test -p quanta-index-contract -p quanta-index-core --lib --all-features

rust-careful:
    RUSTUP_TOOLCHAIN=nightly cargo careful test --workspace --all-features --locked

rust-tsan:
    RUSTUP_TOOLCHAIN=nightly RUSTFLAGS="-Zsanitizer=thread" \
        cargo test -Z build-std --target $(rustc -vV | sed -n 's|host: ||p') \
            --workspace --all-features --lib --tests

rust-asan:
    RUSTUP_TOOLCHAIN=nightly RUSTFLAGS="-Zsanitizer=address" \
        cargo test -Z build-std --target $(rustc -vV | sed -n 's|host: ||p') \
            --workspace --all-features --lib --tests

rust-mutants:
    cargo mutants --package quanta-index-core --timeout 60 --baseline=skip --no-shuffle

rust-udeps:
    RUSTUP_TOOLCHAIN=nightly cargo udeps --workspace --all-targets --all-features

rust-deny:
    bash scripts/run-cargo-deny.sh

rust-workspace-lints:
    python3 scripts/check_workspace_lints.py

rust-no-allow:
    bash scripts/check-rust-allow-attributes.sh

rust-hexagonal:
    python3 tools/ci/lint/lint-hexagonal-boundaries.py

rust-derive-allowlist:
    python3 tools/ci/lint/check-rust-derive-allowlist.py

rust-cargo-toml-hygiene:
    python3 tools/ci/lint/check-cargo-toml-hygiene.py

rust-module-discipline:
    python3 tools/ci/lint/check-module-discipline.py

rust-error-shape:
    python3 tools/ci/lint/check-error-shape.py

rust-digest-fallibility:
    python3 tools/ci/lint/check-digest-fallibility.py

rust-cargo-modules:
    python3 tools/ci/lint/check-cargo-modules-snapshot.py

rust-cargo-modules-update:
    python3 tools/ci/lint/check-cargo-modules-snapshot.py --update-baseline

rust-llvm-lines:
    python3 tools/ci/lint/check-llvm-lines.py

rust-llvm-lines-update:
    python3 tools/ci/lint/check-llvm-lines.py --update-baseline

rust-public-api:
    python3 tools/ci/lint/check-public-api.py

rust-public-api-update:
    python3 tools/ci/lint/check-public-api.py --update-baseline

rust-fuzz-build:
    cd crates/quanta-index-contract/fuzz && cargo +nightly fuzz build
    cd crates/quanta-index-lq-ranker/fuzz && cargo +nightly fuzz build

rust-fuzz-smoke seconds="60":
    cd crates/quanta-index-contract/fuzz && cargo +nightly fuzz run ipc_request_decode -- -max_total_time={{seconds}}
    cd crates/quanta-index-contract/fuzz && cargo +nightly fuzz run ipc_response_decode -- -max_total_time={{seconds}}

rust-fuzz-ranker-smoke seconds="60":
    cd crates/quanta-index-lq-ranker/fuzz && cargo +nightly fuzz run weights_hash_no_panic -- -max_total_time={{seconds}}
    cd crates/quanta-index-lq-ranker/fuzz && cargo +nightly fuzz run weights_hash_determinism -- -max_total_time={{seconds}}
    cd crates/quanta-index-lq-ranker/fuzz && cargo +nightly fuzz run composite_scorer_no_panic -- -max_total_time={{seconds}}

rust-policy:
    @just rust-workspace-lints
    @just rust-hexagonal
    @just rust-no-allow
    @just rust-derive-allowlist
    @just rust-cargo-toml-hygiene
    @just rust-module-discipline
    @just rust-error-shape
    @just rust-digest-fallibility
    @just rust-deny

verify-rust:
    @just fmt-check
    @just rust-check
    @just rust-clippy
    @just rust-policy
    @just rust-machete
    @just rust-bench-build
    @just rust-test
    @just rust-doc

# Heavy correctness rail. Requires nightly toolchain + cargo-careful/miri/etc.
# Use `just verify-rust-heavy` locally before merging anything load-bearing.
verify-rust-heavy:
    @just rust-miri
    @just rust-careful
    @just rust-mutants
    @just rust-udeps
    @just rust-llvm-lines
    @just rust-public-api
    @just rust-cargo-modules
    @just rust-fuzz-build

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
