# RBR 구조적 보완 — 병렬 구현과 직렬 통합

시작 source: `7cefac4a10a06ed56b6f5b9f42b3726468b1f198` + 공유 dirty, 2026-09-26. 현재 통합 source: `f9c3b4dc487a1b54a260e4ab2d3dd199310d3a5e` + 공유 dirty. peer commit으로 HEAD가 이동했으며 이 task에서 commit/push하지 않았다. 이전 `604149ed` 감사의 잔여를 현재 코드에서 재확인했다. 기존 searchd/CI/benchmark migration 변경은 별도 소유로 보존한다. clean-source 자격은 발급하지 않았다.

## 소유 경계와 상태

| lane | 소유 파일/계약 | 이번 구현 | 검증 상태 |
| --- | --- | --- | --- |
| parity-validator | embed crate, `parity_reference.py`, RBR-07 | strict schema2·9개 canonical adversarial input·norm/triangle 정합성·exclusive native capture | `VERIFIED` local — asset-free6, embed lib76(ignored3 제외), actual model 정상/capture 각1; invalid8+schema1은 모두 exit101로 거부. raw `/private/tmp/qi-rbr07-validator.2MFS28/`. transitive input drift/clean/T15 자격 제외 |
| query-observation | query dispatcher/routes/stage timing, daemon query config, RBR-01/09 | exact enabled/disabled; disabled stage clocks/Vec/storage 제거·DTO None; 공통512-byte reserve로 byte-fit 결과 유지 | `VERIFIED` local — search-plane 전체425 및 config38. raw `/private/tmp/qi-query-observation.3RQXR5/`. 실제 daemon A/B·tight-deadline 비용·quiet-host 측정은 별도 |
| ingest-observation | semantic build/adapter/core port, ingest dispatcher, IPC publish payload, SDK publish, RBR-10 | transient canonical report/status·V2 wire; required null; compact binding; DTO와 old-request fixture manual serde | `VERIFIED` local — 최종 contract146/SDK110 및 all-target Clippy; 별도 core semantic_stream6, semantic stage4, ingest binding/replay2. raw `/private/tmp/qi-rbr10-observation.sW0yqY/`. installed/full daemon·clean proof 제외 |
| serial integrator | benchmark main/sdk/diagnostics, Python run/spec/schema/tests/inventory, SDK roundtrip, 중앙 상태 문서 | diagnostic5/protocol3 server-config SHA·receipt/ACK·expected scope; strict scalar/fresh-sequence/canonical TrackId drift 거부 | 실제 첫 SDK17/17·actual sidecar 정상+변조5거부 local 관측. 소스 변경 중 SDK결과는 자격 아님. Python283 exact collection 회수, 최종 bytes 전체/Rust/SDK/daemon terminal 재회수 중. `/private/tmp/qi-rbr-integrate.Fg8v62/`. clean 자격 `NOT_RUN` |
| conditional-proof peer | embed/semantic proof exporters, `conditional_proof.py`, `query_timing_overhead.py`, RBR-12 | 원본 vectors/full rows·typed operation oracle·unaffected owner sentinel·strict terminal/scalars와 overhead 비교 | `VERIFIED` focused local4 최종 strict-cells; 독립 재감사 정상T16 5pass·기존4결함과 sentinel 삭제 거부. `/private/tmp/qi-rbr-conditional-final.JNYc3M/`. 실제 exporter terminal 대기; local self-reported custody는 외부 attestation 제외 |

같은 공용 파일을 복수 lane이 동시에 수정하지 않는다. 소유 밖 변경은 통합 담당에게 명시적 API/patch 요청으로 전달한다. 의미 있는 source/HEAD 변경마다 기존 실행 증거의 재사용 범위를 다시 확인한다.

## 통합 순서

1. parity validator를 독립 적용하고 asset-free mutant → actual pinned model 양성 → 같은 invalid artifact의 거부를 검증한다. 기존 tolerance 확대 금지.
2. query/ingest canonical 계약을 확정하고 producer→port→IPC/SDK→runner→sidecar→Python replay를 연결한다. 미관측/partial/replay를 0이나 성공으로 치환하지 않는다. durable receipt의 digest와 replay 의미는 유지한다.
3. 실제 SDK roundtrip·record/merge/replay, inventory exact equality, owning unit/integration과 좁은 Clippy/fmt를 확인한다. API/module/fuzz/daemon escalation은 canonical gate를 사용한다. raw artifact는 외부에 source/input/environment/binary/command/exit/digest를 포함해 기록한다. source 전후가 바뀐 실행은 현 입력의 자격으로 재사용하지 않는다.
4. 개발 실험은 원본 query/corpus/model을 고정한 유한 matrix로 실행한다. 청킹/ANN/fetch/ranking/ingest의 개별 원인과 품질/성능을 섞지 않는다. 근거가 부족한 제품 정책은 유지한다.
5. T15/T16은 해당 claim을 열 때 independent raw producer/terminal custody가 필요하다. 이 구현·검증 전 fail-closed를 유지하고 자기 보고 JSON으로 열지 않는다. external admission/gold/quiet host는 수동 작업 티켓이 아닌 qualified claim 입력 조건이다.

## 종료 경계

구현, local focused/integration, frozen-source contract/SDK receipts, 실제 비교 자격을 별도 갱신한다. shared dirty 작업은 허용하나 qualification으로 승격하지 않는다. 모든 lane 통합 전 `DONE`/`QUALIFIED`를 쓰지 않는다. `PAIR_VALID`/`QUALITY_DELTA`/`PERF_QUALIFIED` final claim은 현재 `NOT_RUN`이다.
