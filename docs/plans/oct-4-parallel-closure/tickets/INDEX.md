# OCT-04 잔여 작업 인덱스

2026-10-05 완료 구현·결정은 [Accepted ADR](../../../adr/README.md#oct-05-implemented-contracts)에 압축했다.
이 문서가 원본 5개 handoff와 29개 티켓의 **미완료 조건 및 범위 판정의 단일 기준**이다.
[담당·인계](../README.md) · [실행 웨이브](../WAVES.md) · [원본 복구](../../ARCHIVE-INDEX.md#oct-05-handoff-and-ticket-compaction).

코드 대조: 구현 존재/수리23개, 조건부4개, 현 계약 비적용1개, 계약 입력이 필요한 운영 코드1개.
이는 29개 요청의 전체 qualification 완료율이 아니다. 기존 구현을 다시 만드는 작업은 남기지 않는다.

## 공통 실행 조건

- `VERIFIED`는 실제 실행한 해당 scope, `FAILED`는 실행 실패, `BLOCKED`는 필수 입력 부재,
  `NOT_RUN`은 미실행이다. 조건부 변경의 미측정 상태를 `NOT_APPLICABLE` 완료로 바꾸지 않는다.
- 현재 source/dirty/owner와 실제 입력·selector를 확인하고 좁은 결정적 owner rail부터 실행한다.
  새 raw/log/capture/receipt는 checkout 밖 fresh root에 두며 기존 실패/partial raw를 덮어쓰지 않는다.
- source/query/unit/model/profile/runtime/clock 변경은 영향 proof 및 cells를 재실행한다.
  qrel-only 재채점도 native binding이 허용해야 한다. 옛 raw의 source/digest를 새 값으로 덮어쓰지 않는다.
- unknown/missing/unresolved를 grade0·no-answer·empty success로 채우지 않는다.
  complete-empty·capped·timeout·capacity refusal 및 common-eligible0은 각각 그대로 설명한다.
- 실제 build/test/model/Docker/native/scale jobs는 I0가 host별 직렬 admission으로 관리한다.
  source 조사·fixture·정적 점검은 병렬 가능하다. 실패 sibling은 ready repository를 막지 않는다.
- 아래 실행 명령은 해당 잔여를 수행할 때의 진입점이다. 이 문서 정리는 새 제품 실행이 아니다.
  placeholder는 실제 값으로 확정하며 zero-selected/skipped를 통과로 취급하지 않는다.

## E1

담당: E1. 구현 계약: [review/admission/result identity ADR](../../../adr/OCT-05-001-review-admission-and-result-identity.md).

### O4-E1-01

P0 · W1 · 코드 구현 완료, 최종 판단 입력 `BLOCKED`.

- SQLAlchemy 잔여146pairs와 Zellij476pairs의 실제 최종 AI 판정을 완료한다.
  유효 SQL334/480 및 기존 두 reviewer raw는 보존한다. service quota/auth/model identity를 재확인한다.
- Tailscale 필터 패키지 밖 UDP 상태 테스트의 grade1/3 rubric 경계를 확정한 뒤 재검수한다.
- 재개 입력: `/Users/songmin/Documents/code-new/qi-b08-closeout-20261004-2i72kj91` 아래
  `c3-review-resume-quota-qcshswey/<repo>/`의 launch/terminal/log와 valid raw cache,
  frozen corpus/suite/query/rubric. 사라진 임시 driver는 원본 증거로 사용하지 않는다.
- 완료: C3 240tasks 각각 issued/excluded/failed/blocked와 실제 judgment provenance가 설명되며
  미판단 pair는 scored population에 없다. AI를 human으로 표시하지 않는다.

### O4-E1-02

P0 · W1→W5 · 코드 구현 완료, current9 blind input `PREPARED`, 실제 신규 검수 미완료.

- 신규151tasks/742pairs만 두 reviewer+adjudicator로 판단하고 원본 유효 라벨과 병합한다.
  bat 원본358+보충51=409는 재호출하지 않는다. old/new/reused/unresolved/excluded 수를 대조한다.
- 재개 입력: `/private/tmp/qi-current-nine-supplemental-pool-97eedd-actual-v1/ledger.json`,
  SHA-256 `78013ed33e5647bfa5e109fc5edabb422e41dc48d0cbf4f63f713785642818ec`.
  source97의 실제5제품 union이며 제품명/순위/점수는 owner custody에만 있다.
- 원본 first-six pool은 Quanta/Semble record12개 부재로 historical replay `BLOCKED`다.
  자체 hash나 같은 pair 집합으로 native provenance를 대체하지 않는다.
- 완료: source/query/rubric/threshold/model에 결속된 새 labels와 독립 raw replay.

### O4-E1-03

P0 · W3 · admission 코드 구현 완료, 나머지3repo 및 후속 final revisions 미발행.

- SQLAlchemy/Zellij/Tailscale 및 새 merged labels를 canonical suite/pack/split/license/review/proof에
  연결한다. repository별 실제 source/runtime과 I0-02 matching proof 이후 admission ISSUE.
- frozen product0e6의 bat 포함9repo/180tasks/4,262judgments admission은 그 범위로 유지한다.
  원본 source107이나 원 review-validator revision을 current product revision으로 재명명하지 않는다.
- B08이 계속 요구하는 C5 stale4 suites는 manifest/query/source를 재확인해 reissue 또는 명시적
  exclusion을 발행한다. B09 global12와 합치거나 NL-only diagnostic을 mixed-track decision으로 승격하지 않는다.
- 완료: 각 ready repository의 정확한 admission inputs/result 및 변경 labels의 새 revision.
  threshold/query/grade/family/unit/source/runtime/proof/license 불일치는 발행 거절이다.

### O4-E1-04

P1 · W1/W4 · name-span 구현 완료, 다른 지원 name/typo cells와 최신 source 영향 미판정.

- Gin exact1,196 symbol/name 실제 capture/scoring(product0e6/driverb55)은 반복하지 않는다.
  다른 지원 unit의 선언 ID/name bytes/span을 independent source oracle와 native selected unit으로 평가한다.
- same-line 두 선언, same-name receiver, use-only, Unicode/case negative를 유지한다.
- 완료: unit별 실제 supported/unsupported 분모와 source-attested recovery.
  file hit·잘못된 이름·context enlargement는 name recovery가 아니다.

### O4-E1-05

P1 · W1/W5 · source/split 도구 구현 완료, license·acceptance 입력 `BLOCKED`; gold/holdout 미발행.

- 입력: `/Users/songmin/Documents/code-new/qi-oct4-unseen-prepare-k7exyv41/`의
  `candidate-freeze`, `release-candidate`, `source-split-prepare`.
  corpus-set5,684와 release code_only6,079는 서로 다른 selection 분모이며 candidate12repo는 미승인이다.
- license approver·사전 acceptance/critical-stratum 허용 회귀를 확정하고 query/family/exposure,
  near-copy/parser coverage, 독립 relevance/gold/review와 admission을 발행한다.
- 각 family의 기존1,000+ 목표는 실제 eligible population/underfill로 판정한다.
  동일 family 복제나 exposed corpus 재명명으로 표본 목표를 채우지 않는다.
- 완료: development와 holdout의 source/query/family 분리 및 provenance,
  ambiguous/excluded/underfilled 집합. 기존 Gin/C3/B09를 renamed unseen으로 재사용하지 않는다.

### O4-E1-06

P1 · W5 · scorer/report 구현 완료, final labels·재채점·전체 cohort 판정 미완료.

- E2-04의 실제 native outcomes/union과 E1-02/03의 새 judgments/admissions로 final reports를 재계산한다.
  ready cohort replay는 다른 name/미사용 holdout의 완료를 기다리지 않는다.
- lane별 common eligible·operational coverage·repository cluster CI·pool exposure sensitivity,
  name/NL/no-answer 및 ARB original/adapted/B09 분모를 각각 유지한다.
- 완료: 독립 raw recomputation과 rows/denominators/scores/report 일치, 모든 required-cell outcome 설명.
  common-eligible0은 채점 불가능한 diagnostic이며 0점 우열이 아니다.

### O4-E1-07

P2 · W1→조건부W2 · bounded kernel/cache 구현 있음, 추가 최적화 조건 미확인.

- 1,196-row full-caller cold compute/RSS와 사전 목표/memory ceiling을 측정한다.
  bat20 whole-verdict의 추가 numeric 최적화는 관측 profile 범위에서 이미 `NOT_APPLICABLE`이다.
- 채택 시 declared10,000 resamples/method/seed/draw/strata를 independent scalar/reference와 대조하고
  NaN/Inf·duplicate task·out-of-order draw·huge cache/hidden growth를 거절한다.
- 완료: 실제 병목 근거에 따른 최적화+parity 또는 no-code disposition. RNG 변경의 옛 byte parity는 추정하지 않는다.

## E2

담당: E2. 구현 계약: [native capture/clock/index scope ADR](../../../adr/OCT-05-002-native-capture-clock-and-index-scope.md).

### O4-E2-01

P1 · W4 · completed-clock 구현·actual scope 완료. 정식 반복 검증은 [E4-06](#o4-e4-06).

새 반복 실행에서 request construction→complete normalized/validated output과 clock/output hash를 확인한다.
Historical transport/worker clocks를 소급 승격하거나 invalid/failed observations를 속도 표본에서 숨기지 않는다.

### O4-E2-02

P1 · W1/W4 · native reader/capture/replay 구현 완료, 전체 서비스 universe qualification 미완료.

- Sourcegraph source97의12repo/13,347files native replay 및 OpenGrok readonly 전후 disk/source/aux/API
  관측은 완료된 scope로 유지한다. 후속 producer/decoder 변경의 affected fresh evidence는 별도로 발행한다.
- `VERIFIED`: driver base089의 고정7파일로 instrumented bat20 capture와 새 프로세스 독립 replay를 실행했다.
  Native 전후12repo/17,615live = source13,347 + directory4,256 + settings12가 일치했다.
  bat20 요청의 acquired reader(commit4/version15/live113/max116)와 nonce를 결속했고,
  pristine 대비20/20 본문이 같았다. 별도 정상16hit·0hit control2개도 같은 native commit과 일치했다.
  `opengrok_query_reader_scope.attested=true`는 이20개 instrumented 요청만 포함한다. Global3flags는false다.
- Actual roots: `/private/tmp/qi-e2-og-query-reader-bat20-20261005-v1`(capture SHA
  `58f20a0e32b8f6de24e67ae2f8dca061bf5d08f1dad3c536f3106399d3dfb800`),
  `/private/tmp/qi-og-query-reader-fixture-20261005-v1`(compiler/control/replay).
  Fixed image javac4classes·집중78tests(105.12s)·current closure/authority244tests(47.30s)·Ruff가 통과했다.
- `VERIFIED`: 같은 고정7파일로 cli20 capture와 별도 프로세스 replay도 exit0였다.
  `/private/tmp/qi-e2-og-query-reader-ready9-cli20-20261005-v1`의 capture SHA는
  `c5ba28a1ab7dce1394d8449a2c981a44e77bd4e76f4425a5050855356836dbc9`다.
  Native 전후12repo/17,615live가 일치했고, 실제20개 cli 요청은
  segments_4/generation4/readerVersion16/live1,358/max1,359 reader에 결속됐다.
  Selected-request attested=true, all-project readers=false, diagnostic_unqualified 범위다.
- `VERIFIED`: django20 capture와 별도 프로세스 replay도 exit0였다.
  `/private/tmp/qi-e2-og-query-reader-ready9-django20-20261005-v1`의 capture SHA는
  `412325e818b51fd38474af6f8e56543aca081dbcaad76460f46a02869fa51463`다.
  Native12repo/17,615live와20개 요청의 segments_4/generation4/readerVersion16/live3,031/max3,032를
  결속했다. Bat/cli/django는 발행 suite/pack과 일치하는 ready9 중3repo/60requests 범위이며,
  selected-request attested만true이고 전체 서비스 reader/비교 qualification은 미완료다.
- `VERIFIED`: lo20 capture와 별도 프로세스 replay도 exit0였다.
  `/private/tmp/qi-e2-og-query-reader-ready9-lo20-20261005-v1`의 capture SHA는
  `6bf35fe3203ef504f68141e5b59297353426f824cd18023e7dc7a671a4a5500e`다.
  Native12repo/17,615live,20개 요청의 segments_4/generation4/readerVersion16/live158/max159가 일치했다.
  동일 발행 입력의 bat/cli/django/lo4repo/80requests를 capture·독립 replay했으며,
  나머지 ready5repo와 전체 서비스/비교 qualification은 남아 있다.
- `VERIFIED`: mocha20 capture와 별도 프로세스 replay도 exit0였다.
  `/private/tmp/qi-e2-og-query-reader-ready9-mocha20-20261005-v1`의 capture SHA는
  `1f3fe484dcb9acdbf7a2587b03b5f05a2e22cb80ca2813f2790d94df6289c4e2`다.
  Native12repo/17,615live,20개 요청의 segments_4/generation4/readerVersion16/live561/max562가 일치했다.
  완료된 ready5repo/100requests의 selected-request attested만true다. 나머지 ready4repo 및
  전체 서비스 reader/비교 qualification은 남아 있다.
- `VERIFIED`: Nushell20도 별도 fresh capture 및 새 process native replay가 각각 exit0이다.
  Root `/private/tmp/qi-e2-og-query-reader-ready9-nushell20-20261005-v1`, capture SHA256
  `9bb14a98f9ce2c63c435b96b580656dd4066811f13e901de49c3c32e0bb2cdd1`.
  Native12repo/17,615live,20개 요청의 segments_4/generation4/readerVersion16/live2,298/max2,299가 일치했다.
  이 실행 시점에 ready6repo/120requests의 selected-request attested만true였다.
- `VERIFIED`: TypeORM20 fresh capture와 별도 process native replay가 각각 exit0이다.
  Root `/private/tmp/qi-e2-og-query-reader-ready9-typeorm20-20261005-v1`, capture SHA256
  `816494c33d7d99d60b6365dbab19b28f506b6eb45c003df77d0327b94587e799`.
  Native12repo/17,615live와20개 요청의 segments_4/generation4/readerVersion16/live5,588/max5,589가 일치했다.
  해당 실행 시점의 ready7repo/140requests에서 selected-request attested만true였다.
- `VERIFIED`: Uvicorn20 fresh capture와 별도 process native replay가 각각 exit0이다.
  Root `/private/tmp/qi-e2-og-query-reader-ready9-uvicorn20-20261005-v1`, capture SHA256
  `a60e8c47445bed4e8ecba6ac7111e82bea7676999d7753988d265ef7405df2cd`.
  Native12repo/17,615live와20개 요청의 segments_4/generation4/readerVersion16/live89/max90가 일치했다.
  해당 실행 시점의 ready8repo/160requests에서 selected-request attested만true였다.
- `VERIFIED`: Zustand20 fresh capture와 별도 process native replay가 각각 exit0이다.
  Root `/private/tmp/qi-e2-og-query-reader-ready9-zustand20-20261005-v1`, capture SHA256
  `8e2e9dbb5b5e853b6d4ca40b536296757a463c2eb30947229cccab7bdb0caf90`.
  Native12repo/17,615live와20개 요청의 segments_4/generation4/readerVersion16/live68/max69가 일치했다.
  현재 ready9repo/180requests의 selected-request attested 범위는 완료다. All-project readers,
  global indexed universe/서비스 전체 권위 및 최종 제품 비교 qualification은 남아 있다.
- 남은 repository/profile의 실제 acquired-reader scope 및 필요한 전수 source-byte/posting 권위를 확정한다.
  `opengrok_query_fixture.py`가 고정 원본→patch→Java→4classes 재현을 제공한다.
  `VERIFIED`: 외부 fresh `/private/tmp/qi-og-query-fixture-repro-20261005-v1`에서
  `python -m tools.benchmark.retrieval.opengrok_query_fixture --original <fixed-upstream-source> --output <fresh-root> --build-web-inf <sealed-baseline-WEB-INF>`를
  고정 이미지·`--pull=never --network=none`로 실행해 원본3자료와 기존4classes의 byte/SHA가 모두 일치했다.
  새 owner7 및 current authority/closure244를 함께 실행해251passed·45.04s/exit0, catalog guard/Ruff도 통과했다.
  optional build 실패 시 최종 출력은 미공개이며 기존 출력은 거절·보존한다.
  Instrumented timing은 pristine latency로 채점하지 않는다.
- source UID/file, directory/settings의 독립 분모·deployed ABI·frozen manifest를 유지한다.
  API GET/PUT403·read-only bind·declared seal 시간만으로 loaded reader를 입증하지 않는다.
- 완료: 제품×repository×profile의 입증한 source/index scope와 missing/extra/unknown 집합,
  실제 service/query/index 결속. disk 모드의 global universe/loaded-reader flags는false를 유지한다.

### O4-E2-03

P0 · W1/W4 · scheduler/admission consumer 구현 완료, 전체 required inventory의 결과가 남는다.

- 모든 required cell을 executed/reused/unsupported/failed/blocked/not_run으로 설명하고,
  terminal/actual input bytes를 검증한다. original source와 qrel-only reuse 허용 여부를 구별한다.
- ready repository를 먼저 drain한다. 살아 있는 process, malformed/wrong-repo terminal,
  upstream 종료 후 missing, output 경합은 readiness/success가 아니다.
- 완료: 누락 없는 inventory 및 실패 sibling에 독립적인 실제 ready drain.

### O4-E2-04

P1 · W4→W5 · collectors/joins 구현 완료; 나머지3repo·다른 lanes·최종 qualification 미완료.

- source97의 bat+required8 captures/replays/full5 joins는 완료 scope로 유지한다.
  현재 prepared9 밖 SQLAlchemy/Tailscale/Zellij는 admission 이후 실제 capture/replay/join한다.
- required lanes: exact1,196; prefix/infix/components; default/explicit typo; no-answer;
  C3 NL240; Gin20; ARB original17/88와 adapted88; B09 OSA/CLARC/CSN.
  four typo lanes1,192/1,178/1,192/1,192의 계약을 서로 합산하지 않는다.
- Quanta/Semble quality matrix와 native external collector를 동일 required inventory에서 연결하고
  source/query/unit/model/profile/clock 변경의 영향 셀만 fresh root에서 재실행한다.
- 완료: 실제 raw/exit/request/source/unit/clock 독립 replay 및 마지막 unjudged union의 E1 인계.
  실제 native completion은 relevance/whole-universe qualification이 아니다.

### O4-E2-05

P2 · W4 · Semble parent/process 계측 구현 완료, 반복 A/B·재사용 최적화 판정 미완료.

고정 package/lock/env/model/assets와 입력에서 parent/worker phase 및 unattributed residual을 대조한다.
재사용은 immutable validation과 native rows/status parity가 있는 작업에만 적용한다.
정식 speed는 [E4-06](#o4-e4-06)의 boundary/host/schedule을 따른다.

### O4-E2-06

P1 · W4 · bat quality warmup0/1 actual parity 완료; 다른 scope의 zero 정책 미판정.

bat 밖에서0을 선택할 때만 같은 task set/cold probe/profile/seed/repetitions의 두 actual runs와
각자 protocol SHA/measured schedule/phase ledger 및 task별 rows/status/score bits를 검증한다.
그 전에는1을 유지한다. order-sensitive 차이가 있으면0을 채택하지 않으며 speed는 warmup≥1이다.

## E3

담당: E3. 완료된 구현 계약과 독립 regression owners:
[Active/runtime lifecycle ADR](../../../adr/OCT-05-003-active-query-and-runtime-lifecycle.md).
Shipping/current-source 및 release 검증은 아래 I0가 소유한다.

### O4-E3-01

P0 · W1 scope 완료. 실제 disk-backed OS-child에서 G1선택→G2/G3활성화→양 track 물리퇴역→
typed refusal/open0→fresh G3/head/token/rows 및 정상 stop을 검증했다.
후속 selection/state 변경의 matching source proof는 [I0-02](#o4-i0-02); shipping Linux는 [I0-03](#o4-i0-03).

### O4-E3-02

현 Accepted retire-first refusal 계약에서 `NOT_APPLICABLE`.
선택만 된 generation의 무조건 성공/short-lived admission-pin transfer는 미채택이다.
강화 계약의 실제 채택·counterexample/lock ordering 및 bounded release oracle가 있을 때만 재개한다.
모든 active handle 영구 pin이나 SDK retry를 추가하지 않는다.

### O4-E3-03

P1 · W2 구현/owner scope 완료. 지원7 Active variant의 single-RPC response/head/token binding은 유지한다.
Current shipping acceptance에서 route별 실제 RPC trace/rows와 joint domains, ABA/stale token,
ancestor/cursor/exact-only refusal을 [I0-02](#o4-i0-02)에서 확인한다. Text/Symbol live count를
나머지 route의 실제 roundtrip으로 승격하지 않는다. 효과 판정은 [E4-06](#o4-e4-06).

### O4-E3-04

P1 · W1 구현/OS-child scope 완료. slow disk5cadence 동안 active readiness와 실제 adapter 완료,
owned cancellation/stop/join을 검증했다. backend loss/fatal 및 zero-active/restored identity negatives를
보존한다. Shipping/state-source 영향은 [I0-02](#o4-i0-02), Linux release는 [I0-03](#o4-i0-03).

### O4-E3-05

P1 · W1 default30s/OS-child/replay scope 완료. timeout 뒤 admitted publish Committed,
child 종료/재조립 뒤 exact replay/build0 및 conflicting digest refusal을 검증했다.
Shipping/source 영향은 I0에서 판정하며 async ACK·parallel dispatch는 현재 backlog가 아니다.

### O4-E3-06

P1 · W1 operator owner/OS-child 및 Linux 실제2UID socket component scope 완료.
Auth-before-ring·wrap/drop/instance/request correlation과 transport bound negatives를 보존한다.
Shipping Linux daemon 및 P11 process-truth는 [I0-03](#o4-i0-03)의 별도 actual inputs/results다.

## E4

담당: E4. 구현·조건부 변경 경계:
[cost/capacity/qualification ADR](../../../adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md).

### O4-E4-01

P1 · W1/W4 · F14 native 재사용·정확도 owner 회귀 `VERIFIED`; 전체 delta 비용 qualification 미완료.

- Matching release에서 full/delta/delete/no-op/reopen·fresh rebuild parity를 유지하며
  seal streaming/posting scan, fsync/syscalls/physical I/O, token/IPC 비용을 독립 분리한다.
- Opt-in causal 계측은 구현돼 있다. fresh-build actual profile과 canonical risk-daemon 결과를 확인한다.
  child>parent·mixed CPU·sample gap을 peak으로 추정하거나 logical disk를 physical I/O로 바꾸지 않는다.
- `VERIFIED`: clean08d에서 `./scripts/cargow --lane test-daemon-lane nextest run -p quanta-index-searchd-harness --lib --all-features --locked -E 'test(/^scale::tests::/)' --test-threads 1 --no-tests fail --success-output final`
  의32개 owner tests가 모두 통과했다. release tier actual/qualified speed는 포함하지 않는다.
- Historical F13: QI-BB-006 byte gate는 clean08d에서 실패했다. base08d에 reusable native-segment
  diagnostic test만 적용한 source에서9.482s/exit100: base402docs/1segment → delta401+1docs/2segments,
  base segment6files의 shared0/missing6 및 고정 untouched filler400개 전부의 segment 이동을 관측했다.
  삭제 처리 compaction이 실제 surviving data를 재작성했다. 전체 metadata139,885bytes 발행은 별도 비용이다.
  이 결과를 후속 F14 source의 현재 결과로 표시하지 않는다.
- F14 수리: 모든 native 문서 생산 경로에 exact indexed-field census를 저장하고,
  mandatory committed live-BM25 sidecar가 삭제·교체 문서의 통계를 차감한다. Native scorer는
  live N/token/DF를 사용하며 surviving segment를 compaction으로 재작성하지 않는다.
  메인에 반영한34개 경로는 [I0-02](#o4-i0-02)의 actual-tested candidate와 byte 일치한다.
  lexical394개 회귀에는 고정400개 untouched 파일의 native 위치/6개 component 재사용·byte gate,
  independent fresh rebuild의 score bits/pages, 연속 delta/delete/no-op 및 sidecar 손상 거절이 포함된다.
  기능·native byte 범위만 `VERIFIED`다. Marker parser43개 통과는 actual cost 관측이 아니다.
- 남음: fresh release full/delta/delete/no-op/reopen profile의 foreground read/write/elapsed,
  metadata 발행·custody 전체 읽기, correction 누적·segment fanout·transient peak를 검증한다.
  changed retained segment의 delete bitmap 비교에는 O(max_doc) CPU 순회가 남고,
  `NoMergePolicy`의 장기 segment 누적 비용은 미검증이다. Logical bytes·retained estimate를
  physical I/O·peak memory로 표시하지 않는다. 전체 QI-BB-006 비용 closure는 미완료다.
- 완료: 명시적 clock/resource domain과 source-bound 결과로 주요 residual의 실제 원인을 설명한다.

### O4-E4-02

P1 · 조건부W2 · group durable barrier 미채택; isolated sync 병목 조건 `NOT_RUN`.

E4-01 실제 syscall cost가 지배할 때만 설계한다. File sync/rename/hardlink/directory/root publish/
cleanup cut별 fault와 crash/reopen에서 old 또는 완전한 new root·참조 file/digest를 검증한다.
Barrier 실패 후 seal/activate를 거절하고 inherited page custody를 유지한다.
Power-loss 범위는 별도 실제 storage proof가 없으면 `NOT_RUN`이다.

### O4-E4-03

P1 · W1/W4 · scanner A/B comparator/CLI·independent 회귀 구현 완료, whole-call 결정 미완료.

- Scanner만 다른 exact source/binaries, 같은 source/input/observation clocks와 독립 tokenizer/full-DP
  oracle에서 실제 whole-call A/B 후 유지/수정/철회를 판정한다. 불가용 과거+8.75%는 새 proof가 아니다.
- bytes/span/case/order/status/cursor/work/config parity를 유지하고 mixed Unicode, short names,
  token cap/cancellation/cache identity를 검증한다. child 개선이 whole-call 악화를 덮지 않는다.
- 실행 진입점: `uv run --frozen --extra dev python tools/benchmark/retrieval/query_timing_overhead.py --help`.
  `--scanner-ab`의 실제 flags/spec를 확인한다. On/off observer 비교와 scanner 비교를 섞지 않는다.

### O4-E4-04

P2 · 조건부W2 · persistent token authority 미채택; repeated token-scan 병목 조건 `NOT_RUN`.

E4-01/03 after-scanner full-caller profile에서 조건이 성립할 때만 구현한다.
Exhaustive tokenizer/full-DP OSA1, source/grammar/folded byte/name witness와 delta/delete/no-op/reopen,
cold-open/build/residency/cap/cancel 계약을 독립 검증한다. 비용·memory/build tradeoff 미충족 시 추가하지 않는다.

### O4-E4-05

P2 · W4 · typed scale/load/preflight/ANN 구현 완료; default capacity gate `FAILED`.

- 256/4,096/32,768 tiers의 matching release/profile/lifecycle/open-loop·OS restart를 판정한다.
  default large30s timeout과 xlarge4,000,461 memberships 대4,000,000 cap 거절을 보존한다.
- large300s/256MiB diagnostic 성공 및4,096 OS restart 성공을 default 성공으로 바꾸지 않는다.
  지원 목표/latency/resource 계약을 결정한 뒤 원인 수리 또는 명시적 제품 계약 변경을 수행한다.
- 완료: 각 tier/profile의 독립 source/result/count/oracle와 terminal, offered/served/errors/timeouts/drops
  reconciliation. Fixture 축소·cap 미세 상향만으로 요청 capacity를 통과시키지 않는다.

### O4-E4-06

P1 · W4 · performance tooling 구현 완료, Darwin frequency admission `BLOCKED`; 정식 실행 `NOT_RUN`.

- 허용된 host의 continuous load/frequency/thermal/power/disk timeline과 사전 effect/uncertainty criteria,
  exact source/binaries/input/config/topology 및 동일 completed-response boundary를 확보한다.
- B07 최소5 fresh roots/route당1,000 warm observations, warmup≥1, randomized paired schedule 및
  independent full schedule/source/raw verdict replay를 실행한다. Host probe1회는 지속 admission이 아니다.
- 완료: 관측 effect/CI가 사전 acceptance로 판정된다. Phase/scale/shared-host timings은 diagnostic이다.

### O4-E4-07

P2 · W5 · 정책 변경 조건 미확인; 독립 qrels/span/holdout 뒤 판정.

Default OSA23·Gin4·NL/semantic residuals를 candidate/lane/contribution/rank unit/budget/cap/source/model/
generation으로 추적한다. Confirmed defect/accepted policy/label ambiguity/unsupported/qualification gap을
구별하고 같은 qrel의 independent ablation을 수행한다. Explicit OSA1 성공은 default 성공이 아니다.
변경 시 critical strata/no-answer/ambiguity 및 untouched holdout의 사전 허용 회귀를 충족해야 한다.

## I0

담당: 단일 integration owner. 구현 계약: [source/qualification ADR](../../../adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md).

### O4-I0-01

W0 coordination/control plane 구현 완료. Shared owner·소비자·독립 oracle·impact map과 actual
source/binary/input namespace 관리는 이후 각 epoch의 상시 규칙이다. 전체29 qualification 종료를 뜻하지 않는다.

### O4-I0-02

P0 · W3 및 source 변경 시 재수행 · proof issuer/verifier 구현 완료; fresh Contract preflight `FAILED`, 최신 SDK·hosted CI `NOT_RUN`.

- Frozen product `0e6c7e7e9494b63fdb33f4594df059817459d3b1`와 Python native/join
  `97eedd11b70e76c66985b15a968211a2faf92c6d` 결과는 각각의 historical source 범위다.
  후속 수리·문서/source-closure 변경을 그 전체 결과로 승격하지 않는다.
- `VERIFIED`: clean08d의 `PATH=/Users/songmin/.codex/worktrees/oct4-semantic-repair/quanta-index/.venv/bin:$PATH CARGO_BUILD_JOBS=1 just rust-profile test-daemon`
  은214passed/1skipped·226.315s/exit0였다. 같은 clean08d의 runtime lib에서
  `admitted_publish_timeout_tests::*`와
  `process_slow_disk_tests::os_child_slow_disk_port_does_not_stale_active_readiness`를
  `./scripts/cargow --lane test-daemon-lane nextest run -p quanta-index-searchd-runtime --lib --all-features --locked`
  의 exact selector·`--test-threads 1 --no-tests fail --success-output final`로 실행해3passed·34.701s/exit0였다.
  latest full/release는 별도다.
  OpenGrok index-scope/query-witness/fixture3 owner target은 benchmark-control local/PR 및 source closure에 등록했다.
  기존 formal Contract Python788에는 이 테스트들이 없으며,78/251 owner pass를 그 formal proof로 표시하지 않는다.
- Historical `FAILED`: clean08d lexical lib + `sealed_manifest`, `sealed_commitment_cost`,
  `generation_delta_base_carryforward`, `text_authority_shards`의 serial nextest에서
  `delta_generation_does_not_rewrite_unchanged_index_bytes`가 실패했고 이후50개는 미실행이다.
  같은 테스트만 exact selector로 재실행해12.090s/exit100, fresh index262,696 > base493,336/2를 확인했다.
  삭제된 segment compaction과 전체 authority metadata 발행을 독립 분리 검증하며 fixture·예산을 완화하지 않는다.
- F14 owner `VERIFIED`: `08d53378` 기반 private candidate의 검증된34개 경로는
  clean main `22ed5b0113f1209e208e9b7faba456cffcdebcc6`에서 전체 postimage byte 일치를 확인했다.
  Root는 stage/commit/push하지 않았다. Actual command:
  `./scripts/cargow --lane test-daemon-lane nextest run -p quanta-index-lexical --lib --test l2_file_mutation --test sealed_manifest --test sealed_commitment_cost --test generation_delta_base_carryforward --test text_authority_shards --test unicode_normalization_goldens --all-features --locked --test-threads 1 --no-tests fail --no-fail-fast --failure-output final --success-output never`
  →394passed/8skipped·519.233s/exit0.
  같은 Rust source의 `PATH=/Users/songmin/.codex/worktrees/oct4-semantic-repair/quanta-index/.venv/bin:$PATH CARGO_BUILD_JOBS=1 just rust-profile test-daemon`
  →214passed/1skipped·206.546s/exit0.
  `./scripts/cargow --lane test-daemon-lane nextest run -p quanta-index-searchd-runtime --test runtime_extended_suite --all-features --locked -E 'test(/^e2e_ranked_pages::/) or test(/^e2e_lexical_sealed_overlays::/)' --test-threads 4 --no-tests fail --failure-output final --success-output never`
  →5passed/77skipped·3.647s/exit0. Overlay publish refusal 1건에 nextest `LEAK`가 있어
  별도 조사했다. Test/runtime harness source는08d와 동일하며 driver shutdown+join 경로가 있다.
  같은 command의 `-E 'test(=e2e_lexical_sealed_overlays::an_overlay_publish_into_a_sealed_generation_is_refused_typed)'`
  ·`--test-threads 1 --success-output final` 단독 재실행은1passed/81skipped·1.306s/exit0, `LEAK` 없이 통과했다.
  최초 병렬 실행의 표시 원인은 미확정이며 child process/pipe 종료 qualification으로 표시하지 않는다.
  lexical `--all-targets --all-features --locked` Clippy `-D warnings`、hexagonal/module-cycle/wire/
  test-authority/no-allow/cargo-modules/format guards, causal parser43개는 각 실행 범위에서 통과했다.
  Cargo-modules는 contract/core만 보호하므로 lexical module tree 검증으로 표시하지 않는다.
  Matching-source release/SDK/hosted CI/operational qualification은 `NOT_RUN`이다.
- Fresh Contract `FAILED`: clean `615224a8985e64b081b0806d942b8662bfcd6cdf`에서
  `CARGO_BUILD_JOBS=1 QUANTA_INDEX_SCCACHE=0 just retrieval-contract-proof /private/tmp/qi-retrieval-contract-f14-615224a8-20261005-v1`
  은 pytest collection과 source-controlled required inventory 불일치로 preflight exit1이었다.
  실제791개에서 누락은0개, 추가는 기존 frozen-field bool/float alias refusal 회귀3개였다.
  `benchmarks/retrieval/proof-required-tests.json`에 이3개 identity만 추가한 watcher commit
  `4c55ead6f7ee761572ca20253330d17266ada3ed`는 기존 Python788·Rust191·SDK27을 보존한다.
  `uv run --frozen --extra dev python -m pytest -q tools/ci/tests/test_holdout_review.py -k completed_forms_refuse_typed_aliases_in_frozen_fields`
  →3passed/111deselected·0.93s/exit0. 별도 `proof_inventory.collect_pytest()`와
  `verify_inventory_authority(..., 'python')`는 실제791개와 고정 목록 일치/exit0였다.
  이는 collection 및 focused3 proof이며 formal Contract791개의 behavioral pass가 아니다.
  재시도는 새 clean source와 새 외부 root를 사용하며 실패한 root를 재사용하지 않는다.
- 선택 epoch의 포함 코드/driver/scorer/ADR 및 mandatory surfaces를 검증하고 matching fresh
  Contract/SDK/source closure/binaries를 발행·portable replay한다. E3 shipping acceptance와 CI도 실제 scope로 판정한다.
- 공개SDK/contract 변경: `just rust-public-api`; wire/decode: `just rust-fuzz-smoke`;
  module: `just rust-hexagonal`, `just rust-cargo-modules`; selection/state/ingress: `just rust-profile test-daemon`.
- runtime `autotests=false`: read-view/ingest는 `runtime_fast_suite`, generation/cursor/restart는
  `runtime_risk_suite`, crash/readiness는 `runtime_extended_suite`; 등록된 OS-child owner targets는 별도다.
- 완료: 정확한 source/command/selector/binary actual results와 필요한 CI/SDK/contract surface.
  Provider/Linux/release/scale 등의 미포함 경계를 명시한다.

### O4-I0-03

P1 · 코드 입력 먼저, W6 실행 · **P11 operational producer/recipes 미구현·설계 입력 `BLOCKED`**.

- 배포·활성화·restore-forward 실제 명령, distinct independent pre/post 성공 관측,
  authorized Linux host/path/config/state/retention/rollback window를 확정한다.
  현재 parser/schema는 nextest/pytest authority며 staged action을 발행할 수 없다.
- 입력 뒤 기존 result producer/schema/manifest/checker/aggregate와 Justfile recipes를 함께 구현한다.
  Generic shell exit0·caller-written success JSON·빈 test authority로 staged를 실행 가능하게 만들지 않는다.
- Canonical owner: [S21-12](../../sep-21-search-plane-sota-hardening/tickets/S21-12-cross-repo-terminal-receipt-cutover.md),
  [SEP-21 residual plan](../../sep-21-search-plane-sota-hardening/tickets/FINAL-RESIDUAL-EXECUTION-PLAN.md),
  [proof registry](../../../../tools/ci/proof-authority.toml).
- P00–P02 current prerequisites; P03–P06 activation/read-view/query/SDK;
  P07 approved real-provider; P08–P10 actual Linux supervision/readiness/state migration;
  P11 exact Semantica/Quanta pair and actual actions; P12A infrastructure 및 P12 aggregate를 각각 판정한다.
  Owner proof는 staged Linux release node를 닫지 않는다.
- Pair: actual dependency graph/package roots/QBC tests 및 binary custody를 결속한다.
  Producer omission oracle가 없으면 producer-side work로 남기며 lexical benchmark와 독립 진행한다.
- 완료: 요청된 `CODE_QUALIFIED`, `DEPLOYED`, `ACTIVATED`, `ROLLBACK_PROVEN` 각각의 actual prerequisites.
  상태 inventory, commit/push, local infrastructure pass는 해당 qualification이 아니다.

## 잔여 실행 진입점

실제 실행 전 입력과 selector를 확인한다. 이미 완료된 owner 회귀를 문서 정리 때문에 재실행하지 않는다.

| 범위 | Canonical command |
| --- | --- |
| review/binding 원인 수리 | `uv run --frozen --extra dev python -m pytest tools/ci/tests/test_holdout_review.py -q` |
| name/unit 원인 수리 | `uv run --frozen --extra dev python -m pytest tools/ci/tests/test_retrieval_native_span_projection.py tools/ci/tests/test_source_oracle_suite.py -q` |
| native metadata/capture 원인 수리 | `PYTHONPATH=. uv run --frozen --extra dev pytest -q tools/ci/tests/test_opengrok_index_scope.py tools/ci/tests/test_live_lexical_external.py` |
| actual quality matrix | `uv run --frozen --extra dev python tools/benchmark/retrieval/run.py quality-matrix --spec <issued-spec.json>`; 이후 `quality-matrix-verify --spec <same-spec.json>` |
| actual external native | `uv run --frozen --extra dev python tools/benchmark/retrieval/live_lexical_external.py --spec <issued-spec.json>`; 이후 `--verify <fresh-native-root>` |
| final source Contract | `just retrieval-contract-local`; `just retrieval-contract-proof <fresh-external-root>` |
| final source SDK | `just retrieval-sdk-proof-fresh <fresh-external-root>`; `uv run --frozen --extra dev python tools/benchmark/retrieval/portable_proof.py verify --receipt <fresh-root>/execution-context.json` |
| scale/open-loop | `scale_matrix` / `open_loop_matrix` matching binaries의 실제 `--help`로 flags를 확인한 뒤 external output 실행 |
| exact producer pair | `just rust-verify-hellgate-cross-repo <actual-Semantica-checkout>` |
| P12A / aggregate | `just proof-p12a-proof-infrastructure`; 실제 manifests와 `SEMANTICA_CHECKOUT`로 `proof-authority-code-gate`, `proof-authority-release-gate`, `proof-authority-final-qualification` |

세부 owner target/required identity는 위 canonical registry, Justfile와 actual collection을 따른다.
Historical 명령·SHA·실패 이력은 [Git 복구 인덱스](../../ARCHIVE-INDEX.md#oct-05-handoff-and-ticket-compaction)에 보존했다.
