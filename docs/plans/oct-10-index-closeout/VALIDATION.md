# 검증 계획

[최종 구현안](README.md)의 직접 변경에 대한 검증만 관리한다. 아래 코드 검증은 모두
`NOT_RUN`이다. 문서 검사·컴파일·owner 테스트·실제 daemon 결과는 구분한다.

## Semantic compatibility

| 시나리오 | 합격 조건 |
| --- | --- |
| 같은 차원 모델 A→B 또는 revision/normalization/metric/policy 변경 | 유효하지만 비호환인 계약은 기존 typed compatibility refusal. provider/첫 window·신규 최종 event binding·target 변경 0 |
| 기존 target와 delta base | 직접/paired 경로에서 둘 다 검사. base marker/manifest와 manifest/build contract 불일치 거절 |
| persisted format/필수 key | 구형은 typed rebuild refusal. 현 형식의 key 누락은 corruption. 명시적 optional None과 누락을 구분 |
| 같은 계약 delta와 모델 변경 replacement | 고정 source/chunk/membership oracle 또는 독립 full rebuild와 결과 일치. 다른 모델 벡터 혼합 없음 |
| 기존 query/replay/finalize 복구 | 기존 query ID/revision gate 유지. 원 generation/digest/roots/sequence와 replay applied=false 유지. 재임베딩/retarget·완료 target의 GC된 base 재요구 없음 |
| preflight 뒤 경합 | operation lock 안에서 재검사. stale 계약으로 provider/최종 binding/track mutation 진행 없음 |
| 첫 window보다 이른 거절 | 비호환 fixture의 `next_window`/provider는 호출 시 실패하도록 설정. 거절 후 호출 횟수 0 및 기존 target bytes 불변 |
| 모델·정책별 독립 oracle | 같은 dimension에서 ID/revision/metric/normalization/embedding policy/view policy/corpus policy를 한 필드씩 변경. 허용되는 새 generation full replacement와 분리 |

이미 별도 요청으로 발급된 C5 번호와 기존 immutable Prepared journal은 거절 뒤에도
보존할 수 있다. publish 거절이 선발급을 취소한다는 oracle을 쓰지 않는다.

## RepoMap lifetime

| 시나리오 | 합격 조건 |
| --- | --- |
| acquire↔retire/GC | barrier로 두 순서 고정. 성공한 pin의 physical object 유지, 실패한 acquire는 typed refusal |
| G1 pin 보유 중 G2 activation/GC | 기존 G1 view 유지, 신규 retired acquire 거절, 마지막 drop 후 reclaim |
| publish↔acquire/GC | 최종 설치와 reclaim의 중간 상태 노출·pin 객체 unlink 없음 |
| 의도한 panic/조기 return/복수 pin | acquire 성공·pin=1·본문 진입·정확한 panic payload·unwind pin=0을 확인. 마지막 pin 전 reclaim 없음 |
| lock order·기존 activation/replay/durability | gate 안 재진입/역순 lock 없음. query/Drop은 gate 밖. 기존 수용 범위의 회귀 통과 |
| 경쟁 회귀의 감도 | read-to-pin에서 acquire를 멈춘 뒤 retire/GC를 진행시키는 순서를 고정. 성공 acquire의 object 부재를 검출하며 원래 공백이 있는 구현을 통과시키지 않음 |
| replay 경쟁 | 이미 존재하는 정확한 publish의 object 검증과 GC가 gate로 직렬화. GC 후 새 역사적 receipt 성공을 요구하지 않음 |

기존 false-positive를 먼저 고친다. sleep 반복이나 catch_unwind 오류 여부만으로 PASS하지 않는다.
GC 후 역사적 receipt 복구, 복수 독립 store handle, 새 corruption/alias 정책은 검증 범위가 아니다.

## Generation reservation

| 시나리오 | 합격 조건 |
| --- | --- |
| 최초 선발급/같은 event retry/restart | persist 뒤 응답, 동일 generation과 원 binding 복구. 번호만 발급됐을 때 publish/activation 성공으로 표시하지 않음 |
| 같은 identity의 다른 revision/payload/source base | typed conflict, 원 예약 불변 |
| 기존 bound event의 cross-revision publish replay | 새 allocation 요청의 revision conflict와 구분. 기존 publication은 원 revision/generation/binding을 복구하며 기존 회귀를 유지 |
| 서로 다른 허용 stream의 동시 예약 | unique 번호. 같은 stream unresolved 및 오래된 event의 기존 typed busy/순서 제한 유지 |
| 양 track 점유·explicit writer·rollback·GC | active+1 사용 안 함. 예약 target 탈취 없음, high-water 비감소와 gap 허용 |
| 최종 body binding/crash/ACK 유실 | 검증된 digest/key에 1회 연결. 원 event 재시도로 복구, 다른 body/key 재결합·암묵 retarget 없음 |
| capacity/overflow/저장 불확정/구형 형식 | typed 오류, 번호 재사용·누락 high-water 기본값 대입 없음 |
| 선발급 뒤 active head 변경/rollback | 원 예약 번호 유지. 예약에 CAS가 없음. publication 뒤 caller의 명시적 activation CAS와 기존 generation/순서 검사를 따름 |
| activation CAS conflict | 기존 typed conflict와 sealed publication 보존. SDK의 자동 expected-head recapture 없음 |
| 제어된 restore | 저장된 예약 incarnation과 현재 값 불일치 시 binding 거절. 자동 예약 취소·stream 해제·root-wide 복구 없음 |
| unresolved 보호 | 선발급 상태도 기존 selector에서 보호. timeout/GC로 event 또는 번호 자동 재사용 없음 |
| digest 없는 선발급·미완성 점유 | fake digest 없이 generation 점유를 보호. bound에는 기존 exact digest 검사 유지. sealed-only floor를 허용하지 않으며 미추적 점유 확인 불가 시 typed refusal |
| 실제 control/SDK/daemon | Admin/native admission, canonical codec literal/negative, 실제 match와 오류 보존, daemon restart 후 원 예약 복구 |
| source fingerprint의 target 독립성 | 기존 `source_event_payload_sha256`에서 generation/containing revision/manifest 변경은 같은 source digest. source mutation 변경은 다른 digest. 새 allocation revision conflict는 별도 검사 |
| high-water 최초 생성 | 양 track에 미완료 directory/점유가 남았는데 counter가 없으면 refusal. 진짜 빈 pair만 최초 counter 생성. 누락 필드를 0으로 읽지 않음 |
| 명시적 writer 선행/경쟁 | explicit publish가 큰 번호를 점유하면 다음 allocation은 그 위의 번호. allocation이 먼저 점유한 target은 다른 event의 explicit publish가 탈취하지 못함 |
| 저장 장애 위치 | persist 전 실패는 성공 ACK 없음. rename 뒤 parent sync 실패는 uncertainty 유지. durable persist 뒤 ACK 유실은 restart 후 원 번호 반환 |

caller가 생성한 bytes를 그대로 expected로 쓰지 말고 request/response literal을 고정한다.
새 allocation 상태와 기존 Pending/Staged/Active를 함께 검사하며 두 번째 activation 상태 축은 만들지 않는다.

## 체크 순서와 실행 범위

1. D의 기존 false-positive panic oracle을 고정하고 A/D/C5의 직접 실패를 owning test target에서
   재현한다. 구현과 함께 회귀를 통과시키며, 실패하는 test-only 중간 commit을 main에 통합하지 않는다.
2. owning crate의 직접 회귀를 먼저 실행한다. 새 필수 port와 test double, codec 및 실제 exhaustive
   match가 모두 연결된 상태에서 영향 shared-surface build와 기존 API inventory를 검사한다.
3. D/A의 독립 green bundle은 먼저 통합할 수 있다. C5 전체를 최신 main에 통합한 뒤 영향 daemon을
   실행한다. Rust build/native test는 동시에 여러 개 시작하지 않는다.
4. 마지막 통합 소스로 daemon binary를 새로 build하고 기존 SDK L2에서
   allocate→prepare/freeze→publish→explicit activation, ACK 유실/restart를 검증한다.
   번호 발급 ACK, publish receipt, activation ACK를 각각 확인하며 한 결과로 다른 단계를 대체하지 않는다.

unit/in-process owner test 성공은 실제 UDS/restart 성공을 의미하지 않는다. 새 simulator나 formal
benchmark를 추가하지 않고 아래 기존 target과 SDK L2를 사용한다.

## Affected checks

변경 후 가장 좁은 결정적 회귀부터 실행한다. 아래는 기존 target이며 새 catalog/IPC 회귀의
정확한 selector는 구현 시 owning target에 추가한다. 무거운 실행은 직렬화한다.

```sh
./scripts/cargow --lane test-daemon-lane test -p quanta-index-semantic --test persisted_semantic --all-features --locked
./scripts/cargow --lane test-daemon-lane test -p quanta-index-semantic --test generation_delta_reuse --all-features --locked
just rust-profile test-read-view-lifetime-owner
just rust-profile test-candidate-activation-owner
./scripts/cargow --lane test-daemon-lane test -p quanta-index-core --lib domains::source_publication::tests --all-features --locked
./scripts/cargow --lane test-daemon-lane test -p quanta-index-search-plane --lib readiness::activation_catalog::tests --all-features --locked
./scripts/cargow --lane test-sdk-binding-owner-lane test -p quanta-index-contract-base -p quanta-index-contract -p quanta-index-sdk --all-features --locked
```

공유 port/wire 변경에는 영향 build와 `just rust-public-api`, 실제 boundary 변경이 있으면
`just rust-hexagonal rust-module-discipline rust-cargo-modules`를 실행한다.
공유 ingest/control 변경에는 `just rust-profile test-daemon`과 아래 fresh-binary SDK L2를 실행한다.

```sh
./scripts/cargow --lane test-daemon-lane build -p quanta-index-searchd-runtime --bin quanta-index-searchd --all-features --locked
./scripts/cargow --lane test-sdk-binding-owner-lane test -p quanta-index-sdk --test l2_daemon_publication --all-features --locked -- --ignored --test-threads=1
```

실제 build artifact를 확인해 `QUANTA_INDEX_L2_TEST_BINARY`로 지정한다. 이전 binary를 쓰지 않는다.
기존 L2에 예약→publish→명시적 activation 및 restart 회귀를 붙이며 별도 harness는 만들지 않는다.

## Quality inputs

정식 품질 평가는 이번 코드 수정의 gate에서 제외한다. 기존
[review admission](../../adr/OCT-05-001-review-admission-and-result-identity.md)의 독립 gold/입력과
실제 실행이 필요하며 현재 계획에서 새 품질 결과는 `NOT_RUN`이다.

## Performance inputs

일반 성능 최적화·정식 비교는 이번 범위에서 제외한다. 비용 문제를 실제로 측정한 뒤
[qualification 계약](../../adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md)을 적용한다.
이번 문서 수정의 성능 검증은 `NOT_RUN`이다.

## Release and consumer

외부 producer 전체 수용과 운영/release는 이번 gate에서 제외한다. 기존
[Source SDK 계약](../../adr/OCT-04-003-source-preparation-sdk.md)과
[운영 계약](../../adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md)을 따른다.
Index SDK L2 성공을 외부 producer/whole-product 완료로 승격하지 않으며 해당 실행은 `NOT_RUN`이다.

## Final acceptance

- A/D/C5의 위 직접 회귀와 영향 build/daemon/SDK L2가 통과해야 코드 변경을 수용한다.
- C5 catalog/control/ingress/SDK의 부분 구현이나 enum compile만으로 완료 처리하지 않는다.
- 외부 producer 전체 수용·정식 품질/성능·운영/release는 이번 gate에서 제외한다.
  해당 결과를 주장할 때 기존 [비용·qualification 계약](../../adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md)을 따른다.
- 각 실행에 변경 범위, 명령, 관측 결과와 `VERIFIED`/`FAILED`/`BLOCKED`/`NOT_RUN`/
  `NOT_APPLICABLE`를 기록한다. 일회성 log/receipt는 저장소에 추가하지 않는다.
