# OCT-04 후속 실행 웨이브

[단일 잔여 인덱스](tickets/INDEX.md) · [담당·인계](README.md).
완료 C1–C3 구현·회귀 단계는 [Accepted ADR](../../adr/README.md#oct-05-implemented-contracts)에 압축했다.
아래는 **Quanta에서 남은 실행 순서**다. [작업표](tickets/INDEX.md#quanta에서-할-작업)가
현재 범위를 소유하며 외부 producer 연동은 아래 별도 기록에 보존한다.

## 코드 우선

- Manifest allocation bound·staged upload cancellation 및 open-loop artifact/CLI의 stale Scale gold를
  main에 반영했다. Manifest3paths는 patch6c039748의 guards로 통합했으며 collection count·행별 node
  상한과 기존 body tag·array-only digest·BIGPOS version·indefinite EOF를 보존한다. 지원 corpus/encoded
  한도는 유지한다. Leading BIGPOS version과 BIGPOS/BIGNEG body는16byte를 넘는 payload를
  할당 전 거절하며 canonical writer는 이 비정규형을 발행하지 않는다. Focused 회귀를 먼저 실행한다.
- Contract/SDK·Ready9 선택 소스는 clean `5658b953690896525b1ae7bde2950adc00adf6b3`다.
  `886673cd`의 admitted IPC/manifest·open-loop 회귀를 보존하고 테스트 모듈 위치만 수정한565에서
  whole unit/strict/runtime/integration/fuzz4/native와 Root OS3/Contract/fresh SDK를 연결한다.
  병렬 NativeIdentityCopy/Admission API·feature manifest·normalization vendor/control 변경은
  선택 소스에서 제외해 whole main/API 검증과 구분한다. 최신 main38eda80a의42paths도 별도 범위다.
  OS3 release 등록 수리는 별도 clean492에서 검증한다. Debug 전용 semantic 회귀의 mod cfg1줄만
  변경했고 제품·테스트 본문은 동일하다. 기존565 proof/context를492의 결과로 재표기하지 않는다.
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
resolved Python runtime을 결속했다. 그 PREPARE 시점에는 Docker/HTTP 신원·native capture·replay가 `NOT_RUN`이었다.
후속 source63ac의 OG9/180 capture·독립 replay는 아래 회수 결과대로 완료됐다.

Python 전체 CI와 Rust runtime/API/wire 검증은 소유 경로가 겹치지 않는 별도 검증 작업이다.
현재 host에서는 I0가 순서대로 실행한다. AI quota·rubric·holdout 승인·provider/Linux 입력은 해당 scope만 대기한다.

## 확인된 실행 체크포인트

- 최신 main38eda80a에는 normalization vendor/native scratch feature/control 수리42paths가
  반영됐다. Canonical `cargow metadata --locked --offline --all-features --no-deps`는 실제
  exit0/28workspace로 dependency 해석 `VERIFIED`다. 새 Rust·module/API·whole CI는 별도 actual이
  필요하며 진행 중인 선택565 proof를 이 main의 결과로 재표기하지 않는다.
- 최신 Source565의 affected owner46개는 실제46/46·1.126s/exit0로 `VERIFIED`다.
  SDK565 daemon fresh release build도29m04s/exit0다. Whole workspace 및 SDK27개/portable
  proof의 실제 결과는 아직 남는다.
- Source565 Contract Python793개는 실제 `VERIFIED`(289.98s)다. Rust191 inventory의 build는
  2m19s/exit0이며 행동 검증·최종 context/portable verify는 미완료다. SDK fresh release는
  정상 admission 후 실제 build 중이다. Compiler/collection 성공을 테스트 통과로 승격하지 않는다.
- Source565 OS3 actual은 release compile `FAILED`(E0432/exit101·5227.025s), tests0개다.
  Debug 전용 semantic test_support를 쓰는 회귀 모듈에 producer와 같은 `cfg(debug_assertions)`를
  붙인1줄 수리를 main 및 clean492에 적용했다. 제품·회귀 본문 불변·owned fmt/diff는 `VERIFIED`다.
  Source492에서 원본 release OS3 256/4096/32768과 debug interruption1개를 정상 admission으로
  재등록했다. Cold dependency cache는 정상 Cargo 검증 아래 재사용하고 실패 raw는 보존한다.
  이미 admitted인565 fresh SDK·Contract/Ready9 principal source 및55epoch은 유지한다.
  새 actual 결과는 미발행이며565 release suite 성공이나 whole main 통과를 뜻하지 않는다.
- Main16f37의 exact-source CI는 `FAILED`: Python1120 locked dependency fetch가 exit101·0.809s로
  unicode-normalization0.1.25에 없는 `quanta-native-scratch-v1` feature를 보고했고,
  Rust1119 guarded module snapshot 검사는 exit1·4.185s였다. 해당 변경 owner의 dependency 연동
  수리 후 module/API·whole CI를 재실행한다. 실패한 dependency 해석으로 baseline을 갱신하지 않으며,
  진행 중인 선택565의 엔진 검증 소스는 그대로 유지한다.
- Source886 focused owner46/46·3.600s와 open-loop full20/20·13.758s가 실제 exit0/lease release로
  `VERIFIED`다. Old controller를 두 leaf 뒤143으로 정리하고 raw를 보존했다. Body 불변인565에
  Scale broad unit/strict/runtime/integration/fuzz4/native 및 Root OS3 원본3/Contract/fresh SDK를 연결한다.
  Root3 rail은 실제 등록됐고565 OS3 compile 실패와492 수리는 위에 분리했다. Contract/SDK closure는
  1500files/digest58d62e6c로 일치하며 Python793 actual PASS·SDK admitted fresh build는 위에 회수했다.
  Context/portable result는 미발행이다. Ready9 final consumer는 PREP-v3의55-binding epoch 검사를
  통과했고 PREP-v2의54-binding은 audit-only로 유지한다. 실제 stage/final slot은 아직 미발행이다.
- Main98 Rust1111은372.012s/exit101로 동시 추가된 API 테스트 `redundant_clone`1건에서 `FAILED`다.
  비교값의 불필요한 clone만 제거했고 제품/API 본문·선택565는 불변이다. 새 whole CI actual은 남는다.

- Main2c Python1107 전체 job은 `VERIFIED`:4190passed/30skipped·869.04s,
  P00300passed·34.97s와 source-bound manifest/pre-commit actual exit0다.
  Main d192 Rust1110 strict는 `FAILED`(350.112s):manifest `cfg(test)` 모듈 뒤 제품 item1건.
  EOF 이동 수리의 제품/테스트 본문은 동일하며 owned fmt/diff exit0다. 새 strict actual은 남는다.
- Source47e native9 leaf는 canonical completed/exit0 및9/9 receipts로 `VERIFIED`다.
  최종 byte 검사·shared lease release·owned child 종료 후 old controller를 native 경계에서 정리했다.
  SG/CS stage/boundary guard4+4는 PASS이며9capture/9replay 및 final5 actual은 `NOT_RUN`이다.
  Root565 source epoch·75control 재결속은 정적 확인이고 실제 slot handoff가 아니다.

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
  후속5da25296의 structural bound 변환3곳도 explicit branch로 수리·통합했다. 최신 main774594d7의
  hosted verify1767은 새 malformed-reservation test의 panic_in_result_fn1건에서 실패했다.
  세 조건을 explicit Err로 그대로 유지한 test-only 수리를 통합했으며 owned rustfmt/diff가 통과했다.
  OS3v3는 이 후속 수리 적용을 위해 미입장 상태에서 안전 취소해0actual tests/`NOT_RUN`이다.
  API contract 입력과 제품 body는 이 test-only delta의 영향을 받지 않는다.
  Source5da 전체 strict Clippy는2687.131s/exit101로 위 lint1건에서 실패했고 후속 owner24860 결과는 아래와 같다.
  Main47e의 hosted verify1775는 SDK test transport의 Result assertion1·Mutex scope2로 실패했다.
  조건을 유지한 explicit Err/checked arithmetic·짧은 lock/Copy snapshot1path 수리를 main에 통합했다.
  Guarded apply/rustfmt/diff는 통과했으며 실제 Clippy/fixture는 별도로 재검증한다.
  후속 owner24860 Clippy는 SDK를 지나 IPC test helper 시간 계산2·clone3에서 실패했다.
  IPC cfg(test)1path 수리를 main에 통합했고 production prefix byte 동일/guards/rustfmt가 통과했다.
  Coverage helper API/module actual이 모두exit0라 reviewed1linebaseline을 main에 반영했다.
  Coverage consumer2도 nightly2026-08-01/locked/offline actual2passed·exit0로 완료됐다.
  Main242는 API252/255/module193/194이며 cfg(test)2개와 unreachable Tantivy lock edge를 분류했다.
  Producer114패키지 projection 동일/consumer42에 Tantivy와 변경 상위 manifest5개가 없음을 확인했다.
  기존 SDK/consumer5/negative4의 source-bound 결과는 그대로 두며 새 one-helper scope와 구분한다.
  영향 테스트만 수리하고 다음 전체 strict에서는 독립 target 오류도 끝까지 수집한다.
  OS3v4는 SDK 후속 수리 때문에 미입장 취소해 exit143/0actual tests/`NOT_RUN`이다.
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
  후속 Python1760은3870passed/30skipped 뒤 stale reopen fence에서 실패했다. 실제 위임·오류 전달을
  검사하도록 해당 guard를 수리했고6passed/0.43s다. Broad Python과 실제 OS restart는 별도 검증한다.
  Main47e Python1776은4181passed/30skipped/2warnings·858.00s 뒤 Ruff format2곳에서 실패했다.
  해당2파일 포맷 수리 뒤 Ruff check/format 및 focused73tests가 통과했다(0.51s).
  전체 pytest 실행 통과는 hosted job 전체 통과가 아니다.
- 최신7ebaf9d3 Rust1073은 Tantivy 미사용 선언2개로 machete `FAILED`였다. 실제 사용처0을 확인해
  live manifest2선언+lock direct edge1행만 제거했고 `metadata --offline --locked`/machete exit0다.
  `.orig`와 fs2의 winapi transitive는 유지한다. Python1074는4181passed/30skipped·854.52s 및 Ruff442파일
  뒤 cargo-deny의 search-plane default feature에서 실패했다. 의존성5간선의 explicit default-features=false와
  소유 empty default 제거 후 metadata/deny exit0다. Explicit test-runtime-barriers와 deny 규칙은 유지한다.
  기존 model2vec license-field warning은 남는다. Main242 `just rust-policy`15recipes는 actual exit0이며
  hosted1076 fmt+workspace/all-targets/all-features/locked strict Clippy도 exit0/373.173s다.
  Linux strict 결과이며 wholeCI/macOS actual unit/runtime/release는 별도 검증한다.
  같은242 verify-python1075는4181passed/30skipped·853.25s와 Ruff442/rust-policy15/semantic3를
  통과한 뒤 maintenance의 `join().is_err()` fallback 규칙 위반에서 `FAILED`였다.
  Explicit match로 기존 취소·sender close·join·fatal 판정을 유지했고 `just rust-fallbacks`는
  19checker tests/577scoped files에서 exit0다. 파일 rustfmt/diff도 통과했으며 새 whole CI는 남는다.
  Main2fb8e0fe와 clean Scale254a69fe의 Rust 빌드 입력은 동일하다. OS3 release 원본3개와
  immutable main2fb의 canonical Contract793/191·fresh SDK27/성공 뒤 portable verify를 실제 등록했다.
  Correct wait7200s/jobs1/sccache0/gc0이며 actual terminal/새 receipt 발행은 아직 남는다.
  후속 단일 native scope의 verify-time release 변경 감지 누락 수리를 포함하기 위해 Contract/SDK2요청만
  미입장 안전 취소했다: producer143/controller1/0Cargo·behavior/`NOT_RUN`, v4partial 보존·receipt0.
  현재 Ready9 batch는 outer 후행 검사로 해당 경계를 보호한다. 당시 Rust OS3 release254a 요청은 유지했으며 아래 수리 뒤6385033f로 대체했다.
  후속1076 nextest는3567passed/1failed/29skipped·380.582s,606미실행 fail-fast로 `FAILED`다.
  Stale harness256MiB oracle만 fixed Scale2GiB/explicit512MiB equality/+1·zero refusal로 수리했고
  real one-byte seal typed거절을 유지했다. Production prefix 동일·포맷·fallback19/577는 통과했다.
  Main2fb hosted1080 strict도 exit0/407.888s다. 수정 fixture actual 및 whole unit은 별도 검증한다.
  OS3 release254a는 미입장143/624.7525s/0Cargo·tests로 취소해 oldraw/cache를 보존했다.
  Clean6385033f의 원본3counts/seed/caps release/no-fail-fast를 새 cache에 등록했으며 terminal은 남는다.
  Source2fb Python1079는4181passed/30skipped·869.96s 및 모든 Python/Rust policy/Semgrep를 통과했다.
  P00 registry25/300tests·35.23s 뒤 writer direct import에서 `ModuleNotFoundError: tools`로 job `FAILED`였다.
  Direct-script import 초기화를 수리하고 payload 본문은 유지했다. Main host/CLI 회귀28은28passed/0.68s,
  Ruff/diff도 통과했다. Actual CircleCI P00 manifest 발행·whole job은 새 source에서 확인한다.
  후속 source593 Python1090도4187passed/30skipped·880.73s 및 policy step935.449s/exit0,
  P00 300passed/34.62s 뒤 동일 direct import로 job `FAILED`였다. Source731 hosted1094 strict는
  exit0/367.310s이며1093 Python·1094 Rust whole job의 terminal은 아직 남는다.
  Source731 Python1093은4190passed/30skipped·851.53s 및 policy903.306s/exit0, P00 300passed/33.50s와
  actual manifest 발행/source binding까지 통과했다. Whole job은 이후 pre-commit의 README anchor1건 및
  vendor JSON2개 EOF 수정으로 `FAILED`였다. 날짜 독립 anchor와 payload 불변 EOF 정규화 뒤
  EOF hook/doc-path/PM5/diff 재검사는 exit0다. 새 whole CI의 성공을 뜻하지 않는다.
  Native scope는 callee의 소비 전·native 뒤/receipt 전·verify 후 세 경계를 유지하는 수리를 통합했다.
  Batch spec/control guard를 유지하고 중복 outer2검사만 제거해 scope당4→3full scans다.
  Main 관련171tests는171passed/1004.84s 및 exactpostSHA/Ruff/diff가 통과했다. 실제 latency 향상은 미측정이다.
  Clean731a39cf를 immutable `oct6-f15-final-v6/quanta-index`에 고정해 Contract793/191·fresh SDK27 및
  성공 뒤 portable verify를 새 v6 output에 등록했다. 후속 Rust 수리를 포함하도록 이2요청과 OS3release638의
  자기 미입장 leaf만 안전 취소했다. 각 producer143/0Cargo·behavior, v6 receipt0·793collection·빈 target을
  보존한다. 최종 통합 source 한 번 고정 뒤 focused IPC/manifest/open-loop bin → whole unit
  `--no-fail-fast`/1thread·strict `--keep-going` → runtime/fuzz3/native를 Scale owner가 재등록한다.
  Root는 matching OS3 release원본3 및 Contract/fresh SDK/portable verify를 맡는다. Correctwait7200/
  jobs1/sccache0/gc0을 유지하며 새 source의 actual terminal·receipt 발행은 남는다.
- Source593 Rust1089는3571passed/1failed/29skipped·381.070s,602미실행으로 종료했다. 실패한 artifact
  default16MiB gold와 CLI의 같은 default·explicit-pair/default-total gold를 cfg(test)2paths에서
  fixed1GiB/2GiB·`scale-supported-v1`로 수리했다. Production prefix는 동일하며 추가 static history
  caller/artifact 불일치는 찾지 못했다. Fixture actual은 `NOT_RUN`이다.
  Staged-upload 취소 처리는 mainba06fc12에 통합됐고 이 source의 hosted1104 strict는 doc 길이·Option
  match2건에서 exit101/87.747s로 실패했다. Main451908e5에서 의미 동일한 doc/map_or로 고친 뒤
  fmt/diff 및 fallback19/577/179parsed는 exit0다. 실제 owner Rust/Clippy와 whole CI는 남는다.
  당시 Native47e receipt7/9 관측 뒤 native leaf9/9가 종료/lease release까지 완료됐다(위 최신 checkpoint).
  SG/CS capture/replay는 별도 일반 admission으로 실행한다. 최종 join은 matching source proof와
  명시적 slot handoff 뒤 판정한다.
- Final Contract source080568c7 첫 실행은 collection `FAILED`: 기존791 필수 검사는 모두 남았고
  admission split batch 회귀2개가 미등록돼 actual793과 달랐다. 필수 manifest에 해당2ID만 추가했으며
  exact793collection·각 누락 거절을 확인했다. Rust/SDK 목록과 Frozen5796 결과는 그대로 유지한다.
  후속 clean47e Contract는 source closure·793collection authority PASS 뒤 SDK test 후속 수리로
  Rust leaf를 미입장 취소했다(producer143/controller1). Partial을 보존하며 whole rail은 `NOT_RUN`;
  Python793 behavioral pass/Rust191/fresh SDK27 actual 결과는 아직 없다.
  Frozen02bb Contractv3는 Rust build exit0/191collection과 Python793passed·1157.86s를 마쳤다.
  Rust behavior leaf의 resource admission300s timeout/producer124·controller1로 whole `FAILED`이며
  191behavior는 `NOT_RUN`이다. Root 실행의 잘못된 wait env 이름을 확인했다. 다음 실제 변수는
  `QUANTA_INDEX_RESOURCE_WAIT_SECONDS=7200`이다. Partial/source를 유지하며 이후 IPC/manifest/lock과 구분한다.
  OS3v5는 Python-only HEAD drift sourceguard에서65/`BLOCKED`, Cargo/tests0이었다.
  V6 clean24860 원본fixture는 build12m exit0 뒤 tests1383.037s에2/3run1pass1fail/82skip로 종료했다.
  Large4096 cold restart/delete/rank-bit은 통과했고 Medium256은47.687s/default30s Read timeout으로 실패했다.
  XL은 fail-fast로 `NOT_RUN`이다. Host swap35GiB는 관측 사실이며 timeout RCA로 단정하지 않는다.
  Clean82b6af83의 same3counts/seed/caps release/no-fail-fast 요청은 maintenance 후속 수리를 위해
  미입장 안전 취소했다: actual143/812.1525s/0Cargo/tests·`NOT_RUN`, fresh target 생성·컴파일0.
  수정된 clean owner source에서 원본 fixture/caps의 새 release 실행을 등록한다.
  새 source와 fresh output에서 canonical Contract/fresh SDK·portable replay를 다시 실행한다.
- Frozen5796 Contract Python791/Rust191와 fresh release SDK27은 actual 및 독립 portable replay `VERIFIED`.
- 같은 source의 별도 release `scale_matrix` build와 small16/medium256 causal run은 `VERIFIED_DIAGNOSTIC`.
  Large default30s timeout과 XL preflight4M posting 거절은 `FAILED`다.
  Large300s/256MiB는127.612s에 완료한 별도 진단이며 default closure가 아니다.
  Current78d2474 Medium256 OS-child stop/reap/restart 회귀는1passed/82unselected·31.805s로 `VERIFIED`.
  Large/XL OS-child restart·open-loop·quiet-host 성능은 `NOT_RUN`이다.
- ready9 OpenGrok selected-request180은 완료; 전체 service/index 권위는 미완료다.
  Frozen5796 CLI admission20tasks/519pairs만 actual exit0이다. Django admission은 중단했고
  나머지 admissions·pair·full5 join은 완료 증거가 없다. SG/CS 독립 scope는 clean47e checkout에서
  9capture+9replay batch의 BatSG가 stale native runtime receipt(PID1068/Oct4→actual1262/Oct5)로
  거절됐다. Old receipt를 편집하지 않고 현재 runtime native scope를 실제 producer로 새 발행한다.
  Batch 완료 및 final5 join으로 표시하지 않는다.
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

## 조건부 P2 결정

원본 agent-1의 [OCT-04-002](../../adr/OCT-04-002-configuration-and-generation-policy.md)
(effective config/generation policy)와 [OCT-04-003](../../adr/OCT-04-003-source-preparation-sdk.md)
(optional preparation SDK)는 `Proposed`다. 실제 operator 요구 또는 producer fixture 입력 뒤 채택 여부를
결정하며 즉시 구현/API 변경으로 승격하지 않는다. E2의 OG180 selected-project proof는 완료지만
전체12project loaded-reader 권위가 필요하면 별도 witness/consumer 코드가 필요하다.

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
