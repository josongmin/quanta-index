# O4-E1-03 — 최종 suite·split·admission 연결

| 항목 | 값 |
| --- | --- |
| 에픽 / 담당 | [E1 — 정답·검수·admission과 독립 평가](../epics/E1-labels-admission-and-gold.md) / E1 담당 |
| 우선순위 / 종류 | P0 / `INTEGRATION` |
| 기준 웨이브 | [W3 — 소스 통합·검증·admission ISSUE](../waves/W3-source-validation-and-admission.md) |
| 실행 상태 | source107 원본9개 admission은 과거 scope. source0e6 bat409 v4·strict consumer 및 원본8 중 원본8 전체 terminal/aggregate 및16-input 독립 hash readback `VERIFIED`. v3 consumer 거절은 보존. SQLAlchemy/Zellij/Tailscale·전체 cohort qualification 미완료 |
| 선행 결과 | [O4-E1-02](O4-E1-02-supplemental-labels.md), [O4-I0-02](O4-I0-02-matching-source-proof.md) |

[전체 지도](../README.md) · [티켓 인덱스](INDEX.md)

## 목적

검수된 라벨, objective/no-answer strata, split과 실행 receipt를 하나의 기존 admission 경로에서 일치시킨다.

## 2026-10-05 나머지8 current0e6 발행

- clean source `0e6c7e7e9494b63fdb33f4594df059817459d3b1`에서 `uv run --frozen --extra dev python /tmp/qi-e1-other-eight-0e6-issuer-v1.py --output-parent /Users/songmin/Documents/code-new/qi-e1-other-eight-admission-0e6-20261005-v1`을 실제 실행해 exit0으로 끝났다. cli/lo/mocha/uvicorn/zustand/django/typeorm/nushell을 저장소별로 canonical issuance하며 bat409 완료 결과를 반복하지 않는다.
- packet SHA `2d50ec6b6fe53598ebbc834c4d5cb4b366377ab497c51a57c6d3e45d8657ad46`, wrapper SHA `349e1afd37beac5bc8d0fb86711a3863452c6ba480bf12e62efb205ee26643c3`, canonical issuer SHA `1a14f09d58064cfdf238ac5a17ee65f66d7130273e07b854103347753145cf39`다. 실제 Contract/SDK proof·immutable 원본 labels·source107과 동등한5helpers/template를 전후 검증하며 과거 admission을 current로 바꾸지 않는다.
- completed result/terminal 전에는 readiness/8개 통과를 발행하지 않는다. 새 모델 호출·human review·전체 cohort qualification 범위가 아니다. SQLAlchemy/Zellij/Tailscale 미결은 이8개에 합산하지 않는다.
- 완료된8개의 실제 result/terminal은 source0e6·20tasks·canonical16 input paths·stale-source negative1·qualified/humanfalse다. root 순수 readback에서 완료된8개 각각의16 inputs를 재해시해 일치를 확인했다. aggregate의8개 terminal과 각 원본 result를 대조했다. 이번8개160tasks/3,853judgments이며 별도 bat409와 함께 current9repo/180tasks/4,262judgments의 admission 발행 범위다. qualified/humanfalse이며 새 capture·final quality·전체12repo의 완료가 아니다.

| 저장소 | terminal 검증 | file judgments | issuance seconds |
| --- | --- | --- | --- |
| cli | `VERIFIED` | 519 | 572.429 |
| lo | `VERIFIED` | 495 | 617.628 |
| mocha | `VERIFIED` | 511 | 531.716 |
| uvicorn | `VERIFIED` | 383 | 779.099 |
| zustand | `VERIFIED` | 360 | 525.916 |
| django | `VERIFIED` | 509 | 560.006 |
| typeorm | `VERIFIED` | 572 | 653.723 |
| nushell | `VERIFIED` | 504 | 742.309 |

## 배경과 현재 상태

현 run.py는 validate_admission_manifest와 verify_admission_bundle, corpus_binding은 validate_split_manifest를 이미 갖는다. NL-only diagnostic과 mixed-track decision을 구분한다. 과거 C5 4개 stale exclusion은 B08 cohort가 여전히 요구하는 경우에만 새 입력을 발행하며 B09 global12와 합치지 않는다.

## 2026-10-05 canonical verdict readback 비용 관측

- source0e6의 완료 warmup0 manifest를 읽기 전용 `uv run --frozen --extra dev python -m cProfile -o /private/tmp/qi-bat-w0-verdict-profile-20261005-v1.pstats -m tools.benchmark.retrieval.run verdict --repo /Users/songmin/Documents/code-new/qi-s30-b08-holdout-20261002/checkouts/bat --suite /Users/songmin/Documents/code-new/qi-bat-supplemental-admission-0e6-20261005-v2/bat/bat/suite.json --run-manifest /private/tmp/qbw0/bat/run-manifest.json --out /private/tmp/qi-bat-w0-verdict-profile-20261005-v1.json`으로 재생했다. exit0·원본 verdict JSON equality/동일 SHA `20a65be9b453f0e97cec710aff6ed0589de7cfd99ccc2761062ffd0e2bee5b7a`다.
- profiler total320.566s, admission 검증318.159s/split317.483s, 두 release 검증247.003s였다.22 `freeze_repo`,242 owned Git executions,164,012 file digests와19,824 fingerprints가 관측됐다. mean_ci14calls/0.619s, uncached bootstrap6calls/0.618s다. cumulative parent/child를 합산하지 않는다.
- 이는 instrumented readback의 비용·call-count 진단이다. 제품 query/index latency·fsync 원인 분리·quiet-host speed·1196-row numeric workload로 일반화하지 않는다. 현재 bat readback의 지배 비용은 complete split의 Git/source replay이며 bootstrap kernel 개선으로 전체 검증 시간을 해결한다는 근거는 없다.
- 최적화를 선택한다면 기존 source/split authority에서 execution-scoped validated context와 complete source/control recheck를 유지해야 한다. stale cache·mtime-only 판정·검증 생략·실패 producer를 consumer가 보정하는 방법은 사용하지 않는다. 이 관측으로 source/generator/validator를 변경하지 않았으며 prepared0e6 admissions는 그대로 엄격한 replay를 수행한다.

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
- `VERIFIED`: 같은 first-eight 실행의 후속 mocha20tasks/511judgments(572.331s), uvicorn20/383(422.905s), zustand20/360(529.734s) result를 실제 읽었다. 각각 source107, stale-source negative control1, qualified/human false다. Django/TypeORM terminal과 aggregate는 아직 발행 전이며 전체 명령 exit0이나 전체9개 완료로 합성하지 않는다.
- `VERIFIED`: 이후 Django20tasks/509judgments(485.482s), TypeORM20/572(432.661s)가 발행됐고 first-eight 명령이 실제 exit0으로 종료됐다. `summary.json`의8개 repository가 각각 VERIFIED이며 별도 Nushell까지9개/180tasks/4211judgments의 원본 admission 범위다. 모두 source107·qualified/human false다. 새 bat supplemental labels와 후속 ANN 수리는 이 원본 source107 admission에 포함되지 않는다.

## 착수 입력과 후속 발행

- E1-02 merged judgments, frozen suite/pack, repository license decision
- corpus release/checkouts, development/holdout family assignments, source/runtime lock 및 matching Contract/SDK proof
- 2026-10-05 static PREPARE: `/tmp/qi-bat-supplemental-admission-input-cef-v2.json`과 `/tmp/qi-bat-supplemental-admission-issuer-cef-v2.py`는 bat409 labels의 source107 producer와 sourcecef 제품 proof/admission을 독립 결속한다. canonical helper5파일의 SHA가 두 checkout에서 같고 라벨은 같은 corpus/query/rubric에 결속돼 있다. 기존 source107 제품 proof/capture를 sourcecef proof로 재사용하지 않는다.
- v2는 실제 sourcecef Contract/SDK context2개 및 Python/Rust/SDK receipts3개의 정확한 경로를 CLI 필수 입력으로 받으며 revision/context/co-location을 검증한 뒤만 발행한다. 현재 새 proof 발행과 이 admission 실행은 `NOT_RUN`이다. SQLAlchemy/Zellij/Tailscale의 다음 admission은 실제 alternate result와 canonical suite SHA가 존재한 뒤 입력을 고정한다.
- 최종 product source0e6용 별도 v3 PREPARE를 고정했다: `/tmp/qi-bat-supplemental-admission-input-0e6-v3.json` SHA `45a7e62e9217651dc2e1fd9f572562e0b0bdc5d882f16fc06bafaa6b1cba9c54`, `/tmp/qi-bat-supplemental-admission-issuer-0e6-v3.py` SHA `d187ba9627b55c1e74a57fa12abb29c8e844b169b68e343d6d366251c3a90f7c`다. label producer107/corpus/query/rubric 원본 결속은 유지하고 product revision과 proof requirement 문구를 최종 epoch로 바꿨다. 실제 새 proof5경로가 없으므로 admission ISSUE는 아직 `NOT_RUN`이다. 기존 v2와 source107 admissions는 보존한다.
- 후속 source0e6의 actual Contract/fresh SDK 및 두 portable verifier가 모두 exit0이었다. root가 v3 issuer의 `--contract-context/--sdk-context/--contract-python-receipt/--contract-rust-receipt/--sdk-receipt`에 실제 `qi-oct5-contract-proof-0e6-v1`/`qi-oct5-sdk-proof-fresh-0e6-v1`의2contexts/3receipts를 넣어 `/Users/songmin/Documents/code-new/qi-bat-supplemental-admission-0e6-20261005-v1` fresh parent에서 실행 중이다. result/summary 및 canonical freeze/replay 완료 전에는 bat409의 새 admission 통과를 주장하지 않는다.

## 2026-10-05 strict consumer 거절과 producer 수리

- source0e6 v3 issuance는 exit0, 20tasks/409judgments, stale-source negative control1,421.731s였다. `qi-bat-supplemental-admission-0e6-20261005-v1` 원본은 보존한다. 이어 pair v7 `--selection bat --query-warmup-passes 1` preflight를 `/private/tmp/qpbw1p`에서 실행하자 exit2, `quality matrix admission result identity differs`로 거절됐다. 제품 pair는 실행되지 않았다.
- 원인은 external issuer의 canonical `result.json`에 label-producer 필드2개와 확장121-input closure를 추가한 것이다. 현 `run._quality_matrix_verify_member_admission`은 exact result keys와 canonical16-input closure를 요구한다. canonical consumer의 거절을 그대로 유지했다.
- `/tmp/qi-bat-supplemental-admission-issuer-0e6-v4.py` SHA `dea7b2b6d7102e5c34a333dd236d7d6bed6a5483758d9a7fff64ca8021274f30`는 canonical result keys/16paths를 발행하고, label producer107·validation SHA·확장121paths는 별도 `admission-lineage.json`에 보존한다. 원본 v3/input/product source/proof bytes를 변경하지 않았다.
- root가 같은 input v3와 actual Contract/fresh SDK2contexts/3receipts로 `/Users/songmin/Documents/code-new/qi-bat-supplemental-admission-0e6-20261005-v2`에서 v4를 재실행 중이다. 새 result와 canonical consumer replay 완료까지 bat409의 W4 admission은 미완료다.
- 후속 v4 actual issuance `VERIFIED`: exit0,375.874s,20tasks/409judgments, stale-source control1이었다. strict result12keys/canonical16paths와 sidecar121paths를 독립 SHA replay했다. `admission-lineage.json` SHA `271e904529316505c2ae5676d6131c0bcbb9ce8c82f81ac07a35c11e7cd08597`; source0e6 제품과 source107 label producer를 별도 결속한다.
- 이어 pair v7 preflight의 `/private/tmp/qpbw1p2/prepare.json`에서 bat `PREPARED`를 확인했다. 기존 `run._quality_matrix_verify_member_admission`의 전체 replay를 통과했으며, aggregate exit2는 미선택 required7개 `NOT_RUN` 때문이다. `/private/tmp/qpbw1p2/bat.json` SHA `ef763fbd247e6af49180edb62fee78b14e90d33cd78603c9da9cc3179865937f`의 actual warmup1 pair를 시작했다. capture/verdict 완료 전에는 제품 비교 성공으로 승격하지 않는다.
- 나머지 원본8개(cli/lo/mocha/uvicorn/zustand/django/typeorm/nushell)의 current0e6 발행을 준비했다. `/tmp/qi-e1-other-eight-0e6-actual-proof-input-v1.json` SHA `2d50ec6b6fe53598ebbc834c4d5cb4b366377ab497c51a57c6d3e45d8657ad46`는 original review inputs를 byte identity로 유지하고 actual source0e6 proof paths를 소비한다. admission template107/current0e6의 canonical helper5개 SHA equality도 결속했다.
- `/tmp/qi-e1-other-eight-0e6-issuer-v1.py` SHA `349e1afd37beac5bc8d0fb86711a3863452c6ba480bf12e62efb205ee26643c3`는 기존 canonical issuer `issue`를 그대로 호출하고 각 result16paths와 source guard를 검사한다. 선택8개 각각 독립 terminal을 발행하며 actual 실행은 아직 NOT_RUN이다. bat409 v4나 old source107 제품 result를 재작성하지 않는다.

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
