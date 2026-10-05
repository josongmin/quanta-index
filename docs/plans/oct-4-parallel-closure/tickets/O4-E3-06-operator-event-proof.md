# O4-E3-06 — 기존 operator diagnostics·process truth 검증

| 항목 | 값 |
| --- | --- |
| 에픽 / 담당 | [E3 — Active 선택·read-view lifetime·운영 계약](../epics/E3-selection-and-operational-safety.md) / E3 담당 |
| 우선순위 / 종류 | P1 / `PROOF_ONLY` |
| 기준 웨이브 | [W1 — 근거·정답·producer 병렬 준비](../waves/W1-evidence-and-producers.md) |
| 실행 상태 | current process_readiness_owner_v1 실제26 passed(14 OS-child scenario·12 helper). Linux 실제2UID fixture 준비·pinned image 도구 확인 완료; nextest test0 및 v2 protobuf header 부재 compile 실패 보존; v3 canonical cargow test exact1 Linux build 실행 중. Linux release/P11 proof `NOT_RUN` |
| 선행 결과 | 없음. 현재 source 확인과 fixture 준비부터 시작 가능 |

[전체 지도](../README.md) · [티켓 인덱스](INDEX.md)

## 목적

이미 구현된 bounded request-event 경로의 authorization·loss·restart·실제 correlation을 final source에서 입증한다.

## 2026-10-04 fixture 통합

- `binary_query_ring_reports_wrap_loss_and_retains_the_latest_request`가 실제 daemon child에 Text IPC 300개를 보내 oldest/next/dropped와 첫 request eviction, latest ResponseWritten 및 process instance 일치를 검사한다. 현재 `process_readiness_owner_v1`과 extended suite가 포함하며 default `test-daemon`은 제외한다.
- 기존 real UDS + injected peer의 authorization-before-ring-read, 같은 UID binary operator 성공과 다른 OS UID process refusal은 별도 범위다. 다른 OS 사용자 실행은 아직 `NOT_RUN`이며 injected principal을 실제 OS UID 증거로 승격하지 않는다.
- `VERIFIED`: source `904043302f1db8406302a5a62bcffdc0d9412267`에서 `./scripts/cargow --lane test-daemon-lane nextest run -p quanta-index-searchd-runtime -p quanta-index-lexical --test process_readiness_owner_v1 --test l3_exact_source --all-features --locked --test-threads 4 --success-output final` — exit0, 전체56 passed/0 skipped, tests26.073s. process owner는26개 중14 OS-child scenario와12 helper다. L3 exact source30개는 별도 lexical scope다.
- actual binary ring wrap(300 requests), process-instance restart/prior-window discard, payload-free query correlation, active-root loss, inventory-read failure/reopen, 1-file ranked-row OS restart를 포함한다. child는 matching `CARGO_BIN_EXE`와 hash-dev embedder, private0700 state를 사용한다. learned semantic 품질, 모든 scale tier의 OS restart, 다른 실제 OS UID refusal 및 Linux release qualification은 `NOT_RUN`이다.
- `VERIFIED`: 유지보수 fatal ownership 및 process fixture Clippy 수리 뒤 `./scripts/cargow --lane test-daemon-lane nextest run -p quanta-index-searchd-runtime --test process_readiness_owner_v1 --all-features --locked --test-threads 4` — exit0,26 selected/run/passed,0 skipped,tests16.869s. 실제 backend-root loss는16.868s, inventory failure/reopen은11.821s에 통과했다. 이 owner 파일과 product bytes는 formal source `f3f7c68993f383e4ac5fdca761c111fe3d0edc3b`와 같다. source904의 과거 결과를 재사용한 것이 아니며 위 제외 범위를 유지한다.

## 2026-10-05 정적 잔여 판정

- 실제 binary child의 request-ID/queue/backend/terminal correlation, provider ticket, 300-request ring wrap과 dropped window, process-instance restart, active-root loss는 `process_readiness_owner_v1`의 기존 fixtures가 소유한다. injected peer credentials와 같은 UID의 shared socket 검증은 다른 실제 OS UID의 증거가 아니다. 기록된 PASS를 이번에 재실행하지 않았다.
- **다른 실제 OS UID의 operator refusal 및 Linux release 실행은 입력 미확보로 `BLOCKED`**, 이 환경에서 `NOT_RUN`이다. Linux runner에 실제 owner·다른 UID, 해당 UID로 실행할 matching client binary, 경로 traversal 권한이 필요하다. 기존 `E2eRuntime::boot_with_socket_access`는 공유 socket을 접근 가능한 `/tmp` 아래에 두고 기존 `SearchdConfig::with_socket_overrides`를 사용하므로 새 production API는 필요 없다. OS credential을 가진 다른 UID의 Admin `events` 거절과 무노출, owner 성공, 허용된 non-Admin 요청 성공을 각각 검증해야 한다. 기본 `--state-root`의 0700 경로 아래 socket만으로는 다른 UID의 accept 경계를 검증할 수 없다.
- 정확한 현재 owner selector: `./scripts/cargow --lane test-daemon-lane nextest run -p quanta-index-searchd-runtime --test process_readiness_owner_v1 --all-features --locked --test-threads 4`. shared socket 기존 fixture selector: `./scripts/cargow --lane test-daemon-lane nextest run -p quanta-index-searchd-runtime --test runtime_extended_suite --all-features --locked -E 'test(e2e_socket_access)'`. 둘 다 이번 정적 판정에서는 `NOT_RUN`이며 real two-UID/Linux case의 대체 증거가 아니다.

## 2026-10-05 실제 두 UID component fixture PREPARE

- 별도 `/Users/songmin/.codex/worktrees/oct5-linux-uid/quanta-index`의 base0e6 위에 Linux-only ignored parent/helper를 준비했다. 기존 shared `/tmp` socket harness와 실제 `setpriv` client를 사용하며 다른 UID의 typed Admin refusal·query ring 무노출, owner read 성공·허용된 Active Text query/terminal event를 검사한다. production API 변경은 없다.
- 변경은 `e2e_socket_access.rs`(최종 SHA `427b7588766a31ab4e41a1f82f98e11cca56ff2954a9d957ebfd1e1577346a19`)와 ignored-policy2예외(SHA `149181b1123f014d0561566c585137d5e5fa1c277d570569906d0d50d00ad7a6`)뿐이다. raw `send_request`는 response ID를 자체 비교하지 않아 두 child 응답의 exact IDs0xe305/0xe306 검사를 추가했다. canonical `./scripts/cargow fmt --check` exit0, Python3.13의 `tools/ci/lint/check-ignored-test-policy.py` exit0, diff-check exit0이다. Linux typecheck/test는 `NOT_RUN`이다.
- owned disposable container image `sha256:9c558534f4e4aa4b08a94ed27de00410dfa2b5b1a9003081c37de9c3a5ff2ed2`의 사전 inventory는 Linux/aarch64·root·Python3.11.2·setpriv·cc·`/tmp`1777 및 실제 UID/GID65534 전환을 관측했다. `bash -lc` 실행에서 Rust/cargo를 찾지 못했지만 image PATH에 `/usr/local/cargo/bin`이 있어 login shell PATH reset 여부를 `bash -c`로 확인해야 한다. root/다른UID IPC test는 아직 아니다. 실제 build 도구 확인 뒤 정확한 root 전용 selector로 실행한다.
- selector: `./scripts/cargow --lane test-daemon-lane nextest run -p quanta-index-searchd-runtime --test runtime_extended_suite --all-features --locked --run-ignored only -E 'test(linux_two_real_uids_enforce_operator_events_and_serve_listed_query)' --success-output final`. 이 Linux UDS credential component 범위와 P11 실제 release/deploy/config/rollback 범위는 별개다.
- 후속 PREPARE v3는 응답 수신과 ring terminal 기록의 순서를 보완했다. `server.rs`는 `write_all` 이후 `ResponseWritten`을 기록하므로, 기존 `fail_closed_wait`로 exact request0xe306 terminal을 최대5s 관측하고 동일 process instance를 확인한다. 임의 고정 sleep으로 통과를 만들지 않는다. 최종 fixture SHA는 `6bf943b79e6d7641308767b4389d03c9afbcc56a13ee82b6e1723564d59eea34`, policy SHA는 위와 같다. 변경 뒤 `./scripts/cargow fmt --check`, Python3.13 ignored-policy lint, diff-check가 각각 exit0이다. Linux build/test는 여전히 `NOT_RUN`이다.
- 최신 실행 입력은 `/private/tmp/qi-linux-uid-serial-prepare-v3/packet.json`(SHA `77c30645b69bd407423051bb77513c63e86b0f965308962fa93fc1ef474aba86`) 및 같은 폴더의 `owned.patch`(SHA `d62d9a736f85e2bb1eafc150d6b22f2bd03379805547711e53dda20bb05e55ba`)다. v2 입력은 보존하며 실제 실행은 v3의 source/overlay를 다시 확인하고 current root serial batch 종료 후 수행한다. 고유 SSD cache `/Volumes/Extreme SSD 1/qi-linux-uid-component-20261005-v3`에는 입력 준비만 수행했다. locked613 crate archives를 Cargo.lock checksum과 대조하고 sparse index와 함께1,176files/149,708,167bytes를 neutral cargo-home에 복사했다. credentials/config/git checkout/Darwin target는 포함하지 않았다. `/private/tmp/qi-linux-uid-registry-prepare-v1.json` SHA `4213f76376ecc24927c9665aaed8278891291a8042a22b7e7074a7668863f8ea`이며 실제 Linux build/test는 `NOT_RUN`이다.

## 2026-10-05 실제 Linux component 실행

- cached image의 `bash -c` inventory는 root/aarch64, `/tmp`1777·exec 가능 및 실제 UID/GID65534·groups=[]를 확인했다. image default stable Rust1.98과 protoc 부재로 prerequisite 결과는 `BLOCKED`였다. Rust1.92 toolchain은 이미 설치돼 있었다. `/private/tmp/qi-linux-uid-inventory-actual-20261005-v1/stdout.json` SHA `bffc047cc075250740e4f50e7402786d002471440305ccb0ca9c0f3c32962825`를 보존했다.
- owned disposable container에서 `RUSTUP_TOOLCHAIN=1.92.0`을 고정하고 `apt-get update`→`apt-get install -y --no-install-recommends protobuf-compiler`를 실행했다. pinned Rust/cargo·cc·setpriv·protoc·pkg-config inventory는 `PREPARED`다. host 설치나 image pull은 없으며 container network를 분리한 뒤 locked/offline Rust rail을 시작했다.
- v1 actual nextest는 exit96으로 `/work/target/nextest/default` store를 읽기 전용 source mount에 생성하지 못해 실패했다. fixture test0이며 제품 compilation/test 성공을 주장하지 않는다. root `/private/tmp/qi-linux-uid-component-actual-20261005-v1` 및 summary의 `FAILED`, owned container cleanup `VERIFIED`를 보존했다.
- v2 actual canonical fallback은328.791s/exit101로 `lance-encoding v7.0.0` build script에서 `google/protobuf/empty.proto` 부재를 거절했다. fixture test0이며 raw와 owned cleanup `VERIFIED`를 보존했다. `protobuf-compiler`만으로 well-known headers가 제공되지 않았고 product fixture defect는 관측하지 않았다. v3는 새 owned container/root `/private/tmp/qi-linux-uid-component-actual-20261005-v3`에서 container-only `protobuf-compiler`와 `libprotobuf-dev` 설치,8개 header SHA 및 실제 Empty descriptor 컴파일 preflight를 통과한 뒤 실행 중이다. inventory v2 SHA `9a31c58f58531eefad7ea95b5689006d358e47f986a694d7559e6fd730ec78d9`, controller v3 SHA `58acf22889c3e5ff9eba4417df860cbfcf004a8ee114f148491325375c63009b`다. 명령: `./scripts/cargow --lane test-daemon-lane test -p quanta-index-searchd-runtime --test runtime_extended_suite --all-features --locked -- --ignored --exact e2e_socket_access::linux_two_real_uids_enforce_operator_events_and_serve_listed_query --nocapture`. RO `/work`, SSD RW `/qi-cache`, neutral Cargo source cache, `CARGO_BUILD_JOBS=1`,4GiB container memory, sccache/GC/build-logging0이다. 실제1selected/1passed와 source/2owned SHA 전후·owned cleanup을 확인하기 전 결과는 미확정이다. 이 config는 성능 qualification이 아니다.

## 배경과 현재 상태

현재 split.rs의 ProcessRequestEventsV1 request/response, control dispatcher의 ProcessRequestEventsPort, SDK observability.request_events와 searchctl rendering이 존재한다. IpcServerCounters도 process_instance/sequence/drop window를 제공한다. SEP21 옛 계획의 새 endpoint 구현 항목을 그대로 재구현하면 중복된다.

## 착수 입력

- 실제 daemon의 existing Admin/root admission, observer·operator principal fixtures
- 현재 ring wrap/drop/instance restart, request-ID backend/provider/terminal join 및 16MiB transport cap

## 어떤 파일을 어떻게 수정할지

`OWNED`는 에픽 담당 통합, `SHARED`는 I0 반영, `READ`는 기존 구현 소비다. 재현된 결함이나 채택된 계약 변경이 있을 때만 product source를 수정한다. 구현 파일과 독립 검증 파일을 함께 지정한다.

| 파일 | 함수 / 경계 | 구체적인 변경 또는 검증 | 모드 |
| --- | --- | --- | --- |
| [crates/quanta-index-ipc/src/counters.rs](../../../../crates/quanta-index-ipc/src/counters.rs) | RequestEventWindowV1 / existing event ring | READ: 실제 bounded sequence/drop behavior를 검증하고 proved bug만 수정한다. | READ |
| [crates/quanta-index-search-plane/src/control_dispatcher.rs](../../../../crates/quanta-index-search-plane/src/control_dispatcher.rs) | ProcessRequestEventsPort / authorization route | observer denial이 ring read보다 앞서는지 actual fixture로 검증한다. | OWNED |
| [crates/quanta-index-contract/src/ipc/control.rs](../../../../crates/quanta-index-contract/src/ipc/control.rs) | ProcessRequestEvents DTO | READ: max event/encoded byte 계약을 소비한다. 새 DTO/Operate capability는 만들지 않는다. | READ |
| [crates/quanta-index-sdk/src/observability.rs](../../../../crates/quanta-index-sdk/src/observability.rs) | request_events | 현 response binding/limit/plane/process identity를 소비한다. | OWNED |
| [crates/quanta-index-searchctl/src/lib.rs](../../../../crates/quanta-index-searchctl/src/lib.rs) | RequestEvents dispatch/rendering | 현 CLI에서 process/loss/truncation을 정확히 보여주는지 확인한다. | OWNED |
| [crates/quanta-index-searchd-runtime/tests/process_readiness_owner_v1.rs](../../../../crates/quanta-index-searchd-runtime/tests/process_readiness_owner_v1.rs) | real operator process proofs | observer denial/instance restart/child loss 및 ring disclosure negative를 현재 rail에 등록한다. | OWNED |
| [crates/quanta-index-searchd-runtime/tests/e2e_process_readiness.rs](../../../../crates/quanta-index-searchd-runtime/tests/e2e_process_readiness.rs) | binary_daemon_exposes_one_correlated_query_without_payload / binary_daemon_detects_lost_active_backend_root | 이미 있는 binary-process tests를 먼저 소비한다. 빠진 observer/instance/loss control만 이 실제 포함 module에 추가하고 owner wrapper와 registry를 연결한다. | OWNED |
| [crates/quanta-index-searchd-runtime/tests/common/searchd_binary_process.rs](../../../../crates/quanta-index-searchd-runtime/tests/common/searchd_binary_process.rs) | SearchdBinaryProcess / searchd_binary_path | CARGO_BIN_EXE_quanta-index-searchd의 build-profile/source와 실제 child를 확인한다. 임의 env binary로 교체되지 않으며 owner-local binary process와 Linux fresh release proof는 별도다. | READ |
| [crates/quanta-index-sdk/src/tests/control_tests.rs](../../../../crates/quanta-index-sdk/src/tests/control_tests.rs) | ProcessRequestEventsV1 controls | 기존 plane/limit/variant validation tests를 소비하고 missing/wrong process/loss binding case만 보강한다. | OWNED |
| [crates/quanta-index-searchctl/src/render.rs](../../../../crates/quanta-index-searchctl/src/render.rs) | render_request_events | pretty/JSON에서 process instance·sequence/drop/truncation을 실제 DTO 의미대로 출력하는지 검증한다. 재현된 표시 결함이 있으면 현 renderer를 수정한다. | OWNED |
| [crates/quanta-index-searchctl/src/tests/control.rs](../../../../crates/quanta-index-searchctl/src/tests/control.rs) | request-events parse / render tests | 기존 request-events parsing·pretty/JSON fixtures를 실행하고 loss/limits/refusal 표시가 실제 contract와 같은지 보강한다. | OWNED |
| [crates/quanta-index-searchctl/tests/cli_smoke.rs](../../../../crates/quanta-index-searchctl/tests/cli_smoke.rs) | control CLI process smoke | CLI process의 request-events success/refusal와 typed output를 test UDS에서 확인한다. 이 mock dispatcher smoke를 real-daemon proof로 승격하지 않는다. | OWNED |

## 실행 단계

1. 기존 SDK/control/ring/CLI caller graph와 binary_daemon_exposes_one_correlated_query_without_payload 등 actual process tests를 확인한다. 기존 case를 재작성하지 않고 현 source에서 재실행한다. process_readiness_owner_v1.rs는 e2e_process_readiness.rs를 포함하는 target wrapper다.
2. observer principal 요청 시 ring read 호출과 disclosure가 0인지 고정 port로 검증한다.
3. operator success, wrong plane/limit/oversize, ring wrap/drop/sequence exhaustion와 process restart를 검사한다.
4. 실제 query/ingest request를 server→backend/provider→terminal request ID로 join하고 missing/drop은 명시한다.
5. supervisor child/maintenance/backend loss를 readiness false와 연결하고 실제 scenarios를 I0에 전달한다. CARGO_BIN_EXE로 수행한 local binary process proof와 P09 Linux release-daemon-fresh scope를 구별한다. 누락 release target은 I0-03에서 등록한다.

## 검증 계획 — NOT_RUN

아래는 실행할 명령/시나리오다. 본 문서에서 통과를 주장하지 않는다. `<...>`와 외부 root는 실행 전에 실제 값으로 확정한다. test filter는 실제 수집 ID를 확인하고 0 tests를 성공으로 표시하지 않는다.

- `./scripts/cargow test -p quanta-index-ipc --lib --all-features --locked`
- `./scripts/cargow test -p quanta-index-search-plane --lib --all-features --locked control_dispatcher`
- `./scripts/cargow test -p quanta-index-sdk --lib --all-features --locked`
- `./scripts/cargow test -p quanta-index-searchctl --lib --test cli_smoke --all-features --locked`
- `./scripts/cargow test -p quanta-index-searchd-runtime --test process_readiness_owner_v1 --all-features --locked`
- 재현된 public/decode/process surface 수정 시 AGENT_PLAYBOOK의 해당 escalation gate를 실행한다.

## 완료 조건

- existing operator path가 bounded authorization/loss/instance/correlation contract를 실제 daemon에서 충족한다.
- 동등 source와 registry case가 이미 충족하면 code edit 없이 proof target binding으로 닫는다.

## 중단·거절·재개 조건

- 이벤트 ring은 accounting authority가 아니며 request ID/payload를 metric label에 넣지 않는다.
- 필요한 입력 부재는 `BLOCKED`, 미실행은 `NOT_RUN`, 실제 실행 실패는 `FAILED`로 기록한다. 조건 미성립 `NOT_APPLICABLE`에는 실제 판단 근거가 필요하다.
- 변경이 source/input/query/unit/result에 영향을 주면 [I0 source gate](O4-I0-02-matching-source-proof.md)와 영향받는 capture/report를 다시 판정한다.
- 일회성 raw/log/capture/receipt는 checkout 밖 새 root에 둔다. 기존 외부 terminal을 덮어쓰지 않는다.

## 인계 결과

- 실제 source/dirty ownership, 변경 파일과 계약, 실행한 명령/selector, 관측 결과 및 제외 범위.
- raw/model/runtime/binary/input identity는 해당 실행 계약이 요구하는 범위에서 기록한다.
- 완료 조건별 `VERIFIED`/`FAILED`/`BLOCKED`/`NOT_RUN`/`NOT_APPLICABLE`과 후속 티켓에 넘길 입력을 발행한다.
