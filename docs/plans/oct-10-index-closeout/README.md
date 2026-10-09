# Quanta Index closeout 최종 구현안

설계: `FINAL` · 구현: `OPEN` · 실행 검증: `NOT_RUN` · 확정일: 2026-10-10.
소스 기준: `e073465e534f3529d47a43f84ceb1f19992953f1`.

이번 범위는 A/D의 직접 수리와 C5의 최소 영속 예약 공급이다.
이 문서가 구현·소유권의 기준이며, 근거는 [AUDIT.md](AUDIT.md), 합격 조건은
[VALIDATION.md](VALIDATION.md)에 둔다.

## 확정 결정

| 작업 | 구현 | 완료 기준 |
| --- | --- | --- |
| A semantic compatibility | 누락된 모델·정책 저장과 target/base 순수 preflight | 비호환 계약을 첫 window/provider 및 최종 event binding 전에 거절 |
| D RepoMap lifetime | 기존 store gate의 acquire/pin/publish/activation/GC 보호 범위 수리 | 성공한 pin의 객체 보존, 마지막 drop 뒤 reclaim, 실제 unwind 회귀 |
| C5 generation reservation | 기존 catalog의 high-water, event별 선발급, 1회 최종 body binding | 번호 비재사용, 원 예약 재시도/재시작 복구, control/SDK 연결 |

A/D는 현재 owner의 직접 수리이며 각각 독립적으로 main에 통합할 수 있다. C5는 새 기능이므로
catalog/control/ingress/SDK가 모두 연결된 한 묶음으로 수용한다. 이번 계획 변경은 Rust 구현이 아니다.

## 고정 불변식

| 경계 | 불변식 | 실패 시 동작 |
| --- | --- | --- |
| semantic inheritance | 같은 차원이라도 모델·revision·정책이 다르면 base 벡터를 상속하지 않음 | provider/첫 window·신규 최종 binding·target 변경 전에 거절 |
| semantic recovery | 정확히 완료된 원 target의 복구는 새 delta build와 구분 | 기존 receipt/roots/sequence로 복구, 재임베딩·GC된 base 재요구 없음 |
| RepoMap acquire/GC | 성공한 pin 등록과 해당 객체 unlink 사이에 단일 직렬화 순서가 있음 | retire가 먼저면 acquire 거절, acquire가 먼저면 마지막 drop까지 GC 보류 |
| allocation durability | 동일 root incarnation의 repo/revision 안에서 high-water가 감소하지 않음 | persist 불확정 시 성공 ACK 금지, reopen 후 원 event로 재시도 |
| event binding | target-independent source payload와 발급 번호를 최종 body에 한 번만 연결 | payload/base/revision 또는 최종 digest/key 변경은 conflict |
| activation authority | 번호 발급·publication receipt는 activation 권한이 아님 | 기존 explicit CAS 검사, conflict 후 자동 expected-head 갱신 없음 |

## A semantic compatibility

- [ ] `GenerationContract`와 manifest에 기존 모델의 `policy_digest`/`view_policy_digest`를
  보존한다. writer/reader/validation을 함께 바꾼다. 옛 format은 typed rebuild refusal,
  현 format의 필수 key 누락은 decode/storage corruption이다.
- [ ] semantic owner 한 곳에서 target와 delta base를 검사한다. model ID/revision/dimension/
  metric/normalization/embedding policy/view policy 및 공통 corpus policy를 비교한다.
  generation/mode/base 번호, required-corpora와 row별 render 집합 전체 equality는 요구하지 않는다.
- [ ] 직접 build는 첫 `next_window()` 전에 기존 target/base guard 안에서 검사한다.
  paired ingest는 provider·최종 event binding 전과 operation lock 안에서 같은 검사를 한다.
  쓰기를 수행하는 `ensure_generation_contract`를 순수 preflight 대신 쓰지 않는다.
- [ ] 기존 `embedding_model_contract_for` 투영을 공유한다. 빈 필수 identity/policy와
  base/target의 None·빈 revision으로 delta 상속하지 않는다. full replacement admission은 유지한다.
- [ ] terminal replay와 complete-target finalize 복구를 유지한다. 완료된 원 작업을
  재임베딩하거나 GC된 base를 다시 요구하지 않는다. 기존 query ID/revision gate도 유지한다.

C5 선발급은 별도 요청이다. 이미 발급된 번호가 A의 publish preflight 거절로 취소되는 것은
아니다. 거절 oracle은 provider/첫 window·신규 최종 binding·target 변경 0이며,
기존 immutable Prepared journal과 이미 발급된 번호의 보존은 허용한다.

변경 표면: [semantic 계약](../../../crates/quanta-index-semantic/src/generation_contract.rs),
[manifest](../../../crates/quanta-index-semantic/src/manifest.rs),
[build](../../../crates/quanta-index-semantic/src/build.rs), Core semantic port,
[semantic_derive](../../../crates/quanta-index-search-plane/src/semantic_derive.rs),
[paired ingest](../../../crates/quanta-index-search-plane/src/ingest_dispatcher/search_corpus.rs)와 해당 회귀.

구현 순서:

1. `GenerationContract::{from_batch,merge_batch}`와
   `SemanticManifest::{from_generation_contract,validate_against}`의 누락 필드를 함께 보완한다.
   codec의 필수 key 계약과 format stamp를 함께 변경하며 기존 `EmbeddingModelContract`를
   그대로 사용한다. 별도 V2 모델 IR이나 fallback reader를 만들지 않는다.
2. 읽기만 수행하는 target/base compatibility 검사를 semantic owner에 둔다. 새 필수 port를
   추가하면 모든 production adapter와 test double을 함께 수정한다. 기본 `Ok(())` 구현은 금지한다.
   target의 기존 계약 검사와 delta base의 sealed marker/manifest/build contract 검사를 구분하고,
   이미 저장된 target의 mode/base 불변식은 유지한다.
3. 직접 경로는 `generation_build_guards`와 directory lifecycle guard 안에서 첫 window 전에 검사한다.
   paired 경로는 provider 없는 모델 투영을 공유하여 BeforeIntent와 UnderOperationLock에 연결한다.
   preflight 결과를 이후 mutation의 영구 허가로 캐시하지 않는다. 실제 build 진입도 같은 검사를 한다.
4. current query의 모델 identity gate, exact terminal replay, finalize-only/partial recovery를
   함께 검증한 뒤 A 전체를 통합한다. full replacement는 새 generation에서 모델 변경을 허용한다.

## D RepoMap lifetime

- [ ] 기존 `activation_commit_gate`로 head/registry 확인→pin 등록을 보호한다.
  GC도 같은 gate에서 retirement/pin을 재확인하고 unlink/registry 제거까지 수행한다.
- [ ] publish의 최종 seal/catalog/registry 설치와 activation을 같은 lifecycle로 보호한다.
  기존 replay의 object 검증도 GC와 경합하지 않게 한다. replay 응답/보존 정책은 바꾸지 않는다.
- [ ] 순수 compile, query 본문, lease Drop은 gate 밖이다. Drop은 기존 pin table만 사용한다.
  실제 call graph의 lock order와 재진입을 확인하며 새 lock/registry를 추가하지 않는다.
- [ ] panic 테스트는 active view 확보, pin=1, 의도한 body 진입, 정확한 panic payload,
  unwind 후 pin=0과 후속 reclaim을 각각 확인한다.
- [ ] acquire↔retire/GC와 publish↔acquire 경쟁을 barrier로 고정해 검사한다.
  sleep 반복이나 `catch_unwind().is_err()`만으로 통과시키지 않는다.

보장 범위는 기존 state-root lease 아래 한 store Arc를 공유하는 daemon이다.
독립 public `open()` 복수 handle의 보장이나 전역 lifecycle manager로 범위를 늘리지 않는다.

변경 표면: [store](../../../crates/quanta-index-repomap/src/store.rs),
필요한 [pin lease](../../../crates/quanta-index-repomap/src/pinned.rs) 연결과
[lifetime owner 테스트](../../../crates/quanta-index-repomap/tests/read_view_lifetime_owner_v1.rs).

구현 순서:

1. 기존 panic oracle을 먼저 교정한다. 현재 테스트는 G2 활성화 뒤 G1을 acquire하여
   의도한 본문 전에 panic할 수 있다. active generation acquire 성공과 pin=1을 확인한 다음
   의도한 panic을 발생시키고, 정확한 payload와 unwind 뒤 pin=0을 확인한다.
2. 기존 gate가 acquire의 head/registry 조회부터 lease 생성까지, GC의 최신 retired/pin 재검사부터
   unlink/registry 제거까지 보호하도록 한다. compile은 밖에서 수행하고 publish의 최종
   replay/pin 재검사 및 seal/catalog/registry 설치는 gate 안에서 수행한다.
3. `ingest_bundle_v2`의 exact replay/object 검증과 `commit_activation`을 같은 규칙에 연결한다.
   gate를 획득하는 entry와 이미 guard를 가진 helper를 구분해 중복 획득을 없앤다.
   gate 아래에서만 store/catalog의 짧은 guard를 사용하고, pin guard를 쥔 채 gate를 기다리지 않는다.
4. 읽기 본문과 Drop은 기존 Arc/pin table만 사용한다. read-to-pin 및 check-to-unlink 경계의
   test-only barrier로 실제 경쟁 순서를 고정한다. 정상 query 경로에 gate나 테스트 hook을 추가하지 않는다.

## C5 generation reservation

번호 예약은 activation 권한이 아니다. 기존 publish와 activation CAS 분리를 유지한다.

- [ ] repository/revision과 기존 `SourcePublicationEvent`로 번호를 선발급한다.
  기존 `(repository, stream, event)` identity를 유지하며 revision/payload/expected source base
  변경 재사용은 conflict다. 응답은 원 번호와 event binding이며 publish receipt가 아니다.
- [ ] 기존 repository envelope에 revision별 영속 high-water를 둔다. 현 형식에서 등록된 양 track의
  sealed/in-flight/rollback/history와 기존 event의 점유보다 높은 번호를 발급하고 persist 후 응답한다.
  명시적 generation writer도 같은 점유 admission에 참여한다. 예약된 target 탈취를 막고
  explicit writer가 high-water를 추월한 경우 다음 발급에 반영한다.
  sealed 목록은 미완성 디렉터리를 제외하므로 floor의 유일한 근거로 쓰지 않는다.
  최초 pair는 adapter 소유의 순수 empty 검사로 확인한다. 기존 미추적 점유가 있거나 empty를
  확인할 수 없으면 typed refusal한다. 기존 root의 floor를 추정해 자동 편입하지 않는다.
- [ ] GC/rollback/timeout으로 번호를 재사용하지 않는다. gap은 허용한다.
  overflow/capacity/저장 불확정은 기존 typed 오류로 처리하며 누락 high-water를 0으로 읽지 않는다.
  persisted format은 한 번 변경하고 구형은 typed rebuild refusal로 처리한다.
- [ ] 기존 event record에 최종 body가 없는 선발급 상태 하나만 추가한다.
  발급 번호를 포함한 최종 body를 검증한 뒤 canonical digest/journal key를 한 번 연결하고
  기존 Pending→Staged→Active 경로를 사용한다. fake digest/key나 별도 journal은 만들지 않는다.
- [ ] 같은 event 재시도와 restart는 원 번호/최종 binding을 돌려준다. 다른 body/key의
  재결합은 거절한다. 기존 single-pending 및 오래된 미완료 event의 순서 제한을 유지한다.
  선발급 상태도 기존 unresolved target 보호에 포함하며 timeout으로 자동 해제하지 않는다.
  unresolved selector는 digest 없는 선발급과 digest 있는 bound target을 구분한다.
  선발급에는 generation 점유/보호를, bound에는 기존 exact digest 검사를 적용한다.
- [ ] 제어된 restore 뒤 이전 예약의 재사용은 저장된 기존 root incarnation과 현재 값을
  binding 시 비교해 거절한다. 자동 rebase나 root 전체 reconciliation을 추가하지 않는다.
  이 최소안은 중단된 event의 무조건 진행이나 restore 이후 자동 stream 해제를 보장하지 않는다.
- [ ] 기존 control mutation 하나를 Admin admission으로 공급하고 SDK
  `SearchCorpusNamespace`에 예약 메서드 하나를 연결한다. codec/오류/실제 exhaustive match와
  native admission을 함께 수정한다. `GenerationNamespace`에 중복 facade를 두지 않는다.

**activation CAS는 예약에 넣지 않는다.** caller가 기존 `activate_published`/
`publish_and_activate`에 명시적으로 전달한 expected head를 기존 방식으로 검사한다.
SDK가 충돌 후 expected head를 자동 갱신하지 않는다. head 변경/rollback은 번호 예약을
자동 취소하지 않으며, 예약 시 frozen CAS 때문에 생기는 별도 Superseded 상태 축도 도입하지 않는다.
동시 번호 발급은 같은 pair의 병렬 publication이나 무조건 진행을 뜻하지 않는다.
새 allocation 요청의 revision 변경 conflict와 기존 publish replay는 구분한다.
이미 bound된 event의 publish replay가 새 요청 target 대신 원 target을 복구하는 기존 계약은 유지한다.

변경 표면: Core [source publication](../../../crates/quanta-index-core/src/domains/source_publication.rs),
[activation catalog](../../../crates/quanta-index-search-plane/src/readiness/activation_catalog.rs)의
envelope/source-events, 필요한 기존 unresolved selector,
Contract control/split/codecs, control/ingest dispatcher와
[SDK SearchCorpusNamespace](../../../crates/quanta-index-sdk/src/lexical.rs).

구현 순서와 선형화 지점:

| 단계 | 변경 | 완료/선형화 기준 |
| --- | --- | --- |
| C1 계약 | 기존 event record를 선발급과 최종 binding을 구별하는 단일 canonical 타입으로 변경 | 선발급은 generation/event/incarnation만 보유하고 fake manifest/key를 요구하지 않음 |
| C2 발급 | 기존 envelope에 pair별 high-water와 원 event의 선발급 기록 추가 | 기존 `persist_repository`의 durable 완료 뒤 ACK. memory-only 발급 없음 |
| C3 최종 연결 | 검증된 body로 기존 `reserve_source_event(binding)`을 호출 | 원 예약 번호와 최종 manifest/batch digest/journal key를 한 번 영속 연결한 뒤 양 track 변경 |
| C4 경로 연결 | control Admin admission, codec, dispatcher/native match, SDK 메서드와 response binding | 실제 UDS 요청이 owning catalog를 호출하고 원 event/repo/revision/generation을 검증 |
| C5 복구 | 기존 journal/reconcile/activation과 unresolved selector 연결 | Allocated→bound Pending→Staged→Active, 원 event retry/restart와 explicit CAS 유지 |

`source_event_payload_sha256`는 이미 containing revision/generation/manifest/transport digest/seal을
제외한다. 이를 선발급의 source fingerprint로 사용한다. 최종 body의 batch digest와 혼동하거나
번호를 받기 위해 임시 generation/가짜 digest를 만들지 않는다. 새 allocation의 revision 비교는
이 fingerprint와 별도로 수행한다. 최종 publish에서 기존 source payload 재검증도 유지한다.

W0에서 실제 production ingress와 번호 점유의 관계를 먼저 닫는다. 현재 wire의 paired track
입구는 `PublishSearchCorpusBatch` 및 그 경로로 합류하는 staged upload다. 같은 root에 쓰는
추가 production 경로가 발견되면 첫 physical mutation 전에 동일 점유 admission에 연결한다.
seal 이후 inventory만으로 미완료 target이 없다고 추정하지 않는다.

하위호환은 제공하지 않는다. 구형 envelope/semantic format을 현재 형식으로 자동 변환하지 않는다.
high-water가 없는 pair의 최초 생성은 양 adapter에서 기존 점유가 없음을 확인한 경우에만 허용한다.
미추적 점유가 있으면 typed rebuild/refusal이며, 이 범위에 기존 root의 자동 복구·최대 번호 추정
migration을 추가하지 않는다. 새 형식의 명시적 generation publish도 persist된 점유와 high-water를
갱신한 뒤 쓰며, 이미 다른 event에 선발급한 번호는 사용할 수 없다.

counter 단조성은 동일 root incarnation 안의 보장이다. 제어된 restore는 기존 incarnation 회전을
사용하고 이전 예약의 binding을 거절한다. restore를 가로지르는 전역 단조 counter나 자동 stream
해제를 보장하지 않는다. 미완료 event는 기존 capacity/single-pending 제약을 소비하며 취소/TTL은 없다.

## Execution order

구현 시 배정안은 A/D/C5 owner 3개 + 통합 1개다. 이번 계획 작성에서 worker를 실행한 것은 아니다.
독립 코드 작성은 병렬로 배정할 수 있고, 무거운 build/native
검증은 한 번에 하나만 실행한다. 기존 dirty 문서·benchmark·Justfile·CI 파일은 다른 작업 소유다.

W0에서 고정할 연결 계약은 다음 세 가지뿐이다.

- A: 필수 `preflight_contract(pin, semantic_contract) -> Result<(), CoreError>` port와
  provider 없는 모델 투영. target/base 검사 및 completed-target 복구 예외를 함께 전달한다.
- C5: `allocate(repo, revision, event)`의 원 번호/incarnation 응답과 검증된 최종 body의
  1회 binding. 명시적 generation의 기존 `reserve_source_event(binding)`도 같은 점유 admission 사용.
- C5 unresolved target: 선발급의 generation-only 점유와 bound의 exact identity를 표현한다.
  fake manifest digest를 넣지 않는다. wire에는 내부 catalog 타입을 노출하지 않는다.

최초 counter 생성에 필요한 양 adapter의 순수 empty/점유 검사는 W0에서 별도로 고정한다.
Core `domains/generation.rs`의 최소 read-only 경계로 제공하고 sealed scan과 구분한다.
디렉터리 이름을 catalog가 직접 읽거나 sealed-only 결과를 empty로 투영하지 않는다.
이를 소유하는 adapter 구현과 모든 test double이 연결돼야 C5 발급을 활성화한다.

| 단계 | A | D | C5 | 통합 |
| --- | --- | --- | --- | --- |
| W0 | preflight port/모델 투영 고정 | gate call graph/panic oracle 확인 | allocate/bind/selector shape 고정 | 소유권 확인, 공유 exports·wire shape 고정 |
| W1 | A1 persisted 계약 → A2 직접 preflight/adapter → A3 회귀 | D1 panic oracle → D2 acquire/GC gate → D3 publish/replay/activation 회귀 | C1 Core/selector → C2 envelope/high-water → C3 allocate/bind/restart 회귀 | I1 control DTO/codec → I2 Admin/native/SDK binding. catalog 완성을 기다리지 않음 |
| W2 | paired 연결 요구 인계 | 독립 구현 제출, 다른 lane 검토 가능 | explicit writer admission 요구 인계 | I3 공용 ingest에 A 후 C5 연결, 공용 test double/composition 수정 |
| W3 | 먼저 비는 A/D lane이 SDK facade/consumer 회귀 담당 | SDK 담당이 아니면 lifetime/lock order 검토 | 점유·restart·binding 회귀 보완 | I4 집중 테스트 → 영향 daemon → fresh-binary SDK L2 직렬 실행 |

SDK 작업은 W3까지 대기시키지 않는다. I1 wire가 고정되고 A/D 중 한 lane의 독립 구현이
제출되는 즉시 SDK 파일을 그 lane에 인계한다. 그동안 통합 담당은 I3를 진행한다.
아직 빈 lane이 없으면 통합 담당이 I2에서 SDK를 작성한다. 같은 파일의 동시 writer는 두지 않는다.

핵심 의존성은 C1→I1이며, 이후 C2/C3와 I2/SDK는 병렬이다. I3는 A2와 C3 계약을 소비한다.
D는 독립 진행한다. C5의 점유 증명이 막혀도 A/D 구현·집중 검증은 계속한다.

D의 publish compile은 gate 밖에서 하고 최종 설치 전에 gate 안에서 replay/pin을 재확인한다.
이미 gate를 잡는 `commit_activation`의 바깥 caller에서 같은 mutex를 다시 잡지 않는다.
I3는 같은 state root에 쓰는 실제 production ingress를 확인해 첫 physical mutation 전에 점유를
기록한다. 그 증명이 없는 우회 경로를 sealed 목록만으로 안전하다고 처리하지 않는다.

각 handoff는 변경 파일, 호출 서명, 거절 조건, 회귀 selector와 미실행 항목만 전달한다.
공용 파일에 대한 병렬 patch·동시 workspace format·공용 schema/API 변경은 하지 않는다.

## Ownership

아래 write set을 고정하며 착수 전 현재 dirty hunk와 writer를 확인한다.

- A: `crates/quanta-index-semantic/` 전체(`src/lib.rs` adapter impl 포함),
  Core `domains/semantic/`와 `tests/semantic_stream.rs`, search-plane `semantic_derive.rs`,
  `ingest_dispatcher/semantic.rs`와 `ingest_dispatcher/tests/semantic.rs`.
- D: `crates/quanta-index-repomap/`와 해당 테스트.
- C5: Core `domains/source_publication.rs`, search-plane `readiness/activation_catalog*`,
  `search_corpus_lifecycle/pair_lock.rs`의 unresolved-target port/type,
  필요한 `search_corpus_history.rs` selector 소비와 catalog 테스트,
  lexical `adapter_lifecycle.rs`의 순수 점유 검사와 owning 회귀.
- 통합: 공용 exports/API inventory/Contract control·split·codec, control dispatcher,
  `ingest_dispatcher/{dispatcher,search_corpus,generation_plan}.rs`, 공용 테스트
  `ingest_dispatcher/tests/{support,search_corpus,idempotency}.rs`, composition root와 필요한 Cargo 변경.
  실제 match인 SDK `binding.rs`, searchd `app/ipc_dispatcher.rs`와 native admission도 포함한다.
  SDK `lexical.rs`/`binding.rs`/전용 consumer 테스트는 wire 고정 후 빈 A/D lane에 일괄 인계할 수 있다.
  Core `domains/generation.rs`의 점유 port와 공용 `lib.rs` exports는 통합 전용이다.
  semantic `src/lib.rs`는 A의 독립 변경 제출 전까지 A 전용이다. 그 뒤 C5 점유 검사 impl을
  해당 파일의 writer와 함께 명시적으로 인계한다. C5와 A가 동시에 수정하지 않는다.
  Core/search-plane 공용 `lib.rs`는 통합 전용이다.

공유 파일은 단일 writer가 수정한다. handoff 전 preimage를 확인하고, C5는
catalog/core/control/ingress/SDK가 연결된 전체 변경으로 수용한다. 독립 A/D 검증에 외부
Semantica build를 선행시키지 않는다.

## Main 통합 단위

| 묶음 | 포함 파일/경계 | main 수용 조건 |
| --- | --- | --- |
| D | RepoMap store/pin과 lifetime/candidate owner 회귀 | 실제 pin/unwind oracle, acquire/GC 및 publish/replay/activation 경쟁 검증 |
| A | semantic persisted 계약/adapter/Core port, 모델 투영, paired ingest/test double | 필드 round-trip·비호환 무부작용·동일 계약 delta/full rebuild·복구 회귀 |
| C5 | allocator/envelope/Core selector, control DTO/codec/admission, 공용 ingest, SDK/consumer | 번호 점유·영속 재시도·최종 binding·실제 UDS/restart/explicit CAS 전체 검증 |
| 통합 검증 | 영향 build/API inventory, daemon, 새 binary SDK L2 | 동일 통합 소스에서 직접 회귀 통과, 실행하지 못한 항목 명시 |

D/A는 검증을 먼저 마친 순서대로 통합한다. C5는 독립 branch에서 완성하되 최신 main의 A/D를
흡수한 뒤 통합한다. API만 노출하거나 catalog만 연결한 C5 중간 상태를 main의 완료 결과로
올리지 않는다. 문서 정리·benchmark 변경과 섞어 commit하지 않는다.

각 묶음 통합 전 main과 실제 diff를 대조하고, 해당 파일의 dirty 소유권을 확인한다. 이미 포함된
패치는 재적용하지 않는다. 통합 뒤에는 영향이 달라진 검사만 재실행하고 마지막 통합 HEAD에서
daemon/SDK L2를 직렬 실행한다. 검증되지 않은 public match, test double, API inventory 갱신을
후속 TODO로 남기지 않는다. C5 전용 schema/API의 공용 파일은 끝까지 통합 담당 단일 writer다.

## 이번 범위에서 제외

- RepoMap GC 후 object-free 역사적 receipt 복구와 catalog/envelope alias 추가 hardening.
- C5 publication/activation 이중 상태 축, 자동 Superseded/cancel, rollback과 예약의 원자 갱신,
  root 전체 restore reconciliation, retention 정책 재설계.
- 다중 모델 migration, RCU/새 registry/별도 journal, Loom·새 simulator·외부 fault platform 도입.
- 장기 event-history rollover, 일반 성능 최적화, 새 자원 budget, source format/config 확대.

제외 항목을 현재 안전성이 증명된 것으로 표시하지 않는다. 기존 fail-closed/typed busy·보존
계약을 유지하며, 구체적 제품 요구나 재현된 결함이 있을 때 별도 범위로 판단한다.
정식 benchmark·release·외부 producer 수용은 이번 수정의 완료 gate에 넣지 않는다.
