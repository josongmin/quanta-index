# W3 — 소스 통합·검증·admission ISSUE

[웨이브 전체 지도](../WAVES.md) · [에픽 지도](../README.md) · [티켓 인덱스](../tickets/INDEX.md)

- 상태: `PLANNED`. 본 문서의 product/model/capture/performance/release 실행은 `NOT_RUN`.
- 기준 배치 2개 티켓. 이 배치는 주된 수행 단계이며 PREPARE·후속 검증·새 epoch 반복은 다른 웨이브에서도 가능하다. 모든 티켓 종료를 한 번에 기다리는 전역 장벽이 아니다.

## 목적

producer까지 통합한 source를 검증하고 같은 source의 repository별 admission을 발행한다.

## 기준 티켓·작업 범위

| 티켓 | 담당 | 작업 | 이 웨이브의 실행 범위 |
| --- | --- | --- | --- |
| [O4-I0-02](../tickets/O4-I0-02-matching-source-proof.md) | I0 | 최종 source의 Contract·SDK·CI 검증 | 선택 epoch의 모든 producer/code proposal를 통합해 mandatory gates·Contract/SDK·matching binaries를 발행한다. |
| [O4-E1-03](../tickets/O4-E1-03-admission-and-split.md) | E1 | 최종 suite·split·admission 연결 | W1에서 producer를 PREPARE하고 W3 검증 뒤 repository별 suite/split/pack/admission을 ISSUE한다. |

## 진입 조건

- W0 scope·W1 code-ready proposals/labels와 이번 source에 포함한 W2 수리/최적화의 owner proof를 소비한다.
- E1-03 producer·E2 controller/collector/schema와 selected scorer 코드는 이미 PREPARE돼 있어야 한다. admission 발행 뒤 추가 patch로 실행을 보정하지 않는다.

## 반드시 지킬 내부 순서

| 순서 | 담당 / 티켓 | 결과 |
| --- | --- | --- |
| 3A PREPARE 통합 | I0 / I0-02 | selected product·collector·admission/scorer·SHARED schema/registry/dependencies를 하나의 source에 반영 |
| 3B VALIDATE | I0 / I0-02 + 해당 owner | 좁은 owner rail·변경 surface별 mandatory gates, 실제 Contract/SDK seam·source/input/binary·portable replay와 필요한 CI 상태 |
| 3C ISSUE | E1 / E1-03 | 검증된 같은 source의 repository별 suite/split/blind pack/license/review/proof와 admission |
| 3D 인계 | I0+E1 → E2/E4 | matching binaries·producer/controller identity·immutable 입력 경로/revision/digest와 required-cell scope |

## 범위별 gates

- public SDK/contract·wire·module·selection/state/ingress의 실제 변경 surface gates는 원래 I0-02 티켓과 AGENT_PLAYBOOK을 따른다.
- source/SDK/Contract/registry 실패·필수 raw/입력 부재면 **영향 scope만** FAILED/BLOCKED다. 실패한 gate를 건너뛴 source를 qualified로 발행하지 않는다.
- 이번 실행에 필요하지 않은 name/새 holdout/single-RPC/XL tier 완료를 baseline의 선행으로 추가하지 않는다.
- warmup1과 selected response boundary를 기본 입력으로 발행할 수 있다. warmup0 채택은 W4 parity 후 canonical protocol/config/admission binding을 다시 ISSUE한 다음 실행한다.
- config/input만 바뀌면 영향 producer/validation/ISSUE를 갱신한다. 코드·binary/source가 바뀌면 3A/3B를 다시 연다.

## 종료 조건·재개

- 각 ready repository/claim의 same-source source gates·matching binaries·admission이 결속돼 있다.
- E1-03은 I0-02 이후 ISSUE하며, E1-03 PREPARE는 그 이전이다. 두 티켓의 source 준비/발행 단계를 혼동하지 않는다.
- ISSUE 뒤 source drift·known failure·producer/consumer mismatch가 생기면 W3를 다시 연다.
- I0-03의 CODE qualification은 이 시점부터 자체 registry prerequisites가 준비된 범위에서 진행 가능하다. 제품 품질/전체 최적화 완료를 자동 선행으로 만들지 않는다.

## 인계 규칙

- 정확한 source/dirty ownership·proposal·independent oracle·selected command/result·scope/limits를 넘긴다.
- raw/input/binary/model identity·경로/digest는 원래 producer와 선택 verification 계약이 요구하는 범위에서 기록한다. 일회성 output은 checkout 밖 fresh root다.
- OWNED/SHARED/READ 파일과 명령의 상세는 원래 티켓을 따른다. SHARED는 I0만 공유 checkout에 반영한다.
