# OCT-04 후속 실행 웨이브

[단일 잔여 인덱스](tickets/INDEX.md) · [담당·인계](README.md).
완료 C1–C3 구현·회귀 단계는 [Accepted ADR](../../adr/README.md#oct-05-implemented-contracts)에 압축했다.
아래는 **남은 실행 순서**이며 초기 PLANNED 상태나 과거 terminal 진행표를 복사하지 않는다.

## 코드 우선

- 실제 미구현: [I0-03 P11 typed operational producer/recipes](tickets/INDEX.md#o4-i0-03).
  실제 actions와 독립 pre/post 성공 판정 계약·authorized target 입력이 필요하다.
- E1-07/E4-02/E4-04/E4-07은 실제 병목·독립 정책 실패 뒤에만 채택하는 조건부 변경이다.
- E1/E2/E3 및 scale/scanner/proof의 기존 구현은 실행 증거가 부족하다는 이유로 재작성하지 않는다.

## W0–W6

| 웨이브 | 실제 다음 작업 / 담당 | 선행·인계 및 종료 경계 |
| --- | --- | --- |
| W0 | I0: 후속 source/dirty/hunk owner와 claim/input 범위 확인 | Shared 단일 owner, 실제 source 영향·fresh namespaces. 상시 통합 규칙 |
| W1 | E1: SQL/Zellij 최종 판단·Tailscale rubric·새742pairs·holdout 준비. E2: actual reader/index scope. E4: causal/full-caller profile | 각 input/quota/host 범위만 BLOCKED. 완료 raw 보존, 독립 strata/oracle·조건 판정 |
| W2 | 확인된 defect 수리 또는 E1-07/E4-02/04의 조건부 최적화 | 실제 반례/비용 실패·accepted contract 뒤에만 code. Owner regression→I0 영향 검증 |
| W3 | I0-02: selected source Contract/SDK/CI. E1-03: final revisions 및 남은3repo admission ISSUE | PREPARE→VALIDATE→ISSUE. Frozen old proof를 최신 전체 source로 재표기 금지 |
| W4 | E2: ready required cells actual capture/independent replay/join·scope별 warmup parity. E4: A/B·capacity·qualified performance | Matching binaries/input/index/clock, 실제 host/schedule. Failed sibling은 ready cells를 막지 않음 |
| W5 | E1: 마지막 unjudged union→labels/admission→independent scores/CI. E4: holdout 기반 정책 RCA | Qrel-only reuse 허용 여부 확인. Name/NL/no-answer/ARB/B09 분모·human/unseen 범위 별도 |
| W6 | I0-03: exact pair·real provider·Linux release/state·P11 actions·aggregate | 실제 registry prerequisites와 authorized inputs/results. 상태 정리만으로 qualification 종료 불가 |

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
