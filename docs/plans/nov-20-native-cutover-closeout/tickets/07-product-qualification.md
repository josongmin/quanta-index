# 07 · 최종 후보의 SDK60·Large·ANN·Hybrid·E07

**잔여 인계 · 11월 22일 01:34 KST.** 최종 설치 전에 기존 D 서명 입력, TEXT quiet-host 및 원 코퍼스, B ANN4K/whole SDK 원 oracle, F-EVAL 독립 심사를 각 오너가 병렬 준비한다. 이미 있는 서명 소비·벤치 코드와 동결 R56/ANN 결과를 각 범위에서 인수한다. 현행 06.4 후보의 SDK60/Large·signed Hybrid·ANN·Mixed/E07·두 비교 기준 실행과 원 영수증이 남았다. [레인별 검수](LANES.md#지금-병렬로-준비할-작업)

**담당** 기존 TEXT와 B는 생산·벤치, D는 설치 신원 공급, F는 실제 설치 증거 소비·품질/성능 통합. **상태** R56의 Record와 Large는 동결본 `VERIFIED`; 최종 퇴역 후보의 제품 수용 `NOT_RUN`.

05:48 KST 후속 V2 집중 회귀 `release-cohort.sjb9vswk`는 D recovery의 테스트 오류형 불일치로 **FAILED · E0308 · exit 101 · 본문 0**이다. Root가 [기존 D 테스트 helper](03-signed-published-cp2.md)의 오류형을 교정했다. 기존 TEXT 오너가 수리를 포함한 동결본의 집중 Rust 회귀를 인수한다. [원 집중 로그](</Volumes/Extreme SSD 1/build-cache/nc-text-v2-shared-case.ArZMh8/focused-nextest.log>)

05:54 KST 기존 TEXT의 Layered UDS `c5204948-cfcd-40e7-acf1-67e456116a60`은 **2 PASS · 1 FAIL**이다. 이전 Exact view 경로가 먼저 통과했다. 새 view에서 공개 path 오름차순의 첫 행인 `Renamed.rs` 대신 fixture가 `a.rs`를 기대했다. 기존 오너가 기대 배열 순서 한 줄을 수정했다. 후속 delete·repack·restart 단언까지 실행하는 재검증이 필요하다. [원 본문](</Volumes/Extreme SSD 1/build-cache/nc-text-v2-shared-case.ArZMh8/layered-uds-nextest.log>)

05:59 KST 기존 TEXT의 Lexical/Kernel 집중 실행은 **30 PASS · 1 FAIL**이다. `segmented_full_unanchored_and_regex_use_pinned_document_tables`에서 `ResourceLimited`가 발생했다. 코드에서 fixture의 일반 질의 예산 **1,000,000 B**와 Regex 엔진·준비 고정 청구 **15,782,912 B**의 불일치를 확인했다. 원 로그에는 NOT과 Regex 중 어느 단계에서 처음 거절됐는지 없다. 기존 TEXT 오너의 fixture와 실행을 인수한다. [원 집중 본문](</Volumes/Extreme SSD 1/build-cache/nc-text-v2-shared-case.ArZMh8/text-kernel-unit-nextest.log>) · [Regex 예산 코드](../../../../crates/quanta-index-lexical/src/native/text_regex.rs#L63)

기존 TEXT 작성자가 일반 예산의 Regex 거절 단언을 추가하고 이 fixture의 성공·손상 확인에만 별도 **32 MiB** 예산을 부여했다. 코드 합류를 확인했으며 수정 본문은 **NOT_RUN**이다. [수리된 fixture](../../../../crates/quanta-index-lexical/src/native/text.rs#L1525)

06:18 KST 정렬 fixture 수리본의 정확한 Layered UDS 재실행 `02ea5dab-03bf-4e36-a315-8593f7e0052b`은 **VERIFIED · 1/1 · exit 0 · 6.695초**다. 기존 두 PASS와 동일 selector 그룹의 이전 결과를 인수했다. [Text 오너의 원 terminal](</Volumes/Extreme SSD 1/build-cache/nc-text-v2-shared-case.ArZMh8/case-uds-retry.log>)

06:25 KST 별도 Regex 예산 fixture 재실행 `24869882-025c-4a0d-9776-81d32c8f042b`는 **VERIFIED · 1/1 · exit 0**이다. 선행 30 PASS는 그 선택과 입력 범위에서 유지한다. [Text 오너의 원 terminal](</Volumes/Extreme SSD 1/build-cache/nc-text-v2-shared-case.ArZMh8/regex-unit-retry.log>)

07:24 KST 기존 Text Path census 후보의 실제 제품 회귀 **6/6 · exit 0**을 인수했다. G1 Path-only, V2 본문 파티션 없는 경로, split·compaction, rename·재시작 및 4,096파일·두 OS 재시작 Large를 실행했다. Large는 **118.283초**다. [현행 Text 제품 회귀](</Volumes/Extreme SSD 1/build-cache/nc-text-v2-path-census.jwvgi717/path-regressions.a0dxuj4o/runtime-nextest.log>)

## 먼저 작성할 Hybrid 코드

- [ ] **07.1** **선행 입력** D 오너에게서 승인 Q ELF·설치 선택·Native BOOT·config/policy를 함께 인증하는 기존 계약과 검증 결과를 인수한다. 현재 peer 및 선택 일치 검사는 그 계약의 소비 기반으로 재사용한다.

**07.1 직접 서명 입장 · 04:12 KST.** CURRENT 직접 서명도 기존 D 재개기가 현행 Source에 고정한 승인 요청과 공용 네 역할 빌드 영수증을 검증한 뒤 스테이지를 준비한다. 기존 승인 선택·BOOT·config/policy 발행은 D3 ELF와 동결 S/Q 공급을 기다린다. [03 제어 회귀](03-signed-published-cp2.md#current-소실재개서명-수리--2026-11-22)

**07.1 입장 감사 · 2026-11-22.** D 재개기와 서명기는 동일한 CURRENT 핀에서 provenance와 D3 파일 해시를 확인한 뒤 승인 요청을 처리한다. 공개 VM도 같은 intake의 서명 readback과 Q ELF를 대조한다. 현행 D3 ELF와 승인 요청이 필요한 07.1 발행은 **BLOCKED**다. [CURRENT 입장 감사](03-signed-published-cp2.md#current-소실재개서명-수리--2026-11-22) · [서명 입장](</Users/songmin/Library/Application Support/semantica-codegraph-v2/recovery/p07-runtime-closeout-20261109-01a183c0/feature-on-retry-linux-main-f82f0ca721d3/sdk-structural/rr-internal-controller/published-current-20261115/sign-installed.py:58>)

**07.1 발행·소비 코드 · 2026-11-21 21:50 KST.** 기존 D 서명기는 `P07_NATIVE_Q_APPROVAL_REQUEST`로 승인 요청의 정상 절대 경로를 받는다. 요청은 `deployment_root`, `selection_path`, `selection_sha256`, `decision_policy_path`, `decision_policy_sha256`를 지정한다. Root가 검증한 Native ELF와 launcher의 선택·BOOT 실물 검증을 사용해 기존 배포 키로 같은 active generation·manifest에 서명한다. 실제 게시된 active bytes를 대조한 뒤 readback의 `native_q_approval`을 공급한다. F의 선택된 관리형 캡처 plan `schema_version=2`는 이 값을 `signed_install_approval`로 받아 캡처 입장, 게시 직전, 재생에서 현재 보호된 설치 키·manifest와 대조한다. 승인 요청과 현행 설치는 **BLOCKED**, 실제 Linux 발행·캡처는 **NOT_RUN**이다. [D 승인 발행](</Users/songmin/Library/Application Support/semantica-codegraph-v2/recovery/p07-runtime-closeout-20261109-01a183c0/feature-on-retry-linux-main-f82f0ca721d3/sdk-structural/rr-internal-controller/published-current-20261115/guest_prepare_installed.py:50>) · [승인 검증](../../../../tools/deployment/native_release_approval.py#L146) · [F 관리형 진입](../../../../tools/benchmark/native_file_capture.py#L266)

**07.1 서명 재시도 · 2026-11-21 22:33 KST.** 서명기는 동결 Q 오너의 선택·정책 해시 검사를 스테이지와 볼륨 준비 전에 호출한다. 게스트는 동일 검사와 실제 선택 파서를 사용해 BOOT 앞뒤를 확인한다. 교체된 정책은 볼륨 조회 전에 거절되고 요청과 실물이 일치하면 같은 설치 단계에 재진입한다. Q launcher·승인·캡처·판정 로컬 회귀는 **266 PASS · Linux 전용 4 SKIP · exit 0**이다. 승인 회귀는 실제 선택 파서와 잠금을 사용하고 searchd 프로세스 실행은 테스트 대역으로 제어한다. **07.1 발행 BLOCKED · Linux BOOT·signed capture NOT_RUN**이다. [Q 승인 입력 검사](../../../../tools/deployment/native_release_approval.py#L93) · [D 실행 입장](</Users/songmin/Library/Application Support/semantica-codegraph-v2/recovery/p07-runtime-closeout-20261109-01a183c0/feature-on-retry-linux-main-f82f0ca721d3/sdk-structural/rr-internal-controller/published-current-20261115/sign-installed.py>)

**07.1 현행 서명 입장 · 2026-11-22.** 기존 D 재개기는 완료한 공급의 Q 영수증에 요청을 대조한 뒤 현행 Source에 고정한다. 고정 전에 서명 볼륨 경로 충돌, 실제 선택 ELF·Native config SHA, BOOT 입력 형식과 정책 SHA를 기존 launcher와 대조한다. 잘못된 요청은 고정 전에 거절해 수정한 요청으로 재진입할 수 있다. 제어 회귀 **212/212**, Q 승인 회귀 **5/5**를 통과했다. 요청이 없으면 `WAITING_CURRENT_NATIVE_Q_APPROVAL_REQUEST`에서 멈추고 동일 빌드의 서명을 재개할 수 있다. 서명기는 실제 Q ELF와 선택·정책 SHA를 기존 서명 볼륨 준비 전에 확인한다. 현행 03.6은 같은 서명 readback을 요구한다. **07.1 발행 BLOCKED · Linux BOOT·복구 본문 NOT_RUN.** [서명 오너](</Users/songmin/Library/Application Support/semantica-codegraph-v2/recovery/p07-runtime-closeout-20261109-01a183c0/feature-on-retry-linux-main-f82f0ca721d3/sdk-structural/rr-internal-controller/published-current-20261115/sign-installed.py:43>) · [03.6 기존 실행 계약](03-signed-published-cp2.md)

**07.1 재개 결속 · 23:26 KST.** 기존 D 재개기는 서명 준비 전에 요청을 현행 Source에 고정한다. 프로세스 교체 뒤에도 고정한 승인 요청이 필요하고, 다른 요청·미승인 설치·다른 Q ELF를 거절한다. F 검증기는 실제 설치의 trust 디렉터리 체인과 열어 본 파일의 신원을 확인한다. 기존 F 최종 게시 검사는 정책·ELF·설치 교체 시 게시를 거절한다. [D 요청 고정](</Users/songmin/Library/Application Support/semantica-codegraph-v2/recovery/p07-runtime-closeout-20261109-01a183c0/feature-on-retry-linux-main-f82f0ca721d3/sdk-structural/rr-internal-controller/resume-published-with-source-lease.py>) · [Q 설치 신뢰 검증](../../../../tools/deployment/native_release_approval.py#L103)

**D 설치 신원 결속 · 2026-11-21 20:58 KST.** 기존 D 서명 성공 기록은 실제 active-generation·release manifest 및 원 설치 readback SHA를 고정한다. VM과 signed SDK가 같은 설치 신원을 검증한다. [서명 기록 수리와 실측 입장 상태](03-signed-published-cp2.md)

- [ ] **07.2** **F 작성 범위** 기존 `native_file_runtime` → `native_file_capture` → `native_file_decision`에서 실제 Linux peer와 승인 선택·root·BOOT·config·policy·runner·capture를 같은 설치에 결속한다. 공개 SDK 동일 view·lexical pool·RunStore·replay 판정은 기존 구현을 사용한다. 기존 capture/decision 회귀에서 정상 증거와 누락·foreign·이전 또는 교체된 증거의 typed 거절을 확인한다. [실제 peer](../../../../benchmarks/retrieval/src/native_file_runtime.rs) · [capture](../../../../tools/benchmark/native_file_capture.py#L463) · [decision](../../../../tools/benchmark/retrieval/native_file_decision.py#L131) · [F 설치 결속](../../nov-12-native-cutover/tickets/NC-F-integration.md#f-code-1--hybrid-설치본-결속)

BOOT operation의 boolean epoch와 다른 root에 결속된 selection을 각각 재현했다. 기존 JSON 정규화로 wire 타입을 보존하고, selection 검증 오류는 `NativeDecisionError`로 전달해 CLI exit 2로 거절한다. capture·cohort·decision·CLI·composite binding 회귀 **112/112 VERIFIED**. 승인 Linux 설치본의 실측은 **NOT_RUN**이다.

**07.1/07.2 공급 인수 · 2026-11-21 01:27 KST.** F의 공용 `exit_code` 타입 수리와 RunStore·capture/decision 제어는 관련 Python **383/383 · exit 0**을 실제 확인했다. `python3.12 -B run-caller-current-published-20261115.py --ordinary-symbol-borrow-successor --inspect-source-loss-preimages`는 exit 0이며 `PHYSICAL_SOURCE_MISSING`, manifest SHA256 `32e05a6b70611ea616b43d5bdf2565da3cccab2cbf0214fd931f79275fb3f13c`를 재확인했다. 내부 capture pool은 **5,548,941,312 B · 0.2781%**, 원 50 GiB 및 2.75% 조건에서 exit 75다. Root Q PID 18183은 별도 관측에서 **4,625,936,384 / 54,868,499,559 B**, `WAITING_CAPACITY`다. `just target-gc --dry-run --json`은 회수 0 B였으며 `python3.12 -B quanta_build_cli.py gc --dry-run --max-lanes 12 --json`은 nonempty 정리 가능 lane 0개, 고아 후보 294곳의 target namespace도 모두 부재했다.

기존 Root 큐에서 Q 선행 본문과 용량 조건을 충족한 뒤 위 manifest를 실행 직전에 재확인하고 기존 supersession·capture·review·pin을 사용한다. D는 동일 후보의 승인 Q ELF SHA·선택 파일 SHA/root·Native BOOT 영수증·config/policy를 함께 인증한 원 검증 출력을 공급한다. 현재 원 입력 보관·재캡처, Native ELF·서명 생성 및 실제 Hybrid 본문은 **NOT_RUN**이고 공급은 **BLOCKED**다. S 네 역할의 기존 서명 및 old-Q 참조 ELF는 각자의 기존 범위로 인수한다. [공개 설치 재개 조건](03-signed-published-cp2.md) · [서명 책임 범위](../../nov-12-native-cutover/tickets/NC-F-integration.md#f-code-1--hybrid-설치본-결속)

**07.2 결속 회귀 수리 · 2026-11-21.** RunStore 승격·도메인 재생·프로필 게시·소비에 같은 레코드 digest를 전달한다. Native capture와 replay는 봉인 SHA·크기를 실제 읽은 바이트에 결속하고 게시 직전에 입력을 재확인한다. 품질 판정은 capture와 채점 입력을 같은 바이트에 결속하며, 실제 `runtime_binding.command`가 섞인 코호트를 거절한다. `uv run --frozen --offline --extra dev python -B -m pytest -q -p no:cacheprovider`의 통합 RunStore·Native 재생 **617/617**, `benchctl`·evidence bridge·policy **139/139**, criterion·recorded·retrieval·lexical·pair 생산자 **303/303**이 각각 exit 0이다. 준비된 홀드아웃 12 family·200질의의 입력 파일 SHA 72개를 재확인했다.

`just benchmark-control-contract-local` 첫 실행은 **2,475 통과·5 실패·exit 1**이다. 홀드아웃 proof 목록을 현행 927개 수집 ID와 맞춘 뒤 retrieval proof·source closure·holdout **242/242**, 기존 nextest fixture에 필수 target을 전달한 뒤 nextest/로컬 scope **62/62**가 각각 exit 0이다. Root의 corpus 입력 타입 수리까지 반영해 첫 실행의 원 실패 **5/5·exit 0**을 확인했다. 광역 전체 재실행은 **NOT_RUN**이다.

**07.1 공급 재확인 · 2026-11-21 02:50 KST.** 기존 Root/D에 인계를 요청했다. `post-reboot-9` PID 46003은 입장·본문 0에서 exit 143이고, 후속 `post-reboot-10` PID 18838은 `lexical/WAITING_Q_ADMISSION`, 본문 0이다. 원 Source 조회는 위와 동일한 manifest SHA에서 `PHYSICAL_SOURCE_MISSING`을 반환했다. 실제 재캡처가 쓰는 내부 풀은 **9,381,408,768 B · 0.470%**로 원 기준 미달이다. D의 승인 Native ELF·선택·BOOT·config/policy 원 검증 출력은 **BLOCKED**, 그 출력이 필요한 07.2 서명 Linux 실행은 **NOT_RUN**이다. [Root 선행 큐](01-q-regressions.md#완료-조건)

**07.2 추가 감사와 수리 · 2026-11-21 03:04 KST.** Native 재생은 `pass`인 contract 레코드만 수용한다. 재생 종료와 프로필 게시 직전에 실행 출력의 경로·SHA·크기를 재확인한다. 판정의 family별 경로·raw SHA·크기 및 cohort·policy·설정·source·selection은 동일 RunStore 기록의 결속과 대조한다. v2 `serve-native`는 선택 증거가 필수이며, 계획의 바이너리·config가 선택과 다르면 기동 전에 거절한다. `uv run --frozen --offline --extra dev python -B -m pytest -q -p no:cacheprovider`의 Native capture·cohort·decision·RunStore·CLI·retrieval proof 영향 범위 **797/797·exit 0**, Ruff 및 `git diff --check` exit 0이다. 정상 v2 선택과 변조 거절은 Python fixture 범위다.

동시 관측에서 기존 Root `18838`은 살아 있고 여전히 `lexical/WAITING_Q_ADMISSION`, 본문 0이다. Caller 원본 조회는 같은 SHA에서 `PHYSICAL_SOURCE_MISSING`을 반환했다. D canonical 재캡처 검사 **16,592,183,296 B · 0.832%**, 원 50 GiB·2.75% 미달이다. 승인 Native 빌드 상태·원 로그·서명 영수증은 부재다. **07.1 `BLOCKED` · 07.2 서명 Linux `NOT_RUN`**을 유지한다.

**07.2 입력 읽기 보완 · 2026-11-21 03:13 KST.** 재생은 고정 descriptor에서 봉인 크기를 확인한 뒤 그 길이만 읽고 SHA를 검증한다. 크기가 바뀐 suite/capture의 본문을 읽는 두 사례를 재현했다. 수정 후 같은 크기의 다른 SHA·본문 읽기 전 거절까지 포함해 Native·RunStore·CLI·retrieval proof 영향 범위 Python **800/800·exit 0**이다. 준비 manifest SHA `01ed0e35…`의 12 family에서 실제 SDK 입력 **98,364파일·549,529,919 B**의 목록과 SHA를 전수 대조했다. Root `18838`은 살아 있고 본문 0으로 잠금 대기 중이다. D canonical 재캡처 풀은 **17,298,374,656 B · 0.867%**이며 승인 Native 빌드·서명 공급은 **BLOCKED**다. 이 소스 대조는 SDK 실행 전 입력 검증이다.

**07.2 경로 충돌 수리 · 2026-11-21 03:23 KST.** Native 출력을 프로필 포인터 위치에 두거나 기존 정책을 그 위치에서 읽으면, 이전 게시기가 성공하면서 해당 파일을 덮어썼다. 증거 저장소나 출력을 원 릴리스 아래에 두면 파일을 만든 뒤에야 실패했다. 입장 전에 계획·cohort를 고정하고 RunStore 관리 경로, 원 릴리스 및 Git 소스와 충돌하는 경로를 거절한다. 입장 뒤 실행 직전에도 같은 결속을 대조한다. 충돌 6건의 무쓰기 거절과 독립 `exports/` 정상 게시를 포함해 Python 영향 범위 **807/807·exit 0**, Ruff와 `git diff --check` exit 0이다.

Root `18838`은 살아 있고 `lexical/WAITING_Q_ADMISSION`, 본문 0이다. Caller 조회는 기존 SHA에서 `PHYSICAL_SOURCE_MISSING`, D canonical 재캡처 풀은 **16,087,412,736 B · 0.806%**다. 승인 Q ELF·선택·BOOT·config/policy를 묶은 서명 공급이 확인될 때까지 07.1은 **BLOCKED**, 실제 07.2 서명 Linux는 **NOT_RUN**이다.

**07.2 평가·게시 감사 · 2026-11-21 14:30 KST.** 코호트 재생에 앞서 family별 suite·pack·capture·source universe·binding의 SHA-256과 크기를 고정한다. 같은 바이트를 채점하고, 집계 뒤 manifest·경로·바이트를 재검사한다. 릴리스 경로를 검증기에 그대로 넘겨 심볼릭 링크 거절을 유지한다. Native 평가의 출력은 기존 파일이나 평가 중 다른 실행이 만든 파일을 덮어쓰지 않는다. 관리형 캡처는 끊어진 출력 링크를 입장 전에 거절하고, 재생 종료와 프로필 게시 직전에 선언된 입력 경로와 해시를 다시 확인한다. 수정 전 회귀 13건이 실패했고, 수정 후 `test_native_file_capture.py`와 `test_native_file_cohort.py`는 **75/75 VERIFIED · exit 0**이다. [평가기](../../../../tools/benchmark/retrieval/evaluator.py#L4732) · [관리형 캡처](../../../../tools/benchmark/native_file_capture.py#L255)

평가기 수정 뒤 시작한 Python 15개 파일 확대 회귀는 **1,308/1,308 VERIFIED · exit 0**이다. 관리형 캡처 수리까지 적용한 최종본에서 Native·composite binding·benchctl·프로필 게시 8개 테스트 파일의 `-k 'native or replaced_after_domain_replay'` 선택은 **322/322 VERIFIED · exit 0**이다. 변경한 Python 네 파일의 Ruff와 diff 공백 검사도 exit 0이다.

**07.1 공급 재확인 · 2026-11-21 14:34 KST.** [Root Q 49/49](01-q-regressions.md)와 Linux Q ELF `885152ca2b4d…`의 현행 Source 재사용 영수증을 확인했다. 기존 D 설치기는 같은 Source에서 `BUILDING_PAIRED_SEMANTICA_FOUR_ROLE`이며 서명 영수증이 없다. 동일 후보의 승인 설치 선택·Native BOOT·config/policy 검증 출력이 필요한 **07.1은 BLOCKED**, 실제 **07.2 서명 Linux SDK 캡처·재생은 NOT_RUN**이다. [기존 설치 큐](03-signed-published-cp2.md)

**07.2 v2 config 경로 재검증 · 2026-11-21 14:40 KST.** 공유 HEAD `df090e56a72f45bcd676686c9c1d8437f54d83db`에서 선택·BOOT가 유효한 v2 fixture를 사용했다. 코호트의 config 링크만 동일 SHA-256의 다른 경로로 바꾸자 도메인 재생 직후 RunStore 프로필이 게시되고, 독립 품질 판정도 `quality_admit`을 반환했다. 캡처는 기존 plan↔cohort 런타임 검사를 재사용해 실행 후·재생 후·게시 직전에 적용하고, 판정은 완료 전에 family가 가리키는 config 경로를 채점 시점의 경로와 대조한다. 집중 선택 `daemon_config and (input_drift_after_domain_replay or same_bytes_runtime_path_replacement)`은 수정 전 **2 FAIL · exit 1**, 수정 후 **2/2 VERIFIED · exit 0**이다. RunStore 음성 회귀에서 프로필 포인터가 생성되지 않았다. [캡처·재생](../../../../tools/benchmark/native_file_capture.py#L306) · [판정](../../../../tools/benchmark/retrieval/native_file_decision.py#L777)

```sh
uv run --frozen --offline --extra dev python -B -m pytest -q -p no:cacheprovider \
  tools/ci/tests/test_native_file_cohort.py tools/ci/tests/test_native_file_capture.py \
  tools/ci/tests/test_native_file_evidence.py tools/ci/tests/test_native_file_decision.py \
  tools/ci/tests/test_native_file_decision_cli.py tools/ci/tests/test_composite_source_binding.py \
  tools/ci/tests/test_benchctl.py tools/ci/tests/test_benchmark_profile_capture.py \
  -k 'native or replaced_after_domain_replay'
```

동일 제어 회귀 선택의 최종 결과는 **324/324 VERIFIED · exit 0**이다. 변경 Python 네 파일의 Ruff와 diff 공백 검사도 exit 0이다. 14:40 KST 기준 D 원 상태와 재사용 영수증의 동일 Source를 확인했다. Q ELF SHA-256은 `885152ca2b4d851fb17ee87c7df2d36d478c9da3a54acc5174d08c7f6e388da4`이고 설치는 `BUILDING_PAIRED_SEMANTICA_FOUR_ROLE`이다. `signing-status.json`과 `signed-product-receipt.json`이 부재해 동일 후보의 승인 선택/root·Native BOOT·config/policy 공식 출력은 **BLOCKED**이고 실제 Linux peer·SDK 캡처/RunStore 재생은 **NOT_RUN · 본문 0**이다. [기존 D 설치 상태](</Users/songmin/Library/Application Support/semantica-codegraph-v2/recovery/p07-runtime-closeout-20261109-01a183c0/feature-on-retry-linux-main-f82f0ca721d3/sdk-structural/rr-internal-controller/published-current-20261115/installed-status.json>)

**07.1 D 검증 · 2026-11-21 14:40 KST.** 현행 `ovl-d7610e1d72244d22fd386e7ad0a36205`의 기존 Linux 볼륨에서 Native Q ELF `885152ca2b4d851fb17ee87c7df2d36d478c9da3a54acc5174d08c7f6e388da4`를 실제 해시로 확인했다. 현행 동결본의 S 서명 검증기와 공개 번들 검증기는 S의 CLI·server·SQL worker·parser worker 네 역할을 인증한다. 승인 Q ELF, Q 선택 digest·root, Native BOOT, Q config/policy를 이 서명 설치에 함께 결속할 계약과 원 검증 결과가 현재 경로에 없다. Q launcher의 선택 CAS 검사는 Q의 선택 신원을 조회한다. 기존 S의 번들/서명 검증 회귀는 **3/3 · exit 0**이며 정식 P11 운영 입력은 `staged`다. **07.1 BLOCKED · 현행 합동 서명 본문 0 · exit 미발행**을 유지한다. [현행 Q/S 입력](</Users/songmin/Library/Application Support/semantica-codegraph-v2/recovery/p07-runtime-closeout-20261109-01a183c0/feature-on-retry-linux-main-f82f0ca721d3/sdk-structural/rr-internal-controller/published-current-20261115/published-source-intake-current.json>) · [S 서명 필드](/Users/songmin/Documents/code-new/semantica-codegraph-v2/tools/daemon_release_manifest/installed_linux_trust_v1.py:47) · [번들 역할](/Users/songmin/Documents/code-new/semantica-codegraph-v2/tools/daemon_release_manifest/product_bundle.py:83) · [Q 선택 검사](../../../../tools/deployment/launcher.py#L779) · [P11 계약](../../../operator/p11-operational-proof.md)

**07.1 설치 입력 감사 · 2026-11-21 16:31 KST.** D의 서명 완료 및 VM 입장에서 게스트가 전송한 선택 manifest·원 빌드 영수증·설치 readback을 함께 검증한다. 기존 Q launcher의 선택·BOOT·config 관련 제어는 로컬 **147 PASS · Linux 전용 4 SKIP · exit 0**이다. **07.1 BLOCKED**다. 기존 서명 오너의 검증 출력에 설치의 active-generation 및 release manifest digest와 **승인 Q ELF·선택 digest/root·Native BOOT·config/policy**가 함께 결속돼야 한다. 현재 서명 payload는 S의 네 실행 역할을 인증한다. [D 설치 검증](03-signed-published-cp2.md) · [서명 필드](</Users/songmin/Documents/code-new/semantica-codegraph-v2/tools/daemon_release_manifest/installed_linux_trust_v1.py:50>) · [Q 선택 검사](../../../../tools/deployment/launcher.py#L791)

**07 종료 감사 · 2026-11-21 15:56 KST.** 앞선 두 감사의 중복을 합친 코드 지적 4건은 현행 RunStore digest 결속, 보관 입력 SHA·크기, 채점 원본 결속, v2 실행 모드 검사 및 후속 입력 경로 회귀에 대응한다. `UV_OFFLINE=1 PYTHONDONTWRITEBYTECODE=1 env -u PYTEST_ADDOPTS uv run --frozen --offline --extra dev just benchmark-control-contract-local` 전체는 **2,611/2,611 VERIFIED · exit 0 · 32분 37초**다. 첫 시도에 추가한 `PYTEST_ADDOPTS`가 제어 증거의 환경 검사에 거절돼 해당 실행을 중단했다. 옵션을 제거한 공식 실행의 결과를 인수한다. 실행 뒤 현행 HEAD `d8a490565`에서 위 8개 파일의 Native·게시 선택 **324/324**, `test_benchmark_policy.py` **14/14**를 다시 확인했다. 두 선택 모두 exit 0이다.

**남은 제품 수용.** Q ELF 공급 이후 기존 D의 private late-peak 진단은 통과했다. 기존 설치기의 exit 137은 retained 수리본으로 전환하기 위한 Root의 종료 기록이다. 후속 `ovl-a8e2aace…` 등록은 기존 잠금과 용량 조건을 기다리며 설치·서명 산출물이 없다. 동일 후보 Q ELF·승인 선택/root·Native BOOT·config/policy 공식 검증 출력 인수 **07.1 BLOCKED**, 해당 출력을 연결한 실제 signed Linux peer·SDK capture·RunStore 재생 **07.2 NOT_RUN · 본문 0**이다. [D 후속 큐](03-signed-published-cp2.md#완료-조건)

**07.2 코드 정적 감사·수리 · 2026-11-21 16:17 KST.** HEAD `d8a490565`의 작업 트리에서 Native peer → `benchctl run` → RunStore 재생·게시 → 판정 호출을 대조했다.

- **P1 수리 완료.** 보관된 정책의 SHA와 실행 기록이 일치해도 다른 코호트의 정책이면 재생이 거절해야 한다. 캡처와 직접 판정에 있던 정책 검증기를 봉인된 정책 바이트로 재생할 때도 사용한다. 프로필 게시, CLI 재생·검증, 판정의 controlled capture 소비가 같은 재생 경계를 지난다. [보관 정책 검증](../../../../tools/benchmark/native_file_capture.py#L517) · [정책 소유자](../../../../tools/benchmark/retrieval/native_file_decision.py#L67)
- **P2 수리 완료.** 게시 재생 직후나 최종 해시 검사 도중 Git 소스 링크가 다른 경로를 가리켜도 게시할 수 있었다. 입장에 사용한 Git·suite·pack·source 문서·release 경로를 실행 명령과 재생에 전달한다. 종료 시 봉인 바이트를 검사한 뒤 선언 경로를 다시 확인한다. 두 교체 사례 모두 프로필 포인터 없이 거절한다. [입력 경로 검사](../../../../tools/benchmark/native_file_capture.py#L312) · [재생 입력](../../../../tools/benchmark/native_file_capture.py#L378)

다른 코호트 정책과 Git 링크 교체 두 시점의 회귀는 수정 전 세 사례에서 각각 거절 실패를 확인했다. 최종 아래 선택은 **327/327 VERIFIED · exit 0**이다. Ruff와 `git diff --check`도 exit 0이다. [실행 안내](../../../../tools/benchmark/retrieval/README.md#explicit-native-file-profile)

```sh
uv run --frozen --offline --extra dev python -B -m pytest -q -p no:cacheprovider \
  tools/ci/tests/test_native_file_cohort.py tools/ci/tests/test_native_file_capture.py \
  tools/ci/tests/test_native_file_evidence.py tools/ci/tests/test_native_file_decision.py \
  tools/ci/tests/test_native_file_decision_cli.py tools/ci/tests/test_composite_source_binding.py \
  tools/ci/tests/test_benchctl.py tools/ci/tests/test_benchmark_profile_capture.py \
  -k 'native or replaced_after_domain_replay'
```

**제품 수용 잔여.** 기존 04 로컬 Offline Custody는 공용 잠금 입장 후 컴파일 중이다. 03의 외장 Linux 실행 입력과 같은 후보의 서명 설치·선택·BOOT·설정/정책 검증 출력 인수는 **BLOCKED**다. 07.2의 실제 서명 Linux SDK 캡처·재생과 독립 홀드아웃 판정은 **NOT_RUN**이다. [D 공급 상태](03-signed-published-cp2.md#완료-조건) · [04 실행 범위](04-maintained-test-migration.md)

**07.1·07.2 소비 인수 상태 · 2026-11-21 16:31 KST.** 기존 D readback의 active generation과 release manifest digest는 S 네 실행 역할의 설치를 검증한다. Q ELF·승인 선택/root·Native BOOT·config/policy까지 묶은 현행 후보의 검증 출력은 공급 원 경로에서 확인되지 않았다. 해당 경로의 서명 상태·영수증·일반/retained 서명 입력이 모두 부재하다. **07.1 BLOCKED**다. F의 기존 정책·경로·RunStore 회귀 원 terminal은 **327/327 · exit 0**이다. 기존 `benchctl run native-file-capture --native-plan … --evidence-root …`와 `benchctl replay <run-id> --evidence-root …` 진입점을 대조했다. D 출력 연결은 **BLOCKED**, 실제 서명 Linux 실행·재생은 **07.2 NOT_RUN · 본문 0**이다. [D 공급](03-signed-published-cp2.md#완료-조건) · [캡처 계약](../../../../tools/benchmark/retrieval/README.md#explicit-native-file-profile)

홀드아웃 입력은 12개 조합·200개 질의까지 준비됐다. 기존 Linux 실행 오너가 같은 Q ELF와 모델·예산 설정을 공급하면, 각 조합의 별도 root·socket에서 `native_corpus`의 Semantic seed를 한 번 실행한다. 성공한 `result.json`의 `capture_view`와 실제 instance ID·config SHA를 cohort v2와 기존 `--native-plan`에 결속한다. `source-ack.json`은 Semantic coverage 전에 기록되므로 캡처 view로 사용할 수 없다. 기존 capture를 보존한 뒤 독립 qrels를 모으고 같은 원본으로 채점한다. Q ELF의 컴파일 증거를 확인했다. 승인된 서명 Linux 설치와 해당 설치의 성공 seed 영수증을 기다리며 실행은 **NOT_RUN**이다. [실제 준비 입력](</Volumes/Extreme SSD 1/build-cache/repoqa-f-eval-git-ll7r_mmy/native-review-input-v1/prepared-inputs.json>) · [캡처 계약](../../../../tools/benchmark/retrieval/README.md#explicit-native-file-profile)

11월 21일 실제 Git fixture의 blind pack에서 `top_k`를 `10`에서 `10.0`으로 바꾸고 원본 바이트 SHA를 갱신했을 때, Python 객체 비교가 이를 원 계약과 같은 값으로 수용했다. Native 재생기는 원본 SHA 검사를 유지하고 JSON 타입을 보존하는 정규화 바이트로 동결 pack을 대조한다. 신규 회귀는 수정 전 **FAILED**, 수정 후 증거·판정·review CLI **330/330 VERIFIED · exit 0**이다. 실제 SDK 홀드아웃 점수는 **NOT_RUN**이다. [동결 pack 결속](../../../../tools/benchmark/retrieval/evaluator.py#L4638) · [거절 회귀](../../../../tools/ci/tests/test_native_file_evidence.py#L1615)

**07.2 동결 입력 보호 · 2026-11-21 20:58 KST.** 관리형 캡처 입장·게시·보관 재생과 단일·코호트 평가 및 품질 판정이 공통 경로 검사를 사용한다. 전체 코호트의 Git·release·입력 파일을 캡처와 RunStore 쓰기에서 보호하고, 평가·판정의 보고서 출력은 해당 입력이 없는 경로에 둔다. 디렉터리 신원을 대조해 대소문자 별칭도 거절한다. 게시와 평가·판정 종료에는 기존 Git 검증기로 동결 commit과 작업 트리 상태를 확인하고, 순환 링크를 입력 오류로 변환한다. [공통 경로 검사](../../../../tools/benchmark/retrieval/native_file_paths.py#L47) · [게시 전 Git 확인](../../../../tools/benchmark/native_file_capture.py#L891) · [운영 안내](../../../../tools/benchmark/retrieval/README.md#explicit-native-file-profile)

**07.1 D 공급 재확인 · 2026-11-21 20:58 KST.** 기존 D 경로에서 서명 상태, 제품 영수증, 일반·retained 설치 입력이 부재하고 외장 볼륨은 미마운트다. D가 같은 후보의 승인 Q ELF·선택/root·Native BOOT·config/policy를 설치 증거에 결속할 때까지 **07.1 BLOCKED**, 실제 서명 Linux SDK 캡처·재생 **07.2 NOT_RUN · 본문 0**이다. [기존 D 설치 경로](</Users/songmin/Library/Application Support/semantica-codegraph-v2/recovery/p07-runtime-closeout-20261109-01a183c0/feature-on-retry-linux-main-f82f0ca721d3/sdk-structural/rr-internal-controller/published-current-20261115/installed-status.json>)

**회귀 검증.** 아래 선택은 **691/691 VERIFIED · exit 0**이다. 배치에는 실제 임시 Git의 HEAD 변경·대소문자 경로 별칭·출력 상위 경로 교체, 다른 family의 Git·release로의 쓰기, 보관 재생, 세 CLI의 순환 링크 거절이 포함된다. Rust SDK 응답을 대체한 회귀와 실제 Git 검사를 함께 사용했다. Ruff와 `git diff --check`는 exit 0이다.

```sh
PYTHONDONTWRITEBYTECODE=1 uv run --frozen --offline --extra dev python -B -m pytest -q -p no:cacheprovider \
  tools/ci/tests/test_native_file_{capture,cohort,decision,decision_cli,evidence}.py \
  tools/ci/tests/test_{composite_source_binding,benchctl,benchmark_profile_capture}.py \
  tools/ci/tests/test_{benchmark_source_closure,benchmark_policy,holdout_review}.py --tb=short
```

**07.2 게시 순서·CLI 재감사 · 2026-11-21.** 재생 뒤 첫 RunStore 재조회 또는 최종 manifest 검증 도중 실제 Git HEAD가 바뀌면, 이전 경계는 프로필과 캡처 레코드를 둘 다 게시했다. 이제 봉인 manifest와 RunStore 원본을 먼저 재검증하고, 동결 Git을 마지막에 확인한 뒤 캡처 레코드와 프로필을 쓴다. `run`·`validate`·`summarize`·`replay`는 순환 `--evidence-root`를 입력 오류로 반환한다. 6개 재현이 변경 전에 실패했다. 관련 Python 회귀 **333/333 VERIFIED · exit 0**, Ruff와 diff 검사도 통과했다. 관리형 캡처 회귀는 실제 Git과 SDK fixture를 사용했다. [게시 경계](../../../../tools/benchmark/profile_capture.py#L560) · [Native Git 검사](../../../../tools/benchmark/native_file_capture.py#L891) · [CLI 입력 검사](../../../../tools/benchmark/benchctl.py)

```sh
PYTHONDONTWRITEBYTECODE=1 uv run --frozen --offline --extra dev python -B -m pytest -q -p no:cacheprovider \
  tools/ci/tests/test_native_file_capture.py tools/ci/tests/test_benchctl.py \
  tools/ci/tests/test_benchmark_profile_capture.py tools/ci/tests/test_native_file_cohort.py \
  tools/ci/tests/test_native_file_decision.py tools/ci/tests/test_native_file_decision_cli.py --tb=short
```

**07.2 서명 설치 수용 · 2026-11-21.** `--require-signed-install`은 캡처·검증·요약·RunStore 재생에 승인된 v2 실행 계획을 요구한다. 재생은 원 게시 manifest에 기록된 단일 run ID와 digest를 대조한다. 품질 판정은 각 family의 실제 채점 입력과 보관 캡처를 대조하고 동일 active generation·release manifest·Q ELF를 요구한다. 캡처 게시와 서명 판정 직전에 설치를 다시 검증한다. 승인 모듈과 회귀는 벤치마크 소스 봉인 및 공식 로컬 테스트 선택에 포함된다. [운영 안내](../../../../tools/benchmark/retrieval/README.md#explicit-native-file-profile) · [게시 경계](../../../../tools/benchmark/native_file_capture.py#L946) · [품질 판정 경계](../../../../tools/benchmark/retrieval/native_file_decision.py#L879) · [테스트 등록](../../../../tools/ci/test-authority.toml#L688)

**집중 회귀.** `d8a490565e6f` 기반 작업 트리에서 아래 선택은 **391/391 VERIFIED · exit 0**이다. 서명 검증은 테스트 키와 설치 fixture로 검사했다. 첫 실행에서 실제 프로듀서의 비정상 종료와 SIGTERM 회귀 두 건이 호스트 부하 중 1초 제한에 걸렸다. 타임아웃 회귀에만 1초 제한을 두고 비정상 종료 코드를 `7`로 검증한 뒤 전체 선택을 다시 통과했다. Ruff, 테스트 등록 검사, `git diff --check`도 통과했다.

```sh
PYTHONDONTWRITEBYTECODE=1 uv run --frozen --offline --no-sync --extra dev python -B -m pytest -q -p no:cacheprovider \
  tools/ci/tests/test_benchctl.py tools/ci/tests/test_native_file_capture.py \
  tools/ci/tests/test_native_file_decision.py tools/ci/tests/test_native_file_decision_cli.py \
  tools/ci/tests/test_native_release_approval.py tools/ci/tests/test_benchmark_profile_capture.py \
  tools/ci/tests/test_benchmark_source_closure.py --tb=short
```

**제품 실행 잔여 · 23:26 KST.** 기존 D 재개는 프로세스 교체 후에도 Source에 고정된 Q 승인 요청을 검증한다. 재마운트된 기존 Q 동결본에는 승인 모듈과 D3 수리가 반영되지 않았다. 현행 S/Q 재캡처, 같은 설치의 승인 발행과 증거 인수는 **07.1 BLOCKED**다. 실제 서명 Linux SDK 캡처·RunStore 재생은 **07.2 NOT_RUN · 본문 0**이다. [기존 D 설치 큐](03-signed-published-cp2.md#완료-조건) · [F 공급·소비 경계](../../nov-12-native-cutover/tickets/NC-F-integration.md#f-code-1--hybrid-설치본-결속)

**07.2 최종 게시 재감사 · 23:20 KST.** 최종 RunStore 재조회 중 정책 파일이나 캡처 출력이 바뀌면 프로필이 게시되고 뒤이은 재생이 거절됐다. 이제 최종 RunStore와 manifest 검증 뒤, 게시 전에 동결 경로·바이트·출력·서명 설치·Git을 모두 검사한다. 두 재현 모두 수정 전 실패했고, 위의 7개 모듈은 수정 후 **396/396 VERIFIED · exit 0**이다. 서명 정책과 Q ELF의 최종 변경 거절도 포함한다. Ruff, 테스트 등록 검사, `git diff --check`는 통과했다. [공용 게시 경계](../../../../tools/benchmark/profile_capture.py#L474) · [Native 종료 검사](../../../../tools/benchmark/native_file_capture.py#L932) · [재현 회귀](../../../../tools/ci/tests/test_native_file_capture.py#L1062)

**07.1 외장 볼륨 복귀 · 23:20 KST.** `Extreme SSD 1`은 91 GiB를 남기고 마운트됐으며 기존 Product `e98501f2…+ovl-d7610e1d…`의 S/Q 원본 경로가 모두 존재한다. 지정된 동결 Q와 S 옆 Q 양쪽에서 07.1 승인 모듈이 부재했다. 기존 설치 상태는 `PAIRED_SEMANTICA_BUILD_FAILED · exit 137`이고 현행 서명 영수증·설치 입력은 부재하다. 기존 D 큐에 동일 후보 S/Q 동결본 및 공급 입장 재확인을 요청했다. 23:28 KST 내부 여유는 약 33 GiB로 기존 50 GiB·2.75% 입장 조건에 미달했다. **07.1 BLOCKED**, **07.2 NOT_RUN · 본문 0**이다. [기존 D 설치 큐](03-signed-published-cp2.md#완료-조건)

**07.1 공급 재확인 · 2026-11-22.** 외장 SSD는 마운트돼 있다. 현행 D intake가 가리키는 S 동결 경로와 그 상위 디렉터리는 `ENOENT`이고, 남아 있는 Q 동결본에는 승인 모듈이 없다. 내부 여유 약 31 GiB는 기존 입장 기준에 미달하며 공식 서명 영수증·설치 입력은 부재하다. 현행 S/Q 재동결과 공급은 **BLOCKED**, 실제 서명 Linux SDK 캡처·RunStore 재생은 **NOT_RUN · 본문 0**이다. [기존 D intake](</Users/songmin/Library/Application Support/semantica-codegraph-v2/recovery/p07-runtime-closeout-20261109-01a183c0/feature-on-retry-linux-main-f82f0ca721d3/sdk-structural/rr-internal-controller/published-current-20261115/published-source-intake-current.json>)

**07.2 공통 Git 검증 재감사 · 2026-11-22.** 깨끗한 작업 트리 상태 조회 직후 HEAD가 바뀌거나, 동일 HEAD의 수정된 체크아웃이 같은 경로로 들어오는 두 사례를 실제 임시 Git으로 재현했다. 공통 검증기는 상태 조회 뒤 HEAD를 다시 읽고 검사 전후의 체크아웃 장치·inode를 대조한다. 두 사례는 수정 전 거절 실패를 확인했다. 수정 후 `d8a490565e6f` 기반 작업 트리의 아래 선택은 **565/565 VERIFIED · exit 0**이다. Ruff, 테스트 등록 검사, `git diff --check`도 통과했다. [공통 Git 검증](../../../../tools/benchmark/retrieval/retrieval_contract.py#L194) · [실제 Git 회귀](../../../../tools/ci/tests/test_native_file_evidence.py#L1669)

```bash
PYTHONDONTWRITEBYTECODE=1 uv run --frozen --offline --no-sync --extra dev python -B -m pytest -q -p no:cacheprovider \
  tools/ci/tests/test_{native_file_evidence,native_file_capture,native_file_cohort,native_file_decision,native_file_decision_cli,pair_capture,code_search_workflow,retrieval_contract_proof,benchmark_source_closure}.py --tb=short
```

## SDK60와 Large

04:51 KST 기존 TEXT V2의 `large-v2.a3lt_99w`는 동결본 `segmented_layered.rs`의 `serde_json::json!([77_u8; 16])`에서 **FAILED · compile 101 · 본문 0**으로 끝났다. 현행 파일은 제품 계약 fixture의 `format`을 `serde_json::to_value`로 변환해 사용한다. 이 수정의 Large Rust 본문은 **NOT_RUN**이다. Record intake는 Caddy `ingested.json` 부재로 exit 1, Off는 기존 Record 결과 부적합으로 exit 1이다. 기존 TEXT 오너의 다음 원 실행을 수용한다. [Large 원 로그](</Volumes/Extreme SSD 1/build-cache/nc-text-v2-postboot.MHHgvS/large-v2.a3lt_99w/nextest.log>) · [현행 fixture](../../../../crates/quanta-index-searchd-runtime/tests/native_product_process/segmented_layered.rs#L81)

05:09 KST 후속 V2 동결본 `release-cohort.g_jaq9uc`에서 release 빌드 **exit 0**, Caddy Record **VERIFIED**다. etcd Record는 FULL `BuildPrepare`에서 mutations **999,969 + 182 > 1,000,000**으로 `ResourceLimited`, 본문 **FAILED · exit 1**이다. 첫 거절은 Catalog의 `managed_install.rs:108`이며 원 코퍼스와 네 축 상한을 유지한다. 이 바이너리의 Clean Off와 Large 완주 판정은 아직 미확인이다. [Record 원 terminal](</Volumes/Extreme SSD 1/build-cache/nc-text-v2-inline.3ENZPq/release/record-status.json>) · [etcd 원 거절](</Volumes/Extreme SSD 1/build-cache/nc-text-v2-inline.3ENZPq/release/etcd-record-1/first.stderr>)

05:28 KST 동일 정규화 경로의 대소문자 posting을 공유한 V2 동결본 `release-cohort.7nq8pv_u`에서 **Caddy 590·etcd 1,421 원 SDK Record 모두 VERIFIED · exit 0**이다. 두 결과의 `segmented-layered-v2`와 데몬 SHA `43072380…`가 일치한다. 원 terminal에 결속된 각 결과에서 submit→게시 시간은 Caddy **3.455초**, etcd **7.763초**이고 stage→검색 가능 시간은 각각 **5.248초**, **11.311초**다. 각 trace ingest의 종료 요청 2건은 모두 `ok`다. etcd FULL 사용량은 **497,183,698 B·990,909 mutations**, 잔여 **39,687,214 B·9,091 mutations**였다. 같은 실행의 Clean Off는 host load **30.51 > 8.0**로 **BLOCKED · exit 75**다. [Record 원 terminal](</Volumes/Extreme SSD 1/build-cache/nc-text-v2-shared-case.ArZMh8/release/record-status.json>) · [Caddy 원 결과](</Volumes/Extreme SSD 1/build-cache/nc-text-v2-shared-case.ArZMh8/release/caddy-record-1/result.json>) · [etcd 원 결과](</Volumes/Extreme SSD 1/build-cache/nc-text-v2-shared-case.ArZMh8/release/etcd-record-1/result.json>) · [Off 입장 실패](</Volumes/Extreme SSD 1/build-cache/nc-text-v2-shared-case.ArZMh8/off.log>)

- [ ] **07.3** 원본 Caddy 590·etcd 1,421파일에서 동일 최종 바이너리로 각각 **Off 2회·Record 1회**, FULL→조회→OS 재오픈을 수행한다. 원 **30,000 물리 파일·512 MiB WorkBytes·R/W/M 각각 100만·SDK 60초**를 유지한다. 단계별 최초 거절·ACK·동일 Source digest·trace 무누락과 네 축의 최종 사용량을 기록한다. R56 Off는 부하 입장에 막혔고 양쪽 Record는 동결 후보에서 통과했다. [TEXT 실행 결과](../../nov-12-native-cutover/tickets/NC-TEXT-build-final.md#admission-decision-and-remaining-product-verification)
- [ ] **07.4** 원본 Large **4,096파일, 두 OS 재시작**, 동일 daemon SHA와 전체 Source·소유자·저장소별/전역 순위·SDK 시간을 확인한다. 기존 R56 1/1은 그 daemon과 결속된 근거로 유지하고, 최종 후보에서 입력·바이너리가 달라진 수용을 실행한다. [Large 계약](../../nov-12-native-cutover/DOD.md#기존-고정-성능-및-자원-기준)

05:38 KST 동일 shared-case V2 동결본의 실제 Large **VERIFIED · 1/1 · exit 0**이다. Nextest `8fe1c13c-d3da-4000-8cc2-7bffde29391f`에서 `native_large_scoped_corpus_two_process_restarts_preserve_sources_and_repo_ranks`가 **69.845초**에 통과했다. 원 4,096파일과 두 OS 재시작을 실행한 선택이다. 결과가 실행 전후 Source/daemon 결속을 유지했고 공유 잠금을 반환했다. Clean Off는 별도 호스트 부하 gate 대기다. [Large 원 terminal](</Volumes/Extreme SSD 1/build-cache/nc-text-v2-shared-case.ArZMh8/large-v2.is25udy9/result.json>)

11월 21일 Native Text `fix1` release Nextest는 첫 Text 진단 6건을 해소한 뒤 **FAILED · exit 101 · 본문 0**으로 종료했다. ANN 부모 모듈에서 삭제한 `interruptible`을 `row_attestation.rs`가 가져와 E0432와 후속 E0282 6건이 발생했다. 기존 B 작업 트리에서 row attestation을 동일 읽기 완료 경계인 `settle_query_batches`로 연결한 후속 소스를 확인했다. B의 원 Semantic14와 TEXT의 동일 selector 후속 본문은 **NOT_RUN**이다. [원 컴파일 로그](</Volumes/Extreme SSD 1/build-cache/nc-text-v2-postboot.MHHgvS/fix1/nextest.log>) · [후속 호출](../../../../crates/quanta-index-semantic/src/native/ann_lance/row_attestation.rs#L15)

11월 21일 20:41 KST TEXT 재검증의 내부 동결본은 Main `d8a490565` 입력 3,160개와 경로 계측·SDK·하네스 수리 파일 4개의 검증된 SHA를 포함한다. 원 Caddy 590개·etcd 1,421개 manifest와 모든 파일을 다시 대조했다. 외장 SSD 연결이 대기 중 끊겨 내부 SSD에서 `./scripts/cargow --lane test-workspace-lane build --release --locked --offline --no-default-features --message-format=json-render-diagnostics -p quanta-index-searchd-runtime --bin quanta-index-searchd -p quanta-index-searchd-harness --bin native_corpus`를 접수했다. 공용 잠금을 3,600초 동안 얻지 못해 **BLOCKED · exit 124 · Cargo 이벤트 0**으로 종료했다. [빌드 terminal](/Users/songmin/Library/Caches/quanta-index/nc-text-v2-main-requalify.s4rxp934/release/build.stderr) · [동결 입력](/Users/songmin/Library/Caches/quanta-index/nc-text-v2-main-requalify.s4rxp934/release/source-before.json)

512 MiB·기존 요금 조건의 원본 FULL·조회·재오픈, etcd 최종 Work와 잔여량, `large_restart::native_large_scoped_corpus_two_process_restarts_preserve_sources_and_repo_ranks` 본문은 **NOT_RUN**이다. 호스트 사전검사는 **1분 평균 부하 238.37/상한 8.0, 외부 Rust 6개**로 거절됐으며 quiet Off는 양쪽 0/2회다. 최종 06.4 후보 공급과 07.3·07.4 수용은 대기 상태다. [호스트 검사](/Users/songmin/Library/Caches/quanta-index/nc-text-v2-main-requalify.s4rxp934/host-gate-at-build-timeout.json)

11월 21일 21:05 KST 같은 release 빌드가 공용 슬롯에 입장했다. 입력 3,162개의 첫 실행은 Native DSL의 비공개 `readiness::runtime_state` 경유에서 **FAILED · E0603 · exit 101**이었다. 가시성을 crate 내부로 열었고, 입력 3,166개의 후속 실행에서 Search Plane 라이브러리 산출물을 확인했다. 이 후속 빌드는 Native-only에서 Legacy 호출 전용인 `entry_names_at`의 `dead_code`로 **FAILED · exit 101**이었다. 함수와 관련 import를 기존 `legacy-tantivy` 조건에 맞춰 다음 실행을 접수했다. 두 실행의 원본 SDK·Large 본문은 0건이다. [E0603 로그](/Users/songmin/Library/Caches/quanta-index/nc-text-v2-main-admission.fyvyi2h1/release/build.stderr) · [Legacy 전용 함수 로그](/Users/songmin/Library/Caches/quanta-index/nc-text-v2-main-admission.jn7z_o_k/release/build.stderr) · [후속 입장](/Users/songmin/Library/Caches/quanta-index/nc-text-v2-main-admission.mxq1x13w/admission.stderr)

21:12 KST 후속 3,166개 입력에서 Search Plane과 Lexical을 새로 컴파일했다. Kernel은 `projection.rs`의 테스트 전용 `SourceCut` import가 생산 모듈에 남아 **FAILED · exit 101**이었다. import를 사용하는 테스트로 옮겼고 같은 release selector를 재접수했다. 이 회차까지 원본 SDK·Large 본문은 0건이다. [Kernel 컴파일 로그](/Users/songmin/Library/Caches/quanta-index/nc-text-v2-main-admission.mxq1x13w/release/build.stderr) · [후속 입장](/Users/songmin/Library/Caches/quanta-index/nc-text-v2-main-admission.9c2weypn/admission.stderr)

21:25 KST 텍스트 빌드에서 Kernel 산출물을 확인했다. Runtime은 DSL의 미사용 import와 이동된 범위 재사용, Object Read의 중복 패턴에서 **FAILED · exit 101**이었다. 해당 수정이 live Main에 들어온 것을 대조한 후 이전 실패 소스를 보존하고 같은 빌드 경로에서 다시 실행했다. 후속 동결본은 Runtime의 DSL 미사용 결과 두 건에서 **FAILED · exit 101**이었다. 이 두 항목의 담당 변경도 live Main에서 확인해 다음 release를 기존 슬롯에 접수했다. Large 검증기의 별도 입장 경로는 소스·바이너리 SHA 검사를 유지하며 재사용 소스 경로를 인식하도록 연결했다. 이번 후보의 원본 SDK·Large 본문은 아직 0건이다. [Kernel 이후 Runtime 로그](/Users/songmin/Library/Caches/quanta-index/nc-text-v2-main-admission.9c2weypn/release/build.stderr) · [후속 Runtime 로그](/Users/songmin/Library/Caches/quanta-index/nc-text-v2-main-admission.9n_rfd77/release/build.stderr) · [다음 입장](/Users/songmin/Library/Caches/quanta-index/nc-text-v2-main-admission.jsf2i3bv/admission.stderr)

21:52 KST Main `d8a490565` 입력 3,167개를 결속한 릴리스 빌드 **VERIFIED · exit 0 · 6분 33초**. 데몬 SHA `240ccfb6…`의 원본 Caddy 590·etcd 1,421파일은 SDK FULL·검색·OS 재오픈과 두 trace 검증 모두 **VERIFIED**다. FULL ACK는 각각 4.499초와 9.281초, Literal·Text는 검색과 재오픈 뒤 각각 1 hit다. 최초 trace 검증의 의존 파일 누락은 검증기 사본에 SHA가 고정된 helper를 추가해 복구했다. 저장된 원 trace만 다시 검증했고 원 SDK 본문은 각각 1회다. etcd 최종 사용량은 **497,183,870 B·990,909 mutations**, 잔여는 **39,687,042 B·9,091 mutations**다. 두 FULL trace의 `NativeTextPostings/49`는 각 1회 완료했다. 이 후보의 Large는 실행 중이고, quiet Off와 06.4 최종 후보 수용은 남아 있다. [릴리스 결속](/Users/songmin/Library/Caches/quanta-index/nc-text-v2-main-admission.jsf2i3bv/release/pair.json) · [Record 원장](/Users/songmin/Library/Caches/quanta-index/nc-text-v2-main-admission.jsf2i3bv/records.json)

22:07 KST 같은 데몬의 첫 Large는 `ENOSPC`로 **FAILED · exit 101 · 본문 0**이었다. 두 번째 `large_restart::native_large_scoped_corpus_two_process_restarts_preserve_sources_and_repo_ranks`는 컴파일을 통과해 본문 1건을 시작했지만 종료 기록 없이 프로세스가 사라졌다. 이전 실행이 종료된 것을 확인한 뒤 같은 동결 소스와 데몬으로 다시 접수했다. quiet Off의 재입장은 1분 평균 부하 **109.91 / 상한 8.0**, 외부 Rust **3개**로 **BLOCKED · exit 75**였고 누적 통과는 각 0/2회다. [디스크 부족 terminal](/Users/songmin/Library/Caches/quanta-index/nc-text-v2-main-admission.jsf2i3bv/large-v2.j58dfak_/result.json) · [중단 로그](/Users/songmin/Library/Caches/quanta-index/nc-text-v2-main-admission.jsf2i3bv/large-v2.95ddnllk/nextest.log) · [재실행 로그](/Users/songmin/Library/Caches/quanta-index/nc-text-v2-main-admission.jsf2i3bv/large-v2.d62tuyor/nextest.log) · [quiet Off 원장](/Users/songmin/Library/Caches/quanta-index/nc-text-v2-main-admission.jsf2i3bv/off.json)

22:41 KST 같은 동결 Main과 릴리스 데몬으로 Large **VERIFIED · 1/1 · exit 0 · 172.802초**. 4,096파일의 소스와 repo 순위가 두 번의 데몬 프로세스 재시작 뒤에도 유지되는지를 확인했다. 실행 전후 동결 입력과 릴리스 바이너리 SHA가 일치했다. 이번 결과는 동결 후보의 07.4 본문 검증이다. 06.4 최종 후보 수용과 Caddy·etcd quiet Off 각 2회는 남아 있다. [Large terminal](/Users/songmin/Library/Caches/quanta-index/nc-text-v2-main-admission.jsf2i3bv/large-v2.d62tuyor/result.json) · [Nextest 로그](/Users/songmin/Library/Caches/quanta-index/nc-text-v2-main-admission.jsf2i3bv/large-v2.d62tuyor/nextest.log)

23:11 KST Large 종료 후 quiet Off 입장을 다시 확인했다. **BLOCKED · Caddy 0/2 · etcd 0/2**. 1분 평균 부하 **390.72 / 상한 8.0**과 외부 Rust **1개**가 확인됐으며 Off 본문은 시작하지 않았다. [호스트 사전검사](/Users/songmin/Library/Caches/quanta-index/nc-text-v2-main-admission.jsf2i3bv/host-after-large-final.json) · [quiet Off 원장](/Users/songmin/Library/Caches/quanta-index/nc-text-v2-main-admission.jsf2i3bv/off.json)

23:40 KST 같은 릴리스의 quiet Off 입장을 20분간 재확인했다. 호스트 1분 부하 **216.39 / 상한 8.0**, 외부 Rust **3개**로 대기가 **BLOCKED · exit 75**로 종료됐다. Off 본문은 시작하지 않았고 Caddy·etcd 통과는 각각 **0/2**다. 기존 Record·Large 결과를 재사용했다. [재입장 종료](/Users/songmin/Library/Caches/quanta-index/nc-text-v2-main-admission.jsf2i3bv/quiet-resume.g55ho0di/terminal.json) · [마지막 사전검사](/Users/songmin/Library/Caches/quanta-index/nc-text-v2-main-admission.jsf2i3bv/quiet-resume.g55ho0di/readiness.json)

11월 22일 후속 리뷰에서 quiet Off 재개에 필요한 회차별 증거를 보강했다. 보관된 두 Record의 원본 trace·검증 결과·Work를 다시 대조하고 SDK 재실행 없이 SHA를 고정했다. Off 회차 수용은 동일 호스트의 입장 전·공유 잠금 보유 중·실행 후 영수증과 SDK 원본 결과를 재검사한다. 정상 수용과 증거 누락·변경·서로 다른 호스트·잘못된 부하 상한을 포함한 반례 5건을 검증했다. Large 테스트는 성공한 FULL 게시 ACK 시간을 전체 본문 시간과 별개로 기록한다. [Off 검증기](/Users/songmin/Library/Caches/quanta-index/nc-text-v2-main-admission.jsf2i3bv/qualify.py) · [Large ACK 기록](../../../../crates/quanta-index-searchd-runtime/tests/native_product_process/large_restart.rs#L653)

11월 22일 01:16 KST 보강한 실행기로 quiet 호스트 입장을 20분간 확인했다. 마지막 1분 부하 **69.84 / 상한 8.0**, 외부 Rust **4개**로 **BLOCKED · exit 75**로 종료됐다. Caddy·etcd Off는 각각 **0/2**다. 기존 동결 소스에 TEXT scratch 계측과 Large ACK 테스트 파일 두 개를 얹은 집중 회귀는 공유 빌드 잠금을 기다리고 있다. [quiet 입장 종료](/Users/songmin/Library/Caches/quanta-index/nc-text-v2-main-admission.jsf2i3bv/quiet-post-review.27h2yob3/terminal.json) · [집중 회귀 입장 로그](/Users/songmin/Library/Caches/quanta-index/nc-text-v2-main-admission.jsf2i3bv/proof-repair.4pps9jzo/lexical.log)

## ANN과 Hybrid

TEXT의 동결 입력 3,084개를 사용한 `release-cohort.ma7fmqs6`는 03:17 KST 공유 잠금 대기 1,800초에서 **BLOCKED · exit 124 · compiler 0 · 본문 0**으로 종료했다. 원 로그를 기존 TEXT 오너에게 인계했다. 이 회차에서 Embed/Core 컴파일은 시작되지 않았다. [동결본 원 로그](</Volumes/Extreme SSD 1/build-cache/nc-text-v2-postboot.MHHgvS/release-cohort.ma7fmqs6/nextest.log>)

Native Text `fix2`는 Semantic release 라이브러리 컴파일을 지나 `quanta-index-embed`의 `ProviderBudgetLedger::with_http_attempt` 미해결과 후속 타입 오류에서 **exit 101 · 본문 0**으로 끝났다. 현재 Main의 Core 소스에는 해당 메서드가 있다. 동결한 의존성에서 원 TEXT 선택을 다시 확인해야 하며, 최종 후보의 SDK60과 Large는 **NOT_RUN**이다. [fix2 원 로그](</Volumes/Extreme SSD 1/build-cache/nc-text-v2-postboot.MHHgvS/fix2/nextest.log>) · [TEXT 원장](../../nov-12-native-cutover/tickets/NC-TEXT-build-final.md#current-combined-dependency-status)

- [ ] **07.5** B 오너가 기존 quiet-host 입장으로 원 **4,096×64/Q64/K10** Native LanceDB ANN4K와 whole SDK를 같은 소스·resolver 입력에서 수용한다. B143 graph ANN4K 결과는 해당 입력으로 보존한다. whole SDK의 1,200초 watchdog에 걸린 worker 경로를 수리하고 overlay close·두 번째 재시작을 확인한다. 독립 exact oracle로 필터별 Recall@10 ≥0.95와 빈 적격 결과를 확인하고, region 선택/탐색 손실, old view pin/delete/merge/reclaim 및 reopen을 원 조건에 대조한다. 출하 Runtime에서 Native Catalog가 LanceDB region을 관리하고 Legacy semantic adapter fallback이 0인지 실제 build/search/restore에서 확인한다. [B 잔여 판정](../../nov-12-native-cutover/tickets/NC-B-semantic.md#current-remaining-acceptance) · [Native ANN 계약](../../nov-12-native-cutover/DOD.md#native-lancedb-ann-검증)

  **11월 22일 Root/F 인수 대조.** B의 두 실명과 기존 실행기는 [4K 제품 수용](../../nov-21-bugbash-source-advance-4k/04-product-acceptance.md#종료-시-인계할-값)에 유지한다. 07.5 수용에는 아래 증거를 같은 출하 후보에 결속해야 한다.

  **07:25 KST B 실행 결과.** PREFIX4K Nextest `63cc2a94-2ea8-46f8-ac35-555ef3ebcafb`는 **1/1 · exit 0**이다. Source 4,096개 게시와 prefix 128개 완료를 확인했다. 원 로그의 `full_quality=false`에 따라 전체 ANN Recall@10과 제품 SDK 본문은 미검증 상태다. 기존 ANN4K와 PRODUCT4K 후속 제어기는 살아 있다. [PREFIX 원로그](/Users/songmin/Library/Caches/quanta-index/ann4k-materialization-diagnostic-mug1rt81/PREFIX4K.log)

  **14:56 KST B 실제 본문 인수.** ANN4K Nextest `2a4843ca-a052-44e2-b98e-12d96e2073b3`는 **1/1 PASS · exit 0**이다. `correlated_gt10pct`의 선택 결과는 **640/640**, raw 결과는 **10/10**이며 Source와 resolver drift가 없다. PRODUCT4K Nextest `492227b4-a0d2-4863-9433-e69d6d85a674`는 **0/1 · exit 100**이다. Source 4,096개를 게시한 뒤 exact owner work 대기에서 1,200초 watchdog을 소진했다. 폴러가 관측한 완료는 **3,077/4,096**이고, 만료 뒤 비원자 진단 스냅샷의 완료는 **3,490/4,096**이다. WorkStatus RPC 6,731회의 누적 시간은 111.970초이며 대기 시간은 1,057.325초다. 이 값은 관측기의 벽시계 항목이다. worker 내부 병목은 B의 phase 진단에서 판정한다. 실행 중 Source 204경로와 resolver 입력 2경로가 바뀌었다. 두 본문의 종합 결과는 `quality_incomplete`이다. [ANN 원로그](</Users/songmin/Library/Caches/quanta-index/ann4k-owner-budget-repair-uls30q9w/ANN4K/ANN4K.log>) · [제품 SDK 원로그](</Users/songmin/Library/Caches/quanta-index/ann4k-owner-budget-repair-uls30q9w/PRODUCT4K/PRODUCT4K.log>) · [제품 상태](</Users/songmin/Library/Caches/quanta-index/ann4k-owner-budget-repair-uls30q9w/PRODUCT4K/state.json>) · [종합 판정](</Users/songmin/Library/Caches/quanta-index/ann4k-owner-budget-repair-uls30q9w/quality-pair.json>)

  | 증거 | 기존 생산자와 원 결과 | 현재 인수 조건 |
  | --- | --- | --- |
  | Native LanceDB ANN4K와 whole SDK | B. 현행 ANN4K 1 PASS, whole SDK 1 FAIL이며 승인할 공통 결과가 없다. 이전 ANN에서 `correlated_gt10pct` raw 8/10·파일 506/640, 이전 SDK 최초 SourceAdvance에서 reads `999,999+4 > 1,000,000` 실패를 기록했다. [이전 ANN 원로그](</Users/songmin/Library/Caches/quanta-index/ann4k-lancedb-e1a28m08/ANN4K.log>) · [이전 SDK 원로그](</Users/songmin/Library/Caches/quanta-index/ann4k-product-retry-qz01qq7p/PRODUCT4K.log>) | 두 selector는 각각 실제 본문 1 PASS·exit 0이어야 하며 공통 Source·Cargo resolver가 일치해야 한다. B143 Graph와 Text Large는 각각의 원 실행 범위에 귀속한다. |
  | 물리 reclaim | B130 Runtime의 last-pin 본문은 해당 동결 입력에서 마지막 pin 해제 뒤 이전 파일 삭제와 현행 ANN 결과 보존을 검증했다. [B130 원 결과](../../nov-12-native-cutover/tickets/NC-B-semantic.md#L366) | F의 출하 인수에는 현행 LanceDB 후보의 pin·delete/merge 뒤 실제 파일 회수와 시간, reopen 결과가 남아 있다. |
  | Legacy semantic fallback 0 | B/Runtime은 Native Catalog가 LanceDB region을 관리하는 실제 build/search/restore 증거를 공급한다. 현재 이 제품 결과는 없다. [원 수용 조건](../../nov-12-native-cutover/DOD.md#L86) | F는 실제 세 동작과 같은 후보의 관측에서 fallback 0을 판정한다. **NOT_RUN**이다. |
  | 서명 설치 신원 | D의 기존 설치는 `PAIRED_SEMANTICA_BUILD_FAILED · exit 137`이며 해당 설치의 서명 발급·제품 영수증은 부재한다. [설치 상태](</Users/songmin/Library/Application Support/semantica-codegraph-v2/recovery/p07-runtime-closeout-20261109-01a183c0/feature-on-retry-linux-main-f82f0ca721d3/sdk-structural/rr-internal-controller/published-current-20261115/installed-status.json>) · [07.1 공급](#L140) | 같은 Q ELF·S 빌드·설치 세대의 서명 readback과 Native 선택/root·BOOT·설정·정책을 인수한다. **BLOCKED**다. |

- [ ] **07.6** 기존 F-EVAL 오너가 RRF·ANN·lexical의 실제 동일 view 후보 캡처를 블라인드 입력으로 내고 독립 심사 결과를 원 qrels로 인수한다. 심사 입력에는 검색 경로·순위·점수를 숨기고, 판정에 쓰는 qrels는 원 캡처와 파일 universe에 결속한다. 최소 답변 가능 **100질의/12저장소**, 식별자·hard negative와 무정답 **각 30/4** 및 사전 산정 표본을 적용한다. DEV/EVAL 독립성, timeout 분모와 원 품질 기준을 대조한다. 파일 MRR@10 **+0.02 이상**·저장소 단위 95% 하한 **>0**, Recall@10 하한 **≥−0.01**, 식별자 MRR 차이 **≥−0.01**, 무정답 오탐 차이 **≤+0.01**이다. N3/N4/N5는 DEV에 사용한다. [EVAL 계약](../../nov-12-native-cutover/DOD.md#독립-홀드아웃-실행과-판정) · [원 승격 기준](../../nov-12-native-cutover/DOD.md#이번-이전의-추가-승격-기준)

11월 21일 원 source-only index에 기록된 B08 개발 `release.json`의 SHA-256 `67610c74a57878b6ca69005639f7debf3cba2703cce666377e75a8beaa911ef8`를 실제 파일에서 확인했다. `benchctl.py corpus validate`는 보존 Git 번들 **10개·exit 0**을 확인했다. 개발 코드 **6,477파일**과 EVAL target·parent의 각 **8,197파일**을 다시 해시하고 기존 v3 검사를 적용한 결과 두 쌍 모두 upstream·commit·정확·근접 코드 중복이 0건이었다(123.53초·exit 0). 검증 범위는 해당 보존 릴리스의 소스다. 전체 DEV123/V47 질의 패밀리와 독립 qrels는 **BLOCKED**, 실제 Hybrid 비교는 **NOT_RUN**이다. [B08 보존 릴리스](</Users/songmin/Documents/code-new/qi-b08-closeout-20261004-2i72kj91/release-development/release.json>) · [기존 소스 분리 검사](../../../../tools/benchmark/corpus_binding.py#L439)

split manifest의 `query_family_ids`에 객체나 혼합 타입이 들어오면 정렬·중복 검사에서 `TypeError`가 발생했다. 항목 타입·문법을 먼저 검사해 기존 `EvidenceError`로 거절하도록 수정했다. 두 재현은 수정 전 **2 FAIL·exit 1**, 수정 후 `pytest -q tools/ci/tests/test_corpus_binding.py -k split`은 129개 과거 revision을 포함해 **17/17 VERIFIED·exit 0**이다. [검증기](../../../../tools/benchmark/corpus_binding.py#L316) · [회귀](../../../../tools/ci/tests/test_corpus_binding.py#L748)

원 DEV source-only 목록의 Caddy `18ab0f955fc1075d7727c7658dbfb734c673a5c9`를 보존 Git 객체에서 복구했다. `git fsck --full` 통과, tracked **608**, `code_only` **309**파일, universe SHA-256 `db9d89abaf329f2c12ea84fb301d38b0254e3cb43164538b2e72146c27cf0c79`다. target과 parent의 각 **8,197**파일을 다시 해시한 뒤 기존 v3 기준으로 비교했고 양쪽의 upstream·revision·정확·근접 코드 중복은 0건이었다(58.41초·exit 0). 이 검증은 해당 revision의 보존 소스 범위다. 원 릴리스 번들은 부재하고 전체 DEV 분리·독립 qrels·실제 SDK 품질 판정은 여전히 미수용이다. [Caddy Git 소스](/Users/songmin/.codex/a88-gq0uxqug/go-transfer-src/caddy/18ab0f955fc1075d7727c7658dbfb734c673a5c9)

11월 22일 블라인드 qrels 재생기는 두 리뷰와 재정의 양식 SHA가 서로 다른지 검증한다. 기존 소비기는 같은 양식 SHA를 다른 역할에서 재사용한 라벨을 수용했다. 리뷰 간 중복과 재정의 리뷰 재사용 회귀를 추가했다. 관련 Python 회귀 **324/324**가 통과했다. 실제 SDK 캡처·독립 심사·품질 평가는 **NOT_RUN**이다. [라벨 소비기](../../../../tools/benchmark/retrieval/holdout_review.py) · [회귀](../../../../tools/ci/tests/test_holdout_review.py)

**14:52 KST 심사자 동일성 수리.** 동일 ID에 가장자리 공백이나 탭을 붙이면 양식 발급·Native 라벨 재생·최종 승인 파서에서 다른 심사자로 취급됐다. 공용 평가기의 동일성 비교를 세 경로에 적용하고 영수증에는 제출된 ID를 보존한다. 수정 전 발급·재생 **3 FAIL**, 최종 승인 **1 FAIL**을 재현했다. 수정 후 holdout review·Native file evidence **273/273**, 최종 승인과 고정 영수증 집중 선택 **3/3**을 확인했다. 독립 심사와 Hybrid 승격 실측은 **NOT_RUN**이다. [공용 비교](../../../../tools/benchmark/retrieval/evaluator.py#L163) · [최종 승인](../../../../tools/benchmark/retrieval/run.py#L2894)

## 원 E07과 비교

- [ ] **07.7** 동일 출하 SDK/daemon의 signed Mixed·E07 원 SLO/비용과 별도 HybridFile **960초 p95≤100ms/p99≤500ms**를 확인한다. 실제 요청과 deadline 위반·환경 무효를 원 분모와 함께 보존한다. F2는 부모/자식 terminal의 `case_exit=0`, `status=VERIFIED`, `product_verified=true`를 대조한다. [원 E07 프로필](../../oct-22-finalizing/E07-performance-operations.md#fixed-profiles-and-oracles) · [승격 기준](../../nov-12-native-cutover/DOD.md#이번-이전의-추가-승격-기준)
- [ ] **07.8** 이전 서명 릴리스와 삭제 전 Native **각각**에 대해 동등 기능·같은 Linux 조건으로 `AB`/`BA` 교대 **5쌍**을 실행한다. submit→ACK와 회차별 조회 p95의 짝비 중앙값을 각각 **≤1.05**로 확인하고, 절대 SLO·peak RSS/공간 기준과 원값을 기록한다. [원 비교 기준](../../nov-12-native-cutover/TANTIVY-RETIREMENT.md#벤치-실행과-합격선)
