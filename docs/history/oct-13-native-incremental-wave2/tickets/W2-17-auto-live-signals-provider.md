# W2-17 — Activity/quality 신호 API와 provider/fairness 결합

> HISTORICAL REFERENCE — 2026-10-18 retirement. Current work/status: [Oct-17 SSOT](../../../plans/oct-17-index-closeout/tickets/README.md). Old TODOs, owners, commands and counts below do not authorize new work or qualify current source.


> `HISTORICAL_DESIGN_REFERENCE`. 현재 잔여/상태는 [Oct-17 SSOT](../../../plans/oct-17-index-closeout/tickets/README.md#w2-17)만 따른다. 아래 과거 TODO·진척·실행 순서는 신규 작업을 만들지 않는다. 이 파일에는 상태/담당을 갱신하지 않는다.

기존 요구/큐: R30,R32,R35,R40 / NI-08. 설계 계약은 [RFC](../../../rfc/oct-10-native-incremental-vnext.md), 실행 결과는 [VALIDATION](../../../plans/oct-10-index-closeout/VALIDATION.md)이 소유한다.

## Reuse

[x] `DONE_IMPL` 재사용 경계: SDK/UDS leased activity→recorder/Auto/provider·overlay foreground 재사용. 이 표시의 범위는 명시한 하위 구현이며 티켓 전체 검증 완료를 뜻하지 않는다. 새로운 결함이 확인되지 않으면 재구현·재배정하지 않는다.

## Scope

기존 activity/effort/provider 포트 아래 교체 가능한 Auto 알고리즘과 scoped 신호·원 budget·cancel/drain/fairness를 유지한다.

## Acceptance

- AUTO01–10의 scope/sequence/TTL/restart·duplicate/out-of-order·stale/Unknown/NaN·fake-clock boundary·policy replacement·Await progress·shadow no-sideeffect를 검사한다.
- 기존 replaceable algorithm port에서 detector sensitivity/knobs/hysteresis를 실험한다. same-intent budget·provider permits/draining·Core/Enrichment/Maintenance fairness를 유지한다.
- provider offline/saturation/noncooperative cancel·mixed load에서 Fast 유지/Deep truthful refusal·ghost-work charges·latency/freshness/served-rate/overhead를 함께 검사한다.
- 독립 quality label 부재는 Unknown이다. model/evaluator·reload·production shadow는 실제 capability가 없을 때 해당 subcase만 NOT_APPLICABLE. baseline invariants는 필수다. AUTO09 harness-only 허용; AUTO10 live comparison은 tuning 개선 주장 시 요구한다.

## Document signal contract

- Policy observation receives authenticated session, atomic corpus identity and sorted revision rows. A changed session/corpus cannot inherit another document clear window.
- Document unchanged age comes from captured update timestamps and is capped by the existing live foreground idle/TTL signal. Empty or retired-only snapshots and missing quality labels remain Unknown.
- Validation and age reads use the original RequestBudgetV1 while the query session owns its WorkBudget. No fresh deadline or cancellation scope is created.

## Source entry points

- [native_auto_signals.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/src/native_auto_signals.rs)
- [effort.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-core/src/index/effort.rs)
- [native_provider_signals.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/src/native_provider_signals.rs)

공용 parent/export/contract/config/registry 변경은 [단일 writer 규칙](../../../plans/oct-13-native-incremental-wave2/tickets/WRITE-OWNERSHIP.md)을 따른다. 검사 미실행을 기능 부재로 바꾸지 않는다. 결과에는 source·명령·범위·VERIFIED/FAILED/BLOCKED/NOT_RUN/NOT_APPLICABLE을 구분한다.
