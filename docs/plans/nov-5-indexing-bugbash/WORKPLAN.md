# 구현·검증 계획

## 현재 구현과 다음 수용

- B75·B76·B78·B79·B81·B82는 공유 소스에 반영했고 집중 회귀를 확인했다.
  제품 상태는 README의 최신 실행을 따른다.
- Mixed의 직접 실패는 r53 query preparation이다. capture와 harness는
  Mixed Off만 지원한다. B81의 여섯 구간을 기존 선택된 retained-request
  진단 표본에 연결해 connection 대기, row·clock·COMMIT을 구분한다.
  그 결과로 검증 횟수 통합이나 writer admission 변경의 소유 구간을 정한다.
- XL은 Posting SQL과 설치 COMMIT의 비용이 남았다. 두 패킷 통합에는
  scratch 확인과 installer memory를 함께 승인하는 계약이 필요하다.
  다음 source-bound XL은 해당 owner의 코드 변경과 집중 회귀 뒤에 둔다.
- ASCII token 확장과 원 512MiB Native SDK 경로는 Native SDK 담당 범위다.
  관련 두 파일의 중복 구현을 피하고 아래 B58의 메모리 회귀를 확인한다.
- Runtime strict Clippy의 기존 9건은 README에 별도 실패로 남아 있다.
  실행 수용과 컴파일 품질의 상태를 각각 유지한다.

## B03-B29 — StageSources 설치 트랜잭션 감소

- XL r34 Record에서 32,768개를 1,024번 보내며 Catalog reserve·confirm
  트랜잭션을 각각 1,024번 열었다. Staging은 최초 `SourceAdvance` ACK 구간 앞이다.
- `StageSources`의 요청 상한을 Contract에서 128로 정의하고 Kernel과 SDK가
  공유한다. 기존 8MiB weighted 및 encoded-object 한도는 각각 유지한다.
  최대 256개 객체의 child-first 내구 설치, 본문 중복 제거, 결과 순서와 각 요청의
  work·memory 한도를 유지한다. 같은 입력의 요청은 1,024→256회, 트랜잭션은
  2,048→512회가 예상된다. 실제 실행 전까지 횟수와 wall을 미확정으로 둔다.
- Wire variant와 저장 형식은 유지한다. 구 daemon은 32개 초과를 설치 전에
  거절한다. 33–128개 호출은 대응 daemon과 함께 배포한다. 새 SDK의 128개
  요청/응답과 129개 사전 거절, raw UDS의 Kernel 거절과 같은 ingress의 정상을 검증한다.

## Mixed 거절 경로 분리

- Harness 실패 prefix에서 최초 검색 task의 단계·지연·제한된 오류, 발행의 ACK와
  이후 head·현재/이전 검색, 세션 정리를 분리한다. Native는 retained 검색·제어와
  `ReadView` 각각 최대 8개 실패를 기록한다. 실행 grant, work deadline, 조회 pin과
  성공 응답 계약은 유지한다.
- 취소된 offered scheduler는 `Err`만 반환해 enqueue·queue full·late drop을
  잃었다. B32는 실패 prefix에 scheduler 소유의 고정 크기 부분 카운터를 추가한다.
  계획·도달·큐 결과·worker 완료를 분리하고, 부분 실행을 정상 `OfferedRun`으로
  처리하지 않는다. 기존 성공 결과와 호출 API는 유지한다.
- 같은 session의 검색은 `ReadTicket`을 전체 검색 동안 배타적으로 빌린다. 공유
  세션의 병렬 검색에는 Kernel의 읽기 전용 borrow, 같은 pin 소유 확인, Release
  진입 차단과 in-flight drain, renew와 query 검증의 순서가 필요하다. 별도
  Interactive 경합이 확인되면 기존 자원 예산 안에서 그 승인 경로를 처리한다.
  관측된 거절을 분리한 뒤 해당 경로의 동시성·취소·old-view 회귀를 추가한다.

## B03-B31 — Source record 재읽기와 Mixed 갱신 경계

- SourceApply에서 읽은 record는 같은 attempt의 Text `SourceInputCursor`에서도
  필요하다. 기존 4MiB DATA cache에서 source record와 일반 DATA에 각각 2MiB를
  기본 배정한다. 일반 2MiB보다 큰 본문 하나는 총 4MiB 안에서 미사용 공간을
  쓸 수 있다. SourceApply의 record 경로만 우선 보존을 요청한다. 재사용 시 기존
  `read_retained` work charge와 ref 확인을 유지하고, 실제 record decode와 UTF-8
  body 검증은 원래 경로에서 수행한다. 충돌하거나 한도 밖이면 원 물리 읽기다.
  절감 대상은 Text의 후속 record 재읽기다. 최초 OwnerPrepare나 body 비용은 별도다.
- 원 요청 예산을 retained pin 갱신에도 전달한다. COMMIT 전 취소·deadline이면
  같은 lease/revision을 유지하고, COMMIT 성공 뒤 응답이 끊기면 새 revision을
  재생한다. HeldRead 갱신과 Kernel stage 갱신이 같은 계약을 사용한다.
- Mixed publisher가 예정 시각에 도달한 tick을 held read·정리·snapshot 단계보다
  먼저 집계한다. 이 단계에서 거절돼도 분모와 단계가 남는다. 1Hz·50 QPS와 기존
  성공 판정을 유지한다.

## B03-B33 — Packed occurrence의 pending 슬롯 재사용

- 최초 term이 확인한 pending 슬롯을 같은 packed block의 alias에 전달한다.
  각 alias는 scratch 변경 전에 슬롯 번호와 정확한 `ObjectRef`를 대조한다.
  기존 ref의 payload 동일성 검사는 최초 삽입에서 유지한다.
- 기존 방식은 최대 512개 term 각각에서 최대 256개 pending 객체를 순회했다.
  새 alias 조회는 슬롯 조회 한 번이다. 설치 배치, fanout, work 예산, persisted
  wire는 유지한다. 잘못된 슬롯이 planned row를 추가하지 않는 회귀를 확인한다.

## B03-B34 — Interactive 예약의 bounded FIFO

- `lm02`의 retained 검색 거절은 Interactive 승인 단계에서 관측됐다. 기존
  ledger의 capacity와 보호 floor를 유지하며, 일시적 경합에서만 고정 크기 FIFO로
  기다린다. 자리와 선행 대기가 없으면 기존 즉시 승인 경로를 사용한다.
- retained·일반 Text 검색과 Native ingress·metadata의 Interactive 경로가 원
  `WorkBudget`을 사용한다. 검색은 준비 단계 Control 예약을 놓은 뒤 기다린다.
  취소·만료는 대기 항목을 제거한다. 최종 예약은 기존 `ReservationGuard`를 소유한다.
- 집중 검증은 FIFO 순서, Control floor, cancellation 및 예약 환급을 확인한다.
  제품 검증은 원 Large120과 Mixed 50 QPS·1Hz·960초를 사용한다. 6.419초의
  발행 ACK가 대기 변경만으로 1Hz 기준을 충족할지는 미검증이다.

## B03-B35 — Cold ASCII owner의 토큰 scratch 후보 철회

- 빈 이전 inventory를 가진 packed 초기 구축 중 1–4,096바이트 ASCII owner만
  대상으로 한다. 기존 tokenizer가 생성한 두 case의 key와 position을 받아
  BLOB key·position 순서로 정렬한다. token table의 `DELETE`·`INSERT`·`SELECT`를
  생략한다. 증분, Unicode, 크기 초과 및 선택적 메모리 승인 거절은 기존
  SQLite 경로를 사용한다.
- 최대 8,192개 row, key backing 16KiB, 총 80KiB를 원 stage grant에서
  먼저 승인한다. 정렬 결과의 중복과 위치 상한을 첫 occurrence callback 전에
  검사한다. tokenizer, packed/leaf install과 원 논리 work 계상은 유지한다.
- 적대 검토에서 필수 작업과 같은 stage grant를 사용하는 문제를 발견했다.
  4KiB ASCII에서 선택적 arena 81,920B 승인 후 필수 TokenBatch 1,536B가
  거절될 수 있다. SQLite에서는 처리 가능한 입력이다. `468fa8f3…` 후보를
  철회했다. 다시 검토하려면 후속 필수 메모리까지 보전하는 별도 계약이 필요하다.

## B03-B36 — 일반 Text 실패 귀속과 ReadView 예산

- r40 Mixed의 첫 발행은 4.321초에 ACK했고 그 전에 retained 검색의 2초
  UDS timeout이 발생했다. ACK 뒤의 일반 Exact Text 거절은 별도다.
  기존 Off 실패 수집기에 일반 Text route별 첫 8개를 기록한다. 원 예산과
  Control, 준비, Interactive, 실행, 결과 변환 및 checkpoint 단계를 보존한다.
- ReadView의 Catalog validation도 원 요청 예산으로 writer mutex를 기다린다.
  Catalog pin release와 검색 lease의 수명 및 기존 Interactive queue는 유지한다.
  ACK·검색 지연의 개선이나 Mixed 수용 판정은 실측으로 확인한다.

같은 ReadSession의 read guard를 공유하는 다음 후보는 Kernel의 읽기 전용 ticket
borrow를 먼저 고정한다. 기존 mutable session의 renew 계약을 유지한다. registry는
Release의 Closing 선행·drain, Renew의 writer 우선권, validation 실패의 Unknown
전이를 보존해야 한다. Control 예약을 기다리는 query가 read guard를 쥐고 있으면
Control을 보유한 Release와 서로 대기할 수 있다. 예약과 guard 획득 순서를 함께
검증한다. Catalog의 writer 직렬화는 별도 병목으로 측정한다.

## B03-B37 — Control transient 거절 대기

- r41의 다음 snapshot `Open`이 `retained_control/control_admission`에서
  거절됐다. 기존 ledger에 Control 전용 bounded FIFO를 둔다. 기존 Control
  envelope, 원 deadline, cancellation과 legacy 즉시 호출을 유지한다.
- 일반 Text의 준비와 retained 제어 요청에서 이 FIFO를 사용한다. retained
  제어는 queue 대기 중 reply frame grant 또는 session lock을 보유하지 않는다.
  세션 mutex를 보유한 retained search에는 대기를 추가하지 않는다.
- 기존 Interactive FIFO와 Control release 우선순위의 회귀를 함께 실행한다.
  제품 검증은 같은 Mixed profile에서 `Open` 이후 진행, offer·publisher 및
  첫 ACK를 분리한다. 1Hz·50 QPS·960초와 검색 freshness 기준을 유지한다.

## B03-B38 — Retained 세션의 공유 검색

- 한 세션의 entry mutex가 계획·Interactive 대기·검색·변환·검증 전체를
  직렬화한다. 기존 pin을 공유 참조로 빌리며 read guard를 보유하는 검색끼리
  겹칠 수 있게 한다. 원 ticket을 복제하거나 새 pin을 열지 않는다.
- 사용자·세션 확인 뒤 Control을 먼저 승인한다. entry guard를 보유한 채
  Control FIFO에서 기다리지 않는다. 기존처럼 준비 후 Control을 반환한 뒤
  Interactive를 기다린다. 갱신·해제·sweep은 배타적이며, 갱신 의도와 Closing은
  새 검색보다 앞선다. hard validation 실패의 Unknown은 guard 안에서 기록한다.
- Kernel의 mutable session renew, 2개 query 동시 진입과 같은 pin 검증,
  Release의 Closing·drain, Renew 우선권, 취소·deadline에서의 ticket 보존을
  묶어 확인한다. Catalog validation의 writer 직렬화와 1Hz 발행 비용은 별도다.

## B03-B39 — 만료 tick의 retained 갱신 차단

- tick에 도달하면 기존 scheduled 분모를 기록한다. `scheduled + 1초`에
  도달했으면 missed로 기록하고 held 갱신·release·snapshot 접근 전에 건너뛴다.
  아직 유효한 tick에서만 같은 원 budget으로 lifecycle을 처리한다. 준비 중
  만료되는 tick은 발행 직전에도 검사한다.
- 기존 120초 renew 주기, 180초 최대 gap, 600초 보유 기간과 960초/50 QPS/1Hz
  성공 조건을 유지한다. 경계 tick의 정확한 missed 계상과 원 실패를 검증한다.

## B03-B40 — Catalog pin 검증의 SQL 준비 재사용

- session·pin·durable clock 조회와 clock 갱신을 Catalog writer의 기존
  prepared-statement cache에 연결한다. workspace key 최대 wire 폭의 성공한
  계산도 재사용한다. 검증 횟수와 순서, 오류, writer mutex, clock floor와
  COMMIT 계약을 유지한다.
- WAL의 deferred read는 다른 연결이 갱신하기 전 snapshot을 볼 수 있다.
  동일 millisecond라는 이유로 이 snapshot을 pin 검증의 권위로 채택하지 않는다.
  [SQLite 트랜잭션](https://sqlite.org/lang_transaction.html)

## B03-B41 — Mixed publisher의 직전 인증 view 재사용

- 시작할 때 Exact `ReadView`의 SDK 검증 결과를 가져온다. 이후 ACK 뒤 `Latest`
  일치 검사를 통과한 `CatalogView`를 다음 tick까지 보유한다. 다음
  `SourceAdvance`의 view와 source cut을 같은 값에서 구성해 매 tick의 중복
  Exact `ReadView` 한 번을 없앤다. 기존 CAS, 다음 snapshot pin, 현재·이전 gold
  검증, 검색 50 QPS와 1Hz 기준을 유지한다.
- `mutation_ready_from_scheduled_ms`와 제출 실패 단계를 기록한다.
  input 준비, mutation 준비, SourceAdvance 제출부터 ACK까지를 분리한다.

## B03-B42 — Posting scratch의 고정 폭 owner key

- 각 Posting row에서 32바이트 owner를 row에 인라인으로 보유한다. term의 원
  메모리 grant는 유지한다. 기존 SQL binder가 key slice를 받아 처리하며
  SQL, 64행 packet, 원 work 계상, 저장된 inventory root 재검증은 유지한다.
- 부분 XL에서 Diff 15.671초 중 PostingEdit SQL flush는 9.102초였다.
  owner heap 할당 감축은 그 바깥의 비용에 적용된다. SQL과 전체 XL 절감량은
  동일 기준의 제품 실행으로 구분한다.

## B03-B43 — 열린 namespace 디렉터리의 FD 신원 재사용

- root·objects 디렉터리는 실제 열린 `File`과 dev/inode를 함께 보유한다.
  매 검사에서 현재 path의 owner·mode·type 및 dev/inode를 대조한다. 동적
  lease FD와 링크 검사, 각 객체의 shard/file 검사 및 install 전후 검사는
  유지한다. [Linux unlink(2)](https://man7.org/linux/man-pages/man2/unlink.2.html)
- r39 Record의 install 내부 namespace 검사는 83,632회 합계 1.243초였다.
  이 수치는 r43 Off의 비용 귀속이 아니며, 객체 읽기의 별도 절감량도 미측정이다.
  같은 store를 연 상태에서 권한·rename·symlink 변경을 거절하는 회귀를 실행한다.

다음 Mixed 비용 귀속은 `lm06`의 제출→ACK 중앙값 2.742초를 대상으로 한다.
원 SourceAdvance 내부의 Catalog 대기/COMMIT과 lexical Posting 설치를 분리한다.
held-renew ACK 뒤 witness의 남은 tick budget 고갈도 별도 지표다. 1Hz·50 QPS·960초,
held 600초와 settlement를 그대로 수용 조건으로 둔다.

## B03-B44 — warm Posting의 이전 terms Map 배치 조회

- 같은 prior terms root에서 한 SQL page의 최대 64개 term을 기존
  `PersistentMapPort::lookup_batch`로 조회한다. Cold 빈 root는 lookup을
  생략한다. 배열 backing을 먼저 예약하고 기존 per-term wrapper, df,
  CAS, 독립 seal과 오류 시 미공개 계약을 유지한다.
- Storage `ScopedReads::lookup_batch`도 scalar와 동일한 인증 DATA
  window를 사용한다. 배치 내 decoded path 재사용과 페이지 간 물리 DATA
  재사용을 분리한다. 결과 parity, logical reads, physical get, root 변경,
  취소는 focused 회귀로 확인한다. 제품 latency는 별도로 검증한다.

## B03-B45 — Mixed held ReadSession과 발행 스케줄 분리

- E07의 1Hz 발행과 held 갱신은 같은 960초 load origin을 쓰는 두 일정이다.
  held worker는 발행·검색 backlog와 독립적으로 1초 tick을 사용한다.
  갱신과 witness의 한 tick deadline, 만료 tick 미실행, 120초 갱신 주기,
  180초 최대 gap, 600초 유지와 60초 settlement 한도는 유지한다.
- 한 worker의 실패로 전체 측정을 취소하고, 시작 실패나 panic을 포함해
  모든 worker를 join한 뒤 pin cleanup을 수행한다. 발행 분모, 검색 실패,
  held lifecycle 실패를 따로 보고한다. 발행 처리량·검색 SLO는 원 oracle을
  그대로 통과해야 한다.

## B03-B46 — 디스크 census의 중복 집계 제거

- 같은 inode에 fixture/state/scratch 포함 범위를 한 테이블로 기록한다.
  각 subset에 처음 들어오는 경로의 `st_blocks × 512`, symlink·hardlink
  처리, 디렉터리 재방문 방지, 취소와 500ms cadence는 유지한다.
- 기존 cross-subset hardlink와 누락·취소 oracle을 사용한다. 별도
  workload에서 관측기 wall 및 제품 latency를 비교하기 전에는 절감률을
  주장하지 않는다.

## B03-B47 — Pending settlement 응답 보존

- 마지막 정상 응답의 head·floor·대상 pin 여부를 실패 산출물에 보존한다.
  기존 60초, `SettledAtSnapshot`, 마지막 pin·floor 수용과 성공 artifact
  스키마를 유지한다.
- 다음에는 동일 Catalog snapshot에서 첫 차단 조건을 수집한다. floor·pin,
  work·custody, view retirement, GC queue·object lifecycle과 maintenance
  시도·진행·거절을 분리한다. 원 기준으로 지속 발행 재현과 짧은
  targeted 회귀를 실행한다.

## B03-B48 — Catalog settlement의 첫 차단 조건

- 같은 Catalog snapshot에서 최초 차단을 target pin, exact floor, work below
  floor, terminal work, custody, 미퇴역 view, reclaim queue, object
  lifecycle, unreferenced object로 구분한다. SQL 검사 순서와 short-circuit,
  budget, `Incomplete`, SDK wire와 수용 기준은 유지한다.
- opt-in native work 진단에서 daemon당 조건별 최초 표본 한 건을 기록한다.
  head·floor·대상 pin 상태를 함께 보존한다. Rust 포트의 typed 차단값으로
  실제 결정을 관측하고 별도 SQL 조회나 강제 GC를 붙이지 않는다.

## B03-B49 — 검색 Posting 페이지의 owner/source Map 배치

- 확정된 Posting page가 2개 이상일 때 owner contribution과 pinned source
  owner를 기존 `lookup_batch`로 각각 조회한다. 원 순서·immutable root,
  페이지 크기, funding, 전체 hit·DF·EOF 검증을 유지한다. singleton과
  overlay는 기존 scalar 경로를 사용한다.
- 목적은 shared Map path의 반복 decode 축소다. 개별 contribution, record,
  occurrence 검증과 늦은 손상 거절을 유지한다. 50 QPS 수용은 별도다.

## B03-B50 — XL 단일 packed inventory의 한 행 scratch seal

- empty cold base에서 하나의 `QITXOC01` packed block, 9–512개 term,
  key 최대 32바이트일 때 첫 key의 anchor만 기존 scratch에 기록한다.
  기존 packet의 physical ref·ordinal·confirm 및 독립 SQL replay를 통과한다.
- 저장된 block을 다시 읽어 원 registration ref, 전체 format, term count,
  전체 ordered key seal, anchor 첫 key·ref를 검증한다. 이때만 기존
  `EntryMap::OccurrenceBlock`을 선택한다. 작은 Small, 긴 key, 여러 block,
  기존 증분과 물리 codec·정책은 원 경로를 사용한다.
- fixed 512-term의 anchor 1행, long-key fallback 14행, digest·anchor
  손상 거절, 원 UDS search·증분·재오픈을 검증한다. 새 비용과 기존 gate는
  동일한 32,768-source·120초 실행으로 비교한다.

## B03-B51 — terminal custody 후보 조회의 writer 예약 제거

- `next_terminal_custody`는 Catalog snapshot에서 후보와 operation을 검증한다.
  advisory 조회는 WAL deferred transaction을 사용한다. `release_terminal_custody`
  writer는 현재 operation·상태·root를 다시 검증하고 실제 정리만 commit한다.
- 두 연결을 쓰는 fixture에서 한 연결의 writer 예약 중에도 후보를 읽고,
  writer 해제 뒤 기존 API로 custody를 정리한다. durable ticket clock 쓰기와
  settlement 기준은 그대로 적용한다. [SQLite WAL](https://sqlite.org/wal.html).

## B03-B52 — 같은 요청의 인증된 Text member 재사용

- 선택된 view·instance의 member를 한 번 인증한다. 직접 Content API는 기존
  pinned lookup을 사용한다. reader 경로는 같은 요청의 인증된 member를 넘겨
  view·source root·recipe를 확인하고 기존 root artifact 검증을 수행한다.
- immutable member object의 GET 1회, 원 root 결과, 다른 view 거절을 좁은
  fixture로 확인한다. 실제 UDS BM25·retained read를 함께 회귀 검증한다.

## B03-B53 — packed InventoryDiff의 scalar memo 조회 제거

- same-attempt 및 exact-ref 조건에서 인증된 PackedInventoryCursor는 key별
  position count를 읽는다. 기존 코드에서는 이 분기 전에 최대 64개 RAM
  항목을 훑고, spill일 때 term마다 scalar memo SQL을 읽었다.
- 기존 verified block memo 뒤, scalar memo 앞에서 exact cursor를 사용한다.
  최초 retained read, key별 count와 전체 token 합계, installed block의 독립
  seal, 원 scalar/disabled 경로를 유지한다.
- RAM이 가득 찬 fixture에서 서로 다른 두 packed count와 첫 retained read
  한 번, 다음 term의 추가 memo read 0건을 검증한다. r46은 같은 120초 Large·XL
  기준으로 실행한다.

## B03-B54 — custody root의 독립 페이지 한도

- runtime의 Work/View page와 Custody root page를 각각 1–32로 제한한다.
  JSON `custody_page_limit`를 생략하면 기존 `page_limit`와 같다. 둘 중 큰
  페이지의 최소 work funding을 검증하며 release는 기존 한 transaction을 쓴다.
- Mixed fixture는 Work/View 1, Custody 32를 명시한다. 20ms·2초 round,
  50 QPS·1Hz·960초와 pin600초·settle60초, 같은 global debt predicate를
  유지한다. 이전 Mixed의 운영 설정과 달라 matched-host 비교로 사용하지 않는다.
- driver에서 각 단계에 전달된 cap과 0/33·underfunded 거절을 검증한다.
  실제 Catalog/UDS 유지보수와 동일 제품 기준의 Mixed를 별도로 확인한다.

## B03-B55 — 동일 settlement snapshot의 oldest ready sequence

- TerminalCustody의 같은 scope index에서 가장 오래된 ready row 한 개를
  읽는다. 예산은 1행과 8바이트를 부담한다. 기존 global debt 조건과 SDK
  wire는 그대로다. 별도 SQL이나 전체 count는 추가하지 않는다.
- opt-in 진단은 terminal custody poll 1·16·32·48·60의 oldest sequence를
  기록한다. 다른 blocker는 기존 첫 표본 한 개를 사용한다. oldest가 계속
  같은지 전진하는지 관측해 queue 정체와 후속 발행을 구분한다.

## B03-B56 — 동일 티켓 검사에서 인증된 inode 재사용

- 열린 lease FD와 현재 경로의 inode 비교, OS lock probe를 수행한 뒤
  검증된 dev/inode를 해당 호출의 Catalog session row 대조에 재사용한다.
  같은 FD에 대한 두 번째 fstat와 두 blob allocation을 제거한다.
- retained 요청의 일곱 validation boundary, Catalog session/pin fence와
  durable clock을 유지한다. 기존 ticket clock·renew·release·정산 회귀를 실행한다.

## B03-B57 — custody 고정 행 조회와 트랜잭션 순회

- operation/attempt의 32바이트 object와 digest를 한 indexed 조회에서
  빌려 읽고, byte 예산 후 복사한다. 가변 intent는 별도 길이 선확인과
  registration 검증을 유지한다. 같은 release transaction에서 빈 종류를
  다시 탐색하지 않는다.
- boot protection과 per-row digest, delete/reclaim authority, page+1
  검증과 롤백, 원 operation replay를 유지한다.

## B03-B58 — 승인된 ASCII cold owner의 token 그룹

- empty cold base의 canonical TokenBatch를 입력 길이에 비례한 optional
  메모리로 모아 정렬한다. u32 offset으로 부모의 기존 ASCII 본문 한도
  8MiB까지 다룬다. 입력 N바이트당 최대 2N행·4N바이트 key arena와 실제
  Row 크기의 backing 전체를 먼저 예약한다. 최악 예약 요청은 224MiB이며
  기존 memory cap은 바꾸지 않는다. 예약 거절·Unicode는 원 token SQL을
  사용한다. 2,048행을 넘으면 추가 행 배열 없이 heapsort하고 1,024회
  비교마다 취소를 확인한다.
- 동일 key·position의 순서와 그룹을 검증하고 기존 occurrence pack sink로
  넘긴다. 실제 설치 payload·inventory root의 SQL/direct parity와
  독립 persisted seal을 확인한다.
- 확대 후보의 4,097·8,192·16,383B와 100KiB 입력은 실제 token SQL과
  key·position 및 packed occurrence wire를 대조한다. 100KiB fixture의
  key arena는 67,200B로 u16 범위를 넘는다. optional 예약 거절 뒤 같은
  budget의 SQL fallback과 메모리 반환, 큰 정렬 도중 취소도 검증한다.
  확대 후 실제 SDK 수용은 실행 결과가 나온 뒤 별도로 기록한다.
- ASCII 확장 후보와 관련 호출부·테스트는 Native SDK 담당자의 범위다.
  정적 감사에서 optional
  arena와 뒤따르는 필수 TokenBatch·positions가 같은 stage memory를 쓰는
  B35 위험이 남았다. arena 승인만으로 후속 필수 할당까지 보장할 수 없다.
  기존 SQLite가 수용하는 낮은 메모리 입력에서 optional arena를 포기하고
  원 경로를 끝낼 수 있는지 검증해야 한다. 입력 크기에 맞춘 예약은
  작은 owner의 비용을 줄이지만 경계 메모리의 동등성을 자체로 증명하지 않는다.

## B03-B59 — cold Posting 전용 최대 128행 scratch 배치

- 같은 4개 bind/row, per-row charge, 한 SQL 실행, VFS file cap을 쓴다.
  현재 SQLite 변수 한도 512 이상에서만 128행을 선택한다. 다른 scratch
  surface와 warm Posting은 64행이다. query당 비용과 row당 비용을 분리한다.
- SQL 실행 횟수와 실제 row count, 낮은 변수 한도에서 effect 전 거절,
  인벤토리와 검색 결과 parity를 검증한다.

## B03-B60 — packed Diff count의 순차 ordinal

- 같은 cursor의 다음 ordinal key가 정확히 일치하면 인증된 vector count를
  바로 읽는다. 건너뛴 key·역순·반복은 원 검색 경로로 조회한다.
- 최초 retained read, cancellation, 누락 거절과 전체 token 합계를 유지한다.
  B57–60은 같은 소스로 f114 및 원 120초 Large·XL에서 검증한다.

## B03-B61 — custody의 진행과 거절을 분리

- 기존 phase 결과로만 admission 거절, floor 시도·거절, custody probe·release
  거절, 커밋된 page와 실제 rows, 최근 head probe의 oldest ready/operation,
  그 행의 누적 삭제와 has_more를 관측한다.
- opt-in 45초 간격, 최대 16표본이다. 추가 Catalog query, 재시도, deadline
  갱신이 없다. 실패 page는 커밋 rows에 포함하지 않는다. r48 Mixed에서
  oldest 정체를 확인할 때 스케줄이나 page 폭의 효과를 이 수치로 구분한다.

## B03-B62 — 한 custody page의 reclaim 인증 재사용

- 같은 Catalog writer transaction에서 처음 실제 custody를 삭제할 때 GC
  namespace binding을 검증한다. 이후 같은 page에서 binding을 재사용하고
  기존 reclaim enqueue와 budget을 적용한다.
- 세 custody table의 DELETE는 기존 prepared statement cache를 사용한다.
  protection, row digest, queue 결정, page+1, commit과 rollback 경계를
  유지한다. r48 진행 표본에서 실제 op2의 rows가 커밋되며 admission·release
  거절은 초기 표본에서 없었다. 다음 Mixed에서 row 처리율과 debt0를 확인한다.

## B03-B63 — retained SearchText의 실패 단계 시간

- 기존 request stage 경계에서 Control 대기, session custody, preparation,
  Interactive 대기, 실행, 결과 변환, 최종 checkpoint의 wall 시간을 합산한다.
  기존 첫 8개 실패 로그와 별도로 opt-in·request ID가 1보다 큰 실패를 최대
  8개 기록한다. ID 조건은 진단 선택에만 쓰고 요청 승인에 관여하지 않는다.
- 성공 요청에는 출력하지 않는다. 2초 요청과 1Hz·50QPS, old-view
  witness의 pin·정답 기준을 유지한다. 서버가 취소를 관측하지 않은 client
  timeout은 별도 실패 타이밍 이벤트가 없을 수 있다.

## Mixed와 XL 후속 비용 귀속

- B64·B67 `050e6280…`은 terminal custody writer transaction에서
  인증한 operation의 숫자 PK를 재사용한다. 자식 읽기와 삭제는 그 PK를
  사용하고 row digest에는 기존 canonical operation key를 사용한다. GC
  lifecycle과 queue checksum을 계속 검증하며 queue 조회·삽입 SQL만 캐시한다.
  다른 참조가 있다는 이유로 enqueue를 생략하면 해당 참조의 마지막 해제까지
  durable 후보가 남는다는 보장이 필요하다. 이 계약을 유지한다.
- B65는 ASCII token의 두 case surface 길이를 직접 계산하고 이미 승인한
  버퍼에 원문과 ASCII lowercase를 기록한다. Unicode 경로, 크기·위치·순서,
  결과 형식과 work budget을 유지한다.
- B66은 새 SDK counter의 첫 retained SearchText 후보 16개만 타이밍한다.
  성공과 오류는 각각 최대 8건, 기존 후속 요청 오류는 별도 최대 8건이다.
  이는 진단 표본이며 전체 지연 분포나 요청 성공률을 나타내지 않는다.
- B64–B67은 f121의 Catalog·Lexical·Runtime·실제 UDS 집중 회귀 207개를
  통과했다. 검증한 5개 소유 파일을 공유 작업 트리에 적용했다.
- B68–B71 `1482d857…`는 독립 경로 네 개를 묶는다. Cold alias는
  승인된 sorted token row를 빌려 key·position별 중간 할당을 줄인다.
  기존 QITXOC01 encoder와 설치된 block의 independent seal을 재사용하고
  shape·선택 메모리 거절 시 원 경로를 사용한다.
- B69는 opt-in Posting trace에서 capacity admission과 INSERT 실행을
  처음 8개 및 이후 2의 거듭제곱 ordinal로 분리한다. B70은 원 Work·Views
  page가 빈 경우에만 여유 예산으로 custody 한 page를 더 처리한다. 정규
  순서와 page cap, deadline은 유지한다. B71은 정산의 32바이트 key를
  직접 읽고 lifecycle·reference SQL을 재사용한다.
- f122의 변경 없는 Catalog·Scratch와 f124 Lexical·Runtime·실제 UDS가
  합계 258개를 통과했다. 7개 소유 파일을 공유 작업 트리에 반영했다.
- 32행 custody 페이지는 20ms 간격의 Work·Views·Custody 3상 중 한 번만
  도달한다. 각 round 비용이 0이어도 약 533행/초다. 정산 시작 시 잔여량을
  확인한 뒤, writer hold를 늘리는 대신 고정폭 페이지 조회와 빈 상의 예산
  활용을 검토한다. 60초 전역 debt-zero, foreground latency, rollback을
  함께 검증한다.
- Mixed 전체 Record는 50 QPS의 요청에 계측 비용을 더한다. 진단이 필요하면
  IPC decode 뒤 request span 생성 전에 SourceAdvance와 제한된 retained 검색을
  선택한다. 기존 stage를 재사용하고 선택률·손실·terminal outcome을 기록한다.
  성공·실패의 Control/Interactive 대기, ticket 검증, lexical 실행을 분리한다.
  이 진단은 Off 제품 수용과 별개다.
- retained 검색의 일곱 검증 사이에는 profile read, adapter resolution,
  Interactive 대기, 검색, 변환이 있다. 서로 인접한 검증은 이미 병합했다.
  deferred snapshot의 동일 millisecond 비교는 다른 writer의 clock floor와
  pin 변경을 놓칠 수 있다. 별도 capacity 비교는 공식 5단위 실행 결과와
  분리하며 보호 class와 호스트 자원을 함께 계상한다.
- B72 `802e12b5…`는 opt-in retained 타이머에 `query_execution_detail_us`를
  추가한다. 실제 reader, 실행 전후 ticket 검증, 나머지를 분리하고 기존
  `query_execution` 집계, 7회 검증과 오류·예약 순서를 유지한다. 관측하지
  못한 호출은 null이다. f125 28개를 통과해 공유 작업 트리에 반영했다.
- B73 `d310d040…`은 terminal custody 고정폭 kind를 최대 32행의 정렬된
  SELECT로 읽어 스택에 보관한다. 커서를 닫은 뒤 각 row를 기존 순서로
  삭제하고 GC에 등록한다. 가변 intent 사전검사와 page+1을 유지하며,
  한 chunk 안에서는 뒤 행의 shape 오류가 앞 행의 삭제보다 먼저 검출될
  수 있다. f126 24개에서 rollback과 논리 read·bytes 계상을 확인해
  공유 작업 트리에 반영했다.
- Mixed의 다음 수정은 두 범위로 나눈다. B76은 response를 기존 메모리
  예약 안에서 변환하고, reader 결과를 공개하기 전 마지막 Kernel pin
  검증에서 Interactive 슬롯을 반환한다. r52 최초 실패의 직접 구간인
  registry custody와 준비는 별도로 분해한다. 전체 7개 확인, 원 요청
  deadline, registry 전후 검증을 유지한다. f130에서 Kernel 24개·runtime
  3개·UDS 2개를 확인해 `b05b0c4e…` exact patch를 반영했다.
  성공 경로의 Registry 최종 확인은
  [Native custody ADR](../../adr/NOV-07-001-native-index-execution-and-custody.md#decision)의
  독립 경계다. 축소를 검토하려면 동일 요청·ticket·lease·Catalog 확인의
  증명과 lease 만료·clock 회귀·오류 경로의 동등성이 필요하다.
- B78 `9e7f593d…`는 기존 retained 타이머에 registry lookup, entry/pre,
  exit/post를 추가한다. 기존 stage·custody 합계와 표본 예산은 유지한다.
  Entry에는 registry lock과 pin 확인이 함께 들어간다. 검색 자체가 실패한
  경우 원 오류 stage를 유지하며 exit/post 세부값은 null이다. B79와
  묶은 최종 `932f8e9f…`에서 Kernel 1개·Runtime 14개·UDS 3개를
  확인하고 반영했다.
- B79 `7272bac5…`는 Work/View가 한 행도 retire하지 못하고 같은 scope의
  직전 custody page에 남은 행이 확인된 경우, 원 라운드 예산 안에서
  기존 custody page를 최대 한 개 더 처리한다. 기본 단계와 커서는 그대로
  진행한다. 빈 custody scan은 힌트를 제거한다. f131 Runtime 14개와
  실제 pin 압력·해제 회귀를 포함한 UDS 3개가 통과해 exact patch를
  반영했다. 추가 page가 전체 writer 점유에 미치는 영향과 600초 pin
  이후 60초 정산은 Mixed로 판정한다.
- B81은 Catalog ticket 검증의 기존 여섯 구간을 `SqliteWork` lane
  200–205에 기록한다. session guard, connection 획득,
  BEGIN, session/pin row 확인, durable clock, COMMIT을 원 순서로 감싼다.
  기존 lock span 생성기를 공유한다. Off에서는 새 clock이나 event를
  만들지 않는다. r53 Off의 source는 고정한 채 별도 후보에서 작업한다.
  Catalog 집중 회귀 뒤 지원되는 진단 경로에서 mutex 대기와 실제 transaction 비용을
  구분한다. r53의 성공 표본은 reader 6.3–9.0ms인데 실행 전 검증은
  9.4–365.7ms로 나타났다. 이 수치만으로 대기와 COMMIT을 선택하지 않는다.
  초기 `a8bd8905…` Catalog 16개·UDS 2개 통과 뒤 Clippy가 잡은 5건은
  최종 `3c034df4…`에서 명시적 lock 해제, 동등 match와 문서로 보완했다.
  새 회귀는 Off의 진단 clock 비호출, 성공한 여섯 구간의 순서와 clock
  회귀에서 lane204 실패·COMMIT 미도달을 실제 Catalog로 검사한다.
  f135에서 Catalog 28개·실제 UDS 2개·Catalog strict Clippy를 통과해
  exact patch를 공유 소스에 반영했다.
- r54의 Mixed Record 요청은 capture preflight에서 거절됐다. 원인은
  capture의 Off-only 계약과 harness의 bounded Record profile 미정의다.
  producer body는 0이며 실행·진단 결과는 `NOT_RUN`이다. 다음 후보는
  Catalog 내부의 고정 크기 thread-local 관측을 기존 retained prefix
  타이머가 선택한 요청에만 묶는다. scope는 같은 동기 호출 스레드에서
  종료하고, 중첩 scope를 복원하며 제품 예산·검증·수용 형식은 유지한다.
- B82 `b7ffae26…`의 소유 범위는 Catalog `ticket_diagnostics.rs`,
  export, 기존 lifetime 계측 연결과 Runtime retained 진단 두 파일이다.
  고정 크기 scope는 Catalog 주소와 요청 내 owner를 확인하며, 종료 순서가
  어긋나면 관측을 비운다. 선택된 요청의 여섯 구간 합계·호출 수·실패 수를
  기존 표본에 붙인다. 관측이 없는 구간은 null 또는 호출 수 0으로
  구분한다. 새 진단의 기능 회귀 뒤 원 Mixed Off profile을 RCA 목적으로
  실행한다. 이 소스의 성능 qualification은 별도다. 최초 기능 회귀는
  Catalog 37개·runtime 4개·UDS 2개를 통과했다. annotation 보정 뒤
  Catalog trace 1개·runtime 4개·UDS 2개와 Catalog strict Clippy를
  확인해 반영했다. busy poll·실제 retry sleep 합계는 요청 scope 전체의
  같은 Catalog 경합 값이며 lane201에만 한정한 값으로 해석하지 않는다.
- XL은 SourceApply의 물리 get/cache와 BuildPrepare의 inventory scratch,
  설치/COMMIT을 분리한다. 기존 전체 trace의 중첩 구간은 합산하지 않는다.
  120초 checkpoint 위치만으로 객체 형식이나 수용 기준을 변경하지 않는다.
- r49의 설치 입력 분포 95·96·160·161개는 1MiB staging이 256행 창을
  분할함을 확인한다. 한 창에 8KiB 이하 packed block 최대 256개를 담는
  후보는 2MiB payload와 installer 임시 메모리를 모두 예산으로 보장해야
  한다. 승인 부족 시 기존 분할을 유지한다. 두 COMMIT은 파일 sync 전후
  custody 경계다. 다른 batch와 통합할 경우 기존 Storage의 atomic advance와
  정확한 ordinal 확인을 연결하고, 미확정 scratch를 읽지 못하게 한다.
  현재 최대 256-ref `PacketEvidence`와 단일 `InventoryConfirmation`은
  현재 Planned 행 전체를 독점한다. 두 packet을 동시에 stage하는 경로는
  두 payload와 installer wire의 grant, 두 그룹의 ordinal 증명, 첫
  영속 효과 전 단일 packet으로 돌아가는 경계를 먼저 정의해야 한다.
  Core port만 열거나 1MiB cap만 늘리는 변경은 보류한다.
- Posting scratch의 75,723 batch는 동일 private SQLite에서 매번 page count,
  freelist count, page size, max page count를 확인한다. `xr51` 표본에서
  `raw_execute`가 71%, capacity가 5%였다. `(term, owner)` 순서에 맞는 cold
  적재, 또는 append 뒤 unique index를 만드는 경로의 SQL VM과 디스크 peak를
  먼저 비교한다. 원 계상·SQLite hard cap·`SQLITE_FULL` 중단과 cap 변경
  거절, 중복·취소·정렬 실패의 atomic refusal을 유지한다.
- B75 `e36a150e…`는 Posting scratch 입력의 32바이트 이하 키를 row에
  인라인 보관해 개별 `Vec`와 lease를 없앤다. 긴 키는 기존 경로이며 SQL
  파라미터와 정렬을 유지한다. row backing 폭은 늘 수 있어 32/33바이트
  경계의 lease와 취소를 포함한 Lexical 192개와 실제 UDS 2개가 f127에서
  통과했다. 기존 공유 inventory cursor를 보존한 채 반영했다. XL은
  `NOT_RUN`이며 case-pair layout 변경과 독립이다.
- B50은 단일 cold packed block의 scratch seal을 anchor 한 행으로 줄였다.
  XL에서 그 분기의 실제 owner 비율과 posting scratch·물리 설치의 잔여
  비용을 분리한다. 과거 Record의 posting scratch 59,861 flush는 9.102초
  inclusive로 기록됐다. 다음 변경은 현재 원인과 memory/SQLite bind 상한을
  확인한 뒤 posting 전용 계약으로 검토한다.

## B03-B30 — Pin 검증의 원 요청 예산 전파

- `ReadTicket::validate`는 예산 없이 Catalog writer mutex에 들어간다. 각 성공한
  검증은 같은 session·fence·pin을 검증하며 clock이 전진하면 durable floor를
  갱신한다. retained 검색에서 이 호출이 반복되고 mutex가 이미 사용 중이면 원
  요청이 취소돼도 기다린다.
- 검색과 retained session 경계에서 원 `WorkBudget`을 전달한다. Catalog writer
  mutex 획득과 COMMIT 직전의 checkpoint를 원 deadline·취소에 연결한다. 기존
  durable clock 검증과 pin 만료·fence 계약은 유지한다. 취소나 writer mutex
  deadline으로 검증이 중단되면 살아 있는 세션을 `Unknown`으로 전환하지 않는다.
- writer mutex를 잡은 채 요청이 제한 시간 안에 종료되는 Catalog 회귀와, 검색
  도중 취소 후 같은 pin이 유효한지 확인하는 registry 회귀를 묶는다. SQLite 내부
  busy 및 진행 중인 durable I/O의 강제 중단은 별도다. 기존 clock floor의 영속성을
  유지하는 반복 쓰기 감축은 Mixed 단계별 계측 뒤 별도 계약으로 결정한다.

## B03-B28 — Cold occurrence 위치 배열의 중복 비교 축소

- QITXOC01 `BlockDraft`와 `CheckedBlock`은 각각 현재 vector를 이전 vector 전체와
  비교한다. 최대 512개라 메모리는 bounded지만 비교 수는 제곱으로 증가한다.
- 기존 배열과 first-use ordinal을 유지한다. 첫 FNV index는 유효한 위치 배열로
  같은 슬롯을 몰아 최대 130,816개 후보 탐색을 강제할 수 있다. 후속은 최대 512개
  u16 ordinal의 정렬 index에서 실제 배열을 이진 비교한다. index backing 1KiB와
  작은 블록의 무할당 경로, grant·취소, 8KiB/512 상한, 독립 decode와 동일 wire를
  검증한다. 삽입 시 이동하는 것은 2바이트 ordinal이다.
- 해당 코드의 내부 절감량은 실측하지 않았다. XL Record의 `BuildPrepare`에서
  일부 inventory만 실행됐으므로 이 변경을 XL 전체 ACK 성공으로 판정하지 않는다.

## XL 단일 공개의 남은 계약

- 현행 `SourceAdvance`는 Pending 승인 직후 한 요청에서 구축한다. deadline에
  도달하면 원 키가 terminal `Refused(ResourceLimited)`가 된다. `resume()`은
  Pending에서만 새 fence로 처음부터 재실행하며, attempt-local scratch를 복원하지
  않는다. 기존 owned-work는 Materialize 전용이므로 SourceAdvance에 재사용할
  준비·재개 계약이 없다.
- 단일 ACK와 원자 가시성을 유지하면서 긴 구축을 재개하려면 immutable operation과
  expected head, source event, recipe에 연결한 내구 ready/lease 및 진행 증거가
  필요하다. fence 교체 시 준비 객체 custody를 재검증하고, 단계별 호출 사이에도
  같은 operation의 4축 work cap을 누적한다. Catalog의 기존 원자 commit에서
  결과 roots와 원 event를 한 번만 공개한다.
- `Accepted`는 진행 상태다. E07 XL 수용은 원 operation의 최종 Published, current/old
  검색, cold reopen과 복구를 확인한다. Large 4,096-source의 원 120초는 유지한다.

## B03-B27 — Source 경로 Map의 물리 노드 감소

- XL Record SourceMapApply는 20.504초, 실제 파일 write·fsync 각각 8,792회,
  설치 36배치·중첩 Catalog COMMIT 46회다. `path` 6,605 + `owners` 2,185 + 고정
  객체 2개가 실제 write 수와 일치한다. Large는 819 + 274 + 2 = 1,095회다.
- 기존 v19 QIMAP005 선택을 `SourceEditCursor`의 path ordinal 1에 사용한다.
  경로 값은 child가 없는 인라인 `OwnerKey`라 85-edge cap을 소비하지 않는다.
  owners/frontiers의 writer 선택, CAS·정렬·원자 설치 계약을 유지한다.
- 정적 radix 모델의 path 노드는 Large 819→451, XL 6,605→3,661이다. 실제 감소는
  r36 Large Record의 SourceMapApply에서 실제 파일 write·fsync 각 1,095→727회,
  설치 6→4배치와 중첩 COMMIT 9→6회로 확인했다. f84는 기존 경로 swap·이전
  view 조회와 packed node 실체를 확인했다. XL 전체 ACK와 남은 lexical inventory
  비용은 별도 범위다.

## B03-B26 — Mixed 실행 창과 관측 계약 일치

- 기존 구현은 query scheduler와 1Hz publisher가 각자의 시작 시각을 쓰고, 60초
  warmup을 drain한 뒤 900초 measure를 새로 시작한다. 창별 수량과 p95/p99가
  통과해도 같은 960초 구간의 동시 부하와 발행 cadence는 보장되지 않는다.
- 기존 offered-load scheduler가 worker 준비 뒤 공유 시작 시각을 발행한다. 검색과
  발행은 하나의 960초 구간에서 연속 수행하고 scheduled index로 60/900초 통계를
  분리한다. 창 경계에 drain이나 재시작을 두지 않는다. 기존 50QPS, 1Hz, queue,
  ReadSession, drop 및 지연 수용 조건을 유지한다.
- 기존 capture front door와 mixed artifact 검증을 사용한다. 고정 origin·창 분할·
  취소 회귀와 실제 960초 수용을 별도로 판정한다.

## B03-B25 — QIMAP005 저장소의 구버전 부팅 차단

- v18 Native runtime은 QIMAP005를 해석하지 않지만 v18 marker와 Catalog stamp를
  통과할 수 있다. boot의 물리 closure 확인과 Text Map 도메인 decode는 별도라서
  오류가 조회 시점까지 지연된다.
- Native state를 v19로 올리고 runtime marker, Catalog stamp와 일반 daemon의
  Native root 거절을 함께 맞춘다. 이전 v18 root는 기존 pair로 보존한다. v19는
  새 root와 producer 재구축을 사용하고 boot 중 자동 변환·표식 교체는 하지 않는다.
  v19 내부의 QIMAP004·005 object decoder는 공용이다.
- v18 marker와 직접 v18 Catalog stamp를 각각 거절하고 이전 root를 보존하는지
  f80의 32개 회귀에서 확인했다. 최적화 r34 `645f4edf…`의 구/신 상호 부팅 거절과
  원 Large Off·Record의 ACK·검색·재오픈도 `VERIFIED`다. 기존 설치 상한, 입력,
  work cap을 유지했다. 정식 overhead·XL·mixed·출시 검증은 별도다.

## B03-B24 — 읽기 전용 Catalog의 Record 이벤트 감소

- 상위 Catalog에서 첫 write/sync 시에만 `SqliteWork/22`를 시작한다. 중첩 scope의
  reentry 경계와 callback 전체 counter, sync·거절·budget은 유지한다.
- 읽기 전용 scope와 기존 write/sync/거절/checkpoint/nested 회귀를 B23과 묶는다.
  원 Large에서 emitted events, drop/open, ACK·검색·재오픈과 host 대기를 함께
  기록한다. [B24 RCA](RCA.md#b24--읽기-전용-catalog-scope의-record-이벤트-감소).

## B03-B23 — 동일 Inventory Diff의 packed DATA 재사용

- 이미 열려 있는 next `OccurrenceBlock` cursor의 동일 ref를 count에 재사용한다.
  기존 count memo 우선순위와 zero 설정을 유지한다. 최초 adapter read와 정확한
  DATA 일치 확인을 수행하고, full merge·token 합계 통과 뒤 같은 attempt의
  2슬롯 reader로 소유권을 이동한다. 기존 B21 고정 소스는 따로 검증 중이다.
- 동일 ref의 다음 owner, 마지막 페이지·EOF, 합계 실패 후 미채택 및 0 설정을
  집중 검증한다. Large에서 Diff의 최초 occurrence read와 CPU를 다시 비교한다.
  [B23 RCA](RCA.md#b23--inventory-diff의-packed-data-재사용).

## B03-B22 — Terms Map의 물리 노드 감소

- 대상은 Native Text의 최상위 terms root다. reader의 point lookup과 build의
  원본 root CAS는 같은 Map decoder를 쓴다. Storage의 bounded page를 선택할 수
  있게 하고, Lexical은 terms ordinal에서만 선택한다. owners와 나머지 Map은 기존
  쓰기 계약을 따른다.
- `MapEditCursor`의 기본값 없는 선택 hint를 사용한다. 현재 AttemptContext와
  owner/scoped read port는 같은 cursor를 Storage로 넘긴다. Storage는 scalar
  lookahead 전에 hint를 읽고 범위를 검증하며, 서로 다른 형식을 요구하는 두 root는
  같은 edit fingerprint라도 빌드 결과를 공유하지 않는다.
- 부모에 terminal-only child subtree를 한 단계 인라인한다. 행, encoded payload,
  물리 child 수를 각각 제한한다. 한도를 넘기는 child는 radix의 기존 object 경계에
  남긴다. 인라인 subtree의 값 child를 실제 object edge로 등록한다. 기존 QIMAP004
  root와 mixed subtree는 같은 decoder에서 읽는다. corpus digest가 일치한 정적
  모델의 고유 terms 노드는 12,394→5,272다.
- Warm edit는 원본 root에 대해 CAS를 검증한다. 변경 경로의 page만 다시 쓰고,
  변경과 겹치지 않는 object는 identity를 재사용한다. 키 정렬·완전한 cursor EOF를
  확인한 뒤 reachable object를 설치한다. 배포에서는 B25의 v19 root 형식을
  사용한다. QIMAP004 전용 바이너리는 보존한 이전 root와 함께 사용한다.
- 고정 oracle은 동일 ordered key/value 결과, prefix/빈 key, 동일 child 재사용,
  용량 경계와 malformed wire, 국소 edit의 read/write 상한, old pin 조회, cancel 및
  설치 실패 뒤 재시도, GC·재오픈이다. 동일 Large seed42에서 실제 물리 쓰기와
  COMMIT을 따로 비교하고 원 120초 ACK·검색·재오픈을 판정한다.
- 격리 소스 `4d3924dace0791762f25306773708890f1e1fe65`에 writer, decoder,
  Native 선택, original-base lookup·scan·diff, resident spill 한도와 회귀를 넣었다.
  `rustfmt`와 diff 검사를 통과했다. 기존 wire inventory 불일치 7건은 같은 기준
  소스에도 재현됐다. r31 Off·Record 종료 후 f76 → r32 → Large Off `lo28`·Record
  `lr27`을 실행한다. 공유 `map.rs`와 `original_lookup.rs`의 기존 변경을 보존하는
  합류 검증은 별도다. P04 batch 조회 위임을 유지한 합류 패치의 적용 검사는 통과했다.
  f76은 codec의 unused binding으로 컴파일에서 종료됐다. 그 binding만 수정한
  `ca4785c9af62da5da7845e7b662ffa9fc53dea25`의 f77은 scan 회귀에서 `MapPage`
  역참조 값을 이동하려 해 E0507로 종료됐다. 회귀의 값 소유와 spill fixture의
  content identity를 보완한 `58d69e246c813dd230d2cb7d1d490bec237fa857`로
  f78의 409개 테스트가 통과했다. 같은 소스의 r32 Off `lo28`와 Record `lr27`은
  원 120초에서 최초 ACK46.419초·48.192초, 검색·증분·삭제·이전 pin·재오픈을
  완료했다. terms 노드 5,272개, owners 274개, 설치 22배치·COMMIT 28회를
  raw trace와 canonical report에서 대조했다.
- 공유 작업 트리에 P04 batch 위임을 보존한 패치를 반영했다. packed root에서
  적중·부재·중복과 출력 순서를 fixture oracle과 비교하고, scalar보다 적은
  물리 읽기를 확인하는 회귀를 추가했다. `f54536fb…`의 f79 Storage 69개가
  통과했고 116개가 제외됐다. pending owner-scope batch도 통과했다. 해당 공유
  코드의 v19 제품 실행은 B25의 r34에서 확인했다.

## B03-B21 — 최상위 Map 물리 노드와 Record 검증 비용

- `NativeTextPostings/39 → MapApply/5`의 cross-root 설치 순서와 배치 계약을
  유지한다. Record `MapApply/6`(terms)·`/7`(owners)에 최초 도달한 고유 staged
  노드를 기록한다. 실제 write·sync와 Catalog COMMIT은 기존 물리 span에서 집계한다.
- 동일 노드를 공유하는 두 root, 기존 조회, child registration과 물리 파일 수를
  Storage 회귀에서 함께 검증한다. canonical verifier가 raw child를 한 번만 파싱하며
  변조된 호출자 입력을 계속 거절하는 캡처·검증 테스트 520개는 `VERIFIED`다.
- 현재 50배치·63회 COMMIT은 4배치 창의 최소치다. fixture의 owner radix 분기로
  계산하면 물리 쓰기 12,668개 중 최소 12,394개가 terms에 속한다. terms에
  한정한 page format을 다음 구조 변경 대상으로 삼는다. 기존 Map 호환, pin·GC,
  순서, 증분 국소성, 실패 시 재시작과 120초 ACK·검색·재오픈이 채택 조건이다.
- 최상위 Map과 독립인 cold inventory는 기존 최대 창 256개를 기본값으로 쓴다.
  warm/명시 설정, occurrence·source 바이트 한도와 메모리 승인을 유지한다.
  기존 128개 창 대비 설치·COMMIT 감소, live memory, 동일 검색·재오픈을 확인한다.
- 고정 소스 `60d74310245e7664ef304c79c068156fc3bf7468`은 B20 보완본에서
  필요한 8개 파일을 포함한다. f72는 새 Storage 회귀의 임시 lock guard 수명
  오류 E0716으로 테스트 실행 전 실패했다. 수정하고 B23/B24를 합친
  `50644fa76039b773034e566c39655fea421eef0c`의 Python 548개가 통과했다.
  packed Diff test의 helper shadowing E0618을 확인해 f73은 실행 전에 중단했다.
  해당 test만 보완한 `89a062d25422270b8b34f604ddd30790193e5e76`의 f74는
  403 통과·2 실패·1,258 제외다. 이전 VFS 두 테스트가 empty lane22를 기대했다.
  두 test만 수정한 `01c4bb4d00ed0f3a843e31ed4305f90c13ec9727`의 f75는
  405개 통과·1,258 제외다. r31 최적화 pair도 통과했다. Large Off `lo27`·
  Record `lr26`의 최초 ACK는 각각 79.470초와 77.573초다. 두 제품 경로와 Record
  canonical profile은 `VERIFIED`다. 정식 matched-host 성능 수용은 `NOT_RUN`이다.
  [B21 RCA](RCA.md#b21--최상위-map-물리-노드-귀속과-진단-검증).

## B03-B20 — Dense Posting 물리 노드 감소

- B19의 lane37 Posting 980페이지 중 첫 4페이지가 61.372/63.164초(97.2%)였다.
  이 구간에 물리 설치 90/96회와 Catalog COMMIT 113/122회가 집중됐다. 최상위
  Map은 scratch 단계를 완료했고 파일 설치 중 종료됐다. [B19 실행 RCA](RCA.md#b19-실행-결과).
- 9–8,192 owner의 Posting은 `QITXDD01` 디렉터리와 최대 256행의 `QITXDP01`
  leaf로 표현한다. `(owner32, witness32)`는 정렬·고유하고 leaf는 occurrence
  child를 가지지 않는다. 디렉터리는 실제 leaf를 순서대로 소유하고 첫/마지막 key와
  행 수를 인증한다. term Map은 DF와 실제 디렉터리 child를 인라인한다. bounded
  immutable posting block의 선례는 [Lucene codec](https://lucene.apache.org/core/10_1_0/core/org/apache/lucene/codecs/lucene101/Lucene101PostingsFormat.html)과 [Tantivy postings](https://github.com/quickwit-oss/tantivy/blob/main/ARCHITECTURE.md)다.
- Warm 수정은 이전 page의 마지막 key로 범위를 유지하고 같은 content identity의
  leaf를 재사용한다. 순차 append는 마지막 범위에 합친다. 페이지 수 한도에서는
  같은 표현으로 재압축하고, 최대 행 한도 초과 시 기존 Tree로 전환한다. Tree에서
  6,144행 이하로 줄어들면 packed 표현으로 복귀해 경계 왕복을 피한다. Small·singleton과
  기존 Tree/MapApply 경로는 그대로 사용한다.
- 원본 CAS와 aggregate stream digest·EOF를 확인한 뒤 leaf를 설치하고 디렉터리를
  확인한다. 검색에서는 pin된 inventory의 실제 occurrence와 witness를 대조한다.
  read·재시도와 이전 pin·GC·재오픈 회귀를 집중 검증한다.
- 정렬/누락/위조·경계 변환·취소·GC 집중 검증 뒤 같은 seed42·4,096 sources의
  120초 Off·Record를 한 배치로 실행한다. 7개 dense 페이지와 최상위 Map의 실제
  파일·COMMIT, full ACK·검색·재오픈, warm 요청의 추가 read를 각각 판정한다.
- 고정 소스 `def73607e11673f0ee1c5c7cc006d9f2f406a763`으로 f70 → r29 →
  Large Off `lo25`·Record `lr24`를 실행했다. f70 집중 253개와 r29 build,
  Large Record의 ACK·검색·재오픈은 `VERIFIED`다. 같은 pair의 Off는 `FAILED`다.
- 후속 고정본은 singleton fence와 중복 child 거절을 보완한다. warm edit와 겹치지
  않는 leaf는 인증된 directory의 descriptor로 재사용해 payload 읽기·인코딩·해시를
  생략한다. 페이지 수 상한을 넘기는 경우에만 전체 정렬 stream으로 재압축한다.
  `132145054e81df69a8811b1982a1a640635476fa`의 f71 → r30 → Large Off `lo26`·
  Record `lr25` 검증을 시작했다. f71 집중 회귀는 254개 통과·702개 제외로
  `VERIFIED`다. r30 build와 `lo26` 제품 경로도 `VERIFIED`다. 최초 ACK는
  87.815초였고 검색·증분·삭제·재오픈·drain을 완료했다. `lr25` Record는
  full ACK126.355초에 Catalog protection checkpoint에서 deadline으로 `FAILED`다.
  정식 matched-host 검증은 `NOT_RUN`이다.

## B03-B19 — 단일 packed occurrence를 초기 inventory로 직접 사용

- B18 Large의 첫 Posting 전 occurrence와 inventory Block이 각각 4,096개였다.
  각 owner의 완전한 key 집합을 단일 `QITXOC01`이 가진 경우에만 inventory Block을
  생략한다. 기존 producer, 정렬 CAS merge와 원본 stream stamp는 유지한다.
- 초기 빈 prior만 선택할 수 있다. 동일 ref, 완전한 authenticated payload, 정확한
  key count와 순서를 검사한다. 다중 child·scalar·불일치는 기존 Small/Block/Tree
  경로를 사용하고, 읽기·인증·packed 파싱 실패는 거절한다.
- 별도 `OccurrenceBlock` 표현과 `0x82` contribution을 사용한다. 실제 child는
  `[source record, occurrence]`다. 정상 조회 부재, 페이지 continuation의 retained
  검증, posting witness 및 scalar 거절을 유지한다. 변경된 owner의 증분은 기존
  일반 편집 경로를 쓰며 pinned 이전 view도 계속 읽을 수 있어야 한다.
- 집중 검증은 실제 512 key 단일 block, 512 초과 Tree, 증분 fallback, 재오픈,
  pin/GC 및 후보 key 불일치다. 같은 최적화 pair의 원 Large Off·Record와
  실패 시 Medium으로 검증한다. 목표는 Large ACK·검색·재오픈이다.
- 동결 소스 `046d2c02eb71a679f23529ac0b7ce3c907fbf2d6`의 f69 → r28 →
  Large Off `lo24`·Record `lr23`·Medium `mb19` 배치를 완료했다. f69 집중 252개와
  r28 build는 `VERIFIED`, Large 두 요청은 `FAILED`, Medium 기능은 `VERIFIED`다.
  원 Large에서 full ACK·검색·재오픈은 여전히 미완료다.

## B03-B18 — Native owner contribution의 별도 파일 제거

- 동일 Large pre-Posting에서 80개 물리 배치 중 contribution은 16×256개였다.
  `EntryMap::Block`·`Tree` contribution을 기존 `MapValue::Inline`의 9바이트 태그와
  token count로 인코딩한다. source record와 inventory root는 실제 child 두 개다.
- 기존 Small 인라인과 Object 디코더는 유지한다. 새 layout을 output policy에
  포함해 이전 프로파일과 구분한다. 이전 정책의 member는 새 프로파일에서 재인덱싱한다.
- `5324009a…`의 f66은 216개 통과·4개 실패다. Tree에 별도 설치를 기대한
  기존 oracle을 수정했다. 비인라인 Small로 설치 실패·취소 경계를 보존하고,
  Tree에서 65개 순서와 SQL 페이지 경계, rename의 새 record를 확인한다.
- `9808ca57…`의 f67은 새 rename 테스트 helper의 cursor edit move에서 E0507로
  실행 전 실패했다. 소유형 `OrdinalMapEdit`를 유지하도록 수정한 `ccd5ab92…`의
  f68 Lexical·Kernel 집중 회귀 220개가 통과했다.
  r27 최적화 pair의 원 Large 4,096/120초 Off·Record는 `FAILED`다. 같은 pair의
  Medium에서 full ACK·검색·증분·삭제·재오픈·drain은 `VERIFIED`다.
- Large 첫 Posting 전 실제 file write/fsync는 각각 12,288→8,192회, COMMIT은
  160→128회다. 남은 설치·COMMIT 합계는 39.533초이며 scratch edit는 6.486초다.
  원 Large ACK·검색·재오픈과 최상위 Map 실측이 남았다. [B18 RCA](RCA.md#b18--blocktree-owner-contribution-인라인).

## B03-B17 — 설치 창의 메모리·gate·관측 보완

- B16의 8배치 기본값을 기존 4배치로 복원한다. 배치당 256개와 합계 wire 64MiB를
  유지한다. 5배치 이상을 직접 요청하면 메모리·Catalog·파일 효과 전에 거절한다.
- gate 합집합은 정렬 순서로 한 번 시도한다. 경합이 있으면 잡은 prefix를 해제한 뒤
  충돌한 gate 하나만 기다린다. 전체를 확보한 뒤에만 기존 atomic advance를 실행한다.
- atomic advance 관측에서 이전 확인과 다음 예약의 논리 횟수·객체 수를 각각 센다.
  실제 호출 시간은 한 번만 합산하고, COMMIT 횟수는 SQLite lane3에서 확인한다.
- `366ebb5…`의 f65 집중 회귀 18개가 통과했다. 4배치·5배치 거절, gate 충돌과
  prefix 해제, 실패 뒤 물리 설치 prefix, 실제 Catalog advance, 집계 overflow를 검증했다.
- 남은 검증은 실제 Posting 경로의 동일 입력 COMMIT 계측, 낮은 메모리의 전체 요청,
  동시 writer/reclaim 경합, 원 Large 120초 ACK·검색·재오픈이다.
  [리뷰 RCA](RCA.md#b17--설치-창-리뷰-보완).

## B03-B16 — Posting 물리 배치의 Catalog 전이 창

- B15 `lr20` 세 번째 Posting page의 writer·COMMIT·off-CPU 시간은
  [B16 RCA](RCA.md#b16--posting-writer와-catalog-commit)에 귀속한다. 기존 writer
  인증과 sync/settlement 경계에서 추가로 제거할 중복을 확인하지 못했다.
- B16에서는 Storage의 연속 atomic advance 창을 최대4→8개 물리 배치로 늘렸다.
  현재 기본값은 위 B17의 4배치다. 배치당
  최대256개 객체, 창의 합계64MiB, 하나의 sorted gate union은 유지한다.
  기존 Catalog 트랜잭션이 직전 durable 배치 확인과 다음 배치 예약을 처리한다.
  런 전체의 파일 FD를 유지하지 않고 각 배치의 sync·정산 뒤 다음 배치로 진행한다.
- 8개 경계·9개 거절·후반 advance 실패의 prefix 및 Catalog 실제 전이를
  `adf4c8ed…` 집중 회귀 9개에서 검증했고 r26 최적화 pair를 생성했다.
- 동일 소스의 원 Large seed42·4,096 sources·120초 Off/Record는 `FAILED`다.
  Record가 첫 Posting page 물리 설치 전에 중단돼 변경된 COMMIT 횟수의
  동일 Large 페이지 실측은 `NOT_RUN`이다. Medium ACK·검색·재오픈은 `VERIFIED`다.
  원 Large ACK·검색·재오픈과 경합을 통제한 동일 페이지 비교가 남았다.

## B03-B15 — Inventory scratch와 물리 writer 반복 제거

- 실제 실패 경로는 inventory diff가 posting edit를 저장하는 scratch writer다.
  B14 Large Record의 inventory Block 적용은 Storage `map_apply`를 호출하지 않았다.
  비교 구간과 호스트 지표는 [B15 RCA](RCA.md#b15--inventory-scratch와-물리-writer-병목)에 기록한다.
- Scratch row의 논리 byte 과금은 실제 인코딩과 동일한 길이를 계산해 사용한다.
  SQL bind 시 한 번 인코딩한다. 페이지·freelist·hard cap 검증에 필요한 조회는 유지하고,
  예약 축소가 불가능한 live allocation은 같은 호출에서 얻은 예약량을 재사용한다.
- 배치 writer의 성공 게시가 같은 inode·소유자·mode·단일 링크를 확인한 직후의
  중복 `fstat`을 제거한다. no-replace, 열린 FD 인증, 파일·디렉터리 sync,
  settlement, Catalog 전이는 기존 경로를 사용한다. Runtime·Catalog의 다른 변경과 분리했다.
- B14 고정본에 이 코드와 이미 검증한 v5 collector를 합친 `794468fb…`에서
  Scratch·Storage 집중 회귀 37개와 r25 최적화 pair build를 통과했다.
- seed42·4,096 sources·server 120초·기존 work caps의 Off/Record는 `FAILED`다.
  Record는 inventory 완료 뒤 세 번째 Posting page 물리 설치까지 도달했다.
  동일 pair의 Medium ACK·검색·재오픈은 `VERIFIED`다.
- 원 Large 수용 조건은 full ACK·검색·재오픈 완료다. 남은 RCA는 동일 Posting page의
  writer와 Catalog 구간을 같은 호스트 부하 범위에서 대조하는 작업이다. Large 최상위 map
  계측과 경합을 통제한 비교는 `NOT_RUN`이다.

## B03-B14 — Cold inventory 물리 배치와 연속 Catalog advance

- B13의 실패 prefix에는 occurrence와 inventory block의 64개 물리 배치가
  각각64회 있었다. 초기 empty-base inventory 기본 창을128 owner로 묶는다.
  명시적 batch 설정과 증분 기본 창은1~64/64다. Scratch SQL 페이지64,
  occurrence payload1MiB, 입력 source 창8MiB, 그룹당 객체256 상한을 적용한다.
- Reachable Map은 최대4개 child-first 그룹, 합계64MiB의 창에서 sorted gate
  union을 한 번 잡는다. 그룹별 내구성 증명과 정산을 완료한 뒤 기존 Catalog
  advance로 이전 확인·다음 예약을 한 트랜잭션에 넣는다. 첫 reserve와 마지막
  confirm 사이에는 SQLite 트랜잭션을 열어 둔 채 파일 I/O를 하지 않는다.
- 최상위 terms/owners MapApply의 Record child lane1은 cursor·검증·재적재,
  lane2는 그 안의 edit scratch 쓰기, lane3은 replay·radix 구축, lane4는
  scratch commit, lane5는 물리 설치다. lane1에 lane2가 포함된다.
- 초기 `b4c8b57…`의 f58은 Storage 테스트 fixture 생성자 오류로 body0이었다.
  Packed occurrence alias의 남은 ordinal64 가드와 fixture를 교정했다.
  f59의 64MiB 거절 fixture는 불변 등록값 생성자로 수정했다. `c5d16a3…`에서
  Runtime·Storage·Lexical·Catalog의 f60 집중237개가 통과했다.
  r21 pair build 통과 뒤 lo19 Large Off는 ACK17.280초에
  `BuildPrepare`의 명시적 `ResourceLimited`로 실패했다. Cold block staging이
  일반64개 생성자를 호출했다. 동일 r21의 mb14 Medium Record가 occurrence
  128개 물리 설치 뒤 InventoryMapApply 거절을 재현했다. 교정본
  `60db2fc…`는 빈 inventory에만 물리 packet 생성자를 연결한다. 일반
  생성자는64개다. f61 집중237개가 통과했다.
  r24가 교정본 최적화 pair를 생성했다.
- 코드와 집중 회귀는 완료했다. 로컬 원120초 Large Off `lo20`과 Record `lr19`는
  `BuildPrepare` deadline으로 **FAILED**다. 같은 pair의 Medium `mb15`는 전체
  ACK·증분·삭제·검색·재오픈과 drain을 완료했다. Collector의 v5 CPU lane 인식
  오류를 별도로 수정했고 529개 pytest 및 원 trace 두 epoch의 오프라인 검증이
  통과했다. [실행 판정](README.md#현재-판정) · [단계별 수치](RCA.md#b14--물리-그룹-축소와-기존-advance-연결).
- 남은 수용 조건은 원120초 Large에서 full ACK·검색·재오픈 완료다. 경합을 통제한
  비교와 Large 최상위 map 단계별 계측도 남았다. Medium과 동일 입력 prefix는
  각각 기능과 반복 횟수의 근거다.

## B03-B13 — Native writer의 중복 parent 검사 제거

B12 Record에서 동일 세 Posting page의 writer CPU가5.924초 증가했다. B13은
writer 시작 단계의 allowance 검증을 보존하고, 실제 anchored open과 no-replace
publish 직전에 live parent를 확인한다. Final parent FD에서 직접 clone한 temp의
동일 parent 증명을 재사용한다. 독립적으로 연 temp parent는 별도 확인한다.
정산은 B12의 실제 sync identity를 확인하며, 같은 shard에 대해 이미 얻은 live
inode를 재사용한다. 생성 FD는 기존 held temp 이름과 대조하므로 Native ticket에서
중복 temp pathname 보관을 제거한다.

Retarget 회귀는 시작 이후 실제 create가 거절되고, 양쪽 namespace에 파일을
남기지 않으며 정확히 환급되는지 확인한다. Runtime 두 파일의 B13 hunk를 넣은
`3a94726…`는 ANN fault test import 누락으로 body0이었다. 테스트 import를
교정한 `44486c9…`의 f57은23개 통과·676 제외이며 r19 pair build도 통과했다.
Large Off와 Record는 원120초 deadline으로 실패했다. 동일 세 페이지의 writer
CPU는12.928→8.269초다. [B13 RCA](RCA.md#b13--writer-cpu-회수와-최상위-map-잔여).

B13 실패 prefix의 Catalog 453회 COMMIT과 물리 설치 254개 그룹에서 도출한
후속 범위는 위 B14가 소유한다. [요청 전체 분해](RCA.md#b13-요청-전체-원인-분해).

## B03-B12 — Native writer의 held parent 생성과 publish

- Core ticket이 held parent 기반 임시 생성과 atomic no-replace publish를 제공한다.
  Native writer는 최종 경로 claim 때 연 parent FD를 같은 shard의 임시 선언에
  재사용한다. Storage는 opt-in ticket에 생성과 publish를 위임하고 오류를 전파한다.
- 시작 직전, 실제 open 직전, publish 직전에 현재 parent incarnation을 재확인한다.
  같은 parent임을 두 held FD의 device/inode로 증명하면 해당 시점의 경로 순회는
  한 번이다. Publish는 원본 writer FD와 임시 inode를 대조한 뒤 실행한다.
- 출생 FD 등록 전 실패도 기존 qualified cancel로 정리한다. 성공한 rename은 즉시
  임시 이름 cleanup을 해제하고 기존 파일·디렉터리·공유 barrier를 거친다.
  외부 sync 후 정산은 실제 sync FD의 objects/shard identity를 현재 namespace와
  대조한 뒤 현재 pathname과 held pin을 확인한다.
- 초기 `8f114a2…`의 f54 집중 결과는18/19개였다. Retarget retry fixture 교정과
  실제 sync FD identity를 결속한 `2e19d9d…`의 f55는23개 통과·676개 제외다.
  r18 pair build는 통과했다. seed42 원 Large Off·Record는 BuildPrepare
  deadline에 실패했다. 세 Posting page의 일치 작업에 대한 writer CPU 회귀와
  COMMIT 분해는 [B12 RCA](RCA.md#b12--writer-parent-custody와-catalog-commit-잔여-비용)에 있다.

## B03-B11 — Native writer의 최초 전체 크기 예약

lr14에서 확인한 ticket 준비 경로가 1바이트를 예약한 뒤 `reserve_end`에서
다시 전체 크기까지 증액했다. Native immutable writer가 기존
`try_begin_create_with_bound`에 최종 크기를 전달하도록 연결한다. 최초 claim이
local maintenance reserve를 갱신한 뒤 대상 부재를 확인하고 전체 bytes와 inode
한 자리를 예약한다. 임시 경로 선언·creation_started·FD witness·정산 순서는
유지하며 Sidecar parent-anchor의 별도 변경을 함께 복사하지 않는다.

f53의12개는 기존 target, retargeted root/shard, 전체 용량 거절의 오류 위치 및
생성 전 정확한 bytes/inode/ticket 보유, 실패 후 취소·회수와 실제 Catalog
replay를 통과했다. 고정본 r17의 원 Large Off와 Record는 deadline으로 실패했다.
동일 Posting page의 ticket CPU 감소는 [B11 RCA](RCA.md#b11--최초-전체-writer-예약-검증)에 있다.

## B03-B10 — cold contribution 배치와 writer 잔여 비용 분리

- Bootstrap과 빈 prior Source의 첫 population에서 contribution 객체를 기존
  `MAX_OBJECT_INSTALL_BATCH`256개까지 한 번에 설치한다. 그 다음 ordinal과
  기존 CAS를 보존하며 scratch top SQL은 기존64행씩 제출한다. 일반 증분은
  기존64개 flush를 사용한다. payload, key, 등록 결과의 원 메모리 grant를
  사용하고 별도 등록·권한 경로를 만들지 않는다.
- macOS batch directory sync는 `O_DIRECTORY|O_NOFOLLOW`로 열고 그 FD의
  종류·owner·mode·device를 확인한 뒤 fsync한다. 경로 lstat를 중복하지 않는다.
- 관리형 writer의 기존 aggregate를 ticket 준비, create/open, created witness로
  분리한다. 역사 trace label과 unmanaged 경로를 유지한다. Off에 clock/event를
  추가하지 않으며 Runtime의 parent-anchor 작업을 복제하지 않는다.
- f52에서 64→65 owner 경계와 output ordinal, writer phase, symlink/device,
  B9 custody를 확인했다. Python34개·Rust16개와 r16 build가 통과했다. 원 Large
  Off와 Record는 deadline으로 실패했다. [B10 RCA](RCA.md#b10--posting-전-contribution-설치).

## B03-B9 — writer birth 검증 공유와 인접 Catalog 경계 합치기

- 관리형 writer는 `created` 뒤 이미 읽는 동일 FD의 metadata에서 private file과
  macOS device 조건을 검사한다. 별도 Runtime parent-anchor 작업과 경로·ticket
  인증, cleanup, file/directory sync를 유지한다.
- scratch 기반 managed Attempt의 child-first packet 두 개만 결합한다. 각 최대
  256개, 합산 wire 64MiB이며 두 packet의 gate 합집합을 정렬 순서로 먼저 잡는다.
  첫 예약을 commit하고 첫 파일과 ticket을 동기화·정산한 다음, Catalog가 첫
  확인과 두 번째 예약을 같은 transaction에서 수행한다. 두 번째 물리 쓰기는
  성공 반환 이후이고 마지막 확인은 별도 commit이다. 파일 I/O는 Catalog
  transaction 밖에서 실행한다.
- COMMIT은 두 packet마다 4→3회다. 다른 scope와 원자 전환을 제공하지 않는
  authority는 기존 경로를 쓴다. 중간 transaction 거절은 첫 provisional intent를
  보존하고 두 번째 물리 쓰기를 막는다. Catalog의 기존 collision 인증과
  admission fee는 유지하며 canonical 입력 인코딩을 재사용한다.
- 영향 배치는 Catalog의 atomic rollback/retry/reopen, Storage의 gate·sync·custody,
  기존 map CAS/EOF, trace wire를 포함했다. Python31개·Rust39개가 통과했다.
  `d2c6ff8…`의 r15로 원 Large Off와 실패 후 Record를 공용 lease 안에서 각각
  한 번 실행했다. 두 제품 실행은 deadline으로 실패했고 Record에서 동일 첫
  page의 COMMIT46→35를 확인했다. [B9 RCA](RCA.md#b9의-수리-경계).

## B03-B7 — cold Posting의 두 case 표면을 같은 설치 배치에 배치

lo11은 공용 실행 lease에서도 원120초 수용에 실패했다. 다음 변경은 cold
Posting의 동일 입력을 같은 `apply_stream` 호출에 넣어 기존 Storage 공유를
사용한다. seed42의 소스 투영에서는 두 case에 동일한 dense term 쌍157개가
있지만 기존 raw key 순서는 두 표면을 별도 페이지에 놓는다.

- prior term root가 빈 경우만 두 case의 PK cursor를 연다. suffix byte 순서로
  병합하고 동일 suffix 쌍은 페이지 경계에서 나누지 않는다. 각 페이지는64 term
  이하이며 기존 raw key 순서로 넘긴다. 전체 prefix 범위와 EOF를 확인한다.
- cardinality와 원본 편집 cursor는 두 case의 연속 구간을 각각 읽는다.
  선택 범위를 교차하는 `BETWEEN`이나 dense sort를 추가하지 않는다. 각 범위의
  완전성, 전역 ordinal·CAS·stamp·EOF는 기존 규칙으로 확인한다.
- 접미사 일치는 스케줄링 힌트다. 공유는 Storage의 같은 base와 완전한 편집
  fingerprint·scratch 재확인 조건으로 결정한다. 두 case의 출력과 기존 형식,
  incremental·pin·GC 계약을 유지한다.
- 코드 파일 소유는 case scheduler, spool 범위 소비자, 독립 회귀, Native 연결로
  나눈다. 집중 검증 뒤 단일 바이너리 쌍으로 원 Large Off를 실행한다.

### B7 검증 및 분기

B7의 f43–f45, r12/lo12 및 r13/lr12는 [실행 원장](README.md)의 이력이다.
lo12 Off와 lr12 Record는 원 Large120에서 실패했고, Record는 실제 build 생략을
확인했다. [B7 Record 분석](RCA.md#계측-후-b7-record)을 근거로 scratch 재순회는
아래 B8에, 설치의 writer·Catalog 비용은 위 B9에 넘겼다.

계측은 same-base/count/fingerprint 후보와 scratch 검증 후 실제 생략·build를
구분한다. 동일 case·term 구간과 완료 작업이 확인된 비교에서만 wall·CPU·호출량을
대조한다. 호스트 표본과 Record 상태를 보존하며 Off에는 추가 관측 비용을 넣지
않는다. 원 Large ACK·검색·재오픈 및 quiet-host overhead·mixed/출시는
[고정 제품 계약](#고정-제품-계약)에 따라 각각 수용한다.

### B8 — cold root의 build와 scratch 인증을 같은 순회에서 수행

같은 base·count·fingerprint의 뒤 root가 있는 경우 B7은 앞 root를 만들며
scratch를 읽고, 뒤 root에서 앞 root의 같은 편집을 다시 읽어 인증한다.
B8은 앞 root가 empty base일 때 기존 build cursor가 읽는 편집을 해시하고
EOF에서 원본 count·fingerprint와 대조한다. 일치한 경우만 그 root를 인증 완료로
표시한다. 뒤 root 자신의 scratch 전체 검증, 원본 CAS·EOF, 기존 nonempty
경로 및 설치·정산 순서는 유지한다. 불일치하면 scratch 시도만 폐기하고 물리
설치에 들어가지 않는다.

변경은 Main 작업 트리에 있다. B7만 포함하는 `654d204…`의 r13/lr12는
이 변경 전 대조군으로 보존한다. B8의 정적 효과는 실제 공유되는 empty-base
leader당 scratch 전체 순회 한 번 감소다. 제품 속도 효과는 미확정이다.
손상 digest·빈 EOF와 기존 동일 편집·다른 값·다른 base·late cursor failure 회귀,
Core·Storage·Searchd trace 검증을 하나의 영향 배치로 묶었다. 최종 `6df7803…`의
기능 회귀는 f47의9개와 schema 순서만 고친 f48의1개가 근거다. r14 build는
통과했고 lo13 Large Off는 원120초에서 실패했다. [B8 RCA](RCA.md#b8-재검증).

## B03-B6 — 희소 Posting 준비 조회와 기존 shard 생성 호출

빈 base에서 최종 DF가8 이하인 Posting root의 준비 조회를 최대64 term의
SQL 한 번으로 묶는다. 실제 선택 term마다8행, 총512행까지 보유하고 query·row·
메모리를 원 예산에 청구한다. 선택 root가 두 개 미만이거나 편집 행이 상한을
넘으면 기존 cursor를 사용한다. 기존 dense root의 축소는 원 per-root merge를
사용한다. 전역 cursor의 원본 재조회, 정렬·stamp·CAS·EOF 검증은 유지한다.

Storage는 기존 shard의 nofollow metadata를 먼저 읽고 실제로 없을 때만
`mkdir`를 호출한다. shard가 존재할 때도 owner·mode·type을 검사하고
기존 파일·디렉터리 동기화와 Catalog confirmation 순서를 유지한다.

집중 검증은 혼합 sparse/dense, 초과 행 fallback, 전역 cursor 순서,
Native Posting CAS, Storage batch 설치·replay, 실제 저장소 조회와 delta를
포함한다. 이후 동일 소스의 정상 pair로 원 Large120 계약을 확인한다.
f37의 Storage73개와 optional lease 타입을 교정한 f39의 Native303개 및
lifecycle2개가 통과했다. strict 진단의 lease 초기화만 교정한 `c5c691e…`에서
f41 변경2개도 통과했다. Large는 공용 실행 lease로 같은 저장소의 Cargo와
직렬화한다. f42 strict는 기존35개로 FAILED이며 이번 변경 파일 진단은0개다.
r11 pair build를 완료했다. lo11 Off는 공용 실행 lease 아래 ACK123.772초에
원120초 deadline으로 FAILED다. 전체 ACK·검색·재오픈 수용은 남아 있다.

## B03-B5 — dense Posting Tree의 비교 witness와 실제 custody 분리

코드 합류 후 새 frozen `5e41d204e6248bc9467315a063ae5a48306b5552`의 f31 Native301개가
전부 통과했다(558 제외). 실제9→8→9를 추가한 Kernel lifecycle15개도 통과했다
(65 제외). 후속 resolver 타입 별칭·테스트 borrow 교정만 반영한 현행 frozen은
`3a2ad5dd6393cabe6f83edaf7c5fa137c948687b`다. f35 변경2회귀는 통과했고
f36 strict는 전역33개 기존 진단으로 FAILED·소유 변경 파일 진단0이다.
f31과 composite 집중 proof이며 현행 전체 suite 재실행은 아니다.
r10 pair build는 완료됐다. 원 Large Off와 Record의 ACK는 각각123.507초와
127.683초에 deadline으로 FAILED다. 실제 ACK·검색·재오픈 수용은 남아 있다.
B4의 r9 build/f26 집중423개는
완료했지만 lo9 Off·lr9 Record 모두 원120초에서 FAILED다. lr9는 posting 첫
64-term page를32.329초/CPU23.082초에 완료했고 둘째 page의 apply에서 실패했다.
첫 page의 Catalog reserve+confirm은19.563초(60.5%)로 여전히 주 비용이다.

고정 계약:

- Inventory는 실제 occurrence `ObjectRef`와 그 물리 child를 계속 소유한다.
  같은 pinned Text Root의 owners→Contribution→inventory→occurrence가 실제
  참조·GC 수명 경로다. Posting Tree의 값은 `OccurrenceDigestWitness(Digest)`다.
  정확32B·physical children0의 `MapValue::Inline`이며 역참조 API를 제공하지 않는다.
- query는 owner Contribution과 pinned SourceRecord를 검증하고 inventory의 실제
  ObjectRef를 얻는다. witness와 정확 byte equality를 확인한 뒤 **그 실제 ref만**
  load한다. 기존 inventory/ref 일치·TF·positions·source pin·EOF 조건을 보존한다.
  compound 오류의 확인 순서는 바뀔 수 있지만 불일치 허용으로 바꾸지 않는다.
- 모든 Some→Some CAS를 유지한다. original spool cursor는 여전히 ObjectRef-valued
  원본이며 sorted/root/order·stamp·EOF 검증도 유지한다. Posting Tree provider에
  전달하는 expected/next만 witness 값으로 변환한다. Inventory path는 그대로다.
- Small≤8은 기존 ObjectRef와 명시 child 형식을 유지한다. singleton owner32B와
  digest32B를 inline64로 만들거나 희소 term마다 새 object를 추가하지 않는다.
- Tree→Small의 미변경 survivor만 기존 owner Contribution inventory에서 실제
  ObjectRef를 확인해 승격한다. 변경/신규 survivor는 original delta.next를 사용한다.
  digest를 `ObjectRef`로 감싸지 않는다. sticky Tree·history-dependent canonical
  shape·pending spool schema·새 GC authority를 추가하지 않는다.
- current output policy는 `posting-small8-32/tree-digest-witness32-childless-v1`을
  결속한다. 독립 SHA-256은 admission `[31;32]`에서
  `d6f66f1f3f7270561ec30af27a774f523b45935bd8d3e4f6fda8c4c89de41bac`다.
  B3/B4의 `14c6d531…`를 reader·builder가 typed refuse한다. legacy reader는 없다.
- 첫64 term의240,940개 논리 owner→occurrence 연결에서 posting 쪽 custody를
  제거하는 가설이다. 공유 DAG node가 있으므로 실제 고유 Catalog edge 행 수는 이
  투영과 다를 수 있다. key Tree와 map node 수 자체를 줄인다고 주장하지 않는다.
  물리 value wire는 오히려 약2B/row
  커질 수 있으며 기대 이익은 Catalog edge 인증·등록·GC ledger 작업이다.

소유: Root는 `posting_witness.rs`·정책 gold·문서·검증 큐,
producer lane은 `entry_patch.rs`, integration lane은 `text.rs`·`entry_map.rs`,
독립 oracle lane은 `tests/shared_entry_maps.rs`와 그 하위 파일만 쓴다.
한 lane만 같은 파일을 쓰며 모든 Cargo/제품 실행은 Root 큐 하나에서 수행한다.

검증 순서: 소스/호출 경로 정적 대조→Native 전체 영향 회귀와 새 witness/9→8→9
CAS·stream mismatch oracle→Kernel text/query/delta/pin/GC/reopen→strict 진단→새
정상 binary pair1회→Medium 진단·원 Large Off1회다. 실패하면 새 pair의 Record1회로
분해한다. 120초/caps/ANN4K 조건과 root 공개 원자성은 완화하지 않는다.

## B03-B4 — small-node 배치의 transaction-local child proof

코드와 독립 oracle를 external frozen `213908833f462447fc229519d1d6f1472e978644`에
합류했다. Main의 기존19개 소유 입력은996과 일치했고 새3개 입력을 추가해22개를
대조했다. f26 Storage85개·Catalog27개·Native297개·Kernel lifecycle14개가 모두 통과했다.
f27 Storage/Catalog strict는 기존 `object/read_only/tests.rs` 변환 lint2건으로 FAILED,
소유 변경 파일 진단0이다. r9 정상 pair build 완료, mr9 Medium 진단 cap과
lo9 Off·lr9 Record 원120초 제품 실행은 FAILED다. B4는 성능 완료로 닫지 않는다.
lr8의 첫 posting page는
서로 다른64 root·240,940 owner mapping이다. 동일 전체 root 공유는 적용할 수 없고,
이 페이지의 apply 도중256 input짜리 reserve/install/confirm38 triplet 뒤 deadline에
도달했다. 직접 자식 시간은 confirm23.642초, install13.196초, reserve9.816초다.
COMMIT19.307초에는 checkpoint/backfill10.571초가 포함된다. lock hold를 경합으로
잘못 해석하거나 이 inclusive 시간을 더하지 않는다.

선정한 수리는 기존 Catalog의 같은 transaction 증명 창만 확장한다. 이전 capacity는
각 object의 children 최대값이64 이하면, 배치 전체에 수천 child가 있어도64개만
기억했다. 수정은 **배치 total children≤64일 때만64/64**, 그 이상은 기존 경로와
동일하게 total을4,096개로 제한하고2배 이상 power-of-two slots를 확보한다.
최대8,192×`size_of<Option<ObjectRef>>`=270,336B이며 계획용 창과 실제 검증 창은
동시에 보유하지 않는다. 명목 bytes·실제 할당·grant가 일치해야 한다.

- 실제 locked transaction·exact reference로 검증한 metadata·edge·GC adoption만
  기억한다. 성공 후에만 삽입하고 충돌은 full reference로 비교한다. 포화 뒤에는
  기존 전체 재검증을 수행한다. 다음 transaction으로 증명을 넘기지 않는다.
- `charge_object`의 fee 상수와 전체 product caps는 유지한다. 계획용·실제 창의
  capacity가 같으므로 실제 생략한 adoption만 기존 식으로 credit한다.
- Storage는 별도 좁은 수정으로 empty-base CAS의 불필요한 nibble 변환을 제거하고,
  알려진 nibble 수를 한 번 예약한다. expected mismatch·original stream EOF·sorted
  검증·무설치 실패 순서를 보존한다. 단독으로120초 해결이라고 주장하지 않는다.
- one-writer: Catalog `managed_install.rs`, 독립 SQL/rollback oracle
  `tests/prechecked_children/repeated_children.rs`, Storage `map.rs`다. Root가3개
  소스와 기존19개 소유 입력을 합류해 검증 큐 하나를 운영한다.
- 정적 검토 뒤 Storage map 회귀·Catalog child/managed-install 회귀·strict Clippy를
  모아 실행한다. 새 source의 정상 pair build와 원 Large Off1회를 수행하고, 실패
  구간이 새로 드러나면 같은 pair의 Record1회로 분해한다. Medium은 진단만 한다.
- 완료 조건은 원 Large120초의 full ACK와 후속 search/reopen이다. 집중 테스트나
  더 많은 prefix 진행을 완료로 승격하지 않는다.

**NOT_SELECTED:** 4K 전체 Posting을 단일 Block으로 묶는 단순 한도 확장. 현 codec의
O(n²) child 검사, query의64행마다 전체 재읽기, 단일 변경의 전체 재작성과
4,096→4,097 Tree 전환 cliff를 새로 만든다. 별도 format/query/증분 설계 없이
채택하지 않는다. 같은 Tree build dedup·VFS 안전 검사·자동 checkpoint 재구현도
제외한다. 기존 per-term4,096B 논리 tariff를 SQL query당 fee로 바꾸지 않는다.

## B03-B3 — inventory block child dictionary

계약·코드 합류 및 f22 집중 회귀 완료. r8 Medium은 fixed512MiB 진단 cap,
lo8 Off와 lr8 Record Large는 원120초 deadline에서 FAILED다. B2 이후 B3 이전의
frozen a60은 QITXEM01 inventory에서 term마다 같은 child ref를 반복했다.
B3의 현행 QITXEM02는 그 단일 codec의 물리 중복을 제거했다. 완료된 P1-A의
inventory 기능과 다른 저장소 권한은 재구현하지 않았다.

- 단일 current format은 **QITXEM02**다. header는 magic8 + BEu16 row_count +
  BEu16 unique_child_count다. 정렬된 각 row는 BEu16 key_len + key + BEu16 child_slot이다.
  등록 children은 sorted key 순회 중의 first-use 순서이며 중복이 없다.
- rows1..512, key0..1024, payload≤16384B를 유지한다. 빈 key의 기존 generic map law와
  Small canonical 선택을 보존한다. 모든 slot은 범위 안에 있고,
  첫 사용 ordinal은0부터 연속이며 children 전부가 사용돼야 한다. 중복 children,
  미사용/범위 초과/noncanonical slot, malformed key/order/trailing bytes는 거절한다.
- 논리 key→ObjectRef mapping과 CAS stream binding은 보존한다. lookup·scan·BlockCursor는
  같은 dictionary decoder를 사용한다. Small·posting Tree·generic fallback은 바꾸지 않는다.
- LAYOUT_POLICY에 새 wire를 결속하고 이전 정책은 typed refusal로 닫는다. QITXEM01
  parallel reader나 저장소 migration/새 GC authority는 추가하지 않는다.
- 기존 Catalog의 unique child edge ledger·pin/GC 경계를 유지한다. 부모 등록에는 고유
  children만 전달하고 원 registration/physical install 예산을 그대로 적용한다.
- 코드 one-writer는 codec/reader·builder, Native 독립 codec/cursor/custody oracle,
  Kernel physical-wire oracle·정책 gold로 분리한다. Root가 정적 검토 뒤 검증을 직렬화한다.
- 독립 literal fixture로 서로 다른/공유 child,512 rows/512 unique, row/payload 경계와
  malformed dictionary를 검증한다. 실제 pin·GC·delta·reopen 및 동일 ordered mapping의
  동일 artifact 검증을 보존한다. Medium fixed1M/512MiB 진단과 Large120초 제품·
  별도 ANN4K query 조건을 구분한다.

정확 `LAYOUT_POLICY` 교체의 독립 SHA-256 재계산은 B2 old
`b2c1df18b452485d69ef4e687b7a3519f07cfff76fe25f0a665d8706258d7d95`를
재현했고 B3 new
`14c6d531d1304ea4d7e0c751903224e4a4db0ef8991df6bec52db7421e4cc0a8`을
산출했다. Kernel physical-wire oracle는 초기 delta의10행/고유4 child·216B와
Tree 복귀의10행/고유5 child·248B를 구분하고, 260-selector의 고유1→2 child·
old/current pin을 검사하도록 수정했다. 19개 소유 입력/8개 변경의 external frozen
`39f4a9c3bd82843aea2b390411d8ec531a0d3b2c`에서 f22 Native297개 통과·558개 제외,
Kernel14개 통과·65개 제외로 집중 범위는 VERIFIED다. helper 산술1파일만 교정한
`996230439fbf187661c7462b018c113c8b3de1bd`에서 f25 변경 case1개가 통과했다.
f24 strict Clippy는 전역 타 owner33개로 FAILED, Native 진단0이다. r8 정상 cached
build는 완료됐으나 mr8 Medium은 fixed512MiB 진단 cap, lo8 Off·lr8 Record Large는
원120초 deadline에서 FAILED다.

## B03-B2 — owner-local block의 물리 설치 단위

계약과 코드 합류 완료. f20 Native295개, f21 변경 Native1개·Kernel14개를 통과했다.
f20 Kernel의 helper E0507/body0은 새 테스트 소스에서 교정했다. a608의 typed fee 수리와 f19 집중 회귀는
통과했지만 r6 Medium도 bytes cap에서 실패했다. 다음 후보는 제품 예산을 그대로 두고
owner-local occurrence block의 한도를 **8,192B / 512 canonical terms**로 바꾼다.
소스 기반 seed42 투영은 Medium512→256개, Large12,288→4,096개 block이다.
이것은 물리 객체 수·payload의 추정이며 설치 호출 수나 속도 개선 증거가 아니다.
16KiB 한도는 이 입력에서 추가 이득이 없어 제외한다.

- QITXOC01의 기존 단일 wire/reader를 유지하고 bounds를 output policy에 결속한다.
  직전128행 policy와 leaf-only policy는 typed refusal로 검증한다.
- owner를 섞지 않는다. 동일 positions vector는 한 block에서 공유한다. singleton,
  oversize, nonempty-prior Patch의 canonical leaf와 unchanged old ref 보존은 유지한다.
- Full은 원래 owned 입력을 반환한다. 취소·memory/work 거절을 Full로 바꾸지 않는다.
  모든 metadata/payload/position allocation을 기존 grant로 입장시킨다.
- 하나의 physical ref에 512개 selector가 연결돼도 64행 SQL flush와 aggregate
  planned-confirmation receipt의 정확한 row/ref binding을 보존한다.
- code lane은 codec+codec oracle / producer+policy oracle / Kernel realstore oracle로
  분리한다. 각 lane은 Cargo와 외부 제품 실행을 하지 않고 Root가 검증을 직렬화한다.
- 집중 수용: literal wire와512행·8KiB 경계, cancellation/memory refusal, 64/256행을
  넘는 fanout, public reader의 첫/마지막 selector, delta·old pin·reopen·GC다.
- 제품 수용: Large explicit67M/512GiB logical funding·dispatch120초, 기존 query
  work/memory/SLO를 유지한다. Medium fixed1M/512MiB는 진단 입력이다. query64
  cache의 더 큰 metadata와 cold retained-read bytes를 보고한다.
  객체 수 감소만으로 채택하지 않으며 외부 기존 채팅의 ANN/SDK 경로를 재구현하지 않는다.

Query 메모리의 코드상 상한 점검: 실제 bench의 Text `read_need.memory_bytes`는
64MiB(`crates/quanta-index-searchd-harness/src/scale/native_index.rs:390–422`)다.
`CheckedBlock` layout 20,512B×64와 `PreparedRows` backing을 각각 예약해
2,625,536B가 되며, 최대 payload 8,192B×64=524,288B와 작은 `StoredObject`
overhead를 더하면 약 3.16MB의 charge다. 성장 중 중첩 예약의 peak도 이 steady
상한보다 크지 않다. 이는 64MiB 내 적합성의 소스 산술이며 RSS 전체 보장은 아니다.
Kernel은 registry ceiling 이하의 reader 선언값을 정확히 hold하고 held guard와
재대조한다. SDK 요청은 이를 override하지 않는다. 테스트 helper의 4MiB는 제품
sizing 근거가 아니며 더 작은 custom need는 `ResourceLimited`로 거절될 수 있다.

합류 소스는 외부 stable `s3`의 `878be6c9e4e33dd35b9c80cc5b450c2cf3f6f099`다.
이전 a608 대비 codec/producer/reader gold/독립 oracle의6개 파일이 달라졌고, 소유
입력15개를 고정했다. 새 output policy(admission `[31;32]`)는
`b2c1df18b452485d69ef4e687b7a3519f07cfff76fe25f0a665d8706258d7d95`다.
직전 `eef15862…`(4KiB/128행)와 `559e0482…`(leaf-only)는 typed refusal
oracle로 남긴다. 두 이전 policy의 자동 변환·fallback은 없다. f20 Native는295개 통과,
Kernel은 E0507 compiler FAILED/body0이다. Main의 test helper와 큰 inventory의
EntryMapBlock positional oracle를 교정한 frozen `a60ac6576a0ca2cae306ee68a014b11c3736246f`에서
f21 Native 새 case1개·Kernel14개가 통과했다. f20 Native295개와의 composite 집중
proof이며 정확 a60 전체 Native suite는 미실행이다. r7 정상 cached build는 완료됐지만
mr7 Medium cap과 lo7 Large120 deadline은 FAILED다. 이 source에 f19/r6 증거를 승격하지 않는다.

현행 RCA와 범위는 [README](README.md)와 [RCA](RCA.md)가 소유한다.
이 문서는 새 패킷의 계약과 완료 조건이다. B01 계약은 확정했고 B02 생산자·소비자와
독립 테스트 소스를 합류했다. B02의 Core331개·Text154개와 Python1,040개 영향 회귀는
통과했고, strict Clippy와 실제 생산자→소비자 캡처는 아래 상태를 따른다. 새 mr2에서는
실패-prefix6계수와 capture health를 확인했으나 제품 full ACK는512MiB cap에서 거절됐다.
RR 후속 counter 종료 순서 결함은 canonical parser와 공통 summary reader에서 수리했다.
관련1,024개 통과·2개 skip은 이 검증기 범위의 완료 근거다. B01–B04의 성능 구현·수용
상태는 각 패킷과 README의 원본 가용성 판정을 따른다.

## 고정 제품 계약

- Large4,096-source SourceAdvance의 서버 dispatch120초와 기존 명시적 work caps를 사용한다.
- 정상 ACK 뒤 current/old search, delta/noop/delete, drain/reopen을 검증한다.
- FULL·auto1000, GC·reserve·physical capacity와 원 memory/work 거절 계약을 적용한다.
- identity, 실제 payload 형식, root/namespace 현재성, cancellation과 오류 settlement를 검증한다.
- Medium은 동일 UDS 경로의 진단 입력이다. Large 완료와 정식 overhead를 각각 판정한다.

<a id="execution-plan"></a>
## B03-B1–B3 실행 계약·이력 — 2026-11-07

목표는 원 Large120초에서 full ACK와 후속 정합성을 끝내는 것이다. 계측이나 테스트
통과만으로 목표를 닫지 않는다. 기존 B01/B02를 다시 구현하거나 새 티켓을 만들지 않는다.

| 순서 | 할 일 | 반드시 남길 결정/결과 |
| --- | --- | --- |
| 1 | normal cached build와 Medium Record 실행 완료 | build exit0·mr5 fixed512MiB 실패-prefix4계수/health 확인. CLI `build_binding=UNPROVEN`과 managed-fresh/제품 성공으로 승격하지 않음 |
| 2 | 같은 binary pair로 Large seed42 Off를 원120초·기존 caps에서1회 | lo5 `BuildPrepare` deadline FAILED. full ACK·후속 정합성 미도달, numeric cap 거절 없음 |
| 3 | 비용과 실제 생산자를 대조해 A/B 또는 실제 owner 수리·보류를 판정 | 수정할 함수, 없앨 정확한 작업, 예상 절감 단위, 권위/수명 계약, 반증 oracle를 고정. 여러 가설을 동시에 구현하지 않음 |
| 4 | 공통 계약을 먼저 합류하고 생산자·소비자·독립 oracle를 병렬 구현 | 아래 독점 범위 준수. Root만 공통 타입·parent/spool 접점과 통합 수정 |
| 5 | 코드 합류 뒤 영향 검증을 한 큐에서 실행 | 정적 대조→compile/소유 oracle/strict→정상 binary1회→Medium Record1회→Large Off1회 |
| 6 | 원 계약으로 완료 판정 | 변경 후 Large120 full ACK와 후속 정합성이 실제 통과. 비용 수리의 인과 비교와 정식 tracing overhead는 각각 별도 판정 |

Medium은 진단이며 fixed512MiB의 성공을 별도 explicit funding인 Large의 실행 선행조건으로 두지 않는다.
Medium 실패를 보존하고 원 Large caps로 독립 판정한다. 파일 수16배·bytes23.42배인 Large의 시간 예측으로 쓰지 않는다.
중복 map build를 또 제거하는 B4 안은 제외한다. Storage `map.rs:1880–2080`이
동일 root의 검증된 공유를 이미 구현하며, 새 코드 소유 범위가 아니다.
첫 Large가 deadline 실패하고 Medium만으로 동일 경로를 고를 수 없을 때에만 같은
binary의 Large Record1회를 추가한다. 실패한 source·입력·호스트 조건을 그대로 둔
반복 실행은 하지 않는다. 새 결과가 기존 변경으로 목표를 충족하면 불필요한 B03은 폐기한다.

Root는 정상 binary pair의 원본 build receipt·SHA·고정 source를 함께 보존하고 같은
cohort의 관측에는 재사용한다. 각 관측마다 fresh managed build를 다시 하지 않는다.
prebuilt CLI가 build_binding=UNPROVEN으로 남기는 제한을 임의로 고치거나 정식
qualification으로 승격하지 않는다. 제품 ACK/정합성 결과와 해당 build-binding의
한계를 분리해 기록한다. 제품 코드가 바뀐 경우에만 새 cohort와 정상 binary를 만든다.

## 중복 제외와 소유 경계

새 작업 전 HEAD·dirty hunk·호출 경로를 읽고 기존 담당의 의미상 수정과 비교한다.
파일이 같아도 독립 함수면 분리할 수 있지만 같은 hunk·authority·proof rail은 합류 owner가
한 번만 변경한다. 기존 담당의 실시간 활성 여부는 이 문서 작성만으로 확인되지 않는다.

| 기존 영역 | 이번 패킷에서 소비할 결과 |
| --- | --- |
| Native Text256 packet·Small/Block·inline·unchanged·exact dedup | 현재 구현과 독립 oracle 재사용 |
| occurrence count memo/spill, Kernel read_retained 위임 | 현재 호출 경로와 grant 경계를 먼저 확인 |
| fresh claim·allowance·Catalog/VFS·checkpoint·GC | 기존 소유자 수정 보존. 이 패킷을 이유로 재구현하지 않음 |
| producer/DELTA·SDK·ANN·overlay·Watch·release | 해당 기존 owner와 E01–E08 수용 경계 |
| Python child-root 수리 | 현재 코드와 과거 consumer a41830d의 영향 결과 재사용. 외부 consumer 컷은 현재 부재 |

<a id="b01"></a>
## B01 — 계수와 유효성 계약

상태는 CONTRACT_FIXED이며 Root가 계약과 공용 접점을 소유한다.

### B02 현행 계수 계약

기존 NativeTextInventoryDiff lane2 span 하나에 다음6개 u64를 각각 한 번 기록한다.
Record에서0은 관측된0이고, counter family가 없으면 미관측이다.

| Core counter / wire key | 의미 |
| --- | --- |
| TextOccurrencePriorValidationAttempts / text_occurrence_prior_validation_attempts | prior 쪽 full payload count 검증 함수 호출. 거절·실패도 호출 수에 포함 |
| TextOccurrenceNextValidationAttempts / text_occurrence_next_validation_attempts | next 쪽에서 RAM/spill 재사용으로 끝나지 않아 full count 검증 함수를 호출한 수 |
| TextOccurrenceRamReuses / text_occurrence_ram_reuses | 같은 attempt의 이미 검증된 RAM count 반환 |
| TextOccurrenceSpillReuses / text_occurrence_spill_reuses | 같은 attempt의 confirmed spill에서 이미 검증된 count 반환 |
| TextOccurrenceEqualReuses / text_occurrence_equal_reuses | old와 next가 같은 identity여서 이미 읽은 prior count를 next에도 사용 |
| TextOccurrenceValidationFailures / text_occurrence_validation_failures | 위 prior/next count 검증 함수가 Err를 반환한 수 |

attempt/term/SQL 사전 거절은 payload 검증 호출 전이라 해당 attempts/failures에
포함하지 않으며 Diff span의 실패로 남긴다. failures는 두 attempts의 합 이하이다.
이 계수는 object-port validation 호출이며 syscall·실제 bytes·고유 reference 수가 아니다.
전역 unique 수는 미관측으로 고정한다. 추가 set/SQL 없이 현재 권위에서 얻을 수 있는
정확한 계수만 먼저 구현한다.

Off는 기존 diff를 직접 수행한다. Record는 고정 크기 stack totals만 사용하며 완료와
Err 모두 span 종료 전에6개 값을 발행한다. overflow는 CounterOverflow health를 남겨
증거를 무효화하고 제품의 원래 결과를 유지한다.

V5에서만 새 family를 허용하고 owner는 NativeTextInventoryDiff lane2로 제한한다.
완전한 capture의 family는6개가 함께 있고 중복이 없어야 한다. 종료 뒤 counter는
동일 timestamp도 거절한다. 이전 capture에 family가 없으면 재사용0으로 채우지 않는다.
소비자는 request ancestry별로 성공·실패 Diff 범위와 계수를 보고하고 full ACK나
전역 unique·물리 I/O로 승격하지 않는다.

독립 예시는 새 leaf 최초 검증1회/다음 RAM reuse1회, confirmed spill reuse1회,
prior 검증1회, equal identity의 prior 검증1회+equal reuse1회, 검증 실패의
attempt1/failure1이다. 실패한 검증 결과는 재사용하지 않는다.

편집 전9개 영향 파일은 checkout 밖
`~/.codex/artifacts/nov-5-indexing-bugbash/b02-before-20261105-sk6a9gkl`에 보존했다.
이는 영향 소스 기준선이며 전체 build/performance cohort로 사용하지 않는다.

먼저 기존 `validated_new_count`와 prior 읽기 경로에서 다음 의미를 고정한다.

| 항목 | 필요한 구분 |
| --- | --- |
| 검증 요청 | 신규 confirmed reference와 prior reference |
| 결과 재사용 | RAM memo, confirmed spill, 실제 payload validation |
| 반복성 | 전체 호출과 기존 exact window/confirmed 집합이 보증하는 고유 reference |
| I/O | 논리 get, 실제 신규 write, SQLite requested bytes를 별도 단위로 유지 |
| 시간 | 부모·하위 CPU/wall의 중첩과 미관측 범위 |
| 실패 | 실패 전 관측값과 성공한 전체 요청의 값을 구분 |

새 unbounded set이나 reference별 SQL을 계측용으로 추가하지 않는다.
고유 수를 기존 권위와 예산 안에서 정확히 얻을 수 없다면 계수 domain을 좁히고
전역 고유 수는 미관측으로 남긴다. 각 count의 scope와 exact 여부를 먼저 합의한다.

기존 batch/span 단위 aggregate를 확장한다. per-object trace event나 별도
관측 파이프라인은 필요 근거가 있어야 발급할 수 있다. Off 경로에는 신규
clock/집합/SQL/trace emission이 생기지 않아야 한다.

완료 조건은 독립 예제로 신규1회·반복·prior·memo/spill·실패의 기대 count를 고정하고,
기존 Core 진단 타입·wire·schema·소비자의 변경 지점을 한 계약으로 정리하는 것이다.
코드 편집 전에 해당 소스 기준선을 보존한다. 코드를 먼저 합류하고 실행을 몰아서 하는
순서는 기준선 소스를 버리는 이유가 아니다. B03 효과 비교의 두 arm에는 같은 B02 계측을
적용하고, 비교할 수리 hunk의 차이만 소스와 실행 파일에 결속한다.

<a id="b02"></a>
## B02 — 최소 계측과 소비자 연결

### 현재 소스와 정확한 잔여

- Text 생산자, Core6개 enum, daemon wire/schema·이름 golden, Python parser와
  요청별 report/product/canonical 전달 경로는 소스에 존재한다.
- 새 `native/text/tests/occurrence_validation_trace_tests.rs`에는 RAM·spill·prior·equal,
  실패 비메모화와 Off/Record 동등성의 독립 기대값이 있다. `text.rs`의
  `mod tests`에 등록해 실제 빌드·실행했다.
- 기존 `native/text/tests/diff_trace_tests.rs::run_diff`의 외부 Diff wrapper를 제거했다.
  Lexical·Diff span이 각각 하나인지, 실제 object-port child의 귀속이 맞는지 확인한다.
- 전체0 family, prior 검증 실패, payload 검증 전 term/attempt 거절의0 attempts/failures,
  overflow의 health/전체 family 생략 회귀를 추가했다. 기존 cancellation·손상·수명 회귀는
  재사용한다. Text 소유154개 테스트에서 새8개와 기존 Diff 귀속 회귀가 실제 통과했다.
- 기존 Core `scoped_diagnostic_context(None)`은 inherit 계약이다. 호출부의 mask 주석과
  다른 기존 seam을 발견했다. 원 계약은 유지하고 `scoped_diagnostic_override(None)`을
  명시 mask API로 추가했다. 같은 private RAII를 재사용하며 Off는 request clone/clock을
  추가하지 않는다. Lexical 관측 helper의 child=None과 Diff span 생성 실패를 연결했다.
  요청 identity·deadline·provider 권한·work 사용량·취소·unwind 복원과 실패한 Diff의
  하위 span 미발행을 Core331개·Text154개에서 확인했다.
- Python4모듈 배치의 첫 실행은 FAILED:1,038pass/2fail/2skip이었다. 새 fixture의
  raw/parsed 인자 순서와 overflow 오류 문구 기대값을 교정한 뒤 같은 배치는
  VERIFIED:1,040pass/2skip이었다. wrong-version의 빈 regex 경고도 제거했다.
  skip은 prebuilt capture의 managed-build receipt 부재라는 기존 조건이다.
- strict Clippy는 FAILED다. 최초 dependency 포함 명령은 다른 Contract Go trace의5개
  지적에서 멈췄다. Core/lexical `--no-deps` 진단도 다른 ranker·reader·packed writer
  등의 지적 때문에 실패했다. 이번 계수 생산자/테스트 지적은 수정했으며, 마지막
  테스트 표현식 lint 교정의 좁은 재검증은 정상 빌드 뒤의 같은 admission 큐에 넣어
  300초 입장 대기 후 exit124로 종료됐다. 테스트 본문0개로 NOT_RUN이며 다시 시작하지 않았다.
  이 진단을 전체 gate 통과로 바꾸거나 다른 담당의 기능을 재구현하지 않는다.
- 당시 B02 완료 시점에는 실제 V5 생산자→소비자 검증과 Large120 성능 수용·정식 tracing overhead가 NOT_RUN이었다. 현재 B1 실행 판정은 README가 소유한다.
- 이번 잔여 감사에서 영향 경로의 tracked diff에 대한 `git diff --check`는 VERIFIED다.
  이는 whitespace 검사이며 컴파일·동작·성능 수용 근거가 아니다.

2026-11-06 KST, 공유 dirty HEAD `de16b193b`에서 실행한 영향 검증 명령:

```sh
./scripts/cargow --lane test-workspace-lane test -p quanta-index-lexical --lib native::text:: --locked --offline -- --quiet
./scripts/cargow --lane test-workspace-lane test -p quanta-index-core --lib --locked --offline -- --quiet
.venv/bin/python -B -m pytest -p no:cacheprovider -q --tb=short \
  tools/ci/tests/test_causal_cost_timeline.py tools/ci/tests/test_causal_cost_profile.py \
  tools/ci/tests/test_causal_cost_capture.py tools/ci/tests/test_causal_cost_canonical.py
```

최종 본문 결과는 Text exit0/154pass(0.14초), Core exit0/331pass(0.82초),
Python exit0/1,040pass·2skip(49.06초)다. Rust 최초 빌드는2분03초였다.
mask 회귀의 초기 실패2회는 inherit API를 mask로 오인한 수리가 불충분했음을 보였다.
Core 신규 테스트의 타입 추론·unused-mut 컴파일 오류도 수정 후 같은 배치에서 통과했다.
포맷과 Python Ruff E9/F도 통과했다. 마지막 Text 테스트 표현식의 lint 교정은
`#[cfg(test)]` 한 곳이며 위 전체 통과 이후 변경이다. 그 영향8개 재실행 상태는 별도다.

B01 완료 뒤 코드 작업을 다음과 같이 병렬화한다. 공용 parent와 schema 합류는 Root 한 곳이다.

| Lane | 독점 변경 범위 | 결과 |
| --- | --- | --- |
| Text | `native/text.rs`의 기존 count 검증·diff 관측 함수와 그 영향 테스트 | 합의한 신규/prior/reuse 계수만 기존 scope에서 누적 |
| Consumer | 기존 Python trace/canonical 소비 함수와 해당 독립 fixture | 계수의 domain·누락·실패를 유지하고 범위가 다른 값을 합산하지 않음 |
| Oracle | 기존 Text/Storage fixture의 해당 시나리오와 기대값 | 구현과 독립적으로 count·손상·오류·메모리 기대값 고정 |
| Root | Core 진단 enum·wire/schema·parent 연결과 최종 문서 | producer부터 consumer까지 같은 계약으로 합류 |

Text lane과 Oracle lane은 테스트 함수와 parent 등록 hunk를 시작 전에 나눈다.
새 crate, 설치 backend, service, persisted format은 이 패킷의 산출물이 아니다.

완료 조건은 공통 entry point의 Off/Record 동작, 정상/실패의 bounded aggregate,
정확한 counter와 source 연결을 집중 검증하는 것이다. 아직 실행하지 않은 검증은
NOT_RUN으로 기록한다. 실제 관측에서 비용이 줄어들지 않으면 계측 성공만 기록한다.

<a id="b03"></a>
## B03 — 검증 결과 전달과 구조 수리

상태는 B03_B1_PRODUCT_FAILED_TYPED_FEE_FOCUSED_VERIFIED_MEDIUM_FAILED_B2_PENDING이다. File get 이중 SHA·Catalog registration
재인코딩 제거와 work delta 연결은 집중 검증했지만, 후속0a97e858의 실제 Medium은
512MiB cap, Large Off/Record는 원120초 deadline에서 실패했다. B03-B를 일반적인
cold read 증가만으로 중단한 이전 판정은 아래 원 기준 정정으로 대체한다.
owner-local block의 집중 회귀는 composite local proof로 통과했다. 실제 Large의 첫
Patch 활성화는 `d8e2c52d` 후보에 포함돼 해당 f17 transition test가 통과했지만
mr5 Medium에서는 실제 활성화됐지만 512MiB bytes cap에서 FAILED다. 같은 r5 pair의
lo5/lr5 Large Off/Record도 원120초 `BuildPrepare` deadline에서 FAILED다.
최신 `a6083fb2` 후보의 typed confirmation fee는 코드 구현과 f19 영향 집중
검증을 마쳤다. r6 normal cached pair build exit0 뒤 mr6 Medium도 bytes cap에서
FAILED다. Large120은 NOT_RUN이다. 이전 `d8e2c52d`의 제품 결과를 새 후보에
승격하지 않는다.

### 선택표와 필요한 관측

| 관측 결과 | 결정 | 하면 안 되는 변경 |
| --- | --- | --- |
| 현재 Large가 원 계약으로 통과 | B03 성능 패치 불필요. 남은 통합/overhead 범위만 진행 | 이미 해결된 비용을 이유로 포맷 개편 시작 |
| 신규 full validation이 실제 critical path 비용이고 현재 저장 경로가 추가 read 없이 인증된 payload/count를 생산·전달할 수 있음이 입증됨 | B03-A 검증 결과 전달 | 입력 position 길이·미검증 SQL count를 인증값으로 사용 |
| 같은 Native occurrence 생산자의 작은 신규 파일·namespace/create·sync가 큰 비용 | B03-B Lexical occurrence 묶음의 최소 설계/독립 fixture부터 검증 | 전역 Storage segment backend를 바로 신설, generic batch 크기만 증대 |
| 비용이 SourceApply·map·Catalog·기존 객체 검증에 있음 | 해당 실제 owner의 중복 작업1개를 특정해 다음 hunk를 정함. A/B는 그 원인에 대해 폐기 | occurrence 계수만 보고 unrelated phase를 수리 |
| 남은 wall이 주로 대기이고 CPU/I/O 귀속 불명 | 동일 binary의 필요 진단1개에서 기존 물리 phase와 제한된 OS thread sample로 분리. 비교 불가면 인과 주장은 BLOCKED | wall−CPU 전체를 host-only로 설명하거나 무변경 Large 재실행 |

6개 Diff 계수는 검증 호출/재사용 수다. 고유 reference·syscall·제거 가능한 시간을
보증하지 않는다. 현재 Native 경로에는 object-kind별 물리 집계가 연결돼 있지 않다.
legacy LexicalStore의 object_kind_aggregate를 Native 실측으로 사용하지 않는다.

51b70b34와 동일한 현재 Text 본문에서 기존 ancestry만으로 귀속이 가능함을 확인했다.
Prepare owner/lane35/잔여 flush의 설치 가능 경로는 `OccurrenceWrites::flush`뿐이고,
그 함수의 유일한 설치는 `context.put_batch(&self.inputs)`다. inventory map 설치는
별도 NativeTextInventoryMapApply sibling이다. 따라서 이 고정본의 가장 가까운
Prepare 조상을 가진 physical packet은 occurrence 설치로 읽는다. 추가 wrapper는 불필요하다.

이는 관측된 해당 subtree의 귀속이며 전체 capture의 per-kind census·고유 reference 수를
뜻하지 않는다. 기존 packet 입력·신규 write·phase wall/CPU를 사용하고 신규 write는
성공한 publish/confirm과 함께 해석한다. per-reference event·전역 unique set·새 글로벌
kind 계수는 추가하지 않는다. C17 옛 source byte를 다시 확보하지 않고 이 증명을 과거
C17 전체 capture에 소급하지 않는다.

추가 코드 대조로 좁힌 판정 축은 다음과 같다. 이 목록을 새 미구현 티켓으로 발급하지 않는다.

- NativeTextPostings/NativeTextRoot는 입력 loop 뒤에 있다. Prepare prefix의 개선만으로
  후반 완료를 가정하지 않는다. inventory Block은 posting에서 금지되므로 두 비용을 분리한다.
- lane35 CPU가 큰 경우에만 세 번의 bounded pending 탐색/encode 비용을 검토한다.
  RAM64+spill은 이미 있으므로 새 cache 구현을 기본 수리로 잡지 않는다.
- File get의 read_wire→decode_stored_wire에는 동일 wire hash의 정적 중복이 있다.
  영향이 확인되면 private 검증 결과를 재사용하는 좁은 수리를 검토하되 scratch/resident
  decode의 인증을 제거하지 않는다. 같은 검증을 다른 phase로 옮겨 속도 수리라고 하지 않는다.
- semantic 미설정/provider_units0 fixture의 Lance/DataFusion compile은 빌드 graph 비용이다.
  제품 Text120초의 semantic 실행 원인으로 분류하지 않는다.

<a id="b03-a"></a>
### B03-A — 검증된 결과를 실제 소비자까지 전달

선택 조건은 신규 first-validation의 실제 비용과 전달 가능한 기존 권위가 모두 있는 것이다.
next_validation_attempts가 작거나 대부분 prior 읽기이면 이 후보를 폐기한다.
설치 뒤 새로 같은 read를 추가해 단지 Diff 밖으로 이동시키는 변경도 폐기한다.

우선 기존 `StoredObject`, `read_retained`, confirmed-count와 attempt/grant를
재사용할 수 있는지 확인한다. 새 권한 타입이 필요하면 어느 생산자가 실제 저장 bytes를
검증하는지, 무엇이 identity·형식·count를 보증하는지부터 고정한다.

수정 범위는 생산자의 실제 저장/검증 결과, Kernel wrapper/port의 해당 전달 경로,
`ConfirmedOccurrences::validated_new_count_observed`와 diff의 해당 소비 부분이다.
Root가 authority·attempt·reference·count·memory lifetime과 confirm 실패 시 무효화를
고정한 뒤 writer/reader를 나눈다. DTO만 추가하거나 검증 없이 count를 메모하는 것은 제외한다.

필수 불변식은 다음과 같다.

- candidate position 길이나 미검증 SQL 값만으로 validated count를 만들지 않는다.
- 다른 identity·잘못된 payload·잘못된 count를 기존 gold와 같은 경계에서 거절한다.
- 수리의 결과를 실패한 install·미완료 confirm·다른 attempt에서 재사용하지 않는다.
- namespace 현재성과 실제 저장 authority의 검증을 보존한다.
- retained payload와 metadata를 원 메모리 grant에 포함하고 해제·취소를 검증한다.
- 실패·재시도·no-replace loser·기존 객체 경로에서 일관된 상태와 원 오류를 유지한다.

설치 API를 바꾸는 경우 실제 File backend, Kernel wrapper와 모든 caller를 같이
검증한다. mock·serializer·한쪽 DTO의 성공만으로 완료 판정할 수 없다.

XL cold 후보도 이 패킷에서 검토한다. 설치·confirm과 inventory CAS/EOF를 통과한
결과를 `(term, owner, occurrence ref, validated count)`의 bounded 정렬 run으로
전달하는 방식이다. 고정 merge fan-in과 scratch admission 안에서 기존 posting
builder를 호출할 수 있는지 검증한다. 기존 occurrence/inventory wire, 증분·삭제,
foreign ref·중복·취소·fence·ENOSPC 거절은 유지한다. 현재 증거는 부분 Diff까지만
있다. 전체 절감량과 scratch peak는 미측정이다.

현행 SQL inventory cursor는 `entry_patch::apply` 이후 Diff 종료까지 남는다.
Tree apply는 반환 root의 비어 있음만 확인하므로 현재 Diff의 실제 root 재읽기가
독립 검증을 담당한다. 이 확인 없이 cursor로 대체하지 않는다. inline·인증 완료
OccurrenceBlock을 별도로 비교할 때도 packed count 재사용과 동일한 token 합계를
유지한다.

<a id="b03-b"></a>
### B03-B — Lexical occurrence 저장 단위를 묶기

상태: IMPLEMENTED_FOCUSED_VERIFIED_PRODUCT_NOT_RUN. 원문 RFC12.1–12.5/E03/E07 대조 결과 universal
cold query bytes 비증가는 원 hard gate가 아니었다. 이전 자체 폐기 기준을 정정한다.
block은 구현됐지만 제품 채택 전이며 원 query work/memory/SLO/gold·old/current pin·
증분 locality를 고정 profile에서 만족해야 한다. Large120과 ANN4K 원 한도는 불변이다.

이 후보는 inventory의 기존 Small/Block을 다시 만드는 작업이 아니다. term별 position
payload를 작은 bounded immutable block에 묶어 물리 객체 수를 줄이는 후보이다.
현재 map의 ObjectRef/child-edge와 Storage 설치 API를 먼저 재사용한다. 전역 object
store format/segment/locator/GC authority 개편은 이 단계에서 제외한다.

구현에 적용한 계약은 다음과 같다.

- block 내용은 한 owner의 logical inventory로 한정한다. 현재 physical packet이 여러
  file/ordinal을 담는다는 사실을 block 내용의 허용 범위로 사용하지 않는다. 서로 다른
  owner의 같은 term·다른 positions는 다른 block ref여야 한다. 완성된 block들의 물리
  put_batch는 기존처럼 파일 경계를 넘어 공유할 수 있다. 동일 bytes block alias는 허용한다.
- 하나의 decoder owner에서 term을 지정해 positions/count를 읽는다. key 정렬·고유성,
  position 형식·범위, 빠진/다른 term, 끝의 쓰레기 bytes를 전체 검증 후 결과를 노출한다.
- physical confirmation은 ref 단위, validated count는 `(ref, canonical stored term)`
  단위다. 같은 ref의 서로 다른 tf가 RAM/spill에서 섞이지 않아야 한다.
- bootstrap은 bounded block을 쓸 수 있다. 증분은 변하지 않은 term의 기존 block ref를
  유지하고 변경/new term만 새 leaf에 둔다. 한 term 변경으로 모든 posting ref를
  다시 쓰는 설계는 폐기한다. 삭제는 해당 mapping만 제거한다.
- block 크기·retained bytes·decoder scratch·query 재사용은 기존 memory/work grant로
  먼저 승인한다. object 최대 크기를 그대로 block 기본 크기로 쓰지 않는다.
- count뿐 아니라 BM25 tf, phrase의 term별 positions, sensitive/folded surface와
  old/current snapshot 검색이 기존 gold와 같아야 한다. 같은 block을 여러 query cursor가
  다시 읽어 bytes/CPU가 늘어나는지 확인한다. 필요한 cache는 query-local bounded
  authenticated DATA 재사용이며 전역 cache를 신설하지 않는다.
- Text policy/형식 계약과 old pinned snapshot 읽기를 Root가 함께 고정한다. 기존 leaf와
  새 표현은 단일 lexical 읽기 owner가 다루며 silent fallback이나 generation 혼동은 금지한다.
- 동일 physical ref를 여러 map entry가 참조하는 GC edge/order/중복을 보존한다.
  마지막 참조·pin이 없어지기 전 block을 회수하지 않는다.

코드 접점은 `text_occurrence.rs::OccurrenceReader`의 codec/selector,
`text.rs::{OccurrenceWrites,prepare_inventory,validate_posting_hit}`,
`ConfirmedOccurrences`와 `text_spool.rs`의 확인/count 키다. 단일 현재 정책의
정확한 leaf/block wire와 bounds는 Root가 합류 전에 고정한다.

폐기 조건도 고정한다. 기존 position leaf의 term 간 exact dedup을 잃어 실제 신규 파일
수가 줄지 않거나, 원래 고정된 query work/memory/latency·pin 유지 한도 또는
region-local delta bound를 위반하면 채택하지 않는다. Cold read bytes의 증가량도
측정·보고하되 그 자체를 universal zero-amplification 위반으로 판정하지 않는다. 기존 query/memory/work 한도를 유지하고 새 임의 성능 목표를 만들지 않는다.
별도 ANN 원4K의1M reads/512MiB·SDK30s/worker10s와 Large4,096-source120s의
workload/caps를 혼합하거나 한쪽을 완화하지 않는다.

### B03-B 후속 계약 검토 — 원 기준 정정

- RFC12.1의 metric별 선고정 region bounds와12.3/12.5의 고정 query mix/SLO가 authority다.
  모든 cold query의 bytes가1B라도 늘면 폐기하는 조건은 이 문서가 추가한 과도한 기준이었다.
  이를 원 제품 gate 완화로 표현하지 않는다. 새 후보 측정 전에 기준을 고정한다.
- 후보는 owner 내부 term→positions-vector dictionary이며 서로 같은 positions는 같은
  vector slot을 사용한다. 제안 고정 크기는 payload4,096B/128 term rows, 벡터별 기존
 8,192 positions 상한이다. 폭을 N에 따라 키우지 않는다. 단일/oversize/delta는 기존
  leaf를 사용하고 capacity-full과 실제 memory/work/cancel 거절을 다른 결과로 구분한다.
- 새 policy는 허용된 leaf/block variant와 숫자 한도를 명시한다. 구 policy member는
  기존 typed refusal을 유지하며 startup rewrite/drop이나 silent fallback은 없다.
  미배포 구 codec의 영구 reader는 요구되지 않는다. 실제 구 refs/pins이 남아 있는
  전환은 별도 compatible executable/artifacts 유지 또는 cutover 거절 계약을 따른다.
  새 policy 내부의 old/current generation·reopen·pin은 반드시 보존한다.
- count 재사용은 `(ref,term)`으로 선택한 완전 인증 DATA여야 한다. ref-only scalar memo는
  leaf에만 유지하고 block에는 완전 검증된 bounded layout/cache를 사용한다. physical
  confirm 자체는 ref 단위다. query-local cache는 실행 범위를 벗어나지 않는다.
- 우선 기존 r4 pair의 실제 work delta를 확인한다. 위 wire/cache ABI를 Root가 확정한
  뒤 codec/producer/독립 oracle의 파일을 분리해 구현한다. 중간 결과를 제품 개선으로
  승격하지 않는다. 일반 Storage segment backend나 기존 inventory Block을 재구현하지 않는다.

### B03-B1 — codec·selector ABI 고정

원 Large Off(lo4)는0a97e858에서도 BuildPrepare deadline으로 실패했다. 같은 pair의
Large Record(lr4)도 원120초 deadline에서 실패했으며 새4축 계수를 수집했다. 세 개의 작은 수리만으로120초를
충족하지 못했으므로 다음 구조 후보의 producer/reader/독립 oracle를 파일별로 분리한다.
원 query/memory/work/SLO·old/current pins·delta bounds가 채택 조건이다.

새 current policy의 leaf/block variant이며 구 policy 자동 재해석은 없다. Block은 한
owner의 inventory 안에서만 구성한다. 같은 positions 배열은 block 내부 dictionary의
한 vector를 공유한다. JSON leaf는 singleton/oversize/new changed term용으로 유지한다.

고정 binary: `QITXOC01` 8B magic, BE u16 term_count/u16 vector_count/u32 reserved0
(총16B header), 정렬된 row들의 `(BE u16 key_len,key,BE u16 vector_index)`, 그 뒤
vector들의 `(BE u16 count,count×BE u32 position)`이다. payload≤4096B, term rows2..128,
vectors1..term_count, 각 positions1..8192·strict increasing. vector first-use ordinal이
0..n 순서이고 미사용/중복 vector·중복/역순 key·bad selector·trailing bytes는 거절한다.

private ABI(코드 완성 전 임의 success/stub body 금지):

- `text_occurrence::block::{MAX_BLOCK_BYTES=4096,MAX_BLOCK_TERMS=128,LAYOUT_POLICY}`.
- `BlockDraft::new()->Result<Self,IndexError>`, `len()->usize`,
  `try_push(PreparedKey,PreparedPositions,&WorkBudget)->Result<BlockPush,IndexError>`.
  `BlockPush::{Added,Full{term,positions}}`; Full은 원 소유 입력을 반환하며 budget/cancel
  거절을 Full로 바꾸지 않는다. Dictionary는 동일 positions의 보관을 한 번만 한다.
- `BlockDraft::finish(self,&WorkBudget)->Result<SealedBlock,IndexError>`.
  `SealedBlock::{Leaf{term,positions},Packed{payload:StagedPayload,terms:PreparedRows<BlockTerm>}}`,
  `BlockTerm{key:PreparedKey,vector:u16}`. empty는 거절하고 single은 Leaf로 반환한다.
- `CheckedBlock::parse(StoredObject,&WorkBudget)->Result<Self,IndexError>`가 full preflight한다.
  `object(&self)->&StoredObject`, `selection(&self,term:&[u8],&WorkBudget)->Result<Range<usize>,IndexError>`는
  소유 payload의 선택 vector BE u32 범위만 반환한다. 원 authenticated StoredObject와
  DATA lease를 layout이 소유해 외부 payload 혼합 및 term당 SHA 재계산을 없앤다.
- 전체 wire/row/dictionary/layout allocation을 원 memory scope에서 before-birth 승인한다.
  Producer는 완성된 다른 block들을 기존 put_batch에서 함께 설치한다. same-ref confirm은
  physical ref 단위, term별 count는 완전 검증된 block layout으로 분리한다.

파일 one-writer: codec은 새 `text_occurrence/block.rs`, producer는 새
`text/occurrence_pack.rs`, 독립 codec oracle는 새 `text_occurrence/block/tests.rs`.
Root만 `text.rs`, `text_occurrence.rs`, `text_memory.rs`의 mount/policy/cache/caller 접점을
수정한다. 기존 inventory Block/P1-A·Core/Storage/Catalog/SDK를 이 패킷에서 재구현하지
않는다. Worker Cargo0, Root 정적 합류 뒤 영향 검증을 한 큐에서 수행한다.

### B03-B1 실제 합류와 검증 큐

f9 frozen source에서는 bootstrap caller만 `OccurrenceWrites::pack_owners`를 선택하고
empty prior에서 `occurrence_pack::prepare_owner_blocks`를 호출한다. 그러나 실제
Large harness의 첫 profile은 step995, 첫 SourceAdvance는 step1059의 Patch다.
Bootstrap-only 조건으로는 원 Large full 경로에서 B1을 켜지 못한다. 현재 후보
`d8e2c52d`는 `Bootstrap` 또는 `Patch(delta.prior.owners.0.is_none())`를 선택한다.
profile-first 초기 Patch의 변경 transition test는 f17에서 통과했다. Sparse의
nonempty prior leaf와 old-ref bound는 보존한다. 이후 Patch의 변경 term은 leaf다.
Delta는 prior positions를 term-aware reader로 인증 비교하고 같으면 old ref를 유지한다.
Query의 cache는 diagnostic observer와 독립이며 Off/Record의 제품 경로가 같다.

합류한 독립 oracle:

- `text_occurrence/block/tests.rs`: literal wire·distinct tf·dictionary dedup·malformed15경계·
  128 rows/4096B Full·cancel/memory denial·layout/output grant 환급.
- `text_occurrence/tests/reader.rs`: block selector별 count/positions·leaf no-cache·cap2 축출·
  wrong retained DATA/refusal/cancel·초기 foreign ref·DATA/layout/position lease 수명.
- `text/tests/occurrence_blocks.rs`: actual plan/prepare bootstrap→delta→old/current ticket와
  object-port 재생성 query, 1018-position oversize leaf,132 canonical rows→2physical blocks,
  후속 설치 output mismatch에서 map publish0.
- Kernel `native_lifecycle/occurrence_blocks.rs`: 실제 FileObjectStore/SqliteCatalog에서
  source 먼저→Reconfigure bootstrap, delta의 block+leaf 공존·tf/positions/phrase,
  old/current exact pins·재오픈과 마지막 pin/history/custody 해제 후 occurrence block reclaim.

포맷·diff 공백·hexagonal 검사는 VERIFIED다. 독립 소스 감사에서 확정 P0/P1은 찾지
못했으나 이것은 실행 성공이 아니다. Native 범위 테스트의 첫 admission은300초 뒤
exit124/body0이었다(NOT_RUN). 같은 controller가 끝난 것을 확인한 뒤 wait7200인
단일 successor를 공용 큐에 넣었다. 첫 successor는17분42.653초 후 입장해48.192초
컴파일했고, 새 oracle의 unused-results5건에서 FAILED(exit101)로 끝났다. 테스트 본문은0이다.
Splice의 즉시 drop과 atomic 이전값의 명시적 소비로5건을 수정했다. 그 successor는
f5/f6/f7 고정본 검증으로 이어졌으며 결과는 아래 이력에 기록한다. 가드를 우회하거나
worker Cargo를 병렬 실행하지 않는다.

```sh
QUANTA_INDEX_RESOURCE_WAIT_SECONDS=7200 ./scripts/cargow --lane test-workspace-lane \
  nextest run -p quanta-index-lexical --lib --locked --offline \
  -E 'test(native::)' --test-threads 2
```

위 admission·재큐 기록은 과거 f7 이전의 경과다. 현재 후보는 f15 Native292개와
기존 Kernel lifecycle12개, f17 변경 transition1개의 composite local proof가
VERIFIED다. f16 Native 소유 Clippy 진단0이지만 전체 lexical Clippy는 다른 owner
33개 진단으로 FAILED다. 새 정상 cached pair build는 exit0이고 mr5 Medium 진단
body는 fixed bytes cap에서 FAILED다. lo5 Large Off도 원120초 deadline에서 FAILED다.

첫 독립 source 대조에서 다음 candidate에 필요한 파일은12개였다: lexical 기존5개
(`text/text_occurrence/text_memory/text_query/text_reader.rs`), 신규5개(codec·producer와
독립 oracle3개), Kernel lifecycle parent mount와 새 oracle2개다. 기존 r4의16개 owned
입력 중 이번 수정 외 drift는 없고 text.rs만 B1으로 달라졌다. 전체 dirty tree를 복사하지
않는다. 최종 local snapshot은 Kernel `text_entry_maps.rs`를 더해13개를 복사했다.
외부 `candidate-build-03.py`는 terminal까지 완료됐다. normal cached build exit0,
같은 controller 환경의 mr5 Medium은 producer exit1/512MiB bytes cap FAILED다.
`pipeline_completed=true`는 controller 종료만 나타내며 제품 수용이 아니다.
영향 검증 VERIFIED manifest를 요구하고 동일 안정 s3 경로·r4 private tool aliases를
재사용한다. 새 build와 Medium을 한 controller의 같은 env에서 실행해 r4 PATH drift의
재발을 막는다. normal cached/prebuilt UNPROVEN과 qualified managed-fresh는 계속 구분한다.

추가 계약 대조에서 bootstrap vs delta의 raw artifact/logical_commitment 동일성은
요구되지 않음을 확인했다. RFC의 law는 같은 SourceCut의 membership/postings/statistics·
exact query/ranking 동등성이다. Kernel oracle는 같은 ticket에 기존 incremental instance와
새 bootstrap instance를 함께 두고 input root/policy·stats·TF/positions·부재 term·keyword/
phrase BM25 matched/hits를 비교한다. 이 동등성 oracle의 두 source-order case는
f15 Kernel 집중 범위에서 통과했다. 정확 d8e full Kernel13 동시 실행과 새 a608
source의 검증은 각각 별개다.

공개 `with_inventory_reuse_limit(0)`의 “Zero disables reuse”를 B1에서도 보존한다.
처음 합류에서는 block cache가 무조건 켜지는 P2를 정적으로 발견해 수정했다.
Confirmed/prior preparation의 zero 경로는 uncached reader를 선택한다. 다음 호출 전
current parsed DATA를 지우고 원 get/authentication/charge를 다시 수행한다. 파생 count
memo도 제공하지 않는다. `new(0)`의 invalid-window 거절은 그대로이며 uncached는
내부의 명시적 physical policy다. default/query window 숫자와 output_policy는 불변이다.

실제 소스 동결 검증 이력:

- Main의 재큐가 helper 리팩터 중에 입장해 intermediate method E0599에서 실패했다.
  테스트0이며 Root가 편집과 검증을 직렬화하지 못한 실행 오류다. 완성본 제품 결함으로
  승격하지 않는다. 이후 mutable main 대신 안정 s3 경로의 clean commit에서 검증한다.
- `cc6b1e1`/f5는 정상 컴파일 후 Native292개 중110개를 실행했다.109pass/1fail,
  fail-fast로182개 미실행·558개 scope 제외다. 실패는 새 codec의 row key end가 뒤의
  2B vector selector까지 포함한 결함이다. key bytes를 읽은 직후 end를 고정해 수리했다.
- `36a883a1e44274524803c736ed2128a8f06e1800`/f6는 같은12개 소유 파일을 다시
  고정했고 Native 전체 선택 범위에 `--no-fail-fast`를 사용했다. 이전 f5 실패는 보존한다.

이전 동결본 `642f74ac35487623bb3c1d686074367e1a98ddb8`/f7의 이력이다.
f6는 Native 전체292개 중291pass/1fail이었다. 남은 오류는 새 test port의
RegisteredObject.bytes가 payload 길이만 반환해 기존 inventory Block의 wire 길이
검사에 거절된 fixture 결함이다. 16B header+32B×children+payload와 object-v1
hash domain의 실제 framing/identity를 적용했다. 제품 검증의 wire guard는 유지했다.

f7의 동일 Native scope는 VERIFIED:292pass/558 scope 제외, body3.881초다.
codec literal gold·reader7개·bootstrap/delta/fault4개·기존 Native 영향 범위를 포함한다.
source/tool 전후 검사를 통과했다. f7 Kernel은 stale generic fixture로 FAILED했고,
같은 source의 f8은9pass/3fail이었다. f8 실패는 current MapBlock fault target,
Inline fixture의 cap35>32 강제, Shared64 batch 내 pending dedup reserve oracle의
세 결손이었다. 기대값과 fixture를 실제 계약에 맞게 수리한
`4b06b203e636b2e303652a3f55429b506c2fcd01`/f9에서 Kernel12pass/65 scope 제외,
125.827초, source/tool guard0으로 VERIFIED다. 이 결과에는 기존 source-before-profile
Bootstrap lifecycle이 포함되며 실제 harness의 profile-first Patch는 포함되지 않는다.

f10 strict Clippy는 같은 f9 source에서6개 lib 지적으로 FAILED였다. B1 소유2개인
`block.rs`의 `single_match_else`와 `text_occurrence.rs`의 긴 doc 지적은
공유 Main에서 수정됐다. f16의 lexical lib/tests `--keep-going`은40→33개
다른 owner 진단을 확인했고 Native 소유 진단0이다. 전체 Clippy는 FAILED다.

f11 Tools03의 old Record epoch는 producer body0으로 거절됐다. 15개 path에서
`st_dev`33→30만 달라지고 hash/inode/mtime 등은 같은 새 Tools03 invocation epoch를
고정했다. guard를 약화하지 않았다. f12는 입장 대기 prefix 뒤 PID/open FD가 없어
NOT_RUN으로 종료됐다. 새 epoch의 f13(source347)은 Native292pass, Kernel13 중
11pass/2fail이었다. raw JSON leaf 단정은 public reader TF/positions로 교체했다.
f15(`68ea46acdc952b9506ecded6d75f22b79cd421a7`)는 Native292pass, Kernel13 중
12pass/1fail이었다. 남은 같은-content 다른-ref-mapping에 block id 동일성을 기대한
oracle을 동일 ordered ref mapping의 Tree→Block roundtrip으로 교정했다.
physical map wire/ordered children·pin/GC 검증은 유지했다.

현재 후보 `d8e2c52de9f0e058475aa25e3d746cdad3015672`의 f17 변경 transition
test는1pass/77 scope 제외·6.337초다. d8e2↔68ea diff는 Kernel test 파일1개만이므로
f15 Native292+기존 lifecycle12+f17 변경 case1의 composite local proof는 VERIFIED다.
같은 exact commit의 full suite·제품 qualification이 아니다. `candidate-source-03.json`
focused gate는 VERIFIED이며 당시 source freeze에서 Root는 공유 Main HEAD/index를
수정하지 않았다. `candidate-build-03.py`의
normal cached r5 build exit0, 같은 controller env의 Medium body는
536,866,122B+14,133B에서 FAILED다. `r5/terminal.json`의 `pipeline_completed=true`
또는 controller exit0을 제품 성공으로 읽지 않는다. lo5 Large Off는 원120초
deadline FAILED다. `l8ctl/terminal.json`의 nested execution은 `lo4`를 잘못 읽어
145.286325초로 기록했으므로, lo5 직접 `execution.json`의226.898901초와 refusal을
사용한다. 이전 r4 Large120 실패도 유지한다.

seed42 Large 입력8,609,531B는 확인된 값이다. 현재 코드에서 약33,716 unique
leaf2.94MB를 12,288 block23.93MB로 바꾸는 수치는 **코드 기반 추정**이다.
실행 측정이 아니며 bytes 증가 위험이 있다. 원 region metric bounds·고정 E07
mixed query SLO·work/memory cap으로 채택을 판정한다. 객체 수 감소만으로 채택하지 않는다.

### 계약 합류 후 병렬 소유

동시에 선택한 수리는 하나다. 아래 lane은 최대3개의 source worker이며 heavy proof는
Root만 실행한다. 공용 hunk를 동시에 편집하거나 peer/다른 채팅에 직접 연락하지 않는다.

| Lane | A 선택 시 독점 범위 | B 선택 시 독점 범위 |
| --- | --- | --- |
| 생산자 | 선택된 실제 저장/검증 owner의 result 생성과 실패·정산 | `OccurrenceWrites/prepare_inventory`의 bounded packing과 unchanged 보존 |
| 소비자 | Native count/diff의 권위 소비·수명 | `text_occurrence.rs` term-aware decoder와 query/phrase 호출부 |
| 독립 oracle | count 권위 위조·손상·다른 attempt·실패 confirm·cancel/refund | codec gold, 두 term의 다른 tf, 단일-term delta/delete, phrase, snapshot/pin/GC와 fault |
| Root | 공통 port/type, Kernel/wire 연결, parent 등록, 최종 합류 | 공통 wire/policy, RAM/spill selector 키, shared `text.rs` 선언/parent, 최종 합류 |

Root의 shared `text.rs` 변경은 계약/선언을 먼저 합류하고 freeze한다. 이후 writer와
reader는 겹치지 않는 함수만 편집한다. 독립 test는 별도 파일, parent 등록은 Root다.
이미 소유한 다른 변경의 active 여부는 코드/현재 claim 근거 없이 추정하지 않는다.

정적 완료 조건은 실제 producer→caller→consumer, 남은 ref-only count 소비자,
recipe/decoder/old snapshot, memory owner와 error settlement를 전부 대조하는 것이다.
독립 oracle는 구현의 helper로 기대값을 다시 계산하지 않는다.

<a id="b04"></a>
## B04 — 합류·검증·제품 수용

### 현재 고정본과 실제 실행

외부 `/Users/songmin/.codex/qib6-_0r6mac7/s`의 clean Git source는
`51b70b3470ecbb58beebf757831eb53db85aade6`이다. 원 공유 HEAD `de16b193b`의
tracked/nonignored-untracked3,120개 regular file을 보존하고 필요한 Core 테스트
교정을 반영했다. 원본 inventory는 외부 sibling `source-manifest.json`이 소유한다.
당시 Root는 공유 HEAD/index를 변경하지 않고 외부 검증 저장소에만 snapshot commit을 만들었다.
고정 이후 공유 트리의 마지막 테스트 lint 표현식 변경은 제품 빌드에 포함되지 않는다.

다음 명령은 실제 시작했으나 성공 terminal/receipt 전에 중단됐다.
2026-11-06 KST 재확인에서 기존 tool handle89187이 없고, 해당 외부 root의 실행
프로세스와 build log를 연 FD도0이었다. 원 로그는01:10:57의 compile prefix에서 끝났고
종료코드는 알 수 없다. 성공이나 제품 성능 실패로 추정하지 않는다.
당시에는 Medium 진단 결과와 새 계수 분석이 없었다. 이후 정상 재개와 mr2의 실제 결과는 아래 최신 절을 따른다.

```sh
/Users/songmin/Documents/code-new/quanta-index/.venv/bin/python -B \
  -m tools.benchmark.retrieval.causal_cost_capture \
  --cwd /Users/songmin/.codex/qib6-_0r6mac7/s \
  --source-revision 51b70b3470ecbb58beebf757831eb53db85aade6 \
  --managed-build --backend index-native --tier medium --seed 42 \
  --native-trace-mode record --native-resource-sampling off \
  --trace-max-output-bytes 268435456 --disk-observation boundary-only \
  --max-seconds 300 --out-root /Users/songmin/.codex/qib6-_0r6mac7/m
```

위 명령의 cwd는 고정 source 디렉터리다. stdout/stderr와 fresh target은
`m/build/`에 보존 중이다. source/custody 검사가 실패하면 해당 실행을 실패로 남기며
guard나 제품120초를 완화하지 않는다. 완료 전 동일 managed build를 다시 시작하지 않는다.

### 중단 확인 후 캐시 재사용 재개

외부 source는51b70b34 clean 상태, Native 제품 접점108개 중107개가 공유 트리와
동일하고1개는 cfg(test) lint 표현식 차이다. 관련7crate 전체536개 Rust 파일을 대조한
추가3개 차이는 별도 File paged wire 변경이다. frozen 전체 crate를 최신 공유 전체
검증으로 취급하지 않는다. B03 Native 제품 코드와의 새 drift는 관측하지 못했다.

`resume-01.py`의 정상 재개는 Cargo 실행 전 공용 admission300초 대기에서 exit124로
끝났다. `r1/terminal.json`이 실제 종료를 기록하며 compiler/제품 본문은 시작하지 않았다.
이 controller가 종료됐음을 확인한 뒤 `/Users/songmin/.codex/qib6-_0r6mac7/resume-02.py`를
실제 시작했다. 입장 대기만7,200초로 바꿨으며 제품 dispatch120초나 work 한도,
capacity guard를 변경하거나 생략하지 않았다. 시작 PID71704/2026-11-06 01:29:39 KST다.

source/config/dependency/tool을 전후 검사하며 기존 `m/build/target`을 재사용하고,
동일 canonical cargow/NATIVE_BUILD_ARGS의 정상 bench pair를 완성한다.
별도 detached controller가 실제 자식 종료코드와 `r2/terminal.json`을 남긴다.
이 실행은 managed-fresh build receipt를 만들거나 이전 성공으로 가장하지 않는다.

실제 resume02는 admission758.451초 뒤 compiler에 입장했고39분39초 compile 후 exit0이다.
정상 binary pair는 `r2/normal-build-result.json`에 기록했다. 이후 mr2 Medium fixed Record를
실행했으나512MiB 누적 work bytes에서 실패해 controller가 종료됐다. `r2/terminal.json`의
ended UTC17:24:18.584와 실제 PID 부재를 확인했다. mr2의 CLI2는 producer exit1을 포함한
제품 실패이고, 성공 prebuilt에서도 남는 build_binding=UNPROVEN과 구분한다.

원 controller의 'Medium product/canonical 성공 뒤에만 Large' 조건은 과도한 실행 순서였다.
Medium fixed512MiB와 Large explicit512GiB는 다른 profile이다. mr2 실패를 유지하고
외부 `large-01.py`가 동일 source·pair·filtered environment로 `lo2`를 독립 실행한다.
PID28518/2026-11-06 02:34:43 KST 시작을 실제 command/start-time과 daemon 자식으로
확인했다. 원120초·67M/512GiB를 유지하며 새 build나 guard 우회는 없다.
근거는 외부 `l1/{live,terminal}.json`, `lo2/execution.json`에 남고 원본 mr2는 보존한다.
이 실행의 종료 결과가 나오기 전 제품 성공을 판정하지 않는다.

### 새 원 Large Off 결과와 단일 Record 진단

`l1/terminal.json`과 `lo2/execution.json`은 Large Off의 producer exit1을 기록한다.
outer timeout=false, source/pair 전후 동일, 원explicit67M/512GiB다. 실제 거절은
BuildPrepare의 deadline이며 work bytes4,715,789,654이고 numeric/memory/external
refusal이 아니다. 상세 수치는 [새 RCA](RCA.md#새-lo2-tracing-off에서도-원120초-실패-재현)를 따른다.

Off만으로 Text의 세부 실패 구간은 알 수 없어 계획한 Large Record1회를 실행했다.
외부 `large-02.py`/`l2/`가 동일 filtered environment·pair로 `lr2/`에 기록한다.
원120초와 work caps, sampling off, boundary-only를 유지하고 trace 출력만 Record로
변경했다. Record도174.409초·exit1/BuildPrepare deadline으로 종료했다. 새 build나 반복 Off는 없다. 이 실행은 진단이며 full-success 비교 쌍이
없으므로 elapsed 비율을 정식 tracing overhead로 바꾸지 않는다.

닫힌 child451,717 events를 기존 V5 parser로 읽어 health/loss/open/lease0을 확인했다.
실패 요청의 SourceApply24.865초와 Text94.512초, Prepare/Map/Diff 및 설치 phase 호출
귀속을 [RCA](RCA.md#lr2-오프라인-분석-실제-도달-경로와-다음-코드-범위)에 기록했다.
같은51b pair의 추가 Large 진단이나 fresh build는 필요하지 않다. 이후 cold query/delta
당시에는 B03-B를 중단하고 File hash/work delta를 수리했다. 이 block 중단 판단은
아래 원 기준 정정으로 대체한다. r3는 compiler 전 취소했고 r4 빌드·세 실패 실행은 완료했다.

다시 기다릴 때는 handle 파일이나 lock만 보고 live라고 하지 않는다. PID의 실제
command/start-time, 자식/FD와 terminal을 확인하며, 살아 있는 동일 controller를 재시작하지 않는다.

Root가 실행 큐 한 곳을 소유한다. 코드 작업자는 정적 대조와 좁은 소스 검사를 먼저
완료하고 build/product를 각각 시작하지 않는다.

1. 현재 producer→Kernel→Text→Storage→Catalog/Runtime→consumer의 실제 경로를 대조한다.
2. lane별 diff와 독립 oracle를 합류하고 affected signature/schema/경계를 정적으로 검사한다.
3. 영향 compile/strict와 소유 회귀를 한 배치로 검증한다.
4. 새 결함이 있으면 해당 source의 원인 hunk를 고치고 영향 범위를 다시 검증한다.
5. 현재 source와 정상 harness/daemon, workload/caps·관측 도구와 모드를 고정한다.
6. candidate Medium Record1회로 동일 입력·결과·물리 packet과 제거한 작업을 확인한다.
   정량 speedup을 주장할 때만 비교 가능한 호스트의 paired 실행을 별도로 수행한다.
7. candidate Large Off1회에서 원120초 full ACK/search/reopen과 후속 lifecycle을 판정한다.

경합 중의 명시적 진단 실행은 기능·관측 경로의 근거다.
causal 효과 비교에는 비교 가능한 호스트와 일치한 작업이 필요하다.
최신 코드가 바뀌지 않았다는 이유만으로 같은 Large 실패를 반복하지 않는다.
새 실행은 가설·변경·측정 입력 중 무엇을 확인하는지 미리 명시한다.
과거 C16/C17 source cut·trace·외부 실행기의 경로는 현재 없다. 이번51b70b34는
별도의 새 cohort다. 새 실행의 보존 위치와 수명을 미리 정하고, 정리 가능한 캐시
하나만 원본 참조로 남기지 않는다. 새 cohort 결과를 과거 source의 replay로 부르지 않는다.

### 현재 후속 코드 고정본

File get의 이중 hash 제거·SourceApply/Text4축 work delta는 영향741개와 Kernel lib
check·hexagonal·module 검사를 통과했다. Catalog registration 재인코딩 제거는
집중37개를 통과했다. 강화 SourceApply integration body는 아직 NOT_RUN이다.

- r3/86e6130e는 Catalog 수리를 묶기 위해 compiler 시작 전에 취소했다(exit143).
  제품 본문0이며 성능 실패가 아니다. 당시 PID50125의 대기 기록은 과거 상태다.
- r4의 clean cut은 `s3` / `0a97e8589daa41b736cb6d59545b0116bb3b89c6`이다.
  candidate-source-02의16개 입력을 보존하고 정상 cached pair build를 완료했다.
  `r4/terminal.json`은 Catalog0/build0/pipeline_completed=true다. 당시 Root는 공유 HEAD/index를 수정하지 않았다.
- mr4는12.356839초 producer1, fixed512MiB 초과다. used536865857+10600B,
  reads963417/writes588655/mutations401727. child56,184 events·loss/health/open/lease0.
  Lexical532359833B, Prepare261371848B, Map148640926B, Diff29935047B,
  Postings70301512B는 inclusive 계수다. 단계별 bytes를 합산하지 않는다.
- lo4는145.286325초 producer1, 원 dispatch120 deadline의 BuildPrepare 실패다.
  lr4는146.810698초 producer1, 요청120.152688초 deadline이며 Postings 미도달이다.
  lr4 child637,289 events의 손실/health/open/lease0이며 각 selected stage의4계수가 있다.
  상세 시간/계수·physical IO·실패 checkpoint는 RCA가 소유한다.
- 최초 m4ctl은 build/runtime PATH 차이로 producer 전에 거절했다(NOT_RUN).
  m5ctl은 PATH-only drift를 명시하고 private executable epoch를 재검사한 별도 runtime
  cohort다. lo4/lr4는 이 runtime env와 같다. prebuilt build_binding=UNPROVEN이며
  matched-build/quiet-host speedup·overhead qualification을 주장하지 않는다.

current-policy block의 profile-first Patch 활성화와 해당 Kernel oracle은
`d8e2c52d` local snapshot에 합류했다. r5 normal cached build는 exit0이고
mr5 Medium fixed512MiB는 선택한 진단 bytes cap에서 FAILED다. mr5 trace에서 physical
managed reserve input5,230→1,635, inventory occurrence validation3,083→512로
감소했지만 Prepare logical bytes는261,371,848→321,302,214로 증가했다. 이 비용을
charge site/데이터 단위로 귀속했다. `admit_inventory_confirmation`은 인증된 planned
행에도 `p/n` 각각 generic max195B를 사전 charge했다. 현재 행은 `p` Null/Object,
`n` Object로 검증되므로 `key+(1|33)+33+72`로 바꾸고 read1×/PK move2×에 적용했다.
행당 예상 감소는 prior Null 1,068B, prior Object 972B다. 예산·scratch reserve·
하드 SQLite page cap·typed corruption 및 post-install ref/count 재검증은 유지한다.
실제 총 행 수와 Postings 남은 작업이 확인되지 않아 Medium 통과·CPU 개선은 미입증이다.
같은 pair의 lo5 원 Large
Off는 `BuildPrepare` deadline에서 FAILED다: request124.435900초, producer226.898901초,
accepted10,550,723R/8,042,018W/8,296,724,947B/4,545,263M; numeric/memory/
external-resource 거절은 없다. l6ctl PATH-only
environment guard와 l7ctl tool mount-device guard는 body0/NOT_RUN이다. l8ctl은
새 tool epoch를 검사했고 r5 env와 PATH-only drift를 명시한 local runtime cohort다.
`l8ctl/terminal.json`의 nested execution은 구 `lo4` 경로를 읽은 copy bug이므로
lo5 직접 `execution.json`·refusal로 판정하고 원본 요약을 덮어쓰지 않는다.
같은 r5 pair의 lr5 Record도 request120.654초 deadline FAILED다. child V5는
858,005 events written/4,698 dropped/7 open으로 PARTIAL이며 닫힌 prefix의
LexicalBuild103.728초/CPU42.419초, Prepare41.232초/CPU14.563초,
Map31.917초/CPU14.817초, Diff11.428초/CPU7.386초다. 1,280 owners가
남아 Postings 완료에 도달하지 않았다. nested duration을 합산하거나 r4/Off와
속도·tracing overhead 비율을 내지 않는다. 상세 계수와 물리 I/O는 RCA가 소유한다.
s3는 해당 실행 중 변경하지 않았다. r4 실행이 모두 끝난 뒤 동일 source build 경로를 유지하고
기존 Git commit·pair를 보존해 매번 vendor PackageID를 바꾸는45분 재컴파일를 피한다.
변경된 source/config/deps/tool의 전후 binding은 다시 검사한다.

typed-fee 구현은 `text_spool.rs`와 독립 `planned_inventory.rs` 테스트를 추가해
소유 입력15개의 `c49b2a1fa0e3dc30d88e6189ef0cc0ddbac25bfa`에 고정했다.
literal Object wire33B와 Null/Object20,884B, Object/Object20,980B의 exact fee 및
exact−1 거절 시 planned row 비노출을 원 gold로 둔다. f18은 새 테스트 표현식의
임시값 참조 수명 E0716으로 compile FAILED/body0이다. encoded 값을 지역 변수에
묶은 test-only 수정의 새 frozen source는
`a6083fb22480786d5e8172057b6fbe291af26ee7`이다. f19 terminal에서 Native293개
통과/558 제외와 Kernel lifecycle13개 통과/65 제외, 두 exit0을 확인했다.
이 source의 영향 focused proof는 VERIFIED다. r6 normal cached pair build는 exit0이고
mr6 fixed512MiB Medium Record는 FAILED다. 원120초 Large Off는 NOT_RUN이다.
큰 비용 회귀가 실패하면 해당 charge/custody 원인만 수정하고 영향 범위를 다시 검증한다.

mr6 child trace는45,923 events complete/health·loss·open0이다. InventoryPrepare
accepted bytes는 mr5의321,302,214에서266,699,646으로54,602,568 감소했고,
이는51,126 planned Null/Object 행×1,068B와 정확히 일치한다. typed fee 수리의
WorkBudget 효과는 VERIFIED지만 Postings가 더 진행한 뒤 Catalog `managed_install.rs:108`
charge 10,692B가 fixed512MiB 진단 cap에서 거절됐다. 2,403 physical write/3,946,025B,
물리 I/O 실패0이다. source별 시간 차이를 CPU 개선 또는 제품 성공으로 승격하지 않는다.

B03-B2의 Kernel 독립 oracle는 기존 `occurrence_blocks.rs` realstore harness를 재사용해
profile-first 초기 Patch의130개 ASCII lowercase term/260개 canonical selector가
하나의 block ref로 저장되는지, public reader의 첫 Sensitive·마지막 Folded TF/positions,
마지막 term 변경 뒤 old/current pin·재오픈을 검증하도록 추가한다. 기존 같은 파일의
두 source-order lifecycle case가 last-pin GC를 검증하므로 GC harness를 복제하지 않는다.
이 oracle와 codec/producer lane의 8KiB/512-term 정책은 frozen source
`878be6c9…`에 합류했다. f20 Native295개는 통과했지만 Kernel은 helper E0507로
compiler FAILED/body0이다. Main의 교정은 contribution edge 대신 인증된
EntryMapBlock의 260개 positional row/ref를 독립 golden과 대조한다. 새 frozen
`a60ac657…`의 f21은 Native 변경 case1개·Kernel14개를 통과했다. f20/f21은 제품 코드가
같고 테스트2파일만 다르며 합친 결과는 focused composite proof다.

Root 통합 상세: `OccurrenceReader`의 build2/query64 bounded block DATA window,
`read_retained` reply의 registration+payload 동일성, exact `(ref,term)` 선택을 연결한다.
완전 검증된 confirmed block의 파생 count 재사용은 leaf scalar와 같은 DATA memo이며
ref-only scalar/spill에 block count를 쓰지 않는다. prior/query의 cache hit는 원 port의
retained-read charge/cancel 경계를 그대로 지난다. 진단 count의 full-validation 실패와
retained-read 거절을 혼동하지 않는다(후자는 span error로 관측).

B1 시점 policy gold(admission `[31;32]`)는
`eef15862a20af11bf9e7173747df2e155897d75913475bc5eb4a2ee856bdf33e`였다.
B2 현재 policy는 위 `b2c1df18…`이고, 직전 B1·leaf-only 두 policy는
validate_instance의 typed refusal oracle로 남긴다. 새-policy의 이전/current
generation·pin/reopen은 current leaf/block reader로 검증한다.

### 실행 수와 완료 기준

| 실행 | 횟수/목적 | 조건 |
| --- | --- | --- |
| r4 Medium Record | 완료 FAILED | fixed512MiB·실제4축/6계수 prefix 확인 |
| r4 Large Off | 완료 FAILED | 원120초/4,096-source 제품 기준선 |
| r4 Large Record | 완료 FAILED | Large Prepare/Map의4축과 CPU 확인 |
| B1 소유 회귀·정상 cached pair build | focused VERIFIED, build exit0 | 정확 source의 전체 qualification은 별개 |
| mr5 Medium Record | 완료 FAILED | 실제 full ACK 전 accepted536,866,122B +14,133B 거절. Prepare logical bytes 증가 귀속 필요 |
| lo5 Large Off | 완료 FAILED | r5 pair·원120초/cap, `BuildPrepare` deadline, full ACK 없음. PATH-only 별도 runtime cohort; controller 요약 copy bug 구분 |
| lr5 Large Record | 완료 FAILED | r5 pair·원120초 `BuildPrepare` deadline, child trace PARTIAL(4,698 dropped/7 open). 전체 capture·overhead 자격 없음 |
| typed-fee c49/f18 | compile FAILED/body0 | E0716 test-only 임시값 수명. 제품 코드·gold는 그대로 새 a608 source에 고정 |
| typed-fee a608/f19 | 완료 VERIFIED | exact source Native293/293·Kernel lifecycle13/13 통과, 558/65 제외. 제품 결과로 승격 금지 |
| typed-fee r6 pair build | VERIFIED | `a6083fb…` normal cached build exit0; managed-fresh 아님 |
| typed-fee mr6 Medium | FAILED | 536,864,032B+10,692B Catalog 설치 사전 charge, fixed512MiB 진단 유지. Prepare fee −54,602,568B 확인 |
| typed-fee a608 Large | NOT_RUN | 기존 d8e Large 실패를 새 source 결과로 승격하지 않음 |
| B03-B2 f20 Native | VERIFIED | `878be6c9…`에서295개 통과·558개 제외. 제품 결과 아님 |
| B03-B2 f20 Kernel | FAILED | helper E0507 컴파일 실패/body0. 새 source에서 교정 |
| B03-B2 f21 | VERIFIED | `a60ac657…` Native 변경 case1개 통과·853개 제외, Kernel14개 통과·65개 제외. f20 Native295개와 composite 집중 proof, 전체 exact suite 아님 |
| B03-B2 r7 build | VERIFIED | `a60ac657…` 정상 cached build exit0·l10ctl source/env/tool 일치. managed-fresh 아님 |
| B03-B2 mr7 Medium | FAILED | 536,869,205B+12,164B Catalog 설치 사전 charge 거절, fixed512MiB 진단 유지. I/O 오류0 |
| B03-B2 lo7 Large Off | FAILED | 요청124.194초 Catalog 설치 deadline, producer157.629초·full ACK 없음. 원120초 유지 |
| B03-B3 f22 QITXEM02 집중 | VERIFIED | exact `39f4a9c3…` Native297개·Kernel14개 통과, 각558/65 제외·terminal exit0. 제품 증거 아님 |
| B03-B3 f23 strict Clippy | FAILED | `39f4a9c3…` exit101·전역38개=기존 타 owner33개+Native 새 test helper5개 |
| B03-B3 f24 strict Clippy | FAILED | `99623043…` testhelper1파일 교정. exit101 전역33개 모두 타 owner, Native 진단0 |
| B03-B3 f25 변경 case | VERIFIED | `99623043…` Native1개 통과·853개 제외·source guard0; f22와 composite, Native298로 합산 금지 |
| B03-B3 r8 build | VERIFIED | `99623043…` 정상 cached build exit0; managed-fresh 아님 |
| B03-B3 mr8 Medium | FAILED | 536,864,010B+9,988B Catalog 설치 charge 거절. child49,370 events complete, physical2364 writes/2,256,813B·I/O 오류0 |
| B03-B3 lo8 Large Off | FAILED | `99623043…` producer159.174초·full ACK124.654초 미완료. `BuildPrepare` Storage map/completed_group.rs:140 deadline, numeric/memory/external 거절 없음 |
| B03-B3 lr8 Large Record | FAILED | 같은 pair producer185.890초·full ACK125.721초 미완료. `BuildPrepare` 같은 Storage checkpoint deadline, child841,911 events complete/loss0, 물리31,630 writes/43,770,402B·I/O 오류0 |
| local 저경합 replay | NOT_RUN | 조건: 연속3개 10초 구간 load1<8·swap churn<64MiB. 180초/19표본은 load1 18.04–76.20·swap churn10초당 최대6.564GB로 불충족. 동일 상태 재실행 생략 |
| 정식 Off/Record overhead | 상위 E07의3쌍·동일 작업·비교 가능 호스트 | 위 제품 진단의 성공으로 대체하지 않음. host 조건 부재면 BLOCKED 유지 |

현재 잔여는 원 Large120 full ACK·후속 검색/reopen의 실제 통과, 비교 가능한 호스트의
성능/overhead 관측, 최신 공유 Main 전체 통합·release 수용이다. 공유 Main HEAD
`7a16f48a20e2af2a37ed16847509bf6208dc8a6b`는 다른 작업의 문서 커밋이며 이 작업의
Root는 Main을 commit/add하지 않았다. 소유 입력19개 SHA는 frozen `99623043…`와
정확히 일치(차이0)하지만 Main 소유 파일9개는 modified다. 집중·r8 증거를 최신 전체
Main green으로 승격하지 않는다. 소유 범위 정적 감사에서 바로 수정할 P0–P2 코드
결함은 확인되지 않았으며, 이는 Large120 성능 실패가 해소됐다는 뜻이 아니다.

Large Off는 server dispatch120초, seed42, reads/writes/mutations 각각67,108,864,
bytes549,755,813,888을 유지한다. outer watchdog600초는 server 예산을 연장하지 않는다.
prebuilt 관측은 현재 검증한 pair 경로와 실제 SHA를 CLI에 전달하며, 원 build receipt와
source inventory의 일치 여부를 Root가 함께 대조한다. CLI의 UNPROVEN/partial 상태와
제품 lifecycle 자체의 VERIFIED/FAILED를 각각 기록한다.

완료는 다음 모두를 요구한다. Full ACK만 성공하면 끝낸 것으로 판정하지 않는다.

- 전체 SourceAdvance ACK가 원 server dispatch120초 안에 완료한다. 그 뒤 current/old
  검색, delta/noop/delete, drain/reopen과 기존 결과/점수·phrase gold를 별도로 검증한다.
- B03 패치를 선택했다면 해당 비용 단위가 실제로 제거됐고 기존 durability·memory/work·취소·fault
  거절을 보존한다. B03-B이면 증분 locality와 원 query bytes/work/latency bounds를 충족하고
  실제 read amplification을 보고한다.
- 변경한 producer→consumer 실제 UDS 경로와 독립 oracle가 nonzero로 실행된다.
- strict의 이번 소유 지적은 수정·재검증하고, 다른 변경의 전체 gate 실패는 그대로 남긴다.
  이 상태에서 workspace/release qualification을 완료로 승격하지 않는다.
- 정식 tracing overhead·원 ANN4K·XL/mixed·운영/자원·release의 기존 상위 미수용은
  해당 owner/고정 profile의 증거로만 닫는다. Native Large 통과로 함께 닫지 않는다.

검증 실행 순서는 반드시 정적/좁은 회귀를 먼저 하고 긴 normal pair build를 나중에 둔다.
긴 build가 admission을 가진 동안 대기 제한이 짧은 다른 Cargo를 새로 넣지 않는다.

## 필요한 독립 검증

| Scope | 최소 oracle |
| --- | --- |
| 계수 | 신규·prior·중복·RAM/spill·실패를 독립 입력으로 계산, 관측 없음과0을 구분 |
| 저장/검증 | 잘못된 identity·형식·count, confirm 실패, 다른 attempt, 잘못된 backend 결과 거절 |
| 수명 | memory/work 원 한도, retained release, cancel/drop/retry, 부분 성공 settlement |
| Off/Record | Off의 관측 작업 부재와 동일 제품 결과, complete footer·loss/lease/open0 |
| 실제 경로 | UDS 요청에서 현재 canonical packet과 public port/wrapper를 실제로 거침 |
| 성능 | 각 arm의 source/binary 결속, 같은 입력·작업량, 원 deadline, 완료 ACK/search/reopen |

기존74개 fresh-claim·487개 Python 회귀 등은 해당 고정 소스의 근거로 재사용한다.
변경하지 않은 전체 workspace와 별도 플랫폼을 매 패킷마다 실행하지 않는다.
전체 release·Linux 운영·XL/mixed와 정식≤5% overhead는 상위 E07/E08 분모를 유지한다.
