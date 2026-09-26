# RBR-12 — 사전 고정 평가 설계와 실제 비교·문서 종료

## 현행 판정 — 2026-09-26, [중앙 코드 감사](CURRENT-AUDIT.md) 기준

현재 `f9c3b4dc` + 공유 dirty 통합: native vector capture에 더해 `quanta-index-vector-proof`, `quanta-index-incremental-proof`, `semantic::proof` full-row export, `conditional_proof.py` schema2 replay가 main에 들어왔다. `run.py` T15/T16이 이 원본 validator를 사용한다. 아래 `604149ed`의 “양성 protocol 미구현”은 수정 전 기록이다. 독립 재감사에서 정상T16 5개 통과 및 no-op5·terminal3·dimension2·receipt3 거부, 무관 owner 삭제 fresh5 거부/raw5 실패를 회수했다. `/private/tmp/qi-rbr-conditional-final.JNYc3M/receipt.md`. **실제 exporter 양성 terminal·최종 frozen source custody는 아직 `NOT_RUN`**이다. claim=false는 `NOT_APPLICABLE`이다. query overhead replay 도구 구현과 실제 비용/quiet-host 자격은 별도다.

- 최신 전체 authority: Python 283개. 실제 collection/terminal 결과는 [CURRENT-AUDIT](CURRENT-AUDIT.md)를 따른다. 과거 276/276을 현재 증거로 재사용하지 않는다.
- T15는 component 0.002/cosine 0.005의 고정 tolerance, 정확한 inputs/정책/model2vec 0.9.0/assets와 native raw vectors를 검증한다. Semble native `max_length=512`와 controlled `None`를 동일 정책으로 취급하지 않는다.
- T16은 fresh와 delta의 같은 owner scope/실제 full rows 및 fault/restart 경계를 raw oracle로 검증해야 한다. summary/count만 같은 것은 row-set equivalence가 아니다. exporter와 source/model/dependency/config/binary/environment/명령/terminal custody의 결합 검증이 종료 조건이다.
- T16의 before→typed operation→fresh oracle를 독립 재도출하고 의미 없는 append/replace/tombstone/clear/membership을 거부한다. 무관 owner sentinel을 보존한다. terminal은 단 하나의 마지막 successful build event여야 하며 뒤 이벤트·모순/중복 event를 거부한다. dimension/receipt/exit 및 full-row u32/bool은 정확한 타입을 요구한다. 재감사 중 발견한 table 변수 shadow 회귀도 수정 후 정상 재생을 확인했다.
- 증거 경계: local binary/build hash self-report와 raw bytes 대조는 local custody다. OS 서명·원격 attestation·clean-source qualification으로 승격하지 않는다.

### 수정 전 판정 — `604149ed`

2026-09-26 `604149ed` 코드 재감사: admission v2가 development suite와 experiment-custody manifest의 raw/canonical digest를 묶고, capture/verdict 양쪽에서 source revision, repository commit, gold-bearing file(서로 다른 span 포함), exact gold block, query family, normalized/near-duplicate query를 거부한다. indexed corpus 공유는 허용한다. cross-suite validator 미구현 판정은 철회한다. T15/T16은 `pass`/count/digest만으로 승인하던 false positive를 차단했으나 raw 양성 proof 프로토콜은 **미구현**이다. 최종 holdout pair·quality/performance qualification은 `NOT_RUN`. [현재 감사](CURRENT-AUDIT.md)를 우선한다.

- 구현: 단일 suite train/eval 검사에 더해 admission v2의 development/holdout custody가 gold-bearing file·exact block·query family·near-duplicate query를 검사한다. 동일 indexed corpus는 허용한다.
- 검증: 이번 Python 전체는 **276 passed/32 subtests**, inventory **276/276 exact**, exit 0이며 cross-suite leak/위조, admission replay, T15/T16 summary-only refusal fixture를 포함한다. selected Python 입력과 HEAD는 전후 동일하나 공유 dirty local diagnostic이다. raw `/private/tmp/qi-rbr-source-audit.p3VfSl/python-audit.json`을 따른다. GIN/ripgrep v4 탐색 verdict는 과거 개발 진단이며 qualified final pair가 아니다. admitted final `PAIR_VALID`/`QUALITY_DELTA`/`PERF_QUALIFIED`와 claimed T15/T16 양성 raw proof는 `NOT_RUN`이다.
- 잔여 코드: T15/T16은 raw vector/증분 row-set producer와 정확한 실행 명령·source/model/dependency·binary/environment·collection/terminal receipt를 결합한 양성 protocol이 필요하다. 현재 `run.py`는 shape/digest/identity 검사 후에도 무조건 `raw proof protocol is not implemented`로 거부한다. claim=false면 해당 gate는 `NOT_APPLICABLE`; claim=true일 때만 필수다. 기본 pair 전체의 실행 버그로 표시하지 않는다. RBR-07 local parity test의 exit 0을 이 protocol로 대신하지 않는다. exact-block 범위를 넘는 definition identity 확장은 실제 승인된 claim이 요구할 때만 추가한다.
- 잔여 외부 자격: 독립 gold·admission, frozen corpus/model/spec, quiet host와 final single holdout pair/replay. 이는 개발 티켓의 수동 작업 항목이 아니라 qualified claim의 입력 조건이다. [현재 전수 판정](CURRENT-AUDIT.md).

- 우선순위: P1. 기존 split 검증·verdict framework는 관측됨; 독립 holdout·최종 pair/replay는 `NOT_RUN`. [현재 전수 판정](CURRENT-AUDIT.md).
- 선행: 설계·분리는 즉시. 최종 capture는 적용된 RBR-00~11 통합·검증 후.
- 성격: 개발 구현 완료와 외부 비교 자격을 분리하는 통합 티켓.

티켓 작성 당시 단일-suite 한계는 역사적 반례다. 현 `evaluator.validate_experiment_custody`와 admission v2에서 development/holdout suite를 함께 검증한다. 잔여는 이 구현의 frozen live admission/capture/replay proof이며 기존 v3 suite의 train/eval 의미는 사후 변경하지 않는다.

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
