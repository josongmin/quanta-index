# Jun-2 DSL Benchmarking RFC

> Contract status: `Active detailed contract`. Architecture and claim boundaries are owned by
> [JUN-08-001](../../adr/JUN-08-001-verification-hellgate-and-benchmark-separation.md).

Status: `adopted`
Date: `2026-06-02`
Owner packet: [README.md](README.md)

## Goal

이 RFC의 목적은 현재 shipped DSL surface의 성능 측정을
`compile timing`, `LQ pipeline hot path`, `query latency`
세 계층으로 분리하고, 각 계층마다 비교 대상과 gate를 고정하는 것이다.

핵심 원칙:

1. correctness rail과 benchmark rail을 섞지 않는다
2. cold/warm, native/sourcegraph, lexical/history/runtime/structural을 한 숫자로 합치지 않는다
3. `e2e_perf_chaos`는 성능 벤치가 아니라 boundedness / fail-closed rail로 유지한다
4. baseline 없는 절대 수치 과장을 금지한다

## Current Source Truth

현재 레포에는 이미 다음 측정/증명 자산이 있다.

1. compile timing rail
   - `just rust-timings-fast-check`
   - `just rust-timings-daemon-check`
   - baseline JSON:
     - `tools/ci/timing/baselines/fast-lane.json`
     - `tools/ci/timing/baselines/daemon-lane.json`
2. criterion pipeline bench
   - `crates/quanta-index-lq-norm/benches/pipeline.rs`
   - stage:
     - `tokenize`
     - `parse`
     - `normalize`
     - `hash`
   - input sizes:
     - `1 KiB`
     - `4 KiB`
     - `16 KiB`
3. runtime execution proof rail
   - `e2e_full_corpus`
   - `dsl_scenarios`
   - `sdk_frontdoor`
   - `end_to_end`
   - `e2e_restart_replay_determinism`
   - `e2e_perf_chaos`

현재 부족한 것은 아니다. adopted path의 remaining work는 artifact refresh와
baseline comparison discipline 유지다.

## Non-Goals

이 RFC는 아래를 하지 않는다.

1. `e2e_perf_chaos` 시간을 성능 숫자로 재사용하지 않는다
2. runtime catalog query를 `ripgrep` 같은 text-only engine과 비교하지 않는다
3. cold daemon startup과 warm steady-state를 같은 표에 합치지 않는다
4. lexical/history/runtime/structural을 하나의 aggregate latency로 발표하지 않는다
5. widening packet (`jun-2-dsl-advanced`)의 shadow/admission bar를 대신하지 않는다

## Decision

성능 측정은 아래 3-layer로 고정한다.

### Layer 1. Compile Timing

이 레이어는 이미 canonical rail이 있으므로 새 설계를 하지 않는다.

- fast lane:
  - `just rust-timings-fast-check`
- daemon lane:
  - `just rust-timings-daemon-check`

비교 기준:

1. current branch vs committed baseline JSON
2. 동일 머신
3. clean target dir
4. 동일 feature set

이 레이어는 shipped DSL query semantics가 아니라
compile closure 비용 회귀만 본다.

### Layer 2. LQ Pipeline Hot Path

이 레이어는 `query text -> tokenize -> parse -> normalize -> hash`
의 pure CPU cost를 본다.

canonical rail:

- `./scripts/cargow --lane bench-lane bench -p quanta-index-lq-norm --bench pipeline --all-features --locked`

비교 기준:

1. current branch vs baseline branch
2. same input sizes (`1 KiB`, `4 KiB`, `16 KiB`)
3. per-stage 비교

이 레이어는 daemon, IPC, ingest, route fanout 비용을 포함하지 않는다.

### Layer 3. Query Latency Matrix

이 RFC의 신규 핵심이다.

shipped DSL surface는 아래 둘로 나눠 측정한다.

1. warm steady-state query latency
2. cold daemon first-query latency

둘은 같은 harness를 쓰면 안 된다.

#### 3.1 Warm steady-state benchmark

adopted harness:

- authority runner: `crates/quanta-index-searchd-harness/src/bin/dsl_warm_matrix.rs`
- exploratory criterion view: `crates/quanta-index-searchd-runtime/benches/dsl_query_matrix.rs`

역할:

1. bench-profile authority binary를 direct exec
2. isolated warm pass마다 fresh runtime boot + deterministic fixture ingest + activate
3. pass를 scenario round-robin으로 interleave
4. isolated pass raw samples를 scenario 단위로 pooled aggregation
5. `p50/p95/p99`를 scenario 단위로 기록

measurement contract note:

- bench query clients use one-shot UDS requests, so daemon accept-loop cadence
  is part of the observed warm path
- the current adopted daemon contract is
  `crates/quanta-index-searchd/src/app/searchd.rs`:
  query accept idle `1ms`, control/ingest accept idle `5ms`
- older warm artifacts captured under a uniform `50ms` accept-loop poll are not
  comparable as-if they measured the same steady-state path
- workstation blocking metric is `p50`; `p95` / `p99` remain advisory tail signals

scenario family:

1. lexical
   - keyword
   - phrase
   - regex
   - `file.contains(...)`
   - `repo.has.file(...)`
2. history
   - `since.time:`
   - `since.commit:`
   - `after:`
   - `until:`
   - `diff.added:`
   - `diff.removed:`
   - `diff.touched:`
3. runtime catalog
   - `dirty:no`
   - `changed:`
   - `stale:`
   - `snapshot:`
   - `meta.owner:`
   - `meta.service:`
   - `meta.layer:`
   - `meta.surface:`
   - `affected:`
   - `invalidated_by:`
4. structural
   - mixed `AND`
   - mixed `OR`
   - mixed `AND NOT`
   - pure-negative root

#### 3.2 Cold daemon benchmark

criterion은 warm loop에 최적화되어 있으므로 cold first-query latency는
adopted runner로 별도 측정한다.

- `tools/benchmark/run_dsl_cold_matrix.py`
- support bin: `crates/quanta-index-searchd-harness/src/bin/dsl_cold_matrix.rs`

역할:

1. clean state root 준비
2. daemon boot
3. fixture ingest
4. activate
5. single query 실행
6. process 종료
7. scenario별 wall-clock latency 기록

gate discipline:

1. cold `p95` gate는 measured row당 최소 `20` samples를 요구한다
2. fewer-sample cold runs are exploratory only; they are not benchmark-gate authority
3. the cold orchestrator should build the harness binary once, then invoke the
   bench-profile binary directly per sample instead of paying `cargo run` orchestration on
   every sample

이 레이어는 아래를 포함한다.

1. daemon boot
2. socket connect
3. first request path
4. route-specific first-touch overhead
5. daemon accept-loop cadence on the first query connection

## Shared Scenario Authority

warm/cold/query benchmark가 test scenario와 drift하면 금방 거짓 숫자가 된다.

따라서 benchmark는 현재 test-only scenario truth와 분리된
**bench-owned scenario authority**를 가져야 한다.

adopted structure:

- scenario authority:
  `crates/quanta-index-searchd-harness/src/scenarios.rs`

여기서 관리할 것:

1. scenario id
2. route family
3. query text
4. syntax (`native` / `sourcegraph`)
5. fixture seed id
6. expected result shape
7. latency class (`warm` / `cold`)

authority-run discipline:

1. warm and cold producers must run serially on the same machine
2. a concurrent warm+cold capture is exploratory only; it is not baseline or gate authority
3. the adopted front door for an authority refresh is `just rust-bench-dsl-refresh 20`
4. warm gate authority comes from `dsl_warm_matrix`; criterion `dsl_query_matrix`
   is exploratory only and must not overwrite baselines
5. warm authority binary and cold probe binary both run from workspace `[profile.bench]`

주의:

- `tests/common/frontdoor_scenarios.rs`를 bench에서 path-include 하는 방식은
  adopted path가 아니다
- test와 bench는 shared harness scenario authority를 사용한다

## Comparison Discipline

### Allowed comparisons

1. same scenario, current branch vs baseline branch
2. same scenario, cold vs cold
3. same scenario, warm vs warm
4. same semantic surface, native vs sourcegraph
   - only when parity rail already exists

### Forbidden comparisons

1. `dirty:no` vs `ripgrep`
2. `affected:` vs text-only engine
3. `runtime catalog aggregate` vs `lexical aggregate`
4. cold result vs warm result
5. `e2e_perf_chaos` elapsed wall-clock vs benchmark latency

## Output Contract

warm criterion output 외에 machine-readable artifact를 남긴다.

- proposed artifact files:
  - `artifacts/dsl-bench/warm-matrix.json`
  - `artifacts/dsl-bench/cold-matrix.json`
  - `artifacts/dsl-bench/summary.md`

each scenario row must include:

1. `scenario_id`
2. `route_family`
3. `syntax`
4. `mode` (`warm` or `cold`)
5. `result_shape`
6. `latency_p50_ms`
7. `latency_p95_ms`
8. `latency_p99_ms`
9. `samples`
10. `git_rev`

optional but recommended:

1. `result_count`
2. `typed_error_code`
3. `engine_touched`
4. `early_stop_reason`

## Gate Strategy

초기에는 절대 SLO gate를 두지 않는다.

이유:

1. query family별 cost model이 다르다
2. shipped DSL surface가 넓어서 한 숫자로 SLO를 둘 수 없다
3. 현재 repo에는 query-latency baseline artifact가 아직 없다

따라서 2-phase로 간다.

### Phase A. Baseline capture

1. warm matrix baseline 생성
2. cold matrix baseline 생성
3. artifact commit
4. no fail gate, report-only

### Phase B. Relative regression gate

scenario별 ratchet rule:

1. warm:
   - fail if `p50` regression > `10%` and absolute delta > `1.0 ms`
   - `p95` / `p99` are advisory-only on workstation authority runs
2. cold:
   - fail if `p50` regression > `10%` and absolute delta > `5.0 ms`
   - `p95` / `p99` are advisory-only on workstation authority runs

compile timing gate는 기존 규칙 유지:

1. regression > `15%`
2. and absolute delta > `0.10 s`

이 수치는 final absolute SLO가 아니라 **regression guard**다.

## Canonical Commands

현재/최종 command set:

```bash
just rust-timings-fast-check
just rust-timings-daemon-check
./scripts/cargow --lane bench-lane bench -p quanta-index-lq-norm --bench pipeline --all-features --locked
just rust-bench-dsl-warm
python3 tools/benchmark/run_dsl_cold_matrix.py --samples 20 --out artifacts/dsl-bench/cold-matrix.json
```

Phase B gate command 추가 시:

```bash
python3 tools/ci/benchmark/compare_dsl_bench.py \
  tools/ci/benchmark/baselines/warm-matrix.json \
  artifacts/dsl-bench/warm-matrix.json

python3 tools/ci/benchmark/compare_dsl_bench.py \
  tools/ci/benchmark/baselines/cold-matrix.json \
  artifacts/dsl-bench/cold-matrix.json
```

## Why This Is The Final Recommendation

이 안이 최종안인 이유는 다음이다.

1. 이미 있는 canonical rail을 재사용한다
   - compile timing
   - lq-norm criterion bench
2. 없는 것만 새로 만든다
   - query latency matrix
   - cold first-query runner
3. correctness rail을 benchmark로 오용하지 않는다
4. shipped surface family를 그대로 benchmark family로 반영한다
5. future widening (`jun-2-dsl-advanced`)에도 같은 구조를 그대로 확장할 수 있다

## Files To Add

1. `crates/quanta-index-searchd-harness/src/bin/dsl_warm_matrix.rs`
2. `crates/quanta-index-searchd-runtime/benches/dsl_query_matrix.rs`
3. `tools/ci/benchmark/run_dsl_cold_matrix.py`
4. `tools/ci/benchmark/compare_dsl_bench.py`
5. `tools/benchmark/baselines/warm-matrix.json`
6. `tools/benchmark/baselines/cold-matrix.json`

## DoD

이 RFC는 아래가 다 만족될 때 구현 완료로 본다.

1. compile timing은 기존 canonical rail을 그대로 사용한다
2. pipeline hot path는 `lq-norm` criterion bench로 계속 측정된다
3. shipped DSL query family별 warm benchmark가 존재한다
4. shipped DSL query family별 cold first-query benchmark가 존재한다
5. output artifact가 scenario별로 machine-readable하다
6. gate는 family-aggregate가 아니라 scenario-relative drift 기준이다
7. `e2e_perf_chaos`는 여전히 benchmark가 아니라 correctness/observability rail로 유지된다

## Failure Modes

1. warm/cold를 섞어 숫자를 발표함
2. lexical/history/runtime/structural을 한 aggregate latency로 합침
3. native/sourcegraph parity가 없는 surface를 cross-syntax 비교함
4. correctness rail wall-clock을 benchmark 결과로 재사용함
5. bench scenario가 test scenario와 drift함
6. baseline 없이 절대 수치만으로 pass/fail을 선언함
