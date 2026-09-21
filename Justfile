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
    bash -lc 'test -n "$QUANTA_INDEX_SEARCHD_BIN" || { echo "QUANTA_INDEX_SEARCHD_BIN is required"; exit 1; }'
    env QUANTA_INDEX_SEARCHD_BIN="${QUANTA_INDEX_SEARCHD_BIN}" cargo test --manifest-path {{semantica_root}}/packages/analysis/quanta-v2/Cargo.toml -p quanta-runtime --no-default-features --features index-sdk-ingress --test index_sdk_ingress_publish_contract_test index_sdk_ingress_live_file_contributor_publish_and_query_roundtrip_v1 -- --nocapture

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
    env QUANTA_INDEX_BUILD_LANE=msrv-lane bash -lc 'source scripts/quanta-index-env.sh && cargo +1.92.0 check --workspace --all-targets --all-features --locked'
    env QUANTA_INDEX_BUILD_LANE=msrv-lane bash -lc 'source scripts/quanta-index-env.sh && cargo +1.92.0 test --workspace --all-features --locked --no-run'

rust-bench:
    {{cargo}} --lane bench-lane bench --workspace --all-features --locked

rust-bench-build:
    {{cargo}} --lane bench-lane bench --workspace --all-features --locked --no-run

# Layer-3 DSL query-latency matrix (docs/plans/jun-2-dsl-hardening/RFC-DSL-Benchmarking.md).
# Warm: in-process criterion + p50/p95/p99 artifact. Cold: fresh-process-per-sample runner.
rust-bench-dsl-warm:
    mkdir -p artifacts/dsl-bench
    env CARGO_NET_OFFLINE=true {{cargo}} --lane bench-lane build -p quanta-index-searchd-harness --bin dsl_warm_matrix --profile bench --quiet --locked
    env QUANTA_INDEX_BUILD_LANE=bench-lane QUANTA_INDEX_BENCH_DISABLE_QUERY_OBS=1 bash -lc 'source scripts/quanta-index-env.sh && BIN="$CARGO_TARGET_DIR/release/dsl_warm_matrix" && test -x "$BIN" && "$BIN" --out "$(pwd)/artifacts/dsl-bench/warm-matrix.json"'

# Exploratory criterion view for warm scenarios. Not gate authority.
rust-bench-dsl-warm-criterion:
    mkdir -p artifacts/dsl-bench
    env DSL_BENCH_WARM_OUT="$(pwd)/artifacts/dsl-bench/warm-matrix.criterion.json" \
      {{cargo}} --lane bench-lane bench -p quanta-index-searchd-runtime --bench dsl_query_matrix --all-features --locked

rust-bench-dsl-cold samples="20":
    mkdir -p artifacts/dsl-bench
    env QUANTA_INDEX_BENCH_DISABLE_QUERY_OBS=1 python3 tools/benchmark/run_dsl_cold_matrix.py --samples {{samples}} --out artifacts/dsl-bench/cold-matrix.json

# Authority refresh: run warm and cold producers serially, then gate against baselines.
rust-bench-dsl-refresh samples="20":
    @just rust-bench-dsl-warm
    @just rust-bench-dsl-cold {{samples}}
    @just rust-bench-dsl-compare

# Phase B relative-regression gate (report-only until baselines are captured via --update-baseline).
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
    env QUANTA_INDEX_BUILD_LANE=test-daemon-lane QUANTA_QUALITY_CAPTURE_DATE="$(date -u +%Y-%m-%d)" bash -lc 'source scripts/quanta-index-env.sh && BIN="$CARGO_TARGET_DIR/debug/relevance_matrix" && test -x "$BIN" && "$BIN" --out-dir "$(pwd)/artifacts/search-quality/relevance/latest"'

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
# lexical/semantic/hybrid/count route set with a slow page-maximum client mixed
# in above one client, writing one provenanced BenchArtifactV1 per client count
# (p50/p95/p99, QPS, error/timeout counts, head-of-line ratio). Small on this
# host by default (`requests` per client); the Linux perf runner raises it.
# Artifacts: artifacts/search-quality/concurrency/latest/summary-c{1,8,32}.json
rust-verify-quality-concurrency requests="16":
    python3 tools/ci/timing/check_host_contention.py
    {{cargo}} --lane test-daemon-lane test -p quanta-index-searchd-harness --lib concurrency:: --all-features --locked -- --nocapture
    mkdir -p artifacts/search-quality/concurrency/latest
    {{cargo}} --lane test-daemon-lane build -p quanta-index-searchd-harness --bin concurrency_matrix --all-features --locked
    env QUANTA_INDEX_BUILD_LANE=test-daemon-lane bash -lc 'source scripts/quanta-index-env.sh && BIN="$CARGO_TARGET_DIR/debug/concurrency_matrix" && test -x "$BIN" && "$BIN" --out-dir "$(pwd)/artifacts/search-quality/concurrency/latest" --requests-per-client {{requests}}'

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
# (relevance, ambiguity, snippet, scale, tail, ops, ui) and records an
# integration summary WITHOUT erasing dimension boundaries. This is not a
# substitute for per-dimension closeout. All seven J7Q quality dimensions are now
# live; a future dimension would be added as `pending` until its rail lands.
rust-verify-quality-all:
    @just rust-verify-quality-relevance
    @just rust-verify-quality-ambiguity
    @just rust-verify-quality-snippet
    @just rust-verify-quality-scale
    @just rust-verify-quality-tail
    @just rust-verify-quality-ops
    @just rust-verify-quality-ui
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

rust-test-authority:
    python3 tools/ci/lint/check-test-authority.py

rust-ignored-test-policy:
    python3 tools/ci/lint/check-ignored-test-policy.py

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
    env QUANTA_INDEX_BUILD_LANE=coverage-lane bash -lc 'source scripts/quanta-index-env.sh && cargo llvm-cov nextest --workspace --all-features --locked --lcov --output-path /tmp/quanta-index-coverage.lcov'
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

# SEP-21 proof policy separates static lint, the P00 current gate, the P12
# dependency aggregate producer, and the final release gate. Static lint never
# treats absent future proof artifacts as success or failure. Only the final
# release gate requires every registered receipt, including P12.
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
    python3 tools/ci/write-error-authority-inventory.py
    python3 tools/ci/lint/check-proof-authority.py
    python3 -m pytest \
        tools/ci/tests/test_write_error_authority_inventory.py \
        tools/ci/tests/test_write_proof_aggregate.py \
        tools/ci/tests/test_write_proof_manifest.py \
        tools/ci/tests/test_check_proof_authority.py \
        tools/ci/tests/test_check_lane_handoff.py \
        -q

# P12 records this dependency aggregate as its own terminal evidence. It must
# exclude p12-final-qualification itself; the release gate below validates the
# resulting P12 receipt together with every dependency and therefore is not
# self-validating.
proof-authority-final-qualification:
    @test -n "${SEMANTICA_CHECKOUT:-}" || { echo "SEMANTICA_CHECKOUT is required" >&2; exit 2; }
    @test -n "${P12_TERMINAL_INPUT:-}" || { echo "P12_TERMINAL_INPUT is required" >&2; exit 2; }
    python3 tools/ci/write-proof-aggregate.py \
        --paired-checkout "${SEMANTICA_CHECKOUT}"
    python3 tools/ci/write-proof-manifest.py \
        --proof-id p12-final-qualification \
        --terminal-input "${P12_TERMINAL_INPUT}" \
        --paired-checkout "${SEMANTICA_CHECKOUT}"
    python3 tools/ci/lint/check-proof-authority.py \
        --require-all \
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

# Dedicated P04 read-view recipe: owner-local selector first, then the
# Linux release subrail split the prompt names (NOT_RUN off-Linux).
rust-proof-p04-read-view: proof-p04-read-view-lifetime-owner
