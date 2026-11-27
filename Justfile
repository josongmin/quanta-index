set shell := ["/bin/zsh", "-lc"]

cargo := "./scripts/cargow"
retrieval_python := "uv run --frozen --extra dev python"
sqlite_fd_ledger_manifest := "crates/quanta-index-sqlite-fd-ledger/Cargo.toml"

default:
    @just --list

install-dev:
    uv sync --frozen --extra dev

install-hooks:
    uv run --frozen --extra dev pre-commit install --install-hooks --hook-type pre-commit --hook-type pre-push

cache-root:
    @bash scripts/quanta-index-env.sh

# Prune idle/orphan Cargo target lanes and stale test executables.
# Examples: `just target-gc --dry-run`, `just target-gc --dry-run --json`,
# `just target-gc --idle-hours 48 --max-total-gb 200`.
[positional-arguments]
target-gc *args:
    python3 tools/ci/target_gc.py "$@"

rust-sccache-stats:
    @source scripts/quanta-index-env.sh && if [[ -n "${RUSTC_WRAPPER:-}" && "${RUSTC_WRAPPER:t}" == "sccache" ]]; then sccache --show-stats; else echo 'sccache is disabled or unavailable'; fi

rust-profile-list:
    @printf '%s\n' \
        'dev-fast            default local edit loop; workspace lib/bin compile, excludes searchd runtime' \
        'dev-daemon          searchd runtime only; use when editing daemon/runtime entrypoints' \
        'dev-all-targets     widest compile rail; all targets across the workspace' \
        'validate-shared-surface contract/core/sdk/search-plane shared-surface validation rail' \
        'test-fast           default local test loop; workspace lib/bin tests, excludes daemon e2e' \
        'test-canonical-identity P01 identity, codec, layout-security, and error-authority proof' \
        'test-p02b-operation-journal P02B global sequence authority and operation journal proof' \
        'test-p02b-native-journal-lib P02B Native operation replay, conflict, and rollback supplement' \
        'test-candidate-activation-owner P03 Native Catalog activation, CAS, and recovery owner proof' \
        'test-candidate-activation-release P03 Native activation and restart on the release daemon' \
        'test-read-view-lifetime-owner P04 Native Catalog pin and retirement owner proof' \
        'test-search-plane-read-view-lib P04 search-plane read-view lib proof' \
        'test-query-truth-owner P05 query outcome/cursor/oracle owner proof' \
        'test-query-truth-release P05 Native retained keysets on the release daemon' \
        'test-control-readiness-owner P09 control authorization/readiness owner proof' \
        'test-control-readiness-release P09 Native installation readiness on the release daemon' \
        'test-control-readiness-owner-lib P09 control authorization/readiness lib suites' \
        'test-state-migration-owner P10 current-format backup/restore/verify and old-root refusal owner proof' \
        'test-state-migration-owner-lib P10 current-format offline state lib suites' \
        'test-state-migration-release P10 exact Native rebuild on the release daemon' \
        'test-query-truth-owner-lib P05 outcome/cursor/oracle lib suites' \
        'test-sdk-binding-owner P06 SDK contextual binding negative-matrix owner proof' \
        'test-sdk-binding-release P06 Native SDK exact-view binding on the release daemon' \
        'test-sdk-binding-owner-lib P06 SDK lib suites' \
        'test-provider-boundary-owner P07 semantic admission / provider boundary owner proof' \
        'test-provider-boundary-release P07 Native paid provider budget and cancellation' \
        'test-runtime-supervisor-owner P08 supervised runtime / bounded shutdown owner proof' \
        'test-runtime-supervisor-release P08 Native signal and required-child drain on the release daemon' \
        'test-integration-fast bounded integration loop; excludes slow text/Lance storage' \
        'test-integration-storage persisted lexical data and boot inventory' \
        'test-integration-semantic semantic storage integration slice' \
        'test-integration    complete fast + storage + semantic integration rail' \
        'test-cli-smoke      CLI smoke tests for searchctl + corpus-smoke' \
        'test-daemon-fast    daemon edit loop with the migrated Legacy query regressions' \
        'test-daemon         30 runtime sources plus DSL truth; 3 binaries' \
        'test-daemon-all     49 runtime sources plus DSL truth; 4 binaries' \
        'release-cli        optimized release build for quanta-index-searchctl' \
        'release-cli-fresh  clean release-bin lane, then rebuild quanta-index-searchctl' \
        'release-native-daemon optimized Native daemon without the Tantivy dependencies' \
        'release-daemon     optimized Native daemon using the release artifact guard' \
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
        test-p02b-operation-journal) ./scripts/run-rust-profile.sh "{{profile}}" rust-test-p02b-operation-journal ;; \
        test-p02b-native-journal-lib) ./scripts/run-rust-profile.sh "{{profile}}" rust-test-p02b-native-journal-lib ;; \
        test-candidate-activation-owner) ./scripts/run-rust-profile.sh "{{profile}}" rust-test-candidate-activation-owner ;; \
        test-candidate-activation-release) ./scripts/run-rust-profile.sh "{{profile}}" rust-test-candidate-activation-release ;; \
        test-read-view-lifetime-owner) ./scripts/run-rust-profile.sh "{{profile}}" rust-test-read-view-lifetime-owner ;; \
        test-search-plane-read-view-lib) ./scripts/run-rust-profile.sh "{{profile}}" rust-test-search-plane-read-view-lib ;; \
        test-query-truth-owner) ./scripts/run-rust-profile.sh "{{profile}}" rust-test-query-truth-owner ;; \
        test-query-truth-release) ./scripts/run-rust-profile.sh "{{profile}}" rust-test-query-truth-release ;; \
        test-control-readiness-owner) ./scripts/run-rust-profile.sh "{{profile}}" rust-test-control-readiness-owner ;; \
        test-control-readiness-release) ./scripts/run-rust-profile.sh "{{profile}}" rust-test-control-readiness-release ;; \
        test-control-readiness-owner-lib) ./scripts/run-rust-profile.sh "{{profile}}" rust-test-control-readiness-owner-lib ;; \
        test-state-migration-owner) ./scripts/run-rust-profile.sh "{{profile}}" rust-test-state-migration-owner ;; \
        test-state-migration-owner-lib) ./scripts/run-rust-profile.sh "{{profile}}" rust-test-state-migration-owner-lib ;; \
        test-state-migration-release) ./scripts/run-rust-profile.sh "{{profile}}" rust-test-state-migration-release ;; \
        test-query-truth-owner-lib) ./scripts/run-rust-profile.sh "{{profile}}" rust-test-query-truth-owner-lib ;; \
        test-sdk-binding-owner) ./scripts/run-rust-profile.sh "{{profile}}" rust-test-sdk-binding-owner ;; \
        test-sdk-binding-release) ./scripts/run-rust-profile.sh "{{profile}}" rust-test-sdk-binding-release ;; \
        test-sdk-binding-owner-lib) ./scripts/run-rust-profile.sh "{{profile}}" rust-test-sdk-binding-owner-lib ;; \
        test-provider-boundary-owner) ./scripts/run-rust-profile.sh "{{profile}}" rust-test-provider-boundary-owner ;; \
        test-provider-boundary-release) ./scripts/run-rust-profile.sh "{{profile}}" rust-test-provider-boundary-release ;; \
        test-runtime-supervisor-owner) ./scripts/run-rust-profile.sh "{{profile}}" rust-test-runtime-supervisor-owner ;; \
        test-runtime-supervisor-release) ./scripts/run-rust-profile.sh "{{profile}}" rust-test-runtime-supervisor-release ;; \
        test-integration-fast) ./scripts/run-rust-profile.sh "{{profile}}" rust-test-integration-fast ;; \
        test-integration-storage) ./scripts/run-rust-profile.sh "{{profile}}" rust-test-integration-storage ;; \
        test-integration-semantic) ./scripts/run-rust-profile.sh "{{profile}}" rust-test-integration-semantic ;; \
        test-integration) ./scripts/run-rust-profile.sh "{{profile}}" rust-test-integration ;; \
        test-cli-smoke) ./scripts/run-rust-profile.sh "{{profile}}" rust-test-cli-smoke ;; \
        test-daemon-fast) ./scripts/run-rust-profile.sh "{{profile}}" rust-test-e2e-fast ;; \
        test-daemon) ./scripts/run-rust-profile.sh "{{profile}}" rust-test-e2e ;; \
        test-read-view-lifetime) ./scripts/run-rust-profile.sh "{{profile}}" rust-test-read-view-lifetime ;; \
        test-daemon-all) ./scripts/run-rust-profile.sh "{{profile}}" rust-test-e2e-all ;; \
        release-cli) ./scripts/run-rust-profile.sh "{{profile}}" rust-build-release-cli ;; \
        release-cli-fresh) ./scripts/run-rust-profile.sh "{{profile}}" rust-build-release-cli-fresh ;; \
        release-native-daemon) ./scripts/run-rust-profile.sh "{{profile}}" rust-build-release-native-daemon ;; \
        release-daemon) ./scripts/run-rust-profile.sh "{{profile}}" rust-build-release-daemon ;; \
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
    bash scripts/run-rustfmt.sh write

fmt-check:
    bash scripts/run-rustfmt.sh check

rust-check lane="all-targets-lane":
    {{cargo}} --lane {{lane}} check --workspace --all-targets --all-features --locked
    {{cargo}} --lane sqlite-oracle-lane check --manifest-path {{sqlite_fd_ledger_manifest}} --all-targets --all-features --locked --offline

# Verify the standalone registry SQLite 0.39.1 oracle and Unix hook.
rust-test-sqlite-upstream-oracle lane="sqlite-oracle-lane":
    {{cargo}} --lane {{lane}} test --manifest-path crates/quanta-index-sqlite-unix-syscall/Cargo.toml --locked --offline
    {{cargo}} --lane {{lane}} test --manifest-path tools/sqlite-upstream-oracle/Cargo.toml --locked --offline

# Compare the released Lance APIs in a separate process and lockfile.
rust-test-lance-upstream-oracle lane="lance-oracle-lane":
    {{cargo}} --lane {{lane}} test --manifest-path tools/lance-upstream-oracle/Cargo.toml --locked --offline

# Local edit/compile loop without integration tests, examples, or benches.
rust-check-fast:
    {{cargo}} --lane fast-lane check --workspace --lib --bins --all-features --locked --exclude quanta-index-searchd-runtime

rust-check-daemon:
    {{cargo}} --lane daemon-lane check -p quanta-index-searchd-runtime --all-features --locked

rust-compile-shared-surface lane="shared-validation-lane":
    {{cargo}} --lane {{lane}} nextest run -p quanta-index-contract -p quanta-index-core -p quanta-index-sdk -p quanta-index-search-plane -p quanta-index-ipc --all-targets --all-features --locked --no-run

rust-validate-shared-surface:
    @just rust-compile-shared-surface shared-validation-lane
    uv run --frozen --extra dev python tools/ci/run-local-test-scope.py shared-surface --lane shared-validation-lane
    uv run --frozen --extra dev python tools/ci/run-local-test-scope.py integration-fast --lane shared-validation-lane
    uv run --frozen --extra dev python tools/ci/run-local-test-scope.py cli-smoke --lane shared-validation-lane

# Local Native build loop without integration tests or benches.
rust-build-fast:
    {{cargo}} --lane fast-lane build --workspace --lib --bins --no-default-features --locked --exclude quanta-index-searchd-runtime

rust-build-daemon:
    {{cargo}} --lane daemon-lane build -p quanta-index-searchd-runtime --no-default-features --locked

# Canonical deployable CLI artifact. Uses a dedicated release lane so optimized
# outputs do not share fingerprints with edit-loop targets.
[positional-arguments]
rust-build-release-cli *args:
    {{cargo}} --lane release-bin-lane build -p quanta-index-searchctl --bin quanta-index-searchctl --all-features --locked --release "$@"

# Fresh release build for stale-fingerprint recovery after source-layout cuts.
rust-build-release-cli-fresh:
    {{cargo}} --lane release-bin-lane clean --quiet
    {{cargo}} --lane release-bin-lane build -p quanta-index-searchctl --bin quanta-index-searchctl --all-features --locked --release

# Standard daemon release uses the Native candidate producer.
rust-build-release-daemon: rust-build-release-native-daemon

# Native deployment candidate. Prints the verified digest and its dedicated
# path under the Native release lane.
rust-build-release-native-daemon:
    python3 tools/ci/check-tantivy-retirement.py --check-release-sqlite-supply
    set -o pipefail; {{cargo}} --lane release-native-daemon-bin-lane tree -p quanta-index-searchd-runtime --no-default-features --locked --offline --edges normal,build --prefix none | awk '$1 ~ /^(tantivy|lancedb)([-_]|$)/ { print > "/dev/stderr"; forbidden = 1 } NF { nonempty = 1 } END { exit (!nonempty || forbidden) }'
    python3 tools/ci/check-tantivy-retirement.py --build-native-daemon

# The checker imports from the checkout. Install locked dependencies without rebuilding it.
[private]
tantivy-retirement-python:
    uv sync --python 3.12 --frozen --offline --extra dev --no-install-project --inexact

# Whole-workspace source and preserved-test checks for the retirement candidate.
rust-check-tantivy-retirement: tantivy-retirement-python
    uv run --no-sync --offline python tools/ci/check-tantivy-retirement.py --check-release-sqlite-supply
    uv run --no-sync --offline python tools/ci/check-tantivy-retirement.py --check-migrations

# Record the canonical daemon or CLI/SDK release build for the shipment check.
rust-capture-release-build $role $evidence:
    python3 tools/ci/check-tantivy-retirement.py --capture-release-build "$role" --evidence-dir "$evidence"

# Require the compiler observations and the installed Q/S pair selected by the proof registry.
[positional-arguments]
rust-check-tantivy-shipment *args: tantivy-retirement-python
    uv run --no-sync --offline python tools/ci/check-tantivy-retirement.py \
        --proof-checkout "${SEMANTICA_CHECKOUT:?set SEMANTICA_CHECKOUT to the approved checkout}" \
        --release-build "${QUANTA_RETIREMENT_DAEMON_BUILD:?set the daemon release evidence directory}" \
        --expected-build-receipt-sha256 "${QUANTA_RETIREMENT_DAEMON_RECEIPT_SHA256:?set the daemon build receipt digest}" \
        --release-cli-build "${QUANTA_RETIREMENT_CLI_BUILD:?set the CLI release evidence directory}" \
        --expected-cli-build-receipt-sha256 "${QUANTA_RETIREMENT_CLI_RECEIPT_SHA256:?set the CLI build receipt digest}" \
        --shipped-bundle "${QUANTA_RETIREMENT_INSTALLED_BUNDLE:?set the installed S bundle directory}" \
        --installed-readback "${QUANTA_RETIREMENT_INSTALLED_READBACK:?set the signed install readback path}" \
        --installed-q-approval-request "${QUANTA_RETIREMENT_INSTALLED_Q_APPROVAL_REQUEST:?set the installed Q approval request path}" "$@"

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
    {{cargo}} --lane sqlite-oracle-lane clippy --manifest-path {{sqlite_fd_ledger_manifest}} --all-targets --all-features --locked --offline -- -D warnings

rust-test: require-native-legacy-searchd
    {{cargo}} --lane test-workspace-lane test --workspace --all-features --locked
    @just rust-test-sqlite-fd-ledger

rust-test-sqlite-fd-ledger lane="sqlite-oracle-lane":
    {{cargo}} --lane {{lane}} nextest run --manifest-path {{sqlite_fd_ledger_manifest}} --lib --locked --offline --no-tests fail

# Audited unsafe descriptor ownership stays in this single shared package.
# It is excluded from workspace members and needs its own explicit rail.
rust-test-explicit-fd:
    {{cargo}} --lane test-workspace-lane nextest run --manifest-path shared/quanta-explicit-fd-process/Cargo.toml --lib --locked --offline --no-tests fail --test-threads 1

rust-clippy-explicit-fd:
    {{cargo}} --lane test-workspace-lane clippy --manifest-path shared/quanta-explicit-fd-process/Cargo.toml --all-targets --locked --offline -- -D warnings -D clippy::undocumented_unsafe_blocks

rust-audit-explicit-fd:
    uv run --frozen --extra dev python shared/quanta-explicit-fd-process/audit_source.py
    uv run --frozen --extra dev python -m unittest discover -s shared/quanta-explicit-fd-process -p test_audit_source.py

# Use the standalone registry consumer for either local no-default command.
rust-test-ciborium-no-default: rust-test-ciborium-registry-no-default

# Independently verify the registry dependency without workspace feature unification.
rust-test-ciborium-registry-no-default:
    {{cargo}} --lane test-ciborium-no-default-lane nextest run --manifest-path tools/ci/ciborium-no-default-consumer/Cargo.toml --locked --lib --no-tests fail
    python3 tools/ci/verify_ciborium_no_default.py

# Verify the pinned registry grammar set in an independent consumer.
rust-test-tree-sitter-registry-candidate:
    {{cargo}} --lane test-workspace-lane nextest run --manifest-path tools/ci/tree-sitter-registry-consumer/Cargo.toml --locked --lib --no-tests fail

# Local test loop that keeps unit and binary coverage but skips integration/E2E.
rust-test-fast:
    {{cargo}} --lane test-fast-lane test --workspace --lib --bins --all-features --locked --exclude quanta-index-searchd-runtime

rust-test-unit:
    {{cargo}} --lane test-fast-lane test --workspace --lib --bins --all-features --locked --exclude quanta-index-searchd-runtime

rust-test-canonical-identity lane="test-canonical-identity-lane":
    uv run --frozen --extra dev python tools/ci/run-local-test-scope.py canonical-identity --lane {{lane}}

rust-test-p02b-operation-journal lane="test-p02b-operation-journal-lane":
    uv run --frozen --extra dev python tools/ci/run-local-test-scope.py p02b-operation-journal --lane {{lane}}

rust-test-p02b-native-journal-lib lane="test-p02b-native-journal-lane":
    uv run --frozen --extra dev python tools/ci/run-local-test-scope.py p02b-native-journal-lib --lane {{lane}}

rust-test-candidate-activation-owner lane="test-candidate-activation-owner-lane":
    uv run --frozen --extra dev python tools/ci/run-local-test-scope.py candidate-activation-owner --lane {{lane}}

rust-test-read-view-lifetime-owner lane="test-read-view-lifetime-owner-lane":
    uv run --frozen --extra dev python tools/ci/run-local-test-scope.py read-view-lifetime-owner --lane {{lane}}

rust-test-search-plane-read-view-lib lane="test-search-plane-read-view-lane":
    uv run --frozen --extra dev python tools/ci/run-local-test-scope.py search-plane-read-view-lib --lane {{lane}}

rust-test-query-truth-owner lane="test-query-truth-owner-lane":
    uv run --frozen --extra dev python tools/ci/run-local-test-scope.py query-truth-owner --lane {{lane}}

rust-test-query-truth-owner-lib lane="test-query-truth-owner-lane":
    uv run --frozen --extra dev python tools/ci/run-local-test-scope.py query-truth-owner-lib --lane {{lane}}

rust-test-control-readiness-owner lane="test-control-readiness-owner-lane":
    uv run --frozen --extra dev python tools/ci/run-local-test-scope.py control-readiness-owner --lane {{lane}}

rust-test-control-readiness-owner-lib lane="test-control-readiness-owner-lane":
    QUANTA_INDEX_TEST_POTION_CODE_MODEL_DIR="${QUANTA_INDEX_TEST_POTION_CODE_MODEL_DIR:-$(./scripts/quanta-index-env.sh)/models/potion-code-16M-v2-e9d2a44}" uv run --frozen --extra dev python tools/ci/run-local-test-scope.py control-readiness-owner-lib --lane {{lane}} --run-ignored all

# Requires the approved external Legacy searchd path and SHA-256 pin.
rust-test-process-readiness-no-default lane="test-daemon-lane": require-native-legacy-searchd
    {{quote(cargo)}} --lane {{quote(lane)}} nextest run -p quanta-index-searchd-runtime --test process_readiness_owner_v1 --no-default-features --locked --offline --no-tests fail --success-output never

# Engine-independent lexical boot inventory. This target must remain runnable
# after the optional Legacy serving engine is removed.
rust-test-lexical-boot-no-default lane="test-integration-lane":
    {{cargo}} --lane {{lane}} nextest run -p quanta-index-lexical --test boot_inventory_data --no-default-features --locked --offline --no-tests fail --success-output final

# Requires the approved external Legacy searchd path and SHA-256 pin.
rust-test-f15-no-default lane="test-workspace-lane": require-native-legacy-searchd
    #!/usr/bin/env bash
    set -euo pipefail
    package=quanta-index-lexical
    binary=f15_file_authority
    delta=delta_delete_noop_and_cold_open_match_independent_full_build
    nontext=nontext_source_stays_path_only_across_cold_delta_noop_and_delete
    damage=damaged_file_bodies_stay_quarantined_after_restart
    export NEXTEST_EXPERIMENTAL_LIBTEST_JSON=1
    events="$(
        {{quote(cargo)}} --lane {{lane}} nextest run -p "$package" --test "$binary" \
            --no-default-features --locked --offline --no-tests fail \
            -E "test(=$delta) | test(=$nontext) | test(=$damage)" \
            --message-format libtest-json-plus --message-format-version 0.1 \
            --status-level pass --final-status-level fail
    )" || {
        status=$?
        printf '%s\n' "$events" >&2
        exit "$status"
    }
    printf '%s\n' "$events" | python3 -c '
    import sys
    from tools.ci.nextest_events import parse_nextest_bytes

    package, binary, *names = sys.argv[1:]
    expected = dict.fromkeys(
        (f"{package}::{binary}${name}" for name in names), (package, binary, "test")
    )
    result = parse_nextest_bytes(sys.stdin.buffer.read(), expected)
    print(f"F15 NoFeatures: {result.passed}/{len(expected)} passed")
    ' "$package" "$binary" "$delta" "$nontext" "$damage"
    env -u QUANTA_INDEX_LEGACY_SEARCHD_BIN -u QUANTA_INDEX_LEGACY_SEARCHD_SHA256 \
        {{quote(cargo)}} --lane {{lane}} nextest run -p quanta-index-searchd-runtime --test native_index_uds \
        --no-default-features --locked --offline --no-tests fail \
        -E 'test(file_body_quarantine)'

rust-test-state-migration-owner lane="test-state-migration-owner-lane":
    uv run --frozen --extra dev python tools/ci/run-local-test-scope.py state-migration-owner --lane {{lane}}

rust-test-state-migration-owner-lib lane="test-state-migration-owner-lane":
    QUANTA_INDEX_TEST_REQUIRE_PINNED_POTION_MODEL=1 \
        QUANTA_INDEX_TEST_POTION_CODE_MODEL_DIR="${QUANTA_INDEX_TEST_POTION_CODE_MODEL_DIR:-$(./scripts/quanta-index-env.sh)/models/potion-code-16M-v2-e9d2a44}" \
        uv run --frozen --extra dev python tools/ci/run-local-test-scope.py state-migration-owner-lib --lane {{lane}}

rust-test-state-migration-release:
    uv run --frozen --extra dev python tools/ci/run-local-test-scope.py state-migration-release

rust-test-sdk-binding-owner lane="test-sdk-binding-owner-lane":
    uv run --frozen --extra dev python tools/ci/run-local-test-scope.py sdk-binding-owner --lane {{lane}}

rust-test-sdk-binding-owner-lib lane="test-sdk-binding-owner-lane":
    uv run --frozen --extra dev python tools/ci/run-local-test-scope.py sdk-binding-owner-lib --lane {{lane}}

rust-test-provider-boundary-owner lane="test-provider-boundary-owner-lane":
    uv run --frozen --extra dev python tools/ci/run-local-test-scope.py provider-boundary-owner --lane {{lane}}

rust-test-runtime-supervisor-owner lane="test-runtime-supervisor-owner-lane":
    uv run --frozen --extra dev python tools/ci/run-local-test-scope.py runtime-supervisor-owner --lane {{lane}}

# Real outstanding SQLite VM close/retry without changing production unsafe policy.
rust-test-catalog-checked-close lane="test-integration-lane":
    {{cargo}} --lane {{lane}} test --manifest-path tools/testing/catalog-checked-close/Cargo.toml --locked --offline --all-targets

rust-test-integration-fast lane="test-integration-lane":
    uv run --frozen --extra dev python tools/ci/run-local-test-scope.py integration-fast --lane {{lane}}

rust-test-integration-storage lane="test-integration-lane":
    uv run --frozen --extra dev python tools/ci/run-local-test-scope.py integration-storage --lane {{lane}}

rust-test-integration-semantic lane="test-integration-lane":
    uv run --frozen --extra dev python tools/ci/run-local-test-scope.py integration-semantic --lane {{lane}}

rust-test-integration lane="test-integration-lane":
    uv run --frozen --extra dev python tools/ci/run-local-test-scope.py integration-fast --lane {{lane}}
    uv run --frozen --extra dev python tools/ci/run-local-test-scope.py integration-storage --lane {{lane}}
    uv run --frozen --extra dev python tools/ci/run-local-test-scope.py integration-semantic --lane {{lane}}

[private]
require-native-legacy-searchd:
    #!/usr/bin/env python3
    import hashlib
    import os
    import re
    import sys
    from pathlib import Path

    executable = Path(os.environ.get("QUANTA_INDEX_LEGACY_SEARCHD_BIN", ""))
    expected = os.environ.get("QUANTA_INDEX_LEGACY_SEARCHD_SHA256", "")
    if (
        not executable.is_absolute()
        or executable.is_symlink()
        or not executable.is_file()
        or not os.access(executable, os.X_OK)
    ):
        sys.exit("QUANTA_INDEX_LEGACY_SEARCHD_BIN must be an absolute regular executable file")
    if re.fullmatch("[a-f0-9]{64}", expected) is None:
        sys.exit("QUANTA_INDEX_LEGACY_SEARCHD_SHA256 must be 64 lowercase hex characters")
    digest = hashlib.sha256()
    try:
        with executable.open("rb") as source:
            for block in iter(lambda: source.read(1024 * 1024), b""):
                digest.update(block)
    except OSError as error:
        sys.exit(f"could not read the pinned Legacy searchd: {error}")
    if digest.hexdigest() != expected:
        sys.exit("Legacy searchd SHA-256 differs from the pinned input")

# Native contract checkpoint. Legacy-backed fixtures require a pinned old-Q input.
rust-test-native-index: require-native-legacy-searchd
    {{cargo}} nextest run --locked --lib --test native_lifecycle --test native_bootstrap --test native_projection --test native_semantic_projection --test native_semantic_certified_chunks --test native_source_batch --test native_source_rebase --test native_source_resync --test native_graph --test native_index_uds --test native_graph_uds --test native_source_owner_lookup --test native_producer_source_recovery --test native_owned_pump --test native_effort --test native_product_process --test native_fault_edges --test native_query_session --test native_query_session_real --test native_semantic_rebind --test native_noop_cost --test native_object_namespace --test native_object_namespace_process \
        -p quanta-index-contract-base -p quanta-index-contract -p quanta-index-lq-text-normalizer -p quanta-index-core -p quanta-index-storage -p quanta-index-scratch-sqlite -p quanta-index-kernel -p quanta-index-lexical -p quanta-index-repomap -p quanta-index-catalog -p quanta-index-semantic -p quanta-index-sdk -p quanta-index-ipc -p quanta-index-searchd-runtime -p quanta-index-searchctl \
        --test-threads 2 --no-tests fail --no-fail-fast \
        -E 'package(=quanta-index-contract) | package(=quanta-index-lq-text-normalizer) | package(=quanta-index-kernel) | package(=quanta-index-storage) | package(=quanta-index-scratch-sqlite) | (package(=quanta-index-contract-base) & test(/^native_serde::tests/)) | (package(=quanta-index-core) & test(/^index::/)) | (package(=quanta-index-sdk) & test(/^native_uds::tests/)) | (package(=quanta-index-ipc) & test(/owned_connection_threads|^native_protocol::tests::|^server::tests::.*deadline|^server::tests::cancelled_stream_read_and_write_escape_codec_retry/)) | (package(=quanta-index-searchd-runtime) & (test(/^native(_|::)/) | binary(=native_index_uds) | binary(=native_graph_uds) | binary(=native_source_owner_lookup) | binary(=native_producer_source_recovery) | binary(=native_owned_pump) | binary(=native_effort) | binary(=native_product_process))) | (package(=quanta-index-searchctl) & test(/^tests::native::/)) | ((package(=quanta-index-lexical) | package(=quanta-index-repomap) | package(=quanta-index-catalog) | package(=quanta-index-semantic)) & test(/^native::/))'

# Bounded ANN store, Lance query, and small materialization regressions.
rust-test-native-ann-read lane="test-workspace-lane":
    {{cargo}} --lane {{lane}} nextest run --locked --offline -p quanta-index-semantic --lib \
        --no-tests fail --no-fail-fast --test-threads 1 \
        -E 'test(/^native::ann_lance::(physical_query_tests|trained_tests)::/) | test(/^native::ann_store::read_scope::tests::/) | test(/::(cancelled_ann_query_settles_started_batch_before_returning|first_batch_error_signals_cooperative_tail_and_wins_settlement|exact_region_obeys_selected_filter_and_releases_query_memory|filtered_refill_preserves_cumulative_cap_and_original_work|refill_prefix_schedule_matches_cumulative_work_oracle|refill_skips_repeated_candidate_and_refuses_changed_stamp|rescore_window_still_checks_repeated_candidate_stamp|near_full_initial_prefix_does_not_reserve_unneeded_refill_memory|lance_selected_region_evicts_cached_members_before_required_decode|lance_selected_region_reuses_authenticated_multivector_artifact|native_trained_ninth_region_quality_separates_routing_loss|lance_replays_owner_quota_even_when_the_result_is_full|lance_sparse_owner_scan_fits_bounded_query_memory|lance_tight_owner_limit_visits_relevant_region_first|mixed_ann_ranks_current_delta_member_without_reopening_it|native_materialization_installed_region_observation_has_seal_owner)$/)'

# Small Native owner publication, Lance writes, and virtual-time readiness.
rust-test-native-owner-local lane="test-workspace-lane":
    {{cargo}} --lane {{lane}} nextest run --locked --offline -p quanta-index-catalog --lib \
        --no-tests fail --no-fail-fast --test-threads 1 \
        -E 'test(/::ready_work_lanes_use_ordered_bounded_indexes$/)'
    {{cargo}} --lane {{lane}} nextest run --locked --offline -p quanta-index-kernel --test native_lifecycle --test native_query_session_real \
        --no-tests fail --no-fail-fast --test-threads 1 \
        -E 'test(=lexical_cold_source_advance::product_shaped_cold_text_source_advance_preserves_boundary_work_and_postings) | test(=ann_regions::source_local_ann_one_owner_materialize_keeps_unrelated_pending_and_traces_phases)'
    {{cargo}} --lane {{lane}} nextest run --locked --offline -p quanta-index-semantic --lib \
        --no-tests fail --no-fail-fast --test-threads 1 \
        -E 'test(/::(lance_materialization_carries_prior_region_without_reopening_member_artifact|lance_materialization_seals_at_256_rows_before_owner_limit)$/)'
    {{cargo}} --lane {{lane}} nextest run --locked --offline -p quanta-index-searchd-runtime --test native_product_process \
        --no-tests fail --no-fail-fast --test-threads 1 \
        -E 'test(=ann_quality::readiness_probe_rejects_malformed_complete_coverage) | test(=ann_quality::work_status::tests::product_size_readiness_separates_available_receipts_from_observation_deadline)'

# Native query handoffs on the in-process service fixture.
rust-test-native-ann-runtime lane="test-workspace-lane":
    {{cargo}} --lane {{lane}} nextest run --locked --offline -p quanta-index-searchd-runtime --test native_effort \
        --no-tests fail --no-fail-fast --test-threads 1 \
        -E 'test(=in_process_scoped_semantic_and_distinct_effort_scope_renew_control_without_pin_leaks) | test(=in_process_fast_auto_unknown_and_deep_use_real_old_view) | test(=auto_without_configured_semantic_reader_keeps_fast_result) | test(=optional_deep_refusal_preserves_fast_when_enrichment_envelope_is_smaller) | test(=observer_cancellation_after_fast_blocks_deep_and_releases_pin) | test(=terminal_observer_failure_releases_pin_and_allows_next_deep_query)'

# Small daemon lifecycle and independent product-phase oracles.
rust-test-native-ann-daemon-smoke lane="test-workspace-lane":
    {{cargo}} --lane {{lane}} nextest run --locked --offline -p quanta-index-searchd-runtime --test native_product_process \
        --no-tests fail --no-fail-fast --test-threads 1 \
        -E 'test(=product_daemon_owns_openai_semantic_materialization_query_scope_and_reopen) | test(/::(product_size_readiness_separates_available_receipts_from_observation_deadline|readiness_probe_rejects_malformed_complete_coverage|readiness_probes_coverage_once_per_published_view|selected_change_phase_refuses_expired_original_deadline_before_work|exact_path_completion_oracle_rejects_capped_approximate_and_stale_results|quality_result_oracle_rejects_wrong_score_and_owner_order|overlay_fast_oracle_rejects_empty_and_durable_replacement_hits|overlay_quality_oracle_rejects_misordered_page_and_foreign_revision|restart_provider_oracle_rejects_reembedded_corpus|restarted_lance_oracle_requires_index_record_for_trained_rows|prefix_catalog_rejects_foreign_and_terminal_scope_rows|sealed_region_oracle_rejects_wrong_coverage_binding)$/)'

# History fixtures that consume a pinned old-Q input. Require each selected body to run.
rust-test-native-history lane="test-workspace-lane": require-native-legacy-searchd
    #!/usr/bin/env bash
    set -euo pipefail
    package=quanta-index-searchd-runtime
    history=read_sessions::query::history
    targets=(--test native_index_uds)
    cases=(
        "native_index_uds:$history::native_history_sdk_full_delta_retained_pages_and_reopen"
        "native_index_uds:$history::native_history_append_original_publishes_without_legacy_ack_and_reopens"
        "native_index_uds:$history::staged_source_and_history_bootstrap_publishes_changed_profile_and_reopens"
        "native_index_uds:$history::source_and_next_history_generation_publish_in_one_view_and_reopen"
        "native_index_uds:$history::native_restore::native_history_restore_full_append_and_reopen_cross_root"
        "native_index_uds:$history::dsl_fixture::native_history_dsl_fixed_ledger_warm_cold"
    )
    if [[ {{os()}} == linux ]]; then
        targets+=(--test native_product_process)
        cases+=(native_index_uds:$history::native_protected::native_protected_two_source_heads_import_seal_and_reopen)
        cases+=(native_product_process:native_boot::offline_legacy_history_export_streams_real_rows_and_terminal_ack)
        cases+=(native_product_process:native_boot::legacy_to_fresh_native_preserves_old_root_and_refuses_unverified_rollback)
    fi
    filterset=
    for case in "${cases[@]}"; do
        filterset+="${filterset:+ | }(binary(=${case%%:*}) & test(=${case#*:}))"
    done
    temp_root="$(cd "${TMPDIR:-/tmp}" && pwd -P)"
    inventory="$(mktemp "$temp_root/quanta-history-inventory.XXXXXX")"
    trap 'rm -f "$inventory"' EXIT
    selection=(--locked --no-default-features -p "$package" "${targets[@]}" -E "$filterset")
    {{quote(cargo)}} --lane {{quote(lane)}} nextest list "${selection[@]}" \
        --message-format json > "$inventory"
    export NEXTEST_EXPERIMENTAL_LIBTEST_JSON=1
    events="$(
        {{quote(cargo)}} --lane {{quote(lane)}} nextest run "${selection[@]}" \
            --no-tests fail --no-fail-fast \
            --message-format libtest-json-plus --message-format-version 0.1 \
            --status-level pass --final-status-level fail
    )" || {
        status=$?
        printf '%s\n' "$events" >&2
        exit "$status"
    }
    printf '%s\n' "$events" | python3 tools/ci/history_gate_events.py \
        --inventory "$inventory" "$package" test "${cases[@]}"

# Focused Catalog and Kernel History ownership checks without installed binaries.
rust-test-native-history-owners lane="test-workspace-lane":
    #!/usr/bin/env bash
    set -euo pipefail
    export NEXTEST_EXPERIMENTAL_LIBTEST_JSON=1
    run_owner() {
        local package="$1" binary="$2"
        shift 2
        local cases=() filterset= events status
        for name in "$@"; do
            cases+=("$binary:$name")
            filterset+="${filterset:+ | }(binary(=$binary) & test(=$name))"
        done
        events="$(
            {{quote(cargo)}} --lane {{quote(lane)}} nextest run --locked --offline \
                -p "$package" --lib --no-tests fail --no-fail-fast --test-threads 1 \
                -E "$filterset" --message-format libtest-json-plus \
                --message-format-version 0.1 --status-level pass --final-status-level fail
        )" || {
            status=$?
            printf '%s\n' "$events" >&2
            exit "$status"
        }
        printf '%s\n' "$events" | python3 tools/ci/history_gate_events.py \
            "$package" lib "${cases[@]}"
    }
    run_owner quanta-index-catalog quanta_index_catalog \
        native::authority::tests::offline_protected_routes::protected_route_refuses_corrupt_registered_edge_before_visiting_publications \
        native::authority::tests::offline_protected_routes::orphan_view_root_with_registered_workspace_refuses_current_and_retained_routes \
        native::authority::tests::offline_source_quiescence::single_workspace_boot_and_rollback_refuse_orphan_view_root \
        native::authority::tests::offline_source_quiescence::offline_head_checks_charge_current_and_history_payloads_before_decode \
        native::authority::tests::read_ticket_holds_scratch_through_pin_seal_and_refunds_it \
        native::authority::tests::read_ticket_refuses_root_scratch_without_pin_or_leaked_lease \
        native::authority::tests::materialization_pin_is_atomic_and_does_not_extend_execution_claim
    run_owner quanta-index-kernel quanta_index_kernel \
        request_decode::tests::source_history_resync_refunds_retained_backing_when_history_input_is_missing \
        request_decode::tests::source_history_resync_requires_exact_decode_budget_and_refunds_it \
        request_decode::tests::initial_and_post_reservation_original_cancel_never_become_corruption

# Exercise the published Native DSL through the freshly built release CLI.
rust-test-native-dsl-cli:
    #!/usr/bin/env bash
    set -euo pipefail
    stream="$(mktemp)"
    trap 'rm -f -- "$stream"' EXIT
    just rust-build-release-cli --offline --message-format=json-render-diagnostics > "$stream"
    QUANTA_INDEX_NATIVE_DSL_CLI_BIN="$(python3 - "$stream" <<'PY'
    import json
    import sys
    from pathlib import Path

    executable = set()
    with open(sys.argv[1], encoding="utf-8") as stream:
        for line in stream:
            record = json.loads(line)
            target = record.get("target", {})
            if (record.get("reason") == "compiler-artifact" and target.get("kind") == ["bin"]
                    and target.get("name") == "quanta-index-searchctl" and record.get("executable")):
                executable.add(Path(record["executable"]))
    if len(executable) != 1:
        raise SystemExit("expected one executable from the release searchctl build")
    path = executable.pop()
    if not path.is_absolute() or not path.is_file():
        raise SystemExit("release searchctl executable is missing")
    print(path)
    PY
    )"
    cli_sha_line="$(shasum -a 256 "$QUANTA_INDEX_NATIVE_DSL_CLI_BIN")"
    export QUANTA_INDEX_NATIVE_DSL_CLI_BIN
    export QUANTA_INDEX_NATIVE_DSL_CLI_SHA256="${cli_sha_line%% *}"
    printf 'Native DSL release CLI %s %s\n' "$QUANTA_INDEX_NATIVE_DSL_CLI_SHA256" "$QUANTA_INDEX_NATIVE_DSL_CLI_BIN"
    {{cargo}} --lane test-daemon-lane nextest run --locked --offline --no-default-features \
        -p quanta-index-searchd-runtime --test native_index_uds --run-ignored only --no-tests fail \
        -E 'test(=read_sessions::dsl::native_dsl_twelve_retained_rows_match_pinned_cli_v1)'

# Small Layered V2 restart case with live phase diagnostics.
rust-test-native-lexical-local lane="test-workspace-lane":
    {{quote(cargo)}} --lane {{quote(lane)}} nextest run --locked --offline --no-default-features \
        -p quanta-index-searchd-runtime --test native_product_process \
        --no-tests fail --no-capture \
        -E 'test(=large_restart::native_local_lexical_publish_rank_and_reopen)'

# Heavy: SDK text over a 4,097-file view is answered from Literal grams.
rust-test-native-literal-anchor lane="test-workspace-lane":
    {{quote(cargo)}} --lane {{quote(lane)}} nextest run --locked --offline --no-default-features \
        -p quanta-index-searchd-runtime --test native_index_uds \
        --run-ignored only --no-tests fail --test-threads 1 \
        -E 'test(=read_sessions::query::literal_anchor::sdk_text_over_large_view_reads_only_literal_anchored_bodies) | test(=read_sessions::query::literal_anchor::structural_bound_answers_rare_and_common_and_refuses_rare_or_common)'

# Heavy: the same 4,097-file Literal-anchor oracle on SegmentedLayeredV2.
rust-test-native-layered-literal-anchor lane="test-workspace-lane":
    {{quote(cargo)}} --lane {{quote(lane)}} nextest run --locked --offline --no-default-features \
        -p quanta-index-searchd-runtime --test native_index_uds \
        --run-ignored only --no-tests fail --test-threads 1 \
        -E 'test(=read_sessions::query::literal_anchor::sdk_text_over_large_layered_view_reads_only_literal_anchored_bodies)'

# Heavy: retained text paging read to the end over 100 and 1,000 Sources, without and with the rank-order cache (09.4a/09.4b).
rust-test-native-page-cost lane="test-workspace-lane":
    {{quote(cargo)}} --lane {{quote(lane)}} nextest run --locked --offline --no-default-features \
        -p quanta-index-searchd-runtime --test native_index_uds \
        --run-ignored only --no-tests fail --test-threads 1 \
        -E 'test(=read_sessions::query::page_cost::retained_text_pages_rescan_without_the_order_cache_and_reuse_it_with_it)'

# Pin a score tie across DELTA publication and recover after a hard exit.
rust-test-native-lexical-hellgate lane="test-workspace-lane":
    {{quote(cargo)}} --lane {{quote(lane)}} nextest run --locked --offline --no-default-features \
        -p quanta-index-searchd-runtime --test native_product_process \
        --no-tests fail --no-capture \
        -E 'test(=large_restart::native_local_lexical_delta_interleaves_ties_and_survives_sigkill)'

# Native Text reader, posting integrity, and bounded query unit tests.
rust-test-native-lexical-unit lane="test-workspace-lane":
    {{quote(cargo)}} --lane {{quote(lane)}} nextest run --locked --offline --no-default-features \
        -p quanta-index-lexical --lib --no-tests fail --no-fail-fast --test-threads 1 \
        -E 'test(file_contains_tests::) | test(text_literal_anchor::tests::) | test(count_integration_tests::) | test(anchored_union_uses_one_exact_grant_for_multiple_sets) | test(conjunctive_admission::) | test(structural_bound_follows_candidate_set_algebra) | test(structural_non_candidates_never_satisfy_any_source_outcome) | test(path_write_batch_trace_preserves_sql_edits_and_work) | test(shared_physical_segment_checks_each_live_partition_mask) | test(v2_frequency_scan_) | test(unanchored_universe_refuses_oversize_before_scan_and_late_corruption) | test(excluded_change_drops_window_before_root_install) | test(native_root_json_is_funded_through_put_and_refusal_preserves_wire_commitment) | test(read_capability_admits_only_its_frozen_text_instance) | test(layered_empty_root_prepares_with_a_small_memory_grant) | test(layered_two_segment_mixed_phrase_and_keyword_preserves_positional_score) | test(native_phrase_boolean_not_filters_without_duplicating_positive_term_score) | test(native_phrase_late_cancellation_discards_the_provisional_ranked_hit) | test(packed_literal_membership_reuses_authenticated_blocks) | test(packed_membership_cache_reselects_descending_fence_and_binds_root) | test(posting_scalar_window_) | test(leaf_reads_remain_uncached_and_typed_as_leaf) | test(packed_load_requires_a_checked_block)'

# Native Text and Literal correctness, retained reads and recovery.
rust-test-native-lexical lane="test-workspace-lane": (rust-test-native-lexical-unit lane)
    {{quote(cargo)}} --lane {{quote(lane)}} nextest run --locked --offline --no-default-features \
        -p quanta-index-sdk --lib --no-tests fail --no-fail-fast --test-threads 1 \
        -E 'test(native_code_search::tests::) | test(observed_lexical_) | test(/^native_uds::read_session::(page|path_page_v1)::tests::/) | test(/^native_uds::tests::(literal_query_rejects_malformed_pages_and_owner_cursors|literal_continuation_requires_exact_view_before_connecting|path_query_rejects_malformed_pages_and_moving_continuations)$/)'
    {{quote(cargo)}} --lane {{quote(lane)}} nextest run --locked --offline --no-default-features \
        -p quanta-index-kernel --test native_lifecycle --test native_bootstrap \
        --no-tests fail --no-fail-fast --test-threads 1 \
        -E 'test(=lexical_cold_source_advance::product_shaped_cold_text_source_advance_preserves_boundary_work_and_postings) | test(=occurrence_blocks::native_incremental_text_matches_fresh_full_rank_and_score) | test(=literal_patch_does_not_walk_unaffected_directory_pages) | test(=existing_corpus_installs_all_adapters_atomically_across_pages_and_reopens) | test(=literal_seal::literal_patch_second_directory_diff_page_failure_preserves_old_view)'
    {{quote(cargo)}} --lane {{quote(lane)}} nextest run --locked --offline --no-default-features \
        -p quanta-index-searchd-runtime --test native_index_uds --test native_product_process \
        --no-tests fail --no-fail-fast --test-threads 1 \
        -E 'test(=native_uds_text_query_binds_pinned_view_and_fixed_bm25_gold) | test(/^read_sessions::query::(global_text_pages_keep_original_view_across_publication_and_release|text_page_refusals_preserve_session_pin_and_original_deadline)$/) | test(=read_sessions::query::code_search_frontdoor::sdk_text_cursor_keeps_its_view_after_a_later_publication_and_retention) | test(=read_sessions::excluded_text::excluded_binary_and_utf8_source_stay_path_only_through_update_noop_delete_and_reopen) | test(=native_uds_source_lifecycle_status_old_view_and_reopen) | test(=native_kill_reopen_retains_view_and_reclaims_owned_scratch) | test(=large_restart::native_local_lexical_delta_interleaves_ties_and_survives_sigkill) | test(/^text_shard_contract::native_(multigeneration_delta_matches_model_and_independent_full_after_reopen|boundary_delta_matches_independent_full_and_cold_reopen|multipage_lexical_delta_survives_sigkill_and_matches_full|lexical_partial_mutation_page_recovers_after_sigkill|lexical_commit_rollback_and_truncated_published_ack_preserve_one_operation)$/) | test(/^segmented_layered::(selected_native_text_posting_page_faults_are_typed|layered_text_delta_refuses_a_corrupt_selected_base_posting_page|layered_text_sixteen_live_layers_compact_without_losing_owners|layered_text_full_patch_pinned_views_and_restart|layered_text_last_pin_releases_old_segment_and_posting_files)$/)'

rust-test-cli-smoke lane="test-cli-smoke-lane":
    uv run --frozen --extra dev python tools/ci/run-local-test-scope.py cli-smoke --lane {{lane}}

rust-bench-dsl-truth:
    {{cargo}} --lane test-daemon-lane nextest run -p quanta-index-searchd-harness --test dsl_scenario_truth --no-default-features --locked --no-tests fail

rust-verify-hellgate-fast: require-native-legacy-searchd
    @just rust-bench-dsl-truth
    {{cargo}} --lane test-daemon-lane test -p quanta-index-search-plane --lib --all-features --locked -- --nocapture
    {{cargo}} --lane test-daemon-lane nextest run -p quanta-index-searchd-runtime --test process_readiness_owner_v1 --no-default-features --locked -E 'test(/^(legacy_text_route_hellgate|legacy_structural_hellgate)::/)' --no-tests fail --success-output final
    python3 tools/benchmark/sourcegraph_parity.py --check
    python3 tools/ci/lint/check-dsl-capability-truth.py

rust-verify-hellgate-broad: require-native-legacy-searchd
    {{cargo}} --lane test-daemon-lane nextest run -p quanta-index-searchd-runtime --test runtime_fast_suite --test process_readiness_owner_v1 --no-default-features --locked -E 'test(/^(sdk_frontdoor|legacy_sdk_code_search_frontdoor|end_to_end|e2e_restart_replay_determinism|legacy_lexical_semantic_replay_sdk|e2e_perf_chaos|explain|repo_map_end_to_end)::/) | (binary(=process_readiness_owner_v1) & test(/^legacy_dirty_only_route::reopen_preserves_runtime_/))' --no-tests fail --success-output final

# The signer is inherited here. Bash -p suppresses shell startup hooks.
[positional-arguments]
rust-verify-hellgate-cross-repo semantica_root="/Users/songmin/Documents/code-new/semantica-codegraph-v2":
    #!/bin/bash -p
    exec ./scripts/verify-repomap-cross-repo.sh "$1"

rust-verify-hellgate-all samples="20":
    @just rust-verify-hellgate-fast
    @just rust-verify-hellgate-broad
    @just rust-bench-dsl-refresh {{samples}}

rust-test-e2e-fast lane="test-daemon-lane": require-native-legacy-searchd
    uv run --frozen --extra dev python tools/ci/run-local-test-scope.py daemon-fast --lane {{lane}}

rust-test-e2e lane="test-daemon-lane": require-native-legacy-searchd
    uv run --frozen --extra dev python tools/ci/run-local-test-scope.py daemon --lane {{lane}}

rust-test-e2e-all lane="test-daemon-lane": require-native-legacy-searchd
    uv run --frozen --extra dev python tools/ci/run-local-test-scope.py daemon-all --lane {{lane}}

# Ordinary SDK wire behavior and retained Native File/Path and DSL over UDS.
rust-test-native-frontdoor lane="test-workspace-lane":
    #!/usr/bin/env bash
    set -euo pipefail
    sdk_package=quanta-index-sdk
    sdk_binary=sdk_native_default
    runtime_package=quanta-index-searchd-runtime
    runtime_binary=native_index_uds
    preview=read_sessions::query::preview
    dsl=read_sessions::dsl
    sdk_cases=(
        ordinary_sdk_requires_native_selection_before_transport_birth
        wire::ordinary_sdk_native_reader_preserves_file_page_hybrid_and_vector_dtos
        wire::ordinary_sdk_query_only_has_no_native_write_capability
        wire::ordinary_sdk_native_cancel_deadline_and_zero_key_do_not_send
        wire::ordinary_sdk_native_rejects_wrong_full_instance_spec_before_query
        wire::ordinary_sdk_profile_failure_keeps_acknowledged_session_for_release
        wire::ordinary_sdk_native_unknown_session_is_not_reopened_or_queried
        wire::ordinary_sdk_native_diagnostics_follow_actual_native_send_outcomes
        wire::ordinary_sdk_admitted_reader_keeps_owner_and_checks_captured_role_before_send
        wire::ordinary_sdk_admitted_profile_denial_preserves_ack_and_charged_error_until_release
    )
    runtime_cases=(
        repository_paths::native_uds_repository_paths_page_same_path_and_bind_cursor
        "$preview::native_select_path_combines_path_and_body_matches_after_cold_reopen"
        "$preview::native_text_preview_keeps_rank_and_source_coordinates_across_reopen"
        "$preview::native_text_preview_keeps_source_byte_boundary_and_refuses_oversized"
        "$preview::native_text_preview_work_refusal_keeps_ranked_suffix_after_cold_reopen"
        "$preview::native_select_path_rename_and_delete_survive_published_cold_reopen"
        "$preview::native_select_path_retained_pages_survive_successor_rename_delete_and_reopen"
        "$dsl::native_dsl_twelve_retained_rows_match_independent_gold_v1"
        "$dsl::native_dsl_retained_cursor_refuses_another_view_and_keeps_original_rows_v1"
    )
    expected=()
    filterset=
    register_cases() {
        local package="$1" binary="$2" name
        shift 2
        for name in "$@"; do
            expected+=("$package::$binary\$$name")
            filterset+="${filterset:+ | }(package(=$package) & binary(=$binary) & test(=$name))"
        done
    }
    register_cases "$sdk_package" "$sdk_binary" "${sdk_cases[@]}"
    register_cases "$runtime_package" "$runtime_binary" "${runtime_cases[@]}"
    selection=(--locked --offline --no-default-features
        -p "$sdk_package" --test "$sdk_binary" -p "$runtime_package" --test "$runtime_binary"
        -E "$filterset")
    temp_root="$(cd "${TMPDIR:-/tmp}" && pwd -P)"
    inventory="$(mktemp "$temp_root/quanta-frontdoor-inventory.XXXXXX")"
    trap 'rm -f -- "$inventory"' EXIT
    validate_frontdoor() {
        python3 -c '
    import sys
    from pathlib import Path
    from tools.ci.nextest_events import NextestEvidenceError, parse_nextest_bytes, parse_nextest_inventory

    stage, inventory_path, *names = sys.argv[1:]
    required = set(names)
    if not required or len(names) != len(required):
        sys.exit("Native frontdoor gate: duplicate or empty test roster")
    try:
        inventory = parse_nextest_inventory(Path(inventory_path))
        if set(inventory) != required:
            missing = sorted(required - inventory.keys())
            unexpected = sorted(inventory.keys() - required)
            raise NextestEvidenceError(
                f"nextest collection differs from the required tests: missing={missing}, unexpected={unexpected}"
            )
        if stage == "list":
            sys.exit(0)
        result = parse_nextest_bytes(sys.stdin.buffer.read(), inventory)
    except NextestEvidenceError as error:
        sys.exit(f"Native frontdoor gate: {error}")
    print(f"Native frontdoor: {result.passed}/{len(required)} passed")
    ' "$1" "$inventory" "${expected[@]}"
    }
    {{quote(cargo)}} --lane {{quote(lane)}} nextest list "${selection[@]}" \
        --message-format json > "$inventory"
    validate_frontdoor list </dev/null
    export NEXTEST_EXPERIMENTAL_LIBTEST_JSON=1
    events="$(
        {{quote(cargo)}} --lane {{quote(lane)}} nextest run "${selection[@]}" \
            --no-tests fail --no-fail-fast --test-threads 2 \
            --message-format libtest-json-plus --message-format-version 0.1 \
            --status-level pass --final-status-level fail \
    )" || {
        status=$?
        printf '%s\n' "$events" >&2
        exit "$status"
    }
    printf '%s\n' "$events" | validate_frontdoor run

# Requires a pinned old-Q debug daemon with the Legacy crash hooks enabled.
rust-test-legacy-crash-compat lane="test-daemon-lane": require-native-legacy-searchd
    {{cargo}} --lane {{lane}} nextest run -p quanta-index-searchd-runtime --test legacy_crash_compat --no-default-features --locked --offline --run-ignored only --no-tests fail

rust-test-pyramid: require-native-legacy-searchd
    @just rust-test-unit
    @just rust-test-integration
    @just rust-test-cli-smoke
    @just rust-test-native-frontdoor
    @just rust-test-e2e

rust-doc:
    RUSTDOCFLAGS="-D warnings" {{cargo}} --lane doc-lane doc --workspace --all-features --no-deps --locked
    RUSTDOCFLAGS="-D warnings" {{cargo}} --lane sqlite-oracle-lane doc --manifest-path {{sqlite_fd_ledger_manifest}} --all-features --no-deps --locked --offline

rust-msrv:
    RUSTUP_TOOLCHAIN=1.92.0 {{cargo}} --lane msrv-lane test --workspace --all-targets --all-features --locked --no-run
    RUSTUP_TOOLCHAIN=1.92.0 {{cargo}} --lane sqlite-oracle-lane check --manifest-path {{sqlite_fd_ledger_manifest}} --all-targets --all-features --locked --offline

rust-bench:
    {{cargo}} --lane bench-lane bench --workspace --all-features --locked

rust-bench-build:
    {{cargo}} --lane bench-lane bench --workspace --all-features --locked --no-run

# Machine-checkable benchmark control-plane policy: registry validity, every
# Cargo bench target registered, no production->benchmark dependency, and no CI
# direct producer/comparator bypass. Cheap but requires cargo metadata.
benchmark-policy-local:
    uv run --frozen --extra dev python tools/ci/lint/check-benchmark-policy.py --print-registry-digest

# Print the resolved, digest-bound plan for one registered profile (no mutation).
benchmark-plan profile:
    uv run --frozen --extra dev python tools/benchmark/benchctl.py plan {{quote(profile)}}

# Fresh-process re-validation of one immutable run (run id or run directory).
benchmark-replay run:
    uv run --frozen --extra dev python tools/benchmark/benchctl.py replay {{quote(run)}}

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
    uv run --frozen --extra dev python -m pytest tools/ci/tests/test_live_lexical_external.py tools/ci/tests/test_opengrok_index_scope.py tools/ci/tests/test_opengrok_query_witness.py tools/ci/tests/test_opengrok_query_fixture.py tools/ci/tests/test_code_search_workflow.py tools/ci/tests/test_code_search_matrix.py tools/ci/tests/test_benchmark_manifest.py tools/ci/tests/test_benchmark_policy.py tools/ci/tests/test_bench_protocol_conformance.py tools/ci/tests/test_benchmark_evidence_bridge.py tools/ci/tests/test_benchmark_source_closure.py tools/ci/tests/test_benchctl.py tools/ci/tests/test_relevance_openai_ab_capture.py tools/ci/tests/test_benchmark_profile_capture.py tools/ci/tests/test_criterion_capture.py tools/ci/tests/test_recorded_capture.py tools/ci/tests/test_retrieval_capture.py tools/ci/tests/test_lexical_capture.py tools/ci/tests/test_pair_capture.py tools/ci/tests/test_lexical_file_comparison.py tools/ci/tests/test_lexical_five_product_oracle.py tools/ci/tests/test_no_answer_evidence.py tools/ci/tests/test_native_file_evidence.py tools/ci/tests/test_native_file_cohort.py tools/ci/tests/test_native_file_decision.py tools/ci/tests/test_native_file_decision_cli.py tools/ci/tests/test_native_file_capture.py tools/ci/tests/test_native_release_approval.py tools/ci/tests/test_composite_source_binding.py tools/ci/tests/test_identifier_robustness_report.py tools/ci/tests/test_identifier_robustness_strata.py tools/ci/tests/test_codesearchnet_qrels.py tools/ci/tests/test_codesearchnet_materialize.py tools/ci/tests/test_clarc_adapter.py tools/ci/tests/test_external_snippet_benchmark.py tools/ci/tests/test_holdout_c4_projection.py tools/ci/tests/test_identifier_robustness_fresh_join.py tools/ci/tests/test_identifier_robustness_multiproduct_report.py tools/ci/tests/test_portable_proof.py tools/ci/tests/test_check_bench_artifacts.py tools/ci/tests/test_check_host_contention.py tools/ci/tests/test_compare_dsl_bench.py tools/ci/tests/test_quality_integration_summary.py tools/ci/tests/test_retrieval_contract_proof.py tools/ci/tests/test_retrieval_sdk_proof.py tools/ci/tests/test_write_verification_receipt.py tools/ci/tests/test_agent_outcome_benchmark.py tools/ci/tests/test_concurrency_sample_contract.py tools/ci/tests/test_corpus_release.py tools/ci/tests/test_tool_custody.py tools/ci/tests/test_portable_tool_execution.py tools/ci/tests/test_conditional_window_operations.py tools/ci/tests/test_conditional_tool_execution.py tools/ci/tests/test_corpus_binding.py tools/ci/tests/test_gold_oracle.py tools/ci/tests/test_nextest_ignored_inventory.py tools/ci/tests/test_producer_notifications.py tools/ci/tests/test_bootstrap_cache.py tools/ci/tests/test_proof_command_timings.py tools/ci/tests/test_resource_admission.py tools/ci/tests/test_cargow_resource_admission.py tools/ci/tests/test_cargo_preparation.py tools/ci/tests/test_pair_replay_workspace.py -q

benchmark-prep-local:
    find crates/quanta-index-searchd-harness/src -name '*.rs' -print0 | xargs -0 rustfmt --check --edition 2024
    @just benchmark-policy-local
    uv run --frozen --extra dev just benchmark-control-contract-local
    uv run --frozen --extra dev python -m py_compile tools/benchmark/benchctl.py tools/benchmark/code_search_workflow.py tools/benchmark/registry.py tools/benchmark/manifest.py tools/benchmark/evidence.py tools/benchmark/evidence_bridge.py tools/benchmark/compare_dsl_bench.py tools/benchmark/quality_integration_summary.py tools/ci/lint/check-bench-artifacts.py tools/ci/lint/check-benchmark-policy.py tools/ci/timing/check_host_contention.py tools/benchmark/retrieval/evaluator.py tools/benchmark/retrieval/native_file_evidence.py tools/benchmark/retrieval/holdout_review.py tools/benchmark/retrieval/source_oracle.py tools/benchmark/retrieval/source_oracle_suite.py tools/benchmark/retrieval/identifier_robustness_suite.py tools/benchmark/retrieval/identifier_robustness_report.py tools/benchmark/retrieval/codesearchnet_qrels.py tools/benchmark/retrieval/codesearchnet_materialize.py tools/benchmark/retrieval/clarc_adapter.py tools/benchmark/retrieval/external_snippet_benchmark.py tools/benchmark/retrieval/identifier_robustness_multiproduct_report.py tools/benchmark/retrieval/identifier_robustness_fresh_join.py tools/benchmark/retrieval/arb_adapter.py tools/benchmark/retrieval/query_plan.py tools/benchmark/retrieval/retrieval_contract.py tools/benchmark/retrieval/semble.py tools/benchmark/retrieval/run.py tools/benchmark/retrieval/lexical_file_comparison.py tools/benchmark/retrieval/lexical_external_oracle.py tools/benchmark/retrieval/lexical_five_product_oracle.py tools/benchmark/retrieval/prepare_lexical_pair.py tools/benchmark/retrieval/contract_proof.py tools/benchmark/retrieval/sdk_proof.py tools/ci/source_closure.py tools/ci/write-verification-receipt.py
    {{cargo}} --lane bench-lane test -p quanta-index-bench-protocol --all-features --locked
    {{cargo}} --lane test-daemon-lane test -p quanta-index-searchd-harness --lib --bins --all-features --locked --no-run
    {{cargo}} --lane test-daemon-lane test -p quanta-index-searchd-harness --lib --all-features --locked
    uv run --frozen --extra dev python tools/ci/lint/check-test-authority.py
    git diff --check

# Dirty-checkout retrieval edit loop. This is diagnostic only: use the proof
# rail on a clean source to produce source-bound JUnit/nextest receipts.
retrieval-contract-local:
    uv run --frozen --extra dev python tools/benchmark/retrieval/proof_inventory.py --run-local
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

# Exploratory criterion view for warm scenarios. Not gate authority.
rust-bench-dsl-warm-criterion $evidence_root:
    uv run --frozen --extra dev python tools/benchmark/benchctl.py run dsl-diagnostic --evidence-root "$evidence_root"

# Native DSL SDK diagnostic for the complete retained roster and pinned History fixture.
rust-bench-dsl-native $mode $runner $runner_sha $daemon $daemon_sha $history $history_sha $out *options:
    {{retrieval_python}} tools/benchmark/run_dsl_cold_matrix.py --bin-mode "$mode" --native-runner "$runner" --bin-sha256 "$runner_sha" --native-daemon "$daemon" --native-daemon-sha256 "$daemon_sha" --native-history-fixture "$history" --native-history-sha256 "$history_sha" --out "$out" {{options}}

# Prepare Published History for the official Native DSL plan from a frozen checkout.
rust-prepare-native-dsl-history $export_root $daemon $daemon_sha256: require-native-legacy-searchd
    QUANTA_INDEX_NATIVE_DSL_HISTORY_EXPORT_ROOT="$export_root" QUANTA_INDEX_NATIVE_DSL_DAEMON_BIN="$daemon" QUANTA_INDEX_NATIVE_DSL_DAEMON_SHA256="$daemon_sha256" {{cargo}} --lane test-workspace-lane nextest run --locked --offline --no-default-features -p quanta-index-searchd-runtime --test native_index_uds --no-tests fail -E 'test(=read_sessions::query::history::dsl_fixture::native_history_dsl_fixed_ledger_warm_cold)'

# Authority refresh: run warm and cold producers serially, then gate against baselines.
rust-bench-dsl-refresh samples="20":
    uv run --frozen --extra dev python tools/benchmark/benchctl.py run dsl-authority --cold-samples {{quote(samples)}} --dsl-native-plan "${QUANTA_DSL_NATIVE_PLAN:?set QUANTA_DSL_NATIVE_PLAN to the frozen Native DSL plan path}"

# Relative-regression gate. It fails typed until reviewed canonical baselines exist.
rust-bench-dsl-compare:
    uv run --frozen --extra dev python tools/benchmark/benchctl.py compare dsl-authority

# Sourcegraph filter parity: regenerate the execution-coverage matrix and gate it.
rust-bench-dsl-parity:
    python3 tools/benchmark/sourcegraph_parity.py --check --write

# --------------------------------------------------------------------------
# Search product quality rails. One command proves one quality dimension.
# See docs/adr/JUN-08-001-verification-hellgate-and-benchmark-separation.md
# and tools/benchmark/README.md. Blocking unless a recipe says advisory.
# --------------------------------------------------------------------------

# Relevance ranking reference (J7Q-01A). Requires a pinned Legacy daemon.
# Proves: pure-metric unit tests + adversarial gate tests, then runs the judged
# seeded corpus end to end and enforces per-route MRR@10 / NDCG@10 / Recall@20
# plus top1 / top-k-containment / hard-negative ordering invariants.
# Artifacts: artifacts/search-quality/relevance/latest/{summary,query_judgments,sourcegraph-overlap}.json
# External lexical floor (Sourcegraph overlap, J7Q-01B) is emitted as
# `unprovisioned`, NOT a pass, until a local Sourcegraph instance exists.
rust-verify-quality-relevance: require-native-legacy-searchd
    {{cargo}} --lane test-daemon-lane test -p quanta-index-searchd-harness --lib relevance:: --no-default-features --locked -- --include-ignored --nocapture --skip relevance::report::process::tests::pinned_hash_reference_publishes_and_scores_the_full_roster
    mkdir -p artifacts/search-quality/relevance/latest
    {{cargo}} --lane test-daemon-lane build -p quanta-index-searchd-harness --bin relevance_matrix --no-default-features --locked
    env QUANTA_INDEX_EMBEDDER=potion-code QUANTA_INDEX_BUILD_LANE=test-daemon-lane QUANTA_QUALITY_CAPTURE_DATE="$(date -u +%Y-%m-%d)" bash -lc 'source scripts/quanta-index-env.sh && BIN="$CARGO_TARGET_DIR/debug/relevance_matrix" && test -x "$BIN" && "$BIN" --out-dir "$(pwd)/artifacts/search-quality/relevance/latest"'

# Hermetic mechanical regression rail. This is not learned semantic quality and
# cannot replace the default potion-code relevance artifact.
rust-verify-quality-relevance-hash-dev: require-native-legacy-searchd
    {{cargo}} --lane test-daemon-lane nextest run --locked --no-default-features -p quanta-index-searchd-harness --lib --run-ignored all --no-tests fail -E 'test(=relevance::report::process::tests::pinned_hash_reference_publishes_and_scores_the_full_roster)'
    {{cargo}} --lane test-daemon-lane build -p quanta-index-searchd-harness --bin relevance_matrix --no-default-features --locked
    mkdir -p artifacts/search-quality/relevance/hash-dev/latest
    env QUANTA_INDEX_EMBEDDER=hash-dev QUANTA_INDEX_BUILD_LANE=test-daemon-lane QUANTA_QUALITY_CAPTURE_DATE="$(date -u +%Y-%m-%d)" bash -lc 'source scripts/quanta-index-env.sh && BIN="$CARGO_TARGET_DIR/debug/relevance_matrix" && test -x "$BIN" && "$BIN" --out-dir "$(pwd)/artifacts/search-quality/relevance/hash-dev/latest"'

# Run the pinned external-daemon A/B producer with approved inputs.
rust-capture-quality-relevance-openai-ab $runner $runner_sha $source_checkout $source_head $out:
    {{retrieval_python}} tools/benchmark/benchctl.py capture-frozen-openai-ab --runner "$runner" --runner-sha256 "$runner_sha" --source-checkout "$source_checkout" --source-head "$source_head" --out "$out"

# Ambiguity / repairability rail (J7Q-06). Blocking dimension: ambiguity.
# Proves: typed repair-payload invariants — repairable bridge codes carry
# non-empty supported alternatives + a docs anchor; the internal invariant-break
# code carries none; classes stay distinct families; payloads round-trip the
# wire codec. No silent rewrite: repair is advisory metadata on a failing code.
# Artifacts: artifacts/search-quality/ambiguity/latest/{summary,error_payloads}.json
rust-verify-quality-ambiguity:
    {{cargo}} --lane test-daemon-lane test -p quanta-index-searchd-harness --lib ambiguity:: --no-default-features --locked -- --nocapture
    mkdir -p artifacts/search-quality/ambiguity/latest
    {{cargo}} --lane test-daemon-lane build -p quanta-index-searchd-harness --bin ambiguity_matrix --no-default-features --locked
    env QUANTA_INDEX_BUILD_LANE=test-daemon-lane bash -lc 'source scripts/quanta-index-env.sh && BIN="$CARGO_TARGET_DIR/debug/ambiguity_matrix" && test -x "$BIN" && "$BIN" --out-dir "$(pwd)/artifacts/search-quality/ambiguity/latest"'

# Snippet quality rail (J7Q-02). Blocking dimension: snippet.
# Proves: pure window-oracle unit tests + adversarial gate tests (the gate can go
# RED), then seeds the phrase/regex/multi-hit/long-line fixture and grades the
# engine's emitted snippet against hit-centered-window + bounded-length +
# deterministic-truncation (NOT substring presence). The snippet is graded
# as-emitted; the rail never post-processes engine output to force a pass.
# Artifacts: artifacts/search-quality/snippet/latest/{summary,golden_windows}.json
rust-verify-quality-snippet:
    python3 -c 'import fcntl, os, pathlib; p = pathlib.Path("artifacts/search-quality/snippet/latest"); p.mkdir(parents=True, exist_ok=True); fd = os.open(p, os.O_RDONLY); fcntl.flock(fd, fcntl.LOCK_EX); (p / "summary.json").unlink(missing_ok=True); (p / "golden_windows.json").unlink(missing_ok=True); os.close(fd)'
    {{cargo}} --lane test-daemon-lane test -p quanta-index-searchd-harness --lib snippet:: --no-default-features --locked -- --nocapture
    {{cargo}} --lane test-daemon-lane build -p quanta-index-searchd-harness --bin snippet_matrix -p quanta-index-searchd-runtime --bin quanta-index-searchd --no-default-features --locked
    env QUANTA_INDEX_BUILD_LANE=test-daemon-lane bash -lc 'source scripts/quanta-index-env.sh && BIN="$CARGO_TARGET_DIR/debug/snippet_matrix" && NATIVE_DAEMON_BIN="$CARGO_TARGET_DIR/debug/quanta-index-searchd" && test -x "$BIN" && test -x "$NATIVE_DAEMON_BIN" && "$BIN" --native-daemon-bin "$NATIVE_DAEMON_BIN" --out-dir "$(pwd)/artifacts/search-quality/snippet/latest"'

# Scale-tier rail (J7Q-03). Blocking dimension: scale.
# Proves: deterministic seeded-corpus generator + tier-manifest invariants, then
# measures the SMALL tier end to end (ingest -> seal -> activate -> query),
# capturing ingest/open/query wall-times. Medium/large/xlarge are emitted as
# `declared-advisory`: their blocking latency is owned by the canonical Linux
# perf runner, not this host. An empty/typed-error small-tier query is a non-zero
# exit, never a fabricated zero-latency pass.
# Artifacts: a fresh external /tmp root printed by the producer.
rust-verify-quality-scale:
    python3 tools/ci/timing/check_host_contention.py
    {{cargo}} --lane test-daemon-lane test -p quanta-index-searchd-harness --lib native_index::quality:: --no-default-features --locked -- --nocapture
    {{cargo}} --lane test-daemon-lane build -p quanta-index-searchd-harness --bin scale_matrix -p quanta-index-searchd-runtime --bin quanta-index-searchd --no-default-features --locked
    env QUANTA_INDEX_BUILD_LANE=test-daemon-lane bash -lc 'source scripts/quanta-index-env.sh && SCALE_BIN="$CARGO_TARGET_DIR/debug/scale_matrix" && NATIVE_DAEMON_BIN="$CARGO_TARGET_DIR/debug/quanta-index-searchd" && test -x "$SCALE_BIN" && test -x "$NATIVE_DAEMON_BIN" && SCALE_RUN_ROOT=$(mktemp -d /tmp/quanta-index-scale.XXXXXX) && "$SCALE_BIN" --backend index-native --native-daemon-bin "$NATIVE_DAEMON_BIN" --native-quality --out-dir "$SCALE_RUN_ROOT/result" && mkdir -p artifacts/search-quality/scale/latest && cp "$SCALE_RUN_ROOT/result/tier_manifest.json" artifacts/search-quality/scale/latest/tier_manifest.json && cp "$SCALE_RUN_ROOT/result/summary.json" artifacts/search-quality/scale/latest/summary.json'

# Latency-tail rail (J7Q-04). Blocking dimension: tail (correctness-gated).
# Proves: route-aware p50/p95/p99 budget manifest + percentile invariants, then
# boots one warm runtime and times each budgeted route's representative query
# TAIL_SAMPLES times. Correctness (golden-validated shape/count/error before
# timing) is the blocking signal; per-route latency budgets are explicit but
# advisory on this host this increment (canonical blocking is the Linux perf
# runner). No single global threshold; verdicts carry route-local diagnostics.
# Requires QUANTA_INDEX_LEGACY_SEARCHD_BIN and QUANTA_INDEX_LEGACY_SEARCHD_SHA256.
# Artifacts: artifacts/search-quality/tail/latest/{summary,route_budgets}.json
rust-verify-quality-tail: require-native-legacy-searchd
    python3 tools/ci/timing/check_host_contention.py
    {{cargo}} --lane test-daemon-lane test -p quanta-index-searchd-harness --lib tail:: --no-default-features --locked -- --nocapture
    mkdir -p artifacts/search-quality/tail/latest
    {{cargo}} --lane test-daemon-lane build -p quanta-index-searchd-harness --bin tail_matrix --no-default-features --locked
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
    {{cargo}} --lane test-daemon-lane test -p quanta-index-searchd-harness --lib ann:: --no-default-features --features semantic-bench --locked -- --nocapture
    mkdir -p artifacts/search-quality/ann/latest
    {{cargo}} --lane test-daemon-lane build -p quanta-index-searchd-harness --bin ann_matrix --no-default-features --features semantic-bench --locked
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
# Requires QUANTA_INDEX_LEGACY_SEARCHD_BIN and QUANTA_INDEX_LEGACY_SEARCHD_SHA256.
# Artifacts: artifacts/search-quality/concurrency/latest/summary-c{1,8,32}.json
rust-verify-quality-concurrency requests="80": require-native-legacy-searchd
    python3 tools/ci/timing/check_host_contention.py
    {{cargo}} --lane test-daemon-lane test -p quanta-index-searchd-harness --lib concurrency:: --no-default-features --locked -- --nocapture
    mkdir -p artifacts/search-quality/concurrency/latest
    {{cargo}} --lane test-daemon-lane build -p quanta-index-searchd-harness --bin concurrency_matrix --no-default-features --locked
    env QUANTA_INDEX_BUILD_LANE=test-daemon-lane bash -lc 'source scripts/quanta-index-env.sh && BIN="$CARGO_TARGET_DIR/debug/concurrency_matrix" && test -x "$BIN" && "$BIN" --out-dir "$(pwd)/artifacts/search-quality/concurrency/latest" --requests-per-client {{requests}}'

# Explicit workspace file-mutation to generation-pinned lexical visibility.
# This measures the ingest receipt through seal/activation/query path, not a watcher.
# Requires QUANTA_INDEX_LEGACY_SEARCHD_BIN and QUANTA_INDEX_LEGACY_SEARCHD_SHA256.
rust-verify-quality-freshness samples="20": require-native-legacy-searchd
    python3 tools/ci/timing/check_host_contention.py
    mkdir -p artifacts/search-quality/freshness/latest
    {{cargo}} --lane test-daemon-lane build -p quanta-index-searchd-harness --bin freshness_matrix --no-default-features --locked
    env QUANTA_INDEX_BUILD_LANE=test-daemon-lane bash -lc 'source scripts/quanta-index-env.sh && BIN="$CARGO_TARGET_DIR/debug/freshness_matrix" && test -x "$BIN" && "$BIN" --out-dir "$(pwd)/artifacts/search-quality/freshness/latest" --samples {{samples}}'

# Scheduled arrivals over pinned old-Q query IPC; record offered/achieved QPS and tails.
# Requires QUANTA_INDEX_LEGACY_SEARCHD_BIN and QUANTA_INDEX_LEGACY_SEARCHD_SHA256.
rust-verify-quality-open-loop: require-native-legacy-searchd
    python3 tools/ci/timing/check_host_contention.py
    {{cargo}} --lane test-daemon-lane build -p quanta-index-searchd-harness --bin open_loop_matrix --no-default-features --locked
    env QUANTA_INDEX_BUILD_LANE=test-daemon-lane bash -lc 'source scripts/quanta-index-env.sh && OPEN_LOOP_BIN="$CARGO_TARGET_DIR/debug/open_loop_matrix" && test -x "$OPEN_LOOP_BIN" && "$OPEN_LOOP_BIN"'

# Pinned old-Q operator diagnosis (J7Q-05). Reports the actual candidate, active
# head, typed query refusal, latency metrics, and daemon peak resident bytes.
# Requires QUANTA_INDEX_LEGACY_SEARCHD_BIN and QUANTA_INDEX_LEGACY_SEARCHD_SHA256.
# Artifacts: artifacts/search-quality/ops/latest/{summary,cli_snapshots}.json
rust-verify-quality-ops: require-native-legacy-searchd
    {{cargo}} --lane test-daemon-lane test -p quanta-index-searchd-harness --lib ops:: --no-default-features --locked -- --nocapture
    mkdir -p artifacts/search-quality/ops/latest
    {{cargo}} --lane test-daemon-lane build -p quanta-index-searchd-harness --bin ops_matrix --no-default-features --locked
    env QUANTA_INDEX_BUILD_LANE=test-daemon-lane bash -lc 'source scripts/quanta-index-env.sh && BIN="$CARGO_TARGET_DIR/debug/ops_matrix" && test -x "$BIN" && "$BIN" --out-dir "$(pwd)/artifacts/search-quality/ops/latest"'

# Pinned old-Q UI contract diagnostics (J7Q-07).
# Proves: the typed `LexicalCandidate::snippet_hit_offset` highlight anchor is
# present and points exactly at the matched needle on each probe (short + long
# line), so a downstream UI renders highlights without regex-parsing raw snippets.
# Proven across engine + contract layers (the field round-trips on the wire in
# the contract IPC test). A missing/out-of-range/wrong anchor fails the rail.
# Artifacts: artifacts/search-quality/ui/latest/{summary,contract_snapshots}.json
rust-verify-quality-ui: require-native-legacy-searchd
    {{cargo}} --lane test-daemon-lane test -p quanta-index-searchd-harness --lib ui:: --no-default-features --locked -- --nocapture
    mkdir -p artifacts/search-quality/ui/latest
    {{cargo}} --lane test-daemon-lane build -p quanta-index-searchd-harness --bin ui_matrix --no-default-features --locked
    env QUANTA_INDEX_BUILD_LANE=test-daemon-lane bash -lc 'source scripts/quanta-index-env.sh && BIN="$CARGO_TARGET_DIR/debug/ui_matrix" && test -x "$BIN" && "$BIN" --out-dir "$(pwd)/artifacts/search-quality/ui/latest"'

# Aggregate quality gate (J7Q-08). Orchestrates the LIVE per-dimension rails
# (relevance, ambiguity, snippet, scale, tail, ANN, concurrency, freshness,
# open-loop, ops, ui) and records an
# integration summary WITHOUT erasing dimension boundaries. This is not a
# substitute for per-dimension closeout. A new dimension must have a producer,
# manifest registration, and ticket metadata before joining the aggregate.
rust-verify-quality-all: require-native-legacy-searchd
    uv run --frozen --extra dev python tools/benchmark/benchctl.py run quality-full
    mkdir -p artifacts/search-quality/integration/latest
    uv run --frozen --extra dev python tools/benchmark/quality_integration_summary.py --out artifacts/search-quality/integration/latest/summary.json

rust-machete:
    env QUANTA_INDEX_BUILD_LANE=machete-lane bash -lc 'source scripts/quanta-index-env.sh && cargo machete --with-metadata'

rust-miri:
    env QUANTA_INDEX_BUILD_LANE=miri-lane bash -lc 'source scripts/quanta-index-env.sh && RUSTUP_TOOLCHAIN=nightly cargo miri setup'
    env QUANTA_INDEX_BUILD_LANE=miri-lane bash -lc 'source scripts/quanta-index-env.sh && RUSTUP_TOOLCHAIN=nightly MIRIFLAGS="-Zmiri-strict-provenance" cargo miri test -p quanta-index-contract -p quanta-index-core --lib --all-features'

rust-careful: require-native-legacy-searchd
    env QUANTA_INDEX_BUILD_LANE=careful-lane bash -lc 'source scripts/quanta-index-env.sh && RUSTUP_TOOLCHAIN=nightly CARGO_INCREMENTAL=0 cargo careful test --workspace --all-features --locked'

rust-tsan: require-native-legacy-searchd
    env QUANTA_INDEX_BUILD_LANE=tsan-lane bash -lc 'source scripts/quanta-index-env.sh && RUSTUP_TOOLCHAIN=nightly RUSTFLAGS="-Zsanitizer=thread" cargo test -Z build-std --target $(rustc -vV | sed -n '"'"'s|host: ||p'"'"') --workspace --all-features --lib --tests'

rust-asan: require-native-legacy-searchd
    env QUANTA_INDEX_BUILD_LANE=asan-lane bash -lc 'source scripts/quanta-index-env.sh && RUSTUP_TOOLCHAIN=nightly RUSTFLAGS="-Zsanitizer=address" cargo test -Z build-std --target $(rustc -vV | sed -n '"'"'s|host: ||p'"'"') --workspace --all-features --lib --tests'

rust-mutants:
    env QUANTA_INDEX_BUILD_LANE=mutants-lane bash -lc 'source scripts/quanta-index-env.sh && cargo mutants --package quanta-index-core --timeout 60 --baseline=skip --no-shuffle'

rust-udeps:
    env QUANTA_INDEX_BUILD_LANE=udeps-lane bash -lc 'source scripts/quanta-index-env.sh && RUSTUP_TOOLCHAIN=nightly CARGO_INCREMENTAL=0 cargo udeps --workspace --all-targets --all-features'

rust-deny:
    bash scripts/run-cargo-deny.sh

# Audit separate tool, consumer, and fuzz lock graphs independently of the root gate.
rust-deny-workspaces:
    bash scripts/run-cargo-deny.sh --all-workspaces

rust-workspace-lints:
    uv run --frozen --extra dev python scripts/check_workspace_lints.py

rust-no-allow:
    uv run --frozen --extra dev bash scripts/check-rust-allow-attributes.sh

rust-hexagonal:
    uv run --frozen --extra dev python tools/ci/lint/lint-hexagonal-boundaries.py

rust-read-view-ambient:
    uv run --frozen --extra dev python tools/ci/lint/check-read-view-ambient-lookup.py

rust-derive-allowlist:
    uv run --frozen --extra dev python tools/ci/lint/check-rust-derive-allowlist.py

rust-cargo-toml-hygiene:
    uv run --frozen --extra dev python tools/ci/lint/check-cargo-toml-hygiene.py

rust-module-discipline:
    uv run --frozen --extra dev python tools/ci/lint/check-module-discipline.py

rust-module-cycles:
    uv run --frozen --extra dev python tools/ci/lint/check-module-cycles.py

rust-error-shape:
    uv run --frozen --extra dev python tools/ci/lint/check-error-shape.py

rust-digest-fallibility:
    uv run --frozen --extra dev python tools/ci/lint/check-digest-fallibility.py

rust-semantic-outcomes:
    uv run --frozen --extra dev python tools/ci/lint/check-semantic-outcomes.py

rust-fallbacks:
    uv run --frozen --extra dev python tools/ci/tests/test_check_rust_fallbacks.py
    uv run --frozen --extra dev python tools/ci/lint/check-rust-fallbacks.py

rust-test-authority:
    uv run --frozen --extra dev python tools/ci/lint/check-lexical-test-disposition.py --inventory-only
    uv run --frozen --extra dev python tools/ci/lint/check-runtime-legacy-migration.py
    uv run --frozen --extra dev python tools/ci/lint/check-test-authority.py

rust-ignored-test-policy:
    uv run --frozen --extra dev python tools/ci/lint/check-ignored-test-policy.py

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
    uv run --frozen --extra dev python tools/ci/lint/check-wire-inventory.py

rust-error-code-table:
    uv run --frozen --extra dev python tools/ci/check-search-plane-error-codes.py

# Stale-artifact gate (QI-BB-010, findings §9): every benchmark/relevance
# artifact on disk must be a schema-2 BenchArtifactV1 whose git_head is the
# checkout's HEAD (baselines: a full head). Absence passes; staleness fails.
rust-bench-artifacts:
    uv run --frozen --extra dev python tools/ci/lint/check-bench-artifacts.py

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
    env QUANTA_INDEX_BUILD_LANE=fuzz-lq-norm-lane bash -lc 'source scripts/quanta-index-env.sh && cd crates/quanta-index-lq-norm/fuzz && cargo +nightly fuzz run regex_necessary_gram -- -max_total_time={{seconds}}'

# Checks committed production Rust lines against fresh full-workspace LCOV.
rust-coverage-changed base minimum_percent="90":
    #!/usr/bin/env bash
    set -euo pipefail
    candidate=$(git rev-parse --verify 'HEAD^{commit}')
    require_candidate_source() {
        local head status hidden
        head=$(git rev-parse --verify 'HEAD^{commit}')
        status=$(git --no-optional-locks -c core.fsmonitor=false status \
            --porcelain=v1 --untracked-files=all --ignore-submodules=none)
        hidden=$(git ls-files -v | LC_ALL=C sed -n '/^[Sa-z] /p')
        if [[ "$head" != "$candidate" || -n "$status" || -n "$hidden" ]]; then
            printf 'coverage requires unchanged committed source at %s\n' "$candidate" >&2
            exit 2
        fi
    }
    require_candidate_source
    report=$(mktemp "${TMPDIR:-/tmp}/quanta-index-coverage.XXXXXXXX")
    trap 'rm -f -- "$report"' EXIT
    {{quote(cargo)}} --lane coverage-lane llvm-cov nextest --workspace --all-features --locked --lcov --output-path "$report"
    require_candidate_source
    uv run --frozen --extra dev python tools/ci/lint/check-changed-line-coverage.py \
        --lcov "$report" --base {{quote(base)}} --minimum-percent {{quote(minimum_percent)}}
    require_candidate_source

rust-policy:
    @just rust-workspace-lints
    @just rust-hexagonal
    @just rust-read-view-ambient
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
    @just rust-error-code-table
    @just proof-authority-lint
    @just rust-bench-artifacts
    @just rust-deny

# SEP-21 proof policy separates static lint, the P00 current gate, pre-deploy
# code qualification, and final production qualification. Static lint never
# treats absent future proof artifacts as success or failure. Code qualification
# requires the fixed CODE_QUALIFIED proof closure; final qualification also
# requires deployment, activation, rollback, P12, and the aggregate receipt.
proof-authority-lint:
    uv run --frozen --extra dev python tools/ci/lint/check-proof-authority.py

proof-authority-current-gate:
    uv run --frozen --extra dev python tools/ci/lint/check-proof-authority.py \
        --manifest artifacts/proof-authority/p00-authority-freeze.json \
        --bind-source

lane-handoff-check handoff:
    uv run --frozen --extra dev python tools/ci/lint/check-lane-handoff.py --require-result-head {{quote(handoff)}}

lane-handoff-check-historical handoff:
    uv run --frozen --extra dev python tools/ci/lint/check-lane-handoff.py {{quote(handoff)}}

lane-handoff-chain-check:
    uv run --frozen --extra dev python tools/ci/lint/check-lane-handoff.py --product-chain artifacts/sep-21/handoffs

proof-error-authority-inventory:
    uv run --frozen --extra dev python tools/ci/write-error-authority-inventory.py

proof-error-authority-closed:
    uv run --frozen --extra dev python tools/ci/check-error-authority-closure.py

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

# P02B: generic journal tests plus exact Native Catalog replay, conflict and
# rollback cases. Both scopes must issue source-bound raw results for one
# manifest; wire, IPC fuzz and boundary rails cover the shared receipt surface.
proof-p02b-operation-journal:
    @just rust-profile test-p02b-operation-journal
    @just rust-profile test-p02b-native-journal-lib
    @just rust-hexagonal
    @just rust-wire-inventory
    @just rust-fuzz-smoke

proof-p00-authority-freeze:
    @test -z "${PYTEST_ADDOPTS:-}" && test -z "${PYTEST_PLUGINS:-}" || { echo "pytest environment overrides are forbidden for proof tests" >&2; exit 2; }
    @just proof-error-authority-inventory
    uv run --frozen --extra dev python tools/ci/lint/check-proof-authority.py
    @if [ -n "${QUANTA_PROOF_RAW_DIR:-}" ]; then mkdir -p "$QUANTA_PROOF_RAW_DIR"; uv run --frozen --extra dev python tools/ci/proof_execution_result.py collect-pytest --output "$QUANTA_PROOF_RAW_DIR/p00-inventory.json" tools/ci/tests/test_write_error_authority_inventory.py tools/ci/tests/test_write_proof_aggregate.py tools/ci/tests/test_write_proof_manifest.py tools/ci/tests/test_proof_execution_result.py tools/ci/tests/test_proof_operational_result.py tools/ci/tests/test_paired_cargo_resolution.py tools/ci/tests/test_paired_r5_result.py tools/ci/tests/test_run_local_test_scope.py tools/ci/tests/test_check_proof_authority.py tools/ci/tests/test_check_lane_handoff.py tools/ci/tests/test_handoff_validation.py; fi
    uv run --frozen --extra dev python -m pytest \
        tools/ci/tests/test_write_error_authority_inventory.py \
        tools/ci/tests/test_write_proof_aggregate.py \
        tools/ci/tests/test_write_proof_manifest.py \
        tools/ci/tests/test_proof_execution_result.py \
        tools/ci/tests/test_proof_operational_result.py \
        tools/ci/tests/test_paired_cargo_resolution.py \
        tools/ci/tests/test_paired_r5_result.py \
        tools/ci/tests/test_run_local_test_scope.py \
        tools/ci/tests/test_check_proof_authority.py \
        tools/ci/tests/test_check_lane_handoff.py \
        tools/ci/tests/test_handoff_validation.py -q

# P12A is a Python owner proof. Its exact-source manifest can be issued
# independently; P12 final qualification binds P11 and the paired checkout.
proof-p12a-proof-infrastructure:
    @test -z "${PYTEST_ADDOPTS:-}" && test -z "${PYTEST_PLUGINS:-}" || { echo "pytest environment overrides are forbidden for proof tests" >&2; exit 2; }
    uv run --frozen --extra dev python tools/ci/lint/check-test-authority.py
    uv run --frozen --extra dev python tools/ci/lint/check-proof-authority.py
    uv run --frozen --extra dev python tools/ci/proof_execution_result.py run-p12a \
        tools/ci/tests/test_write_proof_manifest.py \
        tools/ci/tests/test_proof_execution_result.py \
        tools/ci/tests/test_proof_operational_result.py \
        tools/ci/tests/test_paired_cargo_resolution.py \
        tools/ci/tests/test_paired_r5_result.py \
        tools/ci/tests/test_run_local_test_scope.py \
        tools/ci/tests/test_write_proof_aggregate.py \
        tools/ci/tests/test_check_proof_authority.py \
        tools/ci/tests/test_check_lane_handoff.py \
        tools/ci/tests/test_handoff_validation.py

# Operational entries retain staged refusal until a concrete Linux contract is registered.
proof-p11-deployment:
    uv run --frozen --extra dev python tools/ci/operational_proof.py p11-deployment

proof-p11-activation:
    uv run --frozen --extra dev python tools/ci/operational_proof.py p11-activation

proof-p11-rollback:
    uv run --frozen --extra dev python tools/ci/operational_proof.py p11-rollback

# The aggregate is the final release receipt. The release gate revalidates
# every input against the current source pair.
proof-authority-final-qualification:
    @test -n "${SEMANTICA_CHECKOUT:-}" || { echo "SEMANTICA_CHECKOUT is required" >&2; exit 2; }
    @just rust-check-tantivy-shipment --publish-aggregate
    uv run --frozen --extra dev python tools/ci/lint/check-proof-authority.py \
        --require-all \
        --paired-checkout "github:josongmin/semantica-codegraph-v2=${SEMANTICA_CHECKOUT}" \
        --bind-source

proof-authority-code-gate:
    @test -n "${SEMANTICA_CHECKOUT:-}" || { echo "SEMANTICA_CHECKOUT is required" >&2; exit 2; }
    uv run --frozen --extra dev python tools/ci/lint/check-proof-authority.py \
        --require-code-qualified \
        --paired-checkout "github:josongmin/semantica-codegraph-v2=${SEMANTICA_CHECKOUT}" \
        --bind-source

proof-authority-release-gate:
    @test -n "${SEMANTICA_CHECKOUT:-}" || { echo "SEMANTICA_CHECKOUT is required" >&2; exit 2; }
    @just rust-check-tantivy-shipment --require-aggregate

verify-rust: require-native-legacy-searchd
    @just fmt-check
    @just rust-clippy
    @just rust-policy
    @just rust-semantic-outcomes
    @just rust-fallbacks
    @just rust-machete
    @just benchmark-policy-local
    @just rust-bench-build
    @just rust-test
    @just rust-test-native-frontdoor
    @just rust-doc

# Heavy correctness rail. Requires nightly toolchain + cargo-careful/miri/etc.
# Use `just verify-rust-heavy` locally before merging anything load-bearing.
verify-rust-heavy: require-native-legacy-searchd
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
    source scripts/quanta-index-env.sh && uv run --frozen --extra dev python -m ruff format --check --output-format concise .

python-test:
    bash scripts/run-tooling-tests.sh

agent-output-validate $output:
    source scripts/quanta-index-env.sh && uv run --frozen --extra dev python tools/ci/agent/validate_agent_output.py -- "$output"

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
    @just python-lint
    @just python-format-check
    @just python-test
    @just lint-prompt-drift

sync-prompts:
    uv run --frozen --extra dev python tools/prompt-manager/pm.py sync

lint-prompt-drift:
    uv run --frozen --extra dev python tools/prompt-manager/pm.py lint

test-prompt-manager:
    source scripts/quanta-index-env.sh && uv run --frozen --extra dev python -m pytest tools/prompt-manager/tests/test_pm.py -q -o cache_dir="$PYTEST_CACHE_DIR"

verify-prompts:
    @just sync-prompts
    @just lint-prompt-drift
    @just test-prompt-manager

precommit-run:
    source scripts/quanta-index-env.sh && uv run --frozen --extra dev python -m pre_commit run --all-files --show-diff-on-failure

proof-p03-candidate-activation:
    @just rust-profile test-candidate-activation-release

proof-p03-candidate-activation-owner:
    @just rust-profile test-candidate-activation-owner
    @just rust-hexagonal
    @just rust-wire-inventory

rust-test-candidate-activation-release:
    uv run --frozen --extra dev python tools/ci/run-local-test-scope.py candidate-activation-release

proof-p04-read-view-lifetime:
    @just rust-profile test-read-view-lifetime

proof-p04-read-view-lifetime-owner:
    @just rust-profile test-read-view-lifetime-owner
    @just rust-read-view-ambient
    @just rust-hexagonal

proof-p10-state-migration:
    @just rust-profile test-state-migration-release

rust-test-read-view-lifetime:
    uv run --frozen --extra dev python tools/ci/run-local-test-scope.py read-view-lifetime

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

proof-p06-sdk-binding:
    @just rust-profile test-sdk-binding-release

rust-test-sdk-binding-release:
    uv run --frozen --extra dev python tools/ci/run-local-test-scope.py sdk-binding-release

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

proof-p07-provider-boundary:
    @just rust-profile test-provider-boundary-release

rust-test-provider-boundary-release:
    uv run --frozen --extra dev python tools/ci/run-local-test-scope.py provider-boundary-release

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

proof-p08-runtime-supervisor:
    @just rust-profile test-runtime-supervisor-release

rust-test-runtime-supervisor-release:
    uv run --frozen --extra dev python tools/ci/run-local-test-scope.py runtime-supervisor-release

proof-p05-query-truth-owner:
    @just rust-profile test-query-truth-owner
    @just rust-profile test-query-truth-owner-lib
    @just rust-hexagonal
    @just rust-wire-inventory
    @just rust-public-api
    @just rust-fuzz-smoke

proof-p05-query-truth:
    @just rust-profile test-query-truth-release

rust-test-query-truth-release:
    uv run --frozen --extra dev python tools/ci/run-local-test-scope.py query-truth-release

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

proof-p09-control-readiness:
    @just rust-profile test-control-readiness-release

rust-test-control-readiness-release:
    uv run --frozen --extra dev python tools/ci/run-local-test-scope.py control-readiness-release

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
