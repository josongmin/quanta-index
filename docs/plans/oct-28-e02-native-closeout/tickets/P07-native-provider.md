# P07 — Configured Voyage Native batch의 실제 admitted transport

최신 소스 감사: 2026-10-30 KST, S `e9ee1bd5302c604b232ef2e2725fb6879ffbe7fd`와 Provider 소유 2파일 dirty 수정. 최초 작성은 2026-10-28이다. **실제 Native HTTP D1은 BLOCKED, owning compiler/body와 Runtime/SDK D2/D3는 이 채팅에서 NOT_RUN**이다. 소스/정적/실행 상태를 분리한 최신 기록은 [Provider 최종 감사](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-22-finalizing/VALIDATION.md#e02-provider-final-audit-20261030-c)를 따른다. 공통 계약은 [README](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-28-e02-native-closeout/README.md), [write map](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-28-e02-native-closeout/WRITE-MAP.md), [검증 계약](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-28-e02-native-closeout/VERIFICATION.md)을 따른다.

## 담당·선행·병렬 경계

- 담당: `QIDX E02 C Native Provider·Successor…` (`01a183c0-22b5-79c1-a260-fda56a4fc284`). 다른 채팅의 미적용 enum 후보는 참조용이며 수정권 이관이나 별도 Provider lane이 아니다. P05/DELTA, L1–L4, QE 재사용, 공용 gate는 기존 담당을 유지한다.
- 선행: P00의 retained request/response DATA와 actual A·transport failure ABI. credential은 원격 D3 수용 입력이며 내부 구현의 선행 조건이 아니다.
- 착수 조건: 독립 구현·local transport oracle 준비 가능. public selection parent는 P00 합류.
- Source/진척 SSOT: [기존 E02](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-22-finalizing/E02-publication-recovery.md). 이 패킷으로 기존 담당을 재배정하지 않는다.

## 현재 소스에서 확인한 사실

- `VoyageCode3Adapter::embed_batch_into_native_v3` override와 실제 SDK caller가 존재한다. 유효한 nonempty 배치는 요청 바이트 생성 전에 `TransportAdmissionUnavailable`/`UnsupportedOperation`으로 거절한다. 빈 배치는 기존 no-op이다. 기본 trait 메서드만 있다는 이전 관측은 폐기한다.
- 기존 canonical Native JSON serializer와 admitted serde decoder를 재사용한 request/response codec, exterior scratch/부분 벡터/전체 parser 오류, first-cause·reentry·completion guard가 있다. 네 child의 회귀 28개는 작성·mount됐지만 실행 수가 아니다.
- 최종 감사에서 생산부 enum 참조 5곳과 테스트 패턴 1곳을 실제 `SearchPlaneErrorCodeV2::Lexical(LexicalErrorCode)` 계약으로 수리했다. wire/error taxonomy·예산·fixture·28개 body 수는 유지했다. 정적 검사는 VERIFIED이며 compiler GREEN은 주장하지 않는다.
- 잠긴 reqwest 0.12.28에는 caller admission과 비정적 borrowed request를 실제 request/header/channel/TLS/socket/response/error 물리 할당에 연결하는 operation이 없다. private worker에 현재 non-Send/Sync admission을 단순 전달할 수 없다. 이 dependency-owner hook이 실제 선행조건이다. credential 확인은 별도이며 미실시다.

## 수정 허용 파일·함수

| 수정 파일 | 허용 함수·hunk |
| --- | --- |
| [packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/voyage_adapter/mod.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/voyage_adapter/mod.rs) | `VoyageCode3Adapter::embed_batch_into_native_v3 / 기존 request/response transport 접합` |
| [packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/voyage_adapter/wire.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/voyage_adapter/wire.rs) | `같은 schema의 controlled request/response codec` |
| [packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/voyage_adapter/tests.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/voyage_adapter/tests.rs) | `실제 adapter+transport의 protocol/cost/fault oracle` |
| [packages/analysis/quanta-v2/crates/quanta-contract-retrieval/src/embedding_port.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-contract-retrieval/src/embedding_port.rs) | `NativeEmbeddingPortSlotsV3에 필요한 최소 retained DATA ABI; unrelated provider defaults 유지` |

파일 전체 변경 권한을 의미하지 않는다. 위 함수 밖 caller/export/mount/Cargo 변경은 P00에 최소 patch로 반환한다. 새 private child/test 파일은 같은 기능 owner 아래에 만들 수 있으나 기존 parent mount는 지정 writer가 한 번 반영한다.

## 구현 순서

1. 기존 provider instance의 정확한 model/policy와 같은 HTTP transport를 Native request/response caller에 연결한다. Ordinary embed_batch의 Vec 결과를 나중에 Native로 복사하는 우회는 사용하지 않는다.
2. request headers/body, bounded response wire/decoder/numeric vector/normalization의 실제 allocation/work/transport bytes를 기존 Original admission으로 선청구하고 partial DATA를 exterior에 둔다.
3. 취소/timeout/HTTP/provider/parse/model/dimension/count 실패를 typed taxonomy로 보존한다. retry/backoff/response bound는 기존 contract를 유지한다.
4. 실제 SDK Native artifact caller의 기존 method selection을 유지하고, transport attempt의 전체 오류·부분 backing을 enclosing DATA에 보존할 최소 공용 patch만 P00에 전달한다. transport에서 admission을 강제할 기존 owner API가 없으면 정확한 누락 ABI를 공급 요청으로 반환한다. 이미 존재하는 SDK 선택 caller를 다시 만들지 않는다.

## 입출력·오류·custody 계약

입력: 실제 selected Voyage instance·policy·batch texts·actual A·retained slots. 출력: 기존 Native batch vector/artifact owner 또는 typed provider/native refusal. 다른 endpoint·stub·새 embedding engine으로 대체하지 않는다.

## DoD

[공통 D0–D3](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-28-e02-native-closeout/VERIFICATION.md)에 더해 아래 항목을 모두 충족한다. D1은 실제 producer→production caller→consumer와 실패·partial·drop 경계의 접합이며 helper만 제출하면 미완료다. 실제 owning positive/refusal/drop proof가 있어야 D2이며 public Runtime/SDK 소비는 D3다.

- [x] 소스 조건: configured adapter의 Native batch override와 실제 SDK 호출이 존재하고 Ordinary fallback0. 실제 전송 접합/D1 완료를 뜻하지 않는다.
- [ ] 독립 고정 response vector/model/order/count gold, 정상화 값과 차원·정책 binding이 맞는다.
- [ ] bounded local protocol server를 통한 실제 transport 양성 및 malformed/oversized/NaN/Inf/wrong-count/model/dim/HTTP error/short budget/cancel 음성이 통과한다.
- [ ] wire·decoder·vectors·error payload가 최고 Source 이후까지 보존되고 최종 owner drop 뒤 환급된다. cancel 뒤 후속 HTTP/write0.
- [ ] credential이 준비되면 실제 configured remote provider→SDK artifact 소비 D3를 수용한다. 없으면 remote만 BLOCKED, local code/behavior는 별도 판정.

## 검증 대상

기존 Voyage wire/adapter tests를 확장하고 genuine Original admission으로 allocation refusal를 실행한다. real provider provenance/credential은 P09 입력으로 전달한다.

기존 [P07 owning 요청](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-28-e02-native-closeout/VERIFICATION.md#p07의-별도-owning-요청)을 사용한다. Runtime `lib(test)` 전체 compile/link와 module 실행이며 다른 Runtime integration target·QE·L1 검증으로 대체하지 않는다. 새 feature class/lane을 만들지 않는다. 제안·작성된 테스트를 실행된 proof로 세지 않는다.

## 제외·재사용

- EmbeddingPort의 다른 provider default를 전역 활성화하지 않는다.
- stub/proof backend green으로 configured provider 수용을 대체하지 않는다.
- 원격 입력 누락을 Native 코드 구현 완료 또는 N/A로 계산하지 않는다.

## 착수 전 중복 확인

- 10-30 최신 대조는 actual source·dirty hunk·live claims와 읽기 전용 채팅 관측을 결합했다. P05 Root/B, L1 A–D, Graph L2, genuine 회귀 L3, Watch L4의 구현/검증을 C에 재배정하지 않는다.
- C의 enum 생산부/테스트 수리는 공식 claim으로 소유 경로만 등록·reconcile했다. claim은 잠금이나 compiler/body 수용 증거가 아니다. source/문서의 외부 자동 stage/commit을 C의 publication으로 보고하지 않는다.

다른 채팅 조회·메시지·하위 agent 생성은 이 패킷에 포함되지 않는다. 실제 기존 producer/consumer를 읽고 지정 파일·함수 안에서만 진행한다. 겹치는 hunk는 새 구현을 복제하지 말고 기존 writer와 P00의 source-local 인계 경계로 합류한다. claim은 잠금이 아니므로 path 일치만으로 작업 중복이나 작업 불가를 단정하지 않는다.

## 반환 형식

- 실제 변경 파일/함수와 caller 연결, 기존 구현 재사용/제외 항목.
- 입출력/서명 변경, 첫+secondary cause, partial/backing-last, final refund.
- 실행한 owning 명령·source/target/features·선택/실행/통과/실패/무시 수와 VERIFIED/FAILED/BLOCKED/NOT_RUN.
- 미접합 최소 patch와 정확한 supplier/consumer 함수. 구현하지 않은 입력·모드를 성공 또는 N/A로 보고하지 않는다.
