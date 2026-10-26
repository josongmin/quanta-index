> HISTORICAL — pre-migration document from `docs/plans/oct-17-index-closeout/tickets/W2-09-cost-observability.md`. Current work: [Oct-22 SSOT](../../../../plans/oct-22-finalizing/README.md). Old TODO/status/counts do not create work or qualify current source.

# W2-09 — 실제 phase·총비용·지속 운영 수용

상태·담당·다음 완료 조건은 [SSOT](README.md#w2-09)만 갱신한다. 이 문서는 고정 수용 계약이며 실행 이력은 [VALIDATION](../../oct-10-index-closeout/VALIDATION.md), 과거 후보·버전별 논의는 [이력](../../../oct-17-index-closeout/tickets/W2-09-cost-observability.md)에 있다.

## Remaining scope

1. W2-13의 실제 Native18 Runtime/원4K 기능·품질 결과를 먼저 소비한다. 제품이 full ACK에 실패한 상태에서 정식 성능 실행을 제출하지 않는다.
2. 기존 harness로 current-source Large/XL의 ACK/search/reopen을 검증하고, 아래 profile의 Off/Record·지속부하·RSS/temp/IO·debt/drain을 각각 판정한다.
3. 실제 phase·원 authority·측정 창·operation/source/build/capture·clock을 연결한다. definition/span 이름만으로 producer→daemon 전체 비용을 관측했다고 주장하지 않는다.
4. 정확한 fault·공간 압박·last-pin/FD/custody·shutdown을 기존 oracle로 수용한다. 유지보수 정책 수리는 실제 측정·결함이 확인된 범위만 수행한다.

## Reuse and proof limits

기존 foreground funding, Linux aggregate RSS/process IO·allocated-block sampler, trace/footer/producer lease, offered-load/mixed-load adapter, same-key publication observer와 object IO collector를 재사용한다.

- RSS는 PID/start-time으로 묶인 harness/daemon/관측 child 합이다. missing은0이 아니며 shared pages가 중복될 수 있다. 표본 최대는 continuous peak가 아니다.
- disk는 실제 allocated blocks와 inode별 범위다. fixture/state/scratch·하드링크 중복 제거·관측 창과 fixture 밖 임시 파일의 미관측 한계를 명시한다.
- object IO collector는 write 반환량/실패 prefix와 reserve/confirm·file/directory/barrier의 도달 호출/elapsed를 수집한다. transaction·constructor·WAL/GC 전체를 포함하지 않는다. 겹치는 elapsed 합은 phase wall time이 아니다.
- harness/child는 서로 다른 capture와 clock이다. footer 완결성·UI 표시·aggregate parity·source/build binding·qualified overhead는 별도 수용이다.
- 공개 단발 SearchText의 ticket은 요청 수명이다. 장기 pin 증거는 아래 제품 ReadSession의 실제 Open·Renew·held Query·confirmed Release로 생산한다. ViewRef 보관·단발 old-view 검색으로10분 custody를 추정하지 않는다.

## Inputs, ownership and acceptance

현재 source/입력/profile/동일 binary와 host·filesystem·가능 공간을 먼저 결합한다. 각 원 producer/daemon writer가 actual caller 계측을 소유하고 공용 parent는 통합자가 수정한다. W2-05의 실제 producer 비용과 Index-only 비용은 범위를 구분한다. 기능 통과·qualified timing·운영·release를 합치지 않는다.


## 정식 운영 수용의 authority 연결

- Resource high-water는 startup·drain·종료 child까지 포함하는 완결된 harness/daemon/worker 수명에서 기존 process/admission owner가 생산해야 한다. 원 process identity와 관측 창을 결합한다. 주기적 VmRSS·비원자적 descendant census·논리 capacity·불완전한 peak 합은 이 증거를 대신하지 않는다. 미지원/부분 상태를 보존하고 새 metric의 생산자/소비자를 함께 전환한다. 두 번째 collector를 추가하거나 다른 메모리 metric을 aggregate process RSS로 표시하지 않는다.
- 실행 수명 계측: Native capture의 `--linux-cgroup-parent /absolute/delegated/parent` opt-in은 기존 `linux_process.run(qualified=True)`를 단일 실행 소유자로 사용한다. workload pre-exec gate부터 cgroup empty/reap까지 관측하며 원 absolute deadline·raw output·실패 기록·source/binary binding을 유지한다. 미지원 OS/불완전 ownership/parent identity 변경은 실패하고 기본 process-group 경로로 fallback하지 않는다. `execution.json.linux_resource_observation`은 sampled aggregate RSS와 kernel-accounted cgroup `memory.peak`를 구분한다. cgroup migration 외부 차단이 필요하며 실제 Linux 실행 수용은 별도다.
- 잔여 high-water 경계: 내부 Native sampler는 여전히 UDS ready 뒤~TERM 전 구간이다.500ms disk traversal·비원자적 `/proc` census는 sampled diagnostic이며 true aggregate RSS/allocated-disk peak 증거가 아니다. 위 cgroup memory 값을 aggregate RSS로 alias하지 않으며 allocated-disk peak는 null이다. 전체 roots의 실제 할당 authority와 정식 운영 수용은 남는다.
- 마지막 pin 해제는 실제 `CatalogReadPort::release_ticket`/owned read-session 사건이어야 하며 필요한 renewal도 포함한다. ViewRef·완료된 old-view 검색·dead-session recovery count만으로 대체하지 않는다. 기존 pin authority를 재사용하며 측정 전용 pin API는 이 packet의 범위가 아니다.
- 기존 `NativeRetentionConfig`/`NativeRetentionDriver` owner를 통해 retirement scope와 정책을 명시한다. 실제 old-view pin custody가 없는 상태에서 retirement만 켜 기존 gold를 무효화하지 않는다. 정당하게 보존하는 history와 reclaimable debt를 구분한다.
- 동일 Catalog/state-root authority가 eligible exact-history·terminal-work·custody retirement, live/read-session release, durable object GC work와 physical settlement를 함께 확인해야 debt0을 판정할 수 있다. `CatalogReclaimPort::reclaim_work_page`는 객체 큐 단계만 다룬다. `NativeReclaimDriver`의 forward exhaustion·빈 head page는 다른 영역이나 concurrent arrival의 부재를 인증하지 않으며, 물리 FD/writer quiescence도 논리 debt0이 아니다.
- 소비자는 원 release time과 같은 authority가 인증한 완결 debt 상태를 호환 clock/window에서 받은 뒤에만 기존60초 convergence 목표를 판정한다. 입력 부재는 미수용으로 남긴다. synthetic release·queue-empty shortcut·새 ledger·무제한 전역 scan으로 성공을 만들지 않는다.

## 제품 장기 read-session·retention 계약

2026-10-19 사용자가 제품 계약 설계 포함을 승인했다. 아래 제품 계약은 기존 Catalog/Kernel lifetime을 재사용하며 측정용 pin authority를 추가하지 않는다. 구현·검증의 최신 상태는 SSOT에서만 판정한다. 코드 존재는 운영 수용 증거가 아니다.

### Read-session 소유와 API

- daemon은 실제 `Kernel::pin_view`의 `ReadTicket`과 확정된 `CatalogView`를 보유한다. UDS handle은 그 ticket의 transport reference이며 새로운 pin·publication authority가 아니다. 클라이언트가 Catalog ticket/session/fence를 생성하거나 전달하지 않는다.
- 현재 SDK는 호출마다 UDS 연결을 연다. socket 연결 종료는 해당 요청의 cancellation이며 장기 session 종료가 아니다. 32바이트 entropy handle을 accept 시점의 인증된 UID·daemon instance/state-root lease에 묶는다. PID가 없는 플랫폼에서도 같은 계약을 유지하며, 다른 UID·이전 daemon handle·credential 없는 경로는 거절한다. 초기 제어는 기존 operator authorization을 재사용한다.
- 공개 SDK 작업은 `open_read_session`, `renew_read_session`, `read_session_status`, `search_text_in_read_session`, `release_read_session`, `read_session_settlement`다. wire는 기존 `NativeIndexRequest::ReadSession`과 `NativeIndexResponse::ReadSession`의 typed request/reply를 사용한다. Release는 확정된 해제 관측을 반환하고 Settlement는 이후 Catalog snapshot을 별도로 관측한다. 기존 단발 SearchText 계약은 유지한다.
- Open은 admission·bounded registry slot을 먼저 확보하고 같은 Catalog authority에서 view를 확정·pin한 뒤 handle/view/expiry/revision을 반환한다. 같은 UID/open_key의 동일 입력 재시도는 동일 결과, 다른 입력은 Conflict다. ACK 유실로 남은 entry도 capacity에 포함한다.
- per-daemon/per-UID handle 수·memory/FD 비용·terminal reply 보관량을 명시적으로 제한한다. Active·Closing·Unknown·ACK 재시도용 terminal entry를 모두 상한에 포함하고 자리를 만들기 위해 live pin을 제거하지 않는다. slot 부족은 typed refusal다.
- lease는 기존 `validate_read_ticket_lease`의1..=300000ms와 daemon 설정을 따른다. 10분은 같은 ticket을 만료 전에 갱신해서 충족한다. 만료 후 부활·Latest 재선택·새 ticket으로 바꿔치기는 금지한다. Renew는 revision과 직전 요청 fingerprint를 확인해 ACK 유실 재시도를 구분하며 Catalog fence는 서버 내부에 유지한다.

### Query·Release·장애 수명

- **held view를 Exact로 다시 pin하지 않는다.** Catalog의 Exact admission은 retention floor 아래를 거절한다. pin이 view row를 보존해도 재획득 권한은 없다. 기존 `PinnedQuerySession`의 profile/adapter/execute/result 경로가 원 ticket을 borrow하도록 확장하고, 각 Query는 새 요청 WorkBudget·deadline·cancel만 소유한다. query 완료가 장기 ticket을 release하지 않는다.
- per-entry 동기화로 Query의 ticket 사용과 Renew/Release를 직렬화한다. lock 대기도 원 요청 deadline/cancel에 포함하고 무제한 대기하지 않는다. Release는 먼저 Closing으로 전환해 새 Query를 차단하고 기존 query guard가 끝난 뒤 Catalog 해제를 수행한다. 긴 Query가 renewal을 막아 expiry에 도달하면 그 Query도 원 validate/final-validation에서 실패하며 성공으로 기록하지 않는다.
- `ReadTicket::try_release(&mut self)`는 해제 실패 시 원 ticket/session을 보관한다. 성공 이후 중복 호출은 Catalog delete를 반복하지 않고 validate/renew는 거절한다. 기존 consuming release/finish/Drop의 원 오류 우선순위와 cleanup을 유지한다.
- Catalog DELETE 뒤 commit 오류는 **Unknown**일 수 있다. 오류나 SnapshotUnavailable을 성공으로 바꾸지 않는다. public Release의 confirmed ACK는 기존 Catalog owner의 같은 session/ticket/fence 검사·commit 결과에서만 생산한다. 불명확한 경우 bounded status/reconcile로 원 해제를 재시도한다. Unknown 뒤 동일 live session/ticket/fence 부재는 해당 owner가 유일한 remover임을 검증한 경우 pin 부재로 reconcile하되, 원 DELETE 커밋을 확인한 ACK와 구분한다. session death/restart 또는 다른 cleanup 경로가 개입하면 원 Release 결과는 Unknown으로 유지한다. 확인 전 entry를 성공 tombstone으로 바꾸거나 운영 convergence를 통과시키지 않는다. 새 durable receipt ledger는 추가하지 않는다.
- 확인된 Release 응답/tombstone만 ACK 재시도에 사용한다. bounded 보관 기간 뒤 알 수 없는 handle은 unknown/unavailable이며 과거 성공을 추정하지 않는다. 최초 Release 요청 직전의 monotonic 시각을 동일 process/clock에서 모든 재시도·reconcile에 유지하며60초 판정을 재기산하지 않는다. process/clock identity가 끊기면 해당 convergence 수용은 Incomplete다. 실제 last-pin 여부는 같은 Catalog transaction의 해당 view pin 존재 검사로 확인한다. 다른 reader가 남으면 last-pin release로 처리하지 않는다.
- client crash는 연결 종료만으로 판정하지 않는다. 갱신 중단→expiry→in-flight 종료 확인→기존 maintenance cadence에서 explicit release 순서다. expiry 자체는 reader가 객체 사용을 끝냈다는 증거가 아니다. daemon stop은 새 Begin/Query를 막고 query drain 후 session release, 기존 pump join·resource cleanup 순서를 지킨다. daemon crash는 기존 Catalog OS-lock/session-death recovery가 처리하며 token은 재시작 뒤 무효다.

### Retention·settlement

- 제품 설정에 기존 `NativeRetentionConfig`의 workspace/epoch·sequence window·page limit·interval·round budget·head probe를 명시한다. 기존 `NativeRetentionDriver`와 reclaim pump만 사용한다. policy floor는 단조 증가하고 held view는 pin으로 보호한다. 해제 대상 view가 floor 아래인지 먼저 확인한다. 보존 window/current/live work가 정당하게 보유한 데이터는 reclaimable debt에서 제외하되 physical allocation에는 포함한다.
- 같은 Catalog transaction에서 scope/head/floor와 eligible view·terminal work·terminal custody·session cleanup·Candidate/Reclaiming/Draining GC work·durable reference 없는 미큐잉 orphan의 존재를 판정하는 bounded read-only 상태를 추가한다. 기존 retirement/reference/digest 검증을 재사용한다. 범위 밖·손상·예산 초과·검사 미완결은 Error/Incomplete이며0이 아니다.
- 상태는 `Pending`, `Incomplete`, `SettledAtSnapshot`를 구분한다. Settled는 확인된 target pin 부재·floor 초과·위 모든 reclaimable predicate 부재를 **한 Catalog snapshot**에서 확인한 경우뿐이다. page별0 합산·forward cursor 끝·큐1개 조회로 완결성을 추정하지 않는다. bounded indexed predicate로 증명할 수 없으면 기존 Catalog schema/query를 보완하거나 Incomplete를 유지한다. 무제한 scan·별도 debt ledger로 우회하지 않는다.
- Release ACK의 잔여 pin 값은 해당 commit 당시 관측이며 재시도에도 그대로다. Settlement의 잔여 pin 값은 새 동일 Catalog snapshot에서 읽는다. 다른 reader가 나중에 해제됐을 때 과거 ACK의 `true`가 영구 Pending을 만들지 않는다.
- 동시 publication 중 Settled는 그 snapshot 시점의 논리 debt 상태다. 이후 변경의 무부채나 연속 resource peak를 보증하지 않는다. 소비자는 same authority/scope/release identity와 원 시계의60초 창 안에 이 witness를 요구하며, 이후 지속부하·최종 drain·cold reopen도 별도로 확인한다. FD/inode/scratch physical settlement는 기존 stop/join 뒤 physical oracle로 확인하고 Catalog zero와 합치지 않는다.

### 구현 의존관계와 독립 gold

아래는 제품 계약의 의존관계와 회귀 요구사항이며 새 작업 배정이 아니다. 현재 구현·검증 결과와 남은 실행은 [SSOT](README.md#w2-09)만 따른다.

1. Kernel: retry 가능한 release와 owned/borrowed query custody를 기존 session 경로에 결합한다.
2. 병렬 private body: Runtime bounded registry·expiry/drain, Catalog bounded release/settlement observation. Contract·dispatch·SDK·pump parent 합류는 부모/기존 writer가 직렬 처리한다.
3. producer→consumer 정적 확인 뒤 Kernel/registry/Catalog의 영향 회귀를 모아서 실행한다. 이후 SDK/UDS 장기 session→기존 mixed harness로 연결한다. 원4K·Large 기능 gate 전 운영 배치를 제출하지 않는다.
4. 고정 gold: floor 아래 held Query 성공과 새 Exact 거절, 같은 ticket renewal·expiry gap 거절, UID/daemon fencing, Open/Renew/Release ACK 유실, transient release 실패와 ambiguous commit 구분, query/release race, capacity refusal, expiry 중 active read, stop/crash recovery, 두 번째 pin, queue-empty-but-retireable-view·partial Draining·미큐잉 orphan·동시 publication·Incomplete budget.

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
  mixed harness는 제품 ReadSession으로 Open ACK 이후600초를 계측하고,
  300000ms lease를120초마다 갱신한다(최대 갱신 gap180초).
  offered query도 선택한 snapshot의 실제 ReadSession을 결과 검증까지 빌린다.
  publisher의 floor 전진 후 old ViewRef를 Exact로 다시 pin하지 않는다.
  snapshot 교체 후 마지막 borrower가 끝나야 원 handle을 해제하며,해제 실패는
  같은 handle로 보존한다. query의 snapshot lock 대기는 원 scheduled deadline·
  cancellation에 포함하고 모든 worker/publisher join 뒤 잔여 handle을 정리한다.
  최초 Release 시도부터60초 안의 same-scope SettledAtSnapshot을 요구한다.
  mixed에서만 retention window1·registry global/per-UID128·terminal TTL60초·
  sweep16을 명시한다. 이 선택은 원4K·일반 scale의30초 lease/요청 예산을
  변경하지 않는다. 실제 실행 전까지 이 연결은 운영 수용 증거가 아니다.

| Acceptance | Initial target | Boundary / failure rule |
| --- | --- | --- |
| Correctness / durability | 정상 부하의 fixture gold 전부 일치; accepted publication 유실0 | ACK 후 current/held-old view, delete/no-op, reopen/head, final drain 포함. refusal/Unknown을 성공·샘플 제외로 처리하지 않음. 의도된 fault/overload는 별도 oracle |
| Warm lexical query | p95≤100ms, p99≤500ms | client 예정 발행→decoded top20, 실패/timeout 포함. cold/reopen은 별도 기록. ANN/semantic은 W2-13의 recall/coverage gate를 추가로 충족해야 하며 lexical 결과로 대체하지 않음 |
| One-file freshness | p95≤1s, p99≤5s | source staging 시작→원 operation terminal ACK→같은 committed view에서 독립 검색 gold 확인. admission/queue/staging을 빼지 않음 |
| Resource bound | aggregate process RSS≤2GiB; allocated scratch≤4GiB; allocated physical≤64GiB | harness와 모든 daemon/worker child 포함. 100ms 표본 최대는 observed maximum으로만 기록; continuous peak는 기존 admission/OS high-water 증거가 있어야 판정. capacity denial/error·누락0 요구 |
| Maintenance convergence | release 후60s 이내 reclaimable debt0;5s shutdown grace 준수 | pin이 보호하는 객체는 debt0 대상에서 제외하되 union allocated에 포함. 마지막 pin 해제 후 보호가 사라진 객체·FD/lease·scratch 정리와 durable reopen을 원 oracle로 확인 |

시간 목표와 무관하게 cancellation·ENOSPC/short-write/fsync/checked-close의
기존 fail-closed 계약을 유지한다. 이미 완료된 bounded fault/long-pin gold를
반복 생성하지 않는다. 기존 mixed harness의 제품 ReadSession·갱신·해제·
settlement 연결을 재구현하지 않는다. 남은 운영 범위는 원960초 배치 실행,
완결된 자원 high-water 관측과 debt/drain 수용이다. 현재20회 순차 churn은
15분/50QPS 수용을 대신하지 않는다.
Large functional full ACK가 실패한 상태에서는 이 성능 배치를 제출하지 않는다.
XL은 기존 독립 functional profile로 유지하고 Large 결과로 대체하지 않는다.

Off/Record는 기존 [TRACE-WATERFALL 계약](../../../oct-10-index-closeout/TRACE-WATERFALL-PLAN.md)의 동일 binary/input/caps·교대 Large
최소3쌍·각 phase의 paired ratio 중앙값 증가≤5%·drop0 계약을 재사용한다.
full ingest+seal, delta/noop/delete, reopen, 동일 ordinal query group을 각각 비교하고,
짧은 phase는 절대 차이와 분산도 보고한다. noise가 지배하면 INCONCLUSIVE이며 큰 phase로
작은 phase의 회귀를 상쇄하지 않는다. 실행 시점의 직렬 교대 순서·host/cache/filesystem
readiness는 별도 관측해야 한다. 개별 execution/summary의 digest·elapsed만으로 이 조건을
추정하지 않는다. 새 comparator 구현은 필수 조건이 아니며 검증된6개 capture의 고정 phase를
계산·보고할 수 있다. source/host readiness,footer/loss,실제 UI 표시와 timing qualification은
각각 판정한다. 현재 Small 기능 capture나 이 기본 목표의 문서화로 이전 BLOCKED/NOT_RUN을
PASS로 바꾸지 않는다.

근거: [Google SRE의 SLI/SLO 설계](https://sre.google/workbook/implementing-slos/)는
사용자 관점 latency/freshness/correctness와 측정 구현을 구분한다.
[Zoekt FAQ](https://github.com/sourcegraph/zoekt/blob/main/doc/faq.md)는 코드검색
시간이 corpus/query/result limit/hardware/cache에 의존함을 명시한다.
[RocksDB write stalls](https://github.com/facebook/rocksdb/wiki/Write-Stalls)는
background 작업이 입력을 따라가지 못하면 공간·읽기 비용이 누적되어 bounded
backpressure가 필요함을 설명한다. 위 숫자는 이 원칙에 따른 제품 초기 선택이며
해당 시스템들의 성능 수치를 Quanta에 전용한 것이 아니다. 새 storage engine,
전역 optimizer·scheduler·추가 runner 도입 근거로 사용하지 않는다.


W2-13의 원4K per-request1M/512MiB·30s 제품 조건은 별도 고정 gate다. 이 Large 운영 profile로 예산을 대체하지 않는다.

## Same-key publication observation

기존 scale/native_index/publication.rs의 publish/reconcile_submit/observe_terminal을 재사용한다. 측정 caller는 submit 최대1회와 원 operation/publication deadline 안에서 상태를 관측한다. Accepted·Unknown·Pending을 terminal Published와 구분한다. 새 키·자동 재송신·budget/deadline reset으로 성공을 만들지 않는다. W2-05 production의 exact idempotent replay/resume 계약은 별도 실제 producer가 소유한다.

## Resource, trace and steady-state acceptance

기존 diagnostic context/capture/producer lease를 Native boot→request/worker→drain에 전달한다. service/pump/provider/worker join 이후 원래 quiescent 조건에서 object IO snapshot을 수집한다. 원 collector의 제외 범위를 숨기거나 child context가 없는 sibling span을 nested span으로 표시하지 않는다.

세부 caller는 현재 Kernel submit/build/publish, Catalog reserve/confirm/commit, Storage object/scratch, pump WAL/recovery/reclaim, 외부 producer의 실제 호출을 다시 대조한다. 이미 연결된 phase를 새 TODO로 복제하지 않는다. 미계측 항목은 실제 source 부재를 확인한 뒤 같은 writer에게 exact hunk로 인계한다.

정적 감사에서 구분한 실제 연결 경계는 아래와 같다. 이것은 실행 시간·지배 비용의 측정 결과가 아니다. 상태·담당·다음 완료 조건은 SSOT만 갱신한다.

| 실제 호출 경계 | 재사용/추가 연결 계약 |
| --- | --- |
| [Kernel source](../../../../../crates/quanta-index-kernel/src/source.rs)의 apply/path/map | `NativeSourceApply`·`NativeSourceMapApply`가 실제 body에 연결돼 있다. 최종 inline path 값은 첫 validated scratch pass에서 기록하며, 중복 scratch 순회 제거와 함께 `NativeSourcePathInstall`의 별도 emission도 제거했다. enum은 과거 capture 판독용으로 유지한다. SourceApply는 immutable map 설치와 구분하며, 역사적 path span도 per-owner object 설치 시간이 아니었다 |
| [Kernel submit_authorized](../../../../../crates/quanta-index-kernel/src/update.rs)의 metadata/shape/encoding→seal_inputs/admit | 새 request의 metadata/shape/digest·encoding/children은 기존 `BuildPreflight`로, open_ingress·seal_inputs·ordinary admit은 각각 `SqliteWork`로 구분한다. 각 원 Result로 종료하며 payload backing은 기존 install/drop까지 보유한다. 중간 objects.install은 이 Catalog API span 밖이다. 초기 replay/claim lookup과 object identity 조회를 포함하는 admit_owned 전체는 이 범위 밖이며, API wall time을 SQL-only 시간으로 해석하지 않는다 |
| 같은 파일의 build_members/commit_transition | `BuildPrepare`·`NativePublish`는 이미 연결돼 있다. 더 세밀한 Catalog lock/transaction/commit 관측은 기존 상위 span과 포함 범위를 명시하며 object IO elapsed와 합산하지 않는다 |
| [Native ANN file-set 설치](../../../../../crates/quanta-index-semantic/src/native/ann_files.rs)의 install_fileset | 기존 `SemanticSeal` helper로 validation·budget·실제 child object 설치·최종 descriptor 설치의 원 Result를 감싼다. 모든 정상/오류 종료를 명시하며 Off는 clock/event를 사용하지 않는다. 이전 memory-backed Lance build와 이후 directory append는 범위 밖이다. span과 전역 object counter의 시간 겹침만으로 같은 호출의 syscall 오류를 입증하지 않는다 |
| [Native pump](../../../../../crates/quanta-index-searchd-runtime/src/native_pump.rs)·[reclaim driver](../../../../../crates/quanta-index-searchd-runtime/src/native_reclaim_driver.rs) | 기존 startup context→retention/reclaim의 같은 round request→`RuntimeMaintenance` child→실제 WAL/recovery/reclaim budget 연결과 raw terminal을 유지한다. ready-owned-work scan 등 다른 background 경로까지 포함한 전체 pump 비용으로 확대 해석하지 않는다. stop/join 뒤에 capture를 종료한다 |
| W2-05의 실제 Semantica producer | 실제 operation/source/build와 기존 trace/capture·clock 연결을 같은 producer 결과로 인계한다. Index-only 비용·다른 process clock·정의만 있는 span으로 analysis/catch-up 전체 비용을 닫지 않는다 |

Child first/reopen capture마다 권한0600·clock/capture identity·sequence·span balance·footer/drop/loss·boot/request/build/publish 증거를 확인한다. nonzero exit·missing/footer 불완전·취소를 성공으로 기록하지 않는다. Small UI 표시가 확인됐다는 과거 결과는 그대로 소비하며 동일 업로드를 반복하지 않는다.

## Mixed-load integration boundary

기존 offered_load/open_loop의 bounded schedule·queue·예정 발행 시각·실패 분모와 worker join을 사용한다. Native adapter의 opt-in·Large seed42·고정 funding·resource sampling·Off와 full ACK/gold 선행을 유지한다. warmup60s 후900s·50QPS·4worker·queue64, 원 예정시각 기준2s query deadline, publisher1s replace/delete/restore를 그대로 유지한다.

취소/timeout/late/queue-full을 분모에서 빼지 않고 offered=completion+queue-full+late 보존식을 검사한다. freshness·current/old witness·최종 ACK head/reopen을 분리한다. IPC 동안 snapshot mutex를 보유하지 않는다. Full Record 혼합부하는 bounded capture profile 없이 허용하지 않으며 기존 trace cap을 늘려 통과시키지 않는다.

### Explicit ordinary Native Off capture interface

- 기존 `causal_cost_capture`의 `--native-trace-mode {off,record}`로 일반 Native 실행의 trace 모드를 선택한다. 생략하면 일반 실행은 Record, mixed는 Off다. legacy/offline 입력과 mixed Record, Off `--timeline`은 실행 전에 거절한다.
- 일반 Off도 동일한 managed build/source·binary/daemon epoch, explicit Large/XL funding, summary/manifest, publication ACK와 search/reopen phase 검증을 유지한다. Off에서는 parent timeline·child capture directory/raw·child object IO가 없어야 한다. Record의 누락/partial trace를 Off로 복구하지 않는다.
- managed Off 제품 관측만 `VERIFIED_PRODUCT_OBSERVATION`/CLI0이며 prebuilt binding은 `UNPROVEN`/CLI2다. canonical profile·Perfetto·독립 child lifecycle trace proof는 `NOT_APPLICABLE_OFF_NO_CAPTURE`이며, 기존 Rust producer의 ACK/head/reopen 검증을 별도 trace 증거로 확대하지 않는다. 빈 exporter placeholder는 raw trace가 아니다. 이 경로의 회귀 통과는 실제 Large Off/Record 교대 비교나 성능 수용을 대신하지 않는다.

### Explicit Off mixed capture interface

- 기존 `causal_cost_capture`의 `--native-mixed-load`를 명시한 경우에만 혼합부하를 선택한다. `--backend index-native`, Large/seed42, reads/writes/mutations67,108,864와 bytes549,755,813,888의 explicit funding, resource sampling을 요구한다. caller는 warmup60초+measure900초보다 큰 전체 `--max-seconds`를 명시한다. runner는 deadline을 자동 증액하지 않는다.
- 이 모드는 기존 producer에 mixed 환경변수와 `--trace-mode off`를 전달한다. `--timeline`은 실행 전에 거절하며 parent/child canonical trace·Perfetto·canonical-profile은 생성하거나 검증했다고 표시하지 않는다. 정상적인 trace 부재와 예상 밖 trace 생성은 구분한다. exporter의 고정 경로에 남는 빈 placeholder는 producer가 만든 trace가 아니며 `raw_placeholder=true`로 명시한다.
- summary/manifest·daemon/binary epoch·source binding과 실제 product validator는 유지한다. 검증된 observed managed pair의 정상 결과만 `VERIFIED_PRODUCT_OBSERVATION`/CLI0이며 canonical 진단은 `NOT_APPLICABLE_OFF_NO_CAPTURE`다. source/build binding이 입증되지 않은 prebuilt 결과는 CLI2로 남긴다. 이 인터페이스의 성공은 mixed 제품 관측이며 quiet-host 성능·long-pin·RSS/debt·출시 수용을 대체하지 않는다.
- publisher audit의 same-key status reconciliation은 실제 Rust wire 값 `accepted`, `unknown_response`, `unknown_error`를 소비한다. initial submit은1회, 후속 status는 동일 original key·Published여야 한다. `unknown`/`pending`을 별도 성공 wire로 추정하지 않는다.

- 고정 mixed profile에서는 검증된 두 resource epoch의 aggregate process RSS2GiB·native scratch4GiB·state-root allocated64GiB 표본 초과를 거절한다. 경계값은 허용하고 미지원 RSS를0으로 채우지 않는다. state-root는 전체 물리 footprint의 부분집합이며, 상한 이하의 표본으로 연속 peak·startup/drain·외부 roots·pin/debt·운영 수용을 판정하지 않는다. 원 sampled-only 스키마와 제품 관측 결과의 범위를 유지한다.


## Physical and failure contract

- public object/map-root/Kernel staging64와 private child-first install256·aggregate wire64MiB를 유지한다. 배열·실제 child/pending/wire backing은 원 memory budget에서 선예약한다.
- complete Catalog reserve→실제 파일 sync·고유 shard/objects directory/barrier→writer settlement/checked pin close→complete confirm을 유지한다. 실패는 새 root를 반환하지 않으며 provisional custody를 원 recovery가 정리한다.
- scratch SQL의 per-row budget/reservation/hard cap과 scoped VFS를 유지한다. reference-array admission 뒤 외부 설치 전에 commit하며 journal OFF 실패는 connection 폐기다.
- same-Tx child proof·immutable held-FD identity·retirement hint·statement cache의 기존 수리와 실제 fault gold는 재구현하지 않는다.
- 외부 MapEditCursor callback 사이에 decoded prefix를 재사용할 수 있다는 현재 권한 근거는 없다. FD pin은 bytes/current name을 고정하지 않는다. fresh namespace/name/inode/EOF/digest 검사와 오류 순서를 생략하지 않는다.
- 측정상 지배 비용이 입증되지 않은 PRAGMA cache·추가 grouping·scheduler/compactor는 현재 작업에 추가하지 않는다. 과거 후보는 실행 큐가 아니다.

- 기존 `physical-enospc.py`는 host의 binaries/logs와 분리한 임시 bounded image만 사용한다. 명령은 자기 worker를 기다려야 하며 기존 process-group owner의 정상 반환 뒤에만 detach/absence proof/backing 삭제를 진행한다. `--max-seconds`는 양수이며 기본7200초, hdiutil 단계는 각60초로 제한한다. 새 supervisor나 임의 daemon 관측 권한을 만들지 않는다.
- create 완료가 불명확하거나 attach 성공·device witness가 없으면 inventory에 device가 보이더라도 detach/삭제를 진행하지 않고 backing 경로와 원 오류를 보존한다. guard 예외·취소도 nested session 종료를 추정하지 않고 보존한다. 안전하게 시작한 cleanup 중 취소는 정리 후 전달하며 성공0으로 삼키지 않는다. mock-volume/real-process 회귀와 실제 image lifecycle smoke,실제 OS ENOSPC/fault 수용은 각각 별도 증거다.



### Raw object-write ENOSPC and selected-view recovery

- 원 `Write::write` 오류가 문자열로 변환되기 전에 raw `NOSPC`만 `enospc_writes`로 계수한다. 실제 write 호출마다 `enospc_writes <= failed_writes <= write_calls`를 유지하며, quiescent same-origin snapshot과 delta 모두 보존식을 검사한다. 부분쓰기·Interrupted 재시도와 `Ok(0)` 뒤 write_all의 WriteZero를 syscall 오류와 구분한다. Catalog/scratch/create/fsync 오류나 전체 process I/O로 확대하지 않는다.
- Runtime stderr와 harness/Python summary는 동일 capture의 고정24개 숫자 필드를 소비한다. raw file identity·private mode·size·bounded tail SHA·capture ID와 detail epoch의 원문 동등성을 함께 검사한다. 구23필드 artifact에 기본0을 채우거나 synthetic counter를 실제 kernel 증거로 승격하지 않는다.
- 외부 filler의 큰 Write ENOSPC만으로 공간 소진을 완료 처리하지 않는다. 같은 bounded device의 available tail을 filesystem fragment 단위까지 유한하게 소진하고 raw fragment Write의 NOSPC와 available0을 확인한다. sync 오류는 별도 setup 실패이며 이 준비 증거도 원 ANN object Write의 raw NOSPC를 대신하지 않는다.
- 실제 ANN fault는 단일 순차 Kernel attempt의 정확한 install_fileset scope 안에서 원 ticket.created(File)가 성공한 뒤 동일 격리 볼륨을 채운다. 외부 filler의 NOSPC와 원 object writer의 실제 raw NOSPC를 각각 확인하고 실제 cancel 결과를 별도로 보존한다. phase span이나 observer-wide 합계만으로 ANN 귀속을 추정하지 않는다.
- A의 Complete view를 실제 pin으로 보유하고 B의 별도 SourceAdvance 뒤 마지막 ANN seal을 실패시킨다. 실패는 원 operation Unknown/Pending 및 B current source/view·A selected query/coverage를 보존해야 한다. 실제 cancel 완료를 관측한 경우에만 filler 제거·directory sync 후 원 key의 status/resume를 수행하며 원 claim 창을 늘리거나 새 key/claim으로 성공을 만들지 않는다. cleanup이 실패하면 uncertain custody/pending/quarantine을 성공으로 바꾸지 않는다.
- 새 query ticket은 Catalog 쓰기를 유발하므로 공간 해제 뒤, resume 전의 불변성을 검사한다. warm reload는 기존 authority와 실제 old pin을 유지한다. cold process는 마지막 pin release와 모든 기존 owner 종료 후 새 lease로 현재 B root/source와 상속 A(alpha/owner1)·새 B(gamma/owner65)의 고정 점수1·동일 Complete coverage를 확인한다. released old view의 재보존은 요구하지 않는다. 이 owner 회귀는 원4K 품질·공개10분 held-pin·지속부하·실제 SDK/UDS 결합의 수용을 대체하지 않는다.

## Retained child capture custody

Record는 state TempDir 밖의 artifact-owned private directory에 first/reopen JSONL을 직접 기록한다. 측정 함수 반환·다중 tier 지연 기록·실패 cleanup 뒤에도 raw 파일이 남아야 한다. Off는 해당 directory/file을 만들지 않는다.

각 complete drain에서 parser가 실제 읽은 바이트의 SHA-256·정확한 크기·capture ID와 publication 검사 범위를 묶는다. 성공 summary 직전 같은 파일의 footer/sequence/IO·bytes/hash/ID를 다시 검증하고 first/reopen은 서로 다른 process capture identity여야 한다. 바뀐 파일·삭제·부족한 publication witness는 성공 artifact를 거절한다. failure/refusal은 원 오류를 보존하고 경로·drain 당시 검사 여부만 남긴다. partial/missing을 complete/현재 integrity 검증으로 승격하지 않는다.

Child raw record는 기존64MiB total·4096-byte record cap을 유지하고 streaming으로 검사한다. diagnostic raw output은 fixture/state disk census 밖이므로 allocated resource 범위에서 명시한다. 재시작마다 초기 daemon path/content epoch도 검증하며 실제 exec inode나 source/build attestation으로 확대하지 않는다. startup/reopen/drain wall에는 이 실행 파일 검증 읽기가 포함된다.

first/reopen의 stderr도 같은 외부 private directory의 서로 다른 파일로 직접 기록한다. 각 epoch는 create-new·0600으로 열고 이전 원본을 덮어쓰지 않는다. drain의 고정24-field object IO summary는 해당 capture ID와 sync 합계를 대조한 원문에서만 얻는다. 성공 artifact는 같은 FD에서 읽은 최대16KiB tail의 실제 바이트·offset·전체 파일 크기·SHA와 파일 epoch를 재검증한다. 이는 bounded tail의 보존 증거이며 전체 stderr의 content hash나 source/build attestation이 아니다. 실패 artifact는 JSONL·stderr의 경로와 drain 당시 검사 여부를 남기며 현재 integrity는 unverified로 유지한다.

## Native source/build and replay boundary

기존 causal_cost_build/capture/profile/canonical 도구의 `--backend index-native` 경로를 사용한다. managed build는 같은 Cargo invocation의 harness와 daemon을 bench profile로 빌드하고 두 executable의 SHA/epoch·source/config/dependency 입력을 함께 묶는다. Cargo의 bench 산출물은 fresh target의 release directory에 있어야 한다. prebuilt 입력은 두 binary/digest를 명시해도 observed build 증거가 되지 않는다.

capture는 Native foreground work limits·churn을 명시적으로 선택하고 상충하는 inherited profile을 거절한다. 고정 funding은 Small/Medium에만 허용하고 Large/XL diagnostic은 explicit limits를 기록한다. 이는 원4K gate의 예산 변경을 승인하지 않는다. summary의 first/reopen 경로는 현재 invocation의 고정 sibling 경로여야 하며 JSONL과 stderr 원본·daemon·receipt·summary·manifest를 replay 전후 재확인한다.

typed product validator는 실제 publication 순서·고정 operation tuple/sequence·churn cycle, child footer/loss·24-field IO·sync parity를 확인한다. 자원 샘플링을 켠 capture는 first/reopen 관측의 존재·typed 수치·method·명시적 unavailable 축을 검증하고 canonical output에 그대로 보존한다. unsupported OS 지표를0으로 바꾸거나 sampled maximum을 continuous peak로 해석하지 않는다.

parent phase와 두 child capture는 독립 clock으로 보존한다. child 누적 IO를 parent phase별 비용으로 나누거나 clock을 추측해 정렬하지 않는다. managed pair와 canonical replay 수용은 로컬 진단 범위이며 quiet-host Off/Record·운영 한도·출시 수용은 별도다.

## Maintenance context handoff

기존 RuntimeMaintenance stage를 사용해 pump retention/reclaim 라운드의 실제 budget에 capture context를 전달한다. NativePumpServices의 단일 생성자는 기존 startup request의 context를 전달해야 하며, 새로운 round budget의 span child context가 actual driver/Catalog/WAL 호출로 이어져야 한다. 원 deadline·cancel handle·resource reservation을 유지한다.

driver의 raw Result terminal을 ResourceLimited/stop suppression 전에 기록한다. Off는 span을 만들지 않는다. pump stop/join이 끝나기 전에 capture root/producer custody를 종료하지 않는다. 이 shared native.rs parent 결합은 기존 통합자 한 writer가 수행하며 private pump 수정만으로 연결 완료를 주장하지 않는다.

## Pressure refusal diagnosis

실제 물리 압박에서 `DiscardInput`이 일반 `ResourceLimited`로 끝나면 기존 opt-in work diagnostic을 통해 request/execute checkpoint·scheduler admission·reply backing·maintenance scope·Catalog discard 경계를 구분한다. 원 IndexError와 wire 응답·취소/drop·재시도 정책은 유지한다. 입력 body/path나 backend error 문자열 대신 고정 stage와 bounded error class·기존 WorkBudget first-denial 진단만 기록한다. 성공 경로에서 설정 읽기나 로그 할당을 추가하지 않는다.

`first_denial=none`은 해당 WorkBudget의 거절 기록이 없다는 뜻이다. 물리 용량 거절이나 Catalog/VFS 내부 원인을 확정하는 근거로 쓰지 않는다. 실제 실패 경계가 확인된 뒤 기존 source owner가 해당 원인만 수리하며, 예산 확대·임의 retry·관측되지 않은 원인 추정으로 압박 oracle을 통과시키지 않는다.

## Historical section links

아래는 이전 deep link의 호환 anchor다. 원문은 [보존 이력](../../../oct-17-index-closeout/tickets/W2-09-cost-observability.md)에서만 확인하며 현재 할 일을 추가하지 않는다.

<a id="actual-staging-stack-follow-up"></a>
<a id="applied-retirement-hint-and-fixed-sql-reuse---2026-10-18"></a>
<a id="applied-scratch-transaction-packet--2026-10-18"></a>
<a id="current-managed-confirmation-contract"></a>
<a id="frozen-minimal-capacity-repair--scratch-candidate-boundary--2026-10-18"></a>
<a id="implementation-packet"></a>
<a id="large-deadline-follow-up-remaining-lexical-source-candidate"></a>
<a id="mutation-staging-physical-cost-handoff--2026-10-18"></a>
<a id="native-source-phase-packet--2026-10-18"></a>
<a id="optimized-same-oracle-profile-boundary---2026-10-18"></a>
<a id="post-l-read-only-staging-cost-packet---2026-10-18"></a>
<a id="prefix-reuse-contract-audit-conclusion---2026-10-18"></a>
<a id="private-reachable-map-aggregation-contract"></a>
<a id="remaining-caller-handoff-boundaries"></a>
<a id="residual-lifecycle-sql-preparation-candidate"></a>
<a id="w2-09--실제-phase-비용부하유지보수-운영-수용"></a>
