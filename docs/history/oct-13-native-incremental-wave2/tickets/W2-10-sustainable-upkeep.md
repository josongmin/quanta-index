# W2-10 — 측정된 write amplification·유지보수 부채의 bounded 수리

> HISTORICAL REFERENCE — 2026-10-18 retirement. Current work/status: [Oct-17 SSOT](../../../plans/oct-17-index-closeout/tickets/README.md). Old TODOs, owners, commands and counts below do not authorize new work or qualify current source.


> `HISTORICAL_DESIGN_REFERENCE`. 현재 잔여/상태는 [Oct-17 SSOT](../../../plans/oct-17-index-closeout/tickets/README.md#w2-10)만 따른다. 아래 과거 TODO·진척·실행 순서는 신규 작업을 만들지 않는다. 이 파일에는 상태/담당을 갱신하지 않는다.

기존 요구/큐: R17,R10,R12,R39 / NI-03. 설계 계약은 [RFC](../../../rfc/oct-10-native-incremental-vnext.md), 실행 결과는 [VALIDATION](../../../plans/oct-10-index-closeout/VALIDATION.md)이 소유한다.

## Reuse

[x] `DONE_IMPL` 재사용 경계: COW delta·bounded maintenance·자동 reclaim/retention pump. 이 표시의 범위는 명시한 하위 구현이며 티켓 전체 검증 완료를 뜻하지 않는다. 새로운 결함이 확인되지 않으면 재구현·재배정하지 않는다.

## Scope

입증된 비용/부채 문제에 한해 기존 bounded maintenance/reclaim/grouping 정책을 변경한다.

W2-09의 Index-native 비용 입력은 미지정 고정 foreground WorkLimits 또는 명시적
`QUANTA_INDEX_NATIVE_WORK_LIMITS_JSON` 네 축(`reads`·`writes`·`bytes`·`mutations`)이다.
명시값은 양의 `u64` 정확 4필드로 decode하고 generator token-mutation 하한을
넘어야 하지만, 이는 실제 full ACK·유지보수까지의 충분 예산이 아니다. 값이 바뀌면
`fixed`/`explicit` mode·선택 4축·config digest가 다른 workload다. 결과를 본 뒤
한도를 올려 원래 profile의 수용으로 보고하지 않는다. physical cap,
execution memory/I/O grant, maintenance WorkLimits와 request deadline은 별개다.

선택적 child sampler의 Linux `/proc/<pid>/io` read/write/cancelled-write byte는
socket-ready 이후부터 shutdown signal 전까지의 **OS process-accounted** 표본 창
차이다. 100ms 주기·종료 전 경계 표본의 누락·관측 창·counter 역행을 함께 검사한다.
비Linux는 `unsupported`이며, device/media write bytes나 전체 수명 총량으로
대체하지 않는다. RSS100ms와 root allocated blocks500ms도 sampled maximum이지
연속 peak가 아니다. Index-native trace의 이름 붙은 26개 phase는 harness 경계이며
child daemon 내부 maintenance/GC의 원인별 phase가 아니다. phase span은 작업
본체를 감싸지만 `wall_ms`와 정확히 같지 않다. `wall_ms`에는 trace begin/finish
비용도 포함될 수 있고 전후 disk census는 둘 밖의 `observer_ms`다. 후행 census 실패 시
span success와 phase `completed=false`가 함께 기록될 수 있다. 주기 sampler의
오류·누락·counter 역행은 observation metadata이며 root `Ok`만으로 표본의
완전성을 인정하지 않는다. Linux `write_bytes`와 `cancelled_write_bytes`는 독립
원값/차분으로 보존하며 취소량을 write에서 빼 순수 매체 쓰기로 만들지 않는다.
계수 의미는 [Linux kernel proc 문서](https://www.kernel.org/doc/html/v6.12/filesystems/proc.html#proc-pid-io-display-the-io-accounting-fields)를 따른다.

상태 조회 보완 후 Core/Searchd/Harness·`scale_matrix` selected strict와 Mac 선정
회귀 **52/52는 `VERIFIED`**다. 이전 44/45건과 합산하지 않는다. Pre-reconciliation source: Linux daemon/harness build와 Linux 선택 회귀10/10, Small Off/Record 각20 churn·검색/재오픈·IO gold 및 누락0 capture는 **`VERIFIED`**다. Medium fixed는 full ACK `ResourceLimited` **`FAILED`**(churn0)이며 고정 입력을 증액하지 않는다. Large explicit도 full ACK에서 SDK `Unknown(original key)`로 **`FAILED`**다. 원 key status 확인을 빠뜨린 harness 호출부를 보완했으며 영향 회귀52/52·primary strict는 통과했고 새 actual/XL은 **`NOT_RUN`**이다. 기존45/45·이전 strict는 후속 수리 전 소스 결과로 보존한다. 범위와 명령은
[VALIDATION](../../../plans/oct-10-index-closeout/VALIDATION.md)의 최신 W2-09 기록을
따른다. 이 자료만으로 policy 수리 필요성·steady-state 안정성 또는 W2-10
수용을 판정하지 않는다.

## Acceptance

- 같은 fixed live set의 churn/delete/no-op 및 old/current-root gold를 유지한다.
- cancel/fault/restart/long pin/space pressure에서 debt·retained bytes·writes·time을 계수하고 마지막 pin 해제 후 drain을 확인한다.
- 같은 source/config/profile에서 Linux child I/O의 표본 창과 누락, root allocated disk·RSS, 26개 harness phase의 범위를 함께 기록한다. child 내부 write amplification과 GC debt는 별도 계수·gold로 확인하고 비Linux I/O `unsupported`나 미관측 구간을 0으로 간주하지 않는다.
- W2-09가 정책 결함을 입증할 때만 기존 policy/grouping을 수리한다. 필요 없음이 입증되면 새 코드 없이 근거로 닫는다. cap/refusal을 성공으로 숨기지 않는다.

## Source entry points

- [native_pump.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/src/native_pump.rs)
- [lexical_compaction.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-lexical/src/lexical_compaction.rs)
- [maintenance_stats.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-lexical/src/maintenance_stats.rs)
- [Native workload selection](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-harness/src/scale/native_index/workload.rs)
- [Native child observations](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-harness/src/scale/native_index/observation.rs)
- [Native harness trace](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-harness/src/scale/native_index/trace.rs)

공용 parent/export/contract/config/registry 변경은 [단일 writer 규칙](../../../plans/oct-13-native-incremental-wave2/tickets/WRITE-OWNERSHIP.md)을 따른다. 검사 미실행을 기능 부재로 바꾸지 않는다. 결과에는 source·명령·범위·VERIFIED/FAILED/BLOCKED/NOT_RUN/NOT_APPLICABLE을 구분한다.
