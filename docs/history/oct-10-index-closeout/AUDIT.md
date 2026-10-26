> HISTORICAL ONLY. 당시 소스의 설계·실행 기록이며 현재 작업 지시가 아니다. 현재 작업·상태·담당은 [단일 SSOT](../../plans/oct-13-native-incremental-wave2/tickets/README.md), 실행 판정은 [VALIDATION](../../plans/oct-10-index-closeout/VALIDATION.md)을 따른다. 이 본문에 상태를 추가하지 않는다.

# 해결안 감사와 범위 결정

> 기술 계약/날짜별 설계 근거 참고 문서다. 현재 작업·담당·순서는 [Wave2 SSOT](../../plans/oct-13-native-incremental-wave2/tickets/README.md)만 갱신한다. 아래 과거 상태/packet/미구현 판정은 현재 배정표가 아니다. 실행 결과는 [VALIDATION](../../plans/oct-10-index-closeout/VALIDATION.md), legacy의 Accepted 결정은 기존 ADR을 따른다.


감사/범위 수정일: 2026-10-10 · source HEAD: `e073465e534f3529d47a43f84ceb1f19992953f1`.
정적 소스/설계 감사이며 구현·race 재현·Rust/daemon/성능 검증은 `NOT_RUN`이다.
원 legacy A/D/C5 구현 범위와 병렬 배정은 [README](../../plans/oct-10-index-closeout/README.md), oracle과 후속 실행은
[VALIDATION](../../plans/oct-10-index-closeout/VALIDATION.md)이 소유한다. 위 NOT_RUN은 이 감사 당시 실행 범위이며,
후속 A/D/C5 수용을 부정하지 않는다.

여기서 제외한 새 registry/journal·일반화된 lifetime은 원 결함 수리의 비필수 확장이라는
뜻이다. 후속 native 교체 범위의 요구를 금지하거나 취소한 결정이 아니다. 새 native 설계와
전체 티켓은 [Native RFC](../../rfc/oct-10-native-incremental-vnext.md#11-reading-order-and-decision-boundaries)가 소유한다.

Parallel execution refinement: [RFC §23.77](../../rfc/oct-10-native-incremental-vnext.md#2377-parallel-delivery-plan-and-exclusive-write-sets-2026-10-13).
Decoder, producer and physical boot are distinct first slots; shared Runtime
writer/recovery and Contract/config hunks have one integrator. Native BM25 exists;
only actual parity gaps merit repair. Source closure proof O(N) is a separate
producer cost obligation.

## 현재 잔여 작업에 대한 재감사 적용 (2026-10-13)

아래 A/D/C5 판단은 결정 당시 계약이며, 수용된 직접 수리를 다시 시작하는 지시가 아니다.
현재 재감사·분업·다음 작업은 [Wave2 SSOT](../../plans/oct-13-native-incremental-wave2/tickets/README.md)를 따른다. [Native RFC 보완 순서](../../rfc/oct-10-native-incremental-vnext.md#remediation-plan-recheck-and-delivery-order-2026-10-13)는 설계 근거이며 별도 실행 큐가 아니다.
[RFC §23.55](../../rfc/oct-10-native-incremental-vnext.md#2355--current-source-final-action-register-2026-10-12)와
[legacy 계획 §10.37](../../plans/oct-10-index-closeout/LEXICAL-QUERY-PLAN.md#1037-최종-잔여-작업과-native-분업-2026-10-12)은 이전 잔여 목록이다.
native retirement composition과 결과/frame/provider custody는 구현·선정 검증 기록이
있으므로 미구현으로 재배정하지 않는다. 남은 범위는 adapter build metadata,
production aggregate disk/WAL/secondary-file 집행, default product 전환,
실제 producer 연결 및 운영/성능/출시 수용이다. 새로운 scheduler·journal·generic compactor를
추가해야 한다는 결론은 아니다. 다른 소스의 strict/focused PASS나 감사상 blocker 미발견은
현재 전체 계획의 P0/P1=0·release 수용을 뜻하지 않는다.

## 최종 판단

A의 누락 계약과 D의 pin/GC 경쟁은 기존 owner를 수리한다. C5는 영속 번호 예약과 원 event의
최종 body 연결만 추가한다. 이전 감사안의 예약 시 activation CAS 동결은 제거했다.

현재 [SDK](../../../crates/quanta-index-sdk/src/lexical.rs)의 `publish_outcome`과
`activate_published(evidence, expected_active)`는 이미 분리돼 있다. 번호를 발급할 때
active head까지 고정할 필요가 없다. 이를 고정하면 다른 activation/rollback 뒤 stale 예약을
종료하기 위한 Superseded·별도 eligibility·late receipt/retention 전이까지 필요해진다.
이는 기존 API가 요구한 수정이 아니라 이전 설계가 만든 추가 의존성이었다.

최소안은 예약에 CAS를 두지 않고 기존 activation 요청의 명시적 CAS와 generation/순서 제한을
유지한다. conflict 후 SDK 자동 recapture는 하지 않는다. 오래된 unresolved event가 후속 작업을
막는 기존 typed busy는 남으며 무조건 진행을 보장하지 않는다.

## 소스 근거

| 항목 | 관측 근거 | 이번 수정 |
| --- | --- | --- |
| A persisted policy | [generation_contract](../../../crates/quanta-index-semantic/src/generation_contract.rs), [manifest](../../../crates/quanta-index-semantic/src/manifest.rs)에서 policy/view-policy 누락 | 기존 모델 계약을 보존하고 reader/writer 함께 변경 |
| A 검사 순서 | [build_stream_reported](../../../crates/quanta-index-semantic/src/build.rs):1846 첫 window, :1855 target contract 검사 | target와 delta base의 순수 검사를 첫 window 전 수행 |
| D pin/GC | [store](../../../crates/quanta-index-repomap/src/store.rs)의 commit_bundle:501, acquire_pinned:934, gc_retired_objects:1040 | 최종 검사·설치·pin 등록·삭제를 기존 gate로 보호 |
| D false-positive | [panic test](../../../crates/quanta-index-repomap/tests/read_view_lifetime_owner_v1.rs):283 | G1 acquire 실패 대신 실제 active pin/body/unwind를 검사 |
| C5 allocator 부재 | [reserve_source_event](../../../crates/quanta-index-search-plane/src/readiness/activation_catalog/source_events.rs)는 caller가 선택한 target을 받음 | existing envelope에 high-water와 선발급 상태 추가 |
| C5 최종 binding | [SourceEventBindingV1](../../../crates/quanta-index-core/src/domains/source_publication.rs)는 최종 target/journal key 필수 | 번호 선발급 후 검증된 최종 digest/key에 1회 연결 |
| activation 분리 | [catalog activation](../../../crates/quanta-index-search-plane/src/readiness/activation_catalog.rs)의 activate_prepared_under_guard_v1과 SDK activate_published | 기존 explicit CAS 유지, reservation에 head 추가하지 않음 |

위 경쟁 구간은 정적 분석이다. 현재 view가 in-memory Arc를 사용하므로 이 사실만으로
query crash/데이터 손실을 재현했다고 주장하지 않는다. 기존 source-event의
Pending/Staged/Active와 unresolved 순서는 유지한다.

## 추가 소스 대조

- `payload_digest.rs::source_event_payload_sha256`는 materialization target을 제외한다.
  선발급은 이 source identity를 재사용하고 최종 body digest와 분리할 수 있다.
- `SemanticScopeStreamBuildPort`에는 현재 preflight가 없다. 새 검사를 이 port에 연결하면
  production impl과 test double 전체를 수정해야 하며, 기본 성공 구현은 수리가 아니다.
- `ingest_bundle_v2`는 `commit_bundle` 앞에서 replay object를 검증한다. acquire/GC 두 함수에만
  gate를 추가하면 이 replay 경쟁은 남으므로 publish의 최종 replay와 commit entry도 연결해야 한다.
- 현재 semantic inventory는 sealed와 quarantined를 반환하며 unfinished directory를 건너뛴다.
  이것만으로 신규 high-water=0을 초기화할 수 없다. 하위호환을 요구하지 않으므로 구형 format은
  rebuild refusal, 미추적 점유가 있는 신규 pair는 refusal로 제한하고 자동 migration을 추가하지 않는다.
- `ff0cdfe2..e073465e`는 benchmark 변경이며 A/D/C5의 위 source 경계는 변경되지 않았다.
  작업 계획은 현 HEAD로 갱신했지만 코드 수정이나 race 실행의 증거는 아니다.

## 제외한 확장

| 항목 | 제외 이유 |
| --- | --- |
| GC 후 object-free RepoMap publish receipt | 원래 pin/GC 수리와 별개 동작 확장. 정상 retirement와 quarantine/loss를 구분하는 새 custody 조회까지 필요 |
| catalog/envelope identity·주소 alias 추가 hardening | 독립 corruption 방어 변경. lifecycle gate의 직접 수리 완료 조건에서 분리 |
| C5 Superseded/이중 상태 축·rollback 연동 | 예약 시 frozen CAS를 제거하면 이 allocator의 필수 의존성이 아님 |
| root 전체 restore reconciliation·retention 정책 재설계 | 기존 incarnation 비교와 fail-closed/typed busy를 유지하고 자동 복구 보장을 늘리지 않음 |
| named-vector migration·RCU·새 registry/journal | 현재 단일 semantic lane·leased daemon의 직접 결함 수리에 불필요 |
| Loom/새 simulator/외부 fault platform | 기존 테스트와 deterministic barrier로 해당 경쟁을 검사 가능 |
| 일반 benchmark/release·장기 이력/자원/포맷 확대 | 직접 코드 수정의 수용 gate에서 제외. 별도 제품 요구와 실행 입력 필요 |

제외는 기존 동작의 안전성/완료 판정이 아니다. 현재 보존·오류 계약은 유지하고, 기능 확장과
새 운영 보장을 이번 구현에 포함하지 않는 결정이다.

## 1차 레퍼런스와 적용 한계

자료는 2026-10-10에 확인한 공개 1차 출처다. 설계 근거이며 Quanta의 성능 우위를 증명하지 않는다.

| 자료 / 시간 근거 | 이 범위에 적용한 원칙 |
| --- | --- |
| [Weaviate 1.24](https://weaviate.io/blog/weaviate-1-24-release), 2024-02-27 | 다중 모델은 독립 vector/index 기능. 현재 단일 lane은 compatibility 검사와 full replacement로 제한 |
| [RocksDB live SST](https://github.com/facebook/rocksdb/wiki/How-we-keep-track-of-live-SST-files), 표시 갱신일 2021-12-12 | reader reference가 살아 있는 객체는 reclaim하지 않음 |
| [RocksDB stale files](https://github.com/facebook/rocksdb/wiki/Delete-Stale-Files), 표시 갱신일 2021-05-05 | 생성 중/in-flight 객체도 기존 GC 보호 대상에 포함 |
| [AWS idempotent APIs](https://aws.amazon.com/builders-library/making-retries-safe-with-idempotent-APIs/), [공개 발표](https://aws.amazon.com/about-aws/whats-new/2021/01/new-abl-article-making-retries-safe-with-idempotent-APIs/) 2021-01-15 | stable event identity, parameter mismatch 거절, 원 결과 재시도 |
| [PostgreSQL 18 sequence](https://www.postgresql.org/docs/18/functions-sequence.html), [18 공개](https://www.postgresql.org/about/news/postgresql-18-released-3142/) 2025-09-25 | distinct 번호, gap 허용, 비재사용. 외부 응답 전 persist |
| [etcd v3.6 API guarantees](https://etcd.io/docs/v3.6/learning/api_guarantees/), 표시 갱신일 2025-09-26 | durable 완료와 timeout의 불확정을 구분. 단일 daemon에 Raft 추가 근거는 아님 |
| [etcd robustness testing](https://etcd.io/blog/2025/autonomus_testing_with_antithesis/), 2025-10-03 | deterministic schedule과 독립 oracle/negative control. 외부 플랫폼 도입 요구는 아님 |

## 검증 상태

- `VERIFIED`: 소스 검사 순서·현재 source-event record·SDK publish/activation 분리 정적 대조.
- 소스 기준 갱신 시 A/D/C5 관련 crate의 차이는 SDK activation adversarial 테스트뿐임을 확인했다.
- `NOT_RUN`: 위 코드 수정과 Rust/daemon/race 실행. 정적 검토를 실행 결과로 승격하지 않는다.

## 2026-10-12 — 다른 채팅 범위를 제외한 잔여 감사

기준은 HEAD `c32cd191554031b9a7d04207ffa3410e8fc201ba`와 공유 dirty source다.
다른 채팅이 진행 중인 Native vnext/query/pump, trace/profile, GC 삭제-sync 복구,
hard-limit durable checkpoint, 교체 가능성 RFC 및 commit/push는 이 감사의 수정 대상에서
제외한다. A/D/C5의 기존 수용을 최신 전체 source의 통합 수용으로 확대하지 않는다.

| 우선순위 | 현소스 근거 | 남은 해결 |
| --- | --- | --- |
| P1 — semantic rollback 수용 대기 | 이전 dataset 선삭제 경로를 제거하고 `dataset_promotion.rs`의 generation-local intent로 교체했다. old/new directory inode와 contract inode·길이·digest를 묶고, rollback은 새 dataset 격리→이전 pair 복원·sync→staged contract·격리 파일 정리 순서다. forward recovery도 새 pair sync 전에 backup을 삭제하지 않는다. contract bytes가 같아도 inode로 단계를 구분한다. | 독립 filesystem fixture 15개와 실제 build/crash 회귀의 컴파일·실행 수용이 남는다. 정적 독립 검토에서는 지정 crash matrix의 추가 정합성 blocker를 찾지 못했다. 기존 intent 없는 dataset+backup 상태는 삭제 없이 거절한다. maintenance/recovery 출력은 전체 resource ledger에 아직 미연결이다. |
| P1 — 원계획의 구현 공백 | 기본 Runtime의 `SearchdRuntimeParts.publication_admission`은 `None`. Core의 control/work 및 출력 하위 예약 계약과 materializer hook은 있지만 production estimator/inventory/shared ledger가 없다. semantic atomic sidecar에는 쓰기 전 예약·경로 binding·내구 완료·후속 rename/삭제 hook을 연결했다. Tantivy/LanceDB 본체, control/source/scratch 및 semantic maintenance/recovery에는 전체 회계가 없다. pool quota는 object encoded 파일에 한정된다. full-resource 설정을 무시한 실행 결함으로 판정한 것은 아니다. | 기존 Runtime authority에 publication 전체 자원 회계를 연결한다. opaque vendor writer도 출력 증가 전에 제한해야 하며 완료 후 크기 검사만으로 대체하지 않는다. pool 외 Tantivy/LanceDB·index/control/SQLite/staging과 retained capacity를 포함한다. 생성·교체·부분 실패·동일 inode/link 공유·동일 process 복구·재시작 inventory를 함께 처리한다. Native owner의 전역 scheduler를 별도로 복제하지 않는다. |
| P2 | root admission은 두 physical session을 보유한 채 단일 service-owned deadline과 main-file cap이 있는 SQLite로 inode를 측정한다. 전체 walk와 배타 잠금은 여전히 남는다. semantic promotion의 새 durability fence도 inherited dataset 전체 파일·디렉터리를 순회하며 `sync_all`한다. depth 제한은 있으나 파일 수·시간 제한은 없다. | root-wide 증분 회계로 hot publication의 전수 스캔을 줄인다. semantic은 immutable base의 내구 custody가 인증된 재사용 파일과 changed/new 파일을 구분해 fence 비용을 줄여야 한다. 단순 hardlink 수 또는 동일 path만으로 sync를 생략하지 않는다. 유한 예산·순회 envelope의 영향 회귀와 잠금 유지 시간·busy/retry 가용성을 검증한다. 60초 기본값은 운영 guardrail이며 측정된 수용값이 아니다. |
| P2 | pair session은 요청 안의 membership/union을 증분 계산하지만 새 요청의 첫 generation 인증과 전체 root scan은 남는다. 미봉인 writer cache가 남으면 physical session도 거절된다. | 첫 인증·root scan·재시도 비용을 분리하고 idle sweep/build/GC 사이 forward progress를 확인한다. 전체 publication이 O(delta)라는 주장은 아직 하지 않는다. |
| P2 | merge policy는 명시적 수치를 받으며 기본 비활성이다. last-sealed merge facts는 logical attribution이고 실제 I/O·RSS·임시 disk peak가 아니다. | 지원 workload에서 threshold·output reserve·backpressure를 보정하고 enabled 정책을 수용한다. 임의 cap 증가로 대체하지 않는다. |
| 계약 결정 | 현 worker는 publication 내부 준비 결과를 소비/폐기하며 원안의 idle staging을 제공하지 않는다. | 원안 유지 또는 publication-local preparation으로 범위 축소를 명시한다. idle cache를 새로운 필수 코드 과제로 자동 추가하지 않는다. |

이번 정적 수리:

- semantic dataset/contract promotion을 하나의 내부 모듈로 결합했다. intent를 dataset
  rename 전에 내구 기록하고 실제 staging tree도 먼저 sync한다. definite pre-contract
  rename 실패는 rollback intent를 내구 전환한 뒤 새 dataset을 격리한다. contract rename
  성공 뒤 sync/정산 오류는 pair를 보존해 forward recovery한다. cleanup의 실제 부분 삭제,
  복원 sync 재실패, 동일 contract bytes의 새 inode와 legacy ambiguous state를 고정
  filesystem oracle로 작성했다. 포맷·whitespace는 통과했으며 컴파일/테스트는 `NOT_RUN`이다.
- Core 출력 하위 예약은 control/work allowance를 구분하며 한쪽만 설치된 owner의
  반대 종류 출력은 거절한다. output child가 owner보다 오래 살아도 parent permit을
  보존한다. 바이트/파일 상한, 경로 binding, 생성 시작·내구 완료·후속 rename·확정 삭제
  후 정산 계약을 추가했다. borrowed output context는 구체적인 validation owner에
  의존하지 않으며 materializer→semantic port→adapter→sidecar writer로 전달된다.
  출력 예약은 즉시 반환하는 service 소유 용량 판단이며 peer cancel/deadline을 상속하지
  않는다. 기존 두 외부 scripted permit도 출력 ledger 부재를 명시적으로 거절한다.
  semantic sidecar는 `.next`를 최종 contract로 승격할 때 같은 permit을 유지하고,
  rename 성공 뒤 sync/정산 오류에서는 dataset만 rollback하지 않는다. vendor 본체와
  전체 용량 ledger·내구 identity 회계는 미구현이며 이 부분 연결 자체로
  whole-publication reserve를 수용하지 않는다. 새 테스트 본문은 `NOT_RUN`이다.
- lexical/semantic pair 측정과 단발 restore 측정을 같은 production session으로 연결했다.
  존재 generation의 mutation stripe를 비차단 획득해 scrub receipt 쓰기를 세션 수명 동안
  막는다. 부재 generation은 pin/cache하지 않고 다음 probe에서 확인한다.
- inode/object membership은 scratch SQLite의 한도 안에 저장하고 임의 요청 집합을
  checked union으로 계산한다. 실패 rollback·한도 거절·하드링크·부분집합·busy 재시도
  회귀 소스를 추가했다. scratch 주 파일 한도는 aggregate RSS/free-space reserve가 아니다.
- Native transition/verifier의 공유 intermediate proof를 하위 모듈로 이동해 역방향 타입
  의존성을 제거했다. checkpoint 알고리즘은 변경하지 않았다.
- physical session은 두 트랙 guard를 측정 종료까지 유지하며 acquisition 충돌은 비차단
  거절한다. lexical pool GC는 같은 flock로 차단하고 idle writer release는 lifecycle read를
  보유한다. writer release/discard는 background merge를 join하고, join 실패 뒤에는
  quiescence를 인정하지 않는다. 이는 linked-inode 파일 길이의 안정된 측정 계약이며
  allocated blocks, unlinked live handle, 전체 process RSS 또는 free-space 예약 계약이 아니다.
- 두 track의 physical census는 동일한 service-owned deadline을 공유한다. 기본 60초이며
  `QUANTA_INDEX_RETENTION_MEASUREMENT_TIMEOUT_MS`로 명시할 수 있다. peer 취소 scope를
  상속하지 않아 이미 승인된 durable publication의 소유권을 외부 disconnect로 끊지 않는다.
- full-track inode 집합을 SQLite main-file cap으로 중복 제거하고 directory-only 순회도
  pending 4096 frame/depth 64 envelope에서 거절한다. 실패한 부분 합계를 반환하지 않고
  임시 DB 정리 실패도 전파한다. live disk gauge는 동일한 bounded helper와 caller budget을
  사용하지만 snapshot custody 없는 관측이라는 의미를 유지한다.
- 교체된 lexical retention producer에 기존 closure 인증·SQL 계측을 연결했다.
  cache hit는 read/hash 재인증으로 세지 않고 rollback한 expanded row는 `SqliteRows`에
  넣지 않는다. 기존 full-set measurer는 작은 독립 test reference로만 보존한다. collector,
  exporter 및 정식 trace qualification은 다른 채팅의 소유 범위다.

2026-10-12 최종 재대조에서 위 snapshot 소스의 확정 잠금 순환은 찾지 못했다.
새 물리 측정·writer join·retention 테스트 본문과 실제 동시 실행은 `NOT_RUN`이다.
focused batch는 컴파일에서 실패했다. 이번 범위의 정수 타입/import/fixture 수리 후에도
다른 owner의 Runtime quota fixture와 File/Native verifier test 반환값 연결이 공유 source에
미반영되어 있다. 이것은 본문의 코드 과제에 중복 배정하지 않고 통합 검증 의존성으로 둔다.
idle staging은 계약 선택으로 남기며, 현 publication-local preparation에 추가해야 할
필수 코드 과제로 자동 승격하지 않는다.

`VERIFIED`는 해당 정적 모듈 순환·포맷·diff 검사 범위다. 새 retention 실행 및 sustained
churn/long pin/space pressure/query/cancel, 최신 전체 workspace/producer/remote CI/release는
별도다. 실행 결과는 [VALIDATION.md](../../plans/oct-10-index-closeout/VALIDATION.md)의 최신 절에서 판정한다.

출력 계약 후속 재대조의 잔여는 P1 semantic rollback 복구, P1 자원 회계,
P2 전수 측정/가용성, P2 실측 기반 enabled 정책의 네 묶음이다. writer가 사용하는 Tantivy `MmapDirectory`와
LanceDB connection은 출력 하위 예약을 받지 않는다. root census의 유한 deadline과
SQLite cap은 lock 유지 시간을 줄인 증거나 physical reserve가 아니다. idle staging은
계약 선택으로 유지한다. semantic maintenance의 receipt 쓰기는 explicit `None`이고,
`recover_dataset_artifacts`의 이전 `.next` rename/unlink도 ledger와 연결되지 않았다.
이 경로의 회계를 갖추기 전에 production authority를 설치하면 정상 복구가 과잉 charge
또는 누락 charge를 남길 수 있으므로 위 P1의 완료 조건에 포함한다. 공유 hexagonal gate의 scratch crate 누락은 다른 채팅의 private
통합 후보에 수정이 있으며 새 독립 과제로 중복 배정하지 않는다. private 수정 존재를
공유 source의 통과로 승격하지 않는다.

최신 요청에 따른 최종 재대조에서 이 채팅의 잔여 코드/정책은 위 네 묶음이다.
Native session/governor, trace, checkpoint, pool unlink/sync 및 통합 lint/fixture 수리는
각 소유 채팅의 범위로 제외했다. 새 P0는 이 정적 대조 범위에서 확인하지 못했다.
모듈 순환 gate는 통과했고 Core snapshot gate는 새 `PendingOrphanUnlink`의 baseline
누락으로 실패했다. 해당 GC 계약은 다른 채팅의 변경이며 이 감사에서 수정하지 않는다.
Rust 컴파일·테스트와 실제 운영 측정은 이번 재대조에서 `NOT_RUN`이다.
rollback P1은 source와 독립 정적 검토로 확인한 실패 경로이며 실제 filesystem 오류
재현은 `NOT_RUN`이다. 범위는 실패한 미봉인 target의 복구다. 기존 A/D/C5 수용이나
sealed serving generation에서 데이터 손실을 재현한 결과로 확대하지 않는다.

### 최신 소유 범위와 자원 원장 재대조 (2026-10-12)

같은 HEAD의 현재 공유 소스를 다시 읽었다. `Review Oct 10 index closeout --보완`은
semantic promotion 회귀의 통합 수용과 SQLite WAL·LanceDB의 쓰기 전 예약 경계를
조사 중이다. `증분`은 Native query/observer/custody 및 제품 transport/기본 전환을
진행 중이다. 이 실행·adapter 조사·Native 전환을 여기의 독립 작업으로 중복 배정하지
않는다. trace/profile, checkpoint, pool unlink/sync, replacement RFC, commit/push도
기존 소유 범위를 유지한다. 채팅의 진행 상태는 코드 수용 증거가 아니다.

| 우선순위 | 이 채팅에 남는 범위 | 구체적인 완료 조건 |
| --- | --- | --- |
| P1 — production publication authority | 기본 Runtime의 `publication_admission: None`은 유지된다. 새 Core `publication_capacity`는 순수 논리 파일 길이/unique inode 원장이고, `publication_inventory.rs`는 module에 등록됐지만 production 호출은 없다. | 기존 자원 authority와 결합한 control/work estimator, adapter open 전 boot inventory, output binder와 pool subset의 원자적 회계를 설치한다. root FD/lease identity와 모든 controlled writer의 coverage를 갖추기 전에 production hard limit으로 수용하지 않는다. pool과 전체 원장이 같은 inode를 별도로 중복 charge해서는 안 된다. |
| P1 — retained mutable output와 복구 정산 | Core `reserve_resize`/`ResizeTicket`는 retained inode의 증가량 예약·부분 shrink 정산을 표현한다. 실제 열린 inode와 writer/GC 수명에 연결하는 Runtime binder는 없다. SQLite DB/WAL처럼 기존 inode를 여러 publication이 수정하는 경우의 charge 소유권도 정해지지 않았다. | retained inode의 쓰기 전 증가량 예약, resize 실패/부분 쓰기, 교체·하드링크·pin·열린 FD의 마지막 수명, 동일 process pending 정산과 재시작 inventory를 하나의 계약으로 연결한다. vendor writer hook 자체는 위 소유 채팅의 조사 결과를 통합한다. 사후 fstat과 path unlink만으로 reserve/refund를 판정하지 않는다. |
| P2 — hot-path 전수 작업과 가용성 | pair 내부 union은 증분이나 요청 최초 인증·root census는 남는다. semantic inheritance와 promotion fence는 상속 tree 전체 순회/파일 sync이며 fence는 depth만 제한한다. | root-wide 증분 회계로 publication마다 전수 스캔하는 경로를 줄이고, 인증된 immutable base의 durable reuse와 changed/new sync를 구분한다. 파일 수·작업 시간 envelope 및 잠금 유지/busy 재시도 비용을 수용한다. hardlink 수만 보고 sync를 생략하지 않는다. |
| P2 — measured enabled LQ5 정책 | soft/hard·post-merge admission 코드는 있으나 기본 정책은 비활성이다. logical merge facts는 실제 I/O·RSS·임시 disk peak가 아니다. | 지원 workload에서 threshold와 merge reserve/backpressure를 보정하고 enabled 정책을 수용한다. sustained churn·long pin·space pressure·query/cancel 및 aggregate RSS/temp disk peak를 한 운영 검증 배치로 확인한다. |

Core leaf의 정적 재대조에서 boot seed가 over-cap 실패한 뒤에도 seal할 수 있는 경로와,
서로 다른 ledger의 숫자 token이 충돌하는 경로를 확인했다. 이는 미연결 새 leaf의
결함이며 현재 기본 Runtime의 데이터 손실 재현으로 확대하지 않는다. 수리와 독립
고정 oracle의 상태는 `VALIDATION.md`의 최신 절을 따른다. parent 전액 예약/child 분할,
commit slack, dropped created ticket의 보수적 유지, active allowance 상한과 monotonic
pre-growth를 추가했지만 이 산술만으로 opaque writer의 실제 강제를 증명하지 않는다.

boot helper는 `max_entries`·depth·pending frame·caller budget으로 순회를 제한한다.
새 unique inode를 map에 넣기 전에 누적 byte/file cap을 검사하도록 수리했다. inode map은
`max_files`와 `max_entries` 양쪽으로 제한되며 hardlink alias는 중복 charge하지 않는다.
inventory module은 등록됐다. 새 조기 거절 fixture의 실행 수용은 `VALIDATION.md`의 소스별 결과를 따른다.
lease는 열린 root directory FD를 보존하며 scanner는 `require_opened_state_root_v1`으로
root와 held lock inode의 identity를 확인한다. production boot 설치와 writer binder는 없다. 단일 협력 writer/boot 전제 밖의 일관 snapshot,
unlinked-open inode, allocated blocks/free space/RSS/I/O는 보장하지 않는다.
재시작 시 실제 파일 집계와 함께 미완료 durable attempt/intent의 pending output와 승인된
잔여 allowance를 신규 admission 전에 재채택해야 한다. volatile counter 또는 process-local
token을 persisted 예약의 대체물로 사용하지 않는다.

semantic 상태 기계의 지정 crash matrix에서는 추가 확정 결함을 정적으로 찾지 못했다.
18개 module filesystem fixture의 실행과 실제 build/subprocess 회귀는 별도 수용이다.
create 실패 또는 rename 뒤 sync 실패에서 output permit을 잃는 경우의 보수적 pending은
binder reconciliation으로 처리해야 한다. 이전 복구 P1을 그대로 신규 코드 과제로
중복 배정하지 않는다. 이번 범위에서 새 production P0는 확인하지 못했다.

### 최종 정적 감사의 구체적인 수리 경계

위 네 묶음은 LQ5의 미완료 요구이며 다른 채팅의 vendor hook·Native·trace·checkpoint
작업을 새 독립 과제로 포함하지 않는다. HEAD와 공유 dirty source를 읽기 전용으로 대조했다.

- **P1 / authority 설치:** Runtime factory의 `publication_admission`은 `None`이다. boot scanner는
  호출되지 않는다. Core의 live retained-inode lookup은 구현됐다. writer open 전의 실제 root
  inventory, control/work 비용 상한, pool subset과 공통 원장의 단일
  charge를 factory에서 결합해야 한다. 단순히 `Some`으로 바꾸거나 pool-only permit을
  넣으면 전체 publication 강제라는 완료 조건을 충족하지 못한다.
  `SourcePublicationUploadStore`의 create/grow/TTL unlink는 journal/control admission
  이전에 수행될 수 있다. upload-local disk/slot quota를 공통 원장의 대체물로 삼지 않는다.
  ingress/upload allowance를 같은 authority에 잡고 publication에 넘길 때 이미 retained된
  inode를 재차 charge하지 않아야 한다.
- **P1 / mutable·수명·복구:** `reserve_resize`는 retained inode의 쓰기 전 성장 예약과 부분/실패 shrink의 실제 길이 정산을 표현한다.
  zero-file 성장/shrink allowance도 표현하지만 production writer/GC binder는 없다.
  retained inode의 증가량을 쓰기 전에 예약하고 overwrite/truncate, 동시 resize/GC,
  부분 쓰기·sync 실패 및 오래된 정산 token을 구분해야 한다. root lease는 canonical path, root directory FD와
  lock-file FD를 보유한다. semantic AtomicOutput은 path와 permit만 보유한다. 실제 I/O를
  leased directory/열린 inode의 custody와 결합하고, 교체된 old inode는 마지막 hardlink와
  service-owned FD 해제 뒤에만 환급한다. 같은 process의 orphan과 재시작 attempt/intent
  재채택은 기존 journal을 사용하며 별도 journal을 추가하는 요구가 아니다.
- **P2 / 변경량과 가용성:** root-wide census는 lifecycle/pool gate를 잡고 전체 track을
  순회한다. semantic inheritance도 전체 base tree를 읽고 promotion fence는 각 파일을
  sync한다. depth 제한만으로 파일 수나 경과 시간 상한을 보장하지 않는다. root-wide
  증분 회계와 인증된 immutable base의 durable reuse를 먼저 갖추고 changed/new output의
  sync를 구분한다. 인증되지 않은 파일의 fsync를 생략하는 수리는 수용하지 않는다.
- **P2 / enabled 정책:** 설정 기본값과 env 미지정은 maintenance `None`이다. merge
  threshold와 aggregate reserve/backpressure의 지원 workload 수용이 남는다. 논리 inode
  길이 상한, allocated blocks/free-space headroom, process RSS, 실제 I/O는 각각 다른 범위다.
  지속 churn·긴 pin·공간 압박·동시 query·취소의 한 배치에서 운영 수치를 수용한다.

구현 의존 순서는 mutable/수명 계약 → boot/binder/estimator → production factory와
소유 채팅의 writer hook 통합 → 전수 비용 축소 → 정책 수용이다. Core는 순수 회계와
포트, Runtime은 공통 authority·조립, adapter는 실제 I/O를 소유하며 전역 scheduler를
leaf마다 복제하지 않는다.

**VERIFIED — 이번 정적 대조:** `rg`, `nl -ba`, `sed`로 위 생산 경로·API·설정과
최근 소유 채팅 범위를 대조했다. 이는 계약 공백의 소스 확인이며 실행 성공이 아니다.
**NOT_RUN:** 이번 최종 감사의 Rust 컴파일·테스트·성능·지속 운영 실행. 기존 수용은
각 기록의 소스와 범위를 유지하며 현재 전체 workspace GREEN으로 승격하지 않는다.

### Mutable accounting와 root custody 수리 (2026-10-12)

- `retained_inode`, `reserve_resize`, `verified_reuse`에서 진행 중 resize의 busy 판정을
  transient 길이/child 상한 비교보다 앞에 둔다. 10-byte inode가 예약된 17-byte 범위에서
  16 bytes로 변경되는 동안의 동시 조회·재예약·reuse는 typed capacity refusal이다.
  정상적인 경합이 공통 authority를 영구 poison하지 않는다. 다른 authority의 token은
  여전히 먼저 거절한다. resize 정산 직후의 stale ID도 mutation 전에는 비poison
  `InvalidContract`로 거절하고 fresh ID로 재시도한다. 현재 ID의 미회계 길이 불일치와
  잘못된 terminal settlement는 fail-closed poison을 유지한다.
- 고정 숫자 oracle은 위 세 reader의 snapshot 불변과 원 resize의 16-byte settlement,
  정산 경계의 stale ID 거절·fresh ID로 17-byte 재시도, 낮은 growth 요청에서 예약한
  17-byte high-water 유지, same/different parent의
  live/closed/orphan 마지막 삭제 환급을 검사한다.
- `PublicationOutputReservation`의 path-bound child는 정확히 한 file slot만 허용한다.
  여러 파일을 한 staging/target callback으로 대표하지 않는다. 생성자와 custody/routing
  fixture를 함께 수정했다. 현재 semantic 생산 호출은 한 파일을 예약한다.
- `StateRootLease` 복제본은 동일한 root/lock FD를 공유한다. 복제한 디렉터리 FD는
  `StateRootDirectoryV1`과 lease owner를 함께 반환한다. guard는 borrowed `AsFd`와
  identity 재검증만 제공하고 bare `File`을 반환하지 않는다. 마지막 guard가 사라질 때까지
  독점 lock이 유지되는 fixture를 추가했다. 명시적으로 별도 FD를 복제하는 I/O owner도
  guard를 실제 I/O 수명 동안 보관해야 한다.
- boot scanner도 이 guard에서 인증된 root FD를 복제해 순회하고 seal까지 custody를 유지한다.
  lease가 인증한 symlink root 별칭을 다시 `O_NOFOLLOW`로 열면서 거절하던 경로 불일치를
  제거했다. 종료 때 원 경로와 guard를 모두 재검증하므로 root/lock 교체 거절은 유지한다.
  root 내부 entry에는 기존 nofollow 검증을 적용한다. canonical과 별칭의 고정 4-byte/
  2-file 원장 동일성 fixture를 추가했다.

sidecar binder는 `Review Oct 10 index closeout --보완`의 통합 범위이므로 별도 구현을
추가하지 않았다. 이 수리는 순수 회계와 custody 계약이다. production writer 전체 연결,
boot factory 설치, pending journal 재채택 및 지속 운영 수용의 완료를 뜻하지 않는다.
실행 판정은 `VALIDATION.md`의 해당 소스와 범위를 따른다.

### Source-upload 디렉터리 custody 수리 (2026-10-12)

- 기존 upload store는 root pathname을 매번 따라가며 stage/load/discard를 수행했다.
  열린 뒤 root 또는 부모 경로가 교체되면 원 root lease와 다른 디렉터리에 접근할 수 있었다.
  store는 이제 parent custody와 upload directory FD를 보관하고 `openat/statat/unlinkat`
  및 FD-relative `Dir`로만 body I/O를 수행한다. 각 transition과 성공 반환 전 열린 inode,
  parent entry 및 현재 root pathname의 동일성을 확인한다.
- `open_at`은 한 normal component와 parent pathname/FD identity를 생성 전에 검증한다.
  Runtime 조립은 `StateRootDirectoryV1`을 넘기므로 store의 마지막 I/O 수명까지 root
  lease가 유지된다. child directory FD가 parent guard보다 먼저 닫힌다.
- 단순 `AsFd` 보유만으로는 parent의 held lock entry 검증이 호출되지 않는 추가 결함을
  발견했다. root와 upload 디렉터리는 그대로 둔 채 lock entry만 교체하면 새 owner가
  독립 lock을 얻고 기존 store도 ACK할 수 있었다. IPC의 `SourcePublicationUploadParent`
  custody 계약은 생성 전과 각 root 검증 때 `require_current_v1`을 호출한다. Searchd guard는
  기존 root/held lock inode 검증으로 위임한다. lock 교체 뒤 기존 stage/load/discard 거절과
  body 불변의 gold를 추가했다. 검증과 syscall 사이의 동시 악의적 교체를 원자적으로
  방지한다는 주장은 하지 않는다.
- inventory는 최대 8개 entry frontier와 entry별 원 요청 budget checkpoint를 가진다.
  stage는 쓰기 전과 ACK 전에 다시 확인한다. 취소된 부분 쓰기의 기존 바이트를 보존하며
  재시도는 중복 구간의 동일성을 확인한다. 특별 파일은 nonblocking/nofollow open과
  regular/private/single-link 검사로 거절한다.
- 열린 body의 최종 ACK/load 검증은 inode뿐 아니라 regular/private/single-link 조건도
  다시 확인한다. 이전 helper는 open 이후 mode 또는 link count 변경을 거절하지 못했다.
  초기 open과 최종 전환이 같은 검사로 위임하며, 열린 FD의 chmod와 추가 hardlink를
  각각 거절하고 원 바이트를 보존하는 독립 gold를 추가했다.
- 생성한 upload directory와 parent를 store 공개 전에 sync한다. TTL unlink는 각 삭제 직후
  directory를 sync하여 뒤따르는 취소·거절에서도 삭제의 내구성을 보존한다.
- local 1 GiB/8-slot quota는 단일 store 전제다. production은 state-root lease와 단일 store
  조립으로 이를 유지한다. 이 수리는 common-ledger ingress allowance, 전체 writer reserve,
  allocated-block/free-space 또는 process RSS 수용을 구현하지 않는다.

root 교체, parent identity 불일치·비local component, inventory frontier, FIFO 거절과
production lease 수명 gold를 추가했다. 빌드·실행 판정과 공유 의존 소스의 실패는
`VALIDATION.md`의 해당 수용 절을 따른다.

daemon profile에서 shared lexical body scrub 회귀가 retention physical snapshot의
lifecycle try-write busy로 실패했다. 작은 inline publication 경로이며 upload port를
호출하지 않는다. scrub과 measurement가 같은 read/write gate를 공유한다는 정적 조건은
확인됐지만 실패 순간의 실제 holder는 미확정이다. 물리 GC/retention 소유 범위의 수용
차단 항목으로 유지하며 테스트 비활성화·거절 무시·관측되지 않은 재시도 성공으로 닫지 않는다.

최신 retention 소스는 `SearchCorpusRetentionMeasurementBusy`를 실제 quota 거절과
구분한다. service-owned 원 deadline 안에서 guard를 반환한 후 제한된 backoff로 전체
plan/census를 다시 계산한다. 기존 durable record 경로에도 같은 재시도를 적용한다.
기존 파일의 고정 CBOR 내용·OS inode·entry 수를 보존하는 독립 회귀를 추가했다.
정적 구현 확인과 daemon/회귀 실행 수용은 구분한다. 과거 두 실패는 새 소스의 실제
재검증 결과가 나오기 전까지 역사적 `FAILED`로 유지한다.

별도 staged crash/restart owner도 fixture root를 생성 시점부터 0700으로 고친 뒤
동일 lifecycle busy로 실패했다. staged crash/restart 완료는 미수용이다. 현재 busy와
실제 retention cap 초과는 최신 소스에서 별도 typed 오류다. held guard를 정리한 뒤
원 service-owned deadline 아래 plan과 census를 다시 계산하는 회귀를 포함해
upload/lease/inventory/history **82 PASS**를 확인했다. 이 focused 결과만으로
staged crash/restart의 실프로세스 수용을 닫지 않는다. blocking lock 전환·sleep under
lock·quota 생략 또는 fixture-only retry로 두 실패를 숨기지 않는다. 독립 oracle과 실행 범위는
`VALIDATION.md`의 upload 수용 절에 기록했다. 해당 production 경로는 기존 physical
GC/retention 소유 범위이며 여기서 중복 수정하지 않았다.

최신 mandatory daemon 배치는 **58 PASS / 1 FAIL / 258 selected NOT_RUN**이다.
4,096-source Large seal의 300초 응답 제한과 `ingest-accept` 종료 실패가 새 차단 항목이다.
provider window 4개 반환 이후에도 마지막 semantic 저장·ANN 학습·dataset promotion,
lexical 게시·독립 검증·source transaction·retention이 남는다. 정적 검사에서는 확정적인
교착이나 corpus 제곱 시간 루프를 찾지 못했다. 단계별 진단 전에는 원인이나 수리 완료를
확정하지 않으며 기존 기대값과 시간 제한을 유지한다.

후속 실행 중 trace/1초 stack 관측은 semantic seal 완료 뒤 Native positions 구축과
File run page decode를 분리했다. Native 새 세그먼트는 unique local ID를 쓰면서도
문서마다 전체 term 사전의 제거 순회를 호출했다. 새 append 경로는 중복 ID를 mutation
전에 거절하고 `add_token`을 사용한다. File delta는 이미 인증·decode한 old page를
다음 적용 함수에서 다시 읽고 decode했다. 이제 prior ref/page를 전달하고 현재 map ref와
canonical 내용·길이·hash·key를 검증해 중복을 제거한다. prior/next 페이지 동시 보유에
맞춰 run edit batch만 512개로 제한한다. 독립 seal, wire 한도, quota와 시간 제한을
유지한다. 새 소스의 집중 lexical 회귀 13개와 production library strict가 통과했다.
posting 예산 검사의 지연 charge와 구형 leaf를 훼손하던 corruption fixture도 수리했다.
실제 query가 읽는 run page를 훼손해 restricted 결과 보존과 unrestricted 거절을 함께
검사하며, 기존 read bound는 유지한다. Large·XL/bulk 구축은 `증분 (2)` 진행 범위다.
전체 daemon·성능 수용과 이전 scrub/crash-restart 회귀의 판정은 최신 `VALIDATION.md`를 따른다.


### Staged recovery와 전체 daemon 실패 후속 수용 (2026-10-12)

이 절은 위의 staged/전체 daemon 대기·실패 checkpoint를 실제 후속 결과로 대조한다.
이전 실패 기록을 지우지 않는다.

- **VERIFIED — staged 기능**: G36의 유효한 replacement, 원 body/예약/checkpoint,
  G+1 authority 거절, G35 replay/활성화와 prefix/tail 검색이 103.834초에 통과했다.
  hard/input cap과 시간 제한을 유지했다.
- **FAILED — 고정 후보 전체 daemon**: 기존 canonical daemon-all 70개 catalog 행,
  15개 바이너리의 521개 선택을 `--no-fail-fast`로 끝까지 실행했다.
  519 PASS / 2 FAIL / 11 skipped, 1,857.933초. 이전 receipt·Medium scale profile·
  trace path·Large 문제는 해당 후보에서 통과했고 물리 GC 두 건만 실패했다.
- **수리**: 독립 CBOR 오라클을 File v20/Native v8 및 registry/premerge/run-postings/
  PageRef 실제 wire에 맞췄다. shared-pool 수거 fixture는 기존 24시간 dormant scrub
  대신 daemon 시작 전 100ms pacing을 사용한다. 실제 digest 객체 삭제·재생성,
  quota/디스크 일치, KEEP=2 및 30초 제한은 유지했다. 생산 GC는 수정하지 않았다.
- **VERIFIED — 전체 영향 GC/scrub**: 같은 후보 전용 target에서 12 PASS / 242 skipped,
  89.211초. 실패했던 두 테스트와 pinned route, orphan, 두 트랙 삭제, 손상·격리를
  포함한다. scoped rustfmt/diff도 통과했다.

명령과 source digest는 VALIDATION의 Shared-object integration 절을 따른다.
수용 후보에 다른 채팅의 후속 메모리 변경을 섞지 않았다. `publication_admission: None`,
boot pending 재채택·estimator·mutable WAL/vendor writer·inode/열린 FD 정산 및 측정 기반
LQ5 운영정책은 여전히 미완료다. 최신 공유 전체 workspace, native Large/XL·지속부하,
실제 producer·quiet-host 성능·remote CI·release·공유 commit/push는 이 결과로 수용하지 않는다.
