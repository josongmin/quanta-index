# RBR-03 — Semble native-default와 통제 실험 분리

## 현행 판정 — 2026-09-26, `f9c3b4dc` + 공유 dirty

- 구현 관측: `semble.py`의 네 profile·공통 cold/warmup/measured dispatch·lane/phase event와 `run.py`의 frozen profile 검증이 현재 owning 소스에 있다. 아래 99파일·20질의 Semble 0.6.0 캡처와 exploratory pair는 **과거 개발 관측**이다. profile 재구현이 아닌 실제 pinned output/phase capture 재발급이 잔여다.
- 검증: 과거 bare-symbol lexical-only 도구 5 tests와 GIN/ripgrep v4 exploratory verdict는 역사적 개발 진단이다. 당시 file Recall@10/latency·artifact digest는 [비교 기록](../../sep-23-retrieval-bench/tickets/CODE-SEARCH-COMPARATORS-2026-09.md)을 따른다. 이번 감사에서 새 Semble 실행 또는 해당 전체 과거 artifact의 재검증은 하지 않았다. 현 source의 pinned capture/contract/SDK receipt·admitted qualified final pair·quality/performance는 `NOT_RUN`이다.
- 잔여: 같은 frozen source/spec에서 네 profile의 pinned reference output·lane 호출·phase event·raw hash를 재발급하고 최종 validator collection/terminal에 바인딩한다. Python authority 283개는 실행 성공이 아니며 과거 276/280 결과를 현재 증거로 재사용하지 않는다. 독립 admission 없는 개발 캡처를 qualified pair로 승격하지 않는다. [중앙 코드 감사](CURRENT-AUDIT.md), [잔여 작업](GAP-REGISTER.md).

- 우선순위: P0. profile/phase 계약 구현 관측; 과거 개발 캡처는 역사적 증거이며 최종 clean-source receipt와 qualified pair는 `NOT_RUN`. [현재 전수 판정](CURRENT-AUDIT.md). 선행: RBR-00.
- 성격: 비교 모드 표기/실행 계약. Semble 0.6.0 과거 캡처와 새 실행의 실제 pinned dependency/source identity를 구분한다.

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

## 2026-09-26 과거 개발 실행 상태 — 현 source 재실행 아님

- 고정 Semble 0.6.0과 GIN 99파일·20질의 입력에서 4 profile을 현재 worker로 다시 실행했다. 각 실행 이벤트 40건, 파일 누락/추가/해시 불일치 0건, pure-lane 격리 확인. 원본 경로, SHA-256, 재현 명령은 [비교 실행 기록](../../sep-23-retrieval-bench/tickets/CODE-SEARCH-COMPARATORS-2026-09.md)에 있다.
- 어댑터가 총 BM25/semantic 호출 수와 각 이벤트의 lane 진입·candidate depth, 관측 alpha 범위를 대조하도록 보강했다. 관련 적대 테스트와 전체 Python retrieval 계약 테스트(287 passed, 32 subtests passed)가 통과했다.
- 같은 native-default profile을 Quanta와 Gin·ripgrep의 20질의씩 pair로 실제 실행했다. 두 promoted verdict 모두 `PAIR_VALID=pass`, 80/80 시도, 오류 0이며 replay가 byte-for-byte 동일하다. 점수·지연·원본 해시는 [비교 실행 기록](../../sep-23-retrieval-bench/tickets/CODE-SEARCH-COMPARATORS-2026-09.md)에 있다. 이 과정에서 hybrid lane-prefix와 multi-route capture 검증기의 실제 불일치를 발견해 수정하고 회귀 테스트를 추가했다.
- 위 실행은 한 번의 warmup·측정 반복으로 만든 개발 진단이다. 독립 gold, W0-B 승인, 현재 소스 clean receipt와 qualified pair는 없다. 두 exploratory verdict에서 `QUALITY_DELTA`와 `PERF_QUALIFIED`는 `not_applicable`이고, qualified claim 자체는 `NOT_RUN`이다.
