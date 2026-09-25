# RBR-03 — Semble native-default와 통제 실험 분리

- 우선순위: P0. 구현/검증: `NOT_RUN`. 선행: RBR-00.
- 성격: 비교 모드 표기/실행 계약 공백. 설치된 Semble 0.6.0을 기준으로 한다.

## 파일·함수

- [semble.py](../../../../tools/benchmark/retrieval/semble.py): `WORKER_TEMPLATE`, worker spec, native output/normalization.
- [run.py](../../../../tools/benchmark/retrieval/run.py): spec validation, frozen inputs/protocol, replay.
- 같은 디렉터리 `pair-spec.schema.json`, `run-manifest.schema.json` 및 Python adapter fixtures.
- 외부 pinned `semble/search.py::search`, `_search_bm25`, `_search_semantic`, `SembleIndex.search`는 reference이며 이 저장소에서 vendor 코드를 수정하지 않는다.

## 고정 profile

1. `native-default`: upstream alpha 자동 선택 및 기본 rerank를 유지. 제품 전체 비교로 표시한다.
2. `hybrid-no-rerank`: 명시적 alpha와 rerank=false. alpha={0,0.5,1}은 **점수 ablation**이며 단일 engine latency가 아니다.
3. `lexical-only` / `semantic-only`: pinned upstream 단일 lane 함수를 호출하여 해당 lane만 실행. 필요하면 version-specific bridge를 얇게 두고 함수/source hash와 library version을 묶는다. 사용할 수 없으면 명시적으로 unsupported; dual-lane fallback 금지.

## 작업

- cold/warmup/measured가 같은 profile dispatch 함수를 호출하게 한다.
- 요청 profile, 실제 alpha, rerank, lane 호출, 내부 candidate depth, tokenizer/encoder 정책과 chunk profile을 남긴다. 위 세 profile을 동일 결과열로 합치지 않는다.
- pure-lane은 top-k에서 직접 reference 결과를 얻고, hybrid는 upstream overfetch를 그대로 기록한다. 내부 후보량이 같다고 가정하지 않는다.
- native chunking 차이는 제품 비교 특성으로 남긴다. 공통 청크 실험을 추가한다면 별도 profile로 표시하고 byte universe와 mapping proof를 유지한다.
- 스코어링/랭킹을 adapter에서 재구현하지 않는다. 벤치 mode 변경은 freeze/replay binding 변경과 함께 반영한다.

## 테스트·완료

- spy/failing sentinel로 lexical-only에서 encode/dense 호출 0, semantic-only에서 BM25 호출 0을 증명한다.
- alpha 0/1의 hybrid profile은 여전히 dual lane임을 검증한다.
- native-default 결과가 같은 pinned upstream 호출과 일치한다. version/API mismatch와 unknown mode는 거부한다.
- actual settings 위조, phase별 mode 차이, 파일 누락, 다른 SHA의 corpus를 거부한다.
- 각 profile의 real pinned Semble 개발 캡처와 raw output이 남는다. strict pair 자격은 [TEST-PLAN](TEST-PLAN.md)와 RBR-12에서 별도 판정한다.
