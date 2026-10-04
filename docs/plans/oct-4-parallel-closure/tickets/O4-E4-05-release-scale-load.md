# O4-E4-05 — matching release scale·load·restart 실행

| 항목 | 값 |
| --- | --- |
| 에픽 / 담당 | [E4 — 인덱싱·typo 실행 비용·release 성능·scale](../epics/E4-storage-query-and-scale.md) / E4 담당 |
| 우선순위 / 종류 | P2 / `EXECUTION_AND_PROOF` |
| 기준 웨이브 | [W4 — 실제 캡처·성능·scale](../waves/W4-native-capture-performance-and-scale.md) |
| 실행 상태 | harness 회귀·source107 release build·default small16/medium256 scale·small open-loop `VERIFIED`; large default/diagnostic `FAILED`(timeout/ANN segment contraction), 구조 수리 준비. 다른 tier·OS restart·qualified performance 미완료 |
| 선행 결과 | [O4-I0-02](O4-I0-02-matching-source-proof.md), [O4-E4-01](O4-E4-01-index-phase-profile.md) |

[전체 지도](../README.md) · [티켓 인덱스](INDEX.md)

## 목적

현재 release harness가 실제 규모·overload·lifecycle에서 동작하는 범위와 제한 거절을 구분한다.

## 배경과 현재 상태

현 scale history2, open-loop8, default16MiB이며 explicit300s/256MiB는 별도 diagnostic profile이다. old256 lifecycle은 과거 source,4096 timeout/retention exhaustion,32768 posting-cap refusal였다. SDK proof는 scale/open-loop binary를 build/execute한 증거가 아니다.

## 2026-10-04 실행 준비와 owner 검증

- sourcebf의 I0 owner 배치가 972/972 passed였다. 선택에 harness lib/bin의 scale·open-loop source fixture, history/timeout bounds, requested/effective refusal JSON, offered accounting과 existing lifecycle oracle가 포함됐다. 실제 release tier/load를 실행한 결과는 아니다.
- 각 medium256/large4096/xlarge32768 tier는 별도 fresh external root에서 실행한다. default scale timeout30s/history16MiB·2generations와 explicit300s/history256MiB는 구별한다. open-loop request timeout의 허용 상한은30s이며 scale300s 인자를 그대로 적용하지 않는다.
- 현재 `delete_reopen`은 same-process runtime 재개방이다. OS child restart, cold page cache, physical write I/O 및 4,096/32,768문서 독립 ranking gold는 이 harness artifact로 증명하지 않는다.
- 아래 source107 결과 이전의 release build/run은 `NOT_RUN`이었다. 공유 macOS host의 diagnostic 실행에서 frequency/thermal/quiet qualification을 합성하지 않는다.

## 2026-10-04 source107 matching release binaries

- `VERIFIED`: clean `/Users/songmin/.codex/worktrees/oct4-qualified-source/quanta-index`의 `./scripts/cargow --lane release-lane build -p quanta-index-searchd-harness --bin scale_matrix --bin open_loop_matrix --all-features --locked --release` — exit0,14m50s. 실제 binaries의 `--help`도 각각 exit0으로 current CLI를 확인했다. 빌드/usage만으로 tier qualification을 주장하지 않는다.
- target은 `/Users/songmin/Library/Caches/quanta-index/target/222449791cf7286e/release-lane/release`다. scale_matrix SHA `2181bd0c047976859b516d6002220189f073ac83efabf22567d5a9af6cf5d2b0`, open_loop_matrix SHA `8a10e58e2b81f406cf856c3f2e5977f4491ec340ed5236fddab146d1eca37c31`를 실제 bytes에서 계산했다.
- default 각 tier와 diagnostic override는 별도 fresh external root에서 직렬 실행한다. summary 존재·producer exit만 보지 않고 실제 passed/config/tier/phase/point accounting 및 resource refusal를 판독한다. same-process reopen와 OS-process restart를 구분한다.

## 2026-10-04 default small·medium 실제 실행

- `VERIFIED`: 위 scale binary의 `--tier small --seed 5864059738136528177 --out-dir /private/tmp/qi-scale-small-107-v1/result` 및 medium의 `/private/tmp/qi-scale-medium-107-v1/result`를 각각 새 root에서 실행해 exit0이었다. 두 summary는 source107, `detail.passed:true`, result_count10, requested timeout/history null 및 effective30s/16MiB·2generations다.
- small16files/1repo: build1027.826ms, activate44.697ms, first query8.496ms, warm p504.525ms. 여섯 lifecycle resource phases가 존재한다. medium256files/4repos: build5557.130ms, activate87.676ms, first query11.275ms, warm p507.498ms. full ingest/seal·delete seal/activate·same-process reopen을 포함한 열 resource phases가 존재하며 delete seal1161.930ms, same-process reopen218.261ms다.
- 같은 OS process의 재개방이며 실제 process restart를 증명하지 않는다. sampled RSS/allocated-root observations와 logical bytes는 true peak·physical I/O가 아니다. 큰 tier 및 open-loop 결과를 이 두 실행으로 합성하지 않는다.

- `FAILED`: default large4096은 `/private/tmp/qi-scale-large-107-v1/result/refusal.json`에서 `status:failed`, `stage:build_seal`, `ipc Read timed out after 30000 ms`이며 producer exit1이었다. source107,16source repos,8,607,106source bytes, requested overrides null/effective30s·16MiB로 결속됐다. build 완료나 이후 lifecycle 성공을 합성하지 않는다.
- `FAILED`: 별도 `/private/tmp/qi-scale-large-diag-107-v1/result`의 같은 tier/seed, `--client-timeout-ms 300000 --history-max-bytes 268435456` actual run도 exit1이다. refusal의 stage는`delete_seal`이며 `inherited index has 1 segments, the base generation sealed 2`로 semantic seal이 거절됐다. 기본30s timeout과 다른 실패이며 diagnostic을 default PASS로 합치지 않는다.
- current harness는 delta로 바꾼 첫 파일을 다음 단계에서 삭제한다. 기존 ANN append admission은 segment count만 보존된다고 가정했다. 전체 appended segment의 live rows가 삭제될 때 library가 빈 segment를 제거하는 경계를 independent300+60→delete60 fixture 및 forged UUID negative로 좁혀 검증한다. 실제 fixture·수리는 완료 전이다. base seal 검증이나 coverage/UUID 검사를 약화하지 않는다.
- `VERIFIED`: source107 small open-loop `--tier small --seed 5715144129723191120 --out-dir /private/tmp/qi-open-small-107-v1/result` actual exit0 및 summary readback을 확인했다. 기본25/50/100/200QPS·10s·32workers·queue256·2s timeout에서 offered/served254/490/1041/1958, 합계3743이며 typed/unexpected/timeout/transport/invalid/drop 모두0이다. 네 구간 unsaturated이며 공유 Darwin/hash-dev 단일 진단이다.

## 착수 입력과 실제 tier 실행

- I0-02 frozen source/release runner·daemon 외 matching scale/open_loop binaries
- 실제 256/4096/32768 inputs, ScopedOracle와 independent lifecycle 기대값, serial resource host
- default와 explicit diagnostic timeout/history profile

## 어떤 파일을 어떻게 수정할지

`OWNED`는 에픽 담당 통합, `SHARED`는 I0 반영, `READ`는 기존 구현 소비다. 재현된 결함이나 채택된 계약 변경이 있을 때만 product source를 수정한다. 구현 파일과 독립 검증 파일을 함께 지정한다.

| 파일 | 함수 / 경계 | 구체적인 변경 또는 검증 | 모드 |
| --- | --- | --- | --- |
| [crates/quanta-index-searchd-harness/src/scale.rs](../../../../crates/quanta-index-searchd-harness/src/scale.rs) | ScaleRuntimeConfig / generate_scoped_corpus / ScopedOracle / lifecycle 실행 | 현 fixed source oracle/phase resource validation을 사용하고 actual OS restart proof의 누락만 보강한다. | OWNED |
| [crates/quanta-index-searchd-harness/src/open_loop.rs](../../../../crates/quanta-index-searchd-harness/src/open_loop.rs) | schedule / measure_point / run / artifact | fixed arrivals, offered/success/error accounting, saturation vs correctness failure를 검증한다. | OWNED |
| [crates/quanta-index-searchd-harness/src/bin/scale_matrix.rs](../../../../crates/quanta-index-searchd-harness/src/bin/scale_matrix.rs) | current CLI/output | fresh external --out-dir와 requested/effective config를 결속하고 refuse/timeout을 pass로 바꾸지 않는다. | OWNED |
| [crates/quanta-index-searchd-harness/src/bin/open_loop_matrix.rs](../../../../crates/quanta-index-searchd-harness/src/bin/open_loop_matrix.rs) | current CLI/result gates | 동일 resource policy·default/override profile·terminal outcomes를 기록한다. | OWNED |
| [crates/quanta-index-searchd-runtime/tests/e2e_restart_replay_determinism.rs](../../../../crates/quanta-index-searchd-runtime/tests/e2e_restart_replay_determinism.rs) | runtime_risk_suite restart | same-process reopen와 실제 process restart의 row/source/pin parity를 분리 검증한다. | SHARED |
| [docs/plans/jun-7-search-product-quality/tickets-wave2/J7Q-03-large-corpus-scale-tiers.md](../../../../docs/plans/jun-7-search-product-quality/tickets-wave2/J7Q-03-large-corpus-scale-tiers.md) | scale verdict | current source별 success/refusal/not_run을 갱신한다. | OWNED |
| [docs/plans/jun-7-search-product-quality/tickets-wave2/J7Q-04-latency-tail-hardening.md](../../../../docs/plans/jun-7-search-product-quality/tickets-wave2/J7Q-04-latency-tail-hardening.md) | load verdict | offered/executed/failed/saturation과 tail/resource limits를 갱신한다. | OWNED |

## 실행 단계

1. owner-local harness oracle와 override refusal을 실행한 뒤 matching release harness binaries를 별도로 freeze한다.
2. 256부터 full/delta/delete/no-op/reopen을 실행하고 각 단계에서 independent source expected count/order/hash를 확인한다.
3. default4096/32768를 실행해 success/refusal/timeout capacity boundary를 측정한다.
4. explicit300s/256MiB 필요 시 별도 root/profile로 실행하며 default 통과와 합치지 않는다.
5. real OS process restart/cold-open과 open-loop fixed offered schedule을 실행하고 offered requests를 완전 회계한다.
6. sampled RSS/CPU/gap·logical vs allocated disk·transient high water와 saturation point를 보고한다.

## 검증 계획 — NOT_RUN

아래는 실행할 명령/시나리오다. 본 문서에서 통과를 주장하지 않는다. `<...>`와 외부 root는 실행 전에 실제 값으로 확정한다. test filter는 실제 수집 ID를 확인하고 0 tests를 성공으로 표시하지 않는다.

- `./scripts/cargow test -p quanta-index-searchd-harness --lib --bins --all-features --locked`
- `./scripts/cargow test -p quanta-index-searchd-runtime --test runtime_risk_suite --all-features --locked e2e_restart_replay_determinism`
- actual scale/open_loop CLI는 --help로 flags를 확인하고 output를 checkout 밖으로 지정한다.
- Negative: wrong fixture identity, missed offered requests, history override concealment, nonfinite latency, phase sample gap·wrong count/source after delete refusal.

## 완료 조건

- matching release binary의 각 tier/profile/lifecycle에 independent result와 terminal outcome이 있다.
- successful runtime capacity와 enforced resource refusal, diagnostic override·OS restart를 따로 표시한다.

## 중단·거절·재개 조건

- 실패 tier는 성공 timing으로 기록하지 않는다. sampled max를 true peak·logical delta를 physical write I/O로 표시하지 않는다.
- 필요한 입력 부재는 `BLOCKED`, 미실행은 `NOT_RUN`, 실제 실행 실패는 `FAILED`로 기록한다. 조건 미성립 `NOT_APPLICABLE`에는 실제 판단 근거가 필요하다.
- 변경이 source/input/query/unit/result에 영향을 주면 [I0 source gate](O4-I0-02-matching-source-proof.md)와 영향받는 capture/report를 다시 판정한다.
- 일회성 raw/log/capture/receipt는 checkout 밖 새 root에 둔다. 기존 외부 terminal을 덮어쓰지 않는다.

## 인계 결과

- 실제 source/dirty ownership, 변경 파일과 계약, 실행한 명령/selector, 관측 결과 및 제외 범위.
- raw/model/runtime/binary/input identity는 해당 실행 계약이 요구하는 범위에서 기록한다.
- 완료 조건별 `VERIFIED`/`FAILED`/`BLOCKED`/`NOT_RUN`/`NOT_APPLICABLE`과 후속 티켓에 넘길 입력을 발행한다.
