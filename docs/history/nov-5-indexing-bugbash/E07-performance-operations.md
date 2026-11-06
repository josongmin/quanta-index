> 2026-11-05 KST에 보존한 이력이다. 본문의 날짜별 현재·다음·잔여는 당시 기록이다.
> 현행 Native 인덱싱 RCA와 신규 작업은 [bugbash SSOT](../../plans/nov-5-indexing-bugbash/README.md)가 소유한다.

# E07 — 총비용·성능·관측·운영

상태는 [SSOT E07](../../plans/oct-22-finalizing/README.md#e07). 원 요구 R13/R39, 종전 W2-09/10과 각 기능의 성능/운영 결과를 소비한다. PHYSICAL/harness와 기존 SEARCH 원4K의 단일 실행 큐를 유지한다.

최신 코드·실행 상태는 [SSOT E07](../../plans/oct-22-finalizing/README.md#e07), 이 채팅의 실제 잔여와
병렬 준비/단일 실행 큐는 [2026-10-30 종료 순서](E07-WAVES.md#e07-closeout-plan-20261030),
수정권은 [E07 비중복 경계](../../plans/oct-22-finalizing/OWNERSHIP.md#e07-nonoverlap-boundary-20261030)를 따른다.
선택 WAL 수리 이후의 현재 코드·병렬 준비·실패 후 다음 수리·단일 실행 조건은
[2026-11-03 C0–C7 실행계획](E07-WAVES.md#e07-wal-policy-execution-20261103)에 고정한다.
10-31 잔여 계획은 해당 source의 이력이며 새 writer 배정은 위 C0–C7을 따른다.
아래 RCA·capture·resource 절은 source별 결정/실행 이력과 고정 수용 계약이다.
이력의 `현재`·`다음`은 해당 source/기록 시점의 표현이며 새 작업 배정이 아니다.
완료한 수리를 재발급하거나 이전 source 결과를 현재 성공·실패로 승격하지 않는다.
아래 [DoD](E07-performance-operations.md#dod)는 최종 수용 분모로 유지한다.

기존 수리 코드·마지막 실패 이후 변경·조건부 Block 후보와 검증 공백의 구분은
[현행 코드·증거 감사](../../plans/oct-22-finalizing/VALIDATION.md#e07-actual-code-scope-audit-20261103)를 따른다.
Block은 DoD가 지정한 필수 형식이 아니다. 정확성 수리의 소스 존재,해당 source의
실행 증거,남아 있는 성능 실패와 운영 미수용을 각각 판정한다.
후속 inventory-only Block과 Catalog transaction child exact set은 반영·집중 검증됐다.
C8 Large Off/Record는 원120s에서 FAILED다. controlled WAL의 최대8 append frame
배치 후 영향 회귀는 VFS83/Catalog300/lexical133/Kernel4/UDS5가 통과했다.
[WAL 후속 계약·실제 결과](../../plans/oct-22-finalizing/VALIDATION.md#e07-wal-append-batch-20261103)를 따른다.
48MiB Catalog cache의 C10 Large Off143.328s/Record146.979s도 원120s에서 FAILED다.
Record Map spill0/WAL calls25741은 이전 C9의 spill158543/WAL calls336046과
서로 다른 prefix라 속도 개선율이 아니다. Prepare2405개/51.199s 중 calling-thread
CPU30.043s는 남은 비용의 귀속 자료다. 현재 token-stream 수리는 기존 TextSpool의
term,pos 단일 cursor와64행 token INSERT를 쓰며 full/delta 공통 prepare consumer,
원 WorkBudget/lease/원 오류를 유지한다. lane2 전체 Prepare 아래 lane34 tokenize,
lane35 term-loop를 중첩 관측하고 Off는 진단이 모두 없을 때 직접 호출한다. 두 하위
시간은 부모에 더하지 않는다. 이전170/171의 generic100-read fixture 오류는
128그룹/257회 읽기를 기존1024-read fixture로 연결해 수정했고96B 메모리 상한과
제품 예산은 유지했다. 수정96cf1f2에서 strict·lexical/reader/spool/trace171·Kernel4·
UDS5·hexagonal은 VERIFIED다. 구조/정상 pair도 통과했지만 C11 Large Off145.324s/Record141.878s는 원120s FAILED다. [현재 source·검증](../../plans/oct-22-finalizing/VALIDATION.md#e07-token-stream-20261103).
C11은3325/4096파일 준비가 끝났고 token SQL은1.103s였다. 새 후보는 관측된
Prepare+Map main write523K/admission16.328s를 줄이는 max8 연속 checkpoint write다.
원 safe-frame/reader lock/WAL sync/main sync/backfill 순서를 유지하도록 합류했고,
C UBSan은 c6e911b6에서 VERIFIED이며 C가 같은 f591a0e3에서 VFS85·Catalog300·
lexical/reader/spool/trace171·Kernel4·UDS5(총565),strict·경계·모듈도 VERIFIED다.
실제1K/4K checkpoint 호출 감소와8K scalar·복구,정상 pair도 확인했다. C12는
Off155.630s/Record164.032s로 원120s FAILED다. 첫64-input Map의 동일20439880B에서
main xWrite2553→1416(-44.54%),WAL305동일로 실제 coverage를 확인해 구현은 유지한다.
전체 timing 개선은 미수용이다. 실행 후16CPU host load72.76/53.21/46.57,
swap37543.38M이 관측돼 quiet 비교는 BLOCKED다. 조용한 로컬 현재 pair/원 caps로
다시 판정하고,여전히 실패하면 같은 source의 실제 비용에 따라 후속 코드를 선택한다.
호스트 경합이 유일한 SLO 원인이라고 단정하거나 완료된 수리를 재구현하지 않는다. [계약·검증](../../plans/oct-22-finalizing/VALIDATION.md#e07-checkpoint-batch-contract-20261104).
현재 사용자의 code-first 지시에 따라 [256 설치 envelope](../../plans/oct-22-finalizing/VALIDATION.md#e07-install-envelope-contract-20261104)를 합류했다. Core canonical256을 Storage/Catalog와 공유하고 고유 payload1MiB·SQL64·owner64를 유지한다. 설치 전 PK-move와 confirmed-record의 scratch 요구량을 합산해 한 번 예약하고 전체256 출력 검사 뒤 Ready로 이동한다. shard 경로도 쓰기 전에 원 grant로 준비한다. 일반 child-rich 배치의 prefix 선형조회 P2는 고정 정렬 테이블로 수리한다. 영향 strict·558개 회귀(동일 소스 원 근거 재사용 포함)·경계·모듈·정상 test=false pair는 VERIFIED다. C13 Off195.726s/Record182.282s는 producer 전체 시간이며 둘 다 원 BuildPrepare120s에서 FAILED다. 실제 같은1507입력 설치 쌍24→6,6841입력110→29로 경계 감소를 확인했다. Record Lexical101.706s/CPU37.723s와 host logical paging in78.09GB/out85.49GB를 분리 기록했다. 건강한 host에서120s 통과·host-only 원인·일반 속도 개선은 미입증이다. 현재 코드 동결,quiet 비교 BLOCKED이며 근거 없는 wire/Map256/read-cache 확장은 발급하지 않는다.
후속 [요청 단위 RCA·pending 조회 수리](../../plans/oct-22-finalizing/VALIDATION.md#e07-sourceadvance-rca-20261104)는
R7 동결 뒤 현재 조치다. 120s는 Lexical 단독이 아닌 SourceAdvance 전체 예산이며
실패 요청에서 SourceApply17.638s→Lexical101.679s가 직렬이다. Lexical 안의
COMMIT33.945s와 reserve→confirm 사이 물리 설치 경로35.808s는 겹치지 않는다.
pending alias의 확정 음성 SQL 조회를 제거했고 첫 owner65개 고유 ref/130개
논리 행의 추가 조회65회·원 inventory/물리 설치/환급 oracle과 영향181개가 통과했다.
owning all-target strict도 VERIFIED이며 변경 소스의 제품 성능은 NOT_RUN이다.
host-only 원인·120s 해결로 승격하지 않는다. 현재 추가 성능 결정은 COMMIT
CPU/native sync와 물리 설치 세부 비용을 분리해야 하며 단순 quiet 재시도만으로
잔여 코드가 없다고 단정하지 않는다. 기존 producer/DELTA/SDK owner는 유지한다.
현재 [namespace 검사 수리·CPU attribution](../../plans/oct-22-finalizing/VALIDATION.md#e07-namespace-cpu-repair-20261104)는
R3에서 합류했다. 관리형 root/objects의 중복 stat2회만 제거했고 namespace의
권한·inode·lease 검증과 standalone 경로는 유지한다. 변조12경우 oracle,
managed/SQL sync·settlement CPU 및 V5 parser/Perfetto consumer를 연결했다.
R3 owning all-target strict는 VERIFIED이고 Python772·Core production strict는
해당 source의 실제 근거다. 집중 Core30/Storage65/Lexical181과 hexagonal은 VERIFIED다.
변경하지 않은 Runtime/Catalog의 넓은 unit 재실행을 합산하지 않으며 정상 pair와
실제 Large로 접합·원120s를 판정한다. 정상 pair는 VERIFIED이고 수정본 C14NO1
Off 원120s는 FAILED다. reads/writes/bytes/memory 초과가 아닌 BuildPrepare의
deadline이며 거절 감지 위치를 병목으로 취급하지 않는다. 동일 조건 C14NR1
Record도 원120s FAILED,새 CPU의 child trace는 유실 없이 complete다.
실패 요청120.205s 중 SourceApply15.696s/CPU4.955s→Lexical103.021s/CPU42.088s다.
후속 [Block/shard 수리](../../plans/oct-22-finalizing/VALIDATION.md#e07-block-cursor-repair-20261104)는
page64와 기존 read_retained 정책·grant·거절을 유지해 객체 재읽기를 줄이고,
검증된 shard Metadata를 재사용한다. 영향 strict·254/254·경계는 VERIFIED다.
실제 Kernel wrapper의 read_retained 누락도 기존 포트 위임·reference/취소
검사로 수리했다. production strict·caller17/17·경계는 VERIFIED,정상 pair도
VERIFIED다. 수정본 C15 Off/Record도 원120s FAILED다. 최신 요청은 SourceApply
34.966s/CPU9.074s→Lexical83.416s/CPU28.485s이며 물리 gap합35.909s 중 최소
21.668s는 전체 capture의 write/sync/barrier 호출 본문으로 설명되지 않는다.
이는 CPU/host/metadata의 측정 비율이 아니다. 전체 Kernel 타깃은 변경하지 않은
Source rebase lint2개로 FAILED다. 다음 코드는 같은 설치 묶음의 경로·예약·생성·
게시·FD 종료·정산 비용 분할이다. 실행 큐는 종료했고 원120s 해결은 미입증이다.
C13 COMMIT33.945s 중30.705s가 disjoint native sync/settlement/checkpoint envelope다.
이는 device/host 비율이 아니며 parent sync/FULL/auto-checkpoint를 제거하지 않는다.
48MiB cache는 기존 global384MiB/floor320MiB 안의 논리 예약이며 실제 RSS 상한
증거가 아니다. quiet overhead와 전체 운영 수용도 OPEN이다.
11-05 [같은 설치 묶음 물리 비용 분할](../../plans/oct-22-finalizing/VALIDATION.md#e07-physical-attribution-20261105)은
Core/V5→Storage→parser/summary/Perfetto/canonical의 묶음당9개 이하 고정 집계다.
계측 구현과 해당 고정본의 영향 검증은 완료했다. C16의 Prepare namespace/create
17.064s/CPU15.197s를 근거로 중복 anchor 탐색을 canonical 단일 fresh
nonreplacing claim으로 수리했다. Runtime strict·74/74·hexagonal은 VERIFIED다.
초기 admission 차단 후 공유23입력 변경을 보존하고 clean external8e94ca5c에서
정상 pair를 빌드했다:0/872.034s·drift0. 원 Large Off C17IO1/Record C17IR1은
모두120s BuildPrepare deadline으로 FAILED다. Record418,380events/41,836,325B는
COMPLETE·유실/health/orphan/open/lease0으로 canonically VERIFIED다.
Lexical 두 span 합계 wall100.629s/CPU29.479s,차이71.150s다. 이 차이는 스케줄링·
I/O 등 대기이며 전부 host 시간으로 귀속하지 않는다. Catalog wait59.792µs는
지배적이지 않다. 전체 구축·현재 공유 트리·정식 overhead·XL/mixed 수용은 미완료다.
child/FD/credit·FULL·예산은 유지한다. full-bound 선행 예약은 maintenance/종료
거절 순서 반례로 제외했고 Native Text의 기존 inline/묶음 중복 제거는 재구현하지
않았다. 추가로 안전하게 삭제할 코드 중복은 확인되지 않았다. Root 실행/분석은
종료했다. 아래 과거 미구현/실패 설명을 새 source 결과로 재사용하지 않는다.

## 비용 계수·유지보수 설정 해석

이 절은 코드의 계수와 설정 의미를 고정한다. 현재 판정과 source별 증거는
[SSOT E07](../../plans/oct-22-finalizing/README.md#e07)·[10-31 코드 재감사](../../plans/oct-22-finalizing/VALIDATION.md#e07-code-reaudit-20261031)가 소유한다.

- `NativeTextIndex::plan/prepare`는 Bootstrap/Patch를 같은 builder에서 처리한다.
  `InventoryReuse`와 `InventoryBatch`는 prepare당 하나이며, 확인된 occurrence는
  packet flush 뒤에도 재사용된다. 기본 확인 창은 첫64 identity를 보존하고
  교체하지 않는다. 창 밖 재등장의 실제 빈도·비용은 별도 근거가 필요하다.
- `OccurrenceWrites::flush`의 packet-local 수리는 상위 `put_batch` 입력·객체 준비·
  출력 backing을 줄인다. 하위 `FileObjectStore::install_batch`는 원래 digest별
  `BatchIndex`로 unique 입력을 물리 설치에 넘긴다. Kernel의 호출 전 입력 gold가
  4→2인 것은 물리 reserve/confirm/sync나 새 파일 생성이 절반이라는 증거가 아니다.
- C6 후속 unique64 producer는 중복 fanout을 기존 scratch inventory의 Planned
  partition으로 옮기고 RAM/물리 packet은 고유 객체64개 이하다. 확정 전 cursor는
  첫 결과부터 거절한다. 전체 물리 identity 검사 뒤 refs/count/attempt에 묶인
  사전 승인 receipt만 Ready 전환을 수행한다. 새 format/공개 port/내구성·상한 변경은
  없다. 새 영향/제품 판정은 SSOT만 갱신하며,underfill 수리만으로120s 통과를 예측하지 않는다.
- 최신24f11 observer는 buffered inventory next/EOF와 posting push를 trace하지 않고
  실제 page fill 시도·SQL flush·occurrence port get만 세분화한다. C6BR1은454454events
  COMPLETE/loss0이다. 이전 row-level span 수와 최신 page-span 수는 다른 단위이며
  FD/device reads 또는 전체 성능 개선율로 비교하지 않는다. 원120s는 여전히 FAILED다.
  bounded compact EntryMap은 아직 미구현 후보이며 바로 확정하지 않는다.
  [F0→조건부 F1–F3](E07-WAVES.md#e07-entry-map-rca-plan-20261103)에서 canonical
  입력의 적합률·payload/children/wire 크기를 먼저 분리한다. 작은 delta 재작성과
  실제 reader/profile/GC 경계까지 원 계약을 지켜야 후보를 수용한다.
  세부 실행은 [R0 계약·4slots 병렬·C1–C8 테스트](E07-WAVES.md#e07-entry-map-contract-parallel-tests-20261103)를
  따른다. positional children의 inventory-only block을 선택했고 posting Small/Generic은
  유지한다. F0의 새 모델2014B payload/11150B wire는 실제 codec 성능 증거가 아니다.
- `ObjectIoSnapshot.reserve_objects/confirm_objects`는 도달한 호출의 객체 항목 수다.
  기존 파일 재확인·replay도 포함한다. batch 내 unique, 기존 파일 확인, 새 파일
  생성과 maintenance/Catalog 잠금 비용을 구분한 뒤 수리 함수를 선택한다.
- Native `maintenance`는 명시 운영 설정이며 생략 시 자동 draining이 꺼진다.
  scale의 Large/XL fixture는20ms 간격·라운드당1단계·2초 기한·512KiB 물리 reserve를
  선택한다. 이는 운영 기본값이나 필수20ms 계약이 아니다. 긴 라운드 뒤 재예약이
  이미 있으므로 실제50Hz 실행이라고 해석하지 않는다. pin 해제 후60초 debt0의
  결과 조건은 유지하며, 설정이 다른 실행으로 기존 실패를 소급 통과시키지 않는다.
- client600s는 socket I/O 기한이고 server120s는 별도 dispatch 권위다.
  `managed_install` charge의 deadline 기록은 만료 감지 위치이며 그 줄의 독점 비용이
  아니다. Catalog checkpoint는 잠금을 잡고 실행되지만, 이전 source의 중첩 span을
  합산해 현재 source의 critical path나 예상 개선율을 만들지 않는다.
- Native `source_mutation_count`는 fixture shape의 선언값 검사이며 물리 mutation이나
  genuine dependency fanout 측정이 아니다. sampled RSS·일부 root의 디스크 관측과
  완전한 실패 trace도 각각 전체 수명 cap·제품 성공·정식 Off/Record 수용을 대체하지 않는다.
- 개수만 소비하는 inventory 검증은 기존 occurrence object·전체 JSON 검증을
  공유하는 `text_occurrence::load_count`를 사용한다. 위치 배열을 소비하는 검색/
  phrase는 기존 `load`를 유지한다. count 경로는 parser scratch를 원 budget으로
  승인하고 sorted1..8192·u32·필드/끝 검사를 모두 수행하며 위치 Vec/두 번째 parse만
  생략한다. Kernel attempt DATA cache는 이미 있으므로 lexical miss를 물리 재읽기와
  동일시하지 않는다. 이 구조 변경의 제품 수용은 [11-02 결과](../../plans/oct-22-finalizing/VALIDATION.md#e07-count-reader-20261102)에 따로 묶는다.
- canonical trace는 수명과 독립 Catalog 설정 관측의 operation root가 공존할 수
  있다. child 검증기는 root 수만으로 거절하지 않되,quiescent I/O 카운터3종을
  동일한 닫힌 수명/boot root에서만 수용한다. 여러 root의 카운터 합성이나
  request/child의 위장은 거절하며,trace 완전성이 제품 실패를 성공으로 바꾸지 않는다.

- Symbol attribution requires matching complete code sections and addresses,not just
  source hashes or equal section sizes. Embedded build paths can change instruction
  bytes. A separately linked diagnostic cohort must bind its own product/companion;
  it cannot supply old-product timing or names. `atos`/nearest local `nm` symbols
  remain candidates where source-line/function-boundary evidence is absent.
- A validated immutable read FD reused for sync must still match the current
  namespace/path dev/ino immediately before sync. Preserve its original admitted
  owner through close and every file/directory sync/full barrier;syncing a retired
  inode cannot confirm a replacement path. This is the existing PHYSICAL boundary.
- Same-prepare occurrence reuse may spill confirmed exact references into the
  existing admitted disposable TextSpool. The existing positive reuse limit bounds
  resident hot windows;zero keeps the fully uncached path. Scratch capacity must
  be admitted before the first physical install,and a reference becomes reusable
  only after successful original put and complete ordinal/identity validation.
  The original attempt/fence and Catalog protection remain authority;pending,
  published/prior or foreign-attempt references cannot seed a hit. No new RAM
  envelope,store,serializer,sync exemption or quota-error fallback is introduced.
  This structural packet's execution follows [D and the same C queue](E07-WAVES.md#e07-residual-packets-20261102).

- Inventory diagnostics use bounded synchronous owner-prepare,owner-diff and
  at-most64-owner map-apply spans. They preserve the original budget,custody and
  failures through observe_attempt;no token-level events or semantic/cache changes.
  Calling-thread CPU is valid only for those synchronous stages. Inclusive lexical
  and child durations overlap;neither sums nor checkpoint counts establish owner
  progress or an exclusive bottleneck. These spans are selective: scratch flush,
  source-cursor and contribution work may remain in the parent lexical residual. [E contract](E07-WAVES.md#e07-residual-packets-20261102).

- A native VFS lane22 interval is a Record-scope envelope,not exclusive callback
  CPU. Use its admission/lower-envelope/post-callback counters to distinguish
  work;lower includes C glue/native file witnesses and is not pure device time.
  Requested write bytes are not successful,physical or unique bytes. Do not add
  these nested counters to Catalog commit/hold or inventory spans. A preparation
  span count omits cached/reused paths and is not total owners completed.

- WAL truncation scheduling is independent of the GC emergency byte reserve and
  reclaim tick. A logical backfill checkpoint and physical WAL truncation have
  different costs and purposes. Reuse the existing maintenance driver and actual
  IO/work admission;preserve WAL/FULL,VFS custody,GC cadence and reserve. The first
  repair must coalesce optional truncation attempts and distinguish deferred
  checkpoint work from cancellation/deadline/storage failure. Do not switch off
  automatic checkpoints or enlarge cache as a substitute for a measured policy.
  [Source/trace RCA and bounded repair contract](../../plans/oct-22-finalizing/VALIDATION.md#e07-wal-reclaim-policy-rca-20261102).

- Automatic reclaim attempts the existing recovery/GC round first. A successful or
  ResourceLimited round may use its remaining original WorkBudget and held scope
  for one independently due optional WAL truncation if its request is still live.
  A failed GC round remains failed and is not retried in the same round;this only
  offers conditional headroom for a later attempt. GC cancellation/storage failure
  does not authorize new WAL work. Explicit manual reclaim
  and prepare_discard retain their pre-cleanup headroom semantics. Real capacity,
  storage,cancel and deadline failures remain failures;only optional unfunded work
  and genuine SQLite busy are typed deferrals. No second budget resets failed GC.
  The existing automatic-round ResourceLimited wrapper still records the failed
  round and retains durable debt/cursor before continuing bounded retry;it does
  not turn Catalog capacity refusal into an optional deferral or product success.
  Product Catalog busy_timeout2s equals the fixture round timeout:optional TRUNCATE
  must not wait on that handler. Scope a nonwaiting busy setting under the original
  Catalog mutex and check restoration on both success/error. Ordinary writer busy
  policy is preserved. In-flight backfill/fsync is still not preemptible.
  GC-first does not promise successful cleanup under actual ENOSPC:preserve debt,
  custody and explicit recovery,then prove progress after original funding returns.
  [Contract and independent oracles](E07-WAVES.md#e07-wal-policy-execution-20261103).

## RCA history — 2026-10-28

후속 소스에는 두 수리를 적용했다. Postings의 독립 두 번째 cursor를
기존64-term/64-row 경계의 PK keyset range로 변경했고, renewal은 기존
worker trace ref→pump lane29→Kernel→Catalog 동기 TLS scope에 연결했다.
Postings 신규7/7 회귀는 VERIFIED(고정129-term tuple/ordinal,실제 SQL
129→5회,PK/no temp sort,연속 page·늦은 손상·빈 key·예산·메모리 환급).
Kernel 실제 renewal1/1·Runtime link/Off2/2도 VERIFIED다. 기존152 영향 회귀의
초기151통과/1실패는 교체 전 SQL을 관측한 횟수 probe였다. 관측 대상만
새 range SQL로 변경하고 출력 oracle·비용 상한을 유지한 재검증은152/152
VERIFIED다. 전체3패키지 strict는 기존 관측/E02 테스트의 lint로 FAILED다.
새 caller/cursor의 지적은 수정했다. 기존 test-only lint는 별도 잔여이며
제품 성능 결함으로 과대 분류하지 않는다. 아래03d3의
제품 실패와 구분하며, 후속 소스 원4K/Source30s/worker 성능은 NOT_RUN이다.

**직전03d32194의 원4K 전체 Off는 FAILED다.** SourceAdvance가
30.004044125s에 DeadlineExceeded였고,원 key 상태는 Pending/outcome Unknown,
provider rows/queries는 모두0이었다. 최초 거절은 `cancelled`이며
reads713626/1M,bytes495009299/536870912로 수치 cap의 최초 거절이 아니다.
취소 관측은 NativeCoverageMapApply의 object.rs:579(예약 뒤 checkpoint)다.
이 위치는 누적 요청 시간이 소진된 관측점이며 해당 함수가30초를 썼다는
뜻이 아니다.원4K 본문0/1·83.88s,command88.523s/exit101,outer timeout 없음,
source/binary binding VERIFIED·drift0·daemon 정상 종료다. 후속 worker/Q64/
recall/재오픈/증분/overlay는 이번 실행에서 NOT_RUN이다.

후속86314의 같은 binary 원4K Record도 Source30.0033145s에 실패했다.
canonical capture는 COMPLETE23207/loss0/health0/orphan0이며6개 실행된
동기 구간 CPU가 모두 있다. SourceApply15.849s/CPU7.413s(owner9.104s/
CPU3.810s,map6.564s/CPU3.530s),중단된 Postings8.670s/CPU4.405s다.
Coverage/Publish/worker는 미도달이다. 그러나 함수 sample은20s 수집 뒤
25.021s 종료 guard에 걸려 파일 없이 중단됐고 NOT_RUN이다. observer가
추가된 실행이므로 Off/이전 source와 시간·완료량을 직접 비교하지 않는다.
profiler 자체11227은 소유 sleep 프로세스에2s/10ms로 실행해5.080s에
정상 종료·동일 PID/start·13174B Call graph를 확인했다. 수집 인자2초가
보고서까지2초를 뜻하지 않는다. 이 확인은 큰 daemon에서25s가 부족한
원인을 확정하지 않는다. 다음 함수 관측은 짧은 수집과 보고서 완료 대기를
구분하며,확인 전 같은20s/25s 조합의 제품 실행을 반복하지 않는다.
후속22570의 같은 기존 binary 원4K는 Source29.505213667s 게시 후
worker에서 FAILED다.126.953s 시점에222/4096 완료를 관측했고,이후 비원자적
실패 snapshot은 completed263/blocked1/claimed1/pending3831이다.
watchdog 만료가 아니다. row3732의 dispatch2는670ms cadence+1419ms
Catalog renewal 뒤 WorkFenceLost→cancel→Interrupted→dispatch2 소진으로
Blocked(ResourceLimited)가 됐다.42reads/9748bytes는 cap 초과가 아니다.
정확한 WorkFenceLost 조건과 mutex 대기/SQL·COMMIT의 분리는 아직 미확정이다.

추가 소스 감사에서 **renewal 관측 연결 누락(P2)** 을 확인했다. 갱신은 이미
별도 OS 스레드에서 실행되지만, `Kernel::renew_owned_work`가 Catalog의
thread-local diagnostic scope 없이 원 port를 직접 호출한다. 따라서 기존
capture에서 갱신의 lock wait/hold·COMMIT을 분해할 수 없다. 선택한 수리는
기존 worker trace ref를 started 메시지로 전달하고, 갱신마다 dispatcher 아래
짧은 SqliteWork/lane29 span을 열어 동기 Catalog scope에 연결하는 것이다.
완료된 worker span은 부모로 사용하지 않고 link 대상으로만 사용한다.
이는 관측 수리이며 lease 실패의 성능 수리가 완료됐다는 뜻은 아니다.

Postings2s/10ms·`-mayDie` 샘플은185689B로 정상 완료했다. macOS가 Path를
privacy-redact하여 원 runner의 strict path 판정은 NOT_RUN으로 보존했다.
별도5768 custody audit는 pre/post PID/start/command·parent·정확한
binary SHA·report UUID/load·완료 메시지로 VERIFIED이며,redacted Path는
신원 증거로 사용하지 않는다.함수 이름은 동일 TEXT companion 확인 전 미확정이다.
전체 trace는134587765B,events_dropped513/footer invalid라 PARTIAL이다.
이 실행을 loss0·tracing overhead 수용으로 사용하지 않는다.

이전3b13f740의 단일 Source-only Record는26.645467333s에 Published,provider0으로 종료했다.
원30s/1M reads/512MiB는 유지했다.캡처 COMPLETE25119/loss0/health0이며
13개 동기 구간의 CPU가 모두 관측됐다.계측 추가 외 성능 알고리즘 변경은
없으므로,직전ca176의30초 실패 대비 수리 개선율로 계산하지 않는다.
경합 진단 단일 실행이고 남은 시간 여유는3.355s뿐이다.p95·quiet timing·
원4K ANN·worker·RSS·Off/Record overhead 수용은 이 역사 결과에 포함되지 않는다.
같은3b13의 Python377/377·Core/recorder52/52·Source trace1/1과 해당 strict
범위는 VERIFIED다.3b13 실행은 종료됐으며 후속 코드 검증은 아래와 같다.

후속 코드03d32194는 Postings cardinality를 기존64-term page의 단일 range
집계로 연결했다.모든 key/base posting 인증→page SQL→checked DF→기존
edit-stream 순서이며,Scratch TLS scope를 다른 객체 읽기까지 보유하지 않는다.
정상 성공의 term당 logical read1은 유지하고,임시1024B backing은 별도
Temporary lease로 예약·환급한다.내부 경쟁 오류 순서/partial usage는 달라질
수 있다.정적 적대 검토·format·hexagonal 및 lexical strict85.623s,
lexical145/145·Kernel3/3은 VERIFIED다.최초2297의 lint 실패는 수정했다.
같은03d3의 원4K 전체 Off6436은 위 실패로 종료했다. SQL 실행 수 축소는
구현·기능 증거이며 Source30s 해결이나 제품 시간 개선의 증거는 아니다.
아래3b13 결과를03d3의 성능 증거로 사용하지 않는다.

| 이전3b13 동기 구간 | wall | 호출 스레드 CPU | 의미 |
| --- | ---: | ---: | --- |
| Source owner 준비 |4.138s|2.913s(70.4%)|인증/읽기/맵 조회/scratch 혼합;PRAGMA 단독 원인 아님 |
| Source map 적용 |3.065s|2.474s(80.7%)|CPU 비용 큼;같은 MapPort 사용 |
| TextPostings |9.583s|5.963s(62.2%)|최우선 코드 비용 분석 경계 |
| Coverage map 적용 |3.323s|2.607s(78.4%)|기존 shared root/bulk 재구현 제외 |
| Work seal |.131s|.124s(94.9%)|주 병목 아님 |
| Publish |1.544s|1.440s(93.2%)|이번 실행은 정상 완료 |

CPU는 사용자/커널을 포함한 호출 스레드 값이다.중첩 구간을 합산하거나
wall−CPU를 I/O 또는 host 경합 기여도로 단정하지 않는다.Source 하위
Catalog hold13.092s/COMMIT7.921s,wait32.961us로 mutex 대기는 지배적이지 않다.
VFS177692 writes/474043312 요청B,BeforeWrite5.887s/lower1.299s/post.417s,
거절/쓰기 오류0,pager miss106318/spill10535/write58393이 관측됐다.
이 역시 중첩된 wall/요청/호출 누계이며 순수 device bytes/time이 아니다.

아래ca176은 이전 실패의 보존 기록이며3b13/03d3와 섞지 않는다.

| 확정 관측 — ca176 | 값 | 의미 / 한계 |
| --- | ---: | --- |
| Source client / server | 30.001s / 30.050s | client DeadlineExceeded,server cancelled;provider0 |
| SourceApply | 9.646s | Owner4.702s,Path.063s,Map4.863s 포함 |
| BuildPrepare | 17.102s | 성공;Lexical10.012s,CoverageScan2.107s,CoverageMap4.823s 포함 |
| TextPostings / Publish | 8.116s / 1.408s | postings 성공;Publish의 work spool에서 cancelled |
| Catalog hold / COMMIT | 15.111s / 9.057s | 중첩 wall envelope;CPU·device 시간 아님 |
| Catalog wait | 37.581us | mutex 경합 병목 근거 없음 |
| 논리 reads / bytes | 831033 / 517798872 | 1M/512MiB 수치 cap이 최초 거절 원인 아님 |
| 캡처 | COMPLETE24807,loss0,health0 | 실패 원인 분석 가능;제품 성공 증거는 아님 |

당시 bytes 잔여는19072040B(약18.19MiB)였다.거절은 시간 cap이었으며,
취소 시점의 여유를 전체 작업 수용으로 보지 않는다.후속3b13은 같은
reads/bytes cap 안에서 게시까지 완료했으나 단일 Source 진단의 결과다.

기존ca176 원본(SHA79b0beb09bf6fa554924e6484b648a719f22f04b5a7b2ff98e22b4c9b2a07556)을
canonical parser로 다시 읽어 각 동기 stage의 하위 관측을 좁혔다.

| 구간 | COMMIT wall | VFS write admission wall | xSync wall |
| --- | ---: | ---: | ---: |
| SourceMap |2.394s|1.232s|.341s|
| TextPostings |3.965s|2.322s|.715s|
| CoverageMap |2.496s|1.470s|.396s|

각 열은 하위 envelope/aggregate로 서로 중첩되며 더하지 않는다. xSync도
witness를 포함해 순수 device 시간은 아니다. OwnerPrepare에는 이 descendant
Catalog/VFS scope가 관측되지 않았다. 이 결과는 COMMIT 전체를 fsync로
귀속할 수 없고,Map/Postings의 admission·scratch 및 Owner 준비를 우선
좁혀야 한다는 근거다. admission 내부 CPU·metadata 대기는 아직 미분리다.

여기서 write admission은 scratch의 PRAGMA 예약이 아니라 VFS BeforeWrite
callback 전체 envelope이다. `qiv_write`→RouterCallback::on_event→
Adapter::before_mutation→FileMutation::reserve_end_and_start로 연결되며,
반복 쓰기에도 live root/lock open·stat,allowance 검증·top_up(0,0),상태 mutex를
지난다. 최초 쓰기의 begin_native_resize는 별도다. C FD witness는 이 timer
밖에서 lower write 전후로 실행된다. 어느 하위 함수가 지배적인지는 아직
미측정이다. transaction 단위 authority cache나 매-write 검증 생략은 제외한다.

같은 ca176 raw를 재집계한 Source 하위 관측은 VFS write166572회
(main50717/WAL115855),요청438394824B이다. BeforeWrite 누계6.140254733s,
lower-write envelope1.857204810s,post-callback.523960328s이며 거절/쓰기 오류0이다.
Catalog hold59개 모두 pager counter가 있고 hit2266292/miss104520/
spill9887/write55962이다. SourceMap의 spill은0,Postings1915,
CoverageMap2003이다. 이 값은 반복 페이지 쓰기 비용의 조사 근거지만,
논리 payload 대비 증폭 배율이나8MiB cache/auto-checkpoint1000의 인과를
확정하지 않는다. callback·COMMIT·phase의 중첩 wall에 다시 더하지 않는다.

FileObjectStore의 quiescent 전체 수명 관측은 file-sync.898426473s,
directory-sync.408488032s,Mac batch barrier1.783990211s,합3.090904716s다.
입력 staging을 포함한 전 수명 값이므로 Source 전용 시간으로 귀속하지 않는다.
다만 Source subset의 해당 object-install sync 누계 상한이므로 이 동기화만으로
30초를 설명할 수는 없다. Catalog/WAL/scratch/constructor/GC는 이 상한에
포함되지 않는다. 기존 bulk install은 이미 per-file fsync와 batch당 한 번의
F_FULLFSYNC를 사용하므로 새 batch-flush 구현은 중복이다.


확정한 실패 메커니즘은 **한 요청 안의 인증된 owner 처리와 postings/metadata
처리의 누적 wall 지연**이다.이전3b13은 해당 경계의 호출 스레드 CPU도
상당함을 확인했다.세부 SQL/검증/할당별 기여와 blocking 원인은 미분리다.
현재03d3의 원4K Off도 실패했으므로 tracing이 실패의 필수 원인은 아니다.
실패 checkpoint 위치를 주 병목으로 보거나,중첩 시간을 더하지 않는다.

완료한 수리와 범위를 구분한다.

- Catalog namespace COUNT prepared-statement 재사용:71의 집중2/2 VERIFIED.
  SQL은 매-child 실행하며 binding/digest/shape/예산을 보존한다.
- owner scratch의 최대3개 쓰기를 owner당 한 번 예약:9617의 Kernel strict·
  27/27(필수9개 포함) VERIFIED. 4096 add에서 최소8192→4096 admission으로
  줄지만,동일 cut Source 실패로 전체30초 해결에는 불충분함을 확인했다.
- managed object read의 같은 호출 안 root/objects 일반 검사 중복 제거:
  ca176의 Storage strict·tamper/closure/namespace14/14 VERIFIED. 매-read
  NamespaceLease·파일 inode·EOF/digest 검증은 유지한다. 성능 기여는 NOT_RUN.
- 선행 ingress 큐 왕복,writer-cache의 명시 계약·과금·설정 readback,
  checked Catalog/VFS 종료 수리는 기존 범위에서 VERIFIED다. 다시 구현하지 않는다.

추가 감사에서 owner 원본 stream seal,내구성 reserve/write/confirm,live root·lock
읽기 권한 확인을 안전하게 생략할 근거는 없었다. `openat`→`stat` 치환은 실제
접근 권한/ACL과 오류 의미를 바꾸므로 제외했다. Postings64-term 집계 SQL은
03d3에 구현·집중 검증했지만 제품 지연 개선율은 아직 미측정이다.

판별용 thread CPU 계측을 기존 Clock/중앙 SpanGuard/Counter에 구현했다.
Source의13개 동기 coarse stage만 동일 스레드의 CPU 차이를 기록하며,SqliteWork·
객체별 span·async root는 제외한다. Off는 syscall0,Record는 해당 span당
clock 최대2회다. BuildPrepare는 await를 지나 같은 스레드의 다른 작업 CPU가
섞일 수 있어 적대적 검토 P2로 제외했고 해당 거절 회귀를 추가했다.
기존 nix의 안전한 time API를 쓰며 unsafe·새 crate는 없다.
clock 실패·스레드 이동·차이 역전은 미관측으로 남긴다. 기존 v4 counter
schema와 recorder mapping·Python 소비자를 함께 갱신하며,v1–v3은 이 키를
거절한다. wall-CPU를 곧바로 I/O 시간으로 해석하지 않는다. 구현·집중 검증은
VERIFIED이며,제품 CPU 측정도3b13 Source 진단 범위에서 VERIFIED다.추가 인증
생략이나 cache 상향을 반복하기 전에 이 판별을 수행한다.

BuildPrepare 종료 뒤 Publish 시작 전 `WorkSpool::seal_lazy`에는 coarse span이
없었다. `NativeWorkSeal`로 기존 동기 호출을 감싸고 phase 예산 컨텍스트를
복원한다. 원 오류·failed_stage·취소·인증 stream은 변경하지 않는다. 기존ca176의
SourceApply+BuildPrepare+Publish는28.156s이므로,seal을 포함한 모든 나머지
구간의 합은 약1.894s 이하이다. 이 공백 하나를 주 병목으로 단정하지 않는다.

현재 측정과 원4K 실패로 다음 판별 대상을 좁혔다.

1. Postings9.583s/CPU5.963s를 첫 경계로 선택한다.그 안의 COMMIT4.546s,
   BeforeWrite2.588s와 xSync.715s는 중첩된다.반복 SQL 집계·edit patch·
   최종 map/install의 실제 비용을 분리해 typed cause·쓰기 전 거절·게시 순서·
   원 cap을 보존하는 기존 경로 수리를 선택한다.특정 SQL의 지배는 미입증이다.
   03d3는8194→129 SQL 실행 축소를 구현했지만 원4K Source는 여전히 실패했다.
   특정 SQL이 전체 실패의 주 원인이라는 가설은 아직 입증되지 않았다.
   서로 독립적인 입력 오류와 cap 오류의 경쟁 우선순위는 공개 계약으로 확인되지
   않았다. 내부 진단 순서 차이만으로 구조 변경을 배제하지 않는다.
2. Owner4.138s/CPU2.913s와 SourceMap3.065s/CPU2.474s가 다음 경계다.
   owner21개 예약은 PRAGMA 비용이 지배적이라는 증거와 near-cap 수용 gold가
   없으므로 미선택이다.WorkSeal.131s는 우선순위에서 제외한다.내구성 검사
   생략·무예약 cache 상향·auto-checkpoint 변경으로 시간을 맞추지 않는다.
3. 정적 적대 감사에서 worker index가 optional CPU를 부분 합산하는 P2를
   확인했다. worker 그룹 CPU 합계를 제거하고 canonical per-span 값만 유지하도록 수리했다.
   cached group CPU도 거절한다. 부분/전체 관측·겹침·다른 counter 보존을 포함한
   Python377/377 및 Rust owning52/52·Source trace1/1은 VERIFIED다.
4. 원 Source30s가 닫힌 뒤 원4K의 후반 owner worker 처리량을 별도 분석한다.
   Source-only 통과를 worker/Q64/recall/재오픈/증분 수용으로 확장하지 않는다.


worker CoverageSave 재감사도 producer→consumer를 확인했다. 고정 필드
CoverageBinding→prepare_parent→단일 coverage-key COW 갱신이며,owner token은
최대4 root/object를 합류한다. reachable install은 staged `take_prepared`가
없는 기존 subtree를 가지치기하고 새 노드와 직접 child만 예약/확인한다.
이 경로에서 기존 DAG 전체 재등록이나 owner 수에 비례하는 coverage JSON
직렬화는 확인되지 않았다. 따라서 해당 경로의 O(N²) 재빌드를 원인으로
단정하지 않는다. 이 정적 결과가 다른 ANN 경로나 실제 후반 처리량을
검증한 것은 아니다.

현재 worker 호출 경계도 분리했다.Owned Materialize는 Kernel의 일반
Source build_members로 내려가지 않고 materialize에서 early return한다.
Registry의 owner adapter 표로 semantic build_owner/prepare_searchable만
호출하며 NativeText는 CoreRequired 표에 별도 등록된다.따라서 semantic-only
worker가 NativeText::prepare/apply_postings를 다시 실행한다는 근거는 없다.
SourceLocal ANN도 선택 owner delta를 적용하고 projected reverse 갱신을
건너뛴다.이번 SQL 수리를 worker 처리량 수리로 확장하지 않는다.

원4K Record22570과 동일 daemon의 짧은 stack 관측은 종료됐다. 후속 cursor와
renewal 관측 수리의 집중 회귀를 합류한 뒤 원4K에서 Source 조건과 갱신의
Catalog 비용을 판별한다. 같은 원본의 sample/companion 수집을 다시 시작하지 않는다.
이전 원4K의2715/4096@1200s는 별도 잔여이며,Source 진단으로 닫지 않는다.
정식 동일 binary/input Off/Record≥3쌍·phase 중앙값≤5%·loss0,전체 수명 RSS/공간,
운영/fault/출시 수용도 별도다. quiet timing은 BLOCKED,나머지 미실행 gate는
NOT_RUN이다. 모든 원30s/1Mreads/512MiB·worker·내구성 정책은 유지한다.
[최신 CPU Source 결과와 변경 범위](../../plans/oct-22-finalizing/VALIDATION.md#e07-source-cpu-product-result-20261028).

<a id="rca-solution-20261028"></a>
## RCA와 채택한 해결안 이력 — 2026-10-29

**후속 정리 — 2026-10-30:** hot operation 참조 정규화를 구현했다. Catalog 전체291개 중289개
통과 후 테스트 fixture2개만 수정했고 집중2/2도 통과했다. 생산 코드는 그 사이
바뀌지 않았다. 생산 strict도 통과했고 source90c163e8/2879를 고정했다. 기본 bench
제품 빌드와 완료 캐시를 쓴 재개 빌드 모두1800s supervisor에서 중단됐다
(각1804.587s/1807.0s,body0/drift0,새 컴파일 오류 없음). 이후 외부 helper의 중복
1800s 제한 제거와 모의 lifecycle4건/AST 검증은 완료했다. canonical cargow의
admission/7200s 빌드 감독과 제품 cap은 유지한다. 후속 기본 bench 빌드는
179.903s/exit0/driftfalse로 VERIFIED이며 config
거절1/1도 통과했다. 같은 바이너리의 원4K Off는 Source4096을12.663531917s에
게시했으나 원1200s watchdog exact3778/4096에서 FAILED101이다.
provider3800unique/중복0/query0,source/binary 사후 결속true이며 body1216.17s다.
Q64/recall/reopen/delta/overlay는 NOT_RUN이다. 단일 초기 daemon/config는 관측했으나
reopen 미도달로 전체2-epoch config 결속은 NOT_OBSERVED다. 현재 처리량 실패의
canonical Record 분석까지 완료했다. Record는 exact3725/4096에서 FAILED이나 캡처는 COMPLETE(6253452events/loss0/health0/open0)다. 분석693.56s/exit0의 closed worker3754개에서 CoverageSave440.239s와 그 중첩 COMMIT296.676s/VFS349.522s를 확인했다. worker Catalog wait3.628s여서 조회 간섭을 주원인으로 선택하지 않는다. 후속 sync 통합 후보는4개 회귀/strict 통과 뒤 원4K3371/4096 실패·개선 미입증으로 철회했다. 경합 때문에 이전 소스와의 인과 비교는 불가하며 후보의 추가 ABI/trace 계약을 남기지 않는다. 2879개 source exact 복원 후 현재는4KiB 이하 controlled WAL frame의 header+page를 bounded 실제 write로 묶는 hunk만 선택했다. 원 sync-point/내구성/과금은 유지하며2개 전용 회귀→영향 배치→원4K로 수용한다. 직전 제품
ffca9eab는 Source28.3787s 게시 뒤 원1200s watchdog exact2361/4096으로 FAILED다.
선택 WAL source5c6d0f8a의 원4K 전체는 Off1030.42s/Record1163.71s에 통과했다. 두 capture canonical도5720099/24555events·유실/health/orphan/open span/lease0으로 통과했다. [현재 source별 제품 수용](../../plans/oct-22-finalizing/VALIDATION.md#e07-wal-frame-product-20261031)이 이를 소유하며, 앞선 실패 receipt를 현재 미해결 코드로 복원하지 않는다. 정식 Large 교대≥3쌍/quiet overhead·전체 수명 자원/운영 수용은 별도다. 경합이 다른 이번 한 쌍을 정식 오버헤드로 승격하지 않는다.

**이전 제품 결과 — 22dc1f77 Off FAILED:** 새 intent 수리의 Catalog280/280·strict,
Runtime 실제 설치/short-write/WAL3/3·설정 거절1/1은 VERIFIED다. 원4K는
SourceAdvance30.001041833s deadline에서 실패했다. operation은 조회 시 Pending,
BuildPrepare의 batch sync 전 checkpoint에서 취소됐고 provider0이다. reads476222/
writes201079/bytes369462203/mutations117490로 원 수치 cap 초과가 없으며
external resource refusal도 없다. body100.20s/command104.487s/exit101,
source/binary/config 사후 binding VERIFIED다. 초기 materialization에 도달하지
못했으므로 intent 수리의 worker 처리량 효과는 NOT_RUN이다. 이전 Record와의
host/tracing 조건이 달라 회귀/개선율을 단정하지 않는다. 수정 없는 Off 반복 대신
같은 소스의 Record RCA1회도 Source30.010349208s에서 FAILED다.
canonical COMPLETE21877events/loss0/health0/orphan0으로 종료했고 worker0이다.
SourceApply24.692873s/CPU8.156935s 안에 OwnerPrepare17.090505s/CPU4.701594s,
MapApply7.158870s/CPU3.371001s,PathValidate0.422152s/CPU0.081506s가 포함된다.
Lexical은1.163337s/CPU0.312584s까지만 실행된 뒤 취소돼 완성 비용이 아니다.
Record 중 host16logical CPU/load44.115/foreign rustc7·cargo4를 한 번 관측했다.
비CPU 시간에는 I/O와 descheduling 등이 포함될 수 있지만 각각의 기여는 UNKNOWN이다.
이를 tracing overhead나 intent 변경의 성능 회귀로 환산하지 않는다.

**후속 Source 코드 수리 — 07bb8989:** scratch 용량 검사에서 세 read-only PRAGMA
`page_count/freelist_count/max_page_count`의 program만 prepared-statement cache로
재사용한다. pinned SQLite의 이 값들은 step-time에 읽히므로 매번 현재값을 받는다.
`page_size`는 prepare-time constant여서 원 fresh prepare를 유지하며 모든 setter도
캐시하지 않는다. checkpoint/현재 page·freelist→reserved→page-size/max-cap 검증,
fee/오류 순서·실제 reservation·권한은 그대로다. Scratch31/31·lib/tests strict와
값 증가/freelist/상한 변조/no-DDL page-size 변경 회귀는 VERIFIED다. 실제 Source
호출 경로14/14도 VERIFIED다. 중간07bb 제품 빌드는 아래 합류를 위해 취소했으며
이 새 코드의 원4K/처리량 효과는 NOT_RUN이다.
기존22dc 제품 실행을 이 새07bb source의 제품 결과로 쓰지 않는다.

**관측 누락과 구현:** 기존 `ScratchIoBinding::enter_scope`는 진단 context를
받지 않고 `NativeScratchIo`는 VFS capacity scope만 열었다. 따라서 OwnerPrepare의
scratch xWrite admission/lower-write 비용은 기존 capture에 직접 귀속되지 않는다.
canonical port→ScratchConnection open/scoped→실제 current
WorkBudget→기존 VFS diagnostic scope를 연결했다. 생성 시 context 보관·전역 추측·
새 grant·collector는 추가하지 않는다. 무예산 Drop은 명시 None로 기존 close custody를
유지하며 완전한 종료 I/O 관측으로 주장하지 않는다.
이 수리는 관측 누락을 닫으며 Source30s 성능 수리와 별개다.

**합류 — ed9737cc:** 중간07bb의 Root1478 빌드는1585.862s에 Root가 명시 취소했다.
컴파일러 결함을 관측한 실패가 아니며 no-run VERIFIED나 제품 실행으로 세지 않는다.
해당 소스 제품은 NOT_RUN이다. 이후 baseline 해시가 일치하는17기존 파일과 새
실VFS 회귀1파일을 합류했다. context는 매 open/scoped 호출의 실제 WorkBudget에서
빌리며 연결 객체에 저장하지 않는다. SourceOwnerPrepare와 CoverageMap의 실제
xWrite 부모·중첩 Off clock/event0·SQL 오류/unwind 복원·무진단 Drop·고정SUM28·
allocation→space 해제 뒤 capacity/quiescence를 검사한다. 정적 형식18파일·공백·
hexagonal은 VERIFIED다. 이후 새 회귀의 HashSet→BTreeSet·고정 집합 gold·variant별
predicate·poison 오류 context를 정정했다. 이 테스트 변경으로 Root55378의 초기
입력과 source drift가 생겼으며 해당 실패 기록은 VALIDATION에 보존한다.
production 변경은 없고, 현재 결과는 아래 최종 고정 입력에서 별도로 판정한다.

**컴파일·영향 테스트 — 17e1d621 / 2872inputs:** Root55378은1675.844s/exit101로 FAILED다.
Lexical `text_spool/tests/edit_pages.rs`의 `setup` 자유변수1건(E0425)이 실제
컴파일 오류였으며 source drift와 구분한다. `run_count` closure가 기존 WorkBudget을
명시 인자로 받도록 수정했다. SQL과 고정 insert 기대값0→1은 그대로다.
ed9737cc 이후 차이는 기존 테스트와 새 진단 테스트2파일뿐이며 production 바이트는
같다. Root81900은 동일 영향6-library 기본 bench no-run에서 VERIFIED/exit0/
633.790763s/source drift0다. Root20848에서 Scratch31/31·Storage151/151,
Storage ignored1은 통과했으나, 무관한 File backend 대규모 회귀를 포함한 Lexical
전체733개 선택은 과도했다. Root가 해당 배치를 명시 중단했고 FAILED/interrupted
기록을 보존한다. 중단된 Lexical 전체를 성공으로 세지 않는다. 통과한2범위는 재실행
없이 결속을 확인해 소비하고 Native Lexical→RepoMap→Kernel→Runtime 실VFS 회귀를
이어 실행한다. 최종 영향 body는550passed/0failed/1ignored로 VERIFIED다: Scratch31·Storage151·
Native Lexical192(541filtered)·RepoMap47·Kernel122·Runtime7(493filtered).
새 실VFS 회귀는 정확1회 통과했고 원 중단 영수증/compile/binary/source 사후 결속도
일치한다. Root99923 생산6-library strict는784.407649s/exit101로 FAILED다.
기존 SearchPlane query_embedder.rs의 map_err_ignore2건이며,550개 테스트 실패가
아니다. 현재 고정 source89b581b8/2872inputs는 해당1파일의 미사용 오류 인자명2곳과
rustfmt만 정정한다. 기존 allocation-free scalar 오류와 코드/문구/알고리즘/API는
같다. 이 식별자 전용 수리에 새 runtime test를 추가하거나 불변550개를 반복하지
않는다.550개는17e1 source의 증거로 보존하며89b5 실행으로 재기록하지 않는다.
89b581b8의 생산6-library strict Root77586은 VERIFIED/exit0/85.945431s/
postbindingtrue다. 제품 no-run Root4139은325.475656s에 Root가 명시 중단했다.
terminal FAILED/exit2/error=compile interrupted/source drift0이고 컴파일러 오류를
관측한 실패가 아니다. 제품 body는 실행하지 않았다.

**최종 Off 구조 수리 — 5910b022 / 2872inputs:** 새 scratch bridge의 inline
`diagnostic::Scope.previous: Option<Active>`는 None-context에서도 큰 Active
상태가 scope 할당/이동 크기에 들어가는 문제였다. Active에는 두 counts[6]만으로
96B 이상이 들어가며 정확한 기존 size/시간 기여는 미측정이다. CURRENT와 previous를
Option<Box<Active>>로 바꾸고 명시 context.map에서만 Active를 할당한다. 이 guard는
이제 pointer와 zero-sized marker만 가진다. None-context의 masking/restore는
항상 유지하고 clock/event/Active 할당은 없다. Record에는 scope당 할당1회가
추가되므로 시간 개선이나 무오버헤드로 단정하지 않는다. owner 검사·기존 physical
ScopeGuard·capacity 판정·restore-before-finish·재진입/backfill 처리는 그대로다.
정적 독립감사에서 추가 결함은 없었고 rustfmt/공백은 VERIFIED다.5910b022 생산
6-library strict Root70805는 VERIFIED/exit0/13.228383s/postbindingtrue다.
현재5910 기본 bench 제품 no-run Root21624는 VERIFIED/exit0/1176.256284s/
source drift0/profile override없음이다. 동일 Runtime 바이너리의 scratch7·VFS66은
모두 exit0/0failed로 끝났다. VFS의 직접 stderr 진단이 named test 행과 종료 ok
사이에 섞여 최초 wrapper만 FAILED였다. 원 실패 영수증은 보존하고 source·세
바이너리·로그 SHA·명령·유일 이름·종료 summary를 재검사한 별도 재판정은 VERIFIED다.
테스트는 반복하지 않았다. 같은 바이너리 Runtime3/config1은4/4 VERIFIED다.
원4K Off Root41928은 FAILED/exit101/body79.02s/command81.047s이며
Source30.016400375s에서 Pending/provider0이다. 취소 지점은 registry.rs:233의
NativeSearchablePrepare 진입 checkpoint다. reads818912/writes360582/
bytes471907747/mutations278528, 수치·메모리·external resource 거절은 없다.
source/binary/config 사후 binding은 VERIFIED다. sampled test+daemon RSS
57296KiB/60samples/77s는 lower bound이며 수명 전체 peak가 아니다. 동시 foreign
cargo3/rustc4와 load27.3→27.7은 관측값이며 실패 기여율은 UNKNOWN이다.
기존 수정의 처리량 효과는 worker0여서 NOT_RUN이다. 같은 소스 Record RCA
Root31109도 FAILED/body71.04s/command73.407s/exit101다. client30.002676042s
만료 뒤 원 key 조회는 Published였으므로30초 안 응답 성공으로 승격하지 않는다.
provider0이고 종료 시 갓 dispatch된 worker의 cancellation만 관측했다.550개는
이전17e1의 증거로 보존한다. 전체 Lexical·전체 workspace
수용 주장은 없다.

**새 관측 결함과 수리:** 현재5910 Record raw165107833B는 written1587123/
dropped569350/open13074/footer invalid다. canonical parse Root17274는 exit1/
`SQLite diagnostic counter has no existing owner span`로 FAILED이며 재파싱하지
않는다. 이 trace에서 닫힌 phase CPU/시간을 발명하거나 loss0으로 승격하지 않는다.
Scratch read-only scope마다 lane22 begin/end와12counter를 생성하는 경로가
대량 이벤트의 구조적 원인이다. NativeText는 페이지 단위이며 ANN width64를 term
수로 곱하지 않는다. 실제 Source/trace 시간 기여율은 여전히 UNKNOWN이다.
최종7a882223/2873inputs의 단일 private observer는 Scratch read-only scope에서 span/clock을
생성하지 않고 첫 BeforeWrite/BeforeSync/CKPT 시작에 활성화한다. 첫 admission
거절도 기록하고 Catalog의 eager envelope는 유지한다. lazy Scratch lane22의 wall은
첫 시도부터 scope 종료까지이며 생략된 lane22를 물리 I/O0 증거로 쓰지 않는다.
새 실제VFS 회귀는4096개의 SELECT-only scope clock/event0 뒤 실제 INSERT의
current phase·단일 닫힌 span·12counter·고정SUM12와 capacity 해제를 검사한다.
observer 재진입/panic·첫 거절·sync-only 경계도 같은 영향 배치에 포함한다.
이는 기존 trace overhead 항목의 수리이며 별도 티켓을 만들지 않는다. Source30초
실패는 별도 잔여다. 내구성·권한 검사·원 cap·queue/trace cap은 유지한다.
Runtime 생산 --lib strict는 최초60a3의 rustdoc/표현식/Result::ok6건을 정정한 뒤
Root82300에서 VERIFIED/exit0/15.284350s다. 기본 bench combined no-run Root44802는
VERIFIED/exit0/699.277158s/source drift0/profile override없음이다. 새 바이너리의
Scratch8/VFS70 Root39574는78passed/0failed/0ignored·postbindingtrue로
VERIFIED다. 새4096-read/actual-write 회귀와4개 lazy 경계 회귀는 정확1회씩
실행됐다. 기존 eager/CKPT/동기화/custody 영향도 이 범위에 포함된다. 현재
Root93552의 Runtime3/config1은4/4 VERIFIED다. 원4K Off에서
`rows=4096 elapsed_ns=18204593875 published=true`를 관측해 이번 Source는
18.204593875s에 게시됐다. 전체 Off는 FAILED/exit101/body1230.17s/
command1234.163s/no outer timeout이다. 정확히 확인된 완료는3490/4096이며
1200s watchdog에 걸렸다. 뒤의 비원자 snapshot은completed3518/pending578/
blocked0/claimed0이고 provider3532unique/duplicates0/query0다. 이는 exact3490을
덮어쓰지 않는다. source/binary/config postbinding은 VERIFIED다. 따라서 현재
주 잔여는 Source30초가 아닌 aggregate materialization 처리량이며 ANN quality/
재오픈/증분/overlay는 미도달이다. host·monitor 조건이 달라 이전 실행과 개선율을
계산하지 않는다. readiness WorkStatus115712RPC의 누적 wall216.497s/
관측1155.163s는 worker 대기를 포함하므로 CPU/실제 지연 기여로 환산하지 않는다.
별도 polling/갱신 경로 감사에서 원 고정 oracle를 유지하는 관측 감소를 검토한다.
Root63907의 기존 Source-only4096 Record는 FAILED/exit101/body90.00s다.
Source client30.003244709s에 취소됐으며 reads706575/bytes457615305로 수치 cap
거절은 아니다. 동일 source/binary/config 사후 결속은 VERIFIED다. 정식 파서
Root73436은 raw1회 분석을 마쳤고 capture COMPLETE/37475events/loss0/health0/
open0을 확인했다. 분석 exit2는 Source body 실패를 유지한 결과이며 trace 무결성은
VERIFIED다. 그 전에 Root54702는 외부 분석기의 command 필드 위치 오류로 raw를
열기 전 거절됐다. 해당 실패는 보존하고 wrapper만 정정했다. 원 invalid5910 raw는
재파싱하지 않았다. 이 Source-only fixture는 원4K ANN/worker/overhead 수용이 아니다.

새 유효 trace의 중첩 비용은 SourceApply11.2368s/CPU6.6704s,그 안 OwnerPrepare
6.2562s/CPU3.4820s와 SourceMap4.8591s/CPU3.1123s다. Lexical12.1780s/CPU7.1223s,
그 안 TextPostings9.9287s/CPU5.7066s다. SourceMap managed reserve/confirm COMMIT
합1.8388s,TextPostings4.7280s가 각각 해당 부모 안에 있다. OwnerPrepare scratch
쓰기 envelope는0.01245s,TextPostings0.21415s이므로 전자를 scratch 물리 쓰기
병목으로 설명하지 않는다. COMMIT·부모 wall·CPU·중첩 lane22를 더하지 않는다.
실제 장치 bytes나 wall-CPU 차이의 scheduler/I/O 분해는 UNKNOWN이다.
다음 코드 잔여는 Catalog 쓰기 증폭과 Source 준비 CPU 경로다. WorkStatus의 매번
SQL 컴파일도 별도 정적 확인 중이며115712RPC wall을 제거 가능 지연으로 세지 않는다.
갱신 감사에서 pre-lock 시각 또는 heartbeat 간격 오류는 확인되지 않았다. lease
시간/원 oracle를 완화하지 않는다. 코드 변화 없는 full4K 반복은 하지 않는다.
과거5910의73개 통과를 새 source로 재기록하지 않는다.

**후속 CPU 반복 수리 — 8f7dee92 코드/영향 VERIFIED, 제품 수용 OPEN:** Catalog WorkStatus의 work-row와
root-custody SELECT는 SQL program만 재사용한다. 선택 key와 decoded intent의
독립 비교 뒤 같은 호출 안에서만 key를 재사용하며 root별 JSON/SHA 재계산을
제거한다. 최대3개 root는 고정 배열 iterator로 순회한다. 상태·payload·digest·
각 custody row는 여전히 매번 DB에서 검증하며 authority/result cache는 없다.
새 회귀는 warm 조회 후 Pending→Claimed 최신값·조회 중 DB무변경·source/projection/
prepared artifact custody 삭제/변조의 정확한 거절을 검사한다.

Source mutation commitment의 canonical JSON writer는 매 출력 fragment 대신
최대256 canonical bytes마다 취소/기한을 확인하고 시작·성공 종료에도 확인한다.
큰 단일 write도256B 이하로 나눠 처리한다. 검사 시점은 예전 fragment 단위와
완전히 동일하지 않지만 순수 CPU 계산256B 이내로 제한되며 외부 IO/heap allocation은
추가하지 않는다. canonical bytes/digest·length 후 charge·복제 SHA 뒤 commit·
독립 manifest 재검증은 유지한다. 새 observer 회귀는 실제 serde fragment 수와
호출 수를 비교하고 시작·중간·최종 취소와 기한/byte 거절에서 digest/count/charge를
검사한다. 이 수리의 CPU·원4K 처리량 개선율은 NOT_RUN이며 전체 수용을 닫지 않는다.
Core mutations26·Catalog lib282=총308passed/0failed와 생산 Core/Catalog strict는
VERIFIED다. Catalog 초기 컴파일의 terminal 소비자 불일치5건과 최초 strict4건은
수정 후 재검증했으며 실패 기록도 보존했다. Root85745의 기존4K Source와
exact128 완료 Record 진단은 빌드+본문 외부1800s 제한에서 빌드 중 종료됐다.
Runtime/harness 컴파일까지 진행했으나 body0/raw0이므로 제품 실패가 아니다.
제품 본문/파서 NOT_RUN,빌드 source drift 없음. 코드 변화 없는 재실행은 하지 않는다.

**후속 구조 수리 — compact registration 코드/Catalog 영향 VERIFIED, 제품 OPEN:** immutable checkpoint-prefix의 실제
등록12149행+intent2944행을 동일 rowid/PK/FK/domain/logical fingerprint로 재생한 모델은
JSON10732596B→고정폭3003964B(−72.0%),DB4268→2105pages(−50.7%)다.
등록/intent overflow는 각368→0이다. transient provisional행은0으로 비용 미측정이며
Python SQLite3.50.4 모델은 제품3.53.2/VFS/GC/물리파일/운영 write history를 대체하지
않는다. WAL visible slots를 누적 writes로 해석하지 않으며 runtime 개선율은 NOT_RUN.

채택 범위는 Catalog 내부 RegisteredObject payload뿐이다. 단일 기존 IR을 사용하고
reference32B/bytes8B BE/count4B BE/순서 있는 children32B 형식으로 저장한다. 공개
JSON·ObjectRef·Source issuer·key bytes는 유지한다. capture table에 명시적 codec1
stamp와 digest binding을 넣고 구형 DDL/JSON root는 쓰기 전에 거절한다. fallback/
upcast/자동 migration은 없다. 저장·reserve/confirm·bootstrap·reclaim·terminal custody를
함께 연결하고 SQL shape/admit→digest→decode·독립 edge ledger·rollback/fsync를 유지한다.
기존 managed JSON 비용 census는 보수적 요금 oracle로 유지한다. 모든 writer 통합과
독립 producer→consumer 정적 감사가 끝났고 Catalog288passed/0failed/body50.26s 및
생산 Catalog strict VERIFIED다. 정상 checksum의 잘못된 scalar 값은 기존 소비자가
원래 오류로 거절한다. nonce gate는6flags+3고정값으로148B≤기존160B/2reads를 지킨다.
Root8046의1b6ef76c/2876inputs 기본 bench Runtime lib/product no-run은
VERIFIED1071.451814s/drift0다. Runtime읽기전용9passed/0failed다. Root50701의
4K/128 Record 진단은Source30.0054s/CoverageMap에서FAILED,provider0/worker0다.
원숫자cap초과가아니며 source/binary/configbinding VERIFIED다. Root98963의1회
분석은COMPLETE37260/loss0/health0/open0을확인했다. 현재OwnerPrepare8.6178s/
CPU3.8320,SourceMap3.4901/2.5648,TextPostings8.5436/5.3288,
CoverageScan3.6852/1.7161이다. 중첩값을더하지않고,이전과공유호스트비교를개선율로
쓰지않는다(foreignrustc최대17/load최대42.63). 새잔여는owner/postings준비CPU와
Catalog DML/COMMIT이다. 세 정적RCA에서추가안전한주요수리hunk는확인하지못했다.
원manifest검증/과금/custody를없애는변경은선택하지않는다. 후속RuntimeIO9/WAL1/
config1도VERIFIED다. Root32558 같은source/binary 원4K Off도Source30.003747959s/
CoverageScan에서FAILED(provider0/query0),body84.77s/command88.223s다. 이후original
key는Pending이고usage685614reads/320422writes/425571365bytes/238318mutations,
first_denial=cancelled다. SDK기한→취소이며수치cap·worker1200s실패가아니다.
Source/binary/configbinding VERIFIED,Off의foreignrustc최대6/load최대23.1이다.
tracing단독원인설은배제하지만host경합/각코드의배타비용은아직미귀속이다.

**다음직접코드 — 관측공백수리통합/검증:** 기존stage를그대로쓰는private lanes로
OwnerPrepare의manifest cursor30/base lookup31/record get32/JSON·path·edge33/
body get·UTF8검증34와Postings의page/base35/cardinality36/entry patch37/
wrapper prepare38/put·topmap39를구분한다. stage는기존호출thread CPU측정대상이며
Core/공개ABI/포트/요금/실행알고리즘을바꾸지않는다. Off는원callback을직접실행하고
새시계/clone/할당/checkpoint가없다. Record는원budget진단부모만복원가능하게빌려
하위Catalog/VFS귀속을보존한다. 결과·first_denial·failed_stage·메모리drop순서는유지.
e78cea40/2876inputs로동결후영향검증과선택진단을모아1회수행했다. 원4K수용은OPEN.
기존 module baseline FAILED 기록은 보존했다. 선언과 실제 소비자를 대조한 Core6/
Kernel2 의도된 항목만 반영했고 후속 `just rust-cargo-modules`는 VERIFIED다.
생산 Kernel/Lexical strict와 hexagonal도 VERIFIED다. 새 관측의 Kernel library125와
Source 통합6과 Lexical Native192는0failed로 VERIFIED다(서로 다른 영향 테스트323개).
e78 제품 test-only no-run은 VERIFIED829.094s다. 후속 Record 진단은 Source4096을
22.020648208s에 Published하고 exact128 완료를 확인해 VERIFIED다. canonical433185
events/loss0/health0/open0,모든 private step CPU/부모 귀속도 VERIFIED다. Owner의
record/body 읽기 합7.0131s가 지배하며 JSON/path/edge는0.0204s다. Postings4.3021s
중 final top-map apply 단일4.0408s가 지배한다. 원 CAS/전체 stream/child-first 설치는
이미 있고 삭제 가능한 중복은 확인되지 않았다. Prefix 통과는 전체4K/Q64/재오픈·
quiet paired overhead 수용이 아니며1b6 fullOff FAILED와 구분한다.

**후속 코드 수리 — retirement readiness hint:** 무관한 pinned tombstone이 있을 때
읽기 종료마다 전체 inode를 재검색하던 경로를 수리했다. 전역 sweep·실패 재시도·오류
관측은 그대로 둔다. 같은 mutex에서 전체 검사 후 pinned-only면 힌트를 끄고,두 pin
해제 경로·7개 marker·authority 실패 복원이 힌트를 재설정한다. releasing 상태는
보수적으로 true를 유지한다. 실제 A 마지막 pin 환급을 B reader와 원 writer가 살아
있는 동안 검사하고,무관 read의 추가 전체 scan0·safe reader/new marker·중복 환급
없음을 검증한다. Runtime capacity70+VFS70/0failed와 생산 strict VERIFIED다.
현재ffca9eab/2876inputs동결후원full4K Off용no-run은204.061s/drift0로 VERIFIED다.
원full4K Off 본문은 Source4096을28.378668375s에 Published했으나 원1200s watchdog의
exact 완료2361/4096에서 FAILED다. body1226.80s/command1230.218s/exit101이며
source·binary·실제 Off config 사후 결속은 VERIFIED다. provider2405unique/2406total/
중복1/query0,후속 비원자 snapshot2397completed/1699pending/blocked0은 exact receipt와
구분한다. Q64·recall·재오픈·증분·overlay는 NOT_RUN이다. 수리 성능 기여율도 NOT_RUN이다.
자기 inode만 sweep
하는 대안은 무관한 ready debt의 재시도/오류 관측을 바꿔 기각했다. PHYSICAL의 다른
hunk와 모든 durability/cap 계약은 유지한다.

최신 실패 후 독립 RCA에서 status SELECT/work-root 검증은 이미 캐시되며 읽기 전용이다.
단,공용 Catalog connection mutex를 writer와 공유하므로 head-of-line은 가능하다.
WorkStatus97344회/호출 wall314.519s는 IPC·lock wait·SQLite를 분리하지 못한다.
갱신 clock은 mutex+BEGIN 후 읽으므로 pre-lock stale timestamp 결함은 없다. 느린
갱신1841/3284ms와 Unknown followup은 관측됐지만 lease 거절 원인/중복1건과의 인과는
현재 표본만으로 확정하지 않는다. Published 뒤 FenceLost도 있으므로 전부 실패로 세지
않는다. foreign rustc최대20/load최대41.89,전역 ps561회 누적wall102.404s도 제품 시간과
중첩된다. 단순 차감·host 단독 원인·코드 회귀/개선율 주장은 하지 않는다. 현재 확정된
추가 안전 코드 hunk는 당시 확인하지 못했다. 후속 private relation 수리는 아래 새
측정에 근거하며,동일 실행 반복이나 cap/fsync 완화로 전체 완료를 만들지 않는다.

**후속 구조 수리 — hot operation 참조 정규화:** immutable checkpoint-prefix의 실제
operation-object9204·attempt-object2944·object-intent2944행은319B canonical operation
키를 반복 저장한다. 현재 compact payload를 유지한 동일 행/PK·FK/역인덱스 재생에서
선택 테이블·인덱스의 page가4670→1716(−63.25%)이고 정규화 후 전체 행·digest fingerprint는
같다. parent ROWID/unique operation index 비용도 포함한다. operation-object만 바꾸면
3017pages(−35.40%)다. 이 모델은 전체 DB·원 write history/VFS/GC·제품 처리량 증거가
아니며 입력 main만 immutable로 읽고 짝 없는 forensic WAL은 열지 않았다.

채택 구현은 parent canonical operation BLOB의 UNIQUE/NOT NULL을 유지하고 private
SQLite operation_id PK를 추가한다. hot relation3개만 integer FK를 저장한다. 같은
connection/transaction에서 원 canonical key로 ID를 조회하며,ID는 공개 identity·digest
입력·캐시된 권한이 아니다. terminal/boot/GC/managed 소비자를 함께 연결한다. 원 key
encoding·domain digest·과금·cursor 순서·rollback/fsync·모든 cap은 유지한다. strict
canonical schema preflight가 구형 DDL을 쓰기 전에 거절하므로 fallback/migration은 없다.
서로 다른 parent·custody 변조·재오픈·구형 DDL 거절 회귀3개를 추가했다. Catalog 전체
291개 실행은289통과/2실패다. ingress SQL helper와 구형 DDL fixture의 누락 인덱스를
수정했고 집중2/2 통과를 확인했다. 제품 코드는 해당 실행 후 바뀌지 않았다.
생산 strict도 VERIFIED다. 기본 bench 빌드2회는 Runtime까지 진행 후 supervisor에서
중단됐고 새 원4K는 NOT_RUN이다. 동일 호스트 조건에서 세 번째 반복은 배정하지 않았다.

Provider 중복은 원 RFC의 exactly-once visible effect와 구분되는 물리 I/O 비용이다.
현재 prepared_work/Published 소비자는 이미 재호출을 건너뛰며 initial oracle도 unique
corpus coverage를 검증한다. 중복1건을 새 정확성 실패로 만들지 않는다. cold reopen의
재embedding 금지는 그대로 유지한다. 관측 도구는 stable PID만 표본화하고 전역 목록은
발견/교체 및30s 간격에만 읽도록 수리했다. mock12passed이며 제품 시간과의 인과적
오버헤드는 아직 NOT_RUN이다.

추가 대안의 채택 경계도 고정한다. Catalog workspace/operation key의 고정폭 codec은
PK/FK/역인덱스·digest·terminal decoder·저장 형식 stamp 전체에 영향을 주므로 현재
측정만으로 채택하지 않는다. VFS fresh root/inode 확인 생략·reserve/confirm 병합·
fsync 약화·수치 cap 완화도 제외한다. 새 정적 감사에서 Source owner의 per-row
transaction/FULL sync·불필요한 원본 재읽기는 확인하지 못했다.

**이전 실제 결과 — 11294467 Record FAILED:** Source4096은21.264099333s에
Published했다. 이번 본문 실패는 개별10초 Compaction이 아니라 원1200초
watchdog 안에 초기 materialization을 끝내지 못한 것이다. 마지막 exact receipt
관측3487/4096,후속 비원자 public snapshot3506completed/588pending/2claimed/
0blocked,provider3519unique/3519total/중복0/query0다. body1217.28s/command
1220.521s/exit101/outer timeout 없음,source/binary/config/trace-path 사후 binding
VERIFIED다. WorkStatus 대기1145.827s와 watchdog1200s의 차이는 원 deadline을
설정한 뒤 대기를 시작하기 전 Source staging/submit/handoff에 쓴 시간이다.
Q64/recall/reopen/delta/overlay는 NOT_RUN이다. 일부 WorkFenceLost 관측의
전체 처리량 기여는 미확정이다.

Root65271의 단일 canonical 분석은2127.910028s/exit0로 VERIFIED다.
raw624981238B/6031791events는 COMPLETE, attempted=accepted=written,
dropped/unwritten/open/leases/health/orphan 모두0이다. 전체3521 worker는 닫혔고
3518ok/2error/1aborted다. Compaction54건은 모두ok,wall 합38.309s/CPU20.805s다.
이번 지배 경로는 Compaction이 아니라 일반 owner 저장이다. ordinary3464건의
worker wall 합1003.591s 중 CoverageSave449.589s,그 안 Catalog hold364.627s와
COMMIT300.164s가 관측됐다. 모두 중첩값이며 동시 worker 합을 전역 wall로 쓰지 않는다.
CoverageSave lock wait는0.475s,cache spill은0이다. xWrite4724498회/요청12.718GB,
admission113.020s,lower-write envelope33.057s다. 요청량은 device bytes가 아니다.
누적 managed reserve236.451s/confirm326.433s/barrier71.820s도 전역 호출 합이며
exclusive 비용이나 개선율이 아니다. 변경 없는 제품 재실행은 하지 않는다.

**선행 intent rowid 수리(현재 compact codec 전 단계):** 실제 intent2944행/4090531 payload bytes를 재생한 좁은 저장 모델에서
WITHOUT ROWID→rowid는 전체2824→2092pages,overflow2062→368pages,
12개256행 이하 transaction의 누적 committed WAL3017→2283frames였다.
원 key/fence/payload/digest/NULL·PK 거절/FK·역조회는 동일하다. autocheckpoint1000의
현재 WAL 길이는 누적 쓰기가 아니므로 별도로 세었다. 로컬 Python SQLite3.50.4의
모델이며 제품3.53.2/VFS·원 DML 이력·throughput의 증거는 아니다.
이 근거로 source22dc1f77에서 intent 테이블의 물리 DDL만 rowid로 변경했다.
Catalog 전체280/280,lib/tests strict·형식 거절/재오픈·구조 검사는 VERIFIED다.
새 기본 bench 통합 빌드1533.169s/drift0는 VERIFIED다. 같은 바이너리의
Runtime3/config1은 VERIFIED,원4K Off는 Source30s에서 FAILED다. 초기 worker
처리량 효과와 전체 품질/운영 수용은 미검증이다.
object/provisional의 기존 rowid와 같은 방향이며 작은 ledger는 유지한다. canonical payload/checksum,
명시 NOT NULL·PK/FK·reverse index,원 reserve/confirm·WorkBudget·내구성은 유지한다.
구형 intent만 남은 저장소도 typed rebuild 거절하며 자동 migration은 하지 않는다.
영향 Catalog 회귀와 원4K 제품 수용은 별도로 판정한다.
[최신 실행과 분석](../../plans/oct-22-finalizing/VALIDATION.md#e07-compaction-custody-rca-20261028).

**인과 판정:** Source30s·worker1200s와 개별 worker10s는 별개 실패다. Catalog의 반복
reserve/confirm COMMIT과 page churn은 측정됐고, 큰 payload의 저장 방식은
수리 가능한 구조적 기여 후보다. 두 테이블 rowid 전환은 그 물리 구조를
바꾼 코드이며 제품 지연의 root cause를 완전히 해소했다는 증거는 아직 없다.
Off에서도 실패하므로 tracing 단독 원인설은 배제하지만, tracing 추가분의
크기는 미확정이다. 원 제품 기준을 완화할 근거도 현재 없다.

**이전 저장 구조 수리:** 기존 설치 경로의 authority·codec·내구성은 유지하고
object/provisional 두 테이블을 ordinary rowid로 바꿨다. 당시 intent와 작은
ledger는 유지했으며,최신 intent 선택은 위 실제 분포 모델로 갱신했다. 새 소스 `fce4ceda`의 기본 bench Runtime+원4K no-run은
Root88152에서633.643s/exit0/drift0로 VERIFIED다. 같은 바이너리의 실제
Runtime IO/short-write/WAL3/3와 설정 거절1/1이 VERIFIED다. Root3897의
원4K Off는599.52s body/601.5s command/exit101로 FAILED다. Source4096은
28.292366167s에 Published했으나 owner row668이 ANN Compaction에서 개별
10s deadline을 넘겨 Blocked(ResourceLimited)가 됐다. 첫 거절은 deadline이며
reads16433/writes4248/bytes10267132/mutations3855로 수치 cap 초과가 아니다.
원 watchdog은 아직 만료되지 않았고613585ms 남아 있었다. 마지막 관측1034/
4096;후속 비원자 snapshot1083completed/3011pending/1claimed/1blocked,
provider1090unique/중복0/query0다. source/binary/config 사후 binding VERIFIED.
Q64/recall/reopen/delta/overlay는 NOT_RUN이다. 같은 소스 Record Root30747도
FAILED412.20s body/421.527s command/exit101다. Source4096은26.683951375s에
Published했고 마지막 관측506/4096,후속 비원자 snapshot511completed/
3581pending/2claimed/2blocked,provider515unique/516total/중복1/query0다.
operation27664는 Compaction 중 read_io.rs:171에서10s deadline을 감지했다.
reads5370/writes889/bytes4774369/mutations1130으로 수치 cap 거절이 아니다.
Record의 source/binary/config/trace path binding과 Off 불변성 VERIFIED.
126445321B raw의 Root7277 canonical 단일 parse는304.609s/exit0로 끝났다.
COMPLETE1245615events/health0/drop0/unwritten0/open0/leases0/orphans0/
truncatedfalse,worker516개가 모두 닫혔다. Compaction11개 중3개는error이며
해당 enclosing worker는error1/aborted2다. 세 Compaction wall은8.080/6.770/
8.350s이고,worker 전체는10.037/8.203/9.512s다. 서로 다른 종료 원인을 모두
10s deadline으로 합치지 않는다. 전체4K 제품·1GiB 충분성·overhead 수용은 미완료다.

**새 관측 결함 수리:** Compaction11개 모두 child span0이었고 root CPU도
미관측이었다. `begin_build_stage`가 span만 열고 원 WorkBudget의 diagnostic
context를 자식으로 바꾸지 않아 I/O가 SemanticPrepare의 sibling으로 기록됐다.
기존 scoped_diagnostic_context와 원 AttemptContext의 port/attempt/custody/
scratch를 재사용해 Compaction 아래로 연결하고,scope 해제 뒤 원 context에서
실패 기록·finish를 수행한다. 동기 Compaction만 CPU 관측 대상에 추가했다.
기존 budget/grant/deadline/권한·정책은 바꾸지 않는다. Python reader CPU14/14,
Core diagnostic29/29+scope2/2,실제 object-port marker를 포함한 Semantic carry
2/2가 VERIFIED다. Rust Core+Semantic no-run3m40s/exit0 및 hexagonal/format/
diff-check VERIFIED. 첫 Core+Semantic lib/tests strict는 release 전용 dense-lane
no-op의 `unused_async`1건으로 FAILED다. 해당 stub만 `Ready<()>`로 바꿔 동일한
즉시 완료·기존 `.await`를 유지했다. Root29697 재검증은359.438s/exit101로
FAILED다. bench integration `cancellation_inside_search`가 debug 전용
`test_support`를 import하는 기존 profile/fixture 불일치(E0432)다. debug/test의
실제 hold 경로·oracle를 바꾸거나 테스트를 제외하지 않는다. Root83063의
Core+Semantic production lib strict는119.751s/exit0로 VERIFIED다. 전체
lib/tests strict는 FAILED를 유지한다. 변경 뒤
이 진단 코드의 실제 capture는 당시 NOT_RUN이었고,이후11294467 Record의
완료된 canonical 결과를 위에 반영했다. fce4 evidence와 합산하지 않는다.
[이전 빌드 코호트](../../plans/oct-22-finalizing/VALIDATION.md#e07-rowid-original4k-20261028).
새 계측의 최초 실행은 원4K Record **RCA 전용1회**로 한정한다. 새 기본 bench
바이너리와 Runtime3·설정 거절1을 먼저 검증한다. 기존 실패 Off를 새 Record와
쌍으로 취급하지 않고, 이 단계의 full Off/overhead 수용은 NOT_RUN으로 둔다.
현재 소스11294467(2871 inputs)의 기본 combined no-run은 Root69439에서
1366.993s/exit0/source drift0로 VERIFIED다. Root23510의 Runtime3·설정 거절1은
모두 VERIFIED이고 source/binary 사후 binding도 유지됐다. Root48344가 같은
코호트 원4K Record RCA1회를 완료했으며 원1200초 watchdog 실패다. canonical
분석도 Root65271에서 VERIFIED로 종료됐다. 위 최신 귀속을 현재 판정으로 사용한다.
성능 코드를 바꾼 뒤 원4K Off→Record를 직렬 판정한다.
저장 모델의 WAL 감소를 제품 속도 개선율로 쓰거나 같은 실패를 수정 없이
반복 실행하지 않는다. 이전 b0 결과는 아래에 별도 보존한다.

### 당시 코드 RCA와 수리 선택 순서 — 이력

**확정된 구조:** `native_worker`는 원 grant deadline의 한 WorkBudget으로
submit/resume 전체를 실행한다. `ann_materialization::seal_delta`는 같은 owner
publication attempt에서 `consolidated_members`→capacity→Lance build→file-set
install→region append를 수행한다. `consolidation::include`는 delta와 선택 region의
member/artifact를 인증하고,base map diff 설치와 seal의 map/ref 대조가 이어진다.
따라서 일부 owner에는 ordinary owner보다 훨씬 큰 foreground carry가 붙는다.
일반64후보/최대8병합/512행 상한은 유한 작업량이지10초 완료 보장이 아니다.
이번 Compaction 거절은 Lance SemanticBuild 이전이며, Lance만의 느린 빌드로
단정할 수 없다. 검증 pass의 존재를 곧바로 제거 가능한 중복이라고 보지 않는다.

| 순서 | 수리 경계와 선택 조건 | 보존할 계약·판정 |
| --- | --- | --- |
| 1 | 완료된 귀속: Compaction54건 모두ok/38.309s. ordinary CoverageSave449.589s와 그 안 COMMIT300.164s를 우선한다. lock wait0.475s·spill0이므로 lock 분산/캐시 확대를 선택하지 않는다. | child context/CPU 관측은 VERIFIED. 중첩 비용을 더하거나 관측 수리를 처리량 개선으로 세지 않는다. |
| 2 | 실제 intent 분포의 page/누적WAL 감소에 근거해 intent payload table만 rowid로 전환한다. Catalog 기존 회귀에 old-intent-only 형식 거절·재오픈 byte equality·1500–3000B intent overflow0을 추가한다. | 명시 PK/NOT NULL/FK/reverse index·canonical JSON/digest·원 fee/transaction/내구성 불변. 제품 처리량은 NOT_RUN. VFS retirement 조회 생략안은 last-callback 경합 위험으로 제외한다. |
| 3 | Source owner spool의 용량 검사에서 반복 SQL prepare만 줄인다. 세 step-time PRAGMA의 program 재사용·현재 값 재실행,prepare-time page_size와 setter는 매번 새 준비를 유지한다. | Scratch31/31·strict·동적 cap/size 변조 회귀 VERIFIED. 원 Source caller와07bb 원4K 효과는 별도 판정한다. authority/value cache는 없다. |
| 4 | 국소 수리 후에도 원10초 안에 필수 인증·seal이 들어가지 않는다고 확인될 때만 bounded/resumable compaction으로 작업 단위를 분리한다. 현재 선택한 대규모 재설계가 아니다. | 기존 owner·COW·codec·설치 port 재사용,준비 결과의 정확 base/recipe/member 재검증,원 CAS 전 무노출,취소/join·원 grant 종료·GC pin 수명을 명시한다. stale 후보는 거절하며 원 owner deadline을 재시작하지 않는다. |
| 5 | 성능 코드와 영향 회귀를 합류한 뒤 원4K 전체를 판정한다. | Source30s/1Mreads/512MiB·worker10s·watchdog1200s 유지. 4096 materialization→Q64/K10/recall≥.95→cold reopen/delta/overlay까지 통과해야 제품 실패를 닫는다. |

**미확정:** ordinary Catalog 안 각 ledger의 제품 page 증폭 비중,현재 intent 수리의
실제 처리량 효과와 tracing 추가 비용. Compaction 내부의 candidate/artifact/COW 세부
귀속은 전체 병목 수리의 선행 조건이 아니다. Off/Record는 처리량·종료 지점이
달라 overhead 비율을 계산하지 않는다. 비용이 겹치는 hold/COMMIT/VFS wall을
합산하지 않으며 checkpoint는 예산 소진을 발견한 지점으로만 해석한다.
E04/PHYSICAL과 별도 병렬 구현 티켓을 만들지 않고 E07 단일 실행 큐를 유지한다.

**독립된 처리량 축:** 개별10초 거절을 없애도 전체1200초 수용이 자동으로
성립하지 않는다. 4096/1200≈3.41 completed owners/s는 전체 시간을 모두
materialization에 쓸 때의 명목 하한이며 Source·검색·재오픈 시간만큼 더 높은
처리량이 필요하다. ordinary owner의 반복 durable 설치·CoverageSave 비용도
같이 관리한다. 조기 실패 Off/Record의 관측 수를 시간으로 나눈 값을 steady-state
성능이나 tracing overhead로 쓰지 않는다. 따라서 합격 조건은 worker tail과
전체 완료 시간 둘이며 carry 국소 수리만으로1200초 통과를 예고하지 않는다.

**이전 b0 판정: FAILED.** b0d51005 기본 bench 빌드526.657s/exit0/source drift0
후 원4K Off를 한 번 실행했다. Source4096은23.885721792s에 Published했으나
원1200s watchdog에서 초기 worker 완료를 기다리다 실패했다. 마지막 진행
2867/4096,후속 비원자 조회2903완료/1192pending/1claimed/0blocked다.
provider2922unique/중복0/query0,Q64·recall·reopen·delta·overlay는 NOT_RUN이다.
body1232.76s/exit101,source·binary 사후 binding과 config는 VERIFIED다.
원30s/1Mreads/512MiB와 lease2s/dispatch2/batch1을 유지했다. tracing Off에서도
실패했으므로 tracing만을 원인으로 볼 수 없다. 이전 source 결과와 개선율을
계산하지 않는다. 단건 sync 수리만으로는 처리량 문제를 닫지 못했다.
[현재 코호트](../../plans/oct-22-finalizing/VALIDATION.md#e07-scalar-shadow-original4k-20261028).
같은 b0 binary Record도 FAILED다. Source 요청이30.007668833s에 deadline을
넘겼고 original operation은 Pending/provider0이었다. body82.02s/exit101,
outer84.896s/source·binary·config binding VERIFIED다. worker 진입 전 실패이므로
Off의 worker 실패와 합치지 않고 tracing overhead 비율도 계산하지 않는다.
새2.411MB trace의 canonical COMPLETE23830events/loss0/health0은 이 실패
구간의 기록 완결성만 증명하며 전체4K·Q64·원1GiB 충분성은 NOT_RUN이다.


**이전 Record 판정:** dc622f16 원4K Record는 Source4096을20.802232875s에
Published했으나 원1200s watchdog에서 초기 materialization 미완료로
FAILED다. 마지막 진행 관측2784/4096, 이후 비원자 public snapshot은
completed2813/pending1281/claimed2/blocked0다. provider2816unique/중복0,
query0이며 Q64·recall·reopen·delta·overlay는 NOT_RUN이다. Source30s 실패와
worker 처리량 실패를 같은 원인으로 합치지 않는다. 이 실행의 source/binary
사후 binding은 VERIFIED이며, shared-host 단일 Record이므로 정식 성능과
tracing overhead는 NOT_RUN이다.

### 구조적 수리와 남은 인과 검증

직접 실패는 초기 worker 완료를 기다리던 원 watchdog 만료다. 03d3 Off의
Source30.004s 실패와 같은 소스 Record의 조기 Blocked는 역사 증거로
보존한다. 새 실행은 blocked0이며 같은 실패가 반복됐다고 쓰지 않는다.
경합 조건이나 완료량이 다른 실행 사이의 wall/spill/쓰기 총량으로 개선율을
계산하지 않는다. 특히 단 한 건의 lease 만료로 전체 처리량 부족을 설명하지 않는다.

| 문제 | 확인된 사실 | 미확정 원인 | 다음 판별 / 수리 선택 |
| --- | --- | --- | --- |
| Source30s | 같은 b0 Off23.886s Published/Record30.008s deadline·Pending·provider0. 원30s/1M reads/512MiB 유지 | host 경합과 tracing 추가분의 개별 기여 | 새 canonical Source phases/SQL을 분해. 단일 경합 비교로 overhead%나 원인 단정 금지 |
| Postings 두 번째 cursor | term별 SQL을64-term/64-row PK range로 통합.고정129 singleton terms에서 SQL129→5회,신규7/7 VERIFIED | 원4K 전체 지연 개선량 | 독립 RootCursor·원 ordinal/CAS/digest/EOF 유지.실제 query/row/byte 과금 유지;logical read 수를 SQL 감소율로 환산하지 않음.영향 회귀 후 원4K 판정 |
| worker1200s | 최신 Off2867/4096 관측 후 watchdog FAILED. 후속 snapshot2903완료/blocked0, provider2922unique/중복0 | 건별 필수 Catalog/physical 설치·ANN seal·readiness 경합의 전체 기여 | 기존 capture의 닫힌 worker/child 구간을 같은 source 안에서 분석. 초기·중간·후기 같은 크기 구간별 비용·횟수·분포를 비교하고 실제 지배 hunk만 수리 |
| worker10s | 새 fce4 Off는 원 watchdog 전 ANN Compaction의 deadline 거절로 FAILED. managed_install.rs:257은 child 사전검사 뒤 취소를 감지한 위치 | carry 설치/읽기/Catalog·sync·host tail의 기여와 실제 선택 rows/regions | Compaction은 일반 후보64·병합8·carry512행 경계와 checkpoint가 있어 무한 루프로 단정하지 않는다. 같은 소스 Record의 닫힌 Compaction 자식 비용을 판별하고 실제 지배 경계를 수리 |
| 갱신 lease 만료 | canonical PARTIAL에 not_claimed62/lease_expired1. 만료 건의 lock wait2.015070417s/hold0.624417ms, 기존 lease보다692ms 늦음 | 전체 처리량과 이1건의 기여; 누락 구간의 추가 실패 | 동일 holder stage_work가 Catalog lock2.358041042s 보유. native WAL sync0.651705916s와 parent fsync1.703768459s가 포함됨. 정확한 이1건의 대기는 확인했으나 전체 worker 원인으로 승격하지 않음 |
| 관측 테스트 strict | 최신 Catalog+Storage lib/tests strict VERIFIED. 과거 3패키지 broad strict의 Runtime lib-test43·risk-suite3 지적 FAILED는 별도 | 현재 전체 Runtime strict 결과;제품 성능 결함과 별개 | E02 admitted-publish/shared fixture는 원 owner 유지.새 focused 결과를 whole-strict green으로 승격하지 않음 |
| tracing overhead | Off에서도 worker 미완료, 단일 Record의 시간은 overhead가 아님 | 계측 추가분의 크기 | 동일 binary/input의 quiet Off/Record3쌍; 경합 진단을 정식 수용으로 승격하지 않음 |
| cache/종료 계약 | 명시 ledger 예약·readback·checked close/VFS retire/alias 집중 회귀는 기존 범위 VERIFIED;3b13 Source8MiB readback·03d3 Off 정상 종료 | 실제 성능 효과·전체 RSS/운영 수용 | 완료된 계약을 재구현하지 않고 영향 범위와 남은 성능/운영 gate만 검증 |
| 전체 Record 용량 | 실제1073741068B에서 canonical PARTIAL;queue dropped0/unwritten163069/open5/health0 | 전체4K·Q64까지 담는 데 필요한 축소율과 계측 CPU 추가분 | 원1GiB 안에서 공통 event envelope/payload tuple을 쓰는 최소 v5 wire 수리. 신원·순서·모든 값 보존, producer/schema/Python/Rust reader를 함께 변경. 충분성·overhead는 별도 검증 |

기존 capture의 명시적 관측 보고서는 worker root2795개 중 닫힌 subtree2794개를
선택하고 열린 subtree1개를 제외했다. 이들의 worker wall 합1071.209s 안에서
CoverageSave2793개/448.403s, 그 하위 Catalog hold5586개/373.191s와
COMMIT5586개/300.646s를 확인했다. 중첩 값을 합산하지 않는다. CoverageSave의
설치 입력54089개(중앙20/worker)는 신규 map node 수가 아니다. 해당 Catalog
hold의 cache writes1415245/misses1935004/spills0와 SQLite 요청 write4215205회/
11499740984B는 page churn 증거이며 실제 device bytes가 아니다. VFS write
admission111.436s/lower envelope33.697s는 CoverageSave 안의 VFS write
callback 관측값이다. 현재 집계는 COMMIT 내부 귀속을 직접 증명하지 않는다.

CoverageSave에서 checkpoint marker1587개/60.618s를 관측했다. 해당1587worker에
main-file write1384011회와 main sync48.595s가 모두 관측됐고, marker가 없는
1206worker에는 없었다. marker는 backfill 종료까지이며 뒤의 main sync를
포함하지 않는다. 이 worker 범위에는 explicit reclaim checkpoint lane26이
없고 호출 경로가 auto-checkpoint 기여를 지지한다. Root40800의 첫100/중앙100(0-based1347:1447)/마지막100worker 중앙 wall은
227.790/444.637/243.396ms이므로 단조적인 후반 열화로 설명하지 않는다.
spill0에서 cache 증설, 또는 WAL peak/예약·복구 계약 없는 checkpoint 상향을
선택하지 않는다. provisional/permanent edge 공유도 현재 FK/digest/GC 계약의
교체가 필요하고 table별 기여가 미측정이므로 최소 수리에서 보류한다.
원 trace에는 개별 CoverageSave CPU가 이미 있다. Root40800은 같은 SHA의
재분해를277.77s/exit0으로 마쳤다. CoverageSave2793개 모두 개별 CPU를
관측했으며 wall448.402706059s/동일 스레드 CPU229.601775228s(51.204%)다.
wall-CPU218.800930831s는 I/O·잠금·스케줄링 등이 섞인 값으로 fsync-only가
아니다. CPU에도 계측과 호출된 저장 경로가 포함되므로 전부 SQL로 귀속하지
않는다. 첫/중앙/마지막100의 CoverageSave CPU 중앙값은
60.226/97.700/66.330ms이며 각 구간 표본100/99/100개다. 전체 worker의
CPU나 미관측 구간을 추정하지 않는다. 기존 보고서에서 CPU를 뺐던 것을
계측 누락으로 오인해 새 계측을 추가하지 않는다.

### 당시 해결 순서와 판정 기준 — 이력

아래 closed-workers·v5 wire 구현과 영향 검증 합류는 완료 이력이다.
새로운 parser/recorder 수리나 과거 raw 재분석을 배정하지 않는다. 현 잔여는
[단일 종료 순서](E07-WAVES.md#e07-closeout-plan-20261030)만 따른다.

1. **기존 실패에서 처리량을 분해:** 새 제품 실행 없이 dc622f16 capture의
   완결된 worker와 그 child만 진단한다. 준비·reserve·물리 sync·confirm·
   stage_work·publish·주기 seal별 inclusive union/횟수/분포와 readiness RPC
   경합을 구별한다. 열린 span·부족한 counter·미기록 구간은 미관측으로
   남기며 PARTIAL을 COMPLETE로 바꾸지 않는다. 기존 worker index는
   complete-only로 유지하고, 별도 `closed-workers`가 healthy canonical
   PARTIAL의 관측된 닫힌 subtree만 보고하도록 구현했다.
2. **확정된 관측 결함부터 코드 수리:** 아래 v5 wire 계약을 고정하고
   Rust recorder / schema·Python consumer / Rust child·직접 raw reader에
   비중복 병렬 소스 수리를 적용했다. 한 내부 IR·capture128bit ID·순서·counter·
   terminal·footer를 유지한다. 공통 tuple의 실제 크기와 독립 gold로
   축소량을 확인하며 단순 필드 축약만으로 전체1GiB 수용을 보장하지 않는다.
3. **측정된 worker hunk만 수정:** 기존 batch/prepared 경로를 재사용하고,
   reserve→durable write→confirm과 매-write 권한·수명을 유지한다. owner
   member+coverage2-object batch와 COW DAG256-object batch는 이미 있어
   재개발하지 않는다. provider artifact는 stage_work 전에 durable/registered여야
   하므로 후속 metadata와 단순 병합하지 않는다. seal/SQL/인증의 반복 비용은
   실제 분포로 확인하기 전 확정 병목으로 부르지 않는다.
4. **검증 합류:** 독립 wire gold·malformed/mixed capture·cap/partial·구버전
   artifact·Rust/Python parity와 바뀐 worker 범위의 fault/회귀·strict를 Root가
   직렬 실행한다. 이후 새 소스 원4K Off에서 원 Source cap과 전체1200s
   Q64/recall/reopen/delta/overlay를 판정한다. Record와 overhead 수용은 같은
   binary/input에서 따로 판정한다. 소스 변화 없는 실패 재실행은 하지 않는다.

### 당시 해결안의 확정 범위와 코드 선택 — 이력

- **후속 RCA·코드 판정:** 심볼 보존 진단 source `1ebf2cd6`의 실제 worker
  5초 sample에서 `submit_authorized → ImmutableObjectPort::install`과
  `prepare_and_commit → AttemptContext::put → FileObjectStore::install` 단건
  경로를 확인했다. 기존 batch는 이미 plain `fsync`와 동일 장치의 마지막
  `F_FULLFSYNC` 한 번을 사용한다. 반면 새 scalar 설치는 objects directory·
  file·shard의 `sync_all`마다 macOS `F_FULLFSYNC`를 수행했다. 함수 sample은
  실행 경로 증거이며 sample 횟수는 CPU 비율이나 전체 시간 기여가 아니다.
- **선택한 성능 수리:** managed/macOS/new-file/same-device scalar에 기존
  batch와 같은 directory/file `fsync → publish → shard fsync → 원 writer FD의
  F_FULLFSYNC` 순서를 적용했다. per-install helper로만 분리하며 shared
  authority/cache/새 IR은 추가하지 않았다. 실제 writer 장치가 다르면 원래
  full-sync 경로로 복귀한다. 최종 barrier 뒤에만 ticket 정산과 Catalog confirm을
  수행한다. 기존 파일·collision·unmanaged·non-macOS와 원래 scalar fee/child
  검사는 유지한다. one-element batch 전환은 추가256B fee/메모리 lease와 오류
  순서 변경 때문에 제외했다. 원4K Off 재검증은 FAILED이며 단독 수리로 충분하지 않았다. 이 수리의 독립 성능 효과는 미측정이다.
  원 실패 종료의 cumulative object IO도 file sync36.384s/directory sync61.611s/
  barrier26.321s인 반면 reserve246.948s/confirm318.858s였다. 이는 Source·worker·
  종료까지 포함한 전역 호출 합이며 worker별·CoverageSave 값과 합산하거나
  같은 시점 분모로 비교하지 않는다. scalar flush가 실제 수리 대상이어도
  Catalog 비용까지 제거하거나1200s 통과를 보장하는 단일 원인 수리는 아니다.
- **함께 수리한 P1 확인 결함:** 같은 객체의 permanent/provisional 행이 동시에
  남으면 기존 unique confirm은 valid shadow를 조용히 채택하거나, malformed
  shadow를 앞선 객체 등록 뒤에야 거절했다. durable prefix는 rollback으로
  막혔으나 전체 사전검증이 아니었다. provisional payload/edge 인증 뒤 permanent
  부재를 확인하고, unique 배치는 같은 transaction에서 검증된 insert-only helper를
  사용한다. duplicate/generic 경로는 기존 재검사를 유지한다. 원본 두 반례
  RED2/2 후 Catalog 전체277/277 VERIFIED다. 여기서 사전검증은 첫 객체 등록·
  이관 전이며 자식 GC adoption까지 모든 SQL mutation이 없다는 주장은 아니다.
- **국소 검증:** Storage151통과/1기존 subprocess fixture ignored. 이후 두 테이블
  수리에서 Catalog279/279,마지막 test guard 수정 뒤 영향2/2와
  Catalog+Storage lib/tests strict VERIFIED다. 기존30/후속18지적은 수리 전
  실패 이력이며 현재 Catalog strict 잔여로 재등록하지 않는다. 변경 macOS 코드의
  테스트 통과를 Linux compile·전원 장애 내구성·전체4K 성능 통과로 세지 않는다.
- **적용한 수리:** 원1GiB를 유지하는 V5 wire와 닫힌 worker 진단 도구.
  고정 실제 표본 크기49.13% 감소와 집중 회귀는 VERIFIED지만 새 전체4K
  lossless/속도 수용은 NOT_RUN이다. 관측 수리를 처리량 수리 완료로 세지 않는다.
- **우선 수리 경계:** 기존 CoverageSave→managed reserve/confirm→Catalog
  경로다. 동일 스레드 CPU51.2%와 page write/COMMIT 비용이 함께 있어
  디스크만의 문제, cache 부족, lease 값만의 문제로 닫을 수 없다. 현재 span은
  SQL lane별 wall을 주지만 cache write의 table/SQL 귀속과 CPU stack은 없다.
  현재 함수별 sample은 위 scalar sync 후보를 확인했다. Catalog B-tree의
  table별 CPU/쓰기 기여는 여전히 미확정이다. 원래 authority와 오류 순서를
  유지하는 실제 반복 계산·쓰기만 줄인다.
  그 귀속 전에는 edge schema 교체·checkpoint 정책 변경을 확정안으로 넣지 않는다.
- **새 Source 관측:** 같은 소스 Record는 NativeSourceApply10.983s/CPU6.425s,
  그 뒤 LexicalBuild13.369s/CPU7.518s,NativeCoverageScan3.723s/CPU1.441s를
  기록했다. LexicalBuild 안 Postings11.127s/CPU6.281s는 중첩 값이다.
  Source request 하위 COMMIT38개/7.488s,VFS write119761회/요청315530720B,
  admission5.291s/lower envelope1.466s를 관측했다. COMMIT·phase·VFS 합을
  더하지 않고 device bytes 또는 CPU 함수별 기여로 환산하지 않는다.
- **새 구조적 후보 — payload-heavy table 저장 방식:** 종료된 소유 데몬의
  main/WAL을 raw로 보존했다. 최종 경로가 제거돼 WAL pairing은 미검증이므로
  WAL을 붙이지 않고 main checkpoint prefix만 읽었다. SQLite integrity ok/FK0,
  44036096B/10751pages다. object12149행 payload6642065B와 intent2944행
  payload4090531B에서 overflow4155pages/17018880B를 관측했고 그 안의
  unused10249647B를 확인했다. 이는 최종 logical Catalog나 worker 전체 DB가
  아니며 누적 write의 table별 기여율은 아니다.
  현재 WITHOUT ROWID는 index B-tree로 payload를 저장해4KiB page의 local
  cell 상한1002B를 적용한다. 큰 payload 행의 overflow와 좁은 fanout을 만드는
  구조다. [SQLite 공식 권고](https://www.sqlite.org/withoutrowid.html)도 큰
  BLOB/행에는 ordinary rowid table을 비교하도록 설명한다.
  원 행12149개/동일 순서/원 operation·fence와 reverse index/256-row
  reserve→confirm 모델에서 baseline14935WALframes/9067pages,
  object+provisional만 rowid12595frames/7983pages,세 table 모두 rowid는
  12486frames/7857pages였다. install는 confirm에서 삭제되며 원 payload/
  도메인별 digest/중간 install witness·PK/NOT NULL 거절·FK0/integrity를
  유지했다. 이 합성 분포에서 intent의 추가 이익은109frames였다. 다만 모델은
  object12149행의 payload를 intent에도 복사했다. 실제 prefix intent는2944행/
  4090531B로 평균 약1389B이며 object 평균 약547B와 다르다. 따라서109frames는
  실제 Source의 intent 기여가 작다는 증거가 아니다. 큰 복합 PK의 autoindex와
  추가 탐색을 고려한 보수적 범위로 **object/provisional 두 테이블만 적용했다.**
  당시 저장 방식의 최적성은 미검증이어서 intent와 작은 ledger를 유지했다.
  이후 실제 intent2944행의 원 key/payload/fence 모델에서 page·누적WAL 감소를
  확인해 intent만 추가 전환했다. 위2026-10-29 수리 선택이 현재 범위이며,
  아래 수치는 앞선 합성 모델의 이력이다. 작은 ledger는 유지한다.
  모델은 Python SQLite3.54/auto-checkpoint0의 순수 저장 비교다. 제품3.53.2/
  실제 VFS·원auto-checkpoint1000·공개 설치/GC/발행·속도·lookup 비용을
  검증하지 않았다. 모델 차이를 제품 성능 개선율로 쓰지 않는다.
  원 canonical JSON/digest·명시 NOT NULL/PK/FK·reverse index와 reserve/
  durable write/confirm 순서는 유지한다. 현재 코드의 정확한 schema verifier가
  기존 WITHOUT ROWID root에 StateRootFormatUnsupported를 반환하고 새 root
  재빌드를 요구한다. undeployed breaking-first 범위이며 자동 migration은 없다.
  실제 기존 root는 삭제·변환하지 않았다. old public-open 거절과4KiB 중간
  metadata의 overflow/원 bytes를 확인하는2개 회귀를 추가했다. Root20415의
  Catalog279/279는 VERIFIED다. 이후 test guard의 명시적 해제1건을 수리하고
  Root49393에서 영향2/2와 Catalog+Storage lib/tests strict를 VERIFIED했다.
  변경 뒤 실제 Runtime IO/WAL3/3는 VERIFIED이며 새 원4K Off의 Source4096은
  28.292366167s에 Published했으나 ANN Compaction의 개별10s 거절로 실패했다.
  같은 소스 Record도26.683951375s에 Source 게시 후 Compaction deadline으로 실패했다.
  새 성능 효과·Q64/재오픈·통합·출시는 아직 미수용이다.
  다음 RCA 실행은 새 child-context/CPU 소스·설정을 고정한 기본 bench 빌드에
  Runtime IO 회귀와 원4K target을 합류하고,실제 경로 회귀→Record RCA1회를
  Root가 직렬 판정한다. 성능 코드를 수리한 뒤 원4K Off→Record를 수용한다.
  lossless1GiB·overhead·RSS/운영 수용은 각 원 gate를 별도로 유지한다.
- **기준 변경 제외:** Source30s/reads1M/512MiB,전체watchdog1200s,lease2s와
  batch/inflight/provider concurrency1,dispatch2,FULL/fullfsync/parent sync를
  유지한다. Source의 이번20.802s 통과는30s 완화 근거가 아니다.
- **닫는 순서:** 함수/hunk 선택→그 경계의 독립 fault/회귀와 strict 합류→
  같은 binary/input의 원4K Off/Record와 Q64/recall/reopen/delta/overlay→
  paired overhead/RSS/운영 수용. 동일 실패의 전체 테스트 반복을 코드 수리로
  대체하지 않는다. Catalog/PHYSICAL의 기존 소유 hunk는 중복 배정하지 않는다.

만료 건의 stage_work COMMIT 중 같은 Connection mutex를 풀 수 없다.
별도 SQLite 연결도 WAL 단일 writer를 제거하지 못하고, 갱신 우선순위도 이미
수행 중인 fsync를 선점하지 못한다. 같은 handle에서 과거 parent sync가
성공했다는 bool만으로 후속 sync를 생략하는 안도 제외한다. 현재 FD/inode
검사는 같은 inode의 unlink/relink 뒤 이름의 내구성을 증명하지 못한다.
이를 줄이려면 모든 namespace mutation·외부 변경 배제·재시작을 포괄하는
별도 generation/debt 증명이 필요하므로 현재 국소 수리로 선택하지 않는다.
lease2s·dispatch2·원자적 게시·FULL/fullfsync·parent sync는 유지한다.
관측된 만료 거절은 fencing을 지킨 결과이며, 잘못된 claim 승인이나 clock/fence
불일치의 증거는 아니다. 장기 I/O의 최대 지연을 보장하지 않은 채2s 갱신의
절대 성공을 약속하지 않는다. [SQLite WAL의 writer 직렬화](https://sqlite.org/wal.html).

최신 국소 검증은 Core/Catalog renewal6/6·recorder3/3·Python410/410 및
영향6개 production library의 strict Clippy VERIFIED다. 이전 Runtime
lib-test/risk-suite를 포함한 whole-strict FAILED는 별도다.
[명령·범위](../../plans/oct-22-finalizing/VALIDATION.md#e07-renewal-refusal-20261028).
dc622f16 원4K body와 lossless capture 수용은 FAILED다. 후속 v5 wire와
명시적 observed closed-worker 진단 코드는 적용했다. Python957통과/기존2skip,
CPU 표본 보존 영향43통과, recorder34통과와 별도 Runtime/harness18통과,
변경3개 production library strict VERIFIED다. 후속 Root6359 심볼 보존 빌드는
exit0/826.757s,source drift0으로 끝났다. 해당 pre-P1/pre-scalar source의 작은
UDS query1/1과 진단 prefix1/1은 실제 통과했다. prefix는 Source4096을17.976s에
게시하고 완료129개를 관측했으며 provider201unique/중복0으로 종료했다.
전체4K/Q64 수용이 아니다. V5 capture는 canonical COMPLETE/health0,
310803 accepted=written/dropped0/unwritten0/open0이며 닫힌 worker201개다.
별도 진단 profile의 이 결과를 원/default 성능 비교에 쓰지 않는다. 원4K·
lossless 전체 Record·paired overhead는 NOT_RUN이다. 선택한 scalar 수리와 P1의
검증은 [후속 결과](../../plans/oct-22-finalizing/VALIDATION.md#e07-scalar-sync-shadow-20261028)로 분리한다.
[후속 코드·검증 범위](../../plans/oct-22-finalizing/VALIDATION.md#e07-v5-wire-closed-workers-20261028).
[현재 소스 실행](../../plans/oct-22-finalizing/VALIDATION.md#e07-range-renewal-original4k-20261028).

30s가 불가능하거나 테스트만 과도하다는 증거는 없다. 입력은 이미32개씩
staging하고 mutation도128개/page로 전송하지만,마지막 SourceAdvance의
단일 원자적 게시는30초 제품계약이다. 이를 여러 게시로 나누거나 timeout을
늘리는 것은 수리가 아니라 계약 변경이다.이전26.645s 단일 성공은 가능성의
해당 실행 증거다. 현재20.802s도 이 실행의 통과이며 안정적 지연 수용은 아니다.

다음 코드 선택은 기존 prepared-statement 재사용처럼 인증 결과를 캐시하지
않고 반복 준비 비용만 줄이는 경계부터 검토한다. 같은 트랜잭션의 namespace
조회가 반복된다는 사실만으로 매-child binding 검증을 생략하지 않는다.
직전 실패의 owner 준비4.702s도 wall envelope이므로 CPU·blocking·scheduling 기여를
분리하기 전 파일 권한 검사나 payload 검증을 제거하지 않는다. 원4K Record
실패 후에는 새 제품 실행을 겹쳐 시작하지 않으며 정적 수리 결정을 먼저 한다.

추가 current-code 확인:CoverageMap은 이미64개/8MiB leaf batch를 쓰고,
초기 owner/missing은 shared_pending_roots 한 개를 단 한 번 apply_stream해
공유한다. SourceMap/CoverageMap/Postings는 같은 AttemptContext.maps의
PersistentMapPort로 연결된다. FileObjectStore::apply_stream에는 전역 정렬·
중복·original CAS 검증과 empty-root의 sorted radix frontier가 이미 있다.
새 bulk builder·큰 batch·두 root 공유를 추가하는 안은 중복이므로 제외한다.

조건부 구조 수리는 기존 MapPort 내부에만 둔다. CPU/내부 SQL 측정에서
MapScratch edit 복사가 지배적일 때 `MapRoot(None)`의 검증된 정렬 cursor를
미게시 staged radix 생성에 직접 연결해 EditSpool→MapScratch edit 복사와
재스캔을 줄이는 안을 검토한다. 전체 EOF/order/shape/empty CAS 확인 전에는
reachable node 설치·root 게시를 하지 않고,메모리/디스크 admission과 실패
cleanup과 독립적인 late-corruption/EOF 거절 또는 동등한 인증 seal을 보존한다.
현재 비용은 scratch복사/radix/설치를 분리하지 않으므로
이 안은 미선택이며 구현 완료나 예상 단축률을 주장하지 않는다.

### 최소 변경 후보와 선택 조건

| 후보 | 실제로 줄이는 동작 | 보존할 조건 / 선택 한계 |
| --- | --- | --- |
| Owner scratch bounded group | 기존1-owner 예약을 최대21 owner/63 demand로 묶음.4096 add에서 예약4096→최소196회 가능 | 같은 private DB/transaction·정확한 row bytes·원 SQL 순서·typed refusal·cap tamper 전 SQL0·실패 후 retry0·전체 seal 전 게시0.기존 PathKey의 유료 Temporary 복사 사용,최대 path backing172032B.보수적 선예약은 near-cap 성공 집합을 좁힐 수 있어 prefix 계획과 독립 gold 없이 선택하지 않음 |
| Postings cardinality page —03d3 strict·148 body VERIFIED / 원4K FAILED | 진단 fixture의8194개 term별 SUM 실행을 기존64-term page의 최대129개 범위 집계로 축소 | key/base 인증 후 원8194 logical read 선차감·범위/key/shape/EOF 확인·별도 edit-stream 전체 인증을 보존.줄이는 것은 SQL execute/seek뿐이며 row scan·COMMIT은 유지;제품 시간 기여율 미측정 |
| Postings 두 번째 독립 cursor — 적용/영향152/152 VERIFIED | 두 번째 edit stream의 term별 조회를64-term/64-row bounded PK range로 대체.고정129 singleton gold의 SQL129→5회 | 첫 RootCursor와 둘째 패스의 EOF/order/CAS/shape·실제 행별 예산을 유지.첫 compact 준비까지 제거했다고 주장하지 않으며 이 수리를 다시 배정하지 않음 |
| 명시 writer-cache 비교 | 같은 source에서 ledger 예약을 수반한 작은 cache 변경으로 miss/spill/write 횟수 비교 | 기존 cache 계약을 재구현하지 않음.실제 headroom·readback·제품 bytes/time·RSS를 함께 확인;soft cache target은 RSS 상한 아님.auto-checkpoint 변경은 WAL peak/정리 예산을 별도로 바꾸므로 미선택 |

공통 SQL 감사:owner DML·owner cursor·MapScratch는 이미 prepare_cached다.
4개 read PRAGMA를 일괄 캐시하는 안은 제외한다.SQLite vendor 구현상
page_count/max_page_count/page_size는 자동 재준비하며,freelist_count만
실제 VM 재사용 이득이 있다.작은 준비 비용을 측정 없이 주 해결책으로 삼지 않는다.
03d3의 Postings 변경은 SQL 실행 수를 줄이는 구조적 수리로 선택했다.
실행 시간 개선은 아직 입증하지 않았으며 나머지 후보는 미선택이다.
기존 전체 DF/root·예산·reopen 집중 oracle은 통과했으나 actual Source30s는
실패했다.이 구분을 유지하며 다음 측정 전 추가 cache/batch 변경을 하지 않는다.

현재 고정4096 Source-only fixture는 CreateWorkspace→Reconfigure 뒤 첫 Source다.
기존 owner/path root는 모두 `MapRoot(None)`이며 scalar lookup은 물리 노드
읽기 없이 종료한다. 이 경로에 기존 `scoped_reads`를 추가해도 제거할 물리
재읽기가 없으므로 현재 실패의 수리안으로 채택하지 않는다. 비어 있지 않은
입력 manifest의 paged scan과 source record 읽기는 별도 측정 대상이다.

Source-only 진단과 원제품 full4K는 같은 `stage_quality_sources`,4096개 입력,
provider stub,owned-pump 설정과30초 요청 예산을 사용한다. 제품에만 있는
overlay 설정,진단 Record/제품 Off 모드,진단180초/제품1200초 전체 watchdog은
별도다. 진단의 Source 통과를 원제품의 Source 지연이나 후속 owner4096/Q64/
재오픈/증분/overlay 통과로 확대하지 않는다. 설정 차이가 현재 실패 원인이라는
증거는 없다. 두 경로 모두 기존 제품 SourceAdvance를 호출한다.

실패 stage는 취소를 관측한 위치이며 주 병목의 귀속이 아니다. 초기 Source의
coverage 이전 root 두 개도 비어 있어 해당 scalar lookup은 물리 읽기가 없다.
AppliedSource 경로는 sealed mutation과 실제 owner-root diff를 검증하며
전체 corpus snapshot을 다시 만들지 않는다.64행 diff page마다 공통 조상을
다시 여는 후보는 있지만,기존 scoped_reads.visit_diff도 원 저장소에 위임하므로
wrapper 추가만으로 이 비용을 제거하지 못한다. 현재 성공 구간1.819s만으로
그 인증 검증을 생략하거나 CoverageScan을 주 원인으로 단정하지 않는다.

VFS 감사 후보도 인과 증명과 구분한다. AfterWrite의 lower xFileSize와 같은
FD witness의 fstat는 중복 syscall 후보지만,오류 주입·관측 시점이 달라
값 치환은 기존 event 의미와 완전히 같지 않다. 쓰기 callback의 retirement
검사를 단순 생략하는 안은 close와 callback이 겹칠 때 마지막 회수 기회를
놓칠 수 있어 채택하지 않는다. 원 권한/수명 계약을 보존하는 수리만 허용한다.

읽기 전용 재감사에서 불필요한 전체 corpus rebuild, owner마다의 전역 스캔,
제거 가능한 내구성 COMMIT 중복, 새 cache 수명의 P0–P2 결함은 확인되지 않았다.
이 결과는 성능 원인이나 원4K 수용을 증명하지 않는다. Source30s 및 worker1200s는
원래 제품 조건으로 유지하며, 아직 측정하지 않은 원인에 맞춰 알고리즘을 바꾸지 않는다.

구현된 코드 수리는 다음 하나의 연결 계약이다.

- Catalog는 명시 KiB 정책과 정확히 같은 크기의 기존 MemoryLease를 받는다.
  파일 생성 전 값을 검증하고,실제 연결의 page/cache/spill/autocheckpoint를
  전후 readback한다. WAL/FULL/fullfsync/자동 checkpoint/spill은 유지한다.
- Native는 모든 실행 class floor와 provider/read-session 예약을 보존한 뒤
  연결 수명 전체의 retained cache를 같은 ledger에 과금한다. 부족하면 상태
  생성 전에 거절한다. 제품 JSON에는 catalog_writer_cache_kib가 필수이며
  자동 추정·default는 없다. 제품/scale의8192KiB는 비교 후보일 뿐이다.
- Native 종료의 기존 Drop 의존을 명시 checked close로 바꾼다. 외부 Catalog
  소비자가 남으면 NotReady이고,retirement handle로 해제 후 재시도한다.
  최종 성공은 FD drain만으로 판단하지 않고 기존 VFS retire_checked의 실제
  scope 소멸까지 확인한다. 남은 native handle은 NotReady와 동일 재시도 상태를
  보존한다. close 실패 때 원 connection+I/O authority+lease를 보존한다. 최종 실패
  fallback도 이 소유권을 함께 보존하므로 거짓 메모리 환급을 만들지 않는다.
- 관측은 시작 시 실제 설정5개를 별도 root로 배출한다. 부분 캡처는 부분으로
  보존하며,누락 counter를0이나 정상 설정으로 복원하지 않는다.

현재 구현/정적 통합과 후속 focused 검증 결과는
[단일 기록](../../plans/oct-22-finalizing/VALIDATION.md#e07-admitted-writer-cache-20261028)에만 갱신한다.
e4의 owning349/349와 Native8-target compile,104의 strict·73/73(공개 lifecycle/
resource/pump4개 포함),47의 보강 충돌1/1과 제품 설정 거절1/1은 완료됐다.
이전47의 Source-only 성공 뒤 원4K Off/Record는 모두 실패했다.
후속9617의 owner batch는 focused 검증을 통과했지만 Source30s는 실패했다.
ca176의 read 중복 수리 후 Source도 실패했다. 후속 CPU 계측을 집중 검증한
뒤 같은 제품 경로를 Root 단일 큐에서 판별한다.
같은 논리 작업 완료와 spill/WAL/callback 변화를 평가한다.
반응이 없으면 필요한 metadata/edge SQL 비용을 분석한다. timeout 연장,
내구성 완화,root/auth 검사 캐시,중복 인덱서 추가는 해결안에 포함하지 않는다.

비교의 분모도 고정한다. 이전 FAILED 캡처의 부분 BuildPrepare와 후속 완료
요청의 전체 write/spill 총량을 직접 비교해 개선율을 계산하지 않는다. 두 실행에서
입력 수·완료 여부를 확인한 동일 phase/batch만 비용 비교에 사용한다. 단위 회귀의
spill 감소는 제품 개선 증거가 아니며, 경합 host의 wall-time 차이는 정식 인과
비교가 아니다. 새 Source-only 성공은 원30s 충족의 해당 실행 증거이고,
원4K 완료·worker 후반·tracing≤5%·RSS 수용과 분리한다. cache만의 인과 효과를
정량화하려면 동일 현재 binary/input의 명시 small-cache 대조가 필요하다.
과거 실패 source와 현재 source의 시간 차이를 cache 개선율로 발표하지 않는다.

cache를 더 늘리는 변경은 선택하지 않는다. e4 실패 캡처의 map spill0·
COMMIT 내 fullfsync3.761s는 역사 범위이며 현재47의 수치로 재사용하지 않는다.
새 owner 준비와 path/frontier span은 요청당 각1개이며,성공 캡처에서 각각
4.240s/0.064s로 분해됐다. 이 child 관측은
기존 NativeSourceApply의 first failed_stage·원 typed error를 변경하지 않는다.

정적 확인된 별도 낭비는 owner-spool COMMIT 이후 path scratch.set_refs가
행별 자동 transaction으로 실행되는 점이다. 기존128-row keyset page 안에서만
transaction을 묶고 매행 reserve_for_write·page cap·원 검증/취소 순서를 유지한다.
scratch는 journal/synchronous OFF의 attempt-local DB다. 이 수리를 durable
fsync 감소나 rollback 원자성으로 설명하지 않고,실패한 attempt는 기존대로
폐기한다. 모든 path 검증 전 persistent map 설치 금지도 보존한다. 이 수리는
Source 진단 통과에도 불구하고 원4K30s 실패 전체를 해결한 것으로 세지 않는다.

30s 제품 경계는 기존 client.submit의 budget과 Published 응답으로 판정한다.
서버 request span은 응답 write 이후 counters·peer-watch 해제·span 종료도
포함하므로 같은30s를 독립 상한으로 재적용하지 않는다. 준비된 외부 full4K
supervisor의 추가 daemon-span 상한은 제거했다. 양쪽 elapsed는 계속 기록하고,
원 클라이언트30s·trace 완료/유실·실제 설정·source/binary 검증은 유지한다.

### v5 도입 전 원4K 관측 용량과 선택 근거 — 이력

dc622f16 당시 recorder와 canonical parser의 capture 상한은 모두1GiB다. dc622f16
원4K에서 실제1073741068B로 한도에 도달했다. footer는 attempted/accepted
5510825,written5347756,unwritten163069,dropped0,open5이며 canonical parser가
PARTIAL로 판정했다. 구조 오류/queue drop이 아니라 실제 출력 용량 부족이
확인됐다. 과거199-worker prefix의 전체 크기 환산을 최신 실측 대신 쓰지 않는다.
이 실행은 초기4K와 Q64·reopen/delta/overlay를 완료하지 못했으므로,누락된
이벤트와 남은 phase까지 포함한 최종 크기는 알 수 없다.

최초 원4K 제품 완료·품질은 기존 default Off selector로 검증한다. 같은4096×64,
Q64/K10/recall·score/reopen/delta/path/overlay와 절대1200s는 유지한다. Off 결과는
후반 phase 인과·Record overhead·loss0 수용을 대체하지 않는다. 실패 전용 bounded
진단은 유지한다. 용량 결함이 확인됐으므로 v5의 공통 event envelope와
빈도가 높은 span/counter payload에 compact tuple을 쓰는 변경을 선정해 적용했다.
header/footer는 기존의 명시적 object를 유지했다. exact
tuple arity/tag/type·버전·혼합 거절을 정의하고 공통 내부 IR로 한 번만
정규화했다. full128bit capture_id·sequence·timestamp·모든 event와 foreign
raw event 거절은 유지한다. 기존4MiB 표본의 key 축약18–27%,envelope tuple
약30%는 초기 표본 추정이었다. 선택한 envelope+common payload tuple로
기존 capture의64MiB 지점4MiB 표본21139events를 실제 재직렬화한 결과
4194294B→2133695B(49.13% 감소)다. 이는 해당 표본만의 크기 결과이며,
누락 이벤트·전체4K 수용·실행 overhead는 보장하지 않는다.
writer/schema/canonical parser/Rust child validator/profile/worker index의 버전
소비자를 함께 변경했고 기존 persisted1–4 reader 검증은 유지했다.
capture_id 삭제는 raw 혼합 검출을 약화하므로 제외한다. gzip/binary 전환은
현재 JSONL prefix·부분 캡처·bounded decoder 계약까지 바꿔 우선안에서 제외한다.
sampling/worker-window는 전체 Record를 대신하지 못한다. cap 증가나
Partial→Complete 승격으로 해결하지 않는다. 기존 정식 Off/Record3쌍·각 phase≤5%·
loss0 완료 조건은 그대로 OPEN이다.

### 이번 감사에서 제외한 잘못된 원인

- 원4K는 `config_with_openai_semantic_dimension → config`에 owned_work와
  effort를 추가한다. **maintenance와 retention은 없다.** 설정 parser도
  None을 유지하고 pump는 reclaim Some일 때만 run_reclaim_due를 호출한다.
  별도 scale harness의20ms maintenance/512KiB TRUNCATE 정책을 이 실행에
  적용하지 않는다. 백그라운드 명시적 TRUNCATE 경쟁은 이번 원인에서 제외한다.
- Source owners/paths/frontiers는 한 apply_stream으로 변경한다. Lexical도
  초기에는 전체 새 입력,증분에는 SourceChangeCursor의 변경분을 같은 builder로
  소비한다. 불필요한 전체 재구축·반복 top-level map apply는 확인하지 못했다.
- OwnerScope의 선행 `lookup(base,key)`와 resident CAS는 같은 인증이 아니다.
  선행 조회는 physical original base를 확인하지만 `ResidentStage::read_node`는
  staged wire를 먼저 읽는다. 선행 조회를 지우면 미설치 provisional root가
  원본으로 승인될 수 있어 삭제안을 제외한다. 기존 old-node 전체 DFS도
  확인되지 않았으며 설치는 새 staged DAG만 순회하고 old node에서 멈춘다.
- worker의3 managed 쌍은 요청·provider artifact·owner/coverage graph의 서로
  다른 내구성 경계다. 물리 sync 전 intent와 sync 후 confirmation을 합치거나,
  root/권한 검사를 캐시하거나,FULL/fullfsync를 낮추는 변경은 선택하지 않는다.
- WorkStatus는 미완료 receipt만64개씩 순환하고 진행이 없으면500ms→1s로
  기다린다. 과거 실행의 누적 RPC wall은 worker 실행과 겹치므로 그 값을
  제거 가능한 시간으로 계산하지 않는다. observer가 지배적이라는 현재 증거도
  없다. 후반의 같은 source 관측에서 Catalog lock 경합이 확인될 때만 기존
  정확한 완료 receipt와1200s를 보존한 관측 부하 변경을 검토한다.

### b196 원4K Source 요청의 비용 분해

같은 capture c88120efa22f31cd2a546dde9c7cb555,Source b19668ad만 사용한다.
span5532라는 정수는 다른 capture에서도 재사용되므로 단독 식별자로 쓰지 않는다.

| 분모 | xWrite 횟수 | SQLite 요청 bytes | admission elapsed |
| --- | ---: | ---: | ---: |
| WAL,backfill 외86세그먼트 |190837|312586376|6.250s|
| main-file,backfill27세그먼트 |57030|233594880|2.077s|
| 같은 Source 요청 전체113세그먼트 |247867|546181256|8.326s|

Catalog scope17.214s에는 읽기·SQL·COMMIT이 포함된다. callback admission/
lower/post 합11.029s와의 차이6.185s를 SQL CPU나 trace overhead로 명명하지
않는다. Backfill27회의3.103s는 관측된 기여이며,그 값을 부모 시간에 더하지
않는다. 대부분의 큰 managed 쓰기는 BuildPrepare의256-input confirm 배치에
있지만,상위2개 WAL세그먼트는 managed 밖의 Catalog API 범위다. 따라서 모든
쓰기를3779 managed inputs에 귀속하거나 COMMIT의 xWrite 수로 환산하지 않는다.

### 당시 수정 순서와 중단 조건 — 이력

1. **판별용 코드부터:** 기존 Catalog/VFS 관측 경계에 같은 writer connection의
   `CACHE_SPILL`, `CACHE_WRITE`, 필요시 `CACHE_HIT/MISS` 전후 delta와 실제
   `page_size/cache_size/cache_spill/wal_autocheckpoint`를 붙인다. Record에서만
   고정 개수 카운터를 집계하고 per-write event·전 SQL 문자열 로그는 추가하지
   않는다. 상태 읽기 오류·counter regression·foreign connection은 관측 불가로
   남기며0으로 바꾸지 않는다. SQL/IO 예산이나 트랜잭션 결과는 바꾸지 않는다.
   현재 rusqlite0.40.2에는 db_status safe API가 없고 Catalog/Runtime은 unsafe를
   금지한다. 따라서 연결 수명을 지키는 adapter 안전 경계를 먼저 확정한다.
   금지 규칙 완화·임의 raw pointer 전달·다른 connection 측정으로 대체하지 않는다.
2. **최초 판별 실행은 기존 Source-only 진단:**
   `ann_quality::materialization_diagnostic::diagnostic_native_ann_4096_source_advance_phases`를
   명시적으로 선택한다. 같은4096 입력·원30s/1Mreads/512MiB·WAL/FULL/fullfsync와
   원 code path를 유지한다. 이 최초 실행은7fdf7618에서 FAILED로 끝났고
   실제 spill을 관측했다. 아래 정책/코드가 바뀌기 전 같은 실행을 반복하지
   않는다. 새 제품 runner나 원4K20분 반복을 만들지 않는다.
3. **spill이 쓰기를 증폭한다면:** 실제 dirty set에 맞는 writer page-cache 정책을
   기존 메모리 admission에 연결한다. 8/16MiB는 비교할 후보일 뿐 확정값이 아니다.
   연결 수명 동안 유지되는 cache를 요청 종료 시 반환된 임시 메모리로 과금하지
   않는다. `cache_size`는 RSS hard cap이 아니므로 실제 사용/영향도 별도 검증한다.
   cache_spill은 유지하고,같은 행 작업에서 spill/WAL 요청 bytes/callback calls가
   함께 줄어드는지 확인한다. 줄지 않으면 cache 가설을 철회하고 이 변경을 채택하지 않는다.
4. **spill이 작다면:** cache를 키우지 않고 managed confirm와 위 managed 밖 API를
   고정 statement class별 실행 횟수·변경 행·payload/edge bytes로 좁힌다. 동일 tx의
   이미 검증된 입력으로 중복 SQL/행 왕복을 줄이는 수정만 선택한다. provisional/
   intent/GC 보호 삭제나 저장 포맷 재설계는 필요한 쓰기량이 원인임을 확인하기
   전에는 배정하지 않는다. 별도 producer/publication 담당의 metadata 계약도 건드리지 않는다.
5. **checkpoint 정책은 후순위:** 기본 자동 checkpoint의 COMMIT 경로 비용은
   인정하되,현재3.103s만으로 전체 실패를 설명하지 않는다. 임계값 변경은 WAL
   성장·기존 IO/공간 reserve·재시작/긴 snapshot을 함께 검증할 때만 선택한다.
6. **Source 수리 뒤 원4K 한 번:** 기존 전체 Off selector와 절대1200s를 유지한다.
   후반 Record RCA는 위1GiB 용량 계약을 먼저 해결하고 동일 capture의 유효한
   관측 범위에서 비용을 비교한다. 4096 완료 뒤 Q64·
   recall/score·reopen/delta/overlay까지 통과해야 원4K를 닫는다. 초기199/128
   진단이나 timeout 후 Published는 이 수용을 대체하지 않는다.

근거: SQLite는 transaction 중 dirty-cache overflow 쓰기를
[`CACHE_SPILL`](https://www.sqlite.org/c3ref/c_dbstatus_options.html)로 구분한다.
[`cache_size/cache_spill`](https://www.sqlite.org/pragma.html#pragma_cache_size)은
cache 크기 힌트와 spill 정책이며 process RSS hard cap이 아니다.
기본 WAL 자동 checkpoint는 [임계 COMMIT의 실행 thread](https://www.sqlite.org/wal.html#performance_considerations)에서 수행된다.
이 문서는 원인별 선택 조건을 확정한 계획이다. 후속 구현은 같은 버전의
rusqlite Connection에 안전한64bit pager API를 추가하고,Catalog lock 전후의
HIT/MISS/WRITE/SPILL4종을 Record에서만 관측한다. mutex 해제 뒤 배출하며
미관측·오류·역행을0으로 만들지 않는다. 파서·worker index도 같은 계약을
소비하고,page-write 오류가 있는 capture의 cache 해석은 사용 불가로 표시한다.
Python 집중 회귀302/302·Ruff/서식은 VERIFIED다. source7fdf7618의 owning
strict와 Rust311/311도 VERIFIED다(Catalog267,Core diagnostic21,recorder23).
기존 Source-only4096 진단은 같은 소스에서 FAILED로 끝났고,실제 pager 값과
완전한 capture는 VERIFIED다. 종료 뒤 Python 소비자만 보완해 pre/post callback
거절도 page-write 오류에 포함하고,callback 미관측은 별도 상태로 보존했다.
C callback 이전 직접 거절까지 전부 관측했다는 주장은 하지 않는다.
writer PRAGMA 실제값 readout은 이4 counter와 별도이며, 후속 `78e9e627`에서
명시 cache 정책과 함께 구현했다. 같은 연결의 실제 전후5개 설정을 사용하고
SQLite 기본값으로 대신하지 않는다. 최신 owning 회귀는 검증 중이며,
정책 적용 후 Source/전체4K 성능 비교는 NOT_RUN이다.
이전264/264·daemon3/3과 이번 계측 구현의 검증을 합산하지 않는다.

최신 Source DATA 충돌 처리·테스트 기대값 수리·원4K 결과는
[단일 검증 원장](../../plans/oct-22-finalizing/VALIDATION.md#e04-source-data-collision-20261027), 현재 상태·잔여는
[SSOT 에픽 행](../../plans/oct-22-finalizing/README.md#e07)을 따른다. 아래 checkpoint·prefix 기록은 각 과거 source 범위다.
추가 callback 비용 계측·도구 수리·현재 검증 경계는
[callback 검증 원장](../../plans/oct-22-finalizing/VALIDATION.md#e07-write-boundary-cost-20261027)을 따른다.

현재 checkpoint 계측과 실행 상태는 [단일 검증 원장](../../plans/oct-22-finalizing/VALIDATION.md#e07-checkpoint-source-rca-20261027)을 따른다.
77cb의 실제 진단은 SourceAdvance30.001s에서 실패했고 provider0이다. 완전한
capture의 request5532에서 Catalog15.21s·backfill2.63s·lexical13.91s를 구분했으며,
이 값들은 겹치는 구간이다. 완료되지 않은 source를 worker128/full4K 수용으로
표시하지 않는다. phase-parent 전달은 기존 diagnostic context로 보완했으며,
3bf033bb에서 strict·owning8/8·실제 Source/coverage/no-op3/3이 통과했다. 새 실제128/128과 complete/loss0 capture에서 실제 parent 귀속도 확인했다.
SourceAdvance24.7669s 게시 성공은 전체4K 수용과 구분한다. 이 항목은 E04/E07 공용1회 작업이며 새 병렬 큐가 아니다.
제품 원30s/1Mreads/512MiB·내구성·고정 품질 조건과 다른 에픽의 소유권을 유지한다.

추가 정적 감사에서 pinned-query paged scan의 기존 scoped DATA 캐시 우회1건을
수리했다. source c3f482a7의 Storage all-target strict·회귀5/5·실제 Kernel
query/ANN pin/reopen8/8은 [단일 검증 원장](../../plans/oct-22-finalizing/VALIDATION.md#e07-scoped-scan-reuse-20261027)을 따른다.
기존 scan/인증/논리 과금·취소·메모리 반환을 유지하며,성능 비율이나 원4K 수용은
새로 주장하지 않는다. coverage/Catalog 감사는 reserve-before-physical와
confirm-after-durability를 보존할 때 생략 가능한 중복 SQL을 확인하지 못했다.
후속 stale delta의 중복 읽기/COW 수리는 E04의 같은 큐에서 닫았다. source38dd3190
strict·maintenance20/20·실제 Kernel16/16은 [단일 원장](../../plans/oct-22-finalizing/VALIDATION.md#e04-stale-delta-batch-20261027)을
따른다. 2CPU/6GiB의 동시 두-target 링크 SIGKILL은 동일 소스에서 target별/build jobs1로
직렬화해 통과했으며 제품 runtime failure나 원4K 성능 수리로 분류하지 않는다.


동일 콜백의 `reserve_end→start` 중복 live 인증은 native 전용 결합 경로로
수리했다. 매 쓰기/SHM의 root·lock 검사와 독립 public start 인증은 유지한다.
source70312101의 Runtime strict·관련119/119와 오프라인 심볼 도구16/16이
통과했다. [현재 코드·검증](../../plans/oct-22-finalizing/VALIDATION.md#e07-native-stack-and-callback-rca-20261027).
추가 적대 감사에서 ANN seal/readback·coverage identity 검사를 안전하게 생략할
근거는 발견되지 않았다. 이들 경계를 제거한 최적화는 적용하지 않는다.
별도 심볼 바이너리의 기존 prefix 진단은1/1 통과,Source27.5961s 게시와 목표128
대비 실제 완료132개·complete/loss0 capture를 확인했다. Source 객체 읽기와
worker Catalog WAL 쓰기의 live admission 경로가 실제 함수 스택에 보인다.
일반 bench와 TEXT가 달라 과거 주소 소급 해석·개선율 비교는 거절한다.
원4K 전체·정식 overhead/운영 수용은 아직 열려 있다.

## Fixed profiles and oracles

- 기존 P00–P09 trace/recorder/exporter·제품 ReadSession/renew/release/settlement를 재사용한다. 실패·timeout·reopen stop/join/start에서도 결과가 남고 observer 비용/누락을 분리해야 한다.
- Large=4096파일(seed42,16×256,평균120행), XL=32768파일의 독립 functional ACK/search/reopen profile을 유지한다. debug 결과와 bench/release throughput은 구분한다.
- 기존 Large 운영 profile: Linux aarch64 전용2CPU/6GiB·local SSD, 같은 bench binary/caps·논리 work와 실제 RSS/공간을 분리한다. host/storage 경합은 qualified timing이 아니다.
- 원960초 배치:60초 warmup+900초 open-loop50QPS, 동시4/대기64·초과도 실패,1초마다≤16KiB 파일 replace/delete/restore. ≥10,000 attempts/각 query class≥1,000,old-view witness와 실패/timeout·scheduled queue wait 포함.
- 실제 ReadSession pin600초,300000ms lease·120초 renewal/gap≤180초. 마지막 borrower 뒤 release·60초 이내 settlement. 기존 full/mixed의 cap·retention 선택은 원4K 요청 예산을 변경하지 않는다.
- 정식 Off/Record는 같은 binary/input/caps의 직렬 교대 Large≥3쌍·각 고정 phase paired ratio 중앙값 증가≤5%·drop0. noise/partial capture로 PASS를 만들지 않는다.

### Existing execution and adjudication path

아래 실행 front door와 profile 계약은 재사용한다. 10/25 환경 inventory·E04
live 작업 관찰은 해당 날짜의 기록이며 현재 실행/소유 상태가 아니다. 원4K는
[E07 단일 큐](E07-WAVES.md#e07-closeout-plan-20261030)에서만 이어간다.

The existing `tools.benchmark.retrieval.causal_cost_capture` module is the one
capture front door. It invokes the prebuilt bench `scale_matrix` and bound
Native daemon, validates source revision/binary SHA/epoch, and revalidates
product summary/manifest plus Record parent and both first/reopen child traces.
Do not create another runner or treat diagnostic artifacts as qualified timing.
The capture front door requires a clean bound source; current dirty-source
focused proof is not automatically that formal source qualification.

- Large and XL functional gates select `--backend index-native --tier large` or
  `--tier xlarge`, seed42, explicit frozen four-axis work limits and fresh
  external output. XL's four-axis qualification values must be frozen before
  execution; a necessary mutation floor is not a sufficient whole profile.
- Formal Large observer comparison alternates Off/Record at least three pairs
  on the same prebuilt binaries/input/caps/host/cache. The existing validator
  checks each capture and product result; the allowed final adjudication uses
  the fixed phase walls in `artifact/summary.json`. No existing automatic
  three-pair phase median/noise verdict is claimed. Preserve the <=5%/loss0 gate.
  One observed managed build supplies the same copied bench/daemon files to all
  six arms; repeating `--managed-build` per arm does not guarantee that contract.
  Reuse exact paths/SHA/epochs through the existing prebuilt inputs and fresh
  external arm outputs. Prebuilt `build_binding=UNPROVEN`/PARTIAL_DIAGNOSTIC
  remains unchanged. Separately review transitive source custody from the
  original observed-build receipt through every arm's source/epochs/raw/product
  validation, then adjudicate fixed-phase ratios/noise/loss. A caller receipt
  never grants OBSERVED_LOCAL_BUILD, and arm status is not renamed by this manual
  adjudication. No second runner or six-arm orchestration is required here.
- The existing `--native-mixed-load` path is Large42/Off with explicit work
  limits and sufficient overall max-seconds. It selects the fixed60+900s,
  deterministic50QPS,4 workers/64 queue, scheduled-to-reply deadlines and
  1Hz publisher. The actual ReadSession holds600s, renews its300s lease every
  120s and validates last-release settlement within60s. The original query and
  publisher thresholds are checked before a valid product artifact is emitted.
- NativeSampler's Linux process100ms/disk500ms observations retain PID birth,
  unique-inode allocated state/scratch accounting and missing/error counts.
  Python revalidates the typed method, epoch and availability and rejects
  sampled ceiling violations. Under-cap sampled values or incomplete sampling
  never establish the continuous high-water SLO. Whole-lifetime resources,
  physical faults/churn and the Linux aarch642CPU/6GiB/local-SSD profile remain
  explicit final qualification inputs.

Read-only environment inventory on2026-10-25 finds a reusable Linux arm64
Rust1.92.0 toolchain image `qidx-ann-toolchain:20261018-root`, ID
`sha256:61cfd13d3b65f009cc5ea9771c240a932f634c7893b9816dba4f48b6f6dc331c`.
Its old source/registry mount labels establish neither current source nor a
current compiler cache/binary. The current Docker Desktop Linux aarch64 engine
supports CPU/memory limits and cgroup v2 but runs49 containers. No container is
started or changed for this audit. The engine is not a proved dedicated quiet
2CPU/6GiB/local-SSD qualification host. Clean source plus bound current Linux
bench/daemon SHA/epoch, actual local-SSD authority, delegated cgroup and host
ownership remain required inputs; existing foreign E02 containers are not an
E07 allocation. Do not start a competing heavy build beside E04's live original.

This source audit confirms existing producer-to-validator wiring; it runs no
Large/XL/mixed body and closes no operating or overhead gate. Continue through
this same E07 queue after E04's original4K result and required source/host freeze.

## Capture capacity history before a full Record run

아래 기본64MiB·parser128MiB 진단에서 명시 용량 전달·stream parser·v5까지의
후속 구현을 순서대로 보존한다. 이전의 미연결/미구현 문장을 현행 TODO로
읽지 않는다. 현재 전체 Record/overhead 수용 공백은 별도로 남는다.

현재 기본 recorder는 queue4096건·record4096bytes·output64MiB이고, canonical offline timeline parser는128MiB를 상한으로 둔다. source96578416…의128-owner complete capture는46342events/9269407bytes였다. 이를 단순32배 하면 약283MiB이므로 원4K Record 전체가 두 상한을 넘을 가능성이 있다. 이는 밀도 환산이며 실제4K 크기나 실패를 측정한 결과는 아니다.

상한 초과는 recorder의 unwritten/partial로 남고, 기존 `causal_cost_capture.py`는 complete가 아닌 timeline을 거절한다. daemon/product test의 성공 종료만으로 trace complete를 주장하지 않는다. 정식 Record 실행 전에 writer와 offline parser의 동일 capture 범위를 함께 정하고 실제 footer·written/unwritten·drop·lease/open-span을 확인한다. IPC readiness 수리 후 source8b63…의128-owner 진단은232378events/약43MiB로 늘었고, polling 호출도614→17584로 달라졌다. no-progress pacing 후의 실제 밀도와 동일 workload를 다시 고정해야 하며, 이 서로 다른 진단을 paired overhead로 비교하지 않는다. 파일 상한만 키워서 전체 RSS/관측 overhead가 검증됐다고 취급하지 않는다. 제품 요청·worker·품질 예산은 변경하지 않는다.

이전 cf6a3314…는 pacing과 Catalog wait/hold를 함께 사용한128-owner capture가107831events/21108098bytes로 COMPLETE이며, 상태조회3912회와 정확128/128 완료를 확인했다. 별도 region 초기/reopen capture2개도 COMPLETE다. [직접 비용과 검증 경계](../../plans/oct-22-finalizing/VALIDATION.md#e04-e06-catalog-control-closure-20261024)를 따른다. 이128 밀도를 단순32배 하면 약644MiB로 두 상한을 넘는다. 환산은 실제4K capture 크기·누락·RSS 측정이 아니며, 현재 작은 capture의 complete 판정을 전체 Record 수용으로 올리지 않는다. Worker의 Catalog hold32.017s/operation41.851s와 wait0.230s는 잠금 안의 durable 비용을 지목하지만, parent-sync 정산 계약을 생략하거나 다른 source 진단과 paired speedup을 계산할 근거가 아니다.

청크 배치 수리 후 d07ba483…의 실제128 capture도 COMPLETE/유실0이며,
103963events/20353421bytes,정확128/128·상태조회3680회를 확인했다. 설치쌍은
이전128 진단657→현재649,worker 관리 COMMIT1314→1298이다. 이는 작업 수의
감소이며 경합·관측 workload가 달라 paired latency/overhead 수용이 아니다.
현재 밀도의 단순32배는 약621MiB로 여전히 writer/parser 상한을 넘는다.
현재 worker Catalog hold32.093s/operation42.181s,wait0.0648s와 전체 trace3개의
검증 경계는 [최신 원장](../../plans/oct-22-finalizing/VALIDATION.md#e04-original4k-and-bounded-installs-20261024)에 있다.
clock 동일 시각 생략의 실제 빈도·효과는 미측정이다. 원4K를 다시 통과했거나
정식 Off/Record·전체 process RSS/공간을 수용한 것으로 표시하지 않는다.

후속 분류 계측 df419812…의 실제128은 v4 COMPLETE/유실0,101392events/
19858532bytes이다. 단순32배는 약606MiB로 writer/parser 상한 문제는 그대로다.
Worker32.829s 중 Catalog hold24.371s/wait0.018s,overwrite parent5.184s를
직접 구분했다. Parent 전량 생략의 이상적 산술도 약1.188배이며,내구성 동등성·
실제 credit 적중률·원4K 수용을 입증하지 않는다. Host load32.274/외부 Rust2로
이번 실행도 경합 진단이다. [현재 분류·검증 원장](../../plans/oct-22-finalizing/VALIDATION.md#e04-native-sync-mutation-classes-20261024)을
사용하고 다른 source의 wall 차이를 speedup 또는 observer overhead로 계산하지 않는다.

후속 결합74f80886…은 daemon3개를 포함한 영향82/82가 통과했다.128 capture와
region 초기/reopen capture3개 모두 v4 COMPLETE/유실0이다.128은117199events/
22881188bytes,설치그룹521·관리 COMMIT1042로 df419812의649/1298보다 줄었다.
다만 polling은4848회,host는외부Rust9/load21.296387로 달라졌고,worker wall은
61.034s/Catalog hold45.941s/wait0.251s다. 작업 수 감소만 확인하며 속도·overhead
비교로 승격하지 않는다.128 밀도의 단순32배는 약698MiB로 writer/parser 상한
문제가 남는다. [현재 구현·계측](../../plans/oct-22-finalizing/VALIDATION.md#e04-prepared-delta-closeout-20261024)을
따르며 원4K·정식 paired Off/Record·연속 RSS/공간·운영 DoD는 별도 수용한다.

Readiness observer도 workload로 고정한다.74f80886 원4K Off 실패는106944회의
개별 WorkStatus RPC와233.062s client RPC wall/1134.029s loop를 관측했다.
약94reads/s이며 각 요청은 worker와 같은 Catalog mutex를 사용한다. 이 wall은
CPU·mutex 점유 또는 worker 지연 비율이 아니다. WorkStatus는 read-only이고
COMMIT을 발급하지 않는다. 공개 Native UDS에는 별도 aggregate 완료 알림이
없으며 ReadView는 pin 트랜잭션을 포함한다. view sequence나 ReadyWork의 빈
목록을4096개 exact completion receipt 대신 쓰지 않는다. Observer cadence를
바꾸는 진단은 원 절대 deadline·전수 receipt·final coverage oracle를 유지하고
같은 workload/profile로 다시 비교해야 한다. 현재 증거만으로 polling이 전체
처리량 실패의 원인이라고 확정하거나 낮춘 빈도를 성능 수리로 세지 않는다.

Full Record 준비의 이전 연결·메모리 공백도 이 에픽에서 처리한다. Recorder API의
`DiagnosticCaptureLimitsV1.max_output_bytes`와 달리 실제 scale harness/daemon
CLI에는 별도 용량 전달 인자가 없었고, 자식 capture/profile 소비자도64MiB 고정
검사였다. 이전 offline timeline parser는
128MiB 제한 안에서도 raw bytes·split lines·decoded records/events·최종 span/
observation 결과를 중복 보유한다. Writer의 seen/counter_totals도 출력량에 따라
커진다.128 밀도 환산을 실제4K 실패로 단정하지 않되,상한만 올려서 준비 완료로
표시해서는 안 된다.

이 빈칸은 별도 티켓 없이 이 E07의 기존 full Record/RSS 조건으로 처리한다.
기존 JSONL·동일 검증기·complete/footer/sequence/digest/drop/unwritten/open-span
거절을 유지하고,run별 명시적 용량을 harness→daemon→자식 검증 경로에 일치시킨다.
같은 parser를 행 단위 검증·집계로 바꿔 전체 raw/record 복제를 제거하고,대형
span 결과와 writer 상태 메모리를 원 aggregate RSS/physical profile에서 계측한다.
새 포맷·중복 parser·무한 상한·잘린 capture의 성공 처리는 금지한다.

2026-10-24 구현에서 `--trace-max-output-bytes`를 추가해 용량을 전달한다.
Harness/daemon의 기본64MiB는 유지하며 명시값6144bytes–1GiB를 parent recorder,
첫 실행/reopen child, drain/artifact/Python profile 재검증에 전달한다. Python
capture의 명시 override는 Native Record에서만 허용하며 offline export도 명시
상한을 받는다. 기본 offline parser128MiB와 기본 child64MiB는 유지한다.
원4K/128 진단 helper는 `QIDX_NATIVE_TRACE_MAX_OUTPUT_BYTES` opt-in을 같은
daemon 인자로 전달한다. 제품 요청·worker·quality 예산과 별개인 관측 출력 예산이다.

동일 parser의 `parse_stream`이 bounded binary line과 증분 SHA-256으로 집계하고
`parse(bytes)`도 이 경로를 사용한다. 소비자는 커밋된 RawFile을 사용하며 export는
원본을 다시 검증한다. 전체 raw/decoded record 복제는 제거했지만 최종 span/metadata,
writer의 seen/counter_totals와 viewer 출력은 여전히 데이터량에 따라 커진다.
실제 v4 recorder를 v2 전용 child validator가 거절하던 공백도 지원2–4와 동일
header/footer 버전 검사로 보완했다. [Source별 영향 검증](../../plans/oct-22-finalizing/VALIDATION.md#e07-bounded-record-stream-replay-20261025)은 Rust95/Python658 통과·기존2skip·실제128 COMPLETE/유실0이다. Schema 분기 선택은 기존 전체 parser 결과 SHA를 유지하며 동일19.7MB 재생 CPU39.938→8.139s를 확인했다. 이는 단일 offline 진단이며 전체 peak RSS·정식 Off/Record 수용은 NOT_RUN이다.

이전 원4K Record는 cbba에서 Source30s deadline으로 FAILED0/1이다. 캡처2.56MB COMPLETE/유실0은 실패 관측 증거다. SourceSubmit30.074s 안의 Catalog hold15.767s와 managed COMMIT5.756s는 포함 관계이며, staging을 포함한 전체 reserve/confirm40.8s를 이30s에 더하지 않는다. 후속 scratch mark의 bounded64-row SQL은 f0dca8d6에서 storage strict·map77/77·Kernel source-batch1/1을 통과했다. 별도 Source-only 심볼 표본은 VFS의 매-write root/lock 검사와 SQLite spill/checkpoint를 확인했지만 검사 캐시는 쓰기 전 거절 계약 때문에 제외했다.

기존 AfterWrite 콜백의 scope별 calls·요청bytes·오류·WAL/main/other 집계는062fe720에서 strict·Rust82/82·Python678 pass/기존2skip, 역사128 전체 결과 SHA 동일을 확인했다. 해당 source의 Source-only는 원30s에서 FAILED0/1·provider0이다. SourceSubmit 내부 SQLite xWrite111775회·요청241854244B·하위쓰기실패0을 staging/cleanup과 분리했다. other를 subjournal로 단정하지 않으며, 캐시 통계·실제 기록bytes·전체 CPU 측정으로 취급하지 않는다.

단일 provisional INSERT/Transaction 계약 수리는c39a67e9에서 strict·Rust106/106을 통과했다. 고정256회 provisional INSERT gold·늦은 실패 rollback·재오픈을 유지한다. 동일 source Source-only4096은 Published22.384s/1회 통과했고 원 full4K는 Source30.001s/1회 실패했다. 후자 preflight 외부 Rust30/load23.574에서 authorized 경합 진단으로 실행했으며 host 인과 기여도와 개선율은 미입증이다. 원키 Pending/provider0·3.14MB COMPLETE/유실0을 기록했고 Q64 이후는 NOT_RUN이다.

같은-Tx canonical fence counter UPDATE256→1은 ae051807에서 Catalog strict·74/74 통과했다. 단일 scope의 batch_limit1/max_in_flight1/provider concurrency1을 유지하며 max_dispatches2는 동시성 아닌 재시도 한도다. 원4K 단일 진단은 Source18.810s Published 뒤 materialization의 ResourceLimited에서 FAILED0/1이다. 당시 watchdog 약285s 잔여, snapshot2041/4096 completed·provider 중복1이며 Q64 이후는 NOT_RUN이다.616881711B/3102554events 캡처의 canonical COMPLETE/유실0을 확인했고 하위 거절/renewal RCA를 진행한다. 원인 메시지를 잃던 read/write admission과 최종 coverage-save 경계를 보완한7b56a113은 strict·budget/native-I/O23/23·Kernel8/8이 source drift0으로 VERIFIED다. 후속332c3ec8(코드6·기준파일1)는 설치 재시도에서만 원 authority의 retired-create 복구와 sync 후 경로/FD 재검증을 추가했다. strict·계층/모듈 경계·집중 회귀97/97은 VERIFIED이며 Kernel 접합8/8도 VERIFIED다. 원 fixture 기본 Off4K는 Source30.002s에서 FAILED0/1·Pending/provider0이다. BuildPrepare 취소이며 수치 예산 초과와 external refusal은 없다. 당시 host 외부 Rust2/load20.75여서 tracing/host 인과·정식 성능은 미입증이다. 동일 실패를 재시도하지 않는다. 후속255493d6의 parser terminal 중복 보관 제거는 Ruff·Python684 pass/기존2skip을 통과했고 실제588MiB 구·신 전체 결과 해시가 일치하고 parser peak RSS1549893632→1277411328B(약260MiB 감소)를 관측했다. 단일 직렬 쌍이며 제품 전체 RSS·runtime overhead 수용이 아니다. 실제4K 원인과의 결속은 아직 없다. guard·한도·발급 순서·실제 발급 수만큼의 최종 counter·원 COMMIT 경계를 유지한다. 정식 quiet/paired·전체 RSS/공간/fault/운영 수용은 별도로 남는다.

The existing c39 successful Source-only capture has a further canonical,
same-run phase bound: complete pre-postings inventory1.157187708s,
postings6.636377834s, contained Catalog hold4.960253460s/18 and
COMMIT2.313156499s/18. Inclusive regions overlap and cannot be summed.
All token scratch INSERTs are inside the inventory upper bound; this does not
measure their individual cost or current source/host timing. Initial Source's
old terms root is empty throughout postings, excluding map lookup_batch as a
read-saving repair for this fixture. No candidate is applied without a
matching measured cost. The255 source's R2 Source-only diagnostic proves
both stack collection and independent1s OS CPU/disk-I/O windows inside the
canonical lexical/postings stages. Source still FAILS at30.002143209s;
lower-VFS Source calls192662/requested421821248bytes/failures0 describe
reached writes, not allocation or completed device bytes. The postings1.0006s
window records daemon CPU749.349166ms (176.076625ms user/573.272541ms system)
and OS-accounted writes19996672bytes after Mach timebase correction;
stacks show final map install→Catalog confirm→pager spill/subjournal/WAL→VFS.
Whole Source Catalog hold16.246s and COMMIT6.838s overlap and are not additive.
R1 coupled rusage to11s symbol processing and could not prove stage containment;
its cost attribution is rejected. The corrected observer separates collection,
postprocessing and bounded rusage, rejecting negative deltas and late windows.
Both runs retain original product limits. This is diagnosis underforeignRust16/
load35.15625, not original4K acceptance, phase-wide CPU share, unchanged-machine-
code comparison or Off/Record qualification. No further identical run is assigned.

The earlier17.98438ms CPU value was a unit error: raw Mach ticks were labelled
as nanoseconds. This host's mach_timebase_info is125/3; an independent1s CPU
calibration agrees after conversion. Original R1/R2 raw values stay unchanged,
with corrected interpretation in VALIDATION; no product rerun is required.
This changes CPU interpretation, not trace/stack containment or disk-byte units.

The subsequent26d99ebf cut removes charge_object's discarded JSON Vec solely
for fee estimation, using the existing canonical serializer with a counting
sink. The exact fee and error/checkpoint order remain. Catalog strict and
hexagonal pass; all Catalog library tests252/252,0 skipped pass with source
drift0. Other metadata/intent/digest allocations are unchanged. This is a
scoped allocation repair, not a measured Source30s improvement or product closeout.

The next39e9a830 cut changes the measured lane8 protection write shape: each
existing64-row preparation packet uses cached single-row INSERT OR IGNORE inside
the same outer transaction. All packet key/digest preparation precedes SQL and
all packet protection precedes reclaim transitions. Existing256-object install
groups, precharged WorkUsage, immediate FKs, replay/rollback, CAS, barriers and
COMMIT stay in place. This targets SQLite's multi-write statement journal;
it does not change cache/spill policy or remove durability checks. Strict and
hexagonal plus Catalog252/252 pass at this cut. Runtime VFS is FAILED42/43:
the new discriminator leaves a pending WAL mutation after raw connection close;
its write comparison is not reached. Kernel and original4K are NOT_RUN.
A focused9d0a6eb0 diagnostic preserves the check, identifies only the pending
WAL path with pins0/close_unwitnessed=false and observes packet writes98/
262400bytes. That probe does not reach the scalar arm. The subsequent86d2f5bb
cut changes only the existing Runtime test to use the original Catalog owner
through reopened verification, with WAL/FULL/fullfsync/FK readback and checked
close. Strict/hexagonal, Runtime VFS43/43 and Kernel8/8 pass with2106-input
drift0. The unchanged Catalog252 proof remains bound to39e9a830. The fixed
64-target insertion scope observes packet[98calls,262400bytes] versus scalar
[0calls,0bytes], failures0; COMMIT is outside this scope. Exact reopened rows,
digests/FKs and terminal quiescence pass. This is mechanism evidence, not zero
transaction writes or a measured product speedup. One original default-bench
Off4K at86d2f5bb is FAILED0/1: SourceAdvance is Published in25.455199041s within
30s, but initial materialization reaches3119/4096 at the unchanged1200s watchdog.
Readiness lasts1144.521s; the later non-atomic snapshot is3140 Completed/
956 Pending/0 Blocked and provider3146 unique/0 duplicates. Daemon exits0 and
all2106 hashes remain fixed. Q64/reopen/delta/path/overlay are NOT_RUN.
Normal preflight reportsforeignRust2/load14.39502; this is authorized contended
diagnosis, not a timing comparison. No unchanged full4K retry is assigned.
Readiness123712 RPCs accumulate125.008s wall overlapping product execution;
that is not proof polling caused the roughly31% throughput shortfall.

The followinga5e4e974 cut removes only prepared_work's clock-row UPDATE by
using the existing advisory observed_catalog_now_ms. BEGIN/COMMIT, authenticated
floor/live claim/artifact checks and stage/final durable revalidation remain.
A new zero-SQL-write/restart/clock-corruption/expiry oracle and existing Kernel
Off/Record result/work parity pass in the serialized validation batch. Kernel's
existing prepared/stage API envelopes now use distinct SqliteWork lanes23/24
instead of conflated lane14; other Catalog envelopes keep14. Off gains no clock
or event. Strict/hexagonal, Catalog253/253 and Kernel8/8 pass with2106-input
drift0. One existing128-owner Record diagnostic also passes29.44s,128 unique
provider rows/no duplicates. Its22.6MB schema4 trace is COMPLETE/loss0, with
128 successful worker roots and balanced prepared/stage128/128 API envelopes.
All128 prepared calls perform0 native writes, confirming the lookup repair.
Current worker inclusive total23.869s contains Catalog hold16.310s and managed
COMMIT9.329s, which overlap; stage128 costs0.798s inclusive. This identifies
remaining transaction/durability cost without attributing the whole4K failure
or proving observer overhead/speedup. A fresh normal preflight now passes with
foreignRust0/load7.953125<=8. Root starts one original default-Off4K at this new
cut without a contention override. SourceAdvance is Published16.8077325s within
30s. All4096 exact completion receipts are observed in1143.915s; readiness uses
128541 RPCs/102.576s accumulated wall overlapping worker execution. The body is
FAILED0/1(1211.53s): the shared1200s deadline expires during initial Q64 seed48,
with an11ms remaining IPC read budget. Provider4096 unique/4096 total/0 duplicate
rows and48 unique/49 total queries are observed; daemon exits0 and source drift0.
Materialization completes but full initial quality, reopen/delta/path/overlay do
not complete. This clean start does not qualify paired overhead, matched speedup
or whole-run host isolation. No unchanged full4K retry is assigned. A bounded
4K-source/exact128-completion Record prefix is the next diagnosis, to distinguish
large-map transaction cost from the small128-source working set. It retains all
request/owned/product limits and normal shutdown; partial receipts are never
full4K acceptance.
The recoverable typed VFS detail gap now has an opt-in callback-local repair
and owning proof below. Historical final-error attribution is still UNKNOWN.

The prefix fixture at471ae8d6 changes only existing integration diagnosis source
relative toa5e4. Runtime owning strict is VERIFIED2.929s; one clean-start Record
body is VERIFIED1/1,90.13s(body)/106.874s(command),2106-input drift0. Source4096
is Published16.175444875s and exactly129 Completed receipts are observed in
50.143s(5312 status reads/4.128s accumulated RPC wall). Normal confirmed shutdown
observes200 provider rows/0 duplicates. Canonical parsing of45,677,115B reports
schema4 COMPLETE,232745 events/80941 spans, health/orphan/truncation/loss0. It
contains200 worker roots:199ok and1aborted during intentional shutdown; this is
not200 completed public receipts. Inclusive worker46.834s contains Catalog hold
32.996s and managed COMMIT20.630s; WAL sync15.307s contains parent sync3.614s.
These are overlapping intervals, not additive or matched speedup/CPU shares.
All200 prepared lookups still have0 native writes. This is an early4K working-set
diagnosis, not full4K quality/steady state/observer overhead qualification.
A proposed observational validate_owned_work clock change is rejected: removing
its durable high-water update could accept a subsequent time below an earlier
validation sample. The existing Immediate tx/clock/lease contract stays. The
narrow safe code lane reuses one authenticated operation read in the same tx;
a separate opt-in VFS callback observation will preserve typed refusal detail
without inferring the eventual SQL/worker failure's cause. Both code lanes now have focused validation below; they do not close throughput
acceptance.

The first86f12734 attempt
failed strict on12 new test-only lint findings; no test body ran, and those
findings were repaired before39e9a830.

At092f5788, the Catalog private validator reuses the status from its canonical
same-transaction operation read. BEGIN IMMEDIATE, durable clock persistence,
retained-body/terminal/live-claim validation and error precedence remain. Its
Unknown/Pending/Published operation SELECT count is exactly1; clock corruption,
regression, wrong body and lease expiry remain refusal oracles. Catalog254/254
is VERIFIED at806f4bec with identical Catalog production/tests in092f; no new
Catalog execution is invented at092f.

The Runtime repair samples only the dispatched callback's own typed budget
refusal and matching returned Capacity, up to8 process-wide events. It moves
the owned CoreError into a nested/thread-local frame and emits after Adapter
and Router locks release. Detail is truncated on a UTF-8 boundary at256 bytes;
direct Capacity lacks attributable CoreError and is explicitly unknown. Startup
registration reads the existing opt-in env once. Off adds one TLS read per
callback but no configuration read, clock, heap allocation or output; zero total
performance overhead is not claimed. A disabled inner frame masks an enabled
outer frame; unwind, concurrent8-slot admission and failed output retain the
original result. Successful product-fixture shutdown now relays these bounded
records before deleting daemon stderr. Logs are callback observations, not a
terminal SQL/worker cause or complete event inventory.

Current092f(2107 inputs, one added private Rust module) passes Catalog+Runtime
all-targets strict9.360s, hexagonal0.696s, VFS49/49(80.665s command), Kernel8/8
(26.586s), and diagnostic relay2/2(47.763s). Source drift0. VFS first failed48/49
at806f/c7 because its new oracle incorrectly required BeforeWrite: one held byte
funds the empty first DB, then the journal creation fails at BeforeOpen. The
original one-byte allowance and exact SQLite capacity classification stay;
the fixed expected callback/detail follow that independent producer contract.
The mode is contended focused functional validation, not timing acceptance.
No original full4K rerun is performed. The remaining throughput gate is the
last a5e4 original failure, not this focused batch. The prefix shows basic
workers pay4 managed object groups/8 reserve-confirm COMMITs plus7 other WAL
syncs; dependencies and parent-directory barriers remain. Removing any of them
without an atomic contract/authority proof is not an accepted repair.
[Commands, failed attempts and proof scope](../../plans/oct-22-finalizing/VALIDATION.md#e07-bounded-record-stream-replay-20261025).

The existing resource ledger's Retained lifetime can fund a connection-owned
cache without a new authority. Its lease must survive through actual checked
close, including close failure, with a proven complete memory bound; advisory
SQLite cache_size is not such a proof. No unaccounted cache increase or spill/
guard/fsync suppression is selected as a repair.

The next80e62276 cut is focused VERIFIED: same-tx immutable key encoding,
per-call bounded namespace SQL2→1, and authenticated ANN delta scan-reference
reuse. Strict/hexagonal, Catalog255/255, Semantic234/234, Kernel8/8 and the public
filtered-exact daemon gold1/1 pass. Its single partial4K Record diagnosis also
passes1/1 with131 exact Completed/50.119s and45.9MB COMPLETE/loss0 capture.
The owned Source and worker one-second windows measure user/system CPU and
physical I/O with the actual125/3 Mach timebase. System CPU exceeds user CPU in
both windows, but the short contended samples do not establish a whole-stage
cause, matched speedup or observer overhead. Whole4K is NOT_RUN at this source;
the last original failure remains Q64 seed48 ata5e4. Keep all original limits,
barriers and quality gates. [Implementation, OS observations and limits](../../plans/oct-22-finalizing/VALIDATION.md#e07-catalog-ann-cost-cut-20261025).

### Selected DATA and phase attribution closeout — 2026-10-25

The next source edits reuse an already-held artifact without an additional
cache allocation, and emit real phase ancestry for managed child work. The
Catalog owned-ready normal path removes a redundant same-transaction row read
through the original full validator, preserving corrupt cross-key refusal order.
The focused library/strict/lifecycle/actual-daemon batch now passes across the
explicit ba31b8a5→67741542 fixture-only correction. The same-source partial4K
capture passes1/1 with128 exact Completed/51.711s,
healthy COMPLETE/loss0 and emitted four-phase managed ancestry. The original4K
normal admission is BLOCKED(foreign Rust5/load29.469727>8), body NOT_RUN.
Throughput improvement and Off/Record overhead are not established.
Read-only audits find no further proved small COMMIT/barrier cut or owner-wise
full-corpus scan to remove. Reserve/physical durability/confirm, prepared-work
replay checkpoints and bounded child-first installation remain mandatory.

E04 owns one original4K queue after decisive source/diagnostic evidence. E07
consumes that result, then requires quiet Off/Record≥3 pairs, Large/XL, original
960s mixed, total RSS/allocated space and fault/churn acceptance. These are still
separate open gates. [Implementation and verification scope](../../plans/oct-22-finalizing/VALIDATION.md#e07-selected-artifact-phase-parenting-20261025).


### Sync observer P2 and actual fault follow-up — 2026-10-25

A generic clock/sink reentry defect is repaired in the original VFS observer.
No callback or span drop runs under the TLS borrow; reinstallation checks owner,
exact capture session and the existing write-span identity. Independent nested/
pending/colliding-session regressions pass. At314666fa Runtime strict, VFS48,
short-write/last-pin2 and UDS fixed-cap1 are VERIFIED. The original actual ANN
seal ENOSPC1 also passes on a64MiB isolated HFS+ filesystem with cancellation,
old/current view preservation, same-operation recovery and confirmed detach.
That is one fault oracle, not the complete operating matrix. Linux FUSE fsync/
close bodies were BLOCKED at that checkpoint. The2026-10-26 local Linux replay
now executes original-FD faults and managed ANN fault/recovery cases;
[the current source-bound result](../../plans/oct-22-finalizing/VALIDATION.md#local-fuse-fault-product-20261026)
supersedes the old environment blocker without closing the operating matrix.

Actual Cargo-built symbolized daemon/test attribution is VERIFIED against the
measured daemon UUID. The partial4K body passes1/1: Source17.847s Published,
128 exact Completed/55.578s, COMPLETE/health/loss0. Actual worker frames include
default automatic checkpoint, native SQLite sync and separate parent/object
sync; they do not quantify whole-run cost or justify dropping barriers.
This separate diagnostic profile cannot supply default timing/Off-Record
acceptance. Cargo restores default artifacts and their independent filtered
gold passes1/1. A later original4K normal-admitted Off run is FAILED0/1 at
initial materialization:4020/4096 exact receipts in1162.509s, no Q64,provider
4034unique/0duplicates. Source17.246s succeeds. The post-failure snapshot4031
completed/64pending/1claimed is non-atomic; no budget/quality gate is relaxed.
E04 remains the single run owner, and new source-backed RCA precedes a retry.
[Commands and current scope](../../plans/oct-22-finalizing/VALIDATION.md#e07-sync-observer-reentry-20261025).

### Prepared-work indexed-write cut — 2026-10-25

After314's normal-admitted Off full4K failure,5510c600 changes only Catalog's
stage_work writer to payload/digest-only UPDATE. All existing claim, row bound,
clock, root protection and durable COMMIT checks remain. The pinned engine's VM
write cursors decrease5→1 and secondary IdxDelete4→0. Production behavior is
focused VERIFIED: repaired Catalog1 with unchanged256 reused at ec9b, strict,
Kernel73/73 plus existing skip1, and actual daemon filtered gold1/1.
The same5510 Record prefix passes1/1:Source19.850s,128 public completions54.269s,
COMPLETE/health/loss0. Its exact stage_work scopes record191 calls/4770 xWrite
callbacks/12477088 requested bytes. These are not physical bytes or paired
throughput; original4K and all operating gates remain open.
Following two admission blocks, whole4K Record RCA runs on5510 after normal
admission(foreign Rust0/load7.351562<8). Source4096 publishes18.232688792s, but
the body is FAILED0/1:3556/4096 public completions at1160.546s before Q64.
Provider3576unique/0duplicates/0queries. The closed trace is COMPLETE/healthy/loss0 after one canonical parse; no duplicate product run or unmeasured redesign is assigned. No matched Off/Record overhead or speedup claim is made.
Whole-capture native+post union499.929s and managed COMMIT482.249s overlap
by302.623s; parent sync125.133s is inside post. Stage_work totals30.486s.
Sourceb62c47c9 repairs Store→Catalog managed-span parenting, preserving original
request/Off/authority/barriers. Strict, owning1 and actual128 diagnosis1 pass;
all1094 actual managed calls expose one child COMMIT/lock/BEGIN/VFS scope.
The complete128 capture is22.96MB/health/loss0, distinct from full4K acceptance.
[Current implementation, actual costs and limits](../../plans/oct-22-finalizing/VALIDATION.md#e07-stage-work-payload-only-20261025).

### Writer hot-path repair and bounded paired diagnosis — 2026-10-25

Source31aec3e7 reuses the event encoding Vec and replaces per-event dynamic
stage/counter name formatting with the same79/71 static wire names. Strict,
recorder22/22(existing20+fixed gold2), actual128 diagnosis1 and three serial
Off/Record128 pairs6/6 pass. The first functional diagnosis is contended; the
six paired starts are normal-admitted with foreign Rust0. Record3 captures are
COMPLETE/health/loss0. Median paired body/readiness increases are1.142%/1.187%;
driver-tree CPU increases4.177%. These are bounded observed ratios with visible
variance, not Large's formal per-phase gate or aggregate resources. Original
Large overhead remains NOT_RUN. The current-source original4K Off is now FAILED0/1 at incremental Q64 seed43
under the shared1200s deadline, after all4096 public completions, initialQ64,
cold-reopenQ64 and one-owner delta completion pass. No full4K acceptance follows.
SEARCH's subsequent52296745 query DATA-reuse batch is focused VERIFIED132/132
plus strict, preserving original charges/validation/pin lifetime. Its first original4K admission is BLOCKED at foreign Rust2. After those processes terminate, normal admission(foreign0/load5.656738)
starts one current-source body, now FAILED0/1(1210.51s). All4096 materializations
complete1163.968s; selected source inventory consumes the remaining shared1200s
before Q64. Source15.487s succeeds, corpus provider duplicates0, daemon exit0,
source/binary drift0. Query improvement is not measured by this failure. E07
consumes SEARCH's same deferred owner-install code batch without a second
queue. Core/Storage/Kernel scope, stage-aware original readers, early write
flush and explicit token completion are written; sourcea646 strict and
focused139/139 including actual UDS192 pass. Original4K normal admission is
initially BLOCKED by foreign Rust2/load13.039>8. Fresh normal admission at
2026-10-25T05:57:27.500813Z(foreign0/load6.456055) starts one original body,
now FAILED0/1(body984.14s) at unsaved-overlay Native Keyword lowering. Source
14.672099875s and4096ready881.921s, initial/cold/incrementalQ64 plus path pass.
This is not a watchdog failure or a paired speedup. The same SEARCH queue
implements canonical Keyword token-sequence execution/admission; current4322046b
strict and focused157/157, including actual UDS overlay/short-write/last-pin/
fixed-cap pressure/filtered gold/isolated-volume ENOSPC, pass. The one supported
contention-override original4K is now FAILED0/1(body1209.84s) at shared1200s in
the overlay sweep: Source15.334886083s,4096ready1037.647s, initial/cold/deltaQ64
and selected path checks pass. The old Keyword InvalidInput no longer appears.
After the previous foreign build ends, fresh normal admission is BLOCKED/body0
by new foreign4/load9.756348>8. No Root Rust/SUT remains live; no fixture or
product budget is changed. No unchanged contended full run or installation/publication protocol fusion is
assigned. Aggregate RSS and formal operating/
full-profile observer gates remain open.
[Current query code and proof boundaries](../../plans/oct-22-finalizing/VALIDATION.md#e04-query-scoped-data-20261025).
[Current code, paired results and limits](../../plans/oct-22-finalizing/VALIDATION.md#e07-trace-writer-allocation-20261025).

The final static overlay audit confirms that a two-term phrase currently
validates the union of both posting lists. A common4096/rare1 case therefore
advances about4097 postings, but the exact late-corruption and declared-DF
oracles require EOF validation. No authenticated skip/rank certificate exists;
a rare-driven point-lookup path that ignores the tail would weaken the contract.
This source complexity is not a measured critical-path contribution. The
failure's provider snapshot is cumulative since reopen: relative to selected
path/delta, overlay adds29 row and29 query embeddings, not30 overlay rows or
two overlay row kinds. Request-local Fast results are reused; expected_corpus
is identity checking, not cross-request prepared-vector authority. No proven
narrow missed reuse or safe I/O cut is identified under the current contracts.
Keep attribution UNKNOWN until a valid source/host-bound phase measurement;
do not add a second engine/cache or call either observation the measured cause.

### Current query phase observability — 2026-10-25

The existing recorder lacked actual Snapshot/Fast/Deep, lexical and query-provider
envelopes. Source7a0828f7 adds six closed-vocabulary stages and five aggregate
posting/phrase counters on the original paths, without per-posting spans. Kernel
rebinds diagnostic context on the same WorkBudget and restores it on every exit.
Off uses the original call path without a query clock or budget clone. Provider
spans measure invocation envelopes, not pure external I/O; descriptor/checkpoint/
validation retain their original order. A proved P2 in the new diagnostic code,
provider cancellation recorded as Error, is repaired with typed/cancel-state
classification and Off/Record result/WorkUsage regression. Current strict/Rust274 and Python702/2existing skips are VERIFIED. The single
original4K Record fails at Source30s before query; the initial capture is complete/
healthy/loss0. Formal Large overhead stays NOT_RUN.

Snapshot/Fast/Deep are siblings of Backend under the request root. Lexical is
inside Fast and query/overlay provider envelopes inside Deep. Never add these
nested/overlapping durations. First/middle/last ordinary materialization,
managed COMMIT/sync and periodic seal envelopes can distinguish the next source
cut. Static duplicate immutable-base CAS prechecks and map DATA decoding are
candidates only: existing traces cannot assign their physical read contribution,
and removing prechecks may change error priority. Keep them unpatched until
measurement or an independent preserving contract justifies the change.

The stopped normal432 body has no final verdict. The next original4K Record uses
the same fixture and shared1200s, two original epochs and each1GiB cap. Each epoch
must close before one canonical parse confirms complete/loss0/health0/parent
integrity. A failed body can supply a valid diagnostic capture, never full4K
acceptance or formal Off/Record overhead. Existing Linux resource/source binding,
Large/XL/mixed and operating gates retain their original requirements.
[Current commands and results](../../plans/oct-22-finalizing/VALIDATION.md#e07-query-phase-observability-20261025).

The small actual product daemon now independently verifies both durable and
unsaved-overlay query ancestry, fixed owner/path/revision/cosine and Off/Record
results/scores after reopen. Its bounded trace checks both IPC request roots,
all six new stages and complete footer/loss0; final8387 strict and1/1 pass.
The original product source is unchanged from7a. The same source's normal-only
original4K admission is BLOCKED/body0, so query critical-path proportions and
original Large observer overhead stay unqualified. A suspected default Kernel
context suppression is falsified by the existing context.map semantics and
independent inherited-context oracles; no redundant context API/patch is added.
[Final evidence and remaining gates](../../plans/oct-22-finalizing/VALIDATION.md#e07-actual-uds-query-trace-20261025).
The third consecutive goal-turn admission remains BLOCKED at09:30Z,foreign
Rust3/load32.417480>8,body NOT_RUN. This records local admission blocked by
foreign work at that checkpoint, not a requirement to acquire an external host.
Resume the same original4K queue after normal admission; local code/function
work continues independently. Formal Linux/source inputs and every original
operating/observer gate remain.


### Independent code work while performance admission is blocked — 2026-10-25

The prior full-goal blocked wording was too broad for source work. The latest
user direction resumes independent code remediation before batched runtime.
Source1624aff0 adds bounded query Err observation under the existing
QUANTA_INDEX_NATIVE_WORK_DIAGNOSTICS_V1 flag. It captures only request/connection
IDs, successful Fast/Deep/terminal delivery flags, fixed error class and first
local WorkBudgetDenial. The process-local prefix is at most8 records; dispatcher
Ok results do not read the flag, format or read a diagnostic clock. An Err after
an already delivered terminal is still observed. Full WorkBudget
Debug and provider/query content are excluded. None means no observed local
budget denial, not proof of a scheduler/provider cause. Existing replies,
terminal-emitted/no-second-frame behavior and provider custody stay unchanged.

Root connects the existing ANN successful-run relay and actual daemon valid-
syntax/unsupported-selector refusal witness; independent footer/phase/result
checks remain. The actual run exposed an earlier successful query's postterminal
Cancelled record, which the first witness confused with its explicit refusal.
Only terminal-emitted Cancelled is capped at2 of the total8 records so it cannot
consume every later error slot; a deterministic100-repeat regression preserves
6 ordinary slots. Preterminal cancellation and postterminal Storage keep the
original sampling path. The product witness distinguishes these cases and
retains typed refusal and all original trace/result oracles. Final eed03da5
Runtime all-target strict,diagnostic10/10,actual effort UDS42/42 and product3/3
are VERIFIED,source drift0. The3 include the actual daemon Off/Record result/
score/phase/ancestry/footer/loss/privacy witness and two existing relay tests.
NativeText143/143 retains its unchanged earlier1624 lexical scope; these are
not a single-source198-test result. No formal timing SLO is promoted.
The Linux audit confirms kernel memory.peak is already wired to execution.json;
it is cgroup-accounted memory, not a proved aggregate-RSS upper bound. Do not
create a duplicate sampler or relabel sampling as continuous peak proof.
[Code-first evidence](../../plans/oct-22-finalizing/VALIDATION.md#e07-code-first-remaining-20261025).

<a id="e07-resource-authority"></a>
## Resource acceptance instrumentation boundary — 2026-10-26

아래 authority 한계와 고정 resource 계약은 유지한다. 10/26의 실행·host 부하
관찰은 해당 source의 이력이며 현재 build/원4K 실행 상태는 README E07을 따른다.

User direction is to maximize local completion without requiring an external
host for code or functional checks. The existing Docker Desktop Linux aarch64
kernel supports genuine cgroup v2 delegation inside a new owned private cgroup
namespace. Current same-source Python impact checks are Linux817passed/5skipped
and Darwin807passed/15skipped after the observation-deadline correction below.
These are functional checks, not the original
quiet2CPU/6GiB/local-SSD performance profile. No foreign container is modified.
Landlock enforcement is unavailable on this local kernel (ENOSYS even without
the Docker seccomp filter); its three native bodies stay BLOCKED/NOT_RUN.

The existing qualified Linux owner reads bounded smaps_rollup Rss per process.
Actual local execution exposed normal exit_mm/ESRCH before state becomes Z;
the owner now corroborates PID/start/cgroup and kernel exit state, preserving
normal cleanup and refusal of unknown live data. A leader can exit while worker
threads keep its mm resident. The same owner enumerates cgroup.threads and reads
one live representative with before/after TID start,Tgid,cgroup and original
TGID identity checks. RSS and CPU evidence remain keyed by TGID, summed once per
process. A successful RSS read is retained through a subsequent Z transition
after identity/membership revalidation. One metadata validator has no state-drop
boolean. No second collector,process issuer or approximate-RSS fallback exists.

Actual opt-in tests cover root+child normal exit, timeout while root waits,
root-first exit with a surviving child,and pthread_exit followed by a worker
allocating/touching64MiB only after the leader is Z. They require an explicit
delegated cgroup,check positive RSS,subgroup removal and parent preservation.
Permissions,malformed data,PID/TID reuse,Tgid changes and migration still refuse.
Point samples are not atomic family snapshots or continuous peaks; shared pages
may repeat and short peaks can be missed. Diagnostic process-group stat RSS and
the inner Native sampler retain their declared diagnostic methods. Smaps observer
cost itself still needs performance measurement. See the
[Linux RSS contract](https://man7.org/linux/man-pages/man5/proc_pid_statm.5.html)
and [thread proc interface](https://man7.org/linux/man-pages/man5/proc_pid_task.5.html).

Both existing Linux owner loops now check the time after sample and completion
observation before accepting an empty/dead family. A final slow sample or
completion read at/after the original deadline cannot return timed_out=false.
The checked observation timestamp is also the elapsed_ms endpoint. Qualified
empty+late avoids an unnecessary cgroup.kill while retaining escape cleanup,
wait-empty, final accounting, root wait and subgroup removal. Original request,
absolute deadline and separate cleanup grace remain unchanged. Twelve fixed
clock cases cover both owners, a tighter absolute deadline, before/exact/after
expiry and a slow final sample. No real sleep or fake-PID signalling is used.

ProcessResult.rss_method binds the existing producer to both resource consumers.
Persisted generic resource schema3 requires the backend-matching method and
rejects methodless schema2. The Native capture product validator and timeline
retain the outer observation and refuse mixed sampled aggregate RSS above2GiB.
Ordinary non-mixed profiles do not inherit that cap. Malformed backend,escaped
identities and storage kinds produce typed failure; false/0.0 cannot represent
integer exit0. Capture fixtures now produce final file dev/inode after copying
across filesystems; consumer raw-epoch checks remain strict. The diagnostic
cleanup fixture waits for its child before the original0.3s timeout measurement.

The producer still emits continuous_rss_peak=false and allocated_disk_peak=null.
Kernel cgroup memory.peak is charged memory, not summed RSS; RUSAGE_CHILDREN
ru_maxrss is the largest child, not the process-tree sum. Neither substitutes
for the lifetime DoD. Native inner observation excludes startup and final drain;
the outer cgroup owner spans harness/descendants through cleanup but remains
sampled. Shape validation does not authenticate arbitrary JSON as host proof.
The producer is `_linux_execution_observation` in causal_cost_capture.py;the
existing `_native_linux_resource_observation` validator in causal_cost_profile.py
requires that exact false/null diagnostic contract. An authoritative upper-bound
method therefore needs producer and consumer changes together if the selected
method is not represented by the existing path. This seam is conditional on a
verified authority,not permission to populate true/peak fields from samples.
The [kernel cgroup documentation](https://www.kernel.org/doc/html/latest/admin-guide/cgroup-v2.html)
defines memory.peak over charged cgroup usage and describes shared memory ownership
across cgroups;that accounting cannot simply be renamed to summed process RSS.

Native child TMPDIR stays private below the capture root and the locked socket
shape is checked before output/build. This is placement, not write confinement
or all-root quota. Existing legacy/Native logical byte/file maintenance reserves
are reused and are distinct from physical resource measurements.

Remaining resource acceptance under existing E07/PHYSICAL ownership:

The original contract requires full-lifetime coverage and the fixed resource
ceilings. It permits existing admission/OS high-water evidence; it does not
require an exact instantaneous time series, a new sampler or a per-FD ledger.
A sound conservative upper bound with complete custody may suffice. These are
unresolved authority/evidence inputs, not three demonstrated missing code
bodies. Select and verify the authority before assigning implementation work;
absence of final proof alone is not an observed product defect.

1. Whole-lifetime summed RSS: a compatible authoritative method and external
   migration prevention are required. Faster sampling,VmHWM,RUSAGE_CHILDREN or
   relabelled cgroup memory.peak are insufficient. pids.max times inherited
   RLIMIT_AS is a conservative enforcement alternative only with exclusive
   controls and privilege closure; it counts threads and can overrestrict this
   workload,so it is not selected or silently imposed as the product profile.
   pids.peak times wait4.ru_maxrss is also rejected: this kernel's pids watermark
   can miss a race, auto-reaped children can omit wait4 evidence, and ru_maxrss
   uses approximate kernel RSS counters. Multiplication does not close those
   missing upper-bound premises.
2. All writable roots: bind scratch,state,temporary,logs/traces,WAL,retired/pinned
   objects and relevant open FDs to actual allocated-block authority/enforcement.
   A bounded dedicated filesystem/quota plus write confinement is a candidate;
   it must cover all roots and deleted-open files. A4GiB cap on the whole fixture
   is stricter than scratch-only and cannot silently replace that contract.
3. Once a feasible authority exists, check whether its output needs a concrete
   connection to the existing capture→profile verdict with source,binaries,
   config and first/reopen epochs. Implement only a demonstrated missing seam.
   Shape-only validators or caller proof booleans cannot supply that authority.
   Current false/null markers remain accurate; incomplete proof stays
   non-qualified.

**2026-10-31 local method decision.** The original summed-process-RSS ceiling
cannot be qualified on the current local owner output. The qualified cgroup
owner retains the whole process lifetime and a sampled `smaps_rollup` sum, but
neither that sum nor cgroup `memory.peak` is a conservative upper bound for the
specified simultaneous per-process RSS sum: observations can miss short peaks,
and a shared mapped page can contribute to more than one process RSS while
being charged once by the cgroup. The selected local method is therefore
`BLOCKED`, not an inferred pass or a demonstrated product overrun. Rejected
`pids.peak × wait4.ru_maxrss` and unselected `pids.max × RLIMIT_AS` are not
reopened as implementation tasks. A later RSS method must prove complete
membership/migration control and an upper bound for the original sum without
silently imposing a stricter product workload limit.

For disk, a task-owned probe confirmed that the cached Linux arm64 image can
start with a read-only root filesystem and has ext4/FUSE plus `mkfs.ext4` and
`losetup` available. The prior owned FUSE fault rail proves private mount and
device access, not allocation limits. A private bounded physical filesystem
with a scratch-specific project quota remains technically plausible, but no
project-quota inheritance, writable-mount/FD confinement, deleted-open-file
accounting, or exact block high-water was exercised by this capability probe.
It is **not selected as an authority**. A total-fixture 4GiB bound would alter
the scratch-only 4GiB contract; keep the separate 64GiB physical ceiling.
Only after those premises are demonstrated should the existing capture
`_linux_execution_observation` and profile `_native_linux_resource_observation`
be extended together with typed source/binary/config/epoch-bound evidence.
The local code/functional and sampled-exceedance rails can finish independently;
whole-lifetime RSS/all-root allocated-space acceptance remains `BLOCKED`.

Local code/function checks do not wait for an external host. The same-source
Linux817/5 and Darwin807/15 results above close the repaired seams;
source-identical repetitions add no proof. Continue independent code/fault
checks only when a specific uncovered body or defect justifies them. The next
original4K performance body uses the existing normal admission, whose latest
observation is foreign Rust3/load25.371094>8: BLOCKED,body NOT_RUN. No unchanged
contended4K rerun is assigned. There is no further confirmed code hunk in the
audited E04/diagnostic seams at this checkpoint. This is not a whole-product
or whole-repository completion claim.

Resource concerns1/2 remain distinct; freeze a feasible authority before any
parallel source changes and serialize shared integration. No speculative second
sampler, allocator or mount orchestrator is assigned.
E01/E02/E05 supplier work and E06 cutover retain their owners. Original Large/XL,
paired observer,mixed/fault/churn and release DoD are unchanged.
[Current source,failures and final local evidence](../../plans/oct-22-finalizing/VALIDATION.md#local-linux-rss-lifecycle-20261026).
[Authority audit and local continuation boundary](../../plans/oct-22-finalizing/VALIDATION.md#local-resource-authority-audit-20261026).
[Observation-deadline repair](../../plans/oct-22-finalizing/VALIDATION.md#local-observation-deadline-20261026).

## Local managed-publication fault completion — 2026-10-26

Local Linux FUSE is usable on this Mac through a new owned container; no external
host is required for these functional bodies. The pinned passthrough backend
uses cache=auto for SQLite WAL/mmap, preserves exact original-FD fsync/flush EIO,
and forwards atomic backing renameat2/NOREPLACE without overwrite fallback.
Only the owned external backend and test environment change. FD fixtures use
TMPDIR=mount; managed ANN fixtures explicitly use tempdir_in(mount) with normal
/tmp. All cases are serialized under existing resource admission.

At source6121811e/ELFd5a55299 all five selected bodies finish: original-FD2 and
managed ANN fsync,checked-close,actual ENOSPC. The combined ANN command reaches
its600s runner timeout after close/fsync finish; that batch remains FAILED.
Only unfinished ENOSPC is rerun with the same source/ELF/input and600s guard;
its full summary is VERIFIED1/1 in88.65s. No green aggregate command is invented.
The close oracle authenticates the held A pin's original session/fence/expiry/
view and all residual child A/B custody. Actual post-exit recovery retires1 dead
session and2pins to0; original Pending131,same-key replay and fixed source/
owner/score/coverage/reopen gold pass. An exact aggregate pin_count==1 expectation
would incorrectly reject the failed materialization's additional retained pin.

These local fault bodies do not close the entire operating matrix or qualify
performance/resource peaks. The final normal original4K preflight is BLOCKED
at2026-10-26T02:29:31Z,foreign Rust3/load25.371094>8; body NOT_RUN. Preserve all
original request/worker/quality limits. Full-lifetime resource authority and
formal paired/Large/XL/mixed/churn/release remain separate. Existing source/
static evidence does not establish another specific production code defect in
the audited E04/observation seam.
[Exact source,commands,failed cohorts and final functional results](../../plans/oct-22-finalizing/VALIDATION.md#local-fuse-fault-product-20261026).


## User-requested original4K RCA — 2026-10-26

The user requests an actual run after the normal-admission block. The existing
supported contended diagnostic mode runs source38f70c96 after canonical current
bench/all-feature compilation. Original request/work/quality limits remain.
Actual body is FAILED0/1,1073.76s: Source4096 Published25.423311s,then owned
materialization Blocked(ResourceLimited),2097/4096 observed completion bindings.
The two bounded refusal records show the shared original10s grant deadline,not
1Mreads/512MiB exhaustion. Initial trace is VERIFIED COMPLETE,578405827B,
2912311events,loss/open/leases0; query/reopen/later phases remain NOT_RUN.

Worker active intervals sum934.832s; inside those workers Catalog hold union
sums704.649s and managed-or-SQLite-sync union673.729s. These overlap and must not
be added. Semantic build/seal spans separately total1.510s/2.266s. This points
to durable metadata work as the measured dominant region,without proving CPU,
physical I/O,observer overhead or host causality. Two error workers spend
10.317s/10.922s,including long COMMIT/sync boundaries. Current bounded stderr
operation keys cannot exactly join those qtrace worker roots or public owner718.

The original investigation now binds the existing worker DiagnosticContext/trace ref
and original operation/grant in the same bounded refusal path. Final affected
regressions21/21 and Runtime library strict Clippy are VERIFIED; all-target
strict retains separate Core/risk-suite failures. The connection can identify
the exact worker subtree when a selected failure is emitted; a zero-refusal
cohort cannot supply a nonzero product join or justify a guessed performance patch.
[Connection implementation and narrow verification](../../plans/oct-22-finalizing/VALIDATION.md#e07-owned-grant-trace-correlation-20261026). Preserve fsync/CAS/
reservation/pin authorities and all10s/30s/work/quality limits. No unchanged
contended rerun,second observer,duplicate engine or speculative collector is
assigned. Resource-authority selection and formal paired/Large/XL/mixed/churn/
release remain original gates.
[Current body,capture,measured costs and inference limits](../../plans/oct-22-finalizing/VALIDATION.md#e07-user-requested-original4k-20261026).

The subsequent source1182969a Record run fails the whole watchdog with3302/4096
exact completion bindings observed; Source18.157681s passes. No owned refusal
is emitted, so its product failure-join coverage is NOT_APPLICABLE, while the
actual worker child regression21/21 proves the new connection. Its complete
847MB capture shows worker-active1091.409s,Cataloghold818.392s and worker
lockwait1.853s. This is throughput failure, not another observed10s grant
denial. A same-source/binary Off control retains every original product/quality
limit and is diagnostic only on the contended host. Static pacing inspection
does not justify changing500ms/64-owner polling or weakening the exact bitmap.
That Off control also expires at the shared watchdog with2953/4096 completion
bindings observed; Source27.771361s passes. Both modes fail without an observed
owned-work refusal; causal overhead and formal <=5% acceptance remain unproved.
[Current Record result and Off control](../../plans/oct-22-finalizing/VALIDATION.md#e07-owned-grant-trace-correlation-20261026).

## DoD

- [ ] E01–06의 실제 제품 성공 뒤 Large/XL ACK/current·old search/reopen·같은 operation 복구를 독립 gold로 수용.
- [ ] warm lexical 예정 발행→decoded top20 p95≤100ms/p99≤500ms, one-file staging→ACK→검색 freshness p95≤1s/p99≤5s를 원 운영 profile에서 수용.
- [ ] aggregate 모든 process RSS≤2GiB·allocated scratch≤4GiB·physical≤64GiB, 실제 high-water·orphan/retired/WAL/FD/pin과 누락 없는 비용 회계. sampled maximum은 continuous peak가 아님.
- [ ] pin 해제 후60초 안 reclaimable debt0·shutdown grace5초·지속 churn/debt/drain. 보호 중 객체는 공간 회계에 포함.
- [ ] ENOSPC/short-write/fsync/checked-close·cancel/panic/process-death·long pin·space pressure에서 accepted publication 유실0, old-view 정확성·bounded cleanup.
- [ ] initial build·fixed delta/no-op·N/H/M·fanout/history·공통 term·1/100/1000-file batch·branch/watcher/model/policy 변경을 독립 축으로 계수. 숨은 전수 scan과 maintenance debt를 포함.
- [ ] [RFC 12.3/12.5절](../../rfc/oct-10-native-incremental-vnext.md#123-latency-stability-and-qualification-envelope)의 원 native/IDE profile을 실행 전에 고정하고 latency/quality/freshness·served rate·전체 비용을 함께 E08에 인계.

기존 사용자 유예(continuous RSS/전체 roots allocated peak·delegated Linux 등)는 실행 순서 유예이며 최종 DoD 면제가 아니다. 입력 부재는 BLOCKED/미실행은 NOT_RUN으로 남긴다. 기존 [세부 운영 사양](../oct-22-finalizing/oct-17-index-closeout/tickets/W2-09-cost-observability.md)의 cap/clock/capture/오류/settlement 계약을 보존하며 결과를 본 뒤 같은 실행의 합격선을 바꾸지 않는다.
