# W2-09 — 실제 phase 비용·부하·유지보수 운영 수용

> HISTORICAL REFERENCE — 2026-10-18 retirement. Current work/status: [Oct-17 SSOT](../../../plans/oct-17-index-closeout/tickets/README.md). Old TODOs, owners, commands and counts below do not authorize new work or qualify current source.


상태는 [SSOT](../../../plans/oct-17-index-closeout/tickets/README.md#w2-09)만 갱신한다. 이전 W2-10은 여기의 **측정 후 조건부 수리**로 합쳤다. 실행 결과는 [기존 VALIDATION의 최신 체크포인트](../../../plans/oct-10-index-closeout/VALIDATION.md#w2-09-native-refusal-diagnostics-2026-10-18)에만 기록하며, 이전 funding/IO/trace 결과도 해당 체크포인트에서 연결한다.

## Remaining scope

- 실제 Semantica capture/analysis·async catch-up/fanout과 child daemon Catalog/WAL/object/scratch/seal/publish/GC의 비용을 기존 trace/IO caller에 귀속한다. 미계측 phase/원 authority·측정 창을 먼저 정적으로 열거한다.
- 기존 harness에서 funded profile·실 producer·qualified Off/Record·총비용, 지속 churn/긴 pin/공간 압박·aggregate RSS/temp-disk·debt/drain을 수용한다.
- 측정에서 실제 유지보수 결함이 드러날 때만 기존 bounded policy/grouping/reclaim을 좁게 수정한다. threshold/reserve/scheduler/compactor/queue 전체를 선제 구현하지 않는다.

## Reuse and proof limits

4축 foreground WorkLimits JSON·strict decode/하한 거절·config/refusal binding, 선택적 child RSS/allocated blocks/Linux process IO sampler, 26개 harness phase, 유한 churn·ACK/head 재오픈 gold는 구현돼 있다. 기존 bounded worker와 long-pin8회 기능 증거도 유지한다.

Aggregate RSS는 Linux `/proc`의 harness/daemon과 관측된 자식 집합을
PID·start-time identity로 합산한다. 관측 뒤 reparent된 자식도 유지하고,
재사용된 PID는 합산하지 않는다. census와 누적 identity는 각각4096개로
제한한다. 사라진 process/누락 VmRSS/취소/한도 초과는 missing이며0이 아니다.
공유 페이지는 process RSS 합에서 중복 계수될 수 있고, 두 관측 사이에
생성·소멸한 process와 연속 peak는 증명하지 못한다. 기존 socket-ready→
shutdown-signal 관측 창을 유지한다. Native scratch는 실제
`native-scratch-v1` namespace의 할당 블록을 별도로 기록한다. 하나의 fixture
순회에서 전체 임시 fixture(config/stderr/trace 포함), state, scratch를 inode별로
집계한다. hard link는 각 범위에서 한 번만 계산하고 순회 비용은 fixture에만
귀속한다. fixture 밖의 임시 저장소·전체 생애 disk peak 증거와 구분한다.

- [native_index.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-harness/src/scale/native_index.rs), [workload.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-harness/src/scale/native_index/workload.rs), [observation.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-harness/src/scale/native_index/observation.rs), [trace.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-harness/src/scale/native_index/trace.rs).

새 runner·artifact·측정 원장·자동 budget 증액은 만들지 않는다. harness span은 daemon 내부 원인별 span이 아니다. OS-accounted IO는 실제 매체/전체 생애 IO가 아니고 표본 최대는 연속 peak 보장이 아니다. 비Linux unsupported·누락·역행·실패 prefix를 보존한다. CoreRequired text의 catch-up NOT_APPLICABLE을 전체 producer/semantic 완료로 확대하지 않는다.

## Inputs, ownership and acceptance

기존 harness 담당이 이미 제출한 실행을 먼저 소비한다. 각 producer/daemon source owner가 자신의 actual phase hunk만 수정하고 공용 parent는 통합한다. W2-05의 실제 producer 비용을 합류하며 Index-only 측정은 독립 진행 가능하다.

실행 전 source/dependencies/config/binary/host, corpus N/H/M/D/K/output·vector/query/filter/update·기간, 네 축 foreground budget 및 물리/memory/maintenance cap·deadline을 확정한다. required SLO가 미정인 탐색/기능 실행은 qualification PASS가 아니다.

현재 원장 이전의 고정 Large full-ACK ResourceLimited와 Medium fixed 실패는 보존한다. Small Off/Record 기능·검색/재오픈/누락0 capture 통과는 qualified overhead 비교를 닫지 않는다. Large/XL 명시 budget은 다른 입력 profile이며 기존 고정 profile 성공으로 합치지 않는다. 최신 실행 결과는 VALIDATION에서 조회한다.

## Default acceptance profile — 2026-10-18

사용자가 target/SLO 선정을 위임했다. 아래는 **새로운 초기 수용 목표**이며
현재 측정값·SOTA 성능 인증·운영 배포 성공을 뜻하지 않는다. 기존 고정/명시
profile의 limit/deadline·실패는 보존한다. 이후 변경은 원인/제품 요구와 별도
profile revision으로 처리하며 결과를 보고 같은 실행의 합격선을 바꾸지 않는다.

- Target: Linux aarch64, 2 dedicated CPU, 6GiB memory, local SSD-backed
  filesystem, foreground benchmark 이외의 build/test·부하 없음. 현재 Docker
  functional target의 자원 형태를 채택하되 host storage/cache 경합과 공유 CPU가
  통제되지 않은 실행은 qualified timing이 아니다. runtime/binary/source,
  filesystem·mount·kernel·cgroup와 사용 가능한 공간을 실행 전에 결합한다.
- Build: 기존 workspace `bench` profile(opt3, LTO off, codegen-units16)의
  동일 daemon/harness binary를 사용한다. source/dependencies/compiler/flags·binary
  SHA를 결합하며 debug/dev binary의 거절·지연을 이 성능 수용으로 해석하지 않는다.
  실제 배포 `release` profile과 cutover/release qualification은 W2-19의 별도 범위다.
- Corpus: 기존 Large(seed42, 16 repositories ×256 files ×120 average lines).
  기존 explicit foreground WorkLimits는 reads/writes/mutations 각67,108,864,
  bytes549,755,813,888. execution memory/IO grant64MiB 각각,
  capacity memory384MiB/IO320MiB, physical64GiB·files200K·entries500K와
  scratch reserve16GiB를 유지한다. 이 논리 work funding은 RSS/disk 상한이 아니다.
- Load: full ACK와 independent search/reopen gold 후 15분, open-loop 50 query/s,
  query 동시성 최대4·발행 대기 최대64(초과도 실패로 계수); 1초마다1파일
  replace/delete/restore를 순환한다(각 source
  ≤16KiB). query 70% unique hit,20% bounded broad hit,10% miss, top20,
  repositories round-robin. 60초 warmup을 따로 기록하고 이후 ≥10,000 attempts,
  각 class ≥1,000을 요구한다. 예정 시각 기준 queue delay와 실패/timeout을
  분모에 포함한다. 매 update 후 old-view witness1회를 추가 발행하고
  별도로 계수한다. current/old gold를 확인하며 중단된15분 배치를 합격시키지 않는다.
- Pin: 시작 view 하나를 최초10분 보유하고 이후 release. 동일한 파일 집합과
  source 크기로 churn하여 무제한 logical corpus 증가와 누수를 구분한다.
  기존 Kernel pin/ticket의 실제 획득·유지·마지막 release를 관측해야 한다.
  `ViewRef` 변수 보관·과거 view 검색 성공만으로 장기 pin을 입증하지 않는다.
  현재 UDS harness의 old-view gold는 이 수용을 대신하지 않으며, 측정만을
  위한 별도 pin authority·우회 API를 만들지 않는다.

| Acceptance | Initial target | Boundary / failure rule |
| --- | --- | --- |
| Correctness / durability | 정상 부하의 fixture gold 전부 일치; accepted publication 유실0 | ACK 후 current/held-old view, delete/no-op, reopen/head, final drain 포함. refusal/Unknown을 성공·샘플 제외로 처리하지 않음. 의도된 fault/overload는 별도 oracle |
| Warm lexical query | p95≤100ms, p99≤500ms | client 예정 발행→decoded top20, 실패/timeout 포함. cold/reopen은 별도 기록. ANN/semantic은 W2-13의 recall/coverage gate를 추가로 충족해야 하며 lexical 결과로 대체하지 않음 |
| One-file freshness | p95≤1s, p99≤5s | source staging 시작→원 operation terminal ACK→같은 committed view에서 독립 검색 gold 확인. admission/queue/staging을 빼지 않음 |
| Resource bound | aggregate process RSS≤2GiB; allocated scratch≤4GiB; allocated physical≤64GiB | harness와 모든 daemon/worker child 포함. 100ms 표본 최대는 observed maximum으로만 기록; continuous peak는 기존 admission/OS high-water 증거가 있어야 판정. capacity denial/error·누락0 요구 |
| Maintenance convergence | release 후60s 이내 reclaimable debt0;5s shutdown grace 준수 | pin이 보호하는 객체는 debt0 대상에서 제외하되 union allocated에 포함. 마지막 pin 해제 후 보호가 사라진 객체·FD/lease·scratch 정리와 durable reopen을 원 oracle로 확인 |

시간 목표와 무관하게 cancellation·ENOSPC/short-write/fsync/checked-close의
기존 fail-closed 계약을 유지한다. 이미 완료된 bounded fault/long-pin gold를
반복 생성하지 않는다. 이 profile의 aggregate 혼합 부하/관측 연결은 기존
harness에서 추가·검증할 잔여이며, 현재20회 순차 churn이 15분/50QPS 수용은 아니다.
Large functional full ACK가 실패한 상태에서는 이 성능 배치를 제출하지 않는다.
XL은 기존 독립 functional profile로 유지하고 Large 결과로 대체하지 않는다.

Off/Record는 기존 TRACE-WATERFALL-PLAN의 동일 binary/input/caps·교대 Large
최소3쌍·ratio 중앙값 증가≤5%·drop0 계약을 재사용한다. source/host readiness,
footer/loss, 실제 UI 표시와 timing qualification은 각각 판정한다. 현재 Small
기능 capture나 이 기본 목표의 문서화로 이전 BLOCKED/NOT_RUN을 PASS로 바꾸지 않는다.

근거: [Google SRE의 SLI/SLO 설계](https://sre.google/workbook/implementing-slos/)는
사용자 관점 latency/freshness/correctness와 측정 구현을 구분한다.
[Zoekt FAQ](https://github.com/sourcegraph/zoekt/blob/main/doc/faq.md)는 코드검색
시간이 corpus/query/result limit/hardware/cache에 의존함을 명시한다.
[RocksDB write stalls](https://github.com/facebook/rocksdb/wiki/Write-Stalls)는
background 작업이 입력을 따라가지 못하면 공간·읽기 비용이 누적되어 bounded
backpressure가 필요함을 설명한다. 위 숫자는 이 원칙에 따른 제품 초기 선택이며
해당 시스템들의 성능 수치를 Quanta에 전용한 것이 아니다. 새 storage engine,
전역 optimizer·scheduler·추가 runner 도입 근거로 사용하지 않는다.

## Same-key publication observation

기존 Large explicit 실행은 full-event 단계에서 `NativeUdsError::Unknown(original OperationKey)`로 실패했고, terminal Catalog 상태/근본 원인은 그 결과만으로 확정되지 않았다. 186초 wall time을 특정 timeout 원인으로 해석하지 않는다. Medium fixed ResourceLimited도 소진한 work/lease/physical 축이 확인된 결과가 아니다.

[publication.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-harness/src/scale/native_index/publication.rs)의 `publish/reconcile_submit/observe_terminal`과 모든 publication caller는 원 키 status 관측을 사용한다. 해당 수리의 선정 gold와 primary strict는 검증됐으므로 다시 구현·중복 실행하지 않는다. 현재 상태는 README, source/selector별 결과는 VALIDATION만 소유한다. 이전45/45/Small 결과를 새 actual profile 수용으로 승격하지 않는다.

동결한 helper 계약은 submit 최대1회, Accepted/응답 Unknown/SDK Unknown의 **같은 키·같은 publication deadline** status 관측, Pending→Published와 immediate ACK 구분, Refused·Unknown·status 오류·취소/expiry의 실패 보존이다. 결과 metadata는 submit/status count·completion을 분리한다. 선정 gold는 이 caller 계약을 검증하며 실제 Small/명시 Large/XL의 functional 수용은 별도 기존 실행 큐에서 판정한다. 이벤트 재송신/자동 resume·새 operation·deadline/budget 증가로 성공을 만들지 않고 원 실패를 지우지 않는다.

여기의 submit1은 측정 caller의 계약이며 W2-05 production idempotent replay/resume 정책의 대체물이 아니다. producer/daemon body의 trace/counter 추가는 해당 기존 source writer가 인계받아 수행하고, 계측 lane이 serializer·게시·query 알고리즘을 함께 재작성하지 않는다.

## Resource, trace and steady-state acceptance

Native Serve는 명시적 `--diagnostic-trace`를 받아 기존 capture/producer lease를
boot→service→worker drain까지 유지하도록 연결했다. offline Native 명령은
capture를 거절한다. capture 파일 생성 전에 기존 hardened umask를 적용한다.
0이 아닌 process 종료는 성공 span으로 기록하지 않고 원 exit/error를 보존한다.

기존 `RequestBudgetV1::with_diagnostic_context`,
`WorkBudget::request().diagnostic_context()`, `UdsServer::with_diagnostic_context`를
재사용한다. startup/foreground IPC 요청에서 실제 Kernel `build_members`와
`commit_transition`에 context를 전달한다. Pending/Unknown publication은 성공
terminal로 기록하지 않는다. 별도 Core context/port/IPC trace 형식은 추가하지 않았다.
기존 harness Record는 첫 daemon과 reopen 각각의 child capture에 대해 독립적으로
0600 파일 권한·stream/clock identity·event sequence·span balance·footer counter와
boot/request 증거를 검사한다. 첫 daemon에는 실제 build/publish 증거도 요구한다.
이 연결의 executable 검증은 아래 원장 결과로 판정하며 구현만으로 수용하지 않는다.

Child와 harness는 각각의 clock/capture다. 이 구현은 cross-process clock 정렬이나
정식 source/build-bound trace qualification을 뜻하지 않는다. OwnedPump/background
예산의 context, 세부 seal/admit/Catalog/WAL/scratch/GC caller와 실제 producer
귀속은 아직 남아 있다.

최소 결합 순서는 기존 capture/producer lease의 Native boot→drain 수명,
원 operation의 worker→Kernel context, 각 owner의 실제 caller 계측이다.
대상은 `kernel/update.rs`의 admit/prepare/build/publish,
`catalog/native/authority.rs`의 원자 commit,
`storage/object.rs`의 batch/write, `storage/scratch.rs`의 namespace/cleanup,
`native_pump.rs`→Catalog connection의 WAL checkpoint,
`native_reclaim_driver.rs`→`storage/object/reclaim.rs`의 GC다.
공용 CLI/Runtime/Kernel hunk는 기존 통합자 한 writer가 인계받으며,
producer serializer나 물리 원장을 별도로 복제하지 않는다.

Object install의 `storage/object/io_observation.rs::ObjectIoObservation`와
`FileObjectStore::with_io_observation`는 이미 write/sync/reserve/confirm을 수집한다.
Native `build`의 `open_managed_with_io` 조립에 Record일 때만 이 기존
수집기를 연결했다. Off는 observer를 만들거나 snapshot하지 않는다. 실제
service/pump/worker/provider join 이후 원래의 quiescent/no-in-flight 조건으로
snapshot한다. 기존 trace root에 sync 호출·실패·elapsed 합계를 기록하고,
capture ID가 일치하는 단일 bounded stderr 진단의 23개 unsigned 원값을 harness가
검증한다. trace 합계와 세 sync 종류의 checked 합계가 같아야 한다. 누락·중복·
잘린 줄·overflow·비정수는 거절하며 missing을0으로 바꾸지 않는다. first/reopen
각 epoch를 artifact의 `child_object_io`에 보존한다. 이 결합의 최종 실행 판정은
VALIDATION을 따른다.

constructor·GC·Catalog/WAL은 해당 수집기의 명시적 제외 범위다. elapsed는
동시 호출의 합계라서 wall time을 대체하지 않는다. byte는 object-install의
쓰기량이며 매체 IO가 아니다. `written_bytes`는 실제 write 반환량을 합산하므로
실패 이전 short write도 포함한다. `reserve_objects`/`confirm_objects`는 도달한
호출의 entry 수라 replay/거절을 포함할 수 있으며 신규 설치 파일 수가 아니다.
Object 수집 창은 constructor 이후 observer
연결부터 실제 join 후까지이며, socket-ready→shutdown-signal인 OS sampler 창과
다르다. 두 값의 차이를 미계측 IO나 write amplification으로 해석하지 않는다.
기존 trace vocabulary에 없는 byte/custody를
무관한 recorder-output counter나 label에 넣지 않았다. 조기 timeout/signal에는
join 후 snapshot이 없어 missing으로 남는다. 두 번째 물리 counter/원장은 없다.

독립 search/delete/old-view/reopen gold 후 cold/one-file/delete/no-op/catch-up·mixed load를 비교한다. admission wait·setup·mandatory drain/GC·failure/refusal/timeout/cancel·관측 probe 비용과 누락을 포함한다. observer Off/Record는 같은 입력·host 조건으로 비교하고 경합이 있으면 비교 유효성을 닫지 않는다. 실제 Perfetto capture의 source/build binding·complete/footer/loss·aggregate parity·UI 확인 여부를 각각 판정한다.

유지보수 운영 수용은 sustained churn/held pin/space shortage, ENOSPC/short-write/fsync, aggregate RSS/temp disk·write amplification·bounded debt/drain을 포함한다. 조건부 코드 수리는 측정 원인/기존 owner/독립 oracle가 확정된 경우만 편입한다. legacy LQ5/정식 성능/trace 의무도 동등 수용/명시적 퇴역 전까지 유지한다.

## Mixed-load integration boundary

기존 `open_loop.rs`의 arrival schedule·bounded queue·예정시각·실패분모를
library `offered_load.rs`로 추출하고 기존 Legacy caller도 같은 구현을 소비하도록
연결했다. 고정/seeded-Poisson arrival,100K 발행 상한, 취소 시 최대50ms 단위
대기와 모든 worker join, 정상 반환의 offered=completion+queue-full+late 보존식을
유지한다. 원 Legacy Config의60초 제한은 유지한다. 새 소스의 strict·회귀 판정은
VALIDATION을 따르며 구현만으로 검증됐다고 판정하지 않는다.

Native adapter는 `QUANTA_INDEX_NATIVE_MIXED_LOAD_V1=1`로만 선택한다. Large,
seed42, 위에 선언한 정확한 explicit4축 funding, resource-sampling=1과 Off를
요구하며 full ACK·기존 기능 gold가 성공한 뒤에만 실행한다.60초 warmup과900초
측정은50QPS·4worker·queue64·원 예정시각부터2초 query deadline을 사용한다.
단일 publisher의1초 주기 replace/delete/restore는 기존 same-key publication
helper와 operation/event sequence를 재사용한다. 요청 취소·실패·queue drop·late
발행을 성공률 분모에서 빼지 않으며 중단 결과는 부분 진행으로 보존한다.
source-stage/ACK/current-gold freshness와 old-view witness는 별도 관측한다.
최종 ACK head·first-owner witness를 기존 재오픈 gold로 전달한다. IPC 동안
snapshot mutex를 보유하지 않는다. 실제15분 실행/성능 수용 여부는 별도 결과다.

Full Record 혼합부하는 현재 선언된 bounded capture 보존 profile이 없어
선택을 거절한다. 기존64MiB trace 상한을 이 실행의 통과를 위해 늘리지 않는다.
정식 Off/Record Large 교대 비교와 source/build trace qualification은 별도로
남아 있다. v29 Linux strict·bench test build와 영향63개(scheduler5,
Native29, Legacy open-loop20, scale CLI9)는 통과했다. 여기에는 broad 검색의
독립 top20 rank/binding/partial 거절과 새 adapter fixed gold가 포함된다.
제품3개는 ignored이며 실제 Large/XL·15분 부하는 이 결과에 포함되지 않는다.
정확한 source와 실패/수리 이력은 [v29 검증](../../../plans/oct-10-index-closeout/VALIDATION.md#w2-09-v29-consolidated-harness-gate--2026-10-18)을 따른다.

공개 Native `SearchText`는 한 요청 동안 ticket으로 view를 보호한다. SDK/UDS에
장기 read-pin Begin/Hold/Release 계약은 없다. 따라서 현재 product client만으로
10분 실제 pin 수용은 `BLOCKED`다. `ViewRef` 보관이나 old-view 검색 성공을 pin
증거로 승격하지 않는다. 측정 전용 authority API나 미연결 통계 helper는 추가하지
않았다. 기존 Kernel 장기 pin/fault 회귀는 해당 component 범위로 유지한다.

## Remaining caller handoff boundaries

| Caller / actual scope | Minimum integration | Required owner / evidence limit |
| --- | --- | --- |
| `kernel/update.rs::submit_authorized` metadata/shape/encoding preparation | 기존 `WorkBudget.request().diagnostic_context()`와 `diagnostic::begin/finish`로 `BuildPreflight`; object install 전후 경계를 구분 | Kernel 공용 writer; 정의만 추가하지 않고 actual error/terminal과 결합. ObjectIo elapsed와 합산하지 않음 |
| 동일 caller의 `catalog.seal_inputs`/`admit` | install 뒤 actual Catalog 호출만 기존 `SqliteWork` stage로 관측 | Kernel/Catalog writer; physical install까지 SQL-only 비용으로 이름 붙이지 않음 |
| Catalog `commit_transition` | 이미 상위 `NativePublish` 있음. finer span은 lock/transaction/work-cursor/object-read/commit의 실제 포함 범위 명시 | Catalog writer; 별도 SQLite/device IO counter로 해석하지 않음. context를 child로 전달하지 않았다면 sibling을 nested로 주장하지 않음 |
| Native Pump WAL checkpoint·recovery·reclaim | 기존 pump budget에 capture context/lease를 연결하고 start→stop/join 수명 결합 | Runtime/Pump 공용 writer; 현재 server context만 연결돼 있어 Storage private hunk 추가만으로는 귀속되지 않음 |
| scratch constructor/orphan cleanup·physical GC | boot 또는 pump의 실제 budget/context를 소비 | Runtime/Storage writer; 기존 ObjectIo collector의 명시적 제외 범위 유지 |
| Semantica capture/analysis·async fanout | 실제 producer/COORD caller가 기존 trace context와 원 operation identity를 제공 | 기존 W2-05/INT producer writer; Index-only fixture·복제 serializer로 완료 처리하지 않음 |

이 표는 읽기 전용 source 감사에 따른 다음 수정 경계이며, 공용 hunk의 소유권
인수나 구현/실행 완료 기록이 아니다. source/build-bound raw capture 결합과
전체 aggregate parity,10분 public held pin은 위의 독립 수용 범위를 유지한다.

## Mutation staging physical-cost handoff — 2026-10-18

ROOT diagnostic c identifies mutation staging1536/4096 (twelfth128-owner page),
original single SDK30s budget, operation2 still Unknown before SourceAdvance.
This identifies the caller and boundary; it does not identify the dominant CPU
or IO component. PHYSICAL audited only the downstream physical/Catalog scope;
ROOT retains upstream COW/edit-page producer ownership. All findings below are
static; no new runtime success or estimated speedup is implied.

- Actual path: `kernel/update.rs::stage_mutation_page` → `IngressSink` →
  Map child-first groups (64 unique objects/64MiB) →
  `FileObjectStore::install_prepared_batch` → Ingress
  `reserve_managed`/physical install/`confirm_managed`.
  Attempt `reserve_attempt/confirm_attempt` is not this pre-submit path.
- Storage: reserve precedes physical installation; confirm follows file sync,
  deduplicated shard-directory sync, objects-root sync, platform barrier and
  `ticket.synced`. Referenced shard directories already use a BTreeSet and
  sync once per batch; macOS fullfsync is once via a representative regular
  file. No safely removable fsync/barrier was established. Replay verifies
  stored wire hash and exact expected wire, then retains its required durability
  boundary. Duplicate-input preparation/hash and replay hash+byte comparison
  overlap in CPU work, but removing them requires a separate authentication
  contract; neither explains the1536-owner failure from present evidence.
- Catalog: `managed_install.rs::confirm_ingress` authenticates each external
  child in `require_child_charged`; the same transaction then calls
  `register_confirmed` → `register` (`authority.rs`), which calls
  `require_object` and reads that child's payload/digest/full edge ledger again.
  This same-transaction repeated external-child scan is source-proven. Existing
  `earlier` map already avoids one scan for earlier in-batch children while
  retaining work charge. The v25 provisional-scan repair remains intact and
  must not be reimplemented.
- Lower-risk SQL candidate: repeated fixed query/insert statements in
  `ingress_intent`, provisional object/edge reservation, ingress intent,
  registered object/edge insertion and ingress-confirmed lookup/insert still
  use `Connection::query_row/execute` and therefore prepare repeatedly.
  Shape/payload/edge reads already use existing cached statements. Reuse only
  the existing connection statement cache; retain identical SQL, parameters,
  result/count checks, transaction boundary, authentication and work charges.
  No separate application cache or new ledger is needed. Actual prepare cost
  and speedup remain unmeasured.
- A same-Tx child-scan repair needs an explicit preflight/registration contract
  limited to this authenticated batch. Do not skip validation in the generic
  `register` path or trust a ref-only caller set. Reserve commits independently;
  failed confirm must roll back all its registrations/transfers and retain
  previously durable provisional custody. No backing or lease charge is removed.
- Existing decisive Catalog gold: `confirmed_batch_preflight_rejects_later_provisional_corruption_atomically`,
  `confirmed_batch_transfer_preserves_duplicate_replay_and_durable_edges`,
  `managed_attempt_child_preflight_preserves_integrity_atomicity_and_reopen`,
  `later_confirmation_failure_rolls_back_earlier_registration_and_transfer`,
  `registered_metadata_match_still_checks_bounds_and_edge_ledger`,
  `corrupt_registered_child_edge_shape_refuses_new_reservation`,
  `gc_managed_ingress_batch_confirms_leaf_before_parent_in_one_transaction`.
  If ref-keyed validation reuse is introduced, add one independent gold for two
  valid-shape conflicting metadata values under one ref in the same batch:
  atomic rejection, no earlier registration, unchanged provisional rows.
- Existing Storage gold: `observed_install_batch_counts_real_wire_syncs_and_deduplicated_replay`,
  `object_batch_deduplicates_identical_wire_and_confirms_once`,
  `no_replace_publication_is_single_link_before_directory_sync`,
  `batch_barrier_failure_leaves_reserved_objects_unconfirmed` (macOS),
  `existing_batch_uses_regular_file_barrier_and_rejects_wrong_device` (macOS),
  `cancellation_during_reservation_prevents_physical_installation`,
  `reservation_precedes_external_child_physical_lookup`, and
  `internal_batch_limits_reject_before_reservation_or_physical_install`.
  Component offline ENOSPC/FUSE receipts are not silently promoted to generic
  FileObjectStore short-write/fsync injection proof.

These were the initial read-only findings, before implementation handoff.
The selected same-Tx child proof and fixed registration/provisional SQL reuse
are implemented under the current managed confirmation contract below. The
later private reachable-map aggregation has its separate contract below; no
Storage sync/barrier has been removed. Source-bound outcomes are owned by
VALIDATION and the README row. The historical v29 baseline remains distinct
from v32/v35 code and is not current-source performance evidence.

### Private reachable-map aggregation contract

The adapter input count remains64, and map-root count remains64. Private
prepared installation groups accept at most256 unique child-first objects,
matching the existing Catalog managed boundary. This count is independent of
the unchanged64MiB aggregate wire cap. BatchReferences, RegistrationBatch and
the bounded gate/ticket tables cover that internal group; public BatchIndex
and input preparation retain their original64 limit. No extra coordinator,
request, replay, allowance or cache is introduced. Kernel staging batch64
remains unchanged; the earlier128 producer proposal is excluded.

A group still reserves the exact complete batch before physical writes, syncs
every new file and referenced shard/root, performs the platform barrier, settles
write tickets, then confirms the exact complete batch. Map traversal remains
deduplicated child-first. Temporary wire, metadata and simultaneous old/new
MapBuffer backing require original memory admission; peak memory and ticket/FD
retention may rise within the same authority and can refuse without increasing
limits. A later failure returns no new root and preserves existing durable
provisional custody for recovery. Larger grouping does not establish speedup.

Fixed gold includes256 unique chain files with cross-former64 child edges and
reopen,257 private/public65 pre-custody refusal,512 distinct map keys spanning
multiple groups and a second-group reservation failure, plus Catalog256
confirmation/reopen and last-object corruption before any registration/transfer.
Platform-conditional barrier and actual Runtime capacity results remain distinct
verification scopes. `writer.retire()` closes the original writer FD but each
pending WriteTicket retains its independent FileMutation pin until settlement;
a full256 group can retain roughly256 such FDs plus directory anchors. The
reader-handle64 policy does not govern those writer pins. Original pending/file/
byte/inode limits still reserve sequentially and can refuse a late object;
no batch-wide physical pregrant or increased FD limit is assumed. On failure,
checked close and pending published-prefix custody remain with the original
FileMutation owner. Actual low-FD/aggregate capacity acceptance is separate.
Current executed results are recorded only in VALIDATION.

### Actual staging stack follow-up

The one unchanged-source diagnostic f sampled the owned daemon PID1644 with
macOS `sample` every10ms. Actual `stage_mutation_page` ancestry reaches Storage
`apply_stream`→reachable chunk→prepared physical install→Catalog managed confirm.
Its subtree contains Transaction commit, SQLite WAL frame/checkpoint writes and
OS sync; Storage's platform fullfsync also appears. These observations do not
authorize dropping a durability barrier or treating aggregate stack counts as
exclusive wall time. Scratch mark/edit writes, limit checks and cleanup/retirement
are also present under the same stage.

The GC fixed-SQL candidate below is now seen in actual staging stacks:
`reclaim::read_row`, `queued` and `bound_namespace` call
`Connection::query_row`→prepare→`sqlite3_prepare_v3`; managed confirmation also
reaches adopt/is_bound/dequeue and reserve/confirm lifecycle updates. This
corroborates the reachable source path, not its dominant time or a predicted
speedup. Keep every row shape/digest/namespace authentication and physical sync.
Raw diagnostic files are `/tmp/quanta-ann-staging-daemon-20261018-f.sample.txt`
and `/tmp/quanta-ann-staging-profile-20261018-f.log`; source-bound results and
next source decision remain in VALIDATION/README. No new source change follows
from this read-only attribution.

### Residual lifecycle SQL preparation candidate

The post-v32 read-only audit follows `reserve_ingress` into
`reclaim::reserve_object` / `enqueue` and per-child `adopt`. Their lifecycle
phase/row and queue checks (`mutable_live_row`, `read_row`, `queued`) plus
`insert_row`/`update_row`/queue insertion still use uncached fixed statements.
A narrowly scoped reuse of the existing SQL helpers can retain exact SQL,
parameters, digest/shape/phase checks, checked row counts and call ordering.
No row/namespace authentication may be skipped: reserve followed by enqueue
re-reads lifecycle state, while adopt authenticates namespace binding per child.
Those repeated checks have not been proven removable. No connection cache
capacity setting was found; LRU reuse and actual speedup remain unmeasured,
so broad conversion of unrelated reclaim SQL is not justified.

This is independent of the already implemented v32 registration/provisional
SQL preparation and same-Tx child proof. No new source hunk or execution is
introduced by this audit. The immutable v32 Text replay remains unchanged.

### Current managed confirmation contract

`PreflightedRegistrationChildren<'tx, 'conn>` has private fields and borrows
the actual `rusqlite::Transaction<'conn>` and exact immutable registration batch.
Only the charged managed child preflight constructs it; ingress/attempt callers
consume it immediately in `register_confirmed_preflighted_children`. The shared
orchestrator checks the complete provisional payload/edge batch before any
registration or custody transfer. Generic `register_confirmed` still performs
its own child authentication. The proof is neither cloned nor stored, and no
validation result survives a transaction or a call.

Fixed SQL preparation uses the existing connection statement cache only; SQL,
parameters, checked row counts, original logical charges and transaction/physical
durability boundaries remain unchanged. Independent gold observes conflicting
same-ref metadata rejection inside the still-open transaction, plus external
child corruption after reserve and again after reopening a fresh Catalog.
The source-bound executed outcomes belong in VALIDATION and the README row.

### Large deadline follow-up: remaining lexical source candidate

After v32 Catalog repair, a read-only source audit found another cost candidate:
`apply_postings` computes each term's cardinality with a full scratch SQL aggregate
(`text_spool.rs::posting_cardinality`), then `PostingCursor::next_edit` reads the
same term rows with cached keyset `SELECT ... ORDER BY k LIMIT 1` once per edit.
This is a source-proven second scan, not a measured dominant phase. The current
cardinality pass validates df overflow/underflow **before** COW installation;
simply moving it after the cursor would change failure/provisional-install
ordering. A bounded cursor page is a separate candidate requiring admitted
backing, unchanged key/CAS ordering and add/remove/duplicate/corruption/rollback
gold. No lexical source change, budget increase or speedup claim follows.

The v30 first denial was deadline, not a work-charge axis; raw reserve/confirm
and file-sync elapsed counters overlap in scope and cannot be summed into an
exclusive140.688s wall decomposition. The v32 Catalog result qualifies integrity
only. ROOT retains lexical/public-caller ownership; this candidate is deferred
until the already prepared same-source functional replay establishes the residual
failure. No alternate cursor or new scratch authority has been implemented.

## Implementation packet

- **즉시 정리:** 기존 harness 담당의 source-bound gold/strict와 후속 actual 실행을 소비한다. 같은 failure/source의 Large/XL을 중복 제출하지 않는다.
- **정적 구현:** 미계측 producer/daemon phase를 actual caller·시계·work/IO counter·scope/종료 상태로 열거하고 각 source owner가 기존 trace context에 연결한다. 공용 Runtime/producer body 변경은 그 owner에게 exact hunk로 인계한다.
- **정적 완료:** submit1·원 키/원 publication deadline·failure prefix, harness/child 시계 분리·capture source/build binding·drop/loss/footer·OS IO 표본 범위를 확인한다. funding/sampler/기존26 phase는 재사용한다.
- **반환물:** 실제 추가 계측 위치·측정 가능한/없는 비용·선정 functional gold·동결 profile 입력과 source 변화. 통합자가 기능→Small parity→명시 Large/XL→quiet-host Off/Record/지속 부하 순서의 필요한 배치만 제출한다.
- **조건부 수리:** 지속 부하가 증명한 원인만 기존 유지보수 정책에 반영한다. host 경합이나 logical refusal만으로 scheduler·physical reserve 전체를 새로 만들지 않는다.


### Frozen minimal capacity repair / scratch candidate boundary — 2026-10-18

The exact applied production hunk removes three immediately repeated prefix
root checks in FileMutation::reserve_end/creation_started/mutation_started.
validate_held_allowance retains the fresh original descriptor-bound check as
its first operation. Separate reserve/start, anchored traversal and later IO
boundaries retain their checks. The new private VFS root_identity tests exercise
actual pending SQLite writes after root replacement and same-inode lock
hardlink/mode tamper, preserve bytes/ledger, and demand recovery after restoring
the original identity. Source is frozen for ROOT's consolidated strict and
lease/VFS/capacity group; compilation and runtime remain NOT_RUN.

The disposable MapScratch transaction audit is complete and read-only. Current
journal_mode OFF means any failed transaction must discard the connection;
rollback cannot establish safe reuse. Existing per-write budget/reservation and
hard-page checks must remain. BEGIN/COMMIT need short individual scoped guards,
while each existing SQL operation keeps its own VFS scope. A whole apply_stream
transaction committing after external object installation introduces a new late
failure/orphan-object boundary. Committing edit+node preparation before external
installation is narrower but does not coalesce later marks. cache_size=-2048 is
an approximate cache target, not an RSS bound; spills prevent a guaranteed
performance benefit. ROOT selected the audited transaction hunk as the next
candidate only after the current impact group terminates and any regression is
fixed. The exact implementation and actual COMMIT/xWrite-fault gold plan is
ready in VALIDATION; no transaction source edit or extra product run has begun.
Current source remains frozen for the owning consolidated check.


### Applied scratch transaction packet — 2026-10-18

ROOT's completed n/g group passed production-lib strict and108 lease/VFS/capacity
cases, including the new6 security cases. After freeze release, the audited
Storage transaction helpers and pipeline/group boundaries were applied together
with actual SQLite COMMIT/xWrite-failure gold. The existing reference-array
refusal test now observes the commit callback ordering. ROOT integrated the
existing vendor's Unix test-only feature and its single lock dependency edge.
The packet is frozen; scoped format/test-authority/whitespace checks pass.
Compilation and transaction behavior are NOT_RUN until ROOT's single gate.
Actual helper/group IO refusal does not stand in for full apply_stream fault
injection or managed aggregate-capacity proof. Keep the original ANN/Text
deadline failures and Large/XL/mixed qualification gaps open.


### Applied retirement hint and fixed SQL reuse - 2026-10-18

After j's terminal229/228passed1failed, ROOT selected two further bounded source
repairs. The same Tracking mutex now guards a conservative tombstone hint: all7
creation sites set it, and only an exhausted scan proving no tombstone clears it.
Pinned/releasing tombstones and close-error priority are preserved. Four existing
charge/last-pin golds start from an empty sweep, with two new focused error/state
golds. Scratch's existing statement cache now reuses9 calls/8 fixed SQL shapes
without changing SQL, cache capacity, logical charges, reservation order, memory
bounds, transaction/discard or VFS scope. The combined packet is ready/frozen;
static checks pass and ROOT owns one combined strict/gold execution. New runtime
behavior is NOT_RUN. Prior component green does not close these new changes or
the remaining4K/Text deadline failures.


### Post-l read-only staging cost packet - 2026-10-18

ROOT's original l session91306 terminated100:26run/25passed/1failed/58skipped,
195.057s, build1m33s, admission hold317.588330333s; runID
861b84e5-7f7b-4ef5-92c7-e50c807456bd. The sole4K failure exhausted the original
30s during staging at ordinal2815, original sequence2 Unknown. No SourceAdvance,
recall or restart was reached. Failure-prefix ordinals from different sources
are neither durable ACK totals nor a qualified speedup. q strict and k227/227
remain their actual component scopes. ROOT owns the current official
just rust-profile test-daemon m; no additional PHYSICAL build/product replay.

Read-only install audit: map private256/64MiB child-first groups perform reference
array admission, scratch COMMIT, then one install_prepared_batch. Catalog reserve
and confirm each use one write transaction with actual file IO between them;
they cannot be merged safely. For new objects, file sync is once per object,
objects-directory sync once per group and shard sync once per unique shard,
with one macOS batch FULLFSYNC barrier. Gate sort, BatchReferences binary search
and Catalog earlier-child sets do not establish an O(N-squared) batch rewalk.
NativeObjectWriteIo still reserves before exclusive creation, retains independent
FD custody through the store barrier, settles without another file/parent fsync,
and retires the pin. Adjacent target observation/begin/create, namespace traversal,
physical IO and settlement checks cross race/custody boundaries; no further
removable same-call physical check has been established.

Shared scratch admission is a distinct source candidate: lib.rs pragma calls
Connection::query_row for four fixed read SQL shapes (main.page_size,
main.max_page_count, main.page_count, main.freelist_count). A normal no-growth
reserve_for_write makes four preparations per row. Existing MapScratch statement
reuse does not cover this shared helper or TextSpool's separate connection.
Minimal design: reuse existing prepare_cached only for fixed reads; execute the
dynamic main.max_page_count={pages} setter uncached to avoid one cache key per
reservation. Re-read values each invocation; keep every existing checkpoint,
reserved/max-byte check, reserve_at_least-before-cap order, conversion/error,
hard page cap and caller VFS scope. No cache capacity change or value cache.
No source edit, compile, behavior or speedup claim follows this static packet.
Relevant independent gold remains denied growth/shared reservation, failed
page-size reservation retention, bounded hard cap and actual VFS capacity/close.

apply_stream phase1 separately traverses each base key before inserting its
scratch edit; phase2 already shares the open path. Deferring CAS to phase2 would
reorder an early Conflict versus a later malformed/erroring cursor and is not an
accepted repair. A held FD pins an inode but alone does not freeze bytes or grant
a cross-callback current-name authentication exemption. Reuse requires an exact
existing contract/custody proof and preserved budget/error order; no new cache,
IR or pipeline has been applied. ROOT owns that decision and shared integration.


### Optimized same-oracle profile boundary - 2026-10-18

The l log explicitly reports Cargo test profile [unoptimized + debuginfo].
The30s refusal is a real failure of that functional fixture; it is not a
qualified production throughput result. native_product_process.rs budget()
supplies the30s RequestBudgetV1, and ann_quality stage_quality_sources creates
one mutation_budget for the complete4096-owner stage_mutations iterator.
QUALITY_WATCHDOG1200s bounds the outer process only; configured owned work stays
at10s. Native socket admission separately has120s dispatch and30s IO timeout;
those are not an end-to-end4096-owner SLO. The W2-09 acceptance proposal already
requires optimized bench binaries and does not claim the fixture deadline as a
measured production SLO. No budget, deadline, worker or threshold change follows.

The existing exact native_product_process ANN test can be selected using the
existing Cargo front door with --profile bench (or separately --release).
Daemon::start_with_api_key uses CARGO_BIN_EXE_quanta-index-searchd, so Cargo builds
the child daemon in the same selected profile rather than borrowing a debug
binary. bench inherits release optimization, disables LTO and uses16 codegen
units; it is distinct from deployment release. Exact unchanged oracle remains
4096x64,64queries,K10,recall>=.95, real UDS/SDK/source-owner checks, exact scores,
coverage/traversal and reopen parity. Source has no debug_assertions gate on this
test. Example proposed command, NOT_RUN and not queued by PHYSICAL:

    ./scripts/cargow --lane bench-lane test -p quanta-index-searchd-runtime --test native_product_process --all-features --locked --offline --profile bench ann_quality::product_native_ann_quality_4096x64_reopens_at_fixed_recall -- --exact --nocapture

No new runner or relaxed threshold is needed to express the same oracle.
Optimized build success,30s completion, recall/reopen and qualification are
NOT_RUN until ROOT selects and executes a source-bound batch after m terminates.
The existing ann_matrix directly uses SemanticAdapter and cannot substitute
for product UDS/staging/owner/custody/reopen proof. release-daemon alone builds
only the binary and does not execute this oracle. Old v35 Linux optimized
artifacts do not establish current-source behavior.


### Prefix reuse contract audit conclusion - 2026-10-18

No safe cross-callback decoded-prefix reuse is established by current custody.
apply_stream calls external MapEditCursor::next_edit before each original CAS.
FileObjectStore::read_wire freshly validates root/namespace/shard/current name,
opened inode, complete wire/EOF and digest on every lookup. ObjectReadFile retains
actual FD/pin custody but freezes neither file bytes nor the current name. A
content-addressed reference authenticates bytes when verified; it does not grant
a later callback an exemption from current-name/byte verification. Caching across
next_edit could suppress existing corruption/absence/storage errors or change
Conflict order. Keep original read/bytes charges and admitted memory lifetime.
Only reusing decode work after identical fresh physical/byte checks could be
assessed locally; it saves no IO and is not recommended as a staging repair.
Existing late_existing_root_conflict_preserves_all_original_roots and
late_stream_conflict_or_duplicate_never_reserves_a_map_object cover late CAS and
no publication prefix. No independent existing gold was found that replaces or
corrupts a base node inside an adversarial cursor between two edits. This is a
gold gap for a proposed reuse optimization, not proof of a current product bug.
No source changes or runtime were performed for this audit.


## Native Source phase packet — 2026-10-18

Reuse the original Kernel producer and explicit request diagnostic context:

| Actual source boundary | Closed trace stage | Window |
| --- | --- | --- |
| `source::apply_source` | `native_source_apply` | Authority/frontier/owner/UTF-8/path preflight through returned SourceDelta; early failure closes as error |
| `source::prepare_path_values` | `native_source_path_install` | Native17 fixed inline owner values and bounded scratch reference updates; no separate marker object installs |
| Final frontier stage plus `AttemptContext::apply_stream` | `native_source_map_apply` | Original owner/path/frontier replay, staged map construction and child-first managed installation |

The existing closed stage key `native_source_path_install` is retained; in Native17 its window prepares data-only path values in scratch, and physical path-map installation occurs in `native_source_map_apply`. It must not be interpreted as per-owner marker-object IO. The trace wrapper delegates to one original producer body; there is no alternate indexing path or producer IR. Off obtains no diagnostic context and starts no trace clock/span; no work limits, cancellation/deadline authority or usage are replaced. Existing parent context is reused without fabricating nested parent/request/flow identity. The recorder stage whitelist and canonical timeline schema must agree with the Core enum. A bounded fixture on the existing real Kernel/managed Catalog/FileObjectStore checks Off/Record source-cut and WorkUsage parity, zero Off diagnostic-clock calls, successful path/map spans and an error-only apply span on path-collision preflight. This is functional/cost-accounting parity, not the required Large3-pair overhead qualification. Recording an error with a complete footer is complete diagnostic evidence of failure, not product success. Status and executions remain only in README/VALIDATION.
