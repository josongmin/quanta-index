# O4-E1-02 — 미검수 합집합 검수와 원본 라벨 병합

| 항목 | 값 |
| --- | --- |
| 에픽 / 담당 | [E1 — 정답·검수·admission과 독립 평가](../epics/E1-labels-admission-and-gold.md) / E1 담당 |
| 우선순위 / 종류 | P0 / `EXECUTION` |
| 기준 웨이브 | [W1 — 근거·정답·producer 병렬 준비](../waves/W1-evidence-and-producers.md) |
| 실행 상태 | 새 bat5제품 union·18tasks/51pairs·실제3 AI 역할 판단/raw 재생 `VERIFIED`; merged 발행 v1 `FAILED`(원본 form pack 결속), 수정 중. 과거 first-six raw replay `BLOCKED` |
| 선행 결과 | [O4-E1-01](O4-E1-01-original-review-resume.md) |

[전체 지도](../README.md) · [티켓 인덱스](INDEX.md)

## 목적

최신 다제품 pool의 미검수 task/file 쌍을 실제 판단하고 기존 유효 판단을 잃지 않는 새 label 판을 발행한다.

## 배경과 현재 상태

첫 6저장소의 신규 union은 375쌍, 그중 bat/cli/lo 214쌍의 repaired preflight가 과거에 기록됐다. 이 숫자는 전체 최종 분모가 아니다. 기존 lo 발행 41쌍과 신규 lo 30쌍은 별도다. bind_supplemental_review_tasks는 frozen suite의 threshold/query/source를 검증해 이미 구현되어 있다.

## 2026-10-04 현재 바이트 custody 재검증

- 새 외부 preflight v1은 packet의 잘못된 repository key 가정으로 `FAILED`; v2는 producer와 모든 original `input_sha256`를 재검증하는 실제 request 준비 단계에서 원본 record 부재를 확인하고 거절했다. 두 실패 root는 보존했으며 모델 호출/새 grade는 0이다.
- 기존 `c3-all-five-pool-audit-gsZWij9u/*-canonical/result.json` 6개는 102 refs를 가진다. 현재 원위치에서 78 refs의 SHA가 일치하고, 사라진 suite/query-pack 12 refs는 같은 SHA의 영구 사본이 있다.
- Quanta/Semble `record.json` 12 refs는 `/private/tmp/qdrk7q9q8/{0..5}.staging/`에 있었고 현재 부재다. BASE의 이름 기준 suite/query-pack/record 후보277개와 SHA를 대조했으나 같은 record 12개를 복구하지 못했다. 해당 paired producer terminal은 `FAILED`이고 stdout에는 임시 output path만 남아 있다.
- 따라서 당시 생성된375쌍을 **현재 재생된 5제품 합집합**으로 승격할 수 없다. 필요한 원본 바이트 부재의 replay scope는 `BLOCKED`; current-source Quanta/Semble 재캡처와 pool 재발행을 준비한다. packet의 자체 hash만으로 raw custody를 대체하지 않는다.
- Source-only packet은 current frozen suite/checkout과 task/query/path/file SHA/text를 재결속한 diagnostic 후보 준비에 쓸 수 있지만 canonical returned-file union/admission의 증거가 아니다. actual supplemental review와 merged labels는 새 native 입력 확보 후 실행한다.

## 착수 입력

- E1-01의 issued source-bound suites
- 원본 c3-all-five-pool-audit-gsZWij9u, lo issued-merged, 새 ready repository별 pool
- 새 외부 driver root와 source/argv/input commitments

## 2026-10-04 새 bat capture와 검수 순서

- source107 bat actual pair의40 native rows가 `/private/tmp/qi-p0-v1/bat.staging`에 남았다. 기존 판단과의 차집합은51 `(task_id,path)`이며 과거 bat blind packet의 pair 집합과 같아도 record bytes는 다르므로 과거 producer receipt는 재사용하지 않는다.
- `VERIFIED`: root가 source107의 `uv run --frozen --extra dev python /tmp/qi-bat-current-pool-driver-20261004-v1.py --output /private/tmp/qi-bat-current-pool-107-v1`를 actual 실행해 exit0이었다. 기존 `run.project_pack_and_suite`→route별 `holdout_review.capture_review_pool`→judged 차집합→`bind_supplemental_review_tasks`로18tasks/51pairs/1batch가 결속됐다. 두 reviewer 요청을 준비했으며 실제 model call·adjudication은`NOT_RUN`이다.
- 원 pair의 failed terminal과 source/record bytes는 보존했다. 새 native3제품 수집·재생 후 기존 binder에 다섯 반환 파일 union을 연결해 추가 미판정 여부를 판정한다. 이2route pool에 source/control diversity 또는 whole-corpus relevance를 부여하지 않는다.
- 원 qualified pair의 complete-scored evaluation은 `FAILED`로 유지한다. 리뷰 후보 발행·AI 실제 판단·merged labels·새 admission·fresh final pair를 서로 다른 단계로 판정한다. 두 제품의 미판정51쌍은 새5제품 최종 union 완료를 뜻하지 않는다.

### 후속 실제 5제품 union과 role preflight

- `VERIFIED`: `/tmp/qi-bat-five-product-unjudged-bridge-20261004-v2.py --pool-input /tmp/qi-bat-failedpair-review-pool-input-20261004-v1.json --pool-root /private/tmp/qi-bat-current-pool-107-v1 --native-root /private/tmp/qn/bat --out /private/tmp/qi-bat-five-pool-107-v1` actual exit0. canonical native replay, 원 2route 재생, five owner-pool과 source/input/tree pre/post 결속 후18tasks/51pairs가 남았다. Sourcegraph/OpenGrok/cs 각20 completed rows의 반환 파일은0이므로 이 셋이 union 분모를 늘리지 않았다.
- `VERIFIED`: `/tmp/qi-bat-five-product-actual-review-20261004-v2.py --pool-input /tmp/qi-bat-failedpair-review-pool-input-20261004-v1.json --five-root /private/tmp/qi-bat-five-pool-107-v1 --out /private/tmp/qi-bat-five-actual-review-107-v2` actual preflight exit0. 원 suite binder,51 source pairs,5제품 membership,477 native retained files와 두 blind reviewer 요청의 canonical bytes를 재검증했다. 모델 호출은 `NOT_RUN`; 실제 두 판단 후에만 adjudicator를 실행한다.
- v1 preflight는 tuple/list 표현을 Python 객체로 비교해 `FAILED`였다. 기존 failed root는 보존하고 v2가 producer의 동일 canonical JSON bytes로 비교한다. pool/source/grade를 변경하지 않았다. AI-only 판단, merged suite, 새 admission 및 fresh final pair는 계속 별도 단계다.
- `VERIFIED`: 위 v2 actual role driver에 `--execute`를 붙여 서비스 reset 이후 실행했고 exit0이었다. reviewer-1/reviewer-2/adjudicator 각51쌍,18tasks가 actual native raw·receipt 및 cache replay를 통과했다. adjudicator의 grade0은48쌍, grade1은3쌍, threshold2 이상은0쌍이다. AI-only 결과이며 human provenance/qualification은 false다.
- `FAILED`: `/tmp/qi-bat-canonical-supplemental-issuer-20261004-v1.py --input-plan /tmp/qi-bat-merge-finalize-input-plan-20261004-v1.json --out /private/tmp/qi-bat-canonical-merged-107-v1 --finalize` actual exit1. 원본 completed form을 현재 pack에 직접 검증하는 단계가 `query_pack_sha256` 불일치로 거절했으며 새 suite/receipt는 발행되지 않았다. 실제 원본 single-route pack/seed/custody로 먼저 검증하고 동일 query/source/threshold의 판단만 current form으로 이전하는 경로를 준비한다. 기존 form header나 validator를 약화하지 않는다.

## 어떤 파일을 어떻게 수정할지

`OWNED`는 에픽 담당 통합, `SHARED`는 I0 반영, `READ`는 기존 구현 소비다. 재현된 결함이나 채택된 계약 변경이 있을 때만 product source를 수정한다. 구현 파일과 독립 검증 파일을 함께 지정한다.

| 파일 | 함수 / 경계 | 구체적인 변경 또는 검증 | 모드 |
| --- | --- | --- | --- |
| [tools/benchmark/retrieval/holdout_review.py](../../../../tools/benchmark/retrieval/holdout_review.py) | capture_review_pool / bind_supplemental_review_tasks / finalize_file_review_labels | actual request 직전 canonical binding을 호출하고 merged forms를 기존 finalizer로 발행한다. 함수 재구현보다 외부 producer 연결을 우선한다. | OWNED |
| [tools/ci/tests/test_holdout_review.py](../../../../tools/ci/tests/test_holdout_review.py) | supplemental_request controls | 새 source에서 실제 request builder까지 도달하는 positive와 threshold/query/grade/source mutation negative를 유지한다. | OWNED |
| [tools/benchmark/retrieval/README.md](../../../../tools/benchmark/retrieval/README.md) | supplemental API 계약 | 모델 요청·라벨 발행·재생 단계와 subset으로 전체 no-answer를 판정할 수 없는 경계를 명시한다. | SHARED |

## 실행 단계

1. 제품 identity/rank를 reviewer에서 가린 채 exact task ID/path/source hash로 union을 재생한다.
2. 이미 판단된 pair와 신규 pair를 나누고 repository별 실제 분모를 계산한다. 375 또는 214를 고정 목표로 복제하지 않는다.
3. 기존 suite threshold를 bind한 actual payload preflight와 4종 mutation rejection을 새 source에서 실행한다.
4. 실제 두 reviewer와 adjudication을 실행하고 모든 원본 valid judgment 및 실패 raw를 보존한다.
5. 신규 판단을 기존 forms에 canonical merge하고 새 suite ID/commitment를 발행한다. E2-04에서 추가 union이 나오면 E1-06에서 같은 경로로 최종 보충한다.

## 검증 계획 — NOT_RUN

아래는 실행할 명령/시나리오다. 본 문서에서 통과를 주장하지 않는다. `<...>`와 외부 root는 실행 전에 실제 값으로 확정한다. test filter는 실제 수집 ID를 확인하고 0 tests를 성공으로 표시하지 않는다.

- `uv run --frozen --extra dev python -m pytest tools/ci/tests/test_holdout_review.py -q -k 'supplemental_request or holdout_review'`
- Positive: 원본 lo 41쌍 보존, suite threshold 2 유지, 신규 pair만 actual call로 판단.
- Negative: explicit threshold mismatch, duplicate/already-judged pair, stale source text, supplied grade, invalid cached raw, subset-only no-answer 발행 거절.

## 검수 권위와 task 처분

- canonical helper는 서로 다른 reviewer_id 2개와 별도 adjudicator_id, frozen source/query/form coverage를 검사한다. 그 ID만으로 실제 호출·reviewer independence·human provenance를 증명하지 않는다.
- 외부 실제 driver의 request/run ID·model revision/settings·rubric/input digest와 raw/cache source를 별도 재생한다. 실제 두 blind role 실행과 조정 실행을 AI provenance로 표시하며 사용된 model 수를 independence로 해석하지 않는다.
- pooled files의 answerable=false는 pool 범위 판단이다. corpus-wide no-answer는 독립 source/oracle 또는 명시된 전수 판단이 있어야 한다. unknown을 grade0으로 채워 전체 no-answer를 발행하지 않는다.
- unresolved를 제외할 경우 original task ID와 사유·변경된 분모를 발행한다. failed/blocked task가 남은 full C3 240 완료 scope는 FAILED/BLOCKED/NOT_RUN으로 남고, ready repository의 partial labels만 발행할 수 있다.

## 완료 조건

- old/new/reused/unresolved/excluded pair 수가 원본 raw와 일치한다.
- 새 merged labels를 source/query/rubric/model commitments로 replay하고 historical outputs를 변경하지 않는다.

## 중단·거절·재개 조건

- pool 밖 relevance completeness와 human provenance를 자동 생성하지 않는다.
- 새 qrel을 옛 native record에 임의 rebind하지 않는다. 재사용/재실행 결정은 E2-03의 source/request 영향 분석을 따른다.
- 필요한 입력 부재는 `BLOCKED`, 미실행은 `NOT_RUN`, 실제 실행 실패는 `FAILED`로 기록한다. 조건 미성립 `NOT_APPLICABLE`에는 실제 판단 근거가 필요하다.
- 변경이 source/input/query/unit/result에 영향을 주면 [I0 source gate](O4-I0-02-matching-source-proof.md)와 영향받는 capture/report를 다시 판정한다.
- 일회성 raw/log/capture/receipt는 checkout 밖 새 root에 둔다. 기존 외부 terminal을 덮어쓰지 않는다.

## 인계 결과

- 실제 source/dirty ownership, 변경 파일과 계약, 실행한 명령/selector, 관측 결과 및 제외 범위.
- raw/model/runtime/binary/input identity는 해당 실행 계약이 요구하는 범위에서 기록한다.
- 완료 조건별 `VERIFIED`/`FAILED`/`BLOCKED`/`NOT_RUN`/`NOT_APPLICABLE`과 후속 티켓에 넘길 입력을 발행한다.
