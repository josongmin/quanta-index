# OCT-04 후속 실행 웨이브

[단일 잔여 인덱스](tickets/INDEX.md) · [담당·인계](README.md).
완료 C1–C3 구현·회귀 단계는 [Accepted ADR](../../adr/README.md#oct-05-implemented-contracts)에 압축했다.
아래는 **Quanta에서 남은 실행 순서**다. [작업표](tickets/INDEX.md#quanta에서-할-작업)가
현재 범위를 소유하며 외부 producer 연동은 아래 별도 기록에 보존한다.

## 코드 우선

- 실제 미구현: [I0-03 P11 typed operational producer/recipes](tickets/INDEX.md#o4-i0-03).
  실제 actions와 독립 pre/post 성공 판정 계약·authorized target 입력이 필요하다.
- Frozen5796 Large default timeout과 XL posting admission 거절이 재현됐다.
  E4-02 F15 source pack/root와 bounded authority 수리를 통합했으며 W2 actual 회귀 중이다.
  E1-07/E4-04/E4-07은 실제 병목·독립 정책 실패 뒤에만 채택한다.
- E1/E2/E3 및 scale/scanner/proof의 기존 구현은 실행 증거가 부족하다는 이유로 재작성하지 않는다.
- Large full seal68.733s 중 explicit sync49.778s를 관측했다. Parent sync 묶기만으로
  default30s 여유를 확보하지 못하므로 immutable pack/root와 reader/query budget을 함께 수리한다.
  Delta/noop residual의 exclusive owner 비용은 추가 계측 후 판정한다.
  Bat 재발행은 외부 issuer의 original review pack/merged product pack 신원 재생이며 새 relevance 판정이 아니다.

## 별도 producer 연동

- R3의 dispatch 전 독립 expected semantic replace/tombstone/unchanged 범위·누락 대조는
  Semantica source-plan/shadow policy/prior state/cluster plan owner 작업이다.
  [I0-03](tickets/INDEX.md#o4-i0-03)에 연동 수용 잔여로 보존하며 Quanta 자체 코드 건수에 합산하지 않는다.
- R5 clean pair는 producer 연동 수용 범위다. Quanta 단독 엔진·벤치의 완료 조건과 구별한다.

## 병렬 작업 배정 — 2026-10-06

현재 Quanta 소유 코드만 분리한다. 담당별 경로를 고정하고 변경은 I0가 통합한다.
빌드·테스트·native 실행은 현재 host에서 직렬이며, 별도 host의 실행은 같은 source와 input을 결속한다.

| 담당 | 독립 작업과 소유 범위 | 종료 조건 / 선행 |
| --- | --- | --- |
| I0 · F15 통합/검증 | `file_authority/` 경계 수정의 strict Clippy, codec golden·cold census·delta/delete/noop 영향 회귀, runtime restart | Guarded 수정은 통합됐다. 나머지 담당의 main 변경을 I0가 통합하고 최종 source에서 Contract/SDK를 발행한다. 기존598건은 수정 전 컴파일 source 결과 |
| E4 · Scale 용량 | `scale.rs` 사전 검사·history/timeout profile, Large/XL 단계별 비용 | 공유20M 한도와 fixed17,715,020 허용/20,000,001 거부 회귀는 통합됐다. Full/delta/noop/delete의 실제 retained bytes·latency·메모리로 지원 profile과 typed refusal 경계를 판정. Source 준비·판정 기준은 F15/SDK 대기 없이 진행 |
| Scanner · 두 빌드 비교 | `query_timing_overhead.py`, scanner source/build custody, 비교 테스트·사양 | 고정338-task Bat typo 입력과 scanner만 다른 두 source를 준비. 각 fresh runner를 자기 SHA에 결속하고 출력 parity·전체 호출 비용 비교. Actual arm build는 선택한 clean source 이후; 최종 SDK 발행 자체는 선행이 아님 |
| E2 · Ready9 OG/평가 | 원본90 input bindings, OG9 spec·readonly service/index 검증·raw replay, final join 연결 | OG 단독은 Quanta Rust/SDK proof와 독립. Exact63ac clean checkout에서 OG9 spec PREPARE 완료. 캡처의 Python10-role SHA·런타임·corpus/suite/pack·서비스 신원을 최종 source에서 재검증해 같은 raw를 명시적으로 join. Admission ISSUE·Quanta/SG pair·전체 join은 각 matching proof 이후 |

동시 배정된 에이전트는 `e4_causal_cost`, `process_regression`, `e2_og_universe`다.
이후 사용자 요청으로 실제 작업을 별도 sidebar 채팅 세 개에 위임했다.
Ready9 `01a10d0b-b1dd-72e1-9744-cab043867364`,
Scale `01a10d0b-bc57-7662-a2d3-315c1a07fdb5`,
Scanner `01a10d0b-c4c2-7be1-9a67-2a0482724e4b`이며 I0는 원래 통합 채팅에 남는다.
조사·수정·준비는 병렬, 현재 host actual pipeline은 Ready9→Scale→Scanner 순서로 예약한다.
별도 채팅 담당은 각자 actual 실행까지 소유하며 이전 subagent-only 실행 제한을 적용하지 않는다.
Host slot·terminal outcome/cleanup 조정은 `/private/tmp/qi-sidebar-dispatch-20261006-v1/`에 둔다.
인계 문서는 각각 `/private/tmp/qi-parallel-scale-20261006-v1/`,
`/private/tmp/qi-parallel-scanner-20261006-v1/`, `/private/tmp/qi-parallel-ready9-20261006-v1/`에 준비한다.
OG 독립 source는 `/Users/songmin/.codex/worktrees/oct6-og-63ac/quanta-index`의
`63ac399f27eba896ed9d9ceaef727166ae10d684`이며 최종 Quanta proof source로 재표기하지 않는다.

인계 가능한 작업서는 [Scale](/private/tmp/qi-parallel-scale-20261006-v1/handoff.md),
[Scanner](/private/tmp/qi-parallel-scanner-20261006-v1/WORK_ORDER.md),
[Ready9 OG](/private/tmp/qi-parallel-ready9-20261006-v1/WORKORDER.md)다.
OG9 PREPARE는 원본90 input guards·clean exact source·Python10-role before/after와
resolved Python runtime을 결속했다. Docker/HTTP 신원·native capture·replay는 `NOT_RUN`이다.

Python 전체 CI와 Rust runtime/API/wire 검증은 소유 경로가 겹치지 않는 별도 검증 작업이다.
현재 host에서는 I0가 순서대로 실행한다. AI quota·rubric·holdout 승인·provider/Linux 입력은 해당 scope만 대기한다.

## 확인된 실행 체크포인트

- sourcea84f237c에 clean74bdc9b4의39 owned paths를 exact pre/post SHA로 통합했다.
  SDK caller/binding·staged publication·streaming digest·bounded CBOR scratch·명시적 scale profile을 포함한다.
  Owned Rust30paths rustfmt·diff 검사는 `VERIFIED`다. API/module actual4gates·external consumer5tests·
  접근 차단4compile은 모두 `VERIFIED`다. API255/module194 input bytes 및 actual output/pre/post SHA를 확인하고
  baseline3paths를 main에 통합했다. SDK public API는 기존과 동일하다. Clippy/LargeXL 실제 종료는 남는다.
  F15 selected actual은 아래79passed 결과이며 matching Contract/fresh SDK는 아직 종료하지 않았다.
  이전 source7fb46415의 CircleCI verify1730/verify-python1729는 각각 upload rustfmt drift와 contract/core
  cargo-modules baseline 누락으로 `FAILED`다. 해당 owned 수리 뒤 새 exact-source CI를 확인한다.
- Ready9 final static 원본90/admission73/Bat105/OG39,669파일과 runtime 감사 및 guard9/9가 통과했다.
  Source107 helper3개의 relocated lookup 수리는 외부 guarded 후보다. 새 proof와 host slot 전달 이후
  actual admission/pair/SG-CS/replay/full5를 진행한다. 정적 준비를 actual 결과로 세지 않는다.
- Sidebar 회수: Ready9 OG9/9 capture·independent replay는source63ac에서 `VERIFIED`다.
  Scanner fixed338/79files/2,030completed response parity는sourcef2dfe089의 explicit allow-incomplete
  diagnostic에서 `VERIFIED`; strict symbol coverage·qualified speed는 그 결과에 포함되지 않는다.
  Scanner owned3path 수정은 exact SHA guards로 main에 통합했다. Focused55tests는 owner snapshot 결과다.
- Scale source898d2dfb의 threshold2/owner36/CLI2/Clippy/release는 통과했다. Large default는
  required81,764,348B > pair16,777,216B, XL은 decoded385,260,565B > cap134,217,728B로 `FAILED`다.
  별도 Large explicit diagnostic lifecycle/replay는 `VERIFIED`, XL lifecycle은 wire 거부 뒤 `NOT_RUN`.
  이후 main bc18e67e에 bounded source-upload 코드가 추가돼 Scale 채팅이 SDK/daemon/XL actual을 진행 중이다.
  Latest Rust/runtime 영향 회귀·final Contract/SDK·Ready9 full5 join은 아직 남는다.
- 후속 source4cc8f5b9의 digest 설명과 Large/XL realOS-child owner 회귀를 통합했다.
  Test-authority·ignored-policy·포맷 검사는 통과했으며 Medium default을 유지한다. OS3 실행 결과는 아래 후속 실패/수리와 구분한다.
  Source74bdc9b4 `just rust-test-e2e`는214passed/1skipped·301.789s, main F15 selected79는
  79passed/527skipped·203.351s로 `VERIFIED`다. 영향 회귀 범위이며 전체 workspace·최종 proof가 아니다.
- Sourcec93aa614의 workspace strict Clippy는 must-use1·같은 match arm2, 재검사23a52737은
  harness의 같은 arm2·JSON indexing6에서 각각 `FAILED`였다. Source23a52737/38e44040의4-path
  수정은 exact guards로 main에 통합했고 포맷 검사는 통과했다. JSON 객체/중복 필드 거절과 Result 전파를 추가했다.
  이후38e44040은 runtime test2paths의 lint5건에서 실패했다. Source2ea1408d의 test-only delta도
  exact guards로 main에 통합했다. Source2ea의 OS3 실제 실행은 compile exit101로 `FAILED`, 테스트는 실행되지 않았다.
  SourcePublicationUploadAck 누락3곳을 closed match로 수리했고 unit의 미선언 anyhow1줄도 기존 오류 변환으로 고쳤다.
  Clean05bdaeb9/cache exact3 실제 실행은2passed/1failed/82skipped·439.947s로 `FAILED`다.
  Medium256·Large4096은 통과했고 XL32768은 coverage decode residency envelope에서 typed 거절됐다.
  Canonical envelope/profile·writer preflight·cold decode/reopen/runtime charge 정합 수리 뒤 원본XL을 재실행한다.
  후속e61a31ce의9-path common bound/Arc key 및 capacity/replay 정합을 exact guards로 main에 통합했다.
  고정64MiB/256MiB 한도는 유지하고 malformed 최대 페이지 예약을 포함한다. Owner coverage/upload39는
  39passed/989skipped·35.965s, Python causal76은0.18s/exit0다. 새 contract public helper의 API 영향과
  실제XL/OS3·전체Clippy·final proof는 별도로 실행한다.
  Request-decode fuzz970,826runs/61s는 통과했고 남은 fuzz3은 미입장 취소 후 `NOT_RUN`이다.
  Source2ea release scale_matrix build18m16s/exit0는 `VERIFIED`; 전체 Clippy/unit 재검사와 native Large/XL은 남는다.
  Hosted source31828561의 verify1741도 동일3건, verify-python1742는 module baseline에서 실패했다.
  후속 e6b1b7e9의 hosted verify1751은 staged-upload test-only lint6건, verify-python1752는 baseline에서 실패했다.
  Test-only1path를 기존 invariant를 유지해 수리·통합했다. API/consumer actual은 종료했으며
  후속080568c7 verify1754의 Active-process test-only lint3건도 checked predecessor/panic payload 보존으로
  수리·통합했다. 후속07c5fed9 verify1759의 socket UID singleton iterator1건도 동일 집합을 유지해 고쳤다.
  실제 strict Clippy·전체 테스트는 재검증하며, XL coverage 및 final proof 뒤 Ready9 실행을 연결한다.
  Hosted Python1753은080568c7에서3723passed/30skipped 뒤 required inventory791vs793로 실패했다.
  아래 +2 manifest 수정으로 원인을 수리했으며 새 소스의 hosted 결과로 재검증한다.
- Final Contract source080568c7 첫 실행은 collection `FAILED`: 기존791 필수 검사는 모두 남았고
  admission split batch 회귀2개가 미등록돼 actual793과 달랐다. 필수 manifest에 해당2ID만 추가했으며
  exact793collection·각 누락 거절을 확인했다. Rust/SDK 목록과 Frozen5796 결과는 그대로 유지한다.
  새 source와 fresh output에서 canonical Contract/fresh SDK·portable replay를 다시 실행한다.
- Frozen5796 Contract Python791/Rust191와 fresh release SDK27은 actual 및 독립 portable replay `VERIFIED`.
- 같은 source의 별도 release `scale_matrix` build와 small16/medium256 causal run은 `VERIFIED_DIAGNOSTIC`.
  Large default30s timeout과 XL preflight4M posting 거절은 `FAILED`다.
  Large300s/256MiB는127.612s에 완료한 별도 진단이며 default closure가 아니다.
  Current78d2474 Medium256 OS-child stop/reap/restart 회귀는1passed/82unselected·31.805s로 `VERIFIED`.
  Large/XL OS-child restart·open-loop·quiet-host 성능은 `NOT_RUN`이다.
- ready9 OpenGrok selected-request180은 완료; 전체 service/index 권위는 미완료다.
  Frozen5796 CLI admission20tasks/519pairs만 actual exit0이다. Django admission은 중단했고
  나머지 admissions·pair·SG/CS·full5 join은 완료 증거가 없다.
- Frozen5796 Gin declaration1,196 fresh single-route oracle/capture/scoring 진단은 완료했다.
  1,192success/4capped 및 declaration MRR@10=1.0은 그 분모의 diagnostic이며 독립 holdout/비교/PERF가 아니다.
- Current history/admission Python owner58cases와 후속 retention/batch67cases는 각각 통과했다. Broad Python은 current file-pair
  fixture의 불완전한 manifest에서 실패했다. Canonical stage·clock/binary/capped fixture 보완 뒤
  current-file 전체와 기존 verdict4개 경로는24passed·31.97s로 `VERIFIED`다. Broad 재실행은 `NOT_RUN`이다.
  Hosted Rust 계측 Clippy6건 및 marker enum1건 수리를 반영했다.
- F15와 기존 format/cost fixture 전환 후 lexical 전체는598passed/8skipped·702.173s/exit0였다.
  이 실행은 `0b5409a2`의 컴파일 결과다. 이후 strict 경계 검사·공유 posting 한도 수정 overlay의
  영향 회귀는 별도로 재검증한다. Strict Clippy 최초176건을 수정·통합했고 다음 실행의 이름 충돌4건도
  수정했다. 이후 v6는 lexical lib-test의36건에서 `FAILED`였다. Producer8/reader10/root·verify13/facade5를
  병렬 수정해 통합했다. v7의
  `CARGO_BUILD_JOBS=1 QUANTA_INDEX_SCCACHE=0 QUANTA_INDEX_TARGET_GC=0 ./scripts/cargow --lane test-f15-owner-lane clippy -p quanta-index-lexical -p quanta-index-searchd-harness --all-targets --all-features --locked -- -D warnings`
  는exit0/322.077s로 `VERIFIED`다. 이는 두 owner package Clippy이며 actual Rust 회귀·최종 Contract/SDK proof가 아니다.
- Scanner comparator의 각 runner SHA 결속 fixture와 negative-test 전제 검사를 보완했다.
  `uv run --frozen --extra dev python -m pytest -q tools/ci/tests/test_query_scanner_ab.py`는17passed·10.91s/exit0로
  `VERIFIED`다. 실제 두 fresh 빌드·A/B 캡처는 `NOT_RUN`이다.
- 이후 source/ADR/Justfile 변경의 proof는 새 epoch로 발행한다. 위 frozen 결과의 SHA는 변경하지 않는다.

## W0–W6

| 웨이브 | 실제 다음 작업 / 담당 | 선행·인계 및 종료 경계 |
| --- | --- | --- |
| W0 | I0: 후속 source/dirty/hunk owner와 claim/input 범위 확인 | Shared 단일 owner, 실제 source 영향·fresh namespaces. 상시 통합 규칙 |
| W1 | E1: SQL/Zellij 최종 판단·Tailscale rubric·새742pairs·holdout 준비. E2: actual reader/index scope. E4: causal/full-caller profile | 각 input/quota/host 범위만 BLOCKED. 완료 raw 보존, 독립 strata/oracle·조건 판정 |
| W2 | E4: 확인된 Large sync/XL bounded authority 수리. E1-07/E4-04 조건부 최적화 | Source pack/root·reader/query 한 번의 cutover, global cap/단계별 work·crash custody 및 independent oracle. Owner regression→I0 영향 검증 |
| W3 | I0-02: selected source Contract/SDK/CI. E1-03: final revisions 및 남은3repo admission ISSUE | PREPARE→VALIDATE→ISSUE. Frozen old proof를 최신 전체 source로 재표기 금지 |
| W4 | E2: ready required cells actual capture/independent replay/join·scope별 warmup parity. E4: A/B·capacity·qualified performance | Matching binaries/input/index/clock, 실제 host/schedule. Failed sibling은 ready cells를 막지 않음 |
| W5 | E1: 마지막 unjudged union→labels/admission→independent scores/CI. E4: holdout 기반 정책 RCA | Qrel-only reuse 허용 여부 확인. Name/NL/no-answer/ARB/B09 분모·human/unseen 범위 별도 |
| W6 | I0-03: Quanta 운영 producer/recipes·real provider·Linux release/state·P11 actions·aggregate | 실제 target/독립 관측 계약·authorized inputs/results. Registry의 외부 pair는 별도 연동 수용 범위 |

## 현재 dependency

- E1-01 valid judgments → E1-02 supplemental merge → E1-03 admissions.
- I0-02 current-source proof → 해당 E1-03 ISSUE, proof를 소비하는 E2-04 pair/join 및 E4-06 qualification.
- OG 단독 capture/replay는 Quanta Rust/SDK proof와 독립이다. Final source에서 Python10-role/runtime/input/service binding을 재검증한 raw만 E2-04 join에 연결한다.
- Scanner 두 arm과 E4-05 capacity 진단은 각 clean source/binary/input 결속을 먼저 충족한다. Final Contract/SDK 발행을 모든 준비·진단의 전역 선행으로 두지 않는다.
- E2-02 scope 및 E2-03 required inventory → ready E2-04 native capture/replay/join → E1-06 final pool.
- E2-06 parity를 갖춘 scope만 quality warmup0; 나머지는1. Qualified speed는 warmup≥1.
- E4-01 isolated cost → E4-02 barrier; E4-01/03 after-scanner cost → E4-04 token authority.
- E1-04/05/06 독립 qrel/span/holdout → E4-07 정책 판단. 새 holdout은 ready 기존 cohort 재채점의 전제는 아니다.
- E3-01/03/04/05/06의 완료 owner/process scope는 I0 matching shipping-source/release에서 소비한다.
  E3-02 pin transfer는 현 Accepted 계약에서 비적용이며 dependent code의 대기 조건이 아니다.
- P11 contract/target 입력 → existing typed authority/recipe 구현·검증 → actual action 실행 → aggregate.

## Runtime 실행 규칙

- Source 조사·fixture/static은 병렬; 실제 owner tests/build/model/Docker/native/scale/performance는 host별 직렬.
- 실패·blocked·missing·unsupported·미선택 셀을 inventory에서 제거하지 않는다.
- Raw/input/proof 영향을 받은 범위만 재검증하며 이전 실패/partial와 source identity를 보존한다.
- Wave 진입은 repository/claim별이다. 모든 labels·최적화·Linux inputs를 기다리는 전역 barrier가 아니다.
- 실제 명령·입력·완료/거절 조건은 [잔여 인덱스](tickets/INDEX.md)를 따른다.
