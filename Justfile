set shell := ["/bin/zsh", "-lc"]

cargo := "./scripts/cargow"
retrieval_python := "uv run --frozen --extra dev python"

default:
    @just --list

install-dev:
    python3 -m pip install -e '.[dev]'

install-hooks:
    python3 -m pre_commit install --install-hooks --hook-type pre-commit --hook-type pre-push

cache-root:
    @bash scripts/quanta-index-env.sh

# Prune idle/orphan Cargo target lanes and stale test executables.
# Examples: `just target-gc --dry-run`, `just target-gc --dry-run --json`,
# `just target-gc --idle-hours 48 --max-total-gb 200`.
target-gc *args:
    python3 tools/ci/target_gc.py {{args}}

rust-sccache-stats:
    @source scripts/quanta-index-env.sh && if [[ -n "${RUSTC_WRAPPER:-}" && "${RUSTC_WRAPPER:t}" == "sccache" ]]; then sccache --show-stats; else echo 'sccache is disabled or unavailable'; fi

rust-profile-list:
    @printf '%s\n' \
        'dev-fast            default local edit loop; workspace lib/bin compile, excludes searchd runtime' \
        'dev-daemon          searchd runtime only; use when editing daemon/runtime entrypoints' \
        'dev-all-targets     widest compile rail; all targets across the workspace' \
        'validate-shared-surface contract/core/sdk/search-plane shared-surface validation rail' \
        'test-fast           default local test loop; workspace lib/bin tests, excludes daemon e2e' \
        'test-canonical-identity P01A identity, codec, layout-security, and error-authority proof' \
        'test-p02a-repomap-compiler P02A whole-bundle graph compiler owner proof' \
        'test-p02b-operation-journal P02B global sequence authority and operation journal proof' \
        'test-candidate-activation-owner P03 sealed-candidate activation, recovery, and quarantine owner proof' \
        'test-read-view-lifetime-owner P04 read-view lifetime, pin/GC barrier owner proof' \
        'test-search-plane-read-view-lib P04 search-plane read-view lib proof' \
        'test-query-truth-owner P05 query outcome/cursor/oracle owner proof' \
        'test-control-readiness-owner P09 control authorization/readiness owner proof' \
        'test-control-readiness-owner-lib P09 control authorization/readiness lib suites' \
        'test-state-migration-owner P10 current-format backup/restore/verify and old-root refusal owner proof' \
        'test-state-migration-owner-lib P10 current-format offline state lib suites' \
        'test-query-truth-owner-lib P05 outcome/cursor/oracle lib suites' \
        'test-sdk-binding-owner P06 SDK contextual binding negative-matrix owner proof' \
        'test-sdk-binding-owner-lib P06 SDK lib suites' \
        'test-provider-boundary-owner P07 semantic admission / provider boundary owner proof' \
        'test-runtime-supervisor-owner P08 supervised runtime / bounded shutdown owner proof' \
        'test-integration-fast bounded integration loop; excludes slow text/Lance storage' \
        'test-integration-storage text-authority shard persistence slice' \
        'test-integration-semantic semantic storage integration slice' \
        'test-integration    complete fast + storage + semantic integration rail' \
        'test-cli-smoke      CLI smoke tests for searchctl + corpus-smoke' \
        'test-daemon-fast    9-source/1-binary daemon edit loop; excludes DSL cold truth' \
        'test-daemon         30 runtime sources plus DSL truth; 3 binaries' \
        'test-daemon-all     49 runtime sources plus DSL truth; 4 binaries' \
        'release-cli        optimized release build for quanta-index-searchctl' \
        'release-cli-fresh  clean release-bin lane, then rebuild quanta-index-searchctl' \
        'release-daemon     optimized release build for quanta-index-searchd daemon' \
        'release-daemon-fresh clean release-daemon lane, then rebuild quanta-index-searchd daemon' \
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
        test-canonical-identity) ./scripts/run-rust-profile.sh "{{profile}}" rust-test-canonical-identity ;; \
        test-p02a-repomap-compiler) ./scripts/run-rust-profile.sh "{{profile}}" rust-test-p02a-repomap-compiler ;; \
        test-p02b-operation-journal) ./scripts/run-rust-profile.sh "{{profile}}" rust-test-p02b-operation-journal ;; \
        test-candidate-activation-owner) ./scripts/run-rust-profile.sh "{{profile}}" rust-test-candidate-activation-owner ;; \
        test-read-view-lifetime-owner) ./scripts/run-rust-profile.sh "{{profile}}" rust-test-read-view-lifetime-owner ;; \
        test-search-plane-read-view-lib) ./scripts/run-rust-profile.sh "{{profile}}" rust-test-search-plane-read-view-lib ;; \
        test-query-truth-owner) ./scripts/run-rust-profile.sh "{{profile}}" rust-test-query-truth-owner ;; \
        test-control-readiness-owner) ./scripts/run-rust-profile.sh "{{profile}}" rust-test-control-readiness-owner ;; \
        test-control-readiness-owner-lib) ./scripts/run-rust-profile.sh "{{profile}}" rust-test-control-readiness-owner-lib ;; \
        test-state-migration-owner) ./scripts/run-rust-profile.sh "{{profile}}" rust-test-state-migration-owner ;; \
        test-state-migration-owner-lib) ./scripts/run-rust-profile.sh "{{profile}}" rust-test-state-migration-owner-lib ;; \
        test-query-truth-owner-lib) ./scripts/run-rust-profile.sh "{{profile}}" rust-test-query-truth-owner-lib ;; \
        test-sdk-binding-owner) ./scripts/run-rust-profile.sh "{{profile}}" rust-test-sdk-binding-owner ;; \
        test-sdk-binding-owner-lib) ./scripts/run-rust-profile.sh "{{profile}}" rust-test-sdk-binding-owner-lib ;; \
        test-provider-boundary-owner) ./scripts/run-rust-profile.sh "{{profile}}" rust-test-provider-boundary-owner ;; \
        test-runtime-supervisor-owner) ./scripts/run-rust-profile.sh "{{profile}}" rust-test-runtime-supervisor-owner ;; \
        test-integration-fast) ./scripts/run-rust-profile.sh "{{profile}}" rust-test-integration-fast ;; \
        test-integration-storage) ./scripts/run-rust-profile.sh "{{profile}}" rust-test-integration-storage ;; \
        test-integration-semantic) ./scripts/run-rust-profile.sh "{{profile}}" rust-test-integration-semantic ;; \
        test-integration) ./scripts/run-rust-profile.sh "{{profile}}" rust-test-integration ;; \
        test-cli-smoke) ./scripts/run-rust-profile.sh "{{profile}}" rust-test-cli-smoke ;; \
        test-daemon-fast) ./scripts/run-rust-profile.sh "{{profile}}" rust-test-e2e-fast ;; \
        test-daemon) ./scripts/run-rust-profile.sh "{{profile}}" rust-test-e2e ;; \
        test-daemon-all) ./scripts/run-rust-profile.sh "{{profile}}" rust-test-e2e-all ;; \
        release-cli) ./scripts/run-rust-profile.sh "{{profile}}" rust-build-release-cli ;; \
        release-cli-fresh) ./scripts/run-rust-profile.sh "{{profile}}" rust-build-release-cli-fresh ;; \
        release-daemon) ./scripts/run-rust-profile.sh "{{profile}}" rust-build-release-daemon ;; \
        release-daemon-fresh) ./scripts/run-rust-profile.sh "{{profile}}" rust-build-release-daemon-fresh ;; \
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

rust-check lane="all-targets-lane":
    {{cargo}} --lane {{lane}} check --workspace --all-targets --all-features --locked

# Local edit/compile loop without integration tests, examples, or benches.
rust-check-fast:
    {{cargo}} --lane fast-lane check --workspace --lib --bins --all-features --locked --exclude quanta-index-searchd-runtime

rust-check-daemon:
    {{cargo}} --lane daemon-lane check -p quanta-index-searchd-runtime --all-features --locked

rust-compile-shared-surface lane="shared-validation-lane":
    {{cargo}} --lane {{lane}} nextest run -p quanta-index-contract -p quanta-index-core -p quanta-index-sdk -p quanta-index-search-plane -p quanta-index-ipc --all-targets --all-features --locked --no-run

rust-validate-shared-surface:
    @just rust-compile-shared-surface shared-validation-lane
    python3 tools/ci/run-local-test-scope.py shared-surface --lane shared-validation-lane
    python3 tools/ci/run-local-test-scope.py integration-fast --lane shared-validation-lane
    python3 tools/ci/run-local-test-scope.py cli-smoke --lane shared-validation-lane

# Local build loop without integration tests or benches.
rust-build-fast:
    {{cargo}} --lane fast-lane build --workspace --lib --bins --all-features --locked --exclude quanta-index-searchd-runtime

rust-build-daemon:
    {{cargo}} --lane daemon-lane build -p quanta-index-searchd-runtime --all-features --locked

# Canonical deployable CLI artifact. Uses a dedicated release lane so optimized
# outputs do not share fingerprints with edit-loop targets.
rust-build-release-cli:
    {{cargo}} --lane release-bin-lane build -p quanta-index-searchctl --bin quanta-index-searchctl --all-features --locked --release

# Fresh release build for stale-fingerprint recovery after source-layout cuts.
rust-build-release-cli-fresh:
    {{cargo}} --lane release-bin-lane clean --quiet
    {{cargo}} --lane release-bin-lane build -p quanta-index-searchctl --bin quanta-index-searchctl --all-features --locked --release

# Canonical deployable daemon artifact. The bin lives in
# quanta-index-searchd-runtime, not the quanta-index-searchd library crate.
rust-build-release-daemon:
    {{cargo}} --lane release-daemon-bin-lane build -p quanta-index-searchd-runtime --bin quanta-index-searchd --all-features --locked --release

# Fresh daemon release build for stale-fingerprint recovery after source-layout
# cuts (for example semantic backend module replacement).
rust-build-release-daemon-fresh:
    {{cargo}} --lane release-daemon-bin-lane clean --quiet
    {{cargo}} --lane release-daemon-bin-lane build -p quanta-index-searchd-runtime --bin quanta-index-searchd --all-features --locked --release

# Full compile rail for every Rust target, including integration tests and benches.
rust-build-all-targets:
    {{cargo}} --lane all-targets-lane build --workspace --all-targets --all-features --locked

rust-timings-fast:
    python3 tools/ci/timing/check_host_contention.py
    {{cargo}} --lane timings-fast-lane build --workspace --lib --bins --all-features --locked --exclude quanta-index-searchd-runtime --timings
    env QUANTA_INDEX_BUILD_LANE=timings-fast-lane bash -lc 'source scripts/quanta-index-env.sh && python3 tools/ci/timing/summarize_cargo_timings.py "$CARGO_TARGET_DIR/cargo-timings/cargo-timing.html"'

rust-timings-daemon:
    python3 tools/ci/timing/check_host_contention.py
    {{cargo}} --lane timings-daemon-lane build -p quanta-index-searchd-runtime --all-features --locked --timings
    env QUANTA_INDEX_BUILD_LANE=timings-daemon-lane bash -lc 'source scripts/quanta-index-env.sh && python3 tools/ci/timing/summarize_cargo_timings.py "$CARGO_TARGET_DIR/cargo-timings/cargo-timing.html"'

rust-timings-all-targets:
    python3 tools/ci/timing/check_host_contention.py
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
    python3 tools/ci/timing/check_host_contention.py
    {{cargo}} --lane timings-fast-lane clean --quiet
    {{cargo}} --lane timings-fast-lane build --workspace --lib --bins --all-features --locked --exclude quanta-index-searchd-runtime --timings
    env QUANTA_INDEX_BUILD_LANE=timings-fast-lane bash -lc 'source scripts/quanta-index-env.sh && python3 tools/ci/timing/summarize_cargo_timings.py "$CARGO_TARGET_DIR/cargo-timings/cargo-timing.html" --json --top-crates 25 > /tmp/quanta-index-fast-current.json'
    python3 tools/ci/timing/compare_cargo_timings.py tools/ci/timing/baselines/fast-lane.json /tmp/quanta-index-fast-current.json

rust-timings-daemon-check:
    python3 tools/ci/timing/check_host_contention.py
    {{cargo}} --lane timings-daemon-lane clean --quiet
    {{cargo}} --lane timings-daemon-lane build -p quanta-index-searchd-runtime --all-features --locked --timings
    env QUANTA_INDEX_BUILD_LANE=timings-daemon-lane bash -lc 'source scripts/quanta-index-env.sh && python3 tools/ci/timing/summarize_cargo_timings.py "$CARGO_TARGET_DIR/cargo-timings/cargo-timing.html" --json --top-crates 25 > /tmp/quanta-index-daemon-current.json'
    python3 tools/ci/timing/compare_cargo_timings.py tools/ci/timing/baselines/daemon-lane.json /tmp/quanta-index-daemon-current.json

rust-timings-update-baselines:
    python3 tools/ci/timing/check_host_contention.py
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

rust-test-canonical-identity lane="test-canonical-identity-lane":
    python3 tools/ci/run-local-test-scope.py canonical-identity --lane {{lane}}

rust-test-p02a-repomap-compiler lane="test-p02a-repomap-compiler-lane":
    python3 tools/ci/run-local-test-scope.py p02a-repomap-compiler --lane {{lane}}

rust-test-p02b-operation-journal lane="test-p02b-operation-journal-lane":
    python3 tools/ci/run-local-test-scope.py p02b-operation-journal --lane {{lane}}

rust-test-candidate-activation-owner lane="test-candidate-activation-owner-lane":
    python3 tools/ci/run-local-test-scope.py candidate-activation-owner --lane {{lane}}

rust-test-read-view-lifetime-owner lane="test-read-view-lifetime-owner-lane":
    python3 tools/ci/run-local-test-scope.py read-view-lifetime-owner --lane {{lane}}

rust-test-search-plane-read-view-lib lane="test-search-plane-read-view-lane":
    python3 tools/ci/run-local-test-scope.py search-plane-read-view-lib --lane {{lane}}

rust-test-query-truth-owner lane="test-query-truth-owner-lane":
    python3 tools/ci/run-local-test-scope.py query-truth-owner --lane {{lane}}

rust-test-query-truth-owner-lib lane="test-query-truth-owner-lane":
    python3 tools/ci/run-local-test-scope.py query-truth-owner-lib --lane {{lane}}

rust-test-control-readiness-owner lane="test-control-readiness-owner-lane":
    python3 tools/ci/run-local-test-scope.py control-readiness-owner --lane {{lane}}

rust-test-control-readiness-owner-lib lane="test-control-readiness-owner-lane":
    QUANTA_INDEX_TEST_POTION_CODE_MODEL_DIR="${QUANTA_INDEX_TEST_POTION_CODE_MODEL_DIR:-$(./scripts/quanta-index-env.sh)/models/potion-code-16M-v2-e9d2a44}" python3 tools/ci/run-local-test-scope.py control-readiness-owner-lib --lane {{lane}} --run-ignored all

rust-test-state-migration-owner lane="test-state-migration-owner-lane":
    python3 tools/ci/run-local-test-scope.py state-migration-owner --lane {{lane}}

rust-test-state-migration-owner-lib lane="test-state-migration-owner-lane":
    QUANTA_INDEX_TEST_POTION_CODE_MODEL_DIR="${QUANTA_INDEX_TEST_POTION_CODE_MODEL_DIR:-$(./scripts/quanta-index-env.sh)/models/potion-code-16M-v2-e9d2a44}" python3 tools/ci/run-local-test-scope.py state-migration-owner-lib --lane {{lane}} --run-ignored all

rust-test-sdk-binding-owner lane="test-sdk-binding-owner-lane":
    python3 tools/ci/run-local-test-scope.py sdk-binding-owner --lane {{lane}}

rust-test-sdk-binding-owner-lib lane="test-sdk-binding-owner-lane":
    python3 tools/ci/run-local-test-scope.py sdk-binding-owner-lib --lane {{lane}}

rust-test-provider-boundary-owner lane="test-provider-boundary-owner-lane":
    python3 tools/ci/run-local-test-scope.py provider-boundary-owner --lane {{lane}}

rust-test-runtime-supervisor-owner lane="test-runtime-supervisor-owner-lane":
    python3 tools/ci/run-local-test-scope.py runtime-supervisor-owner --lane {{lane}}

rust-test-integration-fast lane="test-integration-lane":
    python3 tools/ci/run-local-test-scope.py integration-fast --lane {{lane}}

rust-test-integration-storage lane="test-integration-lane":
    python3 tools/ci/run-local-test-scope.py integration-storage --lane {{lane}}

rust-test-integration-semantic lane="test-integration-lane":
    python3 tools/ci/run-local-test-scope.py integration-semantic --lane {{lane}}

rust-test-integration lane="test-integration-lane":
    python3 tools/ci/run-local-test-scope.py integration-fast --lane {{lane}}
    python3 tools/ci/run-local-test-scope.py integration-storage --lane {{lane}}
    python3 tools/ci/run-local-test-scope.py integration-semantic --lane {{lane}}

# W0 decision-gate probes (G0-L / G0-S / G0-R). Vendor- and transport-capability
# evidence for the structural remediation gates; `--nocapture` so the
# G0*-EVIDENCE lines land in the run log the ADRs cite. The G0-C probe crate
# was deleted when the catalog adapter (`quanta-index-catalog`) landed; its
# evidence is recorded in the G0-C ADR.
rust-w0-storage-gates:
    {{cargo}} --lane test-integration-lane test -p quanta-index-lexical --test g0l_tantivy_snapshot_probe --all-features --locked -- --nocapture
    {{cargo}} --lane test-integration-lane test -p quanta-index-semantic --test g0s_lance_snapshot_probe --all-features --locked -- --nocapture
    {{cargo}} --lane test-integration-lane test -p quanta-index-ipc --test g0r_runtime_cancellation_probe --all-features --locked -- --nocapture

rust-test-cli-smoke lane="test-cli-smoke-lane":
    python3 tools/ci/run-local-test-scope.py cli-smoke --lane {{lane}}

rust-bench-dsl-truth:
    {{cargo}} --lane test-daemon-lane test -p quanta-index-searchd-harness --test dsl_scenario_truth --all-features --locked -- --nocapture

rust-verify-hellgate-fast:
    @just rust-bench-dsl-truth
    {{cargo}} --lane test-daemon-lane test -p quanta-index-search-plane --lib --all-features --locked -- --nocapture
    {{cargo}} --lane test-daemon-lane nextest run -p quanta-index-searchd-runtime --test runtime_fast_suite --all-features --locked -E 'test(/^(e2e_text_route_hellgate|e2e_structural_hellgate)::/)' --success-output final
    python3 tools/benchmark/sourcegraph_parity.py --check
    python3 tools/ci/lint/check-dsl-capability-truth.py

rust-verify-hellgate-broad:
    {{cargo}} --lane test-daemon-lane nextest run -p quanta-index-searchd-runtime --test runtime_fast_suite --test runtime_risk_suite --all-features --locked -E 'test(/^(dsl_scenarios|sdk_frontdoor|end_to_end|e2e_restart_replay_determinism|e2e_perf_chaos|explain|repo_map_end_to_end|e2e_full_corpus)::/)' --success-output final

rust-verify-hellgate-cross-repo semantica_root="/Users/songmin/Documents/code-new/semantica-codegraph-v2":
    ./scripts/verify-repomap-cross-repo.sh "{{semantica_root}}"

rust-verify-hellgate-all samples="20":
    @just rust-verify-hellgate-fast
    @just rust-verify-hellgate-broad
    @just rust-bench-dsl-warm
    @just rust-bench-dsl-cold {{samples}}
    @just rust-bench-dsl-compare

rust-test-e2e-fast lane="test-daemon-lane":
    python3 tools/ci/run-local-test-scope.py daemon-fast --lane {{lane}}

rust-test-e2e lane="test-daemon-lane":
    python3 tools/ci/run-local-test-scope.py daemon --lane {{lane}}

rust-test-e2e-all lane="test-daemon-lane":
    python3 tools/ci/run-local-test-scope.py daemon-all --lane {{lane}}

rust-test-full-corpus:
    {{cargo}} --lane test-daemon-lane nextest run -p quanta-index-searchd-runtime --test runtime_risk_suite --all-features --locked -E 'test(/^e2e_full_corpus::/)' --success-output final

rust-test-pyramid:
    @just rust-test-unit
    @just rust-test-integration
    @just rust-test-cli-smoke
    @just rust-test-e2e

rust-doc:
    RUSTDOCFLAGS="-D warnings" {{cargo}} --lane doc-lane doc --workspace --all-features --no-deps --locked

rust-msrv:
    RUSTUP_TOOLCHAIN=1.92.0 {{cargo}} --lane msrv-lane test --workspace --all-targets --all-features --locked --no-run

rust-bench:
    {{cargo}} --lane bench-lane bench --workspace --all-features --locked

rust-bench-build:
    {{cargo}} --lane bench-lane bench --workspace --all-features --locked --no-run

# Machine-checkable benchmark control-plane policy: registry validity, every
# Cargo bench target registered, no production->benchmark dependency, and no CI
# direct producer/comparator bypass. Cheap but requires cargo metadata.
benchmark-policy-local:
    python3 tools/ci/lint/check-benchmark-policy.py --print-registry-digest

# Print the resolved, digest-bound plan for one registered profile (no mutation).
benchmark-plan profile:
    python3 tools/benchmark/benchctl.py plan {{profile}}

# Fresh-process re-validation of one immutable run (run id or run directory).
benchmark-replay run:
    python3 tools/benchmark/benchctl.py replay {{run}}

# Fast local PREP check for the shared benchmark control plane. Retrieval
# contracts have their own local/proof rails and are not run here a second time.
# This does not produce a benchmark artifact, enter the timing preflight, or
# run an end-to-end quality recipe:
# those require a clean source and (for timing authority) a quiet canonical
# Linux host. The shared test-daemon lane compiles harness binaries without
# running their tests, then runs the harness library tests.
# Python capture/custody contracts only: no service, model download or timing
# producer. Invoke through `uv run --frozen --extra dev just` for locked deps.
benchmark-control-contract-local:
    uv run --frozen --extra dev python -m pytest tools/ci/tests/test_live_lexical_external.py tools/ci/tests/test_code_search_workflow.py tools/ci/tests/test_code_search_matrix.py tools/ci/tests/test_benchmark_manifest.py tools/ci/tests/test_benchmark_policy.py tools/ci/tests/test_bench_protocol_conformance.py tools/ci/tests/test_benchmark_evidence_bridge.py tools/ci/tests/test_benchmark_source_closure.py tools/ci/tests/test_benchctl.py tools/ci/tests/test_benchmark_profile_capture.py tools/ci/tests/test_criterion_capture.py tools/ci/tests/test_recorded_capture.py tools/ci/tests/test_retrieval_capture.py tools/ci/tests/test_lexical_capture.py tools/ci/tests/test_pair_capture.py tools/ci/tests/test_lexical_file_comparison.py tools/ci/tests/test_lexical_five_product_oracle.py tools/ci/tests/test_identifier_robustness_report.py tools/ci/tests/test_identifier_robustness_strata.py tools/ci/tests/test_codesearchnet_qrels.py tools/ci/tests/test_codesearchnet_materialize.py tools/ci/tests/test_clarc_adapter.py tools/ci/tests/test_external_snippet_benchmark.py tools/ci/tests/test_holdout_c4_projection.py tools/ci/tests/test_identifier_robustness_fresh_join.py tools/ci/tests/test_identifier_robustness_multiproduct_report.py tools/ci/tests/test_portable_proof.py tools/ci/tests/test_check_bench_artifacts.py tools/ci/tests/test_check_host_contention.py tools/ci/tests/test_compare_dsl_bench.py tools/ci/tests/test_quality_integration_summary.py tools/ci/tests/test_retrieval_contract_proof.py tools/ci/tests/test_retrieval_sdk_proof.py tools/ci/tests/test_write_verification_receipt.py tools/ci/tests/test_agent_outcome_benchmark.py tools/ci/tests/test_concurrency_sample_contract.py tools/ci/tests/test_corpus_release.py tools/ci/tests/test_tool_custody.py tools/ci/tests/test_portable_tool_execution.py tools/ci/tests/test_conditional_window_operations.py tools/ci/tests/test_conditional_tool_execution.py tools/ci/tests/test_corpus_binding.py tools/ci/tests/test_gold_oracle.py tools/ci/tests/test_nextest_ignored_inventory.py tools/ci/tests/test_producer_notifications.py tools/ci/tests/test_bootstrap_cache.py tools/ci/tests/test_proof_command_timings.py tools/ci/tests/test_resource_admission.py tools/ci/tests/test_cargow_resource_admission.py tools/ci/tests/test_cargo_preparation.py tools/ci/tests/test_pair_replay_workspace.py -q

benchmark-prep-local:
    find crates/quanta-index-searchd-harness/src -name '*.rs' -print0 | xargs -0 rustfmt --check --edition 2024
    uv run --frozen --extra dev just benchmark-control-contract-local
    uv run --frozen --extra dev python -m py_compile tools/benchmark/benchctl.py tools/benchmark/code_search_workflow.py tools/benchmark/registry.py tools/benchmark/manifest.py tools/benchmark/evidence.py tools/benchmark/evidence_bridge.py tools/benchmark/compare_dsl_bench.py tools/benchmark/quality_integration_summary.py tools/ci/lint/check-bench-artifacts.py tools/ci/lint/check-benchmark-policy.py tools/ci/timing/check_host_contention.py tools/benchmark/retrieval/evaluator.py tools/benchmark/retrieval/holdout_review.py tools/benchmark/retrieval/source_oracle.py tools/benchmark/retrieval/source_oracle_suite.py tools/benchmark/retrieval/identifier_robustness_suite.py tools/benchmark/retrieval/identifier_robustness_report.py tools/benchmark/retrieval/codesearchnet_qrels.py tools/benchmark/retrieval/codesearchnet_materialize.py tools/benchmark/retrieval/clarc_adapter.py tools/benchmark/retrieval/external_snippet_benchmark.py tools/benchmark/retrieval/identifier_robustness_multiproduct_report.py tools/benchmark/retrieval/identifier_robustness_fresh_join.py tools/benchmark/retrieval/arb_adapter.py tools/benchmark/retrieval/query_plan.py tools/benchmark/retrieval/retrieval_contract.py tools/benchmark/retrieval/semble.py tools/benchmark/retrieval/run.py tools/benchmark/retrieval/lexical_file_comparison.py tools/benchmark/retrieval/lexical_external_oracle.py tools/benchmark/retrieval/lexical_five_product_oracle.py tools/benchmark/retrieval/prepare_lexical_pair.py tools/benchmark/retrieval/contract_proof.py tools/benchmark/retrieval/sdk_proof.py tools/ci/source_closure.py tools/ci/write-verification-receipt.py
    {{cargo}} --lane bench-lane test -p quanta-index-bench-protocol --all-features --locked
    {{cargo}} --lane test-daemon-lane test -p quanta-index-searchd-harness --lib --bins --all-features --locked --no-run
    {{cargo}} --lane test-daemon-lane test -p quanta-index-searchd-harness --lib --all-features --locked
    python3 tools/ci/lint/check-test-authority.py
    git diff --check

# Dirty-checkout retrieval edit loop. This is diagnostic only: use the proof
# rail on a clean source to produce source-bound JUnit/nextest receipts.
retrieval-contract-local:
    uv run --frozen --extra dev python -m pytest tools/ci/tests/test_retrieval_benchmark.py tools/ci/tests/test_source_oracle_suite.py tools/ci/tests/test_holdout_review.py tools/ci/tests/test_completed_response_timing.py -q
    {{cargo}} --lane test-daemon-lane test -p quanta-index-retrieval-bench --lib --bin quanta-index-retrieval-bench --test chunking_contract --test l5_parser_regressions --all-features --locked

# Retrieval benchmark: real-daemon SDK proof (T05-T07, T10). Builds the
# pinned searchd + runner binaries first, then runs the live roundtrip and
# Public canonical route: context-bound schema-v2 receipt plus raw evidence.
# Pass a fresh artifact root outside the checkout; no model download occurs.
# pair-spec.receipts maps sdk_execution_context/source_closure to this root;
# the driver freezes sibling command logs automatically.
retrieval-sdk-proof $out:
    uv run --frozen --extra dev python tools/benchmark/retrieval/portable_proof.py run --rail sdk --out "$out"

# Fresh isolated release build with bound source closure and executable digests.
retrieval-sdk-proof-fresh $out:
    uv run --frozen --extra dev python tools/benchmark/retrieval/portable_proof.py run --rail sdk --build-profile release-fresh --out "$out"

# Public canonical route: context-bound Python and Rust schema-v2 receipts.
# Pass a fresh artifact root outside the checkout.
# pair-spec.receipts maps contract_execution_context/source_closure to this root;
# the driver freezes sibling command logs automatically.
retrieval-contract-proof $out:
    uv run --frozen --extra dev python tools/benchmark/retrieval/portable_proof.py run --rail contract --out "$out"

# Retrieval benchmark: Quanta-only chunk A/B from a pinned spec file.
# The spec names repo/manifest/suite/pack, strategies, binaries and output
# root; captures stay outside the checkout. See RB-05 for the spec schema.
retrieval-quanta $spec:
    {{retrieval_python}} tools/benchmark/retrieval/run.py quanta --spec "$spec"

# Retrieval benchmark: sequential Quanta + Semble paired capture, merge and
# scoring from a pinned spec. Fails closed when the pinned Semble python is
# absent; installs and model caches stay outside the checkout.
retrieval-pair $spec:
    {{retrieval_python}} tools/benchmark/retrieval/run.py pair --spec "$spec"

# Retrieval benchmark: re-score immutable records into the TEST-PLAN §8
# verdict artifact (deterministic re-score path, T13).
retrieval-verdict repo suite run_manifest out:
    {{retrieval_python}} tools/benchmark/retrieval/run.py verdict --repo {{repo}} --suite {{suite}} --run-manifest {{run_manifest}} --out {{out}}

# Retrieval benchmark: host check-record (identity, load, thermal/frequency).
retrieval-host-probe out="":
    {{retrieval_python}} tools/benchmark/retrieval/run.py host-probe {{if out != "" { "--out " + out } else { "" } }}

# Layer-3 DSL query-latency matrix. Architecture:
# docs/adr/JUN-08-001-verification-hellgate-and-benchmark-separation.md.
# Current measurement contract: tools/benchmark/README.md.
# Warm: in-process criterion + p50/p95/p99 artifact. Cold: fresh-process-per-sample runner.
rust-bench-dsl-warm:
    mkdir -p artifacts/dsl-bench
    env CARGO_NET_OFFLINE=true {{cargo}} --lane bench-lane build -p quanta-index-searchd-harness --bin dsl_warm_matrix --profile bench --quiet --locked
    env QUANTA_INDEX_BUILD_LANE=bench-lane QUANTA_INDEX_BENCH_DISABLE_QUERY_OBS=1 bash -lc 'source scripts/quanta-index-env.sh && BIN="$CARGO_TARGET_DIR/release/dsl_warm_matrix" && test -x "$BIN" && "$BIN" --out "$(pwd)/artifacts/dsl-bench/warm-matrix.json"'

# Exploratory criterion view for warm scenarios. Not gate authority.
rust-bench-dsl-warm-criterion $evidence_root:
    python3 tools/benchmark/benchctl.py run dsl-diagnostic --evidence-root "$evidence_root"

rust-bench-dsl-cold samples="20":
    mkdir -p artifacts/dsl-bench
    env QUANTA_INDEX_BENCH_DISABLE_QUERY_OBS=1 python3 tools/benchmark/run_dsl_cold_matrix.py --samples {{samples}} --out artifacts/dsl-bench/cold-matrix.json

# Authority refresh: run warm and cold producers serially, then gate against baselines.
rust-bench-dsl-refresh samples="20":
    python3 tools/benchmark/benchctl.py run dsl-authority --cold-samples {{samples}}

# Relative-regression gate. It fails typed until reviewed canonical baselines exist.
rust-bench-dsl-compare:
    python3 tools/benchmark/compare_dsl_bench.py tools/benchmark/baselines/warm-matrix.json artifacts/dsl-bench/warm-matrix.json
    python3 tools/benchmark/compare_dsl_bench.py tools/benchmark/baselines/cold-matrix.json artifacts/dsl-bench/cold-matrix.json

# Sourcegraph filter parity: regenerate the execution-coverage matrix and gate it.
rust-bench-dsl-parity:
    python3 tools/benchmark/sourcegraph_parity.py --check --write

# --------------------------------------------------------------------------
# Search product quality rails (docs/plans/jun-7-search-product-quality).
# One command proves exactly one quality dimension; see MEASUREMENT_MATRIX.md
# and COMMAND_AND_ARTIFACT_CONTRACT.md. Blocking unless a recipe says advisory.
# --------------------------------------------------------------------------

# Relevance ranking rail (J7Q-01A). Blocking dimension: relevance.
# Proves: pure-metric unit tests + adversarial gate tests, then runs the judged
# seeded corpus end to end and enforces per-route MRR@10 / NDCG@10 / Recall@20
# plus top1 / top-k-containment / hard-negative ordering invariants.
# Artifacts: artifacts/search-quality/relevance/latest/{summary,query_judgments,sourcegraph-overlap}.json
# External lexical floor (Sourcegraph overlap, J7Q-01B) is emitted as
# `unprovisioned`, NOT a pass, until a local Sourcegraph instance exists.
rust-verify-quality-relevance:
    {{cargo}} --lane test-daemon-lane test -p quanta-index-searchd-harness --lib relevance:: --all-features --locked -- --nocapture
    mkdir -p artifacts/search-quality/relevance/latest
    {{cargo}} --lane test-daemon-lane build -p quanta-index-searchd-harness --bin relevance_matrix --all-features --locked
    env QUANTA_INDEX_EMBEDDER=potion-code QUANTA_INDEX_BUILD_LANE=test-daemon-lane QUANTA_QUALITY_CAPTURE_DATE="$(date -u +%Y-%m-%d)" bash -lc 'source scripts/quanta-index-env.sh && BIN="$CARGO_TARGET_DIR/debug/relevance_matrix" && test -x "$BIN" && "$BIN" --out-dir "$(pwd)/artifacts/search-quality/relevance/latest"'

# Hermetic mechanical regression rail. This is not learned semantic quality and
# cannot replace the default potion-code relevance artifact.
rust-verify-quality-relevance-hash-dev:
    {{cargo}} --lane test-daemon-lane build -p quanta-index-searchd-harness --bin relevance_matrix --all-features --locked
    mkdir -p artifacts/search-quality/relevance/hash-dev/latest
    env QUANTA_INDEX_EMBEDDER=hash-dev QUANTA_INDEX_BUILD_LANE=test-daemon-lane QUANTA_QUALITY_CAPTURE_DATE="$(date -u +%Y-%m-%d)" bash -lc 'source scripts/quanta-index-env.sh && BIN="$CARGO_TARGET_DIR/debug/relevance_matrix" && test -x "$BIN" && "$BIN" --out-dir "$(pwd)/artifacts/search-quality/relevance/hash-dev/latest"'

# Local semantic A/B capture. Advisory only: compares deterministic Hash against
# env-resolved OpenAI on the semantic judged fixture and writes delta artifacts
# plus request-shaping telemetry; this is NOT a blocking CI rail.
# Requires: OPENAI_API_KEY. Optional env knobs: QUANTA_INDEX_EMBED_MODEL,
# QUANTA_INDEX_EMBED_DIM, QUANTA_INDEX_EMBED_BATCH,
# QUANTA_INDEX_EMBED_MAX_EST_TOKENS, QUANTA_INDEX_EMBED_MAX_RETRIES,
# QUANTA_INDEX_EMBED_TIMEOUT_SECS, QUANTA_INDEX_EMBED_CACHE.
# Artifacts: artifacts/search-quality/relevance/openai-ab/latest/{summary,cases,provider-stats}.json
rust-capture-quality-relevance-openai-ab:
    bash -lc 'test -n "${OPENAI_API_KEY:-}" || { echo "OPENAI_API_KEY is required"; exit 1; }'
    {{cargo}} --lane test-daemon-lane test -p quanta-index-searchd-harness --lib relevance:: --all-features --locked -- --nocapture
    mkdir -p artifacts/search-quality/relevance/openai-ab/latest
    {{cargo}} --lane test-daemon-lane build -p quanta-index-searchd-harness --bin relevance_openai_ab --all-features --locked
    env QUANTA_INDEX_EMBEDDER=openai QUANTA_INDEX_BUILD_LANE=test-daemon-lane bash -lc 'source scripts/quanta-index-env.sh && BIN="$CARGO_TARGET_DIR/debug/relevance_openai_ab" && test -x "$BIN" && "$BIN" --out-dir "$(pwd)/artifacts/search-quality/relevance/openai-ab/latest"'

# Ambiguity / repairability rail (J7Q-06). Blocking dimension: ambiguity.
# Proves: typed repair-payload invariants — repairable bridge codes carry
# non-empty supported alternatives + a docs anchor; the internal invariant-break
# code carries none; classes stay distinct families; payloads round-trip the
# wire codec. No silent rewrite: repair is advisory metadata on a failing code.
# Artifacts: artifacts/search-quality/ambiguity/latest/{summary,error_payloads}.json
rust-verify-quality-ambiguity:
    {{cargo}} --lane test-daemon-lane test -p quanta-index-searchd-harness --lib ambiguity:: --all-features --locked -- --nocapture
    mkdir -p artifacts/search-quality/ambiguity/latest
    {{cargo}} --lane test-daemon-lane build -p quanta-index-searchd-harness --bin ambiguity_matrix --all-features --locked
    env QUANTA_INDEX_BUILD_LANE=test-daemon-lane bash -lc 'source scripts/quanta-index-env.sh && BIN="$CARGO_TARGET_DIR/debug/ambiguity_matrix" && test -x "$BIN" && "$BIN" --out-dir "$(pwd)/artifacts/search-quality/ambiguity/latest"'

# Snippet quality rail (J7Q-02). Blocking dimension: snippet.
# Proves: pure window-oracle unit tests + adversarial gate tests (the gate can go
# RED), then seeds the phrase/regex/multi-hit/long-line fixture and grades the
# engine's emitted snippet against hit-centered-window + bounded-length +
# deterministic-truncation (NOT substring presence). The snippet is graded
# as-emitted; the rail never post-processes engine output to force a pass.
# Artifacts: artifacts/search-quality/snippet/latest/{summary,golden_windows}.json
rust-verify-quality-snippet:
    {{cargo}} --lane test-daemon-lane test -p quanta-index-searchd-harness --lib snippet:: --all-features --locked -- --nocapture
    mkdir -p artifacts/search-quality/snippet/latest
    {{cargo}} --lane test-daemon-lane build -p quanta-index-searchd-harness --bin snippet_matrix --all-features --locked
    env QUANTA_INDEX_BUILD_LANE=test-daemon-lane bash -lc 'source scripts/quanta-index-env.sh && BIN="$CARGO_TARGET_DIR/debug/snippet_matrix" && test -x "$BIN" && "$BIN" --out-dir "$(pwd)/artifacts/search-quality/snippet/latest"'

# Scale-tier rail (J7Q-03). Blocking dimension: scale.
# Proves: deterministic seeded-corpus generator + tier-manifest invariants, then
# measures the SMALL tier end to end (ingest -> seal -> activate -> query),
# capturing ingest/open/query wall-times. Medium/large/xlarge are emitted as
# `declared-advisory`: their blocking latency is owned by the canonical Linux
# perf runner, not this host. An empty/typed-error small-tier query is a non-zero
# exit, never a fabricated zero-latency pass.
# Artifacts: artifacts/search-quality/scale/latest/{summary,tier_manifest}.json
rust-verify-quality-scale:
    python3 tools/ci/timing/check_host_contention.py
    {{cargo}} --lane test-daemon-lane test -p quanta-index-searchd-harness --lib scale:: --all-features --locked -- --nocapture
    mkdir -p artifacts/search-quality/scale/latest
    {{cargo}} --lane test-daemon-lane build -p quanta-index-searchd-harness --bin scale_matrix --all-features --locked
    env QUANTA_INDEX_BUILD_LANE=test-daemon-lane bash -lc 'source scripts/quanta-index-env.sh && BIN="$CARGO_TARGET_DIR/debug/scale_matrix" && test -x "$BIN" && "$BIN" --out-dir "$(pwd)/artifacts/search-quality/scale/latest"'

# Latency-tail rail (J7Q-04). Blocking dimension: tail (correctness-gated).
# Proves: route-aware p50/p95/p99 budget manifest + percentile invariants, then
# boots one warm runtime and times each budgeted route's representative query
# TAIL_SAMPLES times. Correctness (golden-validated shape/count/error before
# timing) is the blocking signal; per-route latency budgets are explicit but
# advisory on this host this increment (canonical blocking is the Linux perf
# runner). No single global threshold; verdicts carry route-local diagnostics.
# Artifacts: artifacts/search-quality/tail/latest/{summary,route_budgets}.json
rust-verify-quality-tail:
    python3 tools/ci/timing/check_host_contention.py
    {{cargo}} --lane test-daemon-lane test -p quanta-index-searchd-harness --lib tail:: --all-features --locked -- --nocapture
    mkdir -p artifacts/search-quality/tail/latest
    {{cargo}} --lane test-daemon-lane build -p quanta-index-searchd-harness --bin tail_matrix --all-features --locked
    env QUANTA_INDEX_BUILD_LANE=test-daemon-lane bash -lc 'source scripts/quanta-index-env.sh && BIN="$CARGO_TARGET_DIR/debug/tail_matrix" && test -x "$BIN" && "$BIN" --out-dir "$(pwd)/artifacts/search-quality/tail/latest"'

# ANN rail (QI-BB-027 #3). Blocking signal: recall@10 at or above the floor,
# every returned score the exact cosine, every page full.
# Proves: the rail's direction walk and exhaustive cosine oracle, then seals the
# 4,096 × 64 tier through the semantic adapter in a private state root, asks 64
# neighbour queries and writes one provenanced BenchArtifactV1 carrying recall@k,
# p50/p95/p99, build time, index bytes, the dense lane the seal proved and the
# normalization policy. Latency advisory on this host.
# Artifacts: artifacts/search-quality/ann/latest/summary.json
rust-verify-quality-ann:
    python3 tools/ci/timing/check_host_contention.py
    {{cargo}} --lane test-daemon-lane test -p quanta-index-searchd-harness --lib ann:: --all-features --locked -- --nocapture
    mkdir -p artifacts/search-quality/ann/latest
    {{cargo}} --lane test-daemon-lane build -p quanta-index-searchd-harness --bin ann_matrix --all-features --locked
    env QUANTA_INDEX_BUILD_LANE=test-daemon-lane bash -lc 'source scripts/quanta-index-env.sh && BIN="$CARGO_TARGET_DIR/debug/ann_matrix" && test -x "$BIN" && "$BIN" --out-dir "$(pwd)/artifacts/search-quality/ann/latest"'

# Concurrency rail (QI-BB-010 #4). Blocking signal: every request answered,
# no timeouts; latency advisory on this host.
# Proves: the concurrency module's tally/row invariants, then serves one sealed
# generation of the seeded medium corpus and runs 1/8/32 clients over the mixed
# lexical/semantic/hybrid/symbol/count route set with a slow page-maximum client mixed
# in above one client, writing one provenanced BenchArtifactV1 per client count
# (p50/p95/p99, QPS, error/timeout counts, head-of-line ratio). Small on this
# host by default (at least `requests` per client). Each route and the slow
# client require 16 samples; fast clients continue until the slow floor is met.
# Artifacts: artifacts/search-quality/concurrency/latest/summary-c{1,8,32}.json
rust-verify-quality-concurrency requests="80":
    python3 tools/ci/timing/check_host_contention.py
    {{cargo}} --lane test-daemon-lane test -p quanta-index-searchd-harness --lib concurrency:: --all-features --locked -- --nocapture
    mkdir -p artifacts/search-quality/concurrency/latest
    {{cargo}} --lane test-daemon-lane build -p quanta-index-searchd-harness --bin concurrency_matrix --all-features --locked
    env QUANTA_INDEX_BUILD_LANE=test-daemon-lane bash -lc 'source scripts/quanta-index-env.sh && BIN="$CARGO_TARGET_DIR/debug/concurrency_matrix" && test -x "$BIN" && "$BIN" --out-dir "$(pwd)/artifacts/search-quality/concurrency/latest" --requests-per-client {{requests}}'

# Explicit workspace file-mutation to generation-pinned lexical visibility.
# This measures the ingest receipt through seal/activation/query path, not a watcher.
rust-verify-quality-freshness samples="20":
    python3 tools/ci/timing/check_host_contention.py
    mkdir -p artifacts/search-quality/freshness/latest
    {{cargo}} --lane test-daemon-lane build -p quanta-index-searchd-harness --bin freshness_matrix --all-features --locked
    env QUANTA_INDEX_BUILD_LANE=test-daemon-lane bash -lc 'source scripts/quanta-index-env.sh && BIN="$CARGO_TARGET_DIR/debug/freshness_matrix" && test -x "$BIN" && "$BIN" --out-dir "$(pwd)/artifacts/search-quality/freshness/latest" --samples {{samples}}'

# Scheduled arrivals over real query IPC; record offered/achieved QPS and tails.
rust-verify-quality-open-loop:
    python3 tools/ci/timing/check_host_contention.py
    mkdir -p artifacts/search-quality/open-loop/latest
    {{cargo}} --lane test-daemon-lane build -p quanta-index-searchd-harness --bin open_loop_matrix --all-features --locked
    env QUANTA_INDEX_BUILD_LANE=test-daemon-lane bash -lc 'source scripts/quanta-index-env.sh && BIN="$CARGO_TARGET_DIR/debug/open_loop_matrix" && test -x "$BIN" && "$BIN" --out-dir "$(pwd)/artifacts/search-quality/open-loop/latest"'

# Operator-ergonomics rail (J7Q-05). Blocking dimension: ops.
# Proves: read-only operator-diagnosis surfaces are machine-readable and preserve
# provenance — route (engines_touched) + serving generation, the typed-error code
# for a rejected query, queryable perf/tail metrics, and the active generation +
# pin. A surface that swallows its provenance (empty engines, generation
# mismatch, blank typed-error code, no metrics) fails the rail. Read-only first
# increment; no mutating workflow.
# Artifacts: artifacts/search-quality/ops/latest/{summary,cli_snapshots}.json
rust-verify-quality-ops:
    {{cargo}} --lane test-daemon-lane test -p quanta-index-searchd-harness --lib ops:: --all-features --locked -- --nocapture
    mkdir -p artifacts/search-quality/ops/latest
    {{cargo}} --lane test-daemon-lane build -p quanta-index-searchd-harness --bin ops_matrix --all-features --locked
    env QUANTA_INDEX_BUILD_LANE=test-daemon-lane bash -lc 'source scripts/quanta-index-env.sh && BIN="$CARGO_TARGET_DIR/debug/ops_matrix" && test -x "$BIN" && "$BIN" --out-dir "$(pwd)/artifacts/search-quality/ops/latest"'

# UI/UX contract rail (J7Q-07). Blocking dimension: ui.
# Proves: the typed `LexicalCandidate::snippet_hit_offset` highlight anchor is
# present and points exactly at the matched needle on each probe (short + long
# line), so a downstream UI renders highlights without regex-parsing raw snippets.
# Proven across engine + contract layers (the field round-trips on the wire in
# the contract IPC test). A missing/out-of-range/wrong anchor fails the rail.
# Artifacts: artifacts/search-quality/ui/latest/{summary,contract_snapshots}.json
rust-verify-quality-ui:
    {{cargo}} --lane test-daemon-lane test -p quanta-index-searchd-harness --lib ui:: --all-features --locked -- --nocapture
    mkdir -p artifacts/search-quality/ui/latest
    {{cargo}} --lane test-daemon-lane build -p quanta-index-searchd-harness --bin ui_matrix --all-features --locked
    env QUANTA_INDEX_BUILD_LANE=test-daemon-lane bash -lc 'source scripts/quanta-index-env.sh && BIN="$CARGO_TARGET_DIR/debug/ui_matrix" && test -x "$BIN" && "$BIN" --out-dir "$(pwd)/artifacts/search-quality/ui/latest"'

# Aggregate quality gate (J7Q-08). Orchestrates the LIVE per-dimension rails
# (relevance, ambiguity, snippet, scale, tail, ANN, concurrency, freshness,
# open-loop, ops, ui) and records an
# integration summary WITHOUT erasing dimension boundaries. This is not a
# substitute for per-dimension closeout. A new dimension must have a producer,
# manifest registration, and ticket metadata before joining the aggregate.
rust-verify-quality-all:
    python3 tools/benchmark/benchctl.py run quality-full
    mkdir -p artifacts/search-quality/integration/latest
    python3 tools/benchmark/quality_integration_summary.py --out artifacts/search-quality/integration/latest/summary.json

rust-machete:
    env QUANTA_INDEX_BUILD_LANE=machete-lane bash -lc 'source scripts/quanta-index-env.sh && cargo machete --with-metadata'

rust-miri:
    env QUANTA_INDEX_BUILD_LANE=miri-lane bash -lc 'source scripts/quanta-index-env.sh && RUSTUP_TOOLCHAIN=nightly cargo miri setup'
    env QUANTA_INDEX_BUILD_LANE=miri-lane bash -lc 'source scripts/quanta-index-env.sh && RUSTUP_TOOLCHAIN=nightly MIRIFLAGS="-Zmiri-strict-provenance" cargo miri test -p quanta-index-contract -p quanta-index-core --lib --all-features'

rust-careful:
    env QUANTA_INDEX_BUILD_LANE=careful-lane bash -lc 'source scripts/quanta-index-env.sh && RUSTUP_TOOLCHAIN=nightly CARGO_INCREMENTAL=0 cargo careful test --workspace --all-features --locked'

rust-tsan:
    env QUANTA_INDEX_BUILD_LANE=tsan-lane bash -lc 'source scripts/quanta-index-env.sh && RUSTUP_TOOLCHAIN=nightly RUSTFLAGS="-Zsanitizer=thread" cargo test -Z build-std --target $(rustc -vV | sed -n '"'"'s|host: ||p'"'"') --workspace --all-features --lib --tests'

rust-asan:
    env QUANTA_INDEX_BUILD_LANE=asan-lane bash -lc 'source scripts/quanta-index-env.sh && RUSTUP_TOOLCHAIN=nightly RUSTFLAGS="-Zsanitizer=address" cargo test -Z build-std --target $(rustc -vV | sed -n '"'"'s|host: ||p'"'"') --workspace --all-features --lib --tests'

rust-mutants:
    env QUANTA_INDEX_BUILD_LANE=mutants-lane bash -lc 'source scripts/quanta-index-env.sh && cargo mutants --package quanta-index-core --timeout 60 --baseline=skip --no-shuffle'

rust-udeps:
    env QUANTA_INDEX_BUILD_LANE=udeps-lane bash -lc 'source scripts/quanta-index-env.sh && RUSTUP_TOOLCHAIN=nightly CARGO_INCREMENTAL=0 cargo udeps --workspace --all-targets --all-features'

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

rust-module-cycles:
    python3 tools/ci/lint/check-module-cycles.py

rust-error-shape:
    python3 tools/ci/lint/check-error-shape.py

rust-digest-fallibility:
    python3 tools/ci/lint/check-digest-fallibility.py

rust-semantic-outcomes:
    python3 tools/ci/lint/check-semantic-outcomes.py

rust-fallbacks:
    python3 tools/ci/tests/test_check_rust_fallbacks.py
    python3 tools/ci/lint/check-rust-fallbacks.py

rust-test-authority:
    python3 tools/ci/lint/check-test-authority.py

rust-ignored-test-policy:
    python3 tools/ci/lint/check-ignored-test-policy.py

# Asset-dependent owner proof. Both tests are ignored in the ordinary suite;
# this recipe generates the independent Python reference and selects them.
# The model loader and Rust parity test bind the three model file digests.
proof-potion-code-parity $model_dir $reference:
    uv run --no-project --python 3.13 --with 'model2vec==0.9.0' python tools/benchmark/retrieval/parity_reference.py --model-dir "$model_dir" --out "$reference"
    QUANTA_INDEX_TEST_POTION_CODE_MODEL_DIR="$model_dir" QUANTA_INDEX_PARITY_REFERENCE="$reference" {{cargo}} --lane test-integration-lane nextest run --locked -p quanta-index-embed --lib --run-ignored all --no-tests fail -E 'test(=model2vec::tests::full_vector_parity_against_pinned_reference)'
    QUANTA_INDEX_TEST_POTION_CODE_MODEL_DIR="$model_dir" {{cargo}} --lane test-integration-lane nextest run --locked -p quanta-index-searchd --lib --run-ignored all --no-tests fail -E 'test(=app::runtime::tests::potion_code_profile_uses_one_model_for_query_and_corpus)'

# Consumer / wire inventory (plan §11): every IPC opcode and on-disk format
# version must be listed in tools/ci/inventory/wire-surface.toml.
rust-wire-inventory:
    python3 tools/ci/lint/check-wire-inventory.py

# Stale-artifact gate (QI-BB-010, findings §9): every benchmark/relevance
# artifact on disk must be a schema-2 BenchArtifactV1 whose git_head is the
# checkout's HEAD (baselines: a full head). Absence passes; staleness fails.
rust-bench-artifacts:
    python3 tools/ci/lint/check-bench-artifacts.py

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
    env QUANTA_INDEX_BUILD_LANE=fuzz-lq-norm-lane bash -lc 'source scripts/quanta-index-env.sh && cd crates/quanta-index-lq-norm/fuzz && cargo +nightly fuzz build'

rust-fuzz-smoke seconds="60":
    env QUANTA_INDEX_BUILD_LANE=fuzz-contract-lane bash -lc 'source scripts/quanta-index-env.sh && cd crates/quanta-index-contract/fuzz && cargo +nightly fuzz run ipc_request_decode -- -dict=dictionaries/composite_search_corpus_control.dict -max_total_time={{seconds}}'
    env QUANTA_INDEX_BUILD_LANE=fuzz-contract-lane bash -lc 'source scripts/quanta-index-env.sh && cd crates/quanta-index-contract/fuzz && cargo +nightly fuzz run ipc_response_decode -- -dict=dictionaries/composite_search_corpus_control.dict -max_total_time={{seconds}}'
    env QUANTA_INDEX_BUILD_LANE=fuzz-contract-lane bash -lc 'source scripts/quanta-index-env.sh && cd crates/quanta-index-contract/fuzz && cargo +nightly fuzz run search_corpus_ingest_decode -- -dict=dictionaries/search_corpus_ingest.dict -max_total_time={{seconds}}'
    env QUANTA_INDEX_BUILD_LANE=fuzz-lq-norm-lane bash -lc 'source scripts/quanta-index-env.sh && cd crates/quanta-index-lq-norm/fuzz && cargo +nightly fuzz run lq_parse_pipeline -- -max_total_time={{seconds}}'

# Produces a fresh full-workspace LCOV artifact, then fails closed if a Rust
# source line added since `base` is absent or uncovered. cargo-llvm-cov is a
# tool-owned wrapper, so it runs only after the repository environment is set.
rust-coverage-changed base minimum_percent="90":
    {{cargo}} --lane coverage-lane llvm-cov nextest --workspace --all-features --locked --lcov --output-path /tmp/quanta-index-coverage.lcov
    python3 tools/ci/lint/check-changed-line-coverage.py --lcov /tmp/quanta-index-coverage.lcov --base {{base}} --minimum-percent {{minimum_percent}}

rust-policy:
    @just rust-workspace-lints
    @just rust-hexagonal
    @just rust-no-allow
    @just rust-derive-allowlist
    @just rust-cargo-toml-hygiene
    @just rust-module-discipline
    @just rust-module-cycles
    @just rust-error-shape
    @just rust-digest-fallibility
    @just rust-test-authority
    @just rust-ignored-test-policy
    @just rust-wire-inventory
    @just proof-authority-lint
    @just rust-bench-artifacts
    @just rust-deny

# SEP-21 proof policy separates static lint, the P00 current gate, pre-deploy
# code qualification, and final production qualification. Static lint never
# treats absent future proof artifacts as success or failure. Code qualification
# requires the fixed CODE_QUALIFIED proof closure; final qualification also
# requires deployment, activation, rollback, P12, and the aggregate receipt.
proof-authority-lint:
    python3 tools/ci/lint/check-proof-authority.py

proof-authority-current-gate:
    python3 tools/ci/lint/check-proof-authority.py \
        --manifest artifacts/proof-authority/p00-authority-freeze.json \
        --bind-source

lane-handoff-check handoff:
    python3 tools/ci/lint/check-lane-handoff.py --require-result-head "{{handoff}}"

lane-handoff-check-historical handoff:
    python3 tools/ci/lint/check-lane-handoff.py "{{handoff}}"

lane-handoff-chain-check:
    python3 tools/ci/lint/check-lane-handoff.py --product-chain artifacts/sep-21/handoffs

proof-error-authority-inventory:
    python3 tools/ci/write-error-authority-inventory.py

proof-error-authority-closed:
    python3 tools/ci/check-error-authority-closure.py

proof-p01-canonical-identity:
    @just proof-error-authority-closed
    @just rust-profile test-canonical-identity
    @just rust-public-api
    @just rust-wire-inventory
    @just rust-fuzz-smoke
    @just rust-hexagonal
    @just rust-cargo-modules
    @just rust-profile validate-shared-surface
    @just rust-profile test-daemon

# P02B: the global sequence authority and operation journal proof. The
# journal touches the ingest IPC receipt surface, so the wire inventory and
# the IPC decoder fuzz smoke run alongside the scoped journal tests and
# the hexagonal boundary guards.
proof-p02b-operation-journal:
    @just rust-profile test-p02b-operation-journal
    @just rust-hexagonal
    @just rust-wire-inventory
    @just rust-fuzz-smoke

proof-p00-authority-freeze:
    @test -z "${PYTEST_ADDOPTS:-}" && test -z "${PYTEST_PLUGINS:-}" || { echo "pytest environment overrides are forbidden for proof tests" >&2; exit 2; }
    python3 tools/ci/write-error-authority-inventory.py
    python3 tools/ci/lint/check-proof-authority.py
    @if [ -n "${QUANTA_PROOF_RAW_DIR:-}" ]; then mkdir -p "$QUANTA_PROOF_RAW_DIR"; python3 tools/ci/proof_execution_result.py collect-pytest --output "$QUANTA_PROOF_RAW_DIR/p00-inventory.json" tools/ci/tests/test_write_error_authority_inventory.py tools/ci/tests/test_write_proof_aggregate.py tools/ci/tests/test_write_proof_manifest.py tools/ci/tests/test_proof_execution_result.py tools/ci/tests/test_paired_cargo_resolution.py tools/ci/tests/test_run_local_test_scope.py tools/ci/tests/test_check_proof_authority.py tools/ci/tests/test_check_lane_handoff.py tools/ci/tests/test_handoff_validation.py; fi
    python3 -m pytest \
        tools/ci/tests/test_write_error_authority_inventory.py \
        tools/ci/tests/test_write_proof_aggregate.py \
        tools/ci/tests/test_write_proof_manifest.py \
        tools/ci/tests/test_proof_execution_result.py \
        tools/ci/tests/test_paired_cargo_resolution.py \
        tools/ci/tests/test_run_local_test_scope.py \
        tools/ci/tests/test_check_proof_authority.py \
        tools/ci/tests/test_check_lane_handoff.py \
        tools/ci/tests/test_handoff_validation.py -q

# P12A is a Python owner proof. Its exact-source manifest can be issued
# independently; P12 final qualification binds P11 and the paired checkout.
proof-p12a-proof-infrastructure:
    @test -z "${PYTEST_ADDOPTS:-}" && test -z "${PYTEST_PLUGINS:-}" || { echo "pytest environment overrides are forbidden for proof tests" >&2; exit 2; }
    python3 tools/ci/lint/check-test-authority.py
    python3 tools/ci/lint/check-proof-authority.py
    python3 tools/ci/proof_execution_result.py run-p12a \
        tools/ci/tests/test_write_proof_manifest.py \
        tools/ci/tests/test_proof_execution_result.py \
        tools/ci/tests/test_paired_cargo_resolution.py \
        tools/ci/tests/test_run_local_test_scope.py \
        tools/ci/tests/test_write_proof_aggregate.py \
        tools/ci/tests/test_check_proof_authority.py \
        tools/ci/tests/test_check_lane_handoff.py \
        tools/ci/tests/test_handoff_validation.py

# The aggregate is the final release receipt. The release gate revalidates
# every input against the current source pair.
proof-authority-final-qualification:
    @test -n "${SEMANTICA_CHECKOUT:-}" || { echo "SEMANTICA_CHECKOUT is required" >&2; exit 2; }
    python3 tools/ci/write-proof-aggregate.py \
        --paired-checkout "${SEMANTICA_CHECKOUT}"
    python3 tools/ci/lint/check-proof-authority.py \
        --require-all \
        --paired-checkout "github:josongmin/semantica-codegraph-v2=${SEMANTICA_CHECKOUT}" \
        --bind-source

proof-authority-code-gate:
    @test -n "${SEMANTICA_CHECKOUT:-}" || { echo "SEMANTICA_CHECKOUT is required" >&2; exit 2; }
    python3 tools/ci/lint/check-proof-authority.py \
        --require-code-qualified \
        --paired-checkout "github:josongmin/semantica-codegraph-v2=${SEMANTICA_CHECKOUT}" \
        --bind-source

proof-authority-release-gate:
    @test -n "${SEMANTICA_CHECKOUT:-}" || { echo "SEMANTICA_CHECKOUT is required" >&2; exit 2; }
    python3 tools/ci/lint/check-proof-authority.py \
        --require-all \
        --paired-checkout "github:josongmin/semantica-codegraph-v2=${SEMANTICA_CHECKOUT}" \
        --bind-source

verify-rust:
    @just fmt-check
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
    @just rust-fuzz-smoke

semgrep:
    bash scripts/run-semgrep.sh

semgrep-rule-tests:
    bash scripts/check-semgrep-rules.sh

actionlint:
    bash scripts/run-actionlint.sh

shell-lint:
    bash scripts/run-shellcheck.sh

python-lint:
    source scripts/quanta-index-env.sh && uv run --frozen --extra dev python -m ruff check .

python-format-check:
    source scripts/quanta-index-env.sh && uv run --frozen --extra dev python -m ruff format --check .

python-test:
    uv run --frozen --extra dev bash scripts/run-tooling-tests.sh

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
    @just semgrep-rule-tests
    @just semgrep
    @just rust-semantic-outcomes
    @just rust-fallbacks
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

# P02A whole-bundle RepoMap graph compiler owner proof. The scoped rail runs
# the compiler owner target plus the repomap bounded/owner surfaces bound to
# the same source; structural rails guard the contract surface the compiler
# DTO section touched. RSS/production-host benchmark stays a separate
# release-only rail and is NOT_RUN here.
proof-p02a-repomap-compiler:
    @just rust-profile test-p02a-repomap-compiler
    @just rust-hexagonal
    @just rust-wire-inventory

proof-p03-candidate-activation-owner:
    @just rust-profile test-candidate-activation-owner
    @just rust-hexagonal
    @just rust-wire-inventory
    @just rust-public-api
    @just rust-fuzz-smoke

# P04 read-view lifetime owner proof (S21-05): the scoped rail runs the
# repomap lifetime owner target plus the repomap owner surface and the core
# read-view declaration bound to the same source; the search-plane lib rail
# carries the view-level V2 tests. The structural guard proves routes have
# zero ambient lookup and the V1 surfaces stay deleted. The Linux
# production-like release subrail (p04-read-view-lifetime, release-daemon
# binding) stays NOT_RUN on this host.
proof-p04-read-view-lifetime-owner:
    @just rust-profile test-read-view-lifetime-owner
    @just rust-profile test-search-plane-read-view-lib
    @python3 tools/ci/lint/check-read-view-ambient-lookup.py
    @just rust-hexagonal
    @just rust-wire-inventory
    @just rust-public-api
    @just rust-fuzz-smoke

# P06 SDK binding owner proof (S21-07): the scoped rail runs the SDK
# owner integration target (wrong-but-same-variant negative matrix over a
# real UDS scripted peer, query-only profile, coverage table) and the SDK
# lib suites bound to the same source. The SDK public API changed
# (binding error, profile split, coverage inventory), so the structural
# rails include public API and wire inventory plus the IPC fuzz smoke.
# The Linux production-like release subrail (p06-sdk-binding,
# release-daemon binding) stays NOT_RUN on this host.
proof-p06-sdk-binding-owner:
    @just rust-profile test-sdk-binding-owner
    @just rust-profile test-sdk-binding-owner-lib
    @just rust-hexagonal
    @just rust-wire-inventory
    @just rust-public-api
    @just rust-fuzz-smoke

# P05 query-truth owner proof (S21-06): the scoped rail runs the
# contract-base outcome/cursor-envelope owner suite and the search-plane
# query-truth owner suite (independent RRF/window oracle, outcome honesty
# matrix, cursor key custody) bound to the same source. Wire shapes
# changed (window_v2 fields, cursor envelope), so the structural rails
# include wire inventory, public API and the IPC fuzz smoke. The Linux
# production-like release subrail (p05-query-truth, release-daemon
# binding) stays NOT_RUN on this host.
# P07 semantic admission / provider boundary owner proof (S21-08): the
# scoped rail runs the search-plane owner suite (zero-call refusal matrix
# over a counting spy embedder, global reservation/settlement bounds,
# cancellation reconciliation, declared-vs-observed validation, source
# content egress grant). Core gained the semantic admission module, so the
# structural rails include the module-tree snapshot plus hexagonal, wire
# inventory and the IPC fuzz smoke. The Linux production-like release
# subrail (p07-provider-boundary, release-daemon binding) and the approved
# budgeted real-provider proof stay NOT_RUN on this host.
proof-p07-provider-boundary-owner:
    @just rust-profile test-provider-boundary-owner
    @just rust-hexagonal
    @just rust-wire-inventory
    @just rust-cargo-modules
    @just rust-fuzz-smoke

# P08 supervised runtime / bounded shutdown owner proof (S21-09): the
# scoped rail runs the supervisor owner suite (clean drain with
# guards-last drop order, startup rollback, required-child loss,
# cooperative-vs-hard deadline escalation, second-signal abort exit
# semantics, RAII permit reconciliation, real two-process state-root
# exclusion with lock fstat invariants).
# The supervisor lives in quanta-index-searchd and the split changed the
# runtime facade, so the structural rails include hexagonal, wire
# inventory and the module-tree snapshot. The Linux production-like
# release process rail (p08-runtime-supervisor, release-daemon binding,
# SIGINT/SIGTERM on the release binary) stays NOT_RUN on this host.
proof-p08-runtime-supervisor-owner:
    @just rust-profile test-runtime-supervisor-owner
    @just rust-hexagonal
    @just rust-wire-inventory
    @just rust-cargo-modules

proof-p05-query-truth-owner:
    @just rust-profile test-query-truth-owner
    @just rust-profile test-query-truth-owner-lib
    @just rust-hexagonal
    @just rust-wire-inventory
    @just rust-public-api
    @just rust-fuzz-smoke

# Dedicated P04 read-view recipe: owner-local selector first, then the
# Linux release subrail split the prompt names (NOT_RUN off-Linux).
rust-proof-p04-read-view: proof-p04-read-view-lifetime-owner

# P09 (S21-10) control authorization + readiness owner proof: the scoped
# integration targets prove the capability/access matrix and real supervised
# control-UDS readiness; the lib scope proves dispatch-level default-deny
# (zero mutation) and the readiness truth table. Rustx/fuzz rails guard the
# wire shapes the DTOs added.
proof-p09-control-readiness-owner:
    @just rust-profile test-control-readiness-owner
    @just rust-profile test-control-readiness-owner-lib
    @just rust-hexagonal
    @just rust-wire-inventory
    @just rust-public-api
    @just rust-fuzz-smoke

# P10 (S21-11) offline state migration, backup and restore owner proof: the
# scoped integration target proves the offline migrate/backup/restore/verify
# workflow over disposable state roots (legacy-root typed refusal at boot,
# backup-API catalog freeze, manifest-last crash convergence, atomic cutover,
# refusal matrix) and the lib scope carries the engine's unit suites. The
# structural rails guard the surface the offline CLI and the new persisted
# formats touched. The lib scope includes its pinned-model test rather than
# issuing a passed owner receipt with an ignored selection. The model assets
# must match the hashes enforced by quanta-index-embed. The Linux production-like release subrail
# (p10-state-migration, release-daemon binding) stays NOT_RUN on this host.
proof-p10-state-migration-owner:
    @just rust-profile test-state-migration-owner
    @just rust-profile test-state-migration-owner-lib
    @just rust-hexagonal
    @just rust-wire-inventory
    @just rust-public-api
