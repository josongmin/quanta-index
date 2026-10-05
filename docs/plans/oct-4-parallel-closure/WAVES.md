# OCT-04 후속 실행 웨이브

[단일 잔여 인덱스](tickets/INDEX.md) · [담당·인계](README.md).
완료 C1–C3 구현·회귀 단계는 [Accepted ADR](../../adr/README.md#oct-05-implemented-contracts)에 압축했다.
아래는 **Quanta에서 남은 실행 순서**다. [작업표](tickets/INDEX.md#quanta에서-할-작업)가
현재 범위를 소유하며 외부 producer 연동은 아래 별도 기록에 보존한다.

## 코드 우선

- 실제 미구현: [I0-03 P11 typed operational producer/recipes](tickets/INDEX.md#o4-i0-03).
  실제 actions와 독립 pre/post 성공 판정 계약·authorized target 입력이 필요하다.
- Frozen5796 Large default timeout과 XL posting admission 거절이 재현됐다.
  E4-02 source durable publication과 bounded authority 수리는 W2의 확정 작업이다.
  E1-07/E4-04/E4-07은 실제 병목·독립 정책 실패 뒤에만 채택한다.
- E1/E2/E3 및 scale/scanner/proof의 기존 구현은 실행 증거가 부족하다는 이유로 재작성하지 않는다.
- Large full seal68.733s 중 explicit sync49.778s를 관측했다. Parent sync 묶기만으로
  default30s 여유를 확보하지 못하므로 immutable pack/root와 reader/query budget을 함께 수리한다.
  Delta/noop residual의 exclusive owner 비용은 추가 계측 후 판정한다.
  Bat 재발행은 외부 issuer의 original review pack/merged product pack 신원 재생이며 새 relevance 판정이 아니다.

## 별도 producer 연동

- R3의 dispatch 전 독립 expected semantic replace/tombstone/unchanged 범위·누락 대조는
  Semantica source-plan/shadow policy/prior state/cluster plan owner 작업이다.
  [I0-03](tickets/INDEX.md#o4-i0-03)에 연동 수용 잔여로 보존하며 Quanta 자체 코드 건수에 합산하지 않는다.
- R5 clean pair는 producer 연동 수용 범위다. Quanta 단독 엔진·벤치의 완료 조건과 구별한다.

## 확인된 실행 체크포인트

- Frozen5796 Contract Python791/Rust191와 fresh release SDK27은 actual 및 독립 portable replay `VERIFIED`.
- 같은 source의 별도 release `scale_matrix` build와 small16/medium256 causal run은 `VERIFIED_DIAGNOSTIC`.
  Large default30s timeout과 XL preflight4M posting 거절은 `FAILED`다.
  Large300s/256MiB는127.612s에 완료한 별도 진단이며 default closure가 아니다.
  Current78d2474 Medium256 OS-child stop/reap/restart 회귀는1passed/82unselected·31.805s로 `VERIFIED`.
  Large/XL OS-child restart·open-loop·quiet-host 성능은 `NOT_RUN`이다.
- ready9 OpenGrok selected-request180은 완료; 전체 service/index 권위는 미완료다.
  Frozen5796 CLI admission20tasks/519pairs만 actual exit0이다. Django admission은 중단했고
  나머지 admissions·pair·SG/CS·full5 join은 완료 증거가 없다.
- Frozen5796 Gin declaration1,196 fresh single-route oracle/capture/scoring 진단은 완료했다.
  1,192success/4capped 및 declaration MRR@10=1.0은 그 분모의 diagnostic이며 독립 holdout/비교/PERF가 아니다.
- Current history/admission Python owner58cases는 통과했다. Broad Python은 current file-pair
  fixture의 불완전한 manifest에서 실패했고 hosted Rust의 계측 Clippy6건도 수리 후 재실행이 필요하다.
- 이후 source/ADR/Justfile 변경의 proof는 새 epoch로 발행한다. 위 frozen 결과의 SHA는 변경하지 않는다.

## W0–W6

| 웨이브 | 실제 다음 작업 / 담당 | 선행·인계 및 종료 경계 |
| --- | --- | --- |
| W0 | I0: 후속 source/dirty/hunk owner와 claim/input 범위 확인 | Shared 단일 owner, 실제 source 영향·fresh namespaces. 상시 통합 규칙 |
| W1 | E1: SQL/Zellij 최종 판단·Tailscale rubric·새742pairs·holdout 준비. E2: actual reader/index scope. E4: causal/full-caller profile | 각 input/quota/host 범위만 BLOCKED. 완료 raw 보존, 독립 strata/oracle·조건 판정 |
| W2 | E4: 확인된 Large sync/XL bounded authority 수리. E1-07/E4-04 조건부 최적화 | Source pack/root·reader/query 한 번의 cutover, global cap/단계별 work·crash custody 및 independent oracle. Owner regression→I0 영향 검증 |
| W3 | I0-02: selected source Contract/SDK/CI. E1-03: final revisions 및 남은3repo admission ISSUE | PREPARE→VALIDATE→ISSUE. Frozen old proof를 최신 전체 source로 재표기 금지 |
| W4 | E2: ready required cells actual capture/independent replay/join·scope별 warmup parity. E4: A/B·capacity·qualified performance | Matching binaries/input/index/clock, 실제 host/schedule. Failed sibling은 ready cells를 막지 않음 |
| W5 | E1: 마지막 unjudged union→labels/admission→independent scores/CI. E4: holdout 기반 정책 RCA | Qrel-only reuse 허용 여부 확인. Name/NL/no-answer/ARB/B09 분모·human/unseen 범위 별도 |
| W6 | I0-03: Quanta 운영 producer/recipes·real provider·Linux release/state·P11 actions·aggregate | 실제 target/독립 관측 계약·authorized inputs/results. Registry의 외부 pair는 별도 연동 수용 범위 |

## 현재 dependency

- E1-01 valid judgments → E1-02 supplemental merge → E1-03 admissions.
- I0-02 current-source proof → 해당 E1-03 ISSUE/E2-04/E4-05·06 실행.
- E2-02 scope 및 E2-03 required inventory → ready E2-04 native capture/replay/join → E1-06 final pool.
- E2-06 parity를 갖춘 scope만 quality warmup0; 나머지는1. Qualified speed는 warmup≥1.
- E4-01 isolated cost → E4-02 barrier; E4-01/03 after-scanner cost → E4-04 token authority.
- E1-04/05/06 독립 qrel/span/holdout → E4-07 정책 판단. 새 holdout은 ready 기존 cohort 재채점의 전제는 아니다.
- E3-01/03/04/05/06의 완료 owner/process scope는 I0 matching shipping-source/release에서 소비한다.
  E3-02 pin transfer는 현 Accepted 계약에서 비적용이며 dependent code의 대기 조건이 아니다.
- P11 contract/target 입력 → existing typed authority/recipe 구현·검증 → actual action 실행 → aggregate.

## Runtime 실행 규칙

- Source 조사·fixture/static은 병렬; 실제 owner tests/build/model/Docker/native/scale/performance는 host별 직렬.
- 실패·blocked·missing·unsupported·미선택 셀을 inventory에서 제거하지 않는다.
- Raw/input/proof 영향을 받은 범위만 재검증하며 이전 실패/partial와 source identity를 보존한다.
- Wave 진입은 repository/claim별이다. 모든 labels·최적화·Linux inputs를 기다리는 전역 barrier가 아니다.
- 실제 명령·입력·완료/거절 조건은 [잔여 인덱스](tickets/INDEX.md)를 따른다.
