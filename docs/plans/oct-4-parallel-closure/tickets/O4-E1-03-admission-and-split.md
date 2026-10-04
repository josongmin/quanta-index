# O4-E1-03 — 최종 suite·split·admission 연결

| 항목 | 값 |
| --- | --- |
| 에픽 / 담당 | [E1 — 정답·검수·admission과 독립 평가](../epics/E1-labels-admission-and-gold.md) / E1 담당 |
| 우선순위 / 종류 | P0 / `INTEGRATION` |
| 기준 웨이브 | [W3 — 소스 통합·검증·admission ISSUE](../waves/W3-source-validation-and-admission.md) |
| 실행 상태 | Django/TypeORM suite·review receipt 및 source107 bat/cli/lo/nushell fresh admission `VERIFIED`; 다른 ready admission 중앙 발행 진행 중. 전체 cohort qualification 미완료 |
| 선행 결과 | [O4-E1-02](O4-E1-02-supplemental-labels.md), [O4-I0-02](O4-I0-02-matching-source-proof.md) |

[전체 지도](../README.md) · [티켓 인덱스](INDEX.md)

## 목적

검수된 라벨, objective/no-answer strata, split과 실행 receipt를 하나의 기존 admission 경로에서 일치시킨다.

## 배경과 현재 상태

현 run.py는 validate_admission_manifest와 verify_admission_bundle, corpus_binding은 validate_split_manifest를 이미 갖는다. NL-only diagnostic과 mixed-track decision을 구분한다. 과거 C5 4개 stale exclusion은 B08 cohort가 여전히 요구하는 경우에만 새 입력을 발행하며 B09 global12와 합치지 않는다.

## 2026-10-04 원본 검수와 current 발행 경계

- first-six의 suite/pack/annotation/adjudication/license/corpus/split 원본 bytes를 결속한 외부 current admission input/issuer를 준비했다. 옛 source-bound admission/result/host/proof를 재사용하지 않으며 fresh matching Contract/SDK proof와 실제 model/cache/environment가 필요하다.
- `VERIFIED`: `/tmp/quanta-e1-django-typeorm-suite-issuer-20261004.py --input /tmp/quanta-e1-django-typeorm-suite-input-20261004.json --output-root /Users/songmin/Documents/code-new/qi-e1-django-typeorm-issued-20261004-v1`를 source `90404330`에서 실제 실행해 exit 0이었다. Django20 tasks/509 pairs, TypeORM20/572를 completed forms→기존 `holdout_review.finalize_file_review_labels`→`evaluator.validate_suite`→기존 review receipt verifier로 다시 발행했다.
- 새 root에는 각 repository의 suite/blind pack/annotation2/adjudication/validation이 있다. `qualified:false`, `human_provenance_attested:false`; 새 모델 호출·수동 grade·human review 승인·제품 capture/admission 증거가 아니다.
- SQLAlchemy/Zellij/Tailscale의 original review와 canonical suite 발행, actual fresh admission 및 supplemental merged revision은 완료 전이다. 과거 supplemental union은 E1-02의 missing capture record blocker를 유지한다.

## 2026-10-04 source107 admission 실제 발행

- clean source107의 Contract v3 및 fresh release SDK v2 context를 검증한 후 `/tmp/quanta-e1-current-admission-isolated-v2-20261004.py --input /tmp/qi-e1-eight-actual-proof-1071692b-20261004-v1.json --output-parent /Users/songmin/Documents/code-new/qi-e1-first-eight-admission-20261004-v1 --selection first-eight`를 root가 실행했다. 저장소별 canonical issuance와 독립 terminal을 유지하며 아직 전체 명령은 완료 전이다.
- `VERIFIED`: 먼저 발행된 `bat/bat/result.json`은20 tasks/358 file judgments, source107, stale-source negative control1, qualified/human provenance false다. canonical suite/pack/review/license/split/model/environment 및 actual Contract Python/Rust/SDK3 receipts를 소비했다. 기존 local AI benchmark ingestion/internal metrics license scope를 human/legal 승인으로 승격하지 않는다.
- isolated issuer의 host profiles는 저장소별 profile_id 때문에 bytes가 다르다. paired spec은 해당 issuer의 host-profile path/hash를 사용하고 canonical admission 및 current host fingerprint를 다시 검사한다. 공통 profile로 덮어써 hash mismatch를 무시하지 않는다.
- 다른7 issuer 결과가 실제로 발행되기 전에는 ready/PASS로 합성하지 않는다. 준비된 저장소별로 W4에 진입하고 전체8 또는 C3 240 completion은 별도다.
- `VERIFIED`: 후속 `cli/cli/result.json`은20 tasks/519 judgments,source107,stale-source negative control1이며 qualified/human provenance false다. 다른6개가 완료되기 전에는 전체 issuer exit0을 주장하지 않는다.
- `VERIFIED`: 후속 `lo/lo/result.json`은20 tasks/495 judgments,source107,stale-source negative control1이다. 아직 미발행인 repository는 ready/PASS로 합성하지 않는다.
- `VERIFIED`: first-eight에서 제외된 Nushell의 `/tmp/quanta-e1-current-admission-isolated-v3-20261004.py --input /tmp/qi-e1-nine-actual-proof-1071692b-20261004-v2.json --output-parent /Users/songmin/Documents/code-new/qi-e1-nushell-admission-20261004-v1 --selection nushell` actual issuance가 exit0이었다. `nushell/nushell/result.json`은20 tasks/504 judgments, source107, stale-source negative control1,590.334s이며 qualified/human provenance false다. current forms/validation 결속과 과거 raw execution provenance를 구별하며 전체9개 완료로 승격하지 않는다.

## 착수 입력과 후속 발행

- E1-02 merged judgments, frozen suite/pack, repository license decision
- corpus release/checkouts, development/holdout family assignments, source/runtime lock 및 matching Contract/SDK proof

## 어떤 파일을 어떻게 수정할지

`OWNED`는 에픽 담당 통합, `SHARED`는 I0 반영, `READ`는 기존 구현 소비다. 재현된 결함이나 채택된 계약 변경이 있을 때만 product source를 수정한다. 구현 파일과 독립 검증 파일을 함께 지정한다.

| 파일 | 함수 / 경계 | 구체적인 변경 또는 검증 | 모드 |
| --- | --- | --- | --- |
| [tools/benchmark/retrieval/evaluator.py](../../../../tools/benchmark/retrieval/evaluator.py) | validate_suite / _check_split_leakage | 기존 전체 suite 검증으로 objective·reviewed·no-answer·family 계약을 검사한다. 검증을 약화하거나 subset을 whole suite로 승격하지 않는다. | OWNED |
| [tools/benchmark/corpus_binding.py](../../../../tools/benchmark/corpus_binding.py) | validate_split_manifest / capture_gold_batch / validate_gold | 현 source lock과 complete release를 bind한다. 실제 빠진 결속만 고친다. | OWNED |
| [tools/benchmark/retrieval/run.py](../../../../tools/benchmark/retrieval/run.py) | validate_admission_manifest / verify_admission_bundle / freeze_admission | E1이 제안하는 최소 consumer 변경을 I0 통합 담당이 반영한다. 별도 admission parser를 만들지 않는다. | SHARED |
| [tools/benchmark/retrieval/admission.schema.json](../../../../tools/benchmark/retrieval/admission.schema.json) | required fields | 실제 producer/consumer 변경이 필요할 때만 I0가 같은 schema를 진화시킨다. | SHARED |
| [tools/ci/tests/test_corpus_binding.py](../../../../tools/ci/tests/test_corpus_binding.py) | split/runtime/source rejection | wrong split, root, corpus family, grammar/runtime source 변조 검증을 추가한다. | OWNED |

## 실행 단계

1. 최종 목표 lane과 source population을 freeze하고 NL-only/mixed contract를 구분한다.
2. 이미 적격한 objective/no-answer strata를 확인하고 근거 없는 NL negative로 표본을 채우지 않는다.
3. B08이 계속 요구하는 C5 stale 4개 suite를 manifest/query/source별로 재확인하고 새 root에 reissue하거나 명시적 exclusion을 발행한다. B09 global12와 population을 섞지 않는다.
4. family/repository exposure·license/model/runtime 입력을 검증한 split과 gold capsules를 새 namespace에 발행한다.
5. 원본 suite→blind pack→manifest→license/review/split receipt→matching proof를 기존 admission verifier에 연결한다.
6. ready/failed/pending admission inventory를 발행하고 repository별 입력 bytes와 terminal을 검증해 E2로 전달한다.

## PREPARE와 ISSUE 경계

- 선행 표는 최종 **ISSUE**를 위한 조건이다. 입력 builder/validator/schema/fixture의 source PREPARE는 I0-01 소유권 확인 후 먼저 진행하고 변경·selector를 I0-02에 제출한다.
- 순서는 source PREPARE→해당 epoch I0-02 VALIDATE→same-source receipts를 소비한 admission ISSUE다. I0-02 통과 뒤 새 code를 덧붙이고 그 옛 proof로 admission을 발행하지 않는다.
- repository별 ready bundle은 frozen suite/pack/release/split/license/review/proof의 실제 paths·bytes/digests와 terminal을 E2에 전달한다. mutable latest pointer나 ticket 완료 문구는 admission authority가 아니다.

## 검증 계획 — NOT_RUN

아래는 실행할 명령/시나리오다. 본 문서에서 통과를 주장하지 않는다. `<...>`와 외부 root는 실행 전에 실제 값으로 확정한다. test filter는 실제 수집 ID를 확인하고 0 tests를 성공으로 표시하지 않는다.

- `uv run --frozen --extra dev python -m pytest tools/ci/tests/test_corpus_binding.py tools/ci/tests/test_holdout_review.py tools/ci/tests/test_retrieval_benchmark.py -q`
- Positive: 실제 source-bound admitted repository 한 개를 preflight하고 입력과 receipts가 동일함을 확인한다.
- Negative: threshold/query/grade/family/unit/source/runtime/receipt mismatch, stale exclusion, 일부 license 누락, NL-only의 mixed decision 승격을 거절한다.

## 완료 조건

- 각 ready repository마다 matching suite/pack/split/license/review/proof와 admission 결과가 존재한다.
- 최종 보충으로 라벨이 바뀌면 E1-06에서 새 admission revision을 발행하고 영향을 받은 execution binding을 재검사한다.

## 중단·거절·재개 조건

- 필요한 license/model/SDK/contract input 부재는 해당 admission BLOCKED다. fake receipt나 guessed input을 발행하지 않는다.
- 필요한 입력 부재는 `BLOCKED`, 미실행은 `NOT_RUN`, 실제 실행 실패는 `FAILED`로 기록한다. 조건 미성립 `NOT_APPLICABLE`에는 실제 판단 근거가 필요하다.
- 변경이 source/input/query/unit/result에 영향을 주면 [I0 source gate](O4-I0-02-matching-source-proof.md)와 영향받는 capture/report를 다시 판정한다.
- 일회성 raw/log/capture/receipt는 checkout 밖 새 root에 둔다. 기존 외부 terminal을 덮어쓰지 않는다.

## 인계 결과

- 실제 source/dirty ownership, 변경 파일과 계약, 실행한 명령/selector, 관측 결과 및 제외 범위.
- raw/model/runtime/binary/input identity는 해당 실행 계약이 요구하는 범위에서 기록한다.
- 완료 조건별 `VERIFIED`/`FAILED`/`BLOCKED`/`NOT_RUN`/`NOT_APPLICABLE`과 후속 티켓에 넘길 입력을 발행한다.
