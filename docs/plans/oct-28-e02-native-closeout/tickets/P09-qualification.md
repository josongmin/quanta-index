# P09 — 단일 owning cohort·SDK/installed CP3·최종 잔여 판정

작성일: 2026-10-28 KST. 실행 지시용 패킷이며 당시 실행 상태는 **NOT_RUN**이었다. 이 작성 시점 표기를 현재 실행 verdict로 재사용하지 않는다. 최신 상태는 기존 E02 SSOT/VALIDATION이 소유하며, L1 bank의 확인된 구현·공용 접합과 남은 D 검증은 [2026-10-30 최종 감사](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-22-finalizing/VALIDATION.md#e02-l1-final-audit-20261030)를 따른다. 공통 계약은 [README](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-28-e02-native-closeout/README.md), [write map](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-28-e02-native-closeout/WRITE-MAP.md), [검증 계약](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-28-e02-native-closeout/VERIFICATION.md)을 따른다.

## 담당·선행·병렬 경계

- 담당: Root 검증 담당 1명. 같은 host heavy QBC/성능은 기존 admission과 큐에서 직렬 실행한다.
- 선행: 각 관련 P01–P08의 D1/D2와 실제 설치·provider·SDK 입력. 무관한 코드 lane은 외부 입력 때문에 정지시키지 않는다.
- 착수 조건: 입력/fixture·독립 oracle 준비는 즉시 가능. heavy 실행은 관련 코드 배치와 입력이 준비된 후 한 번 합류한다.
- Source/진척 SSOT: [기존 E02](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-22-finalizing/E02-publication-recovery.md). 이 패킷으로 기존 담당을 재배정하지 않는다.

## L3 소스 합류 후 owning 수용 경계 — 2026-11-01

[L3 source](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-22-finalizing/E02-publication-recovery.md#l3-source-only-closeout-20261030)의 공용27/31/34/35/39/37/40, private32/33/36/38/41/42는 실제 적용됐다. API/feature/caller 및14파일 정적 검사를 확인했으나 compiler/type/borrow/link/harness/body/runtime/제품 증거는 아니다. 오래된28/29와 apply-set은 재적용하지 않는다.

기존 exact G2/G3/cold owning rail이 실제 coherent source와 IFX normal dependency40을 수용해야 한다. G2 expected_tests1의 소스는5개 fresh fixture다. current-M proof-read,35 추가 유료 SELECT/9번째 test row, before-refusal probe와 각 fixture의 표준 work/memory/time 예산을 분리한다. registry/필수 denominator/예산은 올리지 않는다.41로 취소/unwind가 남기던 전역 fixture를 검사·소비해 다음 scenario를 준비한다.

whole packet/foreign/busy/observer-loan unwind/마지막 opaque alias·Source weak·7 live 축, 같은 admitted signal→actual V9 typed Cancelled, 확인 transaction snapshot의 exact M no-row·secondary, 실제 M producer marker unwind를 각 관측한다. 42는 취소/unwind의 fixture-only Original keeper와 검증용 lifecycle poll을 제거한다. 같은 유료 budget의 기록된 cause를 poll 없이 읽고 최종 Weak/7축을 검사한다. 최종 상태만으로 내부 packet DATA/Source 파기 시점 각각을 독립 계측한 것으로 계산하지 않는다. producer unwind에는 실제 unwind 가능한 owning profile이 필요하고 release panic=abort는 별도다.

현재 compiler/body/SDK/설치 worker/SQL/예산/process/product NOT_RUN/body0, SourceReady=false다. source-only·다른 chat 조회/연락 금지는 해제되지 않았다. 남은 실행은 기존 사용자 승인 owning executor가 담당한다.

## 현재 소스에서 확인한 사실

- 기존 QE334/334, 정책16/16, Runtime Native/no-default check는 각각 고정 소스 범위의 결과다. 합산해 현재 제품 통과로 계산하지 않는다.
- R34의 마지막 replay는 75/76 FAILED@14e87ec9. current Main 전체나 변경된 provider/DELTA의 동작 결과가 아니다.
- 이전 full boundary는 FAILED, nested workspace --locked는 BLOCKED였다. 현재 상태는 이 패킷 실행에서 재확인하며 오래된 실패 개수를 현재 버그 개수로 승격하지 않는다.

## 수정 허용 파일·함수

기본은 검증-only. 새 production source 수정은 결함이 확인된 원 패킷/owner로 반환한다. 공용 test-support mount·feature·lock은 P00/기존 dependency owner가 합류한다.

파일 전체 변경 권한을 의미하지 않는다. 위 함수 밖 caller/export/mount/Cargo 변경은 P00에 최소 patch로 반환한다. 새 private child/test 파일은 같은 기능 owner 아래에 만들 수 있으나 기존 parent mount는 지정 writer가 한 번 반영한다.

## 구현 순서

1. P01–P08의 실제 producer/caller/consumer·dirty hunk와 dependency manifest/lock/feature를 대조해 한 coherent source를 고정한다. read-only 계획 작성이나 unrelated source 변경을 이유로 동일 green을 반복하지 않는다.
2. 가장 작은 소유자 테스트/정적 검토 뒤 필요할 때 integration check를 먼저 수행하고, 통과한 같은 root/marker/class로 owning behavior target을 실행한다. check와 test 사이에 소스를 재캡처하지 않는다.
3. 기존 warm lane을 재사용한다. immutable root를 수정하거나 peer 파일을 사후 복사하지 않는다. source/inputs 변화가 있는 영향 rail만 다시 검증한다.
4. 실제 SDK→Runtime→native writer→query→GC→DELTA/restart→ACK/checkpoint를 같은 operation/target으로 수용한다. 기능별 외부 입력의 BLOCKED와 코드 실패를 분리한다.
5. E02 결과는 기존 E02 SSOT/VALIDATION 소유자가 갱신한다. E01/E03–E08 전체 종료·원4K/성능·IntelliJ/CI/release는 기존 DoD의 별도 gate로 남긴다.

## 입출력·오류·custody 계약

입력: source-bound 코드 패킷, registered QBC request, installed producer/consumer/worker provenance, independent gold, 실제 provider 설정. 출력: 각 gate의 VERIFIED/FAILED/BLOCKED/NOT_RUN과 정확한 source/target/선택·실행·통과 수/원인/잔여.

## DoD

[공통 D0–D3](/Users/songmin/Documents/code-new/quanta-index/docs/plans/oct-28-e02-native-closeout/VERIFICATION.md)에 더해 아래 항목을 모두 충족한다. D1은 실제 producer→production caller→consumer와 실패·partial·drop 경계의 접합이며 helper만 제출하면 미완료다. 실제 owning positive/refusal/drop proof가 있어야 D2이며 public Runtime/SDK 소비는 D3다.

- [ ] P01 원 Acked 회귀를 포함한 replay target 전체, QE 영향 owner 전체 및 guard 회귀가 해당 source에서 통과한다. ignored/filter 축소/0body green0.
- [ ] 실제 public Native GC·trace/evidence·Immediate/Deferred/no-store/reuse·configured provider·complete DELTA/restore가 D3를 충족한다.
- [ ] 같은 request/operation/attempt/target에서 first-FULL/Facts 기존5 durable cut, stale/foreign/CAS/cancel/unwind/ACK loss/last refund를 독립 oracle로 수용한다.
- [ ] 필수 native feature positive와 no-default negative/absence surface, boundary/root policy, --locked 정합성을 유지한다. lock 수정은 실제 dependency owner가 내용 근거를 갖고 수행한다.
- [ ] CODE_QUALIFIED/제품/설치/활성/rollback/CI/성능을 구분하고, 필수 gate의 미해결을 문서 완료나 작은 테스트로 상쇄하지 않는다.

## 검증 대상

구체 요청/순서/원 결과 재사용은 VERIFICATION.md를 따른다. 현재 계획 작성에서는 Rust/build/remote 실행 NOT_RUN이다.

새 selector/feature class가 필요하면 existing owning QBC resolver에서 먼저 확정한다. 제안한 테스트를 실행된 proof로 세지 않는다. compile·실제 실행 본문·제품을 별도 보고한다.

## 제외·재사용

- 새 acceptance store/영수증 SSOT·다른 QBC wrapper·검증용 serializer를 만들지 않는다.
- bare cargo, --locked 제거, budget/timeouts/필수 denominator 변경으로 통과하지 않는다.
- product 입력 부재를 N/A로 바꾸지 않는다.

## 착수 전 중복 확인

- 재조회 기준 S HEAD `7da55e44a0cea5fc25eceae65407f756bfe1de86`. 계획의 실제 write set만 대조했으며 claim은 잠금/함수 전체 소유권이 아니다.
- 이 write set과 교차하는 활성 exact-path claim 없음. 기존 SSOT 담당과 현재 dirty hunk까지 확인해야 착수할 수 있다.
- P09는 production/test 소스 수정권이 없는 검증 패킷이다. 기존 test target의 소유권은 각 구현 패킷/원 담당에게 유지한다.

다른 채팅 조회·메시지·하위 agent 생성은 이 패킷에 포함되지 않는다. 실제 기존 producer/consumer를 읽고 지정 파일·함수 안에서만 진행한다. 겹치는 hunk는 새 구현을 복제하지 말고 기존 writer와 P00의 source-local 인계 경계로 합류한다. claim은 잠금이 아니므로 path 일치만으로 작업 중복이나 작업 불가를 단정하지 않는다.

## 반환 형식

- 실제 변경 파일/함수와 caller 연결, 기존 구현 재사용/제외 항목.
- 입출력/서명 변경, 첫+secondary cause, partial/backing-last, final refund.
- 실행한 owning 명령·source/target/features·선택/실행/통과/실패/무시 수와 VERIFIED/FAILED/BLOCKED/NOT_RUN.
- 미접합 최소 patch와 정확한 supplier/consumer 함수. 구현하지 않은 입력·모드를 성공 또는 N/A로 보고하지 않는다.
