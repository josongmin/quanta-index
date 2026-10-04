# O4-E3-01 — Active 선택·retention·view 획득 경합 재현

| 항목 | 값 |
| --- | --- |
| 에픽 / 담당 | [E3 — Active 선택·read-view lifetime·운영 계약](../epics/E3-selection-and-operational-safety.md) / E3 담당 |
| 우선순위 / 종류 | P0 / `PROOF_FIRST` |
| 기준 웨이브 | [W1 — 근거·정답·producer 병렬 준비](../waves/W1-evidence-and-producers.md) |
| 실행 상태 | dispatcher G1→G2→G3 barrier 및 runtime physical-retirement fixture `VERIFIED`; 유지보수 수리 뒤 current daemon profile213 passed/1 skipped 재검증. 별도 OS-child에서 같은 retention race는 `NOT_RUN` |
| 선행 결과 | 없음. 현재 source 확인과 fixture 준비부터 시작 가능 |

[전체 지도](../README.md) · [티켓 인덱스](INDEX.md)

## 목적

정적 위험으로 남은 un-tokened Active 선택과 view acquisition 사이의 lifetime 계약을 결정적 interleaving으로 판정한다.

## 배경과 현재 상태

selection.resolve_generation_selector_pin은 catalog head를 pin으로 해석하고 acquire_read_view는 이후 ledger/track handle을 획득한다. dispatcher barrier fixture `active_selection_reaped_before_view_acquisition_refuses_without_opening_g1`은 G1 선택→G2/G3 활성화·retention→G1 acquire를 고정한다. 관측 oracle는 `UNKNOWN_GENERATION` 및 lexical opener 0회다. `e2e_read_view::retired_selected_generation_refuses_before_open_while_fresh_active_serves_g3`는 별도 실제 daemon에서 양 track의 G1 physical retirement와 fresh G3 selected head/token을 검사하도록 통합됐으며 daemon profile 실행은 별도다.

## 착수 입력

- 현재 main·ActivationCatalog/retention/snapshot registry 실제 caller graph
- 고정 G1/G2/G3 source identity, independent expected selection/refusal 계약, barrier로 제어되는 concurrent fixture

## 어떤 파일을 어떻게 수정할지

`OWNED`는 에픽 담당 통합, `SHARED`는 I0 반영, `READ`는 기존 구현 소비다. 재현된 결함이나 채택된 계약 변경이 있을 때만 product source를 수정한다. 구현 파일과 독립 검증 파일을 함께 지정한다.

| 파일 | 함수 / 경계 | 구체적인 변경 또는 검증 | 모드 |
| --- | --- | --- | --- |
| [crates/quanta-index-search-plane/src/query_dispatcher/tests/read_view_lifetime.rs](../../../../crates/quanta-index-search-plane/src/query_dispatcher/tests/read_view_lifetime.rs) | view lifetime cases | 선택 직후·retention 직전·acquire 직전 barrier를 둔 deterministic case를 기존 fixture에 추가한다. | OWNED |
| [crates/quanta-index-search-plane/src/query_dispatcher/tests/read_view.rs](../../../../crates/quanta-index-search-plane/src/query_dispatcher/tests/read_view.rs) | Active/pinned refusal cases | un-tokened/tokened/explicit selector 기대값을 분리한다. | OWNED |
| [crates/quanta-index-search-plane/src/query_dispatcher/selection.rs](../../../../crates/quanta-index-search-plane/src/query_dispatcher/selection.rs) | resolve_optional_selection / resolve_joint_active_selection | READ: 현재 single/joint active selection authority를 추적한다. 재현 전 product 수정하지 않는다. | READ |
| [crates/quanta-index-search-plane/src/query_dispatcher/read_view/view.rs](../../../../crates/quanta-index-search-plane/src/query_dispatcher/read_view/view.rs) | acquire_read_view | 필요하면 test-only scheduling seam만 추가한다. sleep 기반 확률 재현을 만들지 않는다. | OWNED |
| [crates/quanta-index-searchd-runtime/tests/e2e_read_view.rs](../../../../crates/quanta-index-searchd-runtime/tests/e2e_read_view.rs) | runtime_fast_suite의 read_view module | 실제 daemon lifecycle에서도 admission/view 구간을 노출할 최소 fixed scenario를 추가한다. | OWNED |

## 실행 단계

1. Accepted SEP-21-002/003·SEP-27-005에서 각 selector의 허용 성공·NotReady·InvalidContract와 선택 권위를 확인한다. OCT-04-001은 Proposed이므로 현 계약의 결함과 제안된 더 강한 linearization 요구를 구분해 기대값을 확정한다.
2. barrier로 G1 selection을 멈추고 G2/G3 activation+retention을 완료한 뒤 G1 acquisition을 진행한다.
3. selected generation, token, physical handle, ledger/retained set, result rows를 독립 oracle와 비교한다.
4. already-acquired view case와 select-before-acquire case를 각각 실행한다.
5. 현재 관측된 typed refusal이 Accepted 계약의 허용 경계임을 E3-02에 넘긴다. 제안된 serve-after-selection 보장은 별도 채택 전까지 expected oracle로 사용하지 않는다.

## 2026-10-04 실제 daemon profile 결과

- Source `904043302f1db8406302a5a62bcffdc0d9412267`의 `just rust-profile test-daemon`은 exit0, 213 passed/1 skipped, tests259.597s였다. `runtime_fast_suite`의 `retired_selected_generation_refuses_before_open_while_fresh_active_serves_g3`를 포함한다.
- 이 fixture는 실제 UDS/runtime과 양 track의 physical retirement를 검사하지만 daemon은 같은 test process에서 구동한다. 별도 OS-child combined race 또는 Linux release proof로 승격하지 않는다. Accepted 계약의 retire-first typed refusal은 강한 pin-transfer 보장을 새로 채택했다는 뜻이 아니다.
- 유지보수 fatal ownership 수리 뒤 current `just rust-profile test-daemon`도 exit0,213 passed/1 skipped,tests231.161s였다. 후속 formal source는 clean `f3f7c68993f383e4ac5fdca761c111fe3d0edc3b`다. 위 동일-process fixture의 실제 범위를 확장하지 않는다.

## 검증 계획 — 위 실행 scope 외 NOT_RUN

아래는 실행할 명령/시나리오다. 본 문서에서 통과를 주장하지 않는다. `<...>`와 외부 root는 실행 전에 실제 값으로 확정한다. test filter는 실제 수집 ID를 확인하고 0 tests를 성공으로 표시하지 않는다.

- `./scripts/cargow test -p quanta-index-search-plane --lib --all-features --locked read_view`
- `./scripts/cargow test -p quanta-index-searchd-runtime --test runtime_fast_suite --all-features --locked e2e_read_view`
- Positive/negative: token drift, wrong explicit scope, retire-first refusal, active pair 수가 cache capacity 초과, panic/cancel handle release.
- selection/generation/state surface 변경 시 최종 gate는 just rust-profile test-daemon + owning P04 scenario.

## 완료 조건

- 실제 selected source에서 deterministic combined outcome이 반복 가능하며 expected contract와 일치/불일치가 명시된다.
- 정적 가능성, component pass, actual race outcome을 구별한다.

## 중단·거절·재개 조건

- 재현 실패를 engine bug 없다는 일반 증명으로 확장하지 않는다. 기존 acquire 후 lifetime tests만으로 완료 처리하지 않는다.
- 필요한 입력 부재는 `BLOCKED`, 미실행은 `NOT_RUN`, 실제 실행 실패는 `FAILED`로 기록한다. 조건 미성립 `NOT_APPLICABLE`에는 실제 판단 근거가 필요하다.
- 변경이 source/input/query/unit/result에 영향을 주면 [I0 source gate](O4-I0-02-matching-source-proof.md)와 영향받는 capture/report를 다시 판정한다.
- 일회성 raw/log/capture/receipt는 checkout 밖 새 root에 둔다. 기존 외부 terminal을 덮어쓰지 않는다.

## 인계 결과

- 실제 source/dirty ownership, 변경 파일과 계약, 실행한 명령/selector, 관측 결과 및 제외 범위.
- raw/model/runtime/binary/input identity는 해당 실행 계약이 요구하는 범위에서 기록한다.
- 완료 조건별 `VERIFIED`/`FAILED`/`BLOCKED`/`NOT_RUN`/`NOT_APPLICABLE`과 후속 티켓에 넘길 입력을 발행한다.
