# RBR-12 — 사전 고정 평가 설계와 실제 비교·문서 종료

## 현행 판정 — 2026-09-26, [중앙 코드 감사](CURRENT-AUDIT.md) 기준

2026-09-26 후속 dirty 코드 판정: admission v2가 development suite와 experiment-custody manifest의 raw/canonical digest를 묶고, capture/verdict 양쪽에서 source revision, repository commit, gold-bearing file(서로 다른 span 포함), exact gold block, query family, normalized/near-duplicate query를 거부한다. indexed corpus 공유는 허용한다. T15/T16은 `pass`/count/digest만으로 승인하던 false positive를 차단했으나 raw 양성 proof 프로토콜은 **미구현**이다. 최종 holdout pair·quality/performance qualification은 `NOT_RUN`. 아래 기존 감사 문장은 수정 전 snapshot이며 [현재 감사](CURRENT-AUDIT.md)를 우선한다.

- 구현: 단일 suite train/eval 검사에 더해 admission v2의 development/holdout custody가 gold-bearing file·exact block·query family·near-duplicate query를 검사한다. 동일 indexed corpus는 허용한다.
- 검증: 현재 dirty overlay의 Python 전체 274 passed/32 subtests(exit 0, 이동 source local diagnostic)에는 cross-suite leak/위조, admission replay, T15/T16 summary-only refusal fixture가 포함된다. GIN/ripgrep v4 탐색 verdict의 `PAIR_VALID=pass`는 qualified final pair가 아니다. admitted final `PAIR_VALID`/`QUALITY_DELTA`/`PERF_QUALIFIED`와 T15/T16 양성 raw proof는 `NOT_RUN`.
- 잔여 코드: custody의 현재 exact block hash 범위를 넘어서는 semantic definition identity가 필요하면 외부 adjudicated inventory와 검증 경계를 확장한다. T15/T16은 raw vector/증분 row-set, 정확한 실행 명령·source/model/dependency, collection/terminal receipt를 결합한 양성 protocol이 필요하다. 현재 조작된 `pass` JSON은 fail-closed다.
- 잔여 외부 자격: 독립 gold·admission, frozen corpus/model/spec, quiet host와 final single holdout pair/replay. 이는 개발 티켓의 수동 작업 항목이 아니라 qualified claim의 입력 조건이다. [현재 전수 판정](CURRENT-AUDIT.md).

- 우선순위: P1. 기존 split 검증·verdict framework는 관측됨; 독립 holdout·최종 pair/replay는 `NOT_RUN`. [현재 전수 판정](CURRENT-AUDIT.md).
- 선행: 설계·분리는 즉시. 최종 capture는 적용된 RBR-00~11 통합·검증 후.
- 성격: 개발 구현 완료와 외부 비교 자격을 분리하는 통합 티켓.

현재 코드 감사(2026-09-26): `evaluator.validate_suite`는 한 suite의 train/eval만 검사한다. `suite.schema.json`에는 holdout split/외부 experiment identity가 없고, 현재 mutant fixture는 같은 파일의 disjoint train/eval을 통과시킨다. 따라서 두 별도 suite 사이의 같은 파일·정의·질의 가족 누수를 현 validator로 증명할 수 없다. 역사 v3 의미를 사후 변경하기보다, 개발 suite와 최종 holdout suite의 digest·repo/source identity·split key를 함께 받는 새로운 frozen admission 경계가 필요하다. 이는 독립 gold·quiet host 부재와 별개의 **코드 P1** 잔여다.

## 파일·함수

- [corpus_set.py](../../../../tools/benchmark/retrieval/corpus_set.py), `test_corpus_set.py`: 외부 corpus/manifest 생성, lexical/symbol 기계 라벨의 개발 용도.
- [evaluator.py](../../../../tools/benchmark/retrieval/evaluator.py): `validate_suite`, leakage checks, metric/stratified reports.
- [run.py](../../../../tools/benchmark/retrieval/run.py), suite/pair-spec/admission/run-manifest/verdict schemas.
- `contract_proof.py`, `sdk_proof.py`, `portable_proof.py`, `proof_inventory.py`, `proof-required-tests.json`.
- 기존 RB-01~06, `TEST-PLAN.md`, `CODE-SEARCH-BASELINE-2026-09.md`, 이 패킷 INDEX와 각 티켓의 실제 상태.

## A. 튜닝 전 준비

1. 기존 200문항과 노출된 변형은 development로 고정한다. exact/qualified symbol, lexical literal, semantic behavior, architecture/flow, no-answer를 별도 task family로 정의한다. architecture는 단일 함수 위치 문제가 아니라 복수 근거/관계 질의일 수 있음을 rubric에 반영한다.
2. repository/language/family별 표본 수와 split key를 선언한다. 같은 정의·파일·근접 질의 변형이 train/dev/holdout에 새지 않도록 기존 validator를 확장한다. 최소 표본은 현행 qualification floors와 CI 폭 목표로 먼저 정하고 결과를 본 뒤 줄이지 않는다.
3. 원본 질의, source-bound gold, scorer/config, 후보 실험 matrix와 성공 기준을 외부 manifest로 고정한다. 개발 데이터에서 하나의 최종 변경 조합을 선택한 뒤 holdout을 한 번 연다. Semble/Quanta output을 gold로 채택하지 않는다.
4. 자동 생성된 symbol labels는 개발 진단으로 사용한다. 독립 gold가 필요한 단계는 기존 admission으로 import/verify하며, 심사자 섭외나 수동 승인 작업을 이 티켓의 코드 작업으로 만들지 않는다.

## B. 통합 및 실행

1. 티켓별 write-set·source hash를 다시 확인하고 공용 파일을 직렬 통합한다. 실험 후보를 묶은 최종 profile과 기준 profile을 freeze하고 이후 후보를 바꾸지 않는다. source/protocol/model 변경 후 이전 receipt를 재사용하지 않는다.
2. RBR-00 inventory와 새 문서 closure 등록을 확인한다. 관련 계약/Rust/SDK rail을 최종 source에서 실행하고 full raw identity/terminal evidence를 검증한다.
3. fresh external spec으로 같은 원문 query/corpus를 두 시스템에 전달한다. native-default와 controlled lanes, 심볼 capability 실험은 각각 다른 명시적 profile이다. 동일 내부 알고리즘/청크라고 주장하지 않는다.
4. 원인 실험용 trace 실행과 성능 실행을 구분한다. 성능은 선언한 quiet-host/cache/repetition 조건, 동일 계측 수준으로 실행하며 mixed/contended timing은 diagnostic 처리한다.
5. public `retrieval-verdict`로 final promoted path에서 fresh-process replay하고 원본 verdict와 digest를 대조한다. raw result 누락/위조/relocation/policy mismatch 부정 테스트도 유지한다. holdout 결과로 다음 후보를 선택하지 않는다. 조합이 gate를 통과하면 묶음으로 채택하고, 실패하면 기준 조합을 유지한다.

## C. 완료 산출물과 경계

- source/binary/dependency/model/parser/config/host identities, exact commands와 raw results, candidate/coverage/latency/resource reports, stable artifact paths/digests.
- task family·route·repo/language별 quality 및 error/timeout/no-answer 결과. aggregate 한 줄로 lexical/semantic 성과를 섞지 않는다.
- 각 조건부 티켓의 채택/유지 결정과 그 근거. 여러 변경을 함께 판정했으면 개별 변경의 효과를 원인으로 단정하지 않는다. primary metric을 결과 확인 후 교체하지 않는다.
- 코드·테스트·문서 구현 상태와 `PAIR_VALID`, `QUALITY_DELTA`, `PERF_QUALIFIED`를 분리한 최종 표.
- 외부 독립 gold/승인/host 입력이 없으면 가능한 development capture와 구현 검증을 마치고 해당 qualification만 미완료로 명시한다. 자동 생성 receipt로 외부 사실을 꾸미지 않는다.

문서의 계획·상태·source-bound 계약 변경은 final proof **전에** 마친다. 최종 상태표와 결과는 레포 외부의 digest-bound closeout artifact에 기록한다. final receipts는 외부에 보존하고 추후 계약 변경 시 다시 발급한다. 공통 [TEST-PLAN](TEST-PLAN.md)이 이 티켓의 합격 기준이다.
