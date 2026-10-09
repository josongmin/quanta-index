# 해결안 감사와 범위 결정

감사/범위 수정일: 2026-10-10 · source HEAD: `e073465e534f3529d47a43f84ceb1f19992953f1`.
정적 소스/설계 감사이며 구현·race 재현·Rust/daemon/성능 검증은 `NOT_RUN`이다.
현재 구현 범위와 병렬 배정은 [README](README.md), oracle은 [VALIDATION](VALIDATION.md)이 소유한다.

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
