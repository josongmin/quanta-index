# OCT-04 잔여 작업 인덱스

2026-10-05 완료 구현·결정은 [Accepted ADR](../../../adr/README.md#oct-05-implemented-contracts)에 압축했다.
이 문서가 원본 5개 handoff와 29개 티켓의 **미완료 조건 및 범위 판정의 단일 기준**이다.
[담당·인계](../README.md) · [실행 웨이브](../WAVES.md) · [원본 복구](../../ARCHIVE-INDEX.md#oct-05-handoff-and-ticket-compaction).

현재 요청의 실행 목록은 아래 **Quanta 자체 작업**이다. 원본29개 scope의 owner 판정과
외부 producer 연동 기록은 상세 항목에 보존하며 Quanta 작업 건수·진행률에 합산하지 않는다.

## 현재 코드 잔여

소스 대조 기준일: 2026-10-06. 이 절의 anchor는 날짜 변경에도 유지한다.

Contract/SDK·Ready9의 최종 Quanta 검증 소스는 clean `5658b953690896525b1ae7bde2950adc00adf6b3`
(`oct6-quanta-final-order/quanta-index`)다. `886673cd`에 대한 변경은 manifest의 `cfg(test)`
모듈을 파일 끝으로 옮긴1경로이며 제품 함수·테스트 본문은 byte 동일하다.
Main에 병렬 추가된 NativeIdentityCopy/Admission API3경로와 feature manifest2경로는 이 선택 소스에서 제외했다.
선택 소스의 검증을 전체 main/API qualification으로 승격하지 않는다.
OS3의 release 테스트 등록 수리는 별도 clean `492d2fdccc0fc42ec42e4da19db7833ecbf032ea`에서
검증한다. Delta는 debug 전용 semantic 회귀 모듈에 producer와 같은 cfg 조건을 붙인1줄이며
제품·테스트 본문은 동일하다. 기존565 proof/context를 새 SHA의 결과로 재표기하지 않는다.

### 현재 실행 목록

| 범위 | 담당 | 실제 잔여 |
| --- | --- | --- |
| Manifest 복호화 메모리 경계 | I0 / E4 | 전체 generic CBOR Value를 할당 없는 preflight·bounded collection·개별 행 변환으로 대체했다. Source886의 IPC/CBOR/manifest owner46개가 actual PASS이며565의 whole/strict/integration 소비 검증이 남는다 |
| Workspace 검사·단위 테스트 | Scale / hosted CI | Source886 owner46/46·3.600s와 open-loop bin20/20·13.758s가 실제 exit0다. 테스트 모듈 위치만 바꾼565에서 whole unit `--no-fail-fast`/1thread·strict·runtime/integration을 이어간다. Main2c Python1107과 d192 Python1109 전체는 `VERIFIED`; whole Rust는 아직 실패 상태다 |
| Medium/Large/XL 실제 프로세스 재시작 | I0 | Source565 actual은 release compile E0432·exit101·5227.025s로 `FAILED`, tests0개다. Debug-only semantic hook의 모듈 등록 조건1줄을 수정한492에서 원본256/4096/32768·same seed/caps·release/no-fail-fast3개를 재등록했다. 수정 모듈의 debug 회귀1개도 별도로 실제 등록했으며 두 결과는 미발행이다 |
| Current native Scale·runtime·fuzz | Scale | Matching release Large/XL lifecycle·cost·독립 replay·capacity negative, current runtime 및 선택565의 fuzz4이 필요하다. 다른 소스의 request fuzz 성공을 새 소스에 재표기하지 않는다. Timing/RSS qualification은 독립 판정 |
| Ready9 native·SG/CS·최종5제품 join | Ready9 | Source47e의 fresh native9 scope 실제 leaf는 exit0·9/9 receipts·byte 검증·lease/child cleanup으로 `VERIFIED`. SG/CS9capture+9replay·OG9 replay·final admission/pair/full5는 `NOT_RUN`; matching proof와 명시적 slot handoff 뒤 실행한다 |
| Final Contract/SDK | I0 | 선택565의 canonical Contract·fresh SDK를 실제 등록했다. 두 source closure1500files의 digest가 일치하고 Contract Python793 collection·새 SDK target 준비를 마쳤다. Behavioral 결과/최종 context/portable verify는 미완료이며 canceled731 v6 partial은 재사용하지 않는다 |
| Main의 병렬 NativeIdentity feature 연동 | 해당 변경 owner / hosted CI | Main16f37의 Rust1119·Python1120이 실제 실패했다. Locked unicode-normalization0.1.25에 없는 quanta-native-scratch-v1 feature 참조를 해결한 후 dependency fetch·module/API·whole CI를 재검증해야 한다. 선택565의 엔진 검증과 별도 범위다 |

위 목록은 Quanta 소유 실행 범위다. P11 실제 운영 producer는 I0-03의 target/독립 관측 계약이
선행 입력이며, 외부 provider·승인·Linux 운영·Semantica 연동은 아래 조건부 범위와 구분한다.

### Sidebar 실행 결과 회수

- Source565의 OS3는 release 컴파일 `FAILED`(E0432/exit101·5227.025s)이며 실제 tests0개다.
  `e2e_semantic_budget_interruption`이 debug 전용 `semantic::test_support`를 release에서도
  import한 등록 오류다. 해당 mod에 `cfg(debug_assertions)`를 붙여 producer 경계와 일치시켰고
  main 및 clean492에 적용했다. 제품·회귀 본문 불변, owned fmt/diff는 `VERIFIED`다.
  Source492 원본 release OS3와 debug interruption1개를 정상 admission으로 재등록했으며
  실행 중인565 fresh SDK를 유지한다. Failed565 raw/target은 보존하고 release dependency cache만
  정상 Cargo 검증 아래 재사용한다. 565 release suite 성공이나 전체 main 통과로 소급 표기하지 않는다.
- Main `16f37d43f7a1c3d136d7843442089bdc04561770`의 exact-source hosted CI는 `FAILED`다.
  Python1120은 locked dependency fetch에서 exit101·0.809s로 존재하지 않는
  `unicode-normalization/quanta-native-scratch-v1` feature를 보고했다. Rust1119는 guarded module
  snapshot 검사에서 exit1·4.185s였다. Dependency 해석이 유효해진 뒤 module 출력을 재검증하며,
  실패한 해석 결과로 baseline을 갱신하지 않는다. 선택565의 진행 중 검증 소스는 불변이다.
- Source886의 owner46/46·3.600s와 `open_loop_matrix` full20/20·13.758s는 실제 exit0/lease release로
  `VERIFIED`다. 후자는 실제 runtime smoke·history seal·artifact/CLI 고정 oracle을 포함한다.
  Old controller는 두 leaf 종료 뒤143으로 정리해 broad 자동 실행을 막고 원본 결과를 보존했다.
  Source565는 제품/테스트 본문 불변인 위치 수리만 포함한다. 그 소스의 OS3 release 원본3(위 compile 실패) 및
  Contract/fresh SDK를 신규 output에 등록했으며 두 closure1500files/digest58d62e6c가 일치한다.
  `/private/tmp/qi-scale-os3-release-5658b953-20261006-v8`,
  `/private/tmp/qi-f15-contract-5658b953-20261006-v8`, `/private/tmp/qi-f15-sdk-5658b953-20261006-v8`.
  Contract Python793은 collection일 뿐 behavioral PASS가 아니며 context/portable result는 아직 없다.
  Ready9 final consumer는 PREP-v3의55-binding epoch을 실제 검사해 통과했다. PREP-v2의54-binding
  파일은 audit-only이며 final 입력으로 쓰지 않는다. 기존75controls/3manifests 및 raw는 불변이다.
- Main98의 Rust1111 strict는372.012s/exit101로 동시 추가된 NativeIdentityCopy 테스트의
  `redundant_clone`1건에서 `FAILED`다. 비교값의 `Ok(revision.clone())`만 `Ok(revision)`으로 바꿨고
  제품/API 본문 불변·owned fmt/diff exit0를 확인했다. 이 변경은 선택565에 포함하지 않는다.
  Whole main Rust/API 성공을 뜻하지 않으며 별도 CI 결과를 회수한다.

- Main2c의 hosted Python1107은 전체 job `VERIFIED`:4190passed/30skipped·869.04s,
  P00 positive300passed·34.97s 및 source-bound manifest/pre-commit이 실제 exit0다.
  Main d192 Rust1110은 strict `FAILED`(exit101·350.112s):manifest 테스트 모듈 뒤 제품 item1건이다.
  `bounded_manifest_tests`를 EOF로 이동해 제품·테스트 본문 불변 및 owned fmt/diff exit0를 확인했다.
  이 결과로 현재 소스 Rust 전체 성공을 주장하지 않는다.
- Source47e native leaf는 canonical execution `completed`/exit0·wall9,567.513s이고 native receipt9/9다.
  Resource release·자식 종료·최종 바이트 검사 후 boundary와 원래 controller retirement가 `VERIFIED`다.
  Old whole controller는 native 뒤 중단됐으며 SG/CS capture를 수행하지 않았다.
  새 SG/CS stage controller와 boundary controls의 독립 guard4+4는 actual PASS다.
  Capture/replay와 final5 join은 여전히 `NOT_RUN`이며565 source epoch/75control 재결속은 정적 `VERIFIED`다.

- 후속 통합 기준은 `a84f237ca8d4f3a2a5d0bc24f5f4574e73ae8b1c`와 Scale source
  `74bdc9b493d08da8a708ccfda673f669a1e546ea`의39-path overlay다. 각 pre/post SHA와 patch SHA,
  `git apply --check`·통합 뒤 `git diff --check`·owned Rust30paths rustfmt가 `VERIFIED`다.
  SDK caller/binding·daemon staged publication·streaming digest·bounded CBOR scratch·scale profile은 main에 통합됐다.
  F15 sync3곳의 actual 계측도 main에 반영했다. 영향 Rust/runtime·최종 proof는 아직 실행 완료로 세지 않는다.
- 후속 code checkpoint는 `4cc8f5b94f1a3cca57890d1b4f29687b38cb96cb`다. Digest-fallibility의
  정책 hash 설명 누락을 문서3줄로 수리했으며 함수 body bytes는 동일하다. Gate는19sites/0violations다.
  Large/XL 실제 OS-child restart 회귀2개와 exact ignored 등록을 추가했고 Medium 기본 profile은 유지했다.
  Test-authority·ignored-policy·owned rustfmt가 `VERIFIED`다. 아래 OS3 실제 실패와 후속 재실행을 구분한다.
- Actual 영향 회귀: Scale source74bdc9b4의 `just rust-test-e2e`는214passed/1skipped·301.789s다.
  Fast/risk/DSL/active-selection4binaries 범위이며 전체 runtime suite가 아니다.
  Main 영향 F15 owner nextest는79passed/527skipped·203.351s다. `file_authority::` unit 및
  `f15_file_authority`·`sealed_commitment_cost`·`sealed_manifest`·`unicode_normalization_goldens`를 선택했다.
  Log `/private/tmp/qi-f15-final-owner-nextest-20261006-v1.log`; 전체 lexical/runtime·formal proof로 승격하지 않는다.
- Scale sourcec93aa614의 전체 workspace/all-target/all-feature strict Clippy는 새 코드의
  must-use1건·동일 match arm2건, 재검사source23a52737은 harness의 동일 arm2건·JSON indexing6건에서
  각각 `FAILED`였다. Source23a52737/38e44040의4-path 수정은 patch SHA와 모든 pre/post SHA
  대조 뒤 main에 통합했으며 owned rustfmt가 `VERIFIED`다. JSON profile은 객체/중복 필드를 검사하고
  Result를 caller까지 전파한다. 새 negative1건의 actual 실행은 workspace unit 범위에 남는다.
  재검사38e44040은 upload/harness를 지나 기존 runtime test2paths의 lint5건에서 `FAILED`였다.
  Source2ea1408d의 test-only 수정도 exact guards로 main에 통합했다. 해당 source의 OS3 실제 실행은
  `FAILED`: runtime_extended_suite 컴파일에서 새 SourcePublicationUploadAck match3곳이 누락돼 exit101이었다.
  `/private/tmp/qi-scale-os3-2ea1408d-20261006-v1.log`; 테스트는 실행되지 않았다. 두 helper의 closed match를
  exact guards로 보완하고 unit test의 미선언 anyhow 사용1줄도 기존 Box<dyn Error> 변환으로 수리했다.
  후속 clean owner05bdaeb9/cache의 원본 exact3 실제 실행은 `FAILED`: Medium256(21.737s)·Large4096(315.266s)
  통과, XL32768은102.939s에 IngestResourceBudgetExceeded로 거절됐다. 총439.947s/2passed/1failed/82skipped다.
  `/private/tmp/qi-scale-os3-05bdaeb9-20261006-v2.log`; coverage_pages decode_root의 encoded64MiB/heap256MiB
  envelope가 원인이다. Supported profile·writer preflight·cold decode/reopen·runtime charge의 canonical 계약을 수리한다.
  원본 dispatch 예상 source e56 로그와 실제05 source binding을 보존했으며 fixture/seed를 줄이지 않는다.
  후속e61a31ce9paths를 모든 pre/post SHA로 main에 통합했다. Shared Arc key/row 및 snapshot owner의
  구조 상한을 writer/decoder/resident charge에 공통 적용했다. Encoded64MiB·decode256MiB 한도는 그대로이며
  malformed hint의 최대4096행 임시 예약도 과금한다. `test(coverage)|test(staged_)` owner actual은
  39passed/989skipped·35.965s다. 32,768행 write/reopen/delta, reader 불변성·overflow·초과 root의
  target mutation 전 거절을 포함한다. Python causal profile/capture76은0.18s/exit0였다.
  새 public structural_heap_bytes_bound 때문에 affected API/module 입력과 실제 baseline을 재검증한다.
  이 source의 actual XL/OS3·전체 Clippy·final Contract/SDK는 아직 종료되지 않았다.
  후속5da25296의 structural bound 변환3곳도 explicit branch로 수리·통합했다. 최신 main774594d7의
  hosted verify1767은 새 malformed-reservation test의 panic_in_result_fn1건에서 실패했다.
  세 조건을 explicit Err로 그대로 유지한 test-only 수리를 통합했으며 owned rustfmt/diff가 통과했다.
  OS3v3는 이 후속 수리 적용을 위해 미입장 상태에서 안전 취소해0actual tests/`NOT_RUN`이다.
  API contract 입력과 제품 body는 이 test-only delta의 영향을 받지 않는다.
  Source5da25296의 전체 strict Clippy는2687.131s/exit101로 위 test-only lint1건에서 `FAILED`였다.
  후속 owner24860 actual은 아래 SDK 수리를 통과하고 IPC test-only5건에서 실패했다.
  전체 workspace unit은 앞선 컴파일 실패 뒤 재실행 전 `NOT_RUN`이다.
  Controlled ipc_request_decode는970,826runs/61s/exit0로 `VERIFIED`; 남은 fuzz3은 미입장 취소 뒤 `NOT_RUN`이다.
  Source2ea1408d release scale_matrix build는18m16s/exit0로 `VERIFIED`, native Large/XL 실제 실행은 남는다.
  Main source31828561의 CircleCI verify1741도 동일3건, verify-python1742는 미통합 module baseline에서
  `FAILED`였다. 후속 main e6b1b7e9의 verify1751은 staged-upload test3함수의 strict lint6건에서 exit101,
  verify-python1752는 같은 미통합 module baseline에서 exit1이었다. Test-only 후보를 exact SHA guards로
  main에 통합해 기존 조건을 명시적 Err로 유지했다. 후속080568c7 verify1754는
  active_selection_process_v1의 arithmetic1·panic map_err2에서 exit101이었다. Test-only1path를
  checked predecessor 및 원본 payload 보존형 ThreadPanic으로 수리·통합했고 포맷/guards는 통과했다.
  후속07c5fed9 verify1759는 socket UID 테스트의 singleton iterator1건에서 exit101이었다.
  같은1UID 집합을 std::iter::once로 수리했고 guards/포맷은 통과했다.
  실제 strict Clippy·전체 테스트 재검사는 남는다. Python1753은 source080568c7에서3723passed/30skipped 뒤
  required Python inventory791vs793로 `FAILED`였다. 이는 아래 +2 manifest 수리의 동일 원인이며 새 소스의
  hosted Python 결과는 별도로 확인한다. Python1760은source07c5fed9에서3870passed/30skipped 뒤
  reopen 직접 호출을 전제한 stale source fence로 `FAILED`였다. 실제 reopen→try_reopen_in_place→stop_driver
  위임은 오류를 그대로 전파한다. 정적 검사가 두 경계와 cleanup 전 오류 전파를 계속 확인하도록 수리했다.
  해당6tests는6passed/0.43s로 `VERIFIED`; broad Python/실제 daemon restart로 승격하지 않는다.
- Main47e938d6의 hosted verify1775는 SDK `LargePublicationTransport::send`의
  panic_in_result_fn1·significant_drop_tightening2에서 exit101/`FAILED`였다. SDK cfg(test)1path를
  explicit Protocol Err·checked arithmetic·arm별 짧은 lock·Copy progress snapshot으로 수리했다.
  Identity/offset/1MiB cap/exact body/complete commit/publication binding 조건을 모두 유지한다.
  후속 owner24860의 전체 strict Clippy는 SDK 수리를 지나 IPC integration-test helper의
  unchecked 시간 계산2·불필요한 clone3에서 실패했다. 제품 body 변경 여부를 대조하고 해당 테스트만 수리한다.
  Static AST 감사(변경 Rust68paths/Result 함수275개)는 추가 확정 assertion/panic 위반을 찾지 못했으며
  전체 Clippy 통과로 표시하지 않는다.
  SDK 후속 actual strict는 해당 SDK lint를 통과했으며 위 IPC5건에서 실패했다.
  IPC cfg(test)1path는 checked deadline·Copy 필드 FRU로 수리·통합했다. Production prefix는 byte 동일하며
  guards/rustfmt/diff가 통과했다. 전체 strict/수정 fixture의 actual 검증은 아직 남는다.
  같은 source의 verify-python1776은4181passed/30skipped/2warnings·858.00s 뒤 Ruff format2곳에서
  exit1/`FAILED`였다. 두 파일만 포맷한 뒤 Ruff check/format 및 causal-profile·reopen-fence73tests가
  73passed/0.51s다. 전체 pytest 실행 통과와 hosted job 실패를 구분한다.
  OS3v4 ownerac70ad56는 이 SDK 후속 수리를 위해 미입장 취소했다: exit143/0actual tests/`NOT_RUN`.
- 최신 main7ebaf9d3 hosted verify1073은 Tantivy live manifest의 미사용 `test-log`·`winapi` 선언2개로
  machete 단계 exit1/`FAILED`였다. Vendored Rust 사용처0건을 확인하고 두 선언과 lock의 Tantivy→winapi
  direct edge만 제거했다. Upstream `.orig`와 fs2→winapi transitive는 유지한다.
  `metadata --offline --locked` 및 `just rust-machete` actual exit0로 `VERIFIED`다.
  같은 source verify-python1074는4181passed/30skipped/2warnings·854.52s 및 Ruff442파일을 통과한 뒤
  `cargo-deny`의 search-plane default feature 규칙에서 exit2/`FAILED`였다. 의존성5간선에
  `default-features=false`를 명시하고 소유 manifest의 빈 `default=[]`만 제거했다.
  Runtime dev의 `test-runtime-barriers`는 유지한다. 호출부 수리만으로는 실패했으며 소유 feature 수리 후
  `metadata --offline --locked` 및 `just rust-deny` actual exit0(advisories/bans/licenses/sources)다.
  기존 model2vec-rs license-field warning은 남으며 이 결과를 전체 CI/Rust tests 통과로 표시하지 않는다.
  후속 clean242의 `just rust-policy`15recipes는 actual exit0다. Hosted verify1076의 fmt 및
  `clippy --workspace --all-targets --all-features --locked -- -D warnings`도 actual exit0/373.173s다.
  이는 Linux strict 검사이며 전체 hosted CI·macOS unit/runtime·release/native 종료와 구분한다.
  같은 source242의 verify-python1075는4181passed/30skipped/2warnings·853.25s 및 Ruff442파일,
  rust-policy15recipes·semantic-outcomes3families를 통과한 뒤 maintenance의 `join().is_err()` 분기로
  fallback 규칙 검사에서 exit1/`FAILED`였다. Join 결과를 explicit match로 수리해 취소→sender close→
  handle take→join 순서와 panic/fatal 판정을 유지했다. `just rust-fallbacks`는19checker tests 및
  577scoped files/179parsed candidates에서 actual exit0다. 해당 파일 rustfmt/diff도 통과했다.
  수정된 source의 전체 CI·maintenance actual regression은 별도 검증한다.
  수정 코드 main2fb8e0fe와 clean Scale254a69fe의 crates/bench/Cargo.toml/Cargo.lock/vendor/.config는 동일하다.
  OS3 release 원본3counts/seed/caps와 canonical Contract793/191·fresh SDK27을 새 출력 경로에 등록했다.
  Contract/SDK source는 immutable `oct6-f15-final-v4/quanta-index`의2fb8e0fe이며 각 성공 뒤 portable verify를 실행한다.
  Correct resource wait7200s/jobs1/sccache0/gc0을 사용하며 orchestration 전체에 중첩 lock을 잡지 않는다.
  이 등록은 실제 빌드·테스트 종료와 receipt 발행을 뜻하지 않는다.
  후속 정적 감사에서 단일 native scope의 receipt `verify` 이후 release 최종 검사 누락을 확인했다.
  Batch는 outer 후행 검사로 이 경계를 보호하므로 현재 Ready9 batch의 유효성은 유지한다.
  이 Python producer 수리를 최종 proof source에 포함하도록 Contract/SDK 요청2개만 미입장 안전 취소했다.
  Producer143/controller1·0Cargo/behavior·`NOT_RUN`; v4 source closure/793collection 및 빈 fresh target을 보존했다.
  `/private/tmp/qi-f15-contract-2fb8e0fe-20261006-v4/`와 SDK sibling은 receipt가 발행되지 않았다.
  이 시점의 Rust 입력이 동일한 OS3 release254a 요청은 유지했다. 아래 owner cfg(test) 수리 뒤 해당 대기를 취소하고6385033f로 새 등록했다.
  후속 hosted1076은 nextest exit100/`FAILED`:3568/4174actual 중3567passed/1failed/29skipped,
  fail-fast로606개는 `NOT_RUN`이다. Test380.582s/action1400.687s이며 sole observed failure는
  `open_loop::tests::history_budget_uses_scale_policy_bounds_and_reaches_real_seal`의 오래된 기대값이다.
  Generic harness256MiB와 canonical Scale2GiB를 혼동한 cfg(test)만 수리했다. Fixed2,147,483,648 및
  explicit536,870,912에서 equality 허용/+1거절, zero거절 및 real one-byte seal typed거절을 유지한다.
  Production prefix byte 동일·rustfmt·fallback19/577·diff는 `VERIFIED`; 수정 fixture actual은 남는다.
  Main2fb hosted1080 strict Clippy도 exit0/407.888s이며 이후 cfg(test) 수리의 whole actual을 뜻하지 않는다.
  OS3 release254a 요청은 owner 수리 적용을 위해 미입장 취소했다:143/624.7525s/0Cargo/tests·`NOT_RUN`.
  Clean6385033f의 same3 fixture release/no-fail-fast를 새 cache namespace로 다시 등록했으며 actual 종료 전이다.
  Source2fb의 verify-python1079는 Python4181passed/30skipped/2warnings·869.96s와
  PM5/Ruff442/rust-policy/semantic/fallback/benchmark/Semgrep를 통과했다(step924.701s/exit0).
  후속 P00 단계의 registry25 및300tests·35.23s도 통과했지만 terminal writer 직접 호출의
  `ModuleNotFoundError: tools`로 job 전체는 exit1/`FAILED`였다. Direct-script 모드에만 resolved repository
  root를 import 경로에 먼저 등록하도록 수리했으며 payload 생성 본문은 동일하다.
  Isolated direct CLI·module CLI·실제 args의 observed-host 거절을 포함한 `test_proof_host.py`28개는
  main actual28passed/0.68s, Ruff check/format 및 diff 통과다. 실제 CircleCI host의 manifest 발행은 남는다.
  후속 source593의 Python1090도4187passed/30skipped/2warnings·880.73s 및 policy step935.449s/exit0,
  P00 300passed/34.62s 뒤 같은 미수리 direct import에서 job exit1이었다. 새 source731의
  hosted1094 strict Clippy는367.310s/exit0다. Hosted1093 Python·1094 Rust 전체 job은 진행 중이며
  이 부분 결과를 전체 CI 종료로 승격하지 않는다.
  Source731 Python1093은4190passed/30skipped/2warnings·851.53s 및 policy step903.306s/exit0,
  P00 300passed/33.50s와 실제 manifest 발행·source binding 검사까지 통과했다(step39.491s/exit0).
  전체 job은 이후 pre-commit에서 README의 날짜 포함 anchor1건과 vendored JSON2개 EOF 수정으로
  exit1/`FAILED`였다. Anchor를 날짜와 독립적인 현재 코드 절로 고정하고 JSON payload 변경 없이 EOF만
  정규화했다. EOF hook 재검사·doc-path/anchor lint·PM5·diff는 actual exit0다. 새 whole CI는 별도 판정한다.
- 후속 코드 감사·수리: source593 Rust1089는 strict367.437s/deps845.448s를 통과했지만 nextest는
  3571passed/1failed/29skipped·381.070s,602미실행으로 `FAILED`였다. Sole observed failure는
  `open_loop_artifact_binds_requested_and_effective_history`의 오래된16MiB default gold다.
  Mainf3ae4711의 cfg(test)2paths에서 artifact·CLI default를 independent fixed1GiB/2GiB 및
  `scale-supported-v1`로 수리하고 explicit-pair/default-total 조합도 fixed2GiB로 대조했다.
  두 production prefix는 byte 동일하다. 정적 caller/artifact/causal-profile 감사에서는 추가 history 불일치를
  찾지 못했다. Owned fmt/diff는 `VERIFIED`; 이 fixture의 실제 Rust 실행은 아직 `NOT_RUN`이다.
  Mainba06fc12에는 staged upload의 preflight·DTO decode가 실제 request cancellation을 8KiB 단위로
  확인하고 CBOR I/O 오류 포장 뒤에도 원래 typed cancellation/deadline을 유지하는 수리를 통합했다.
  Small/large body·buffer-bypass·pre-open expired-budget 회귀를 추가했다. 해당 source의 hosted1104는
  새 doc 첫 문단 길이와 Option match2건에서 strict exit101/87.747s로 `FAILED`였다.
  Main451908e5에서 짧은 첫 문단·`map_or(otherwise, identity)`로 원래 오류 선택을 유지해 수리했다.
  Owned fmt/diff 및 `just rust-fallbacks`19tests/577files/179parsed는 actual exit0다.
  Manifest3paths는 최종 patch6c039748의 모든 pre/post SHA·apply guards 뒤 main에 통합했다.
  전체 generic Value 대신 할당 없는 collection-count/행별10,280node preflight와 bounded streaming을
  사용하며 개별 scalar/행만 이전 Value 변환으로 처리한다. Exact outer visitor는 indefinite 종료 break와
  EOF/extra field를 검사한다. Body tag·array-only digest·BIGPOS version 의미를 유지하며 sealed16MiB와
  text2,097,152shards/268,435,520encoded bytes의 기존 지원 한도 및 writer format은 유지한다.
  BIGPOS/BIGNEG body와 leading BIGPOS version의 payload는16byte까지 허용한다. 그보다 긴
  leading-zero 비정규형은 할당 전 거절하며 canonical writer는 이를 발행하지 않는다.
  Compact giant count·nested fanout·tagged positive/negative·indefinite 회귀를 추가했고
  실제 Rust compile/test/Clippy는 아직 `NOT_RUN`이다.
  이 Rust delta를 포함한 source를 한 번 고정하기 위해 root OS3release638와 Contract/SDKv6의 자기
  waiting-only leaf3개를 source/argv/parent/no-child/stopped-state guards 뒤 취소했다. OS3는
  actual143/3184.926s/0Cargo·tests이며 Contract/SDK도 producer143/controller1/0behavior·receipt0다.
  V6 source closure/793 Python collection·빈 fresh target 및 모든 원본 로그를 보존했다.
  당시 native receipt7/9 관측 뒤 native leaf9/9가 완료됐다(위 최신 checkpoint). SG/CS capture와
  final join 완료로 승격하지 않는다.
- Ready9 final 준비의 원본90 inputs·admission73·Bat extended105·OG raw39,669파일과
  Python10-role/Java/JAR 결속 감사 및 guard9/9가 `VERIFIED`다. Relocated source107 helper3개의
  삭제된 old path 참조를 외부 guarded 후보에서 고쳤다. `/private/tmp/qi-ready9-final-static-20261006-v1/RESULT.md`.
  Actual admission/pair/full5는 새 selected-source proof·runtime·host slot 전달 전 `NOT_RUN`이다.
  SG/CS는 Quanta proof를 요구하지 않는 독립 scope로 clean47e938d6 checkout에서
  9capture+9independent replay batch의 Bat SG는 stale native runtime binding에서 `FAILED`였다.
  Old receipt PID1068/Oct4와 실제 PID1262/Oct5가 달라 현재 runtime의 native scope를 실제 producer로
  새 발행·독립 검증한다. 기존 receipt/raw는 유지하며 전체 batch 완료 및 final5 join은 아직 `NOT_RUN`이다.
  Actual v4는 Bat native79payload와 scope receipt 발행·검증을 완료하고 cli scope로 진입했다.
  Producer의 scope당4회 full-release 재해시(1.3GB/53,950files) 중복을 확인했다.
  Caller/callee의 release 변경 감지 경계를 유지하는 후보와 회귀를 별도로 준비하며 live 실행 소스는 유지한다.
  후속 native scope 수리는 main에 통합됐다. Callee의 release 소비 전·native 뒤/receipt 전 검사를 유지하고
  verify 직후 반환 전 전체 release 검사를 추가했다. Batch의 중복 outer2검사만 제거해 scope당4→3회이며
  batch spec/control 검사는 유지한다. 원본에서 verify 중 selected/unselected 파일 변조2회귀는 실제 실패하고
  후보에서 통과했다. Main 관련 파일 전체는171passed/1004.84s, Ruff/diff 및 exactpostSHA는 `VERIFIED`다.
  기존47e live batch/raw/source는 유지한다. 배치 재해시 감소를 실제 latency 향상으로 표시하지 않는다.
  Native scope·P00 수리를 포함한 clean731a39cf를 `oct6-f15-final-v6/quanta-index`에 고정해
  canonical Contract793/191·fresh SDK27/성공 뒤 portable verify를 실제 등록했다. Outputs는 각각
  `/private/tmp/qi-f15-contract-731a39cf-20261006-v6/`와 SDK sibling이며 actual terminal/context는 남는다.
- Public API 담당의 actual contract/SDK API rendering과 contract/core module gates4개는 reviewed 후보와 byte-exact로
  `VERIFIED`다. SDK public API는 기존 baseline과 동일하다. 독립 external consumer5tests는5passed,
  접근 차단4compile은 기대한 E0004/E0603 거절로 모두 `VERIFIED`다. API255/module194 input guards와
  actual output SHA·baseline pre/post SHA를 대조해 baseline3paths를 main에 통합했다.
  `/private/tmp/qi-api-vkq0qc7t/owner-result.md`가 actual command/selector를 소유한다. Standalone consumer serde1.0.229와
  producer1.0.228의 lock 차이를 보존하며, daemon/UDS·full SDK·workspace·release proof로 승격하지 않는다.
  Coverage structural bound의 추가 public helper는 별도 contractAPI actual exit0로 reviewed 후보와 일치했다.
  Affected contract module도 actual exit0/기존 baseline byte 동일이다. Reviewed1helper baseline을
  현재 input254/255+SDKcfgtest예외1 및 module194/194/output SHA로 대조해 main에 통합했다.
  새 external consumer2는 nightly-2026-08-01의 `--locked --offline` actual2passed/exit0로 완료됐다.
  Main242 기준 API252/255/module193/194이며 SDK/IPC cfg(test)2개와 root lock을 별도로 판정했다.
  Producer의 보수적114패키지 projection은 동일하고 consumer42패키지에는 Tantivy 및 변경 상위 manifest5개가 없다.
  One-helper API·module·consumer2 범위는 `VERIFIED`; 기존 SDK API/consumer5/negative4를 새 source로 재실행한 것으로 세지 않는다.
- Hosted CircleCI source7fb46415는 `FAILED`다. Verify1730은 upload 코드의 rustfmt drift에서 exit1,
  verify-python1729는 contract/core guarded module tree의 새 upload DTO/port 및 sibling byte wrapper
  visibility baseline 누락에서 exit1이었다. 포맷은 Scale, 두 module baseline과 공개 API는 API 담당이 소유한다.
  이 source의 Clippy·뒤쪽 Python gates는 앞 단계 실패로 실행되지 않았다. 이후 수리와 baseline 통합 뒤 새 hosted CI를 확인한다.
- 이번 통합 직전 main은 clean `e6b1b7e9703fbace81c03a8b59b3168cf3715673`였다. 새 bounded source-upload
  코드가 추가됐으며 이전 Clippy/598 lexical/Frozen5796 proof를 그 source의 최종 증거로 승격하지 않는다.
- Ready9 OG는 source63ac에서9/9 capture·독립 `live_lexical_external.py --verify`가 `VERIFIED`다.
  각20tasks/총180tasks,18개 실행 exit0. 결과는 `/private/tmp/qi-parallel-ready9-20261006-v1/actual-run-v1/result.json`.
  Quanta final Contract/SDK·SG/CS·final pair/full5 join·all-project authority·holdout는 이 결과에 포함되지 않는다.
- Scanner는 sourcef2dfe089에서 두 fresh build/capture와 custody verify·canonical/independent parity가
  `VERIFIED`다. Fixed338queries/79files,2,030completed responses. Explicit `allow-incomplete` lexical-file
  profile이며6 ParseFailed facts를 보존했다. 원래 `require-complete`는 `FAILED`; 정식 speed/adoption은 `NOT_RUN`.
  Source/producer resource-control/byte-span negative 수정3paths는 SHA guard 대조 후 main에 통합했다.
  Focused55tests/29.26s는 해당 owner snapshot 결과다. `/private/tmp/qis.utp62qk5/owner-result.md`가 상세 scope를 소유한다.
- Scale의 source898d2dfb 고정 실행은 threshold2/owner36/CLI2/Clippy/release build가 `VERIFIED`이고
  Large·XL default native는 `FAILED`다. Large required81,764,348B가 pair16,777,216B를 초과했고,
  XL decoded request385,260,565B가 cap134,217,728B를 초과했다. 별도 Large explicit diagnostic lifecycle/replay는
  `VERIFIED`; native XL lifecycle은 wire 거부 이후 `NOT_RUN`이다. `/private/tmp/qi-scale-f15-20261006-v1/result.json`.
  Scale 채팅은 그 후 새 source-upload 경로의 SDK/daemon 회귀와 XL actual을 진행 중이며 아직 완료로 표시하지 않는다.
- 최종 Contract 첫 실행source080568c7은 Python collection 단계에서 `FAILED`였다. 실제793개와 필수791개가
  달랐으며 기존 필수 검사는 누락되지 않았다. 최근 admission split batch의 drift/boundary 회귀2ID를
  필수 manifest에 명시 추가했고 Rust/SDK 목록은 바꾸지 않았다. Exact793 수집과 새 회귀 각각 누락 거절을
  확인했다. `/private/tmp/qi-f15-contract-080568c7-20261006-v1`는 실패로 보존하며 Rust tests는 미실행이다.
  후속 clean47e938d6의 canonical Contract는 source closure와793개 collection authority 확인을 통과했다.
  SDK cfg(test) 후속 수리로 Rust build leaf를 미입장 취소해 producer exit143/controller exit1이다.
  `/private/tmp/qi-f15-contract-47e938d6-20261006-v2`는 partial로 보존하며 whole rail은 `NOT_RUN`이다.
  Collection은 Python793 behavioral pass가 아니며 Rust191과 fresh SDK27도 이 source에서 미실행이다.
  SDK 수리 후 frozen02bb8472 Contractv3는 Rust build exit0·191개 collection과 Python793passed/1157.86s를
  마쳤다. Rust behavior leaf는 resource admission300s 만료/producer124·controller1로 미실행이며
  whole rail은 `FAILED`(실행 입장 timeout)다. Root가 wait env 이름을 잘못 지정한 실행 설정 오류를
  확인했으며 다음 실행은 실제 `QUANTA_INDEX_RESOURCE_WAIT_SECONDS=7200`을 사용한다.
  Partial output/원본 source를 유지하고 이후 IPC/manifest/lock 수리와 최종 source scope를 구분한다.
  OS3v5는446→24860의 Python-only HEAD 이동을 sourceguard가 감지해 exit65/`BLOCKED`였다.
  Cargo/tests0이며 그 결과를 보존한다. V6 clean24860은 build12m exit0 뒤 actual2/3run:
  Large4096 cold restart/delete/rank-bit 비교는 `VERIFIED`, Medium256은47.687s에 기본30s Read timeout으로
  `FAILED`, XL32768은 fail-fast로 `NOT_RUN`이다. Tests1383.037s/producer100·controller1 원본을 보존한다.
  Host swap35GiB 관측만으로 timeout 원인을 단정하지 않는다. Clean82b6af83의 원본3counts/seed/caps
  release/no-fail-fast 요청은 maintenance 후속 수리를 위해 미입장 상태에서 안전 취소했다.
  Actual exit143/812.1525s·0Cargo/tests·`NOT_RUN`; 새 release target은 생성·컴파일되지 않았다.
  `/private/tmp/qi-scale-os3-release-82b6af83-20261006-v1/`에 결과를 보존한다.
  수정된 clean owner source에서 원본 fixture와 cap을 유지해 새 release 실행을 등록한다.
  새 source의 Contract/fresh SDK 실제 발행·독립 portable verify가 남는다.
- 남은 통합 종료 조건: 새 upload/Scale 및 scanner owner 변경을 포함한 영향 Rust/runtime 회귀,
  최종 selected-source Contract/fresh SDK·portable replay, Ready9 final pair/full5 join과 각 별도 qualification.

### Sidebar 배정 시 구현 기준

현재 정리 기준: sidebar 작업 배정 시 clean Quanta source `20c3ae60b257892bceba84c8380e7a7e25c4b64a`.
F15 cfg(test)/Scanner fixture 보완도 이 source에 통합됐다.
F15 strict 경계 검사/shared-limit/scanner comparator 수정은 이 source에 통합됐다.
Frozen5796 이후 main에는 문서·Justfile/CI/R5 tooling·OS-process 회귀·scanner custody·ARB 용어 예산 수리가 추가됐다.
Main에는 scale/open-loop pair/total retention 정책 결속, invocation-scoped 입장 검증 재사용과
전체 file-pair verdict를 사용하는 테스트 fixture가 추가됐다. F15 immutable pack/root/posting,
bounded query reader와 기존 비용/format 테스트 전환·제품 회귀도 main에 통합됐다.
Seal별 exact product-retention bytes 계측과 canonical fixture는 반영됐다.
Source0b의 lexical 전체 회귀는598passed/8skipped·702.173s였으며, 이후 strict 경계 검사·공유 posting 한도
수정 overlay의 영향 회귀는 재검증 대상이다. All-target Clippy는 아래 v7 결과이며 Large/XL 재실행·새 formal proof는 `NOT_RUN`이다.
Clippy v6 lexical lib-test36건을 세 에이전트와 I0가 경로별 수정해 통합했고 v7은exit0/322.077s로 통과했다.
`CARGO_BUILD_JOBS=1 QUANTA_INDEX_SCCACHE=0 QUANTA_INDEX_TARGET_GC=0 ./scripts/cargow --lane test-f15-owner-lane clippy -p quanta-index-lexical -p quanta-index-searchd-harness --all-targets --all-features --locked -- -D warnings`
범위만 `VERIFIED`이며 최신 Rust actual 회귀/Contract/SDK proof와 구분한다.
Scanner comparator17cases는10.91s/exit0로 통과했다. 각 negative가 유효한 baseline/candidate를
먼저 비교한 뒤 자기 mutant를 거부하도록 보완했다. Actual two-arm A/B는 `NOT_RUN`이다.
병렬 소유 경로와 선행은 [병렬 작업 배정](../WAVES.md#병렬-작업-배정--2026-10-06)에 정리했다.
Scale·Scanner·Ready9 OG 담당의 독립 작업서를 준비했다. OG9 specs는 별도 clean63ac checkout에서
원본90 inputs·Python10-role/runtime을 결속한 PREPARE 시점에는 actual capture가 `NOT_RUN`이었다.
이후 source63ac의 OG9/180 capture·독립 replay 및 sourcef2d의 Scanner diagnostic A/B는 위 회수 결과대로 완료됐다.
OG 단독과 선택한 clean BASE의 Scanner A/B 진단은 최종 SDK proof를 전역 선행으로 두지 않는다.
Frozen5796 actual과 아래 current-source owner 회귀를 구분하며 main formal proof로 승격하지 않는다.
Git `52980f58`의 29개 ticket ID와 현재29개 고유 heading은 누락·중복 없이 일치한다.
아래는 실제 생산 경로·코드와 기존 결과를 대조한 잔여이며 새 full-suite/release qualification이 아니다.

### Quanta에서 할 작업

| 우선·종류 | 할 작업 | 종료 조건·owner |
| --- | --- | --- |
| P0 · 검증 | 선택한 최신 source의 Contract/SDK·영향 runtime/public API/wire gates·hosted CI 실행 | Matching source/binary와 actual selected 결과. Frozen5796의791/191/27을 새 HEAD 결과로 바꾸지 않는다. [I0-02](#o4-i0-02) |
| P1 · 검증 | Large4,096/XL32,768 원래 fixture의 지원 profile·typed refusal 검증; open-loop·OS restart 실행 | F15 durable authority와 bounded staged publication 수리는 통합됐다. 새 scale-supported-v1은 별도 명시적 용량 계약이며 기존30s/16MiB 성공을 뜻하지 않는다. Matching source·retained bytes·elapsed·RSS·초과 한도 거절을 확인한다. [E4-05](#o4-e4-05) |
| P1 · 계측→필요 시 수리 | Full/delta/delete/no-op/reopen 전체 읽기·CPU·metadata·IO·memory/segment 누적 비용 분해 | F15 변경 bucket 생산·cold 전수 검증·selected posting read, retained delete bitmap 및 NoMerge fanout 비용을 actual profile로 판정. Native segment 재사용 구현은 완료다. [E4-01](#o4-e4-01) |
| P1 · 성능 판정 | 완료한 Scanner diagnostic A/B의 범위 판정·Semble phase 비용·1,196-row bootstrap full caller·정식 반복 성능 | Scanner fixed338 diagnostic parity는 완료다. 유지/철회와 qualified speed는 사전 acceptance·지속 host 관측 및 최소5 fresh roots/route1,000 warm observations로 별도 판정. [E4-03](#o4-e4-03), [E4-06](#o4-e4-06), [E2-05](#o4-e2-05), [E1-07](#o4-e1-07) |
| P1 · 평가 실행 | SQL146/Zellij476·신규742pairs 판단, Tailscale rubric, admissions·required cells·5제품 capture/replay/join·최종 scores | AI quota/rubric 입력은 해당 범위만 `BLOCKED`. Ready cells는 별도로 실행한다. Exact/prefix/infix/components/default·explicit typo/no-answer/NL/ARB/B09 분모와 외부 index scope를 유지한다. [E1](#e1), [E2](#e2) |
| P1 · 독립 평가 | 독립 holdout/license/exposure/gold 발행과 지원 declaration-name/span·typo 평가 | 실제 미사용 source/query family 및 source-attested gold, critical strata/underfill·supported unit 판정. File hit를 선언 회수로 세지 않는다. [E1-04](#o4-e1-04), [E1-05](#o4-e1-05) |
| P2 · 운영 코드·릴리스 | Quanta typed deploy/activate/restore-forward producer·recipes·parser/checker/aggregate 연결; 실제 provider 및 installed Linux daemon/state/운영 실행 | 독립 pre/post 관측·actual host/config/state/retention/rollback 입력 뒤 구현·실행. 현재 운영 생산자는 미구현이고 대상 입력은 `BLOCKED`다. 기존 staged pair는 별도 연동 owner가 소비한다. [I0-03](#o4-i0-03) |

조건부 P2 결정도 원본 agent-1의 출처대로 보존한다.
[OCT-04-002](../../../adr/OCT-04-002-configuration-and-generation-policy.md)의 effective config/generation policy는
실제 operator 요구, [OCT-04-003](../../../adr/OCT-04-003-source-preparation-sdk.md)의 선택적 preparation SDK는
실제 producer fixture가 있을 때 결정한다. 두 ADR은 현재 `Proposed`; 즉시 구현이나 승인된 public API로 세지 않는다.

확정된 Quanta 작업은 실패한 capacity/durable source authority 수리와 운영 결과 생산자다.
Full sync 병목은 아래 actual로 확인했으며 delta/noop의 exclusive residual 원인은 추가 owner 계측이 필요하다.
Bootstrap 추가 최적화, persistent token authority, 검색 정책 변경은
병목·독립 gold·사전 계약이 성립할 때만 채택한다. Durable barrier 수리는 pack/root crash custody와 함께 검증한다.
E3의 selection·7-route SDK binding·slow disk·timeout replay·operator 구현을 다시 만드는 작업은 없다.

Quanta 실행 순서: source 영향 확인/검증 → matching capacity·cost·A/B → 확인된 원인 수리 및 영향 회귀
→ ready admissions/captures·독립 채점 → holdout/정식 성능·릴리스의 각 입력별 qualification.
실제 실행은 host별 직렬이며 AI/holdout/Linux 입력 대기가 준비된 다른 작업을 막지 않는다.

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

- 검색 평가용 코드 저장소의 최종 AI 정답 판정: SQLAlchemy 잔여146pairs와 Zellij476pairs를 완료한다.
  각 pair는 검색 질의와 후보 코드의 관련도 판단 단위다.
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

P0 · W3 · admission consumer 구현 완료. product0e6의9repo 발행은 historical;
frozen5796 ready9 재발행 및 나머지3repo·후속 final revisions 미완료.

- SQLAlchemy/Zellij/Tailscale 및 새 merged labels를 canonical suite/pack/split/license/review/proof에
  연결한다. repository별 실제 source/runtime과 I0-02 matching proof 이후 admission ISSUE.
- frozen product0e6의 bat 포함9repo/180tasks/4,262judgments admission은 그 범위로 유지한다.
  원본 source107이나 원 review-validator revision을 current product revision으로 재명명하지 않는다.
- clean5796 Contract791/Rust191/SDK27은 [I0-02](#o4-i0-02)의 완료 proof다.
  외부 Bat 재발행 후보는 원본358-pair review pack으로 typed forms/custody를 재생한 뒤
  merged409-pair qrels/suite/pack의 고정 byte 일치를 요구한다. Historical v3 actual은5passed/2failed·22.07s:
  409쌍 재생·두 typed alias 거절·잘못된 merged pack 거절은 통과했고,
  label/product source guard2개는 기록된107 checkout 경로 부재로 실패했다.
  후속 v4에서 동일107 source checkout을 복구·신원 대조하고 새 input으로 실행했다.
  Old raw/packet은 변경하지 않았다.
  이 외부 후보·other8 준비는 actual admission ISSUE가 아니다.
- 후속 외부 v4 재생 `VERIFIED`: clean5796 product와 새 clean107 label checkout을 명시해
  `QI_CURRENT_SOURCE_ROOT=/Users/songmin/.codex/worktrees/oct5-f14-qualified-5796/quanta-index QI_LABEL_SOURCE_ROOT=/Users/songmin/.codex/worktrees/oct5-label-source-107/quanta-index uv run --project /Users/songmin/Documents/code-new/quanta-index --frozen --extra dev python -m pytest -q -o addopts='' /private/tmp/qi-e2-ready9-full5-current-prep-v4/test_bat409_reissue.py`
  →8passed·10.87s/exit0였다. 원본 packet의107 revision·원래 경로·5개 helper SHA와
  새 checkout의 clean HEAD·실제5개 bytes를 검증했다. Label 원본 경로와 검증용 경로를
  새 lineage에 각각 기록하며 원본 packet/raw는 수정하지 않는다. 409쌍 재AI 호출은 없다.
  후속 actual other8 ISSUE 중 CLI는 `VERIFIED`:20tasks/519pairs, wrong-source refusal1,
  source5796·qualified:false·human_provenance_attested:false다. 명령은
  `uv run --project /Users/songmin/Documents/code-new/quanta-index --frozen --extra dev python /private/tmp/qi-e2-ready9-full5-current-prep-v4/reissue-other-eight.py --input /private/tmp/qi-e2-ready9-full5-5796-specs-20261005-v1/admission-other-eight-packet.json --output-parent /private/tmp/qi-e1-other8-5796-20261005-v1 --selection other-eight`.
  CLI cell934.437s의 결과는 같은 parent의 `cli/cli/result.json`에 있다.
  Batch는 Django의 release reconstruction 단계에서 root가 우선순위를 바꾸며 SIGINT/exit130으로
  중단했다. 전체 summary는 없고 Django partial은 완료로 세지 않는다. 원본 root는 보존한다.
  외부 v5의 명시적 remaining7 selector는 준비됐으며 fresh source/input/proof 결속 후 새 root로
  실행한다. Bat ISSUE·남은7·ready9 제품 실행은 미완료다. AI 재판단 결과가 아니다.
- B08이 계속 요구하는 C5 stale4 suites는 manifest/query/source를 재확인해 reissue 또는 명시적
  exclusion을 발행한다. B09 global12와 합치거나 NL-only diagnostic을 mixed-track decision으로 승격하지 않는다.
- 완료: 각 ready repository의 정확한 admission inputs/result 및 변경 labels의 새 revision.
  threshold/query/grade/family/unit/source/runtime/proof/license 불일치는 발행 거절이다.

### O4-E1-04

P1 · W1/W4 · name-span 구현 완료, 다른 지원 name/typo cells와 최신 source 영향 미판정.

- Gin exact1,196 symbol/name capture/scoring(product0e6/driverb55)은 해당 historical scope의 완료다.
  변경된 source의 최신 qualification에는 영향 capture/scoring을 새로 실행한다.
  다른 지원 unit의 선언 ID/name bytes/span을 independent source oracle와 native selected unit으로 평가한다.
- 후속 frozen5796 Gin fresh 진단 `VERIFIED`: canonical `source_oracle_suite.py`의 complete Go
  declaration oracle → `run.py quanta --spec /private/tmp/qi-gin1196-f14-5796-spec.json` →
  `evaluator.py evaluate-diagnostic`를 실행했다. Capture `/private/tmp/qn1196f14`, report
  `/private/tmp/qi-gin1196-f14-5796-name-report.json`:1,196rows/1,192success/4capped,
  선언 및 declaration-name recovery MRR@10=1.0, Recall@10=0.9985493335876968, coverage=1.0.
  `diagnostic_unqualified`/single-route이며 current main·independent holdout·제품 비교·PERF proof가 아니다.
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

- 두 Gin actual의1,196 task/query/category/family와 기록된 input hash는 일치했다. 두 root 모두
  Quanta symbol 단일 route이며 `evaluate-diagnostic`은 paired bootstrap caller를 호출하지 않는다.
  단일 route profile과 paired caller 비용을 구분한다. Matching 두 번째 capture와 caller inputs 없이
  self-comparison·합성 row로 paired 병목을 판정하지 않는다.
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

P1 · W1/W4 · selected-project native reader/capture/replay 구현 완료, 전체 서비스 witness는 별도 코드·판정 범위.

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
  결속했다. 해당 실행 시점의 bat/cli/django는 발행 suite/pack과 일치하는3repo/60requests 범위였으며,
  selected-request attested만true이고 전체 서비스 reader/비교 qualification은 미완료다.
- `VERIFIED`: lo20 capture와 별도 프로세스 replay도 exit0였다.
  `/private/tmp/qi-e2-og-query-reader-ready9-lo20-20261005-v1`의 capture SHA는
  `6bf35fe3203ef504f68141e5b59297353426f824cd18023e7dc7a671a4a5500e`다.
  Native12repo/17,615live,20개 요청의 segments_4/generation4/readerVersion16/live158/max159가 일치했다.
  해당 실행 시점에 동일 발행 입력의 bat/cli/django/lo4repo/80requests를 capture·독립 replay했다.
  이후 ready9/180 완료는 아래에 기록하며 전체 서비스/비교 qualification은 미완료다.
- `VERIFIED`: mocha20 capture와 별도 프로세스 replay도 exit0였다.
  `/private/tmp/qi-e2-og-query-reader-ready9-mocha20-20261005-v1`의 capture SHA는
  `1f3fe484dcb9acdbf7a2587b03b5f05a2e22cb80ca2813f2790d94df6289c4e2`다.
  Native12repo/17,615live,20개 요청의 segments_4/generation4/readerVersion16/live561/max562가 일치했다.
  해당 실행 시점의 ready5repo/100requests에서 selected-request attested만true였다.
  이후 ready9/180 완료는 아래에 기록하며 전체 서비스 reader/비교 qualification은 미완료다.
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
- Canonical witness는 `instrumented_search_requests_selected_project_only`이며 all-project/global/loaded-reader
  flags를false로 고정한다. 같은 selected-request capture를 다시 실행해 전체12project 권위로 승격할 수 없다.
  전체 서비스가 종료 조건이면 별도 loaded-reader witness/consumer를 구현·검증하고, selected-project만
  사용하는 비교라면 caller가 요구하는 scope를 명시 판정한다. 현재180requests는 이 추가 코드의 증거가 아니다.
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
- frozen5796 ready9의 Quanta/Semble pair·Sourcegraph/CS capture·새 full5 join은 `NOT_RUN`이다.
  기존 OpenGrok ready9/180 selected-request 증거와 Sourcegraph scope receipts는
  동일 input·producer bytes·현재 service/native scope에 대한 canonical replay를 통과해야 재사용한다.
  원본 정답 재검수와 새 제품 실행을 구분하며 source97 raw를5796 raw로 재표기하지 않는다.
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
- F14 frozen5796은 unchanged source까지 seal에서 읽고 hash/fold하며 cold에서 두 전역
  `TrigramIndex`를 구성한다. 현재 F15는 변경 bucket만 재작성하고 immutable base
  commitment를 계승한다. Cold open은 독립 전수 검증 후 bounded term/offset/hash directory를
  적재하고 query에서 필요한 posting 범위만 읽는다. Cold 전수 검증 비용은 그대로 측정 대상이며
  producer admission/replay 계수는 물리 I/O 계수가 아니다. 실제 성능·RSS는 아직 `NOT_RUN`이다.
- Seal별 product-retention gauge를 full/delta/no-op/delete에 결속하는 계측을 통합했다.
  Serving pair1개의 고유 inode regular-file `st_size`이며 pair=total이다. `st_blocks` 점유량과
  다른 관측이고 재시작 뒤 gauge0을 복구된 exact bytes로 세지 않는다. 실제 tier 실행은 `NOT_RUN`이다.
- Frozen5796 Large 명시적 진단 `VERIFIED_DIAGNOSTIC`: 아래 E4-05 root에서 total127.612s,
  full seal68.733s와 explicit sync49.778s(약72.4%)를 관측했다. Atomic file4,367회/25.108s,
  atomic parent4,367회/24.646s다. Source4,096개 각각의 file/parent barrier가 생산 경로에 있다.
  Delta seal12.711s/sync0.265s, noop14.733s/0.153s, delete14.393s/0.205s로
  full sync와 다른 residual이다. 전수 읽기·hash/metadata 비용은 source에서 확인했으나
  stage별 exclusive 시간은 아직 `NOT_RUN`이며 잔여 시간을 해당 원인으로 단정하지 않는다.
  Delta logical growth83.252MB/changed source2,160bytes, sampled RSS538,050,560bytes 및
  logical/source11.98배는 physical I/O·true peak·전체 비용 qualification이 아니다.
- 완료: 명시적 clock/resource domain과 source-bound 결과로 주요 residual의 실제 원인을 설명한다.

### O4-E4-02

P1 · W2 · full sync 병목 actual 확인; F15 pack/root publication 통합, actual 회귀 진행 중.

E4-01에서 source별 file/parent barrier의 full sync 비용을 확인했다. Parent sync만 묶으면
file sync25.108s와 다른 work가 남는다. F15는 bounded source/posting objects의 durable write와
directory sync 뒤 root를 atomic durable 발행하고 sealed manifest를 마지막에 발행한다.
Source32,768/raw128MiB/memberships20M 및 pack/posting/directory/resident/query work admission은
논리 한도이며 XL 실측 capacity·RSS 상한의 증거가 아니다. Delta/new-object replay와 normal
admission 계수를 분리했고 fresh/delta의 inherited object 재전수 해시를 제거했다.
File sync/rename/hardlink/directory/root publish/
cleanup cut별 fault와 crash/reopen에서 old 또는 완전한 new root·참조 file/digest를 검증한다.
Barrier 실패 후 seal/activate를 거절하고 inherited page custody를 유지한다.
Power-loss 범위는 별도 실제 storage proof가 없으면 `NOT_RUN`이다.

### O4-E4-03

P1 · W1/W4 · scanner A/B source-bound diagnostic 완료, qualified 유지/철회 판정 미완료.

- Sourcef2dfe089의 fixed338queries/79files 두 fresh release arms에서 capture/custody와
  canonical·independent whole-call parity가 `VERIFIED`다. 총2,030responses(arm별 cold1/warmup338/measured676).
  Explicit allow-incomplete lexical-file profile이며 ParseFailed6facts를 숨기지 않았다.
  Unicode control median32.3997085ms, ASCII candidate37.1435625ms, paired relative median+12.58468%였다.
  Shared host·순차2repetitions의 diagnostic이며 일반 speed/adoption 판정으로 승격하지 않는다.
  `/private/tmp/qis.utp62qk5/owner-result.md`; source/resource/span guarded3path는 main에 통합했다.
  Focused55cases/29.26s는 이 owner snapshot 결과다.

- 각 arm의 source→searchd/runner binary 관계를 별도 build 증거로 먼저 결속한다.
  Scanner만 다른 source/input/observation clocks와 독립 tokenizer/full-DP
  oracle에서 실제 whole-call A/B 후 유지/수정/철회를 판정한다. 불가용 과거+8.75%는 새 proof가 아니다.
  비교 CLI의 선언 source SHA만으로 binary build provenance가 입증되지 않는다.
- Canonical clean/one-overlay source identity와 fresh build/capture producer를 반영했다.
  Whole-file SHA 고정 대신 `code-search-typo-unicode-control-v1`의 고정 변환을 독립 재계산한다.
  `uv run --frozen --extra dev python -m pytest -q tools/benchmark/retrieval/test_scanner_source_identity.py tools/benchmark/retrieval/test_scanner_build_custody.py tools/ci/tests/test_query_scanner_ab.py`
  →49passed·28.60s는 historical owner scope다. 후속 두 arm build/capture/whole-call diagnostic은
  위 결과로 완료됐으며 qualified host/acceptance·독립 tokenizer/full-DP 판정은 `NOT_RUN`이다.
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

P1 · W2/W4 · typed scale/load/preflight/ANN 구현 완료; frozen5796 default capacity gates `FAILED`.
Matching release actual 및 별도 override 진단은 아래 범위로 판정한다.

- source74bdc9b4의39 owned paths를 main에 통합했다. `scale-supported-v1`은 pair1GiB/total2GiB,
  client600s, source128MiB/100,000records, vector256MiB, staged body512MiB, process4GiB 계약이다.
  각1MiB upload part를 디스크에 보관하고 hash/CBOR preflight/기존 sealed event identity를 검증한 뒤
  작은 commit으로 발행한다. 기존 inline request cap·SDK30s 및 harness16MiB default는 유지된다.
  이 profile의 원래 Large/XL full/delta/noop/delete/reopen actual과 over-limit refusal는 아직 `NOT_RUN`이다.
- 256/4,096/32,768 tiers의 matching release/profile/lifecycle/open-loop·OS restart를 판정한다.
  이전 source의 default large30s timeout과 xlarge4,000,461 memberships 대4,000,000 cap 거절을 보존한다.
- clean5796 별도 `scale_matrix` build `VERIFIED`: SDK target을 cache seed로 사용해
  `./scripts/cargow --lane test-daemon-lane build -p quanta-index-searchd-harness --bin scale_matrix --all-features --release --locked`
  →exit0·13m42s. Source closure/toolchain/env를 새 clean checkout과 대조하고 SDK proof를 재검증했다.
  Binary SHA-256 `d28c7b480ebb25c68333aad37fcdb6e4e048da00e8cdee3a90bf86a408558e41`.
  `python -m tools.benchmark.retrieval.causal_cost_capture`의 exact source/binary·seed5864059738136528177
  ·small16 actual은 `/private/tmp/qi-scale-f14-5796-20261005-v1-small`에서 exit0·4.861s,
  `VERIFIED_DIAGNOSTIC`이다. 같은 명령의 medium256 actual도
  `/private/tmp/qi-scale-f14-5796-20261005-v1-medium`에서 exit0·16.768s이며 clean/source/binary 전후 일치다.
  Open-loop/실제 OS-process restart 및 qualified performance는 별도다.
- 동일 capture owner와 binary·seed에서 Large default `FAILED`:
  `/private/tmp/qi-scale-f14-5796-20261005-v2-large-default`, producer exit1/capture exit2,
  elapsed70.395s, `build_seal: ipc Read timed out after 30000 ms`. Client timeout 후 daemon work를
  completed-request latency에 넣지 않는다. XL default도 `FAILED`:
  `/private/tmp/qi-scale-f14-5796-20261005-v2-xlarge-default`, producer exit1/capture exit2,
  elapsed1.852s, `source_preflight`에서 posting4,000,461 >4,000,000 거절이다.
  이 값은 first exceeded count이며 전체 XL 수요가 아니다. Daemon은 시작하지 않았다.
- 별도 Large `--client-timeout-ms 300000 --history-max-bytes 268435456` 진단은
  `/private/tmp/qi-scale-f14-5796-20261005-v2-large-diagnostic`에서 producer/capture exit0,
  elapsed127.612s·build68,742.902ms, warm32/errors0/timeouts0이며 source/binary 전후 일치다.
  Full/delta/delete/noop와 같은 OS-process 내부 daemon 재시작을 포함하는 `VERIFIED_DIAGNOSTIC`다.
  실제 OS-child restart proof는 포함하지 않는다. 실행 명령:
  `uv run --project /Users/songmin/Documents/code-new/quanta-index --frozen --extra dev python -m tools.benchmark.retrieval.causal_cost_capture --cwd /Users/songmin/.codex/worktrees/oct5-f14-qualified-5796/quanta-index --binary /private/tmp/qi-retrieval-sdk-f14-5796a63f-20261005-v1/target/release/scale_matrix --source-revision 5796a63f7a813ae3ac3529ea7d281abd64b9db8f --binary-sha256 d28c7b480ebb25c68333aad37fcdb6e4e048da00e8cdee3a90bf86a408558e41 --tier large --seed 5864059738136528177 --max-seconds 600 --out-root /private/tmp/qi-scale-f14-5796-20261005-v2-large-diagnostic --client-timeout-ms 300000 --history-max-bytes 268435456`.
- Full XL source demand `VERIFIED`(제품 capacity 아님): frozen5796의 exact Rust fixture generator를
  외부 std-only probe로 추출해 Medium actual corpus digest7c9b19c9와 먼저 일치시킨 뒤 계산했다.
  `/private/tmp/qi-xl-source-count-probe-5796-20261005-v1/result.json`:
  files32,768, published source113,056,314bytes, largest file3,737bytes,
  exact aggregate distinct path+content trigram memberships17,715,020,
  corpus digest `sha256:dbc4b0d39b458aa4fd838a28e01caf95c0146601b1fae50bfc5d3b197dd59821`.
  Source128MiB ceiling 안이지만 기존4M posting ceiling 밖이다. U64 posting IDs만141,720,160bytes이며
  dictionary/header와 source bytes는 별도다. Harness의 default history16MiB/total256MiB도 별도 gate다.
  Aggregate/resident/disk/query 및 retention 계약을 함께 정하고 shard별 cap으로 global cap을 대신하지 않는다.
- 같은 source-bound per-file counts의 v15 source-key256bucket histogram은
  `/private/tmp/qi-xl-bucket-count-5796-20261006-v1/histogram.json`에서 global 합계를 재검증했다.
  Max bucket161files/556,075source bytes/87,142memberships다.
  Source count만 `VERIFIED`이며 실제 encoded disk/scratch RSS/capacity proof는 아니다.
- 실제 OS-process Medium 회귀 producer/test registration은 `7a16771a`에 반영했다.
  `runtime_extended_suite::e2e_scale_process_restart`가256source digest/line bounds,
  G1 positive→G2 tombstone, repo2의64개 전체 결과와 ranked/source rows의 실제 child stop/reap/restart
  보존을 검사한다. Current78d2474에서 아래 actual owner가 `VERIFIED`:
  `CARGO_BUILD_JOBS=1 QUANTA_INDEX_SCCACHE=0 ./scripts/cargow --lane test-daemon-lane nextest run -p quanta-index-searchd-runtime --test runtime_extended_suite --all-features --locked -E 'test(=e2e_scale_process_restart::medium_scale_source_survives_real_daemon_process_restart_and_delete)' --test-threads 1 --no-tests fail --success-output final`
  →1passed/82unselected·31.805s/exit0. 실제 child stop/reap/restart와 G2 delete/source/ranked-row 보존이다.
  최초 compile의4unused-result 오류는 수정했고 대상 Rust 포맷도 정리했다.
  Full runtime suite/Large·XL OS restart/Linux/resource qualification은 `NOT_RUN`이다.
- large300s/256MiB diagnostic 성공을 default 성공으로 바꾸지 않는다.
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

P0 · W3 및 source 변경 시 재수행 · proof issuer/verifier 구현 완료;
frozen `5796a63f` Contract·fresh SDK `VERIFIED`, 후속 source7fb46415 hosted CI `FAILED`.

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
  이 owner rail은 matching-source fresh SDK·hosted CI·운영 검증을 포함하지 않는다.
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
- Fresh Contract·SDK `VERIFIED`: clean `5796a63f7a813ae3ac3529ea7d281abd64b9db8f`에서
  `CARGO_BUILD_JOBS=1 QUANTA_INDEX_SCCACHE=0 just retrieval-contract-proof /private/tmp/qi-retrieval-contract-f14-5796a63f-20261005-v2`
  → Python selected/executed/passed791, Rust selected/executed/passed191, failed0/exit0.
  `CARGO_BUILD_JOBS=1 QUANTA_INDEX_SCCACHE=0 just retrieval-sdk-proof-fresh /private/tmp/qi-retrieval-sdk-f14-5796a63f-20261005-v1`
  → SDK selected/executed/passed27, failed0/skipped0/exit0. Fresh release daemon·runner와
  별도 process의 lexical/semantic/hybrid 요청을 포함한다.
  두 root의 `execution-context.json` 각각에
  `uv run --frozen --extra dev python tools/benchmark/retrieval/portable_proof.py verify --receipt <root>/execution-context.json`
  을 독립 실행해 exit0를 확인했다.
  searchd SHA-256 `43355866f76a6f4c3621f31009338750f6c22c7349ee99b0bbdf7f03ed069f34`,
  runner SHA-256 `bfae22c4397df8d0c9be6556a7770b03b16ce5b7004914e2c69eb9cf4a96b666`.
  이 결과는 해당 frozen source의 macOS 로컬 proof다. Semantic/hybrid는 개발용 Hash provider를
  사용했으며 real-provider·Linux 배포/운영·hosted CI·검색 품질/성능 qualification은 미포함이다.
  후속 source revision과 proof를 대조하며 이 결과의 revision을 변경하지 않는다.
- 후속 source epoch의 포함 코드/driver/scorer/ADR 및 영향 mandatory surfaces를 검증하고 matching fresh
  Contract/SDK/source closure/binaries를 발행·portable replay한다. E3 shipping acceptance와 CI도 실제 scope로 판정한다.
  감사 기준 `f251296d`와 history/admission overlay에는 frozen5796 이후 Justfile/ADR/R5/scanner/ARB 및
  owner 변경이 있다. 최신 formal Contract/SDK는 `NOT_RUN`, 아래 hosted CI는 실제 `FAILED`다.
  Frozen791/191/27 결과의 source를 바꾸지 않는다.
- Current source owner 추가 검증: ARB adapter의 raw32→effective35 용어 초과를 canonical planner
  term projection 공유로 수리했다. Adapter v2는 raw32/effective32를 함께 제한하고 식별자 우선·원래 출력 순서를 유지한다.
  `uv run --frozen --extra dev python -m pytest -q tools/ci/tests/test_arb_adapter.py`와 같은 실행에
  `test_retrieval_benchmark.py`의 NFC/indexability/UTF8/lowercase/bounded-NL4selectors를 더해
  →51passed·2.14s/exit0, real Gin88 입력 case도 skipped 없이 실행했다.
  Frozen v1 ARB capture는 v2 결과로 바꾸지 않으며 새 adapted88 capture/scoring은 `NOT_RUN`이다.
- P12A current owner `VERIFIED`: 최초 canonical 실행은276passed/1failed였고,
  cross-repo script의 custody 호출 개수 고정 테스트가7개 실제 경계와 불일치했다.
  경계별 guard/제거 mutants와 locked `uv run --frozen --extra dev python` 진입점을 함께 수정했다.
  `QUANTA_PROOF_RAW_DIR=/private/tmp/qi-p12a-quanta-locked-20261006-v2 just proof-p12a-proof-infrastructure`
  →295selected/passed·93.66s/exit0. Registry lint는25registered/0manifest인 `REGISTRY_ONLY`이며
  P12 aggregate/CODE_QUALIFIED/release 실행 결과를 발행한 것은 아니다.
- Hosted CI 실제 실패와 수리: `7a16771a`의 ARB effective35/OS-process format 원인을 수정했다.
  후속 `c2431375` Python1679는676pass/7skip 뒤 Contract 준비 selector의 필수 `--bin` 누락
  고정 oracle에서 실패했고 Rust1680은 test barrier MutexGuard drop으로 실패했다.
  두 owner 수정 후 `f251296d` Python1683은2,228pass/8skip 뒤 current file-pair fixture의
  불완전한 manifest에서 실패했다. Rust1684는 scale 계측 코드 Clippy6건으로 실패했다.
  Clippy 원인은 현재 history overlay에 수정했으며 file-pair fixture의 전체 canonical replay 수리는 진행 중이다.
  같은 broad Python 명령을 로컬 실행한 결과도 `FAILED`: `uv run --frozen --extra dev python -m pytest tools -q --ignore=tools/ci/tests/test_semgrep_policy.py --ignore=tools/ci/tests/test_check_rust_fallbacks.py -x`
  →2,228passed/8platform-skipped/1failed·926.86s. Latest hosted pass와 full-suite pass는 없다.
- History/admission current owner `VERIFIED`: `test_causal_cost_capture.py`, `test_causal_cost_profile.py`,
  `test_retrieval_benchmark.py`의 batch boundary/input-source drift/disjoint freeze3selectors를 함께 실행해
  →58passed·16.76s/exit0였다. Pair/total requested·effective 값은 daemon/scale/open-loop/artifact/replay에
  결속한다. 기존 default16MiB/256MiB는 유지하며 큰 override는 diagnostic이다.
  입장 검증은 배치 시작·종료의 전체 release replay와 개별 source/gold/review 검사를 유지한다.
  후속 exact seal retention/batch 경계·source drift67selectors는67passed·1.35s/exit0였다.
  새 final-source packet/proof·actual admissions는 `NOT_RUN`이다. F15 lexical 전체 실행은 아래처럼 구분한다.
- F15 lexical 전체 owner `VERIFIED` (source0b 컴파일 범위):
  `CARGO_BUILD_JOBS=1 QUANTA_INDEX_SCCACHE=0 QUANTA_INDEX_TARGET_GC=0 ./scripts/cargow --lane test-f15-owner-lane nextest run -p quanta-index-lexical --all-features --locked --test-threads 1 --no-tests fail --no-fail-fast --success-output never`
  →598passed/8skipped·702.173s/exit0. F15 제품3회귀, format/cost·Unicode 및 기존 lifecycle/query/authority 통합 범위다.
  이후 경계 검사/shared-limit overlay의 영향 회귀는 아직 미실행이며 이 결과의 source를 바꾸지 않는다.
  최초 strict Clippy176건은 guarded codec/root/producer/reader/verify 수정으로 통합했다. 다음 실행의
  producer 이름 충돌4건도 수정했고 all-target/all-feature lexical+harness 재실행 중이다.
  Scale preflight의 stale4M을 canonical lexical20M 선언으로 연결했으며 fixed17,715,020 허용/
  20,000,001 거부 테스트를 추가했다. 실제 XL capacity·history/timeout qualification은 `NOT_RUN`이다.
- 공개SDK/contract 변경: `just rust-public-api`; wire/decode: `just rust-fuzz-smoke`;
  module: `just rust-hexagonal`, `just rust-cargo-modules`; selection/state/ingress: `just rust-profile test-daemon`.
- runtime `autotests=false`: read-view/ingest는 `runtime_fast_suite`, generation/cursor/restart는
  `runtime_risk_suite`, crash/readiness는 `runtime_extended_suite`; 등록된 OS-child owner targets는 별도다.
- 완료: 정확한 source/command/selector/binary actual results와 필요한 CI/SDK/contract surface.
  Real-provider/Linux 배포·운영/scale 등의 미포함 경계를 명시한다.

### O4-I0-03

P1 · 코드 입력 먼저, W6 실행 · **P11 operational producer/recipes 미구현**.
P11 target·관측 계약 입력은 `BLOCKED`이다. R3는 별도 Semantica producer 연동 잔여다.

원본 `agent-1.md`의 Release 범위에서 인계된 Linux 서버 배포·운영 검증이다.
로컬 엔진 회귀와 검색 평가의 완료 판정은 각 owner scope를 따른다.

- R5 component 코드 수리 `VERIFIED` owner scope: Quanta의 existing cross-repo recipe에
  optional `QUANTA_P11_R5_EVIDENCE_ROOT` typed archive를 연결하고, Semantica nextest frontdoor가
  기존 CLI-owned immutable completion custody를 사용하도록 두 main 작업트리에7개 owned paths를 반영했다.
  원본 코드의 두 focused tests는 실제 `FAILED`였고 수정 후 completion/custody/locator/closeout4파일은
  `PYTHONPATH=tools/quanta-build-cli uv run --frozen --only-group architecture-tooling python -m pytest -q -o addopts='' tools/quanta-build-cli/test_nextest_completion_frontdoor_v1.py tools/quanta-build-cli/test_verification_completion_custody_v1.py tools/quanta-build-cli/test_verification_completion_locator_v1.py tools/quanta-build-cli/test_verification_completion_closeout_v1.py`
  →25passed·0.45s/exit0였다.
  Quanta `uv run --frozen --extra dev python -m pytest -q -o addopts='' tools/ci/tests/test_paired_r5_result.py`
  →18passed·0.09s/exit0; 해당2파일 Ruff와 candidate test-authority 검사도 exit0였다.
  Runtime caller list/run/resolver는 Cargo target이 요구하는
  `index-sdk-ingress,retrieval-authority-contract-surface`를 동일하게 선택한다. Kernel feature는
  `index-sdk-ingress-surface`다. Locator/process exit/source/nonce/argv/receipt와 resolver의
  int/bool/float 신원은 typed canonical bytes로 검증하고 actual runner 전후 source/lock/manifest를 대조한다.
  Root는 stage/commit/push하지 않았다. Actual clean-pair QBC caller/kernel 실행·fresh daemon custody는
  `NOT_RUN`; 결과 형식은 `runner-candidate-only`이며 P11 operational staged node를 발행하지 않는다.
  `Justfile`/test-authority와 후속 ADR 변경의 source closure 및 영향 proof는 새 epoch로 검증한다.
- 최종 감사의 R5 집중 재실행 `VERIFIED`: 위와 같은 Quanta 명령은18passed·1.38s/exit0,
  Semantica 명령은25passed·0.72s/exit0였다. Quanta owned5와 Semantica 신규 테스트는
  외부 v4 postimage와 일치한다. Semantica CLI 전체 파일에는 후속 log-GC/proof-guard 변경이 있어
  7파일 전체 postimage 일치를 주장하지 않는다. 검증한 CLI/4개 test bytes는
  Semantica `42fa5287`에서`34d85f50`으로 이동한 뒤에도 동일했다. Actual pair는 계속 `NOT_RUN`이다.
- 별도 producer 연동 잔여 R3: Semantica `search_plane_handoff_dispatch/semantic_state.rs`는 lexical emission과
  prior state에서 semantic plan을 구성하고, `semantic_plan.rs`는 cluster mutations를 추가한다.
  `aggregate_prepare.rs`의 실제 dispatch 준비에는 별도로 산출한 고정 expected
  replace/tombstone/unchanged partition과 배치의 누락 대조가 없다. Quanta의
  `semantic_derive.rs`/`corpus_wire.rs`는 전달된 범위의 중복·충돌을 거절하는 owner다.
  누락이 실제 발생했다는 실행 증거는 없으며, 이 잔여는 upstream 검증 계약의 미구현이다.
  Quanta 자체 lexical/semantic 엔진 결함이나 필수 코드 개발 건수로 세지 않는다.
  Source plan·shadow policy·prior sealed state·cluster plan에 결속한 독립 기대 범위,
  정상 lexical-only/unchanged-empty와 omission/duplicate negatives를 producer dispatch 전에 검증한다.
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
