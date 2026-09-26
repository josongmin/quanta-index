# RBR 구조적 보완 — 병렬 구현과 직렬 통합

시작 source: `7cefac4a10a06ed56b6f5b9f42b3726468b1f198` + 공유 dirty, 2026-09-26. 이전 `604149ed` 감사의 잔여를 현재 코드에서 재확인했다. 기존 searchd/CI/benchmark migration 변경은 별도 소유로 보존한다. branch/commit/push 또는 clean-source 자격 발급은 아직 하지 않았다.

## 소유 경계와 상태

| lane | 소유 파일/계약 | 이번 구현 | 검증 상태 |
| --- | --- | --- | --- |
| parity-validator | embed crate, `parity_reference.py`, RBR-07 | strict typed fixture·canonical adversarial coverage·norm/triangle 정합성 및 asset-free 반례 | `NOT_RUN` — 구현 중; 실제 pinned reference 재생성 후 양성/음성 모두 재실행 |
| query-observation | query dispatcher/routes/stage timing, daemon query config, RBR-01/09 | 실제 server stage observation on/off; default enabled; unknown selector startup 거부; off 미관측 | `NOT_RUN` — 구현 중; 결과/pin/deadline 동등성과 실제 daemon 경로 필요 |
| ingest-observation | semantic build/adapter/core port, ingest dispatcher, IPC publish payload, SDK publish, RBR-10 | durable receipt와 분리된 canonical transient outcome, request/repo/revision/batch/generation binding, executed/replay 구분 | `NOT_RUN` — 구현 중; activation은 별도 제어 호출로 server ingest 범위에서 제외/미관측 명시 |
| serial integrator | benchmark main/sdk/diagnostics, Python run/spec/schema/tests/inventory, SDK roundtrip, 중앙 상태 문서 | 위 두 관측 설정·원본을 sidecar/protocol/replay로 연결하고 identity 혼합/위조 부정 테스트 | `NOT_RUN` — 선행 API를 받아 직렬 통합 |

같은 공용 파일을 복수 lane이 동시에 수정하지 않는다. 소유 밖 변경은 통합 담당에게 명시적 API/patch 요청으로 전달한다. 의미 있는 source/HEAD 변경마다 기존 실행 증거의 재사용 범위를 다시 확인한다.

## 통합 순서

1. parity validator를 독립 적용하고 asset-free mutant → actual pinned model 양성 → 같은 invalid artifact의 거부를 검증한다. 기존 tolerance 확대 금지.
2. query/ingest canonical 계약을 확정하고 producer→port→IPC/SDK→runner→sidecar→Python replay를 연결한다. 미관측/partial/replay를 0이나 성공으로 치환하지 않는다. durable receipt의 digest와 replay 의미는 유지한다.
3. 실제 SDK roundtrip·record/merge/replay, inventory exact equality, owning unit/integration과 좁은 Clippy/fmt를 확인한다. raw artifact는 외부에 source/input/environment/binary/command/exit/digest를 포함해 기록한다.
4. 개발 실험은 원본 query/corpus/model을 고정한 유한 matrix로 실행한다. 청킹/ANN/fetch/ranking/ingest의 개별 원인과 품질/성능을 섞지 않는다. 근거가 부족한 제품 정책은 유지한다.
5. T15/T16은 해당 claim을 열 때 independent raw producer/terminal custody가 필요하다. 이 구현·검증 전 fail-closed를 유지하고 자기 보고 JSON으로 열지 않는다. external admission/gold/quiet host는 수동 작업 티켓이 아닌 qualified claim 입력 조건이다.

## 종료 경계

구현, local focused/integration, frozen-source contract/SDK receipts, 실제 비교 자격을 별도 갱신한다. shared dirty 작업은 허용하나 qualification으로 승격하지 않는다. 모든 lane 통합 전 `DONE`/`QUALIFIED`를 쓰지 않는다. `PAIR_VALID`/`QUALITY_DELTA`/`PERF_QUALIFIED` final claim은 현재 `NOT_RUN`이다.
