# O4-E1-01 — 원본 C3 검수 실패 복구와 실제 판단 발행

| 항목 | 값 |
| --- | --- |
| 에픽 / 담당 | [E1 — 정답·검수·admission과 독립 평가](../epics/E1-labels-admission-and-gold.md) / E1 담당 |
| 우선순위 / 종류 | P0 / `EXECUTION` |
| 기준 웨이브 | [W1 — 근거·정답·producer 병렬 준비](../waves/W1-evidence-and-producers.md) |
| 실행 상태 | Django20/509·TypeORM20/572 실제3-role 및 SQLAlchemy/Zellij 두 reviewer 완료. Opus 대체 actual weekly limit로 `FAILED`; SQL334/480 유효 조정 보존, SQL/Zellij final 발행 `BLOCKED`. Tailscale unresolved rubric 입력 `BLOCKED` |
| 선행 결과 | 없음. 현재 source 확인과 fixture 준비부터 시작 가능 |

[전체 지도](../README.md) · [티켓 인덱스](INDEX.md)

## 목적

C3 240개 질의의 실제 판단을 완료하고 재사용 가능한 모델 cache와 실패 원인을 원본 bytes로 구분한다.

## 배경과 현재 상태

원본 terminal의 7저장소 140/240질의 발행은 역사적 분모다. TypeORM/Tailscale의 unresolved pair 발행 거절은 보존한다. Django/SQLAlchemy/Zellij의 invalid native stdout 재조회에서는 `is_error:true`, `terminal_reason:api_error`와 session-limit 응답이 확인됐다. 현재 form의 candidate 분모와 역사적 issued 분모를 합산하지 않는다.

## 2026-10-04 실행 갱신

- 2026-10-05 root가 `/private/tmp/qi-c3-alternate-adjudicator-prepare-v2/resume.py --source-root /Users/songmin/.codex/worktrees/oct4-qualified-source/quanta-index --output-root /private/tmp/qi-c3-alternate-adjudicator-actual-20261005-v1 --adjudicator-model claude-opus-5-5 --execute sqlalchemy zellij`를 canonical uv 환경에서 실제 실행 중이다. SQLAlchemy adjudicator000의39pairs가98.348s/API96.345s 뒤 actual AI raw validation/replay를 통과했다. 요청/실제 모델은 Opus5.5이며 human_provenance_attested=false다. SQL39/Zellij32 총71 fresh adjudicator batches 중 완료 subset만으로 repository labels를 발행하지 않는다. old Fable adjudications는 새 조정 분모에서 제외한다.
- 후속 실제 명령 `FAILED`: exit1. SQL adjudicator000–020의21batches/334pairs는 actual validation/동일 raw replay를 통과했고021에서 CLI exit1이었다. Zellij000도 CLI exit1이었다. 두 envelope 모두 `is_error:true`, `terminal_reason:api_error`, 주간 한도/reset `Oct 7 at 1am (Asia/Seoul)` 응답이고 modelUsage는 비어 있다. reset은2026-10-07 01:00KST라는 서비스 관측이며 정시 복구 성공을 보장하지 않는다.
- SQL021 raw SHA `9475e225b505d70cffeba28bd85ac695689605152028743aa85c4f0a194f995b`, invalid SHA `0821f9d27107eeb38b01372cde5c4e61ade3519c2c87c5046f25ca23e0a2dbbc`; Zellij000 raw SHA `825496eb6bfde397b76d8ce6b1668767b3f2ed0ba858957bbd5ab2bc87b88226`다. 외부 root의 repository failure를 유지하며 유효334pair를 버리거나 partial final receipt로 승격하지 않는다. SQL146/Zellij476pair의 새 조정과 두 suite issuer는 현재 BLOCKED/NOT_RUN이다. 서비스 한도가 풀리기 전 같은 모델의 맹목적 재시도나 모델 교체로 주간 한도를 우회하지 않는다.
- `VERIFIED`: checkout 밖 `/tmp/qi-c3-current-resume-20261004-os8dvo11/resume.py`에서 현 `holdout_review.prepare`로 original form/custody를 재구성하고 frozen bytes equality를 확인했다. 기존 raw request/result/model/schema/decision identity를 검증한 cache만 재사용했다.
- 명령: `uv run --frozen --extra dev python /tmp/qi-c3-current-resume-20261004-os8dvo11/resume.py django sqlalchemy zellij typeorm tailscale` — exit 0. 이는 미판단 pair를 분리한 preflight이며 신규 모델 판단을 실행한 결과가 아니다.
- 현재 frozen form: 5저장소 100 tasks. 저장소별 candidate pairs는 Django509 / SQLAlchemy480 / Zellij476 / TypeORM572 / Tailscale544. reviewer·adjudicator별 누락은 해당 외부 root의 `preflight.json`에 별도로 기록했다.
- `FAILED`: `uv run --frozen --extra dev python /tmp/qi-c3-current-resume-20261004-os8dvo11/resume.py --execute typeorm` — 실제 adjudicator batch013, exit 1. raw `calls/typeorm/adjudicator/013/stdout.json`은 `is_error:true`, `terminal_reason:api_error`, `You've hit your session limit`을 반환했다. 서비스가 알린 재개 시각은 2026-10-04 18:40 KST다. 모델 실행은 그때까지 `BLOCKED`이며 미판단 pair를 resolved/grade로 바꾸지 않았다.
- 현 native CLI 2.1.288의 실제 version을 확인했다. 사라진 과거 2.1.286 경로를 사용하지 않는다. 재호출은 source binding을 새 외부 root에 재생하고 실패 root를 보존한다.
- 18:40 이후 `/private/tmp/qi-c3-review-after-reset-20261004-iskn2d46/resume.py --execute typeorm`에서 실제 batch013 응답은 `is_error:false`, `terminal_reason:completed`였다. 54 decisions 중 `typeorm.nl.19` / `packages/typeorm/test/github-issues/6265/issue-6265.test.ts` 1쌍이 unresolved라 canonical validator가 전체 batch 발행을 거절했다. raw와 invalid 결과를 보존했으며 53쌍 부분 receipt나 수동 grade를 만들지 않았다. 현재 실패 원인은 quota가 아니라 미해결 판단이다.
- 같은 fresh driver의 `--execute django`로 독립 ready repository의 실제 adjudicator 판단을 진행 중이다. 최종 240-task issuance/admission은 아직 완료되지 않았다.
- 최종 240-task disposition, 미판단 판단, actual finalization/suite issuance는 `NOT_RUN`이다.

## 착수 입력

- 외부 BASE: /Users/songmin/Documents/code-new/qi-b08-closeout-20261004-2i72kj91
- 원본 c3-review-resume-quota-qcshswey/<repo>/actual-review.log, actual-review-launch.json, actual-review-terminal.json 및 유효 raw cache
- frozen corpus checkout, original suite/query pack, review rubric/model identity. 서비스 사용 입력이 없으면 해당 모델 실행만 BLOCKED
- /private/tmp/qi-bench-defect-fix-20261004-46iq_iok는 이번 조사에서 MISSING. 새 driver를 현 source에 결속하고 preflight부터 재생한다.

## 현재 실제 실행 결과

- `VERIFIED`: `uv run --frozen --extra dev python /private/tmp/qi-c3-review-after-reset-20261004-iskn2d46/resume.py --execute django` — exit 0. reviewer-1/reviewer-2/adjudicator 각 20 tasks/509 pairs, missing batches 0; raw replay와 현 canonical finalization PASS. `qualified:false`, `human_provenance_attested:false`다. 240-task 최종 suite/admission은 아직 발행하지 않았다.
- `/private/tmp/qi-c3-review-next-20261004-1zzpet86/resume.py --execute typeorm sqlalchemy zellij tailscale`는 exit 1로 종료됐다. TypeORM은 reviewer-1/reviewer-2/adjudicator 각20 tasks/572 pairs, missing0으로 canonical finalization을 통과했다. 이전 unresolved stdout은 보존했고 미완료 pair에 grade를 합성하지 않았다.
- SQLAlchemy는 reviewer 두 역할 각각20 tasks 및 adjudicator 유효 batch000–016을 보존했다. adjudicator017, Zellij reviewer-1/000, Tailscale reviewer-1/011은 모두 native raw의 `api_error_status:429`, `terminal_reason:api_error`, `duration_api_ms:0`으로 실패했다. 공급자가 알린 reset은 **2026-10-04 23:40 KST**다. 서비스 입력 회복 뒤 동일 immutable driver/input으로 재개하며 실패 raw를 덮어쓰지 않는다. 세 저장소의 finalization/issuance는 `NOT_RUN`이다.
- Django/TypeORM은 `/Users/songmin/Documents/code-new/qi-e1-django-typeorm-issued-20261004-v1`에서 canonical suite·blind pack·review receipts를 실제 재발행/검증했다. 20/509 및20/572 분모를 유지했고 `qualified:false`, `human_provenance_attested:false`다. current-source admission 및 product capture는 별도다.
- reset 이후 동일 immutable driver의 `--execute sqlalchemy zellij tailscale`를 다시 실행 중이다. SQLAlchemy의 새 `adjudicator/017.retry-1/stdout.json`은 duration_api0·429와 `You've reached your Fable limit`을 반환했다. 과거 session reset과 다른 모델 개별 한도이며 조정 결과를 발행하지 않았다. Zellij reviewer-1의 추가 batch000–031이 실제 완료됐고 reviewer-2가 진행 중이다. 세 저장소 finalization은 아직 `NOT_RUN`이다.
- 대체 adjudicator는 별도 `/private/tmp/qi-c3-alternate-adjudicator-prepare-v2`의 명시적 `claude-opus-5-5` 모델/`ai:claude-opus-5-5:adjudicator` ID와 fresh output root를 사용하도록 준비했다. 기존 Opus pass-A/Sonnet pass-B reviewer ID와 구별하며 기존 Fable adjudicator raw는 보존하되 재사용하지 않는다. 제품 계약은 역할 ID3개를 구별하고 모델 제품3종을 요구하지 않는다. 선택 답변이 없으면 이 기본값을 적용한다고 사용자에게 알렸다. 새 모델 실제 호출과 후속 suite issuer는 `NOT_RUN`이다.
- 후속 원본 driver 실행은 exit1로 끝났다. Zellij 두 reviewer의 completed-tasks를 실제 읽어 각각20 tasks/476 pairs를 확인했다. Zellij adjudicator000도 duration_api0·Fable limit429다. Tailscale reviewer-1/011.retry-1은 exit0/API completed/60,858ms였지만23 decisions 중 `tailscale.nl.20` / `net/tstun/wrap_test.go` 1쌍이 unresolved라 batch 전체 발행을 거절했다. 22쌍 partial receipt나 grade를 합성하지 않았다.
- Tailscale의 이번 실패 이유는 과거429 failure.json과 달랐다. immutable save가 기존 failure overwrite를 거절하면서 driver가 종료했다. 새 raw/invalid.json과 이전 failure를 모두 유지하며 후속 실행의 terminal/finalization은 새 output root로 분리한다. rubric/source를 조사해 근거를 보완하거나 explicit exclusion을 판정하기 전 unresolved를 resolved로 바꾸지 않는다.
- reviewer-only v1 preflight도 모델 호출 전에 exit1이었다(`/private/tmp/qi-c3-tailscale-reviewers-20261005-v1/failure.json`). old input/model-input/schema와 prepared objects는 동일했지만 checker가 원본 `plan.canonical`의 끝 LF를 포함하지 않는 `ev.canonical`로 bytes equality를 요구했다. raw와 실패를 보존하고 원본 writer serializer로 exact 비교하는 v2를 별도 준비한다. JSON-only equality로 계약을 완화하지 않는다.
- `VERIFIED`: `uv run --frozen --extra dev python /private/tmp/qi-c3-tailscale-reviewers-prepare-v2/resume.py --source-root /Users/songmin/.codex/worktrees/oct4-qualified-source/quanta-index --output-root /private/tmp/qi-c3-tailscale-reviewers-20261005-v2 tailscale` preflight — exit0. 원본 `plan.canonical`의 exact input/model-input/schema 비교를 유지했고 두 reviewer 각각342 valid pairs를 재생했다. 남은 각각202pairs/10batches는 `NOT_RUN`;20개 complete tasks도 아직0이다. adjudicator는 호출하지 않았으며 label 발행 결과가 아니다.
- 후속 실제 `--execute tailscale`은 exit1/`FAILED`였다. `reviewer-1/011.retry-2`가 다시 `unresolved pair cannot be issued`로 거절됐다. 새 terminal은 `/private/tmp/qi-c3-tailscale-reviewers-20261005-v2/tailscale/failure.json`에 보존했다. 동일 입력의 맹목적 재시도는 중단하고 frozen task/rubric/source의 근거 보완 또는 명시적 제외가 필요하다. batch의 resolved subset을 발행하지 않았으며 adjudicator도 호출하지 않았다. SQLAlchemy/Zellij의 완료 reviewer와 후속 조정은 이 실패와 별도 scope다.
- 정적 raw/source 재검토에서 retry-2는 exit0/API completed이며 같은23쌍 중 같은 `tailscale.nl.20`/`net/tstun/wrap_test.go`만 unresolved였다. candidate source1–1377행·SHA가 일치하고 UDP state 테스트331/475/519–529행도 포함돼 source 누락은 아니다. 원 rubric의 'UDP state tests' 3등급과 'other tests that construct filters' 1등급은 필터 패키지 밖의 직접 동작 테스트에서 겹친다. 해당 정책 결정을 사용자에게 요청했으며 답변 전 Tailscale 발행은 `BLOCKED`다. 이전 actual grade를 수정하거나 부분 receipt로 승격하지 않는다.

## 어떤 파일을 어떻게 수정할지

`OWNED`는 에픽 담당 통합, `SHARED`는 I0 반영, `READ`는 기존 구현 소비다. 재현된 결함이나 채택된 계약 변경이 있을 때만 product source를 수정한다. 구현 파일과 독립 검증 파일을 함께 지정한다.

| 파일 | 함수 / 경계 | 구체적인 변경 또는 검증 | 모드 |
| --- | --- | --- | --- |
| [tools/benchmark/retrieval/holdout_review.py](../../../../tools/benchmark/retrieval/holdout_review.py) | validate_completed_forms / finalize_file_review_labels | 기존 source/query/form 검증을 외부 driver에서 호출한다. 재현된 producer 누락만 수정; grade 기본값·새 labeler 추가 금지. | OWNED |
| [tools/ci/tests/test_holdout_review.py](../../../../tools/ci/tests/test_holdout_review.py) | completed review / finalization controls | 실패 재개에서 valid cache 보존·잘못된 identity/unresolved 주입 거절을 기존 fixture로 보강한다. | OWNED |
| [docs/plans/sep-30-code-search-benchmark-trust/tickets/S30-B08-fresh-multirepo-holdout.md](../../../../docs/plans/sep-30-code-search-benchmark-trust/tickets/S30-B08-fresh-multirepo-holdout.md) | 현재 terminal inventory | 5개 실패의 실제 원인·재시도·발행 범위를 갱신한다. 역사적 성공 receipt는 유지한다. | OWNED |

## 실행 단계

1. 원본 log와 launch의 실제 source/argv/cache binding을 대조해 repository별 failure manifest를 새 외부 root에 만든다.
2. 실행 중인 같은 driver/model job을 process와 terminal로 확인한 뒤 유효 cached batches만 검증해 재사용한다.
3. 불일치 source evidence를 independent reviewer 2회와 별도 adjudicator에게 다시 제시한다. unresolved 판단은 근거 보강 또는 명시적 제외로 처리한다.
4. 현 소스 함수로 actual payload/schema/model input을 생성하는 preflight를 실행한다. 사라진 tmp의 성공을 재사용하지 않는다.
5. 실제 호출→raw cached replay→canonical label finalization→repository별 suite 발행을 끝내고 신규·재사용 호출 분모를 기록한다.

## 검증 계획 — NOT_RUN

아래는 실행할 명령/시나리오다. 본 문서에서 통과를 주장하지 않는다. `<...>`와 외부 root는 실행 전에 실제 값으로 확정한다. test filter는 실제 수집 ID를 확인하고 0 tests를 성공으로 표시하지 않는다.

- `uv run --frozen --extra dev python -m pytest tools/ci/tests/test_holdout_review.py -q`
- Positive: 실제 reviewer1/reviewer2/adjudicator의 별도 role/run ID와 model revision/settings·query/source/rubric·raw request/result를 재생한다. 3개 서로 다른 model 제품을 요구하는 계약은 아니다.
- Negative: unresolved→grade 치환, reviewer 중복, cache source/query 변조, 일부 batch 누락, 프로세스만 살아 있는 상태를 완료로 취급하면 거절한다.

## 완료 조건

- 240개 task 각각에 issued/excluded/failed/blocked 상태와 이유가 있고 실제 미판단 pair는 scored population에 남지 않는다.
- 모든 issued judgment는 raw/model/source/query identity에서 재생 가능하다. AI provenance와 human_provenance_attested:false를 보존한다.

## 중단·거절·재개 조건

- 실제 service quota/auth/모델 identity 입력 부재는 실행 BLOCKED로 기록한다. 문서 작성이나 preflight를 검수 완료로 표시하지 않는다.
- 필요한 입력 부재는 `BLOCKED`, 미실행은 `NOT_RUN`, 실제 실행 실패는 `FAILED`로 기록한다. 조건 미성립 `NOT_APPLICABLE`에는 실제 판단 근거가 필요하다.
- 변경이 source/input/query/unit/result에 영향을 주면 [I0 source gate](O4-I0-02-matching-source-proof.md)와 영향받는 capture/report를 다시 판정한다.
- 일회성 raw/log/capture/receipt는 checkout 밖 새 root에 둔다. 기존 외부 terminal을 덮어쓰지 않는다.

## 인계 결과

- 실제 source/dirty ownership, 변경 파일과 계약, 실행한 명령/selector, 관측 결과 및 제외 범위.
- raw/model/runtime/binary/input identity는 해당 실행 계약이 요구하는 범위에서 기록한다.
- 완료 조건별 `VERIFIED`/`FAILED`/`BLOCKED`/`NOT_RUN`/`NOT_APPLICABLE`과 후속 티켓에 넘길 입력을 발행한다.
