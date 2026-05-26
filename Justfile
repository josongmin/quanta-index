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

rust-profile-list:
    @printf '%s\n' \
        'dev-fast            default local edit loop; workspace lib/bin compile, excludes searchd runtime' \
        'dev-daemon          searchd runtime only; use when editing daemon/runtime entrypoints' \
        'dev-all-targets     widest compile rail; all targets across the workspace' \
        'validate-shared-surface contract/core/sdk/search-plane shared-surface validation rail' \
        'test-fast           default local test loop; workspace lib/bin tests, excludes daemon e2e' \
        'test-integration    contract/core/channel/lexical/repomap integration rail' \
        'test-cli-smoke      CLI smoke tests for searchctl + conformance' \
        'test-daemon         searchd runtime scenario/e2e tests' \
        'verify-rust         standard merge gate' \
        'verify-rust-heavy   nightly/heavy correctness gate' \
        'timings-fast        fast-lane timing capture' \
        'timings-daemon      daemon-lane timing capture' \
        'timings-all-targets full all-targets timing capture' \
        'bench-build         criterion build-only rail' \
        'doc                 rustdoc rail' \
        'msrv                pinned-toolchain compatibility rail'

rust-profile profile:
    @case "{{profile}}" in \
        dev-fast) ./scripts/run-rust-profile.sh "{{profile}}" rust-check-fast ;; \
        dev-daemon) ./scripts/run-rust-profile.sh "{{profile}}" rust-check-daemon ;; \
        dev-all-targets) ./scripts/run-rust-profile.sh "{{profile}}" rust-check ;; \
        validate-shared-surface) ./scripts/run-rust-profile.sh "{{profile}}" rust-validate-shared-surface ;; \
        test-fast) ./scripts/run-rust-profile.sh "{{profile}}" rust-test-fast ;; \
        test-integration) ./scripts/run-rust-profile.sh "{{profile}}" rust-test-integration ;; \
        test-cli-smoke) ./scripts/run-rust-profile.sh "{{profile}}" rust-test-cli-smoke ;; \
        test-daemon) ./scripts/run-rust-profile.sh "{{profile}}" rust-test-e2e ;; \
        verify-rust) ./scripts/run-rust-profile.sh "{{profile}}" verify-rust ;; \
        verify-rust-heavy) ./scripts/run-rust-profile.sh "{{profile}}" verify-rust-heavy ;; \
        timings-fast) ./scripts/run-rust-profile.sh "{{profile}}" rust-timings-fast ;; \
        timings-daemon) ./scripts/run-rust-profile.sh "{{profile}}" rust-timings-daemon ;; \
        timings-all-targets) ./scripts/run-rust-profile.sh "{{profile}}" rust-timings-all-targets ;; \
        bench-build) ./scripts/run-rust-profile.sh "{{profile}}" rust-bench-build ;; \
        doc) ./scripts/run-rust-profile.sh "{{profile}}" rust-doc ;; \
        msrv) ./scripts/run-rust-profile.sh "{{profile}}" rust-msrv ;; \
        *) \
            echo "unknown rust profile: {{profile}}" >&2; \
            just rust-profile-list >&2; \
            exit 2 ;; \
    esac

rust-profile-history-summary:
    source scripts/quanta-index-env.sh && python3 tools/ci/timing/rust_profile_history.py summary

rust-profile-history-summary-json:
    source scripts/quanta-index-env.sh && python3 tools/ci/timing/rust_profile_history.py summary --json

fmt:
    {{cargo}} --lane fmt-lane fmt --all

fmt-check:
    {{cargo}} --lane fmt-lane fmt --all -- --check

rust-check:
    {{cargo}} --lane all-targets-lane check --workspace --all-targets --all-features --locked

# Local edit/compile loop without integration tests, examples, or benches.
rust-check-fast:
    {{cargo}} --lane fast-lane check --workspace --lib --bins --all-features --locked --exclude quanta-index-searchd-runtime

rust-check-daemon:
    {{cargo}} --lane daemon-lane check -p quanta-index-searchd-runtime --all-features --locked

rust-validate-shared-surface:
    @just rust-check
    @just rust-test-integration
    @just rust-test-cli-smoke

# Local build loop without integration tests or benches.
rust-build-fast:
    {{cargo}} --lane fast-lane build --workspace --lib --bins --all-features --locked --exclude quanta-index-searchd-runtime

rust-build-daemon:
    {{cargo}} --lane daemon-lane build -p quanta-index-searchd-runtime --all-features --locked

# Full compile rail for every Rust target, including integration tests and benches.
rust-build-all-targets:
    {{cargo}} --lane all-targets-lane build --workspace --all-targets --all-features --locked

rust-timings-fast:
    {{cargo}} --lane timings-fast-lane build --workspace --lib --bins --all-features --locked --exclude quanta-index-searchd-runtime --timings
    env QUANTA_INDEX_BUILD_LANE=timings-fast-lane bash -lc 'source scripts/quanta-index-env.sh && python3 tools/ci/timing/summarize_cargo_timings.py "$CARGO_TARGET_DIR/cargo-timings/cargo-timing.html"'

rust-timings-daemon:
    {{cargo}} --lane timings-daemon-lane build -p quanta-index-searchd-runtime --all-features --locked --timings
    env QUANTA_INDEX_BUILD_LANE=timings-daemon-lane bash -lc 'source scripts/quanta-index-env.sh && python3 tools/ci/timing/summarize_cargo_timings.py "$CARGO_TARGET_DIR/cargo-timings/cargo-timing.html"'

rust-timings-all-targets:
    {{cargo}} --lane timings-all-targets-lane build --workspace --all-targets --all-features --locked --timings
    env QUANTA_INDEX_BUILD_LANE=timings-all-targets-lane bash -lc 'source scripts/quanta-index-env.sh && python3 tools/ci/timing/summarize_cargo_timings.py "$CARGO_TARGET_DIR/cargo-timings/cargo-timing.html"'

# Compile-regression gate (BLD-06). Builds the relevant lane, summarizes the timing
# artifact, then diffs the in-repo crate aggregates against the committed baseline
# under tools/ci/timing/baselines/. Fails on regressions over 15% AND 0.10s.
#
# All cargo calls go through {{cargo}} (scripts/cargow), which sources
# scripts/quanta-index-env.sh and exports CARGO_TARGET_DIR. Using raw `cargo
# clean` here would wipe the wrong target dir and leave a warm cache underneath
# the timing build, defeating the cold-build assumption (BLD-06A).
rust-timings-fast-check:
    {{cargo}} --lane timings-fast-lane clean --quiet
    {{cargo}} --lane timings-fast-lane build --workspace --lib --bins --all-features --locked --exclude quanta-index-searchd-runtime --timings
    env QUANTA_INDEX_BUILD_LANE=timings-fast-lane bash -lc 'source scripts/quanta-index-env.sh && python3 tools/ci/timing/summarize_cargo_timings.py "$CARGO_TARGET_DIR/cargo-timings/cargo-timing.html" --json --top-crates 25 > /tmp/quanta-index-fast-current.json'
    python3 tools/ci/timing/compare_cargo_timings.py tools/ci/timing/baselines/fast-lane.json /tmp/quanta-index-fast-current.json

rust-timings-daemon-check:
    {{cargo}} --lane timings-daemon-lane clean --quiet
    {{cargo}} --lane timings-daemon-lane build -p quanta-index-searchd-runtime --all-features --locked --timings
    env QUANTA_INDEX_BUILD_LANE=timings-daemon-lane bash -lc 'source scripts/quanta-index-env.sh && python3 tools/ci/timing/summarize_cargo_timings.py "$CARGO_TARGET_DIR/cargo-timings/cargo-timing.html" --json --top-crates 25 > /tmp/quanta-index-daemon-current.json'
    python3 tools/ci/timing/compare_cargo_timings.py tools/ci/timing/baselines/daemon-lane.json /tmp/quanta-index-daemon-current.json

rust-timings-update-baselines:
    {{cargo}} --lane timings-fast-lane clean --quiet
    {{cargo}} --lane timings-fast-lane build --workspace --lib --bins --all-features --locked --exclude quanta-index-searchd-runtime --timings
    env QUANTA_INDEX_BUILD_LANE=timings-fast-lane bash -lc 'source scripts/quanta-index-env.sh && python3 tools/ci/timing/summarize_cargo_timings.py "$CARGO_TARGET_DIR/cargo-timings/cargo-timing.html" --json --top-crates 25 > tools/ci/timing/baselines/fast-lane.json'
    {{cargo}} --lane timings-daemon-lane clean --quiet
    {{cargo}} --lane timings-daemon-lane build -p quanta-index-searchd-runtime --all-features --locked --timings
    env QUANTA_INDEX_BUILD_LANE=timings-daemon-lane bash -lc 'source scripts/quanta-index-env.sh && python3 tools/ci/timing/summarize_cargo_timings.py "$CARGO_TARGET_DIR/cargo-timings/cargo-timing.html" --json --top-crates 25 > tools/ci/timing/baselines/daemon-lane.json'
    @echo "Baselines updated. Commit tools/ci/timing/baselines/."

rust-clippy:
    {{cargo}} --lane clippy-lane clippy --workspace --all-targets --all-features --locked -- -D warnings

rust-test:
    {{cargo}} --lane test-workspace-lane test --workspace --all-features --locked

# Local test loop that keeps unit and binary coverage but skips integration/E2E.
rust-test-fast:
    {{cargo}} --lane test-fast-lane test --workspace --lib --bins --all-features --locked --exclude quanta-index-searchd-runtime

rust-test-unit:
    {{cargo}} --lane test-fast-lane test --workspace --lib --bins --all-features --locked --exclude quanta-index-searchd-runtime

rust-test-integration:
    {{cargo}} --lane test-integration-lane test -p quanta-index-contract --test ipc_query_result_v2_contract --all-features --locked
    {{cargo}} --lane test-integration-lane test -p quanta-index-contract --test lex_scaffold --all-features --locked
    {{cargo}} --lane test-integration-lane test -p quanta-index-core --test hybrid_policy --all-features --locked
    {{cargo}} --lane test-integration-lane test -p quanta-index-core --test semantic_policy --all-features --locked
    {{cargo}} --lane test-integration-lane test -p quanta-index-channel --test hellgate --all-features --locked
    {{cargo}} --lane test-integration-lane test -p quanta-index-channel --test wal_roundtrip --all-features --locked
    {{cargo}} --lane test-integration-lane test -p quanta-index-lexical --test tantivy_smoke --all-features --locked
    {{cargo}} --lane test-integration-lane test -p quanta-index-repomap --test bootstrap_owner_flow --all-features --locked
    {{cargo}} --lane test-integration-lane test -p quanta-index-repomap --test owner_surface --all-features --locked

rust-test-cli-smoke:
    {{cargo}} --lane test-cli-smoke-lane test -p quanta-index-searchctl --test cli_smoke --all-features --locked
    {{cargo}} --lane test-cli-smoke-lane test -p quanta-index-conformance --test cli_smoke --all-features --locked

rust-test-e2e:
    {{cargo}} --lane test-daemon-lane test -p quanta-index-searchd-runtime --test dsl_scenarios --all-features --locked
    {{cargo}} --lane test-daemon-lane test -p quanta-index-searchd-runtime --test end_to_end --all-features --locked
    {{cargo}} --lane test-daemon-lane test -p quanta-index-searchd-runtime --test explain --all-features --locked
    {{cargo}} --lane test-daemon-lane test -p quanta-index-searchd-runtime --test repo_map_end_to_end --all-features --locked

rust-test-full-corpus:
    {{cargo}} --lane test-daemon-lane test -p quanta-index-searchd-runtime --test e2e_full_corpus --all-features --locked -- --nocapture

rust-test-pyramid:
    @just rust-test-unit
    @just rust-test-integration
    @just rust-test-cli-smoke
    @just rust-test-e2e

rust-doc:
    RUSTDOCFLAGS="-D warnings" {{cargo}} --lane doc-lane doc --workspace --all-features --no-deps --locked

rust-msrv:
    env QUANTA_INDEX_BUILD_LANE=msrv-lane bash -lc 'source scripts/quanta-index-env.sh && cargo +1.92.0 check --workspace --all-targets --all-features --locked'
    env QUANTA_INDEX_BUILD_LANE=msrv-lane bash -lc 'source scripts/quanta-index-env.sh && cargo +1.92.0 test --workspace --all-features --locked --no-run'

rust-bench:
    {{cargo}} --lane bench-lane bench --workspace --all-features --locked

rust-bench-build:
    {{cargo}} --lane bench-lane bench --workspace --all-features --locked --no-run

rust-machete:
    env QUANTA_INDEX_BUILD_LANE=machete-lane bash -lc 'source scripts/quanta-index-env.sh && cargo machete --with-metadata'

rust-miri:
    env QUANTA_INDEX_BUILD_LANE=miri-lane bash -lc 'source scripts/quanta-index-env.sh && RUSTUP_TOOLCHAIN=nightly cargo miri setup'
    env QUANTA_INDEX_BUILD_LANE=miri-lane bash -lc 'source scripts/quanta-index-env.sh && RUSTUP_TOOLCHAIN=nightly MIRIFLAGS="-Zmiri-strict-provenance" cargo miri test -p quanta-index-contract -p quanta-index-core --lib --all-features'

rust-careful:
    env QUANTA_INDEX_BUILD_LANE=careful-lane bash -lc 'source scripts/quanta-index-env.sh && RUSTUP_TOOLCHAIN=nightly cargo careful test --workspace --all-features --locked'

rust-tsan:
    env QUANTA_INDEX_BUILD_LANE=tsan-lane bash -lc 'source scripts/quanta-index-env.sh && RUSTUP_TOOLCHAIN=nightly RUSTFLAGS="-Zsanitizer=thread" cargo test -Z build-std --target $(rustc -vV | sed -n '"'"'s|host: ||p'"'"') --workspace --all-features --lib --tests'

rust-asan:
    env QUANTA_INDEX_BUILD_LANE=asan-lane bash -lc 'source scripts/quanta-index-env.sh && RUSTUP_TOOLCHAIN=nightly RUSTFLAGS="-Zsanitizer=address" cargo test -Z build-std --target $(rustc -vV | sed -n '"'"'s|host: ||p'"'"') --workspace --all-features --lib --tests'

rust-mutants:
    env QUANTA_INDEX_BUILD_LANE=mutants-lane bash -lc 'source scripts/quanta-index-env.sh && cargo mutants --package quanta-index-core --timeout 60 --baseline=skip --no-shuffle'

rust-udeps:
    env QUANTA_INDEX_BUILD_LANE=udeps-lane bash -lc 'source scripts/quanta-index-env.sh && RUSTUP_TOOLCHAIN=nightly cargo udeps --workspace --all-targets --all-features'

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
    env QUANTA_INDEX_BUILD_LANE=fuzz-contract-lane bash -lc 'source scripts/quanta-index-env.sh && cd crates/quanta-index-contract/fuzz && cargo +nightly fuzz build'
    env QUANTA_INDEX_BUILD_LANE=fuzz-ranker-lane bash -lc 'source scripts/quanta-index-env.sh && cd crates/quanta-index-lq-ranker/fuzz && cargo +nightly fuzz build'

rust-fuzz-smoke seconds="60":
    env QUANTA_INDEX_BUILD_LANE=fuzz-contract-lane bash -lc 'source scripts/quanta-index-env.sh && cd crates/quanta-index-contract/fuzz && cargo +nightly fuzz run ipc_request_decode -- -max_total_time={{seconds}}'
    env QUANTA_INDEX_BUILD_LANE=fuzz-contract-lane bash -lc 'source scripts/quanta-index-env.sh && cd crates/quanta-index-contract/fuzz && cargo +nightly fuzz run ipc_response_decode -- -max_total_time={{seconds}}'

rust-fuzz-ranker-smoke seconds="60":
    env QUANTA_INDEX_BUILD_LANE=fuzz-ranker-lane bash -lc 'source scripts/quanta-index-env.sh && cd crates/quanta-index-lq-ranker/fuzz && cargo +nightly fuzz run weights_hash_no_panic -- -max_total_time={{seconds}}'
    env QUANTA_INDEX_BUILD_LANE=fuzz-ranker-lane bash -lc 'source scripts/quanta-index-env.sh && cd crates/quanta-index-lq-ranker/fuzz && cargo +nightly fuzz run weights_hash_determinism -- -max_total_time={{seconds}}'
    env QUANTA_INDEX_BUILD_LANE=fuzz-ranker-lane bash -lc 'source scripts/quanta-index-env.sh && cd crates/quanta-index-lq-ranker/fuzz && cargo +nightly fuzz run composite_scorer_no_panic -- -max_total_time={{seconds}}'

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
