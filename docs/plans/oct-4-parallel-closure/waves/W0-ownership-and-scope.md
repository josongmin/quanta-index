# W0 — 소유권·실행 범위 고정

[웨이브 전체 지도](../WAVES.md) · [에픽 지도](../README.md) · [티켓 인덱스](../tickets/INDEX.md)

- 상태: `PLANNED`. 본 문서의 product/model/capture/performance/release 실행은 `NOT_RUN`.
- 기준 배치 1개 티켓. 이 배치는 주된 수행 단계이며 PREPARE·후속 검증·새 epoch 반복은 다른 웨이브에서도 가능하다. 모든 티켓 종료를 한 번에 기다리는 전역 장벽이 아니다.

## 목적

담당·SHARED 파일·host resource와 이번 실행 scope를 먼저 고정한다.

## 기준 티켓·작업 범위

| 티켓 | 담당 | 작업 | 이 웨이브의 실행 범위 |
| --- | --- | --- | --- |
| [O4-I0-01](../tickets/O4-I0-01-ownership-and-contract-freeze.md) | I0 | 공통 파일 소유권·계약·source epoch 관리 | 파일/hunk·SHARED 반영 담당·host admission·출력 namespace·이번 claim을 확정한다. |

## 진입 조건

- 현재 checkout의 HEAD/dirty와 앞선 감사 문서 변경을 확인한다. 사용자/다른 담당 변경을 덮어쓰지 않는다.
- 선택할 결과 범위를 구분한다: quality diagnostic, name 회수, unseen 정책, completed speed, 대규모 capacity/tail/restart, CODE/release/actions.

## 실행 순서

1. I0 한 명이 SHARED 파일 반영·공용 contract·source epoch를 맡고 E1–E4별 OWNED 파일 담당을 고정한다.
2. build/cache/fixture root·외부 index/model jobs·출력 namespace·heavy workload 시간대를 resource별로 배정한다. 같은 host의 heavy builds·제품/ingest/scale/성능은 직렬이다.
3. 이번 baseline/개선 epoch에 포함할 코드와 필수 correctness scopes를 정한다. 알려진 해당 scope 결함은 미포함/optional로 숨기지 않는다.
4. Proposed OCT-04-002/003의 actual operator/producer 입력 담당과 need/no-need/missing-input 판정을 I0-01에 기록한다. 필요성 미확인 구현을 합성하지 않는다.
5. W1의 4개 에픽 담당을 병렬 착수시킨다.

## 인계물·종료 조건

- 파일/hunk·SHARED proposal 반영 방식, host resource와 namespace, 선택 claim·필수 gates가 확정돼 있다.
- W0는 소유권/선택 범위 종료다. source 검증·admission·CODE qualification을 완료한 상태가 아니다.
- 실제 구현 중 contract/input이 바뀌면 I0-01은 계속 갱신한다.

## 인계 규칙

- 정확한 source/dirty ownership·proposal·independent oracle·selected command/result·scope/limits를 넘긴다.
- raw/input/binary/model identity·경로/digest는 원래 producer와 선택 verification 계약이 요구하는 범위에서 기록한다. 일회성 output은 checkout 밖 fresh root다.
- OWNED/SHARED/READ 파일과 명령의 상세는 원래 티켓을 따른다. SHARED는 I0만 공유 checkout에 반영한다.
