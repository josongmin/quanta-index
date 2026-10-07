# OCT-04 실행 지도

Status: `ACTIVE_RESIDUAL`

미완료 조건·실행 순서는 [단일 잔여 인덱스](tickets/INDEX.md)가 소유한다.
완료 구현은 [ADR](../../adr/README.md), 과거 handoff/ticket/실행은
[복구 인덱스](../../ARCHIVE-INDEX.md#historical-record-recovery)가 소유한다. 별도 wave/status 이력을 유지하지 않는다.

| Owner | 소유 경계 | 현재 실행 |
| --- | --- | --- |
| E1 | review/admission/evaluator/source oracle/split/gold/scoring | [labels/admissions/holdout/scores](tickets/INDEX.md#e1) |
| E2 | native external collector/index scope·Semble phases·required cells | [scope/capture/replay/join](tickets/INDEX.md#e2) |
| E3 | SDK binding·selection/view·maintenance/publish/operator | [완료 계약](../../adr/OCT-05-003-active-query-and-runtime-lifecycle.md); 후속 수용은 I0 |
| E4 | lexical lifecycle/query cost·scanner/scale/load·policy RCA | [cost/capacity/performance](tickets/INDEX.md#e4) |
| I0 | shared contracts/DTOs/registry/CI/dependency·source impact/release | [selected proof·CI·운영](tickets/INDEX.md#i0) |

SHARED: Cargo manifests/lock, Justfile/CI, schemas/proof registry, contract DTOs 및 runtime ingress/state.
한 owner가 반영하고 source/consumer/oracle·actual commands/results·포함/제외 scope를 인계한다.
E1→E2는 immutable suite/split/license/review/pack/admission, E2→E1는 native raw/index scope/clock/
required outcomes/unjudged keys, E1→E4/I0는 final qrels/report/denominators/policy 판정이다.

SQLAlchemy/Zellij는 검색 평가용 corpus이며 Zellij 작업은 최종 AI 정답 판정이다.
Semantica는 외부 producer다. R3 producer omission 및 전체 pair는 Quanta 자체 엔진 작업과 별도로
판정한다. Linux/운영 실행에는 실제 authorized host/config/state/retention/rollback 입력이 필요하다.
