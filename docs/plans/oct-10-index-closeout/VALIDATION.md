# 검증 계획

[최종 구현안](README.md)의 변경과 후속 통합 소스 검증을 관리한다. 현재 결과는
마지막 실행 절을 따른다. 앞선 절은 각 실행 당시의 기록이며, 별도 실행
결과가 명시되지 않은 코드 검증은 `NOT_RUN`이다. 문서 검사·컴파일·owner 테스트·실제
daemon 결과는 구분한다.

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
| lexical overlay 선행 | 미예약 target의 6개 overlay apply는 재시도 가능한 `NotReady`, port 호출·물리 쓰기 0. 실제 source 예약 후 동일 target의 overlay→source seal 허용. 원 terminal replay와 sealed-target refusal 유지 |
| 저장 장애 위치 | persist 전 실패는 성공 ACK 없음. rename 뒤 parent sync 실패는 uncertainty 유지. durable persist 뒤 ACK 유실은 restart 후 원 번호 반환 |

caller가 생성한 bytes를 그대로 expected로 쓰지 말고 request/response literal을 고정한다.
새 allocation 상태와 기존 Pending/Staged/Active를 함께 검사하며 두 번째 activation 상태 축은 만들지 않는다.

## Lexical bundle admission

| 시나리오 | 합격 조건 |
| --- | --- |
| malformed CBOR 및 과거 `b"manifest"` sentinel | 두 preflight phase와 typed build 모두 metadata decoder의 `InvalidContract`. 각 호출 직후 state root에 파일이 없음 |
| raw replacement 뒤 malformed full bundle | 앞선 replacement도 쓰지 않고 전체 요청을 `InvalidContract`로 거절. state root는 비어 있음 |
| 유효한 metadata 뒤 명시적 empty payload | 실제 metadata 파일 생성 후 제거. generation directory는 보존 |

기존 `l2_file_mutation`의 `bundle_` 회귀 세 개를 아래 중앙 실행 배치에 포함한다.
이 rail은 lexical adapter admission/storage를 검증한다. source catalog 예약, publication journal 및
daemon 결과는 별도 owning rail로 검증하며 이 테스트의 성공으로 대체하지 않는다.

## 체크 순서와 실행 범위

1. 계약·인터페이스를 먼저 고정하고 정적으로 검토한다. A/D/C5 모듈과 owning test target의
   회귀 코드를 병렬로 작성한다. 담당 간 메시지·재위임과 worker의 build/test 실행은 금지한다.
2. 부모가 모든 모듈과 공용 port/test double/codec/exhaustive match를 통합한다.
   영향 shared-surface compile, API inventory 및 정적 검사를 먼저 실행한다.
3. 통합 소스의 직접 회귀와 영향 daemon 검증을 부모가 일괄 실행한다.
   Rust build/native test는 동시에 여러 개 시작하지 않는다.
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
./scripts/cargow --lane test-daemon-lane test -p quanta-index-lexical --test l2_file_mutation bundle_ --all-features --locked
./scripts/cargow --lane test-daemon-lane test -p quanta-index-contract-base -p quanta-index-contract -p quanta-index-sdk --all-features --locked
```

공유 port/wire 변경에는 영향 build와 `just rust-public-api`, 실제 boundary 변경이 있으면
`just rust-hexagonal rust-module-discipline rust-cargo-modules`를 실행한다.
공유 ingest/control 변경에는 `just rust-profile test-daemon`과 아래 fresh-binary SDK L2를 실행한다.

```sh
./scripts/cargow --lane test-daemon-lane build -p quanta-index-searchd-runtime --bin quanta-index-searchd --all-features --locked
./scripts/cargow --lane test-daemon-lane test -p quanta-index-sdk --test l2_daemon_publication --all-features --locked -- --ignored --test-threads=1
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

C5 선발급 API는 source-first 명시적 generation publish에는 선택사항이다.
overlay-first producer는 완성된 source intent로 먼저 `reserve_generation`을 호출해야 한다.
기존 명시적 generation publish도 같은 durable 점유·high-water admission을 통과한다.
Semantica의 aggregate/replay generation을
선발급 번호로 바꾸는 작업을 이번 잔여 작업으로 추가하지 않는다.

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

## 2026-10-10 선행 통합 실행 결과

다음 결과는 후속 lexical 저장소 통합 전 실행 소스에 해당한다. 현재 통합 소스의 상태는
마지막 절에서 별도로 기록하며, 선행 통과 결과를 자동으로 승계하지 않는다.

현재 변경 작업 트리에서 실행했다. worker는 코드와 정적 검토만 제출했으며 아래 build/test는
부모가 공용 자원 잠금 아래 직렬 실행한다. 정식 replay/release/benchmark qualification 결과가 아니다.

| 검사 | 명령/범위 | 결과 |
| --- | --- | --- |
| 영향 compile | `./scripts/cargow --lane test-daemon-lane check` + 아래 영향 package 집합, `--all-targets --all-features --locked` | `VERIFIED` |
| 영향 Clippy | 같은 package 집합과 lane, `clippy --all-targets --all-features --locked -- -D warnings`; 마지막 실행은 `QUANTA_INDEX_SCCACHE=0` | `VERIFIED`; 최종 SDK/concurrency fixture 포함. Vendor Tantivy의 기존 warning은 별도 |
| 포맷/whitespace | 변경 Rust 파일 `rustfmt --check --edition 2024`, `git diff --check` | `VERIFIED` |
| 구조/inventory | `just rust-hexagonal rust-module-discipline rust-cargo-modules rust-no-allow`, `python3 tools/ci/lint/check-wire-inventory.py` | `VERIFIED` |
| public API | `just rust-public-api` | `VERIFIED`; Contract/SDK baseline 갱신 후 일치 |
| 생성 문서 | `python3 tools/prompt-manager/pm.py lint` | `VERIFIED`; 5 target 일치 |

영향 package 집합: `quanta-index-{contract,core,lexical,ipc,semantic,repomap,search-plane,sdk,searchd-runtime}`.

### Unit / 직접 회귀

최초 unit 배치는 malformed semantic 입력을 더 일찍 거절하면서 publish 본문 진입이
1회에서 0회가 된 기존 기대값 차이로 `FAILED`였다. 입력·거절 메시지·무부작용 oracle을
유지하고 본문 진입 기대값만 수정했다.

| 명령/범위 | 관측 결과 | 최종 상태 |
| --- | --- | --- |
| `./scripts/cargow --lane test-daemon-lane nextest run -p quanta-index-search-plane --lib --all-features --locked -E 'test(/^ingest_dispatcher::tests::/)' --test-threads 2 --no-fail-fast` | 70 통과, selector 밖 482 제외 | `VERIFIED` |
| 같은 lane의 `nextest run` + `quanta-index-{contract-base,contract,core,lexical,semantic,repomap,search-plane,sdk}` + `--lib --all-features --locked --test-threads 2 --no-fail-fast` | 1,615 실행: 1,614 통과, cluster tombstone fixture 1 실패; ignored 1 미실행 | `FAILED`; 아래 소유 범위 재검사로 수정 확인 |
| `./scripts/cargow --lane test-daemon-lane nextest run -p quanta-index-semantic --lib --all-features --locked --test-threads 2 --no-fail-fast` | 89 통과, 제외 0 | `VERIFIED` |
| 위 8개 package 전체 lib 배치 재실행; lexical overlay admission 수리 포함 | 1,616 실행, 1,616 통과; ignored 1 미실행 | `VERIFIED` |

cluster tombstone fixture는 base의 `policy:cluster:v1`를 delta에서도 명시하도록 수정했다.
`required_corpora`는 여전히 빈 목록이며, varying header 전체 equality를 요구하지 않는다.
마지막 행은 수정 후 실제 전체 lib 배치의 결과다. 이는 해당 8개 package lib 범위이며
workspace 전체 suite나 별도 daemon/SDK L2 결과를 뜻하지 않는다.

### Owner / boundary

| 명령/범위 | 관측 결과 | 최종 상태 |
| --- | --- | --- |
| `python3 tools/ci/run-local-test-scope.py integration-semantic read-view-lifetime-owner candidate-activation-owner --lane test-daemon-lane` | 11 binary, 131 통과, 제외 0 | `VERIFIED` |

최초 owner 실행은 missing-base 진단이 `absent`에서 `not sealed`로 바뀌어 실패했다.
순수 preflight에서 base 디렉터리 부재를 먼저 확인하도록 수정해 기존 오류 구분을 복원하고
같은 전체 owner scope를 재실행했다. 비호환/미완료 base는 계속 별도 조건으로 거절한다.

| 명령/범위 | 관측 결과 | 최종 상태 |
| --- | --- | --- |
| `./scripts/cargow --lane test-daemon-lane nextest run -p quanta-index-core -p quanta-index-lexical -p quanta-index-ipc --test semantic_stream --test l2_file_mutation --test admission --test socket_access --all-features --locked --test-threads 2 --no-fail-fast` | 51 통과, 제외 0 | `VERIFIED` |

### Daemon / SDK L2

최초 `just rust-profile test-daemon`은 300개 선택 중 29 통과, 1 실패, 270 미실행,
ignored 10이었다. `a_torn_overlay_is_refused_by_restart_and_by_the_query_door`의 setup이
source 예약 전에 lexical overlay 디렉터리를 만들어 최초 pair의 empty admission과 충돌했다.
이는 실제 writer admission 누락으로 분류해 6개 lexical overlay ingress와 harness 순서를
수정한다. empty 검사를 완화하거나 물리 디렉터리에서 high-water를 추정하지 않는다.
이 실행은 `FAILED`였다. Overlay 수정 후 두 번째 실행은 301개 중 111 통과, 1 실패,
189 미실행, ignored 10이었다. 성공용 fixture 9곳의 과거 `b"manifest"` sentinel을
현재 lexical admission이 거절했다. 해당 fixture는 기존 serializer로 명시적 CBOR metadata를
보내도록 바꾸고 admission은 유지했다. 세 번째 실행은 301개 중 134 통과, 1 실패,
166 미실행, ignored 10이었다. 실제 SDK binary fixture도 overlay를 source 예약 전에
보내고 있어 거절됐으며, 정적 검색으로 SDK matrix와 activation concurrency fixture의 같은
호출 순서도 확인했다. 실제 source intent 선예약을 추가한다. 다음 daemon 실행은 같은 catalog
scope에 `--no-fail-fast`를 붙여 전체 결과를 수집한다. fresh-binary SDK L2는 아직 `NOT_RUN`이다.

위 다음 배치는 301개를 시작했으나 대화 중단 시점에 실행이 종료되어 final summary가 없다.
완료 여부를 추정하지 않으며 daemon 완료 증명은 `NOT_RUN`이다. 이후 동일 작업 트리에
별도 lexical 저장소/authority 변경이 들어왔다. 위 통과 수는 각 실행 당시 소스의 관측 결과이며,
현재 통합 소스에 자동 승계하지 않는다. 재개 후 영향 compile과 daemon/SDK L2를 재검사한다.

## 통합 소스 재개 초기 검사

동일 작업 트리에 들어온 paged lexical 저장소/authority 변경을 포함한다. A/D/C5의
세 worker는 읽기 전용 정적 검토만 수행했고 확정 결함은 보고하지 않았다. 실행 검증은
공용 resource-admission 잠금 아래 직렬화한다. 최초 compile 대기는 300초 만료로
`BLOCKED`였고, 재실행에서 새 retention fixture의 버려진 `fetch_add` 반환값을 발견했다.
반환값 바인딩 한 줄을 추가한 뒤 같은 compile 범위는 통과했다.

별도 공용 lib/integration 배치는 72개 binary에서 2,346개를 실행해 2,345 통과,
1 실패, ignored 17이었다(`FAILED`). 실패는 `vector_index_contract`의 구형 manifest
fixture가 신규 policy 필드를 남겨 둔 채 과거 21필드 형태를 검사한 경우다.
구형 format 7/8 생성 시 신규 `policy_digest`/`view_policy_digest`를 제거하고
기존 21필드 및 typed refusal oracle을 유지했다. 공용 집중 재검사의 해당 회귀는 통과했다.
이 배치는 실행 중 lexical bootstrap 소스가 추가 변경되어 현재 전체 소스의 통과 결과로
승계하지 않는다. 영향 Clippy의 lexical diff or-pattern lint 1개도 동일 의미의 중첩 패턴으로
수정하고 같은 범위를 재실행한다.

| 검사 | 현재 결과 |
| --- | --- |
| 영향 9개 package `check --all-targets --all-features --locked`, `test-daemon-lane`, `QUANTA_INDEX_SCCACHE=0` | `VERIFIED`; lexical bootstrap 후속 수리 포함, 최종 재검사 3분 14초 |
| 변경 Rust 파일 183개 `rustfmt --check --edition 2024 --config skip_children=true`, `git diff --check` | `VERIFIED`; SDK L2 fixture, Contract timer 설명 및 Large fixture의 오류 보존 수정 포함 |
| `just rust-hexagonal rust-module-discipline rust-cargo-modules rust-no-allow` | `VERIFIED`; facade 41개 및 module baseline 일치 |
| `python3 tools/ci/lint/check-wire-inventory.py`, `python3 tools/prompt-manager/pm.py lint` | `VERIFIED`; 생성 문서 5 target 일치 |
| 영향 9개 package Clippy / public API | `VERIFIED`; `--all-targets --all-features --locked -- -D warnings`, Contract/SDK baseline 일치. 후속 SDK L2 fixture도 `clippy -p quanta-index-sdk --test l2_daemon_publication --all-features --locked -- -D warnings` 통과 |
| `quanta-index-{contract,semantic,repomap,search-plane,sdk}` lib, 같은 lane의 `nextest run --lib --all-features --locked --test-threads 2 --no-fail-fast` | `VERIFIED`; 5 binary, 1,037 실행·통과, 제외 0 |
| canonical `integration-semantic read-view-lifetime-owner candidate-activation-owner`, `test-daemon-lane`, `--no-fail-fast` | `VERIFIED`; 11 binary, 131 실행·통과, 제외 0 |
| Core/lexical/IPC의 `semantic_stream l2_file_mutation admission socket_access`, `nextest run --all-features --locked --test-threads 2 --no-fail-fast` | `VERIFIED`; 4 binary, 52 실행·통과, 제외 0 |
| canonical daemon 5 binary, 같은 lane의 `nextest run --all-features --locked --test-threads 4 --no-fail-fast` | `FAILED`; 304 실행, 303 통과, Large restart 1 실패, ignored 10. 616.939초 |
| 실패한 Large restart의 같은 target·정확한 selector, `--test-threads 1 --no-fail-fast` | `VERIFIED`; 1 실행·통과, selector 밖 97 제외. 277.914초. 300초 IPC 제한과 source/pagination/restart oracle 유지 |
| 최종 Large fixture Clippy / nextest 설정 | `VERIFIED`; `clippy -p quanta-index-searchd-runtime --test runtime_extended_suite --all-features --locked -- -D warnings`; 같은 target의 `nextest list -E 'test(=e2e_process_readiness::binary_large_scoped_corpus_restart_preserves_every_source_and_ranked_page)' --message-format json` 설정 수용 및 정확히 1개 선택 |
| 공용 fresh-binary SDK L2, `nextest run -p quanta-index-sdk --test l2_daemon_publication --all-features --locked --run-ignored all --test-threads 1 --no-fail-fast` | `VERIFIED`; 6 실행·통과, 제외 0. 53.352초. 예약/restart/명시 activation 및 crash cut 8개 포함 |

SDK L2의 선행 6개 실행은 paged timer subclock과 lexical object-pool directory에 대한
과거 fixture oracle 때문에 4 통과·2 실패였다. 세 legacy subclock의 `None`과 실제
enclosing clock을 검사하고, canonical generation pair와 object-pool을 구분했다.
source/query/receipt/lineage·generation 존속/GC oracle은 유지했다. 후속 실행은 실제
fresh daemon binary의 생산 소스가 통합 생산 소스와 같음을 확인한 범위이며, 그 사이 변경은
SDK fixture와 Contract 설명뿐이다. 재시작/activation 결과를 외부 producer 수용으로 승격하지 않는다.

daemon의 유일한 실패는
`e2e_process_readiness::binary_large_scoped_corpus_restart_preserves_every_source_and_ranked_page`다.
431.619초에 fixture Drop이 `HardDeadlineEscalated { unfinished: ["ingest-accept"] }`로
실패하면서 선행 `Result` 오류를 가렸다. 대규모 fixture의 setup 결과와 explicit stop 결과를
함께 반환하도록 수정했다. 300초 IPC 제한과 4,096 source·ranked-page oracle을 유지한
단독 재검사는 277.914초에 통과했다. `.config/nextest.toml`의 기존 resource isolation
방식으로 이 회귀에 `threads-required = "num-test-threads"`를 지정했다. 다른 corpus
E2E와 동시에 실행하지 않으며, 운영/client deadline이나 정확성 oracle은 늘리거나 제거하지 않았다.
실행 증거는 최초 배치의 303 통과와 실패 회귀의 후속 단독 1 통과다. 전체 304개를 수정 후
한 배치에서 다시 실행한 것으로 표현하지 않는다. ignored 10은 여전히 미실행이다.
A/D/C5의 직접 변경과 위 영향 검증 범위는 수용한다. workspace 전체 suite, 별도 lexical
성능 수용, 외부 producer, 정식 품질·성능 비교, release 및 remote CI는 이 결론에 포함하지 않는다.

최종 source 대조에서 별도 lexical 후속 변경을 확인했다:
`lexical_store/{map,store}.rs`, `text_authority/{paged_verify,paged_writer}.rs`의
decoded-page cache와 seal current-row 중복 조회 제거다. A/D/C5 직접 코드 변경은 없었다.
위 실행 결과는 각 실행 입력 소스에 해당하며, 뒤에 추가된 이 네 파일의 최종 내용까지
검증했다고 승계하지 않는다. 현재 작업 트리 전체의 runtime qualification은 별도다.
해당 변경의 static/runtime/performance 결과는 lexical 계획의 후속 owning rail에서 관리한다.

위 표는 재개 초기의 source 범위다. 이후 paged 저장·조회·복구와 Native bootstrap
수리가 추가됐으며 위 상태를 그 후속 source에 자동 승계하지 않는다. paged 후속의
실행별 실패·수리·회귀와 Large/XL 비용은
[LEXICAL-QUERY-PLAN.md](LEXICAL-QUERY-PLAN.md) 10절에 모은다.
새 초기 구축, 최종 owner, SDK 실제 프로세스 및 성능 수용 여부는 해당 기록의 실제
완료 결과를 따른다. 집중 테스트 통과를 전체 제품 또는 검색 품질 수용으로 승격하지 않는다.

## Native body 수리 이전 workspace 전체 검증 (2026-10-10)

HEAD `8f11cad9587aec1668a9fb3f5f1f055337487071`의 미커밋 통합 작업 트리에서 실행했다.
기본 workspace 배치 직전에 확인한 Rust/Cargo 파일 1,229개의 내용은 배치·doctest·SDK
L2 종료 후에도 변경되지 않았다. paged Native/File authority, decoded-page cache, 독립
File proof, seal의 current/BlobRef 단일 조회, observer 분리와 A/D/C5를 모두 포함한다.
앞선 초기 검사 결과나 303+1 결과의 승계가 아니라 현재 소스로 실행한 전체 기본 배치다.
읽기 전용 worker의 정적 감사 뒤 build/runtime는 공용 resource-admission 잠금으로
직렬화했다. per-run 출력은 checkout 밖의 `/tmp`에 두고 저장소에는 새 증거 파일을 만들지 않았다.

### 최종 정적 검사

아래 Cargo 명령은 `QUANTA_INDEX_SCCACHE=0`과 `./scripts/cargow --lane test-daemon-lane`
front door를 사용했다.

| 명령/범위 | 실제 결과 |
| --- | --- |
| `check --workspace --all-targets --all-features --locked` | `VERIFIED`; JSON macro recursion 오류의 구조 수리 후 재실행 통과, 1분 58초 |
| `clippy --workspace --all-targets --all-features --locked -- -D warnings` | `VERIFIED`; harness의 must-use/불필요 clone 수리 후 재실행 통과, 2분 24초. vendor Tantivy의 기존 capped warning 8건은 남음 |
| `just fmt-check rust-hexagonal rust-module-discipline rust-cargo-modules rust-module-cycles rust-test-authority rust-wire-inventory rust-no-allow rust-error-shape rust-derive-allowlist rust-ignored-test-policy` | `VERIFIED`; facade 41개 및 error shape 검사 포함 |
| `.venv/bin/python tools/ci/lint/check-public-api.py` (resource admission 아래 실행) | `VERIFIED`; Contract/SDK baseline 일치 |
| `just rust-workspace-lints rust-cargo-toml-hygiene rust-digest-fallibility proof-authority-lint rust-bench-artifacts` | `VERIFIED`; workspace 27개 crate, digest 검사 624개 source·25개 site. proof registry 25개는 `REGISTRY_ONLY`이며 실행 qualification이 아님 |
| `just rust-deny rust-machete` | `VERIFIED`; advisories/bans/licenses/sources 통과, unused dependency 없음. model2vec-rs의 기존 missing-license warning은 남음 |
| `.venv/bin/python -m ruff check .`, `.venv/bin/python -m ruff format --check .` | `VERIFIED`; Python format 417개 파일. 기존 proof-operational test signature의 포맷만 교정 |
| `.venv/bin/python tools/prompt-manager/pm.py lint` | `VERIFIED`; 생성 문서 5 target 일치 |
| `just lint-doc-paths`, `git diff --check` | `VERIFIED`; 삭제된 flat owner를 가리키던 ADR 링크 2개 및 heading anchor 1개 수리 후 통과 |

의존성 검사의 최초 admission 대기는 1,800초에 끝나 명령이 실행되지 않았다(`BLOCKED`).
7,200초 admission wait로 재요청한 실제 명령이 위 통과 결과다. 실행 timeout이나 제품
quota를 늘린 것이 아니다.

### 최종 실행 검사

| 명령/범위 | 실제 결과 |
| --- | --- |
| `.venv/bin/python -m pytest -q tools/ci/tests/test_causal_cost_capture.py tools/ci/tests/test_causal_cost_profile.py --tb=short` | `VERIFIED`; 124개 통과 |
| `./scripts/cargow --lane test-daemon-lane nextest run --workspace --all-features --locked --test-threads 2 --no-fail-fast` | `VERIFIED`; 170개 binary, 4,521 실행·통과, 실패 0, skipped 31, slow 14. 실제 nextest 실행 2,874.452초 |
| `./scripts/cargow --lane test-daemon-lane test --workspace --doc --all-features --locked` | `VERIFIED`; 28개 crate doctest rail, 실제 예제 4개 통과·실패/ignored 0. Contract scalar decode 3개(compile-fail 2개 포함)와 vendor Unicode 1개 |
| `./scripts/cargow --lane test-daemon-lane build -p quanta-index-searchd-runtime --bin quanta-index-searchd --all-features --locked --message-format=json-render-diagnostics` | `VERIFIED`; compiler-artifact의 실제 daemon executable을 고정 복사한 뒤 SDK에서 사용 |
| `QUANTA_INDEX_L2_TEST_BINARY=/tmp/qi-oct10-final-sdk-daemon ./scripts/cargow --lane test-daemon-lane nextest run -p quanta-index-sdk --test l2_daemon_publication --all-features --locked --run-ignored all --test-threads 1 --no-fail-fast` | `VERIFIED`; 실제 process 6개 실행·통과, 제외 0, 58.513초. 예약/restart/명시 activation, 원 event binding, rollback 뒤 crash cut 8개 포함 |

SDK에 사용한 daemon SHA-256은
`afcd48f0845767019646e2b8c325de932b86569421229892ce826c3b73064e64`다.
이는 debug/all-features daemon이며 optimized 성능 binary와 구분한다.
기본 workspace 배치의 orchestration 4,046.82초에는 compile, macOS 최초 binary 수집,
admission이 포함된다. 이를 nextest 실행 시간이나 engine latency로 표시하지 않는다.

기본 배치에서 Large restart 회귀의 두 launcher가 모두 통과했다:
`process_readiness_owner_v1` 186.432초, `runtime_extended_suite` 168.172초.
기존 300초 IPC 제한과 4,096 source·ranked-page·restart oracle을 유지했다.
실행 종료 뒤 `.config/nextest.toml`의 resource isolation을 두 launcher 모두에 적용했고,
nextest 0.9.104의 `nextest show-config version`과 후속 실제 SDK 실행에서 설정을 수용했다.
이 두 launcher isolation을 앞선 workspace 배치가 사용했다고 주장하지 않는다.

observer replay는 boundary-only와 periodic 모두 시작/종료 disk snapshot을 요구한다.
정확한 5개 key, observed의 u64 counter, unavailable의 reason/null counter를 검증하고
유효한 snapshot을 replay 결과에 보존한다. 누락·위조·bool/음수/overflow·혼합 상태를
고정 fixture로 거절하는 회귀와 명시적 unavailable의 통과 회귀가 124개 Python 검사에 포함됐다.

### 수용 범위와 남은 검증

- `VERIFIED`: A/D/C5, 현재 paged/cache/File proof/observer 변경의 정적 검사 및 기본
  workspace 기능 회귀, doctest, 위 6개 SDK 실제 process 회귀.
- Large optimized 결과는 [LEXICAL-QUERY-PLAN.md](LEXICAL-QUERY-PLAN.md) 10.22절의
  `VERIFIED_DIAGNOSTIC`이다. boundary-only와 이전 periodic을 같은 config의 성능
  비교나 수리별 인과적 배속으로 해석하지 않는다. XL은 실제 후속 결과를 따로 기록한다.
- `NOT_RUN`: 위 SDK scope 밖의 기본 ignored 검사, 외부 producer 수용, remote CI,
  정식 품질/성능 비교와 release qualification. proof registry 검사만으로 owner 실행
  publication을 주장하지 않는다.
- 별도 LQ4/LQ5의 staged compaction worker·soft trigger·merge reserve 정책 및 sustained
  churn/long pin/space-pressure, launcher 전환·독립 process 복원 리허설은 해당 후속 계획의
  남은 범위다. 작은 compaction/restore 회귀의 통과로 운영 수용을 대체하지 않는다.
- commit/push는 실행하지 않았다. 검증 대상은 위 HEAD 위의 현재 미커밋 작업 트리다.

## XL quota 거절과 단일 Native body 수리 (2026-10-10)

`da5b036f7869fedfde39c3c6dfd57ddc010bf8c9` export 및
`36f9a99205a196950f9f90c32d8d162e2c6ab6af60eaf1acd0d26a6f863cc4c4` binary의
boundary-only XL 실제 실행은 `FAILED`였다. controller 578.251초, exit 1,
timed_out=false, source/binary 전후 동일·clean이었다. full seal의 Native/File
독립 proof는 통과했지만 필수 generation 1개가 1,142,692,886바이트로 1GiB pair
quota를 초과해 `SEARCH_CORPUS_HISTORY_RETENTION_EXHAUSTED`로 거절됐다.
후속 delta/noop/delete/reopen은 `NOT_RUN`이다. 인자 `--tier xl`의 앞선 parse 실패는
producer 미실행이며 이 실제 `--tier xlarge` 결과와 구분한다.

읽기 전용 세 감사에서 closure의 중복 과금이나 불필요한 File bootstrap page를
확정하지 못했다. Native body는 NFC와 deterministic folded 텍스트를 모두 저장하고,
decoder가 folding을 재계산해 중복 값과 비교한다. 저장할 상태를 NFC 하나로 줄이고
그 기존 folding 결과를 reader에 반환하도록 수리한다. body codec 2, Native root 5,
sealed manifest 18 수리에 후속 map V2 canonical leaf dictionary를 통합했다. 최종 계약은
Native root 6, File root 18, sealed manifest 19이며 이전 형식은 rebuild refusal한다.
원래 두 surface의 합산 body admission bound, 독립 proof, query 의미, durability,
1GiB/2GiB retention quota 및 600초 client 제한은 유지한다.

이 수리의 workspace 전체 재검증과 최종 optimized XL은 실제 후속 실행 전까지
`NOT_RUN`이다. 위 4,521개 결과를 이 추가 변경에 승계하지 않는다.

통합 후 `clippy --workspace --all-targets --all-features --locked -- -D warnings`는
canonical lane에서 `VERIFIED`(1분 56초)다. Native body 최초 lib 배치는 383 실행 중
379 통과·manifest fixture 4 실패·ignored 1이었다. F18 success prefix 및 진단 문자열을
수리한 집중 18개는 통과했다. F19 통합 집중 배치의 진단 문자열 2개는 아직 F18을
기대해 실패했고, F19로 갱신했다. 이는 옛 번호의 success fixture/진단 기대값 수정이며
구형 format refusal과 손상 oracle은 유지한다.

| 최종 계약의 집중 검사 | 실제 결과 |
| --- | --- |
| `nextest run -p quanta-index-lexical --lib --all-features --locked -E 'test(/(compact_leaf_wire_tests\|paged_segment::body_tests\|sealed_generation::manifest::tests)/)' --test-threads 2 --no-fail-fast` | `VERIFIED`; 22 실행·통과, 366 skipped, 0.131초. body/leaf wire와 manifest 독립 손상·구형 거절 포함 |
| `nextest run -p quanta-index-lexical --test unicode_normalization_goldens --all-features --locked -E 'test(a_generation_sealed_under_the_previous_format_is_refused_typed)' --test-threads 1 --no-fail-fast` | `VERIFIED`; 1 실행·통과, 8 skipped, 1.429초. current literal 17→19 수정, legacy 17/18 거절 추가 |
| `nextest run -p quanta-index-sdk --test l2_daemon_publication --all-features --locked --run-ignored all --test-threads 1 --no-fail-fast` (새 F19 daemon 고정 binary) | `VERIFIED`; 실제 process 6 실행·통과, 0 skipped, 71.741초 |
| `nextest run -p quanta-index-searchd-runtime --test state_migration_owner_v1 --all-features --locked -E 'test(real_search_corpus_backup_restore_preserves_file_and_native_queries) \| test(real_search_corpus_backup_refuses_missing_or_corrupt_pooled_native_body)' --test-threads 1 --no-fail-fast` | `VERIFIED`; 2 실행·통과, 55 skipped, 7.667초 |

위 Cargo 명령은 모두 `./scripts/cargow --lane test-daemon-lane`을 사용했다.
Unicode/File mutation/Native shard 독립 oracle 배치는 55 실행 중 54 통과·1 실패였다.
실패는 위 current manifest literal 오류이며 해당 검사만 별도로 다시 실행했다.
이전 55개 배치나 F19 120개 배치의 전체 결과를 재실행 성공으로 덮어쓰지 않는다.

### 최종 형식의 optimized 진단과 환경 실패

당시 export `2e210fd849e1c22c3856be7e7c0d04f03223610d`와 release binary SHA-256
`0fb003adc198e91e5fe109f6b3c2a0aa193cb2b7317e5f5931de32bd3ba0fd2d`를 사용했다.
실행 당시 생산 Rust/Cargo와 capture/replay 도구는 이 export와 일치했고, 당시 차이는 위 Unicode
테스트 fixture뿐이었다. 후속 query/cache 수리는 별도 export로 검증한다.
pair 1GiB·total 2GiB·RSS 4GiB·IPC 600초·sampling gap 500ms와
seed `5864059738136528177`, `boundary-only`를 유지했다.

- Large producer는 exit 0, 121.328초였다. 초기 구축 70.251초, 증분 18.530초,
  no-op 2.755초, 삭제 8.718초, 동일 process 재개방 4.608초이며 모든 검색 oracle가
  통과했다. full seal의 product retention 회계값은 82,097,198바이트였다.
  시스템 Python 3.9의 `zip(strict=...)` 미지원으로 첫 capture 후처리는 `FAILED`였다.
  원본 실패 기록은 보존했다. 저장소 `.venv/bin/python` 3.12.12로
  `python -m tools.benchmark.retrieval.causal_cost_profile --summary <captured-summary>
  --manifest <captured-tier-manifest> --trace <captured-stderr> --binary <above-binary>
  --source-revision <above-export> --tier large --seed 5864059738136528177
  --disk-observation boundary-only --out <external-profile>`을 실행한 strict replay는
  `VERIFIED`다. 결과는 `diagnostic_unqualified`이며 첫 capture를 성공으로 덮어쓰지 않는다.
- 동일 binary의 첫 XL은 `FAILED`: producer exit 1, controller 774.946초,
  capture timed_out=false였다. IPC Read 600,000ms timeout, RSS sampling gap
  36,382.819ms, 종료 시 `HardDeadlineEscalated { unfinished: ["ingest-accept"] }`를
  기록했다. `pmset -g log`에서 실행 중 20:00:04의 배터리 1% `Low Power Sleep`과
  20:25:09의 AC 전원 Hibernate 복귀를 확인했다. host suspend가 포함된 실행을
  성능 수용이나 저장 quota 성공으로 판정하지 않는다. 후속 증분·query는 `NOT_RUN`이다.
- AC 전원에서 임시 `caffeinate -is`를 사용한 동일 코드·입력·제한의 XL 재실행 결과는
  아래 후속 검증에 기록한다. 전원 assertion은 명령 종료 시 해제하며 제품 설정을
  변경하지 않는다. 앞선 quota 실패와 이번 환경 실패는 모두 보존한다.

### XL File query I/O 거절과 요청 내 메타데이터 재사용

같은 `2e210fd8` export/`0fb003ad` binary의 AC 전원 재실행도 `FAILED`였다.
controller 589.573초, exit 1, timed_out=false, source/binary 전후 동일·clean이었다.
full seal 및 activation은 기존 1GiB/2GiB quota 아래 통과했지만
`content:regex:/scalefileneedle001file00000/` CodeSearch가
`LEX_TRIGRAM_PLAN_LIMIT_EXCEEDED: global query authenticated I/O exceeds policy`로
거절됐다. delta/noop/delete/reopen은 `NOT_RUN`이다.

File planner의 gram 선택도 정렬은 이미 존재했다. 남은 AND gram별로 같은 current-doc
Record, Segment, FilePostings를 반복해서 물리 인증하는 경로를 수리했다. 기존 요청별
MapPage cache의 **총 4MiB** 안에서 이 세 metadata kind도 재사용한다. full ObjectRef가
캐시 key이고 BlobRef 논리 길이·SHA 검증은 유지한다. 새 요청은 다시 디스크를 인증하며
body/block은 캐시하지 않는다. 실제 read만 계량하고 cache hit도 cancellation checkpoint를
수행한다. query read/byte 및 retention/RSS/time 상한은 변경하지 않았다.

`./scripts/cargow --lane test-daemon-lane nextest run -p quanta-index-lexical --lib
--all-features --locked -E 'test(request_metadata_cache) | test(request_map_page_cache) |
test(metadata)' --test-threads 2 --no-fail-fast`: `VERIFIED`, 7 실행·통과,
383 skipped, 1.284초. 재사용·위조 descriptor·새 요청 손상·공유 bound 퇴거·물리 I/O
거절 oracle를 포함한다. 아래 최종 배치 실행 전까지 새 cache의 XL/전체 workspace는
`NOT_RUN`이다.

통합한 segment 교집합 수리에서는 상위 Boolean evaluator가 이미 제한한 후보를
전체 anchor route 순회로 되돌리지 않는다. 제한된 후보는 기존 current-locator 경로를
유지한다. 무관한 segment의 posting을 실제로 손상시킨 뒤 제한 질의는 고정 ID를 반환하고
전체 conjunction은 손상을 거절하는 회귀를 추가했다.

`./scripts/cargow --lane test-daemon-lane nextest run -p quanta-index-lexical --lib
--test f15_file_authority --test regex_literal_alternation --all-features --locked
-E 'test(/(request_.*cache|metadata_walk|conjunction|changed_source_retains_stale|route_inventory)/)'
--test-threads 2 --no-fail-fast`: `VERIFIED`, 10 실행·통과, 390 skipped, 7.920초.
128개 source의 broad case-insensitive regex·19-gram 교집합 고정 goldens,
replace/delete의 stale locator, nested Boolean·Unicode, route 손상과 위 cache 검사를 포함한다.

## 최종 통합 배치 — 요청 cache 및 segment 교집합 이후

원 checkout HEAD는 `8f11cad9587aec1668a9fb3f5f1f055337487071`이며 변경은 미커밋이다.
외부 clean export `a15d994989f547a80d7b102a76429481503cc2d0`를 최종 optimized 빌드에
사용한다. 1,229개 Rust/Cargo 파일이 원 작업 트리와 일치함을 확인했다.
빌드·runtime은 공용 resource-admission lock으로 직렬화한다. AC 전원의 임시
`caffeinate -is` assertion 아래 Python 3.12 `.venv/bin/python`으로 capture/replay한다.

- `VERIFIED`: `./scripts/cargow --lane test-daemon-lane clippy --workspace --all-targets
  --all-features --locked -- -D warnings`, 1분 22초. vendor 경고는 dependency 경고이며
  workspace warning 예외를 추가하지 않았다.
- `VERIFIED`: `just fmt-check rust-test-authority rust-wire-inventory rust-no-allow
  rust-error-shape rust-derive-allowlist rust-ignored-test-policy`; doc paths/anchors,
  prompt-manager 5 targets, `git diff --check`; replay/capture Python 124개, 0.21초.
- `VERIFIED`: `./scripts/cargow --lane test-daemon-lane check --workspace --all-targets
  --all-features --locked`, 3분 06초.
- **전체 workspace nextest `FAILED`**: `./scripts/cargow --lane test-daemon-lane
  nextest run --workspace --all-features --locked --test-threads 2 --no-fail-fast`.
  170개 binary에서 4,532개 실행: 4,531 통과(14 slow), 1 실패, 31 skipped;
  nextest 2,691.919초, orchestration 3,393.715초. 실행 전후 1,229개 Rust/Cargo
  소스에 변경이 없었다. 실패는 `runtime_risk_suite`의
  `e2e_physical_gc::gc_metrics_report_reclaimed_and_retained_bytes_that_match_the_disk`다.
  독립 disk oracle가 File root 17/Native root 4를 기대하여 현재 File root 18의
  format 검사에서 거절했다. root tuple의 폭·map 위치·독립 inode/길이/SHA/의존성 순회와 byte 비교는
  유지하고 test-only version literal 및 주석만 18/6으로 갱신했다.
  이 실패 배치를 후속 focused 성공으로 덮어쓰지 않는다.
- 수정한 GC owner 회귀, strict Clippy, fresh daemon SDK L2와 doctest의 결과는 아래와
  같다. 최종 생산 코드는 위 optimized export와 같으며, 이후 Rust 차이는
  `tests/e2e_physical_gc.rs`의 독립 test oracle 갱신뿐이다.

| GC oracle 갱신 후 검증 | 명령/범위 | 실제 결과 |
| --- | --- | --- |
| strict Clippy | `./scripts/cargow --lane test-daemon-lane clippy --workspace --all-targets --all-features --locked -- -D warnings` | `VERIFIED`; 1분 51초 |
| GC owner 모듈 | 같은 lane의 `nextest run -p quanta-index-searchd-runtime --test runtime_risk_suite --all-features --locked -E 'test(/^e2e_physical_gc::/)' --test-threads 2 --no-fail-fast` | `VERIFIED`; 6 실행·통과, 136 skipped, 30.876초. 실패했던 retained/reclaimed bytes의 독립 disk oracle 포함 |
| fresh daemon build | 같은 lane의 `build -p quanta-index-searchd-runtime --bin quanta-index-searchd --all-features --locked --message-format=json-render-diagnostics` | `VERIFIED`; 실제 artifact를 복사해 고정. SHA-256 `d6e89cd164ef7c7a8dca83e3d0522d8ee51147745113be6103584cec3ef98e11` |
| 실제 process SDK L2 | 같은 lane의 `nextest run -p quanta-index-sdk --test l2_daemon_publication --all-features --locked --run-ignored all --test-threads 1 --no-fail-fast`; `QUANTA_INDEX_L2_TEST_BINARY`는 위 고정 artifact | `VERIFIED`; 6 실행·통과, 0 skipped, 61.210초. 예약·restart·명시적 activation·crash cut·ACK 유실 및 원 publication 복구 |
| doctest | 같은 lane의 `test --workspace --doc --all-features --locked`; `QUANTA_INDEX_SEARCHD_BIN`은 위 artifact | `VERIFIED`; 28 target, 4 실행·통과, 실패 0. orchestration 188.169초 |

이 후속 배치 전후 1,229개 Rust/Cargo 파일의 변경은 0이다. 변경된 test oracle의
owner 모듈 전체를 재검증했으며, 그 뒤 전체 workspace nextest를 다시 실행한 결과는
`NOT_RUN`이다. 나머지 4,531개의 앞선 통과와 수정 후 6개의 결과를 하나의 성공 배치로
합산하지 않는다. A/D/C5 및 현재 통합 생산 코드의 영향 회귀·실제 daemon 수용은
위 결과로 확인했고, 기본 제외 31개와 별도 운영·외부 producer·정식 품질/성능·release
qualification을 완료했다고 주장하지 않는다.

최종 release 빌드는 `VERIFIED`(3분 31초)이며 binary SHA-256은
`ba1b69cdf692987546097bc4e748670abcece9e08d49dba074ce4f8fd5cf5929`다.
첫 capture는 `/tmp` symlink의 evidence-path 검사에서 거절돼 producer가 `NOT_RUN`이었다.
실제 `/private/tmp` 경로로 정정하고 동일 binary로 다시 실행했다.

- **Large `VERIFIED`** (capture 상태 `VERIFIED_DIAGNOSTIC`): controller 104.836초, exit 0, timed_out=false,
  source/binary 전후 동일·clean. full/delta/no-op/delete seal·activation 및 동일 process
  재개방과 검색 oracle 모두 통과했고 capture의 strict replay도 성공했다.
  build 57.842초, delta ingest/seal 12.615초, peak RSS 580,927,488바이트;
  product retained gauge는 full 82,128,734, delta 98,316,118,
  no-op 82,272,336, delete 99,152,834바이트다.
  seed와 pair 1GiB·total 2GiB·RSS 4GiB·IPC 600초·sampling gap 500ms,
  boundary-only 입력을 유지했다. profile은 `diagnostic_unqualified`이며
  matched ablation·성능 비교·release qualification을 뜻하지 않는다.

- **XL `FAILED`**: 동일 export/binary·입력·상한의 capture는 900.079초에서
  collector timeout으로 종료됐다. exit_code=null, timed_out=true이며 source/binary는
  전후 동일·clean이었다. full/delta/no-op/delete seal·activation과 재개방 전 검색은
  통과했지만 `same_process_reopen` 시작 뒤 종료 마커와 성공 summary가 없다.
  재개방 성공·후속 검색·strict profile 생성은 확인되지 않았다.
  네 `ingest_publish_under_lock` 관측 구간은 각각 559.921·61.815·48.014·67.577초,
  합계 737.327초다. 그 안에 포함된 finalization 시간을 이 합계에 다시 더하지 않는다.
  시작 마커는 stop/join 및 boot 전체보다 앞에 있으므로 정확히 어느 재개방 단계에서
  남은 시간이 소진됐는지는 이 trace로 구분할 수 없다. 정적 감사에서도 제거 가능한
  불필요한 재개방 작업이나 제품 정확성 오류를 확정하지 못했다.
  기존 quota/RSS/query I/O/IPC/capture 상한은 유지하며 실패 기록을 보존한다.

- **XL 동일 조건 재실행 `FAILED`**: 위 export/binary·seed·boundary-only·quota와
  IPC/RSS/query I/O/capture 상한으로 재실행했으나 900.044초에서 collector timeout으로
  종료됐다. exit_code=null, timed_out=true, source/binary 전후 동일·clean이다.
  full/delta/no-op/delete seal·activation 및 재개방 전 검색은 통과했고 마지막 마커는
  다시 `same_process_reopen` 시작이다. 성공 summary·tier manifest·strict profile은
  생성되지 않았다. 네 비중복 publish 구간은 528.076·64.558·46.716·66.403초,
  합계 705.753초였다. 재개방 stop/join/boot 중 어느 단계가 지연됐는지와 재개방 완료
  시간은 이 trace로 확정할 수 없다. 원 실패와 재실행 실패를 모두 보존한다.

## 마감 판정

- A semantic compatibility, D RepoMap lifetime, C5 durable reservation 및 현재 통합
  생산 코드의 영향 회귀·실제 daemon SDK 수용은 `VERIFIED`다. scope와 실행은 위 표를 따른다.
- 최종 전체 workspace 배치는 `FAILED`(4,531 통과, test-only format fixture 1 실패)다.
  해당 fixture 수리 후 GC owner 전체 6개와 strict Clippy·SDK L2·doctest는 `VERIFIED`다.
  수리 후 전체 workspace 배치 재실행은 `NOT_RUN`이며 후속 통과를 원 배치에 합산하지 않는다.
- Large lifecycle 진단은 `VERIFIED`, XL 전체 lifecycle 진단은 동일 상한 두 실행 모두
  `FAILED`다. XL 재개방 완료·후속 검색 증거가 없으므로 whole-XL 성능 수용은 완료되지 않았다.
  제품 정확성 오류나 제거 가능한 재개방 작업은 현재 source/trace로 확정하지 못했다.
- 기본 제외 31개 중 위 SDK L2 6개는 별도 실행했다. 나머지 제외 scope, LQ4/LQ5의
  sustained churn·long pin·space pressure·launcher 전환·독립 process 복원 리허설,
  외부 producer·remote CI·정식 품질/성능 비교·release qualification은 `NOT_RUN`이다.
- 변경은 원 작업 트리에 미커밋 상태로 남겼다. commit/push는 실행하지 않았다.

## 2026-10-11 — 정적 원인 확인 후 store/GC 수리

소스 범위는 원 checkout `HEAD=8f11cad9587aec1668a9fb3f5f1f055337487071`와 공유 dirty
tree다. 이번 생산 코드 변경은 `lexical_store/store.rs`, `lexical_gc.rs`의 두 파일이며
ADR의 group 계약과 이 마감 문서를 함께 갱신했다. 다른 작업의 dirty hunk와 별도
trace-waterfall worktree는 수정하지 않았다. 위 10-10의 optimized export/binary는 이번
수리 소스와 같지 않으며 이전 성능 결과를 현재 소스의 결과로 승계하지 않는다.

### 정적 확인과 구현

1. 기존 객체 회계/GC는 object마다 같은 insert, duplicate lookup, expanded update를
   다시 준비했다. `ReferenceStatements`가 한 측정 호출에서는 세 SQL statement를
   한 번 준비하고, GC에서는 bounded step마다 준비해 재사용한다. candidate insert와
   liveness lookup도 같은 step 안에서 재사용한다. rows·인증 결과를 캐시하지 않으며
   digest/kind/size 충돌 검사와 모든 객체의 read/SHA/dependency 인증은 그대로 수행한다.
   기존 pending index는 이미 있으므로 index 누락이나 제곱 시간 scan이라는 원인으로
   분류하지 않는다. 전수 객체 순회 자체를 제거한 변경도 아니다.
2. 64참조 group 경계가 byte bound에 도달하지 않은 작은 객체에도 epoch/journal/control
   확정을 반복시킨다. 기존 object codec의 dependency 한도인 128에 group을 맞추고
   writer/flush/pending recovery의 공통 한도를 유지했다. manifest 최대 폭은
   `21 + 128 × 41 + 82 = 5,351`바이트이며, 이를 새 data objects보다 먼저 예약한다.
   group 전체의 2MiB bound, 96KiB object bound, barrier와 pending→append→clean 순서,
   corrupt-chain refusal 및 epoch fence는 유지한다. group은 wall-clock lock bound가 아니며
   한 번 gate 아래에서 처리할 수 있는 참조 수가 늘었다. 실제 latency/lock wait 효과는
   아직 측정하지 않았다. quota/RSS/query I/O/IPC/capture 상한을 올리지 않았다.
3. 고정 oracle를 추가했다. 128참조 manifest의 5,351바이트 encoding과 129참조 거절,
   129개 작은 객체의 두 group/epoch, 128참조 pending journal의 빈·중간 절단·완전 tail
   재시작 복구, 공유 diamond graph의 고정 262바이트와 descriptor 충돌·leaf 손상 거절이다.
   기존 2MiB split, full barrier refusal, reused-manifest repair, genuine unwind,
   epoch 변경/완전한 mark 전 unlink 금지, I/O 거절·SIGKILL 회귀를 유지한다.

### 실행 결과

- `VERIFIED`: `rustfmt --check --edition 2024`의 위 두 Rust 파일, scoped diff/caller/codec
  대조, `git diff --check`, `just rust-no-allow`.
- `FAILED` 후 수리: 최초 lexical Clippy는 현재 `rusqlite`의 cache feature가 꺼져 있어
  `prepare_cached`가 없는 컴파일 오류였다. 추가 feature/dependency 대신 local prepared
  statements로 변경했다. 후속 Clippy의 신규 fixture redundant clone도 제거했다.
- `VERIFIED`: `QUANTA_INDEX_SCCACHE=0 QUANTA_INDEX_RESOURCE_WAIT_SECONDS=7200
  ./scripts/cargow --lane test-daemon-lane clippy -p quanta-index-lexical --lib --tests
  --all-features --locked -- -D warnings`; 최종 22.14초, exit 0.
- `VERIFIED`: 같은 env/lane의 `nextest run -p quanta-index-lexical --lib --all-features
  --locked -E 'test(/^lexical_store::store::journal_tests::/) | test(/^lexical_gc::tests::/)
  | test(/^publication_faults::tests::/)' --test-threads 2 --no-fail-fast`.
  **46 실행·통과(2 slow), 349 skipped, 159.110초**, exit 0. compile 9.16초.
  I/O 거절 matrix 149.499초와 실제 SIGKILL matrix 64.579초는 같은 배치에 포함되며
  겹치는 시간을 배치 시간에 다시 더하지 않는다. 공용 resource-admission lock을 사용했다.
- `FAILED`: `just lint-doc-paths`는 기존 `docs/rfc/oct-10-native-incremental-vnext.md:3626`의
  외부 Semantica source 링크 두 개가 없는 경로를 가리켜 실패했다. 해당 다른 작업의 RFC는
  수정하지 않았다. 이 오류를 현재 store/GC 코드 검사 성공으로 덮지 않는다.

### 현재 잔여 범위

- 수리 후 Large/XL, fresh daemon SDK 및 workspace 전체 nextest는 `NOT_RUN`이다.
  이전 두 XL timeout을 해결한 것으로 표시하지 않는다. 이번에는 scale runtime을 추가하지 않았다.
- retention 전수 closure/physical 조사와 eager compaction은 그대로다. 전수 검증을 생략하거나
  compaction을 무기한 미루지 않았다. 지배 비용을 분해한 계측과 bounded 비용 계약을 먼저
  확정한 뒤 해당 owner를 수리한다. 부분 SQL/group 수리를 O(delta) 갱신으로 부르지 않는다.
- trace-waterfall의 구현/검증은 별도 worktree에 있다. 통합 시 이 statement lifetime과
  128참조/manifest/recovery 계약을 함께 보존해야 한다. 그 worktree의 검증을 현재 checkout의
  검증에 합산하지 않는다.
- 변경은 미커밋·미푸시다. 이번 회귀가 통과한 뒤 생산 코드를 추가로 수정하지 않았다.
## 2026-10-11 — waterfall 통합 고정 source 검증

공유 checkout의 `HEAD=8f11cad9587aec1668a9fb3f5f1f055337487071`와 이번 closeout
변경을 대상으로 한다. 통합 도중 별도 index/kernel/storage/native 작업이 공유 트리에
추가되어, 해당 source·manifest·baseline은 덮어쓰지 않고 기존 A/D/C5·paged store
소스와 이번 waterfall/store/GC 수리만 외부 임시 checkout에 결합했다. 고정 검증 source는
`/tmp/qi-oct11-closeout-qrsk8un_`다. 전체 nextest 실행 revision은
`c45eacbb5a9f99660b8f60468da7d69f757ce235`, 첫 후속 고정 revision은
`ce2290967fff110fff2bf2cfaa2072109fbe79d0`이다. 두 revision 사이에는 getter 문서 1줄,
semantic trace unit fixture 2개의 root/child context, Python parser/test만 변경됐다.
Rust 생산 실행 코드는 이 두 revision 사이에서 바뀌지 않았다. 최종 고정 revision은
`1efa4dd443c37d814a827db9aef523aff96e6345`이며, ce22909 이후 IPC server/테스트와
runtime boot의 세 파일을 추가 수리했다. 마지막 변경에는 생산 Rust 코드가 포함된다.
따라서 앞선 실행 결과는 각각의 revision에만 적용한다. shared 전체 source를 검증한
것으로 표시하지 않는다. 공유 트리 stage/commit/push는 하지 않았다.

### 통합과 정적 수리

- reviewed waterfall source/test 141개 파일을 통합했다. root의 SQL statement 재사용과
  128참조 adoption/recovery 계약은 유지한다. 실제 CLI backup/restore 후 두 fresh
  daemon process에서 Native/CodeSearch pin·고정 hit/source SHA·ranked 결과를 비교하는
  제품 회귀를 기존 process-restart target에 추가했다.
- contract에는 diagnostic DTO만 두고 transport callback port는 SDK로 이동했다. core는
  concrete attempt lease를 제공한다. read/hash/decode/SQL totals는 하위 causal-profile
  모듈에 두어 store→GC cycle을 제거했다. 새 lint 예외/금지 derive를 추가하지 않았다.
- 실제 `Invalid` capture의 harness producer는 동일 연결의 capture preface를 협상한
  뒤에만 token을 보내도록 수리했다. Off와 plain fallback은 기존 deadline을 유지한다.
  작은 scale 회귀는 Complete·health 없음·동일 capture client/server link를 확인한다.
- 전체 target 정적 검사에서 decoder/fixture lint, scan-experiment 인자 누락, 최초
  수집 313개 lint, 후속 오류→Option 변환 및 core 테스트 59개 진단을 수정했다.
  요청 envelope constructor 누락 7개를 함께 이전했고, 전체 literal 정적 스캔의
  추가 실제 누락은 0건이다. compiler 실패는 런타임 실패로 분류하지 않는다.
- 최종 read-only 재감사에서 offline parser가 새 Health 코드 네 종류와 `complete`
  footer의 모순을 거부하지 않는 것을 확인했다. 모든 Health가 complete와 모순된다는
  계약으로 수리하고, schema의 11개 코드 각각에 complete 거절·partial prefix 보존
  회귀를 추가했다. 정상 recorder는 이미 모든 Health에 Invalid footer를 발급한다.
  Rust 실행 중에는 고정 source를 바꾸지 않고 공유 checkout의 Python만 먼저 검증했다.
  capture ID getter는 저장된 배열을 복사하므로 무실패 조건을 명시했다.
- 전체 배치의 semantic trace unit fixture 두 개는 parent 없는 capture context를
  direct build/apply-window에 전달해 `MissingParentSpan`을 일으켰다. 생산 경로는
  IPC Request root→budget child→ingest Operation child→semantic Build child를 전달한다.
  fixture에 명시적 Operation root/child와 terminal을 추가했다. 기존 counter 및
  독립 row/membership oracle를 유지하고 Health 없음 assertion을 추가했다.
- 최종 IPC 감사에서 accept/spawn 오류의 조기 반환이 기존 worker handle을 detach하는
  경로를 수리했다. accept loop 결과를 보존하고 오류 시 shutdown을 알린 뒤 기존
  connection thread를 모두 join하고 socket을 정리한 다음 원 오류를 반환한다.
  추가 회귀는 channel로 worker 종료를 제어하는 helper 수준 검사이며 실제 OS의
  accept/spawn 오류 주입 증거는 아니다. 이 회귀의 실행은 아직 하지 않았다.
- runtime boot의 trace root/span 생성 오류를 제품 build 실패로 전파하던 경로를
  typed Health 기록으로 바꿨다. 세 `.ok()` 변환 대신 닫힌 오류 enum을 명시적으로
  처리하며 새 lint 예외는 없다. 같은 session의 parent 없는 context에서도 실제 runtime
  build는 성공하고 capture는 Invalid가 되는 회귀를 추가했다. foreign session은 기존대로
  state-root 접근 전에 거절한다. 마지막 세 파일의 rustfmt와 diff 검사만 실행했다.

### 실행 결과

| 검증 | 결과 | 범위 / 한계 |
| --- | --- | --- |
| Python capture/profile/timeline 회귀 | VERIFIED, 226 passed | 최종 parser 수리의 공유 checkout Python 범위. focused Ruff lint/format도 통과; 후속 고정 source와 해당 파일 byte-equal |
| format/no-allow/module-cycle/derive/hexagonal/test-authority/wire/error-shape | VERIFIED | fixed checkout 구조 검사. 공유 트리의 다른 작업 전체를 포함하지 않음 |
| public API/module snapshot guards | VERIFIED | 이번 diagnostic API/module 변경만 검토·갱신. 별도 native/index baseline은 수용하지 않음 |
| workspace lint/TOML/module discipline/ignored-test policy | VERIFIED | 고정 source의 정적 규칙. 후속 digest-fallibility 627파일·0위반. proof registry 검사는 REGISTRY_ONLY이며 benchmark artifact 17개 absent를 성공 실행으로 계산하지 않음 |
| strict Clippy workspace/all-targets/all-features | VERIFIED | c45eacb에서 exit 0, `build-finished.success=true`, error 0. 최종 1efa4dd의 결과가 아님. `--cap-lints warn` 수집과 별도 판정 |
| 전체 workspace nextest | FAILED, 4604 passed / 2 failed / 31 skipped | c45 고정 source에서 4606개 실행, 3026.782초, exit 100. 실패는 위 semantic trace fixture 두 개. 원 배치를 전체 성공으로 합성하지 않음 |
| 수리 후 semantic strict Clippy | VERIFIED, exit 0 | ce22909, all-targets/all-features, `-D warnings`, 4분 56초 |
| 수리 후 semantic lib nextest | VERIFIED, 90/90 passed, 0 skipped | ce22909, 8.541초. 원 실패 fixture 두 개도 통과. 생산 실행 코드 변경 없이 fixture만 수리했으므로 unchanged 전체 배치를 반복하지 않음 |
| fresh canonical daemon build | VERIFIED, exit 0 | ce22909, all-features/locked, 1분 8초. compiler artifact SHA256 `a3d14baff1544d62aa7ebe42edf37923872e5986c4d1e1a8a906adae627c4f5f` |
| fresh daemon SDK L2 | VERIFIED, 6/6 passed, 0 skipped | ce22909, `--run-ignored all`, 53.358초. `QUANTA_INDEX_L2_TEST_BINARY`로 위 fresh artifact를 사용 |
| 최종 IPC/runtime 세 파일 rustfmt·diff | VERIFIED | 1efa4dd, `rustfmt --check --edition 2024 --config skip_children=true`와 `git diff --check`, exit 0. compile/test 성공을 뜻하지 않음 |
| 최종 IPC/runtime strict Clippy·영향 회귀 | BLOCKED | 1efa4dd, AC 전원 없음. 추가 worker join/실제 boot negative 회귀는 NOT_RUN |
| 최종 fresh daemon build·SDK L2 | BLOCKED | 1efa4dd, 생산 코드 변경 후 새 artifact가 필요함. ce22909 binary 결과를 승계하지 않음 |
| workspace doctest | NOT_RUN | resource-admission 대기만 수행. 배터리 3%에서 해당 실행에 SIGTERM, exit 143, Cargo 시작 전 종료. test 실패가 아니며 재개는 AC 전원에 BLOCKED |
| 새 소스 release build·Large/XL lifecycle/trace 진단 | BLOCKED | AC 전원 없음, 실행하지 않음. 이전 source나 별도 P09 채팅의 결과를 승계하지 않음 |

명령은 `QUANTA_INDEX_SCCACHE=0 QUANTA_INDEX_RESOURCE_WAIT_SECONDS=7200`과 공용
resource-admission lock을 사용한다. strict compile은
`./scripts/cargow --lane test-daemon-lane clippy --workspace --all-targets --all-features
--locked --keep-going --message-format=json -- -D warnings`다. 런타임 전체 배치는
`./scripts/cargow --lane test-daemon-lane nextest run --workspace --all-features --locked
--test-threads 2 --no-fail-fast`로 실행했다. 수리 후 영향 명령은 같은 lane의
`clippy -p quanta-index-semantic --all-targets --all-features --locked -- -D warnings`와
`nextest run -p quanta-index-semantic --lib --all-features --locked --test-threads 2
--no-fail-fast`다. 추가 process/scale 검증은
각각 별도 결과로 기록한다.

마지막 전원 확인은 `pmset -g batt`: Battery Power, 1%, 예상 잔여 1분,
Battery Warning Final이었다. 이전 Large/XL 실행의 절전 중단 이력이 있어 추가 대형
실행을 시작하지 않았다. 종료한 것은 root 담당의 doctest admission 대기 실행뿐이며
공용 lock이나 다른 담당의 실행은 중단하지 않았다. AC 연결 후 최종 source에서
IPC/runtime all-targets/all-features strict Clippy와 영향 회귀를 먼저 실행하고,
fresh daemon/SDK L2→workspace doctest→release scale artifact→Large/XL을 공용
admission 아래 직렬 실행한다. 변경 없는 전체 4,606개를 반복하여 앞선 결과와 합산하지
않으며 최종 revision의 전체 workspace 성공은 현재 `NOT_RUN`이다.

### 남은 수용 경계

LQ5의 대형 sustained churn·long pin·space pressure와 측정값에 근거한 merge/admission
정책, §9의 binary/config/root 단일 durable launcher 전환 및 crash cut은 현재 작은
compaction/복원 회귀로 검증되지 않는다. 별도 운영/제품 전환 gate로 유지한다.
이번 closeout의 기능/복구 회귀, 성능 진단, quiet-host overhead, 외부 producer,
remote CI 및 정식 release/heavy qualification은 각각 별도로 판정한다.

## 2026-10-11 — AC 재개·배포 런처·restore 중복 측정 수리

앞 절의 AC 부재와 `1efa4dd` 중단은 당시 이력이다. AC 연결 후 공유 공용 lock 아래
검증을 재개했다. 공유 HEAD는 `8f11cad9587aec1668a9fb3f5f1f055337487071`이며,
이하 source revision은 외부 임시 checkout의 commit이다. 공유 트리 stage/commit/push는
수행하지 않았다. 별도 native/kernel/storage 변경을 이 검증에 합치지 않는다.

### 수리와 source 범위

- `43046bbdbeb32c4bda5d7bb4e1cfa0b6aee3f4ef`: private IPC worker RAII owner는
  오류 반환과 unwind 모두 shutdown→join을 수행한다. IPC/runtime strict Clippy와 IPC
  102개는 통과했다. 같은 lib 배치의 새 runtime missing-parent boot fixture 한 개는
  필수 retention policy 누락으로 실패했다(104 run / 103 pass / 1 fail / 7 skipped).
- `bdcc708339f431874922eecf8d2f87de18babff4`: 위 fixture에 명시적 policy를 추가했다.
  부모 없는 trace 때문에 제품 boot를 실패시키지 않는 회귀와 foreign-session 거절이 통과했다.
  아래 runtime 영향/문서 검증은 이 source의 결과다.
- `dd0f386ea2418930e529725e74797ffab35e42f3`: 배포 launcher/schema/회귀 5개 파일과
  기존 wire inventory/fixture mapping의 필요한 hunk만 추가했다. Rust source/Cargo.lock은
  bdcc와 byte-equal이다. 선택은 schema 1의 binary SHA/config/root 단일 private JSON이다.
  file fsync→rename→directory fsync를 마쳐야 성공하며 rename 이후 오류는
  `PersistenceUncertain`이다. 같은 deployment directory의 child-inherited lock은 정상
  drain과 launcher SIGKILL 뒤 child가 살아 있는 동안 전환을 거절한다.
- `install`/`verify-bootstrap`은 pristine backup/restore artifact의 검증이다. 일반
  `serve`/restart는 selection/SHA/lock을 확인한 뒤 기존 daemon boot owner에 mutable-root
  검증·복구를 위임한다. 최초 install의 full serve config 값 검증, 비협력적인 직접 daemon,
  같은 UID의 hash→exec 사이 binary 변조까지 검증했다고 주장하지 않는다.
- cache의 daemon 경로는 후속 Cargo 빌드로 내용이 바뀌었다. 최초 배포 호출은 SHA 검사에서
  실행 전에 거절됐고 pytest는 시작되지 않았다(`NOT_RUN`). canonical daemon을 다시
  빌드해 `/private/tmp/qi-oct11-final-deployment-searchd`로 고정 복사했다. SHA256은
  `efcf231c8f3f7a5079f0e390b95625ee8807a9920a631e90d19f61931c4bdae7`이며 아래 dd0
  SDK/배포 배치의 전후 SHA가 같다. 앞선 bdcc SDK 6개는 최초 cache SHA에 묶인 증거로
  표시하지 않는다.
- `47f904adcce2f5c3eacd9761b24e25ef6af8ec2d`: search-plane 세 파일의 restore 수리다.
  최초 all-observed 측정에 부재가 없고 전체 count/byte가 한도 내면 기존 active/unresolved
  pin 검증 후 전체 측정값을 재사용한다. exact selected-set bytes는 집합 포함에 대해
  단조이므로 기존 planner도 전체를 보존해야 한다. 과한도는 기존 subset planner,
  손상·부재는 기존 conservative 보존으로 처리한다. 마지막 전체 physical census와
  max_total_bytes 거절은 유지한다. cache/새 port/format/한도 변경은 없다.

### 실행 결과

| 검증 | 결과 | source / 범위 |
| --- | --- | --- |
| IPC/runtime strict Clippy, IPC unit | VERIFIED, IPC 102 passed | 43046bb, all-targets/all-features, `-D warnings`. 실제 OS accept/spawn 실패 주입은 아님 |
| runtime strict Clippy | VERIFIED, exit 0 | bdcc708, all-targets/all-features. 122초 compile; 전체 1203.196초에는 외부 lock 대기 포함 |
| runtime parent negative | VERIFIED, 2/2 passed | bdcc708, 7 skipped, 0.158초. missing-parent 실제 boot/Invalid 및 foreign-session 사전 거절 |
| runtime 영향 batch | VERIFIED, 72/72 passed | bdcc708, 190 skipped, 69.007초. supervisor/readiness/read-view/idempotency/boot-quarantine/snapshot registry |
| workspace doctest | VERIFIED, 4 passed / 0 failed / 0 ignored | bdcc708, all-features/locked, 473.431초 외부 대기 포함 |
| 고정 daemon SDK L2 | VERIFIED, 6/6 passed / 0 skipped | dd0f386, 위 고정 SHA, 67.561초. canonical 재빌드 16.356초 |
| 배포 launcher unit + 실제 daemon | VERIFIED, 21 passed / 0 skipped | dd0f386, 9.67초. 독립 wire/SHA/env oracle, 실제 SIGKILL 3 cut·SIGINT/SIGTERM·timeout·상속 lock, backup/restore old/new 선택·재시작 |
| Python lint/format·wire/schema 대조 | VERIFIED | 신규 5개 파일과 scoped inventory mapping. env allowlist 69개, unsupported version/uppercase·trailing newline SHA 거절 |
| restore search-plane strict Clippy | VERIFIED, exit 0 | 47f904a, all-targets/all-features, 83.938초 |
| restore retention/history 영향 batch | VERIFIED, 47/47 passed | 47f904a, 521 skipped, 2.612초. 전체 policy/history 모듈과 기존 손상·부재·pin·root 용량·reap oracle |
| restore fresh daemon build/SDK | VERIFIED, 6/6 passed / 0 skipped | 47f904a, build 163.867초, SDK 67.399초. 고정 SHA `47cdef65cc98c3e7c1645b35b8dd77e3e40f6be706ad1cebe8f103500a7b7cbf` |
| restore 실제 daemon 배포 영향 | VERIFIED, 1 passed / 0 skipped | 47f904a, 5.72초. 위 고정 SHA로 bootstrap 전환·mutable-root 재시작 |
| 최종 release harness build | VERIFIED, exit 0 | 47f904a, all-features/locked, 297.844초. 고정 SHA `eb1b62f187d196e8aee7d57466afc7b4419c15fc6d30dedbf96af9feb9c9214b` |
| 최종 Large product/lifecycle | VERIFIED | 47f904a, producer exit 0, 87.881초, 10개 phase/root ancestry·Complete timeline·Health/loss 0 |
| 최종 XL product/lifecycle | FAILED — TIMEOUT | 47f904a, 900.082504458초, producer exit code null. 10개 phase/재오픈 후 검색 완료라도 제품 artifact와 footer 없음 |
| Large canonical profile parity | NOT_RUN — parity 증거 없음 | CLI exit 2/PARTIAL_DIAGNOSTIC은 정상적인 부분 진단 판정. product/timeline 성공과 구분 |
| owned TempDir 정리 strict Clippy | VERIFIED, exit 0 | a833970, harness all-targets/all-features/locked, `-D warnings`, 223.261초. 최초 307fac6 lint 실패는 아래 보존 |
| owned TempDir 정리 영향 회귀 | VERIFIED, 13/13 passed | a833970, 163 skipped, 0.661초. 기존 teardown/diagnostic-parent·reopen·runtime_custody. small-scale 선택식 하나는 아래 별도 확인 |
| 실제 small-scale trace 영향 회귀 | VERIFIED, 1/1 passed | a833970, 172 skipped, 7.310초. 기존 입력/phase/query oracle와 Complete·Health 없음·attempt link 유지 |

명령은 같은 `QUANTA_INDEX_SCCACHE=0 QUANTA_INDEX_RESOURCE_WAIT_SECONDS=7200
QUANTA_INDEX_TARGET_GC=0`과 `./scripts/cargow --lane test-daemon-lane`을 사용했다.
첫 영향 회귀는 `nextest run -p quanta-index-searchd-runtime --lib --all-features --locked
--test-threads 2 --no-fail-fast -E 'test(/^diagnostic_runtime_parent_tests::/)'`다.
런타임 72개는 `runtime_fast_suite`, `runtime_risk_suite`, `runtime_supervisor_owner_v1`,
`process_readiness_owner_v1`의 supervisor 전체, 4,096-source restart를 제외한 readiness,
`e2e_read_view|e2e_ingest_idempotency|e2e_boot_quarantine|e2e_snapshot_registry`를 선택했다.
4,096-source 두 target은 앞선 c45의 통과 증거이며 bdcc 재실행으로 표시하지 않는다.
SDK 명령은 `nextest run -p quanta-index-sdk --test l2_daemon_publication --all-features
--locked --run-ignored all --test-threads 1 --no-fail-fast`이며
`QUANTA_INDEX_L2_TEST_BINARY`에 고정 copy를 전달했다. 문서 검증은 `test --workspace
--doc --all-features --locked`다. 배포 회귀는 공용 resource admission 아래
`python -m pytest -q -p no:cacheprovider tools/ci/tests/test_deployment_launcher.py
 tools/ci/tests/test_deployment_launcher_searchd.py`로 실행했고 새 고정 binary를
`QUANTA_INDEX_SEARCHD_BIN`으로 전달했다. 임시 로그/fixture/binary는 checkout 밖에 있다.

### 최종 Large/XL 판정과 정적 후속 수리

`./scripts/cargow --lane release-lane build -p quanta-index-searchd-harness
--bin scale_matrix --all-features --locked --release --message-format=json-render-diagnostics`
후 고정 `/private/tmp/qi-oct11-final-scale-matrix`를 사용했다. 두 capture는 tier만 다르고
seed `5864059738136528177`, boundary-only observer, 기존 900초 producer 상한을 유지했다.
source `47f904adcce2f5c3eacd9761b24e25ef6af8ec2d`와 binary SHA는 실행 전후 같다.
외부 build가 있는 host에서의 진단 실행이며 성능 수용이 아니다.

- Large `/private/tmp/qi-oct11-final-large`: producer 87.881363333초, exit 0,
  `PRODUCT_ARTIFACT_VERIFIED`; timeline Complete, Health 없음, drop/unwritten/open spans 0.
  mandatory phase 10개는 완료되고 실제 Operation ancestry가 확인됐다. exporter의
  `viewer_available=true`는 UI 검증이 아니다. legacy sync/BM25/process-I/O/owner-duration
  parity가 성립하지 않아 CLI exit 2/`PARTIAL_DIAGNOSTIC`이며 `causal-profile.json`은
  게시되지 않았다. execution build binding과 timeline source binding은 `UNPROVEN`이다.
- XL `/private/tmp/qi-oct11-final-xlarge`: producer 900.082504458초 timeout,
  execution `FAILED`, product `UNVERIFIED`, timeline partial. 제품 artifact, stdout/stderr,
  footer가 없다. Health 목록은 비었지만 loss는 null이며 0으로 바꾸지 않는다.
  full/delta/no-op/delete seal·activation과 same-process reopen 10개 phase는 모두 ok였다.
  reopen은 812.321→864.610초, 후속 query는 876.033초 전에 끝났다. 마지막 driver join은
  876.0334→876.0817초, 마지막 RuntimeStop event는 876.081820875초다. 미완료 span은
  invocation Operation 하나이며 ArtifactWrite는 시작되지 않았다. phase 부분 성공으로
  XL whole-call 실패를 덮지 않는다.
- source에서 `scale.rs::measure_tier_with_runtime_config_and_capture`는 측정 결과를 만든
  뒤 `rt.stop()`을 완료해야 반환한다. CLI artifact writer는 그 반환 뒤 실행한다.
  `harness.rs::E2eRuntime::stop`은 driver/socket 종료 다음 `drop(self.tempdir.take())`로
  owned 상태 루트를 동기식 재귀 삭제했으며 삭제 오류가 숨겨졌다. 이는 마지막 trace 뒤의
  유력한 미계측 경로다. 기존 trace만으로 24초 공백의 단독 원인이나 필요한 추가 시간을
  확정하지 않는다. nested span 시간을 합산해 CPU/IO 원인으로 판정하지 않는다.
- 후속 `307fac645f8a6f1c30fbe92336dd15faaa865699`의 harness strict Clippy는
  `FAILED`, exit 101, 91.907초다. helper 첫 문단 길이와 새 trace 회귀의 JSON indexing
  lint가 원인이며 nextest는 시작되지 않았다(`NOT_RUN`). 모든 새 indexing을
  `get`/`as_str`/`as_u64`로 바꾸고 문단을 분리한
  `a833970390eb69320897a90b672c1a7dc04c4092`에서 strict Clippy와 영향 회귀 13개가
  통과했다. 제품 로직은 같다. 처음 선택한 `small_scale_capture_negotiates_query_attempt_links`는
  공유 트리에만 있는 추가 test라 고정 소스에는 match가 없었다. 실제 고정 소스의
  `small_scale_trace_negotiates_query_attempts_without_health_events` 한 개를 별도로 실행해
  통과했다(1 passed / 172 skipped, 7.310초). 13개를 재실행하거나 전체 suite 성공으로
  합산하지 않았다.
- ROOT exact-scope amendment에 따라 명시적 stop의 owned TempDir을 `close()`로 정리한다.
  기존 RuntimeStop span을 재사용하고 invocation parent를 보존한다. optional trace의
  admission/clock 오류는 Health만 기록하며 삭제 자체와 I/O 결과를 바꾸지 않는다.
  consumed TempDir은 실패해도 Drop에서 재삭제되지 않는다. 기존 driver primary error,
  caller-owned root와 unwind fallback은 유지한다. deadline 이동·백그라운드 삭제·상한
  확대·부분 결과 게시가 없으며 성능 수리가 아니다. 이 후속 source의 영향 검증은 `VERIFIED`;
  Large/XL 재실행은 `NOT_RUN`이다.

후속 harness 명령은 위 공용 env/admission에서 다음과 같다. strict Clippy의 외부 실행
시간은 223.261초, 13개 nextest는 build 포함 97.169초, 별도 small-scale 명령은
17.266초다. 고정 source와 shared harness는 byte-equal이며 runtime 소유
hunk도 같다. 공유 runtime의 별도 `native`/`native_worker` module 선언은 보존했으며,
이 isolated-source 검증을 별도 native/kernel/storage 또는 공유 checkout 전체 수용으로
승격하지 않는다.

```sh
./scripts/cargow --lane test-daemon-lane clippy -p quanta-index-searchd-harness --all-targets --all-features --locked -- -D warnings
./scripts/cargow --lane test-daemon-lane nextest run -p quanta-index-searchd-harness --lib --test runtime_custody --all-features --locked --test-threads 2 --no-fail-fast -E 'test(/^harness::diagnostic_parent_tests::/) | test(/^harness::teardown_fault_tests::/) | test(small_scale_capture_negotiates_query_attempt_links) | test(same_process_reopen_lifecycle_spans_are_children_of_named_phase) | binary(runtime_custody)'
./scripts/cargow --lane test-daemon-lane nextest run -p quanta-index-searchd-harness --lib --all-features --locked --test-threads 1 --no-fail-fast -E 'test(/^scale::tests::small_scale_trace_negotiates_query_attempts_without_health_events$/)'
```

### 수용 경계

원 A/D/C5 closeout, 기능/복구, 새 trace의 lifecycle, 성능 진단, 운영 전환을 구분한다.
LQ5의 측정 기반 merge threshold/reserve·staged worker·post-merge target admission과
지속 churn/long pin/space pressure는 현재 inline compaction 회귀로 수용되지 않는다.
quiet-host Large Off/Record 3교대 쌍은 BLOCKED다. Large/XL preflight에서 각각 외부 Rust
6개/2개와 load 27.463/15.591 > limit 8을 확인했다. override는 진단용이며 성능 수용을
뜻하지 않는다. 외부 producer/실제 운영 endpoint 전환, remote CI,
정식 release/heavy qualification은 NOT_RUN이다. 합성 Perfetto UI fixture 업로드는 사용자
승인 대기이며 실제 viewer 조작은 NOT_RUN이다. exporter/parser 성공을 UI 성공으로 승격하지 않는다.


## 최소 준비 worker와 bounded owned 정리 코드 마무리 (2026-10-11)

고정 검증 소스는 `dba0bdb227d3e2e8f87006861e1539017d6b2c4e`다. 기존
`a833970…` 이후 target-scoped compaction 준비와 private TempDir shard 정리를
통합했다. shared HEAD는 `8f11cad…`이며 다른 native/kernel/storage 작업을 보존했다.
공유 `lib.rs`의 외부 native 선언과 `harness.rs`의 동등한
`DiagnosticTerminalV1::from_result` 이행 5곳은 고정 소스에서 제외했다. 그 외 제품
소유 hunk는 같다. adapter/export 3파일의 공유/frozen 차이는 rustfmt 서식뿐이다.

| 검증 범위 | 판정 | 관측 결과 |
| --- | --- | --- |
| lexical/harness strict Clippy | VERIFIED | all-targets/all-features/locked, exit 0, 외부 53.048초 |
| 준비/compaction/게시 fault/정리/lifecycle lib 배치 | VERIFIED | 36/36 passed, 557 skipped, 테스트 205.425초, build/공용 락 대기 포함 1023.743초 |
| File/Native/manifest/Unicode 통합 배치 | VERIFIED | 6 binaries, 113/113 passed, 기본 제외 8개, 테스트 363.611초, 외부 470.213초 |
| hexagonal boundary | VERIFIED | `just rust-hexagonal`, exit 0. 새 crate/vendor dependency 없음 |
| guarded module tree | VERIFIED | `just rust-cargo-modules`, exit 0. 고정 소스의 contract/core tree unchanged |
| 새 전체 workspace 배치/remote CI | NOT_RUN | 위 owner 범위를 전체 workspace 성공으로 합산하지 않음 |
| 새 release build | VERIFIED | 07f0430, all-features/locked, exit 0, 217.798초 |
| 새 XL product/lifecycle | VERIFIED | 07f0430, 777.166937750초, producer exit 0, 900초 상한 유지. 제품 artifact·10 phases·정리·footer 완료 |
| quiet-host 성능 수용 | BLOCKED | 새 preflight의 foreign Rust 4개와 load 13.29834 > limit 8. 진단 override만 사용 |
| LQ5 전체 운영/성능 수용 | NOT_RUN | sustained churn, long pin, space pressure, measured merge threshold/reserve, post-merge admission, process RSS aggregate 증거 없음 |

공용 env는 `QUANTA_INDEX_SCCACHE=0 QUANTA_INDEX_RESOURCE_WAIT_SECONDS=7200
QUANTA_INDEX_TARGET_GC=0`이다. 명령은 공용 resource admission 아래 순차 실행했다.

```sh
./scripts/cargow --lane test-daemon-lane clippy -p quanta-index-lexical -p quanta-index-searchd-harness --all-targets --all-features --locked -- -D warnings
./scripts/cargow --lane test-daemon-lane nextest run -p quanta-index-lexical -p quanta-index-searchd-harness --lib --all-features --locked --test-threads 2 --no-fail-fast -E 'test(compaction) | test(preparation) | test(/^text_authority::paged_bootstrap_tests::/) | test(/^text_authority::paged_segment::/) | test(/^publication_faults::/) | test(/^harness::owned_cleanup::/) | test(/^harness::diagnostic_parent_tests::/) | test(/^harness::teardown_fault_tests::/) | test(/^scale::tests::small_scale_trace_negotiates_query_attempts_without_health_events$/)'
./scripts/cargow --lane test-daemon-lane nextest run -p quanta-index-lexical --test generation_delta_base_carryforward --test f15_file_authority --test l2_file_mutation --test sealed_manifest --test text_authority_shards --test unicode_normalization_goldens --all-features --locked --test-threads 2 --no-fail-fast
just rust-hexagonal
just rust-cargo-modules
```

- File 준비 회귀는 독립 scrub이 인정한 64-doc segment에서 세 번의 debt를 처리하며,
  준비/foreground root equality, unrelated edit, touched-debt delete와 cap/cancel을 검증했다.
  fixture의 debt 생성은 직접 map retirement다. 정상 File producer는 touched debt를 바로
  처리하므로 이 fixture를 운영 debt 축적/throughput 증거로 쓰지 않는다.
- Native 준비 회귀는 14개 700KiB body에서 생성한 old debt 5개를 2/2/1로 처리한다.
  고정 query ID, 첫 cold/prepared root equality, stale 선택의 실제 delete fallback,
  retire 예정 body의 읽기 제외, 취소 우선순위와 NFC admission을 확인했다.
- 실제 adapter no-op 회귀는 debt 3→1, live docs 5 유지, fresh logical ID/derive/retire
  없음, 이전 sealed sidecar와 pool object의 SHA/bytes/inode 보존 및 held base query를
  확인했다. 독립 Chunk clear는 기존 coverage 거절로 target을 만들지 않았다.
- 준비 worker는 기존 unsealed target의 lease/base custody 안에서만 동작하며, mutex 전
  join하고 오류/unwind 시 cancel 후 join한다. process permit은 join 이후 두 packet의
  소비/폐기까지 유지한다. 128MiB 중 cache 4MiB와 packet 62MiB씩의 admission은
  foreground heap/process RSS 또는 disk reserve 보장이 아니다. 새 staging disk 파일은 없다.
- private owned-root 정리는 최대 4개의 scoped shard worker를 모두 join한 뒤 TempDir을
  close한다. malformed/symlink fallback, foreign target 보존, worker failure 전부 join,
  cleanup 오류/trace ancestry와 실제 small-scale Complete/Health 없음 회귀가 통과했다.
  이전 전체 deadline과 caller-owned root 경계는 그대로다. 후속 XL에서 정리 시간은
  측정했으나 같은 환경의 before/after 개선량은 증명하지 않았다.
- 최초 Clippy 실패는 `c11844f…`의 lexical lint 12건, `ae55e27…`의 test 문단 1건,
  `4ae8da3…`의 외부 helper 의존 5곳, `411b66b…`의 cleanup lint 8건이다. 모두 exit 101,
  런타임 미실행이었다. 실패를 보존하고 해당 소스/서식/경계만 수리한 dba0bdb에서
  위 배치가 통과했다. dependency Tantivy의 기존 warning 8건은 별도 scope다.

측정 기반 merge policy/reserve/post-merge admission은 최소 준비 worker에 추가하지
않았다. 기존 foreground quota/독립 seal 검증을 유지했으며, 3-cycle 기능 증거로 전체
LQ5 코드나 운영 수용을 완료 표시하지 않는다. quiet-host Off/Record, 정확한 ENOSPC/
short-write/fsync fault 및 worker의 외부 request-cancellation matrix도 별도 미검증 범위다.

### 새 XL 최종 종료 수용

source `07f0430959cbaa7f2d72f7cbb5656cd5aa802670`은 검증 이후 제품 변경 없이
rustfmt/export 정렬 및 문서만 반영한 clean frozen source다. 고정 binary는
`/private/tmp/qi-oct11-compaction-final-scale-matrix`, SHA-256
`e9f2f3a6f9823f161db98585e06898a5831c6cdfcc08738dfa69d9e6fe75e31f`다.
실행 전후 source HEAD/clean state와 binary SHA를 대조했고 제품 소유 hunk는 shared와
일치한다. 이전 frozen binary와 실패 artifact는 덮어쓰지 않았다.

release 명령은 위의 `./scripts/cargow --lane release-lane build
-p quanta-index-searchd-harness --bin scale_matrix --all-features --locked --release
--message-format=json-render-diagnostics`다. 공용 resource admission과 AC 전원/
`caffeinate -is` 아래 Python 3.12의 capture를 다음 고정 인자로 한 번 실행했다.

```sh
python -m tools.benchmark.retrieval.causal_cost_capture --cwd /private/tmp/qi-oct11-closeout-qrsk8un_ --binary /private/tmp/qi-oct11-compaction-final-scale-matrix --source-revision 07f0430959cbaa7f2d72f7cbb5656cd5aa802670 --binary-sha256 e9f2f3a6f9823f161db98585e06898a5831c6cdfcc08738dfa69d9e6fe75e31f --tier xlarge --seed 5864059738136528177 --disk-observation boundary-only --max-seconds 900 --out-root /private/tmp/qi-oct11-compaction-final-xlarge --timeline
```

- **VERIFIED — product/lifecycle**: producer exit 0, timeout false,
  777.166937750초, `PRODUCT_ARTIFACT_VERIFIED`. 모든 10 phase 및 재오픈 후 query,
  root 정리, artifact write와 invocation root 종료가 완료됐다.
- **VERIFIED — canonical trace**: Complete footer, accepted/written 71,240,
  Health 없음, drop/unwritten/open spans/producer leases 모두 0. 10 phase는 모두 ok이며
  하나의 완료된 invocation Operation 아래 실제 parent ancestry를 가진다.
- 마지막 owned-root `RuntimeStop`은 751.928294708→776.049204833초,
  **24.120910125초**다. ArtifactWrite는 776.152025083→776.156405583초이며
  footer는 776.156421125초에 닫혔다. 정리를 deadline 밖으로 옮기거나 생략하지 않았다.
- full seal 440.731초, delta 57.101초, no-op 47.499초, delete 64.583초,
  same-process reopen 47.321초다. 내부 nested span을 합산하지 않았으며 이번 수치로
  이전 contended 실행 대비 정리의 인과적 개선량을 주장하지 않는다.
- **PARTIAL_DIAGNOSTIC — capture CLI exit 2**: legacy sync/BM25/process-I/O/
  owner-duration parity가 아직 부족해 causal-profile.json을 게시하지 않았다.
  execution `build_binding=UNPROVEN`과 timeline source-binding 증거 부족은 유지한다.
  직접 source/binary 대조와 product/lifecycle 성공을 정식 비교·release qualification으로
  승격하지 않는다. `viewer_available=true`는 exporter 결과이며 실제 UI 검증이 아니다.

새 product/trace 파일은 `/private/tmp/qi-oct11-compaction-final-xlarge`에 있고 checkout에는
추가하지 않았다. 코드 영향 검증과 새 XL product/lifecycle 마무리는 완료됐다. 전체
LQ5 운영 정책, quiet-host Off/Record, source/build/profile parity, 외부 producer,
remote CI 및 release qualification은 위에 명시한 별도 경계로 남는다.
