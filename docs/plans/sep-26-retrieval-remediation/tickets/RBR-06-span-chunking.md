# RBR-06 — Span 회계 분리와 기존 청커 대조 실험

## 현행 판정 — 2026-09-26, [중앙 코드 감사](CURRENT-AUDIT.md) 기준

- 구현 관측: strict/line-aligned/whole-file 청커와 손계산 fixture, Quanta v5 indexed/SDK/scored span 계약·merge 표식 보존·evaluator rank-only exact-index Hit@1/MRR@10/Recall@10·context bytes/tokens 진단이 현재 owning 소스에 있다. primary density-aware projected NDCG는 변경하지 않았다. span 회계 재구현은 불필요하며 구형 v5 기록에 새 진단을 소급하지 않는다.
- 검증 경계: 과거 Rust lib82/chunking contract25·Python span/merge fixture·live SDK16은 당시 dirty local diagnostic이다. 현재 authority는 Python288/Rust108/SDK18이며 exact terminal과 source scope는 중앙 감사에 기록한다. clean-source producer→schema→replay→scorer proof와 외부 fixed matrix는 `NOT_RUN`이다.
- 추가 reachable 결함 수정: strict end의 backward snap과 nominal next-start forward snap이 overlap0에서 β/emoji2~4bytes 누락을 만들었다. 실제 before1failed(exit101) 후 next start를 emitted end 이하로 제한했다. 기존 identity에 원문 byte-union9+tiny-budget typed-refusal4를 보강했고 owning chunking25/25·scoped Clippy·fmt/diff exit0를 회수했다. `/private/tmp/qi-rbr06-utf8.t7SDk0/AUDIT.md`, SHA `964af2dbf520dbe72a12c85d4222933a53604d626a13608cd0f80d894d6f1107`. producer SHA `d27c78f2d2b12b96f1ba2c75cbb4ae16b3438bc08d1aefcb6f4054f326567f79`, test SHA `9636294715467fd932bb657745457518e4af6c87affad1044d058c3bd44d2296`, source frozen·ASCII cap/기본값/API/identity 불변. root current Rust108 actual terminal도 이 수정 이후 같은 selected inputs에서108/108·exit0로 검증했다. installed SDK의 더 이른 binary는 이 청커 수정 proof에 쓰지 않는다.
- 잔여 코드: 위 데이터 누락 보완을 제외하면 별도 청커 최적화는 측정 전 도입하지 않는다. 보조 지표의 source-bound receipt와 외부 fixed matrix는 별도다.
- 잔여 실험: 같은 질의/모델/route·1024 window·같은 overlap의 strict vs line-aligned, whole-file 대조군 원본 matrix·coverage·fallback을 외부 코퍼스에서 실행하고 한 후보를 development에서 선택한다. 외부 candidate corpus 존재는 admission/measurement 성공이 아니다. [중앙 코드 감사](CURRENT-AUDIT.md), [잔여 작업](GAP-REGISTER.md).

- 우선순위: P1. span 분리·보조 지표는 구현 관측, fixed matrix와 clean proof는 미완료. [현재 전수 판정](CURRENT-AUDIT.md). 선행: RBR-01/02/03.
- 성격: 확인된 회계 차이의 관측 강화. 현재 density scorer를 버그로 단정하지 않는다.

## 파일·함수

- [record.rs](../../../../benchmarks/retrieval/src/record.rs): `prove_hit`, `line_span_bytes` 사용부.
- [diagnostics.rs](../../../../benchmarks/retrieval/src/diagnostics.rs).
- [chunking/core.rs](../../../../benchmarks/retrieval/src/chunking/core.rs): `FileCoverage`, `CoverageReport`.
- [fixed_window.rs](../../../../benchmarks/retrieval/src/chunking/fixed_window.rs), [syntax.rs](../../../../benchmarks/retrieval/src/chunking/syntax.rs): `StrictWindowChunker`, line-aligned strategy, `parse_rust_items`.
- [evaluator.py](../../../../tools/benchmark/retrieval/evaluator.py): `ndcg_at_k`, diagnostic metrics, `collapse_by_file`.
- `tests/chunking_contract.rs`, `tools/ci/tests/test_retrieval_benchmark.py`.

## 작업과 유한 실험

1. 각 후보의 indexed bytes, SDK line span, projected/scored bytes와 expansion ratio를 기록한다. 지금의 full-line projection을 무음으로 exact-index-span으로 바꾸지 않는다.
2. primary density-aware NDCG 옆에 rank-only MRR/Hit@1, exact-span Recall@10, context bytes/tokens를 추가한다. 같은 gold에 대한 repeat credit은 현재처럼 제한한다. 지표 이름과 scorer identity를 분리한다.
3. 첫 matrix는 기존 `fixed_window_strict` vs `fixed_window_line_aligned`, 같은 window_bytes=1024와 같은 선언 overlap이다. whole_file은 진단 대조군. 같은 질의/모델/route에서만 비교한다. 기존 line-aligned 청커를 재구현하지 않는다.
4. Rust syntax 전략은 별도 Rust 하위집합 실험이다. 현재 top-level impl 전체/oversized/parse-error/non-Rust whole-file fallback을 함수 청킹 성공으로 계산하지 않는다.
5. RBR-04의 independent definition inventory가 준비되면 declaration cut ratio/size distribution/fallback rate를 추가한다. 개선 필요가 입증된 언어에만 grammar boundary chunking을 확장하고 새 profile로 평가한다.
6. overlapping-span 중복 제거는 별도 ablation이다. `collapse_by_file`의 첫 결과만 유지하는 동작을 생산 검색 deduper로 승격하지 않는다.

## 테스트·판단

- mid-line UTF-8, CRLF, 1024바이트 초과 한 줄, nested/긴 함수, attributes/decorators, 다중 정의/같은 파일 후순위 정답.
- 고정 손계산 예제로 exact 10바이트와 1MB context를 구분하고 rank-only는 같은 rank에 같은 점수를 준다.
- coverage/ID determinism, missing bytes, empty tokens, fallback reason, overlap union을 검증한다.
- 출력: matrix별 raw 후보·metric·coverage와 development 단계의 후보 선택. 청커 기본값 승격은 다른 조건부 변경과 함께 고정한 한 조합의 [TEST-PLAN](TEST-PLAN.md) 최종 holdout 판정 이후에만 한다.
