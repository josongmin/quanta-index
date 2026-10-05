# OCT-04 실행 지도

완료 구현·결정은 [Accepted ADR](../../adr/README.md#oct-05-implemented-contracts),
미완료 조건·29개 ID의 scope 판정은 [단일 잔여 인덱스](tickets/INDEX.md)가 소유한다.
[실행 웨이브](WAVES.md) ·
[초기 문서 복구](../ARCHIVE-INDEX.md#oct-05-handoff-and-ticket-compaction).

## 작업 목적과 범위

원본 5개 handoff의 합집합은 아래 세 범위를 포함한다. 진행률과 완료 판정도 범위별로 구분한다.

| 범위 | 목적 | 담당·단계 |
| --- | --- | --- |
| 엔진 구현·검증 | 검색·인덱싱·generation 수명·SDK 동작을 수리하고 변경된 source의 회귀를 검증 | E3/E4 및 I0-02 |
| 검색 품질 평가 | 독립 정답과 실제 검색 결과를 대조해 품질을 판단 | E1/E2 및 W1–W5 |
| 배포·운영 검증 | Linux 서버에서 supervision/readiness/state migration·배포·활성화·복구를 검증 | I0-03 및 W6 |

SQLAlchemy와 Zellij는 **검색 평가용 코드 저장소**다. 해당 잔여는 검색 질의와 후보 코드의
관련도 점수를 최종 확정하는 작업이다. Zellij 항목의 명칭은 **최종 AI 정답 판정**으로 사용한다.
Linux는 원본 `agent-1.md`의 Release 항목에서 인계된 서버 검증 대상이며, 실행에는 실제
host/config/state/retention/rollback 입력이 필요하다. 현재 로컬 검증 환경은 macOS다.

## 담당

| 담당 | 소유 경계 | 완료 구현 ADR | 실제 잔여 |
| --- | --- | --- | --- |
| E1 | review/admission/evaluator/source oracle/split/gold 및 scoring | [OCT-05-001](../../adr/OCT-05-001-review-admission-and-result-identity.md) | [E1](tickets/INDEX.md#e1): 실제 labels/admissions/holdout/final scores |
| E2 | native external collector/scope·Semble parent phases·required cells | [OCT-05-002](../../adr/OCT-05-002-native-capture-clock-and-index-scope.md) | [E2](tickets/INDEX.md#e2): 남은 cells와 실제 service reader/index authority |
| E3 | SDK response binding·selection/view/retention·maintenance/publish/operator | [OCT-05-003](../../adr/OCT-05-003-active-query-and-runtime-lifecycle.md) | 구현/요청 owner scopes 완료; shipping/source/release는 I0 |
| E4 | lexical lifecycle/query cost·scanner/scale/load 및 policy RCA | [OCT-05-004](../../adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md) | [E4](tickets/INDEX.md#e4): causal/A-B/performance/capacity·조건부 변경 |
| I0 | shared contracts/DTOs/registry/CI/dependency·source impact/release | [OCT-05-004](../../adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md) | [I0](tickets/INDEX.md#i0): 최신 source/CI/paired/Linux/P11 authority |

## 통합과 자원

- 동일 owner source는 한 담당자가 통합한다. SHARED hunk는 I0가 반영하고 각 담당은
  concrete proposal·consumer impact·independent oracle·좁은 actual 결과를 넘긴다.
- SHARED: Cargo manifests/lock, Justfile/CI, shared schemas/proof registry, contract DTOs,
  benchmark record/query/diagnostic seam 및 runtime ingress/state. Source 조사 권한과 edit owner는 구별한다.
- 순서: PREPARE → 해당 변경의 source/binary/mandatory surface VALIDATE → repository admission ISSUE.
  Final qrel·policy·timer·SDK/storage 변경은 영향 범위를 다시 검증한다. 모든29개를 기다리는 전역 장벽은 없다.
- Source/fixture/static 작업은 병렬 가능하다. 실제 build/test/model/Docker/native/scale/performance는
  I0가 host별 직렬 admission을 관리하며 실패 sibling 때문에 ready 작업을 대기시키지 않는다.

## 인계

| 인계 | 필수 사실 | 거절 경계 |
| --- | --- | --- |
| owner → I0 | owned diff/shared proposal, source/consumer/oracle, actual commands/results, epoch 포함/제외 | 소유 충돌·known correctness failure·mandatory input 누락 |
| I0 → E1/E2/E4 | 실제 source/dependency/config/binary, selected gates, producer/collector revision | stale binary·wrong selector/source·missing proof |
| E1 → E2 | suite/split/license/review/pack/admission immutable paths/bytes/revisions | unknown grade·threshold/source drift·mutable latest |
| E2 → E1 | native request/raw/rows, product/runtime/index scope/clock, required-cell outcomes, unjudged keys | returned hits를 universe로 승격·missing을 empty로 치환 |
| E1 → E4/I0 | final qrel/report, denominator/coverage/CI, unit/query intent/exposure 및 policy disposition | pool-only no-answer·AI를 human·exposed를 unseen으로 표시 |
| E3/E4 → I0 | implemented/conditional/not-applicable disposition과 owner/process scope | component pass를 shipping Linux/power-loss/operations로 승격 |

## 재사용과 재실행

- qrel/grade만 변경: native query/source/unit/profile/result binding이 허용하면 원 raw를 참조해 재채점한다.
  그렇지 않으면 영향 cells를 재캡처한다. 옛 record의 suite/qrel digest를 새 값으로 덮어쓰지 않는다.
- scorer numeric/CI 변경: independent method/seed/draw/reduction parity 및 영향 report replay.
- query/source/unit/model/index/runtime/profile/warmup/clock 변경: 해당 request/response 권위의 fresh 실행.
- SDK/selection/storage/planner/policy 변경: 해당 I0 source epoch·owner gates·matching binaries 이후 영향 cells/report.
- 무관한 제품 raw까지 일괄 폐기하거나 historical counts를 최신 qualification으로 사용하지 않는다.

원본5 handoff·개별29 tickets·epic/wave 상세는 Git 이력으로 회수한다. 이 parent packet은
labels, measurement, current-source 및 release/operations가 남아 있어 계속 active다.
