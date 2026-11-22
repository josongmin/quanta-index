# INC-03 · 기존 제품 테스트에서 증분 수리 결과 인수

**검증 전용.** 비용·정확성 회귀의 결과는 [Source](INC-01-source-closure.md)와 [Q Literal](INC-02-literal-directory.md)에 기록한다. E02와 NC-F는 아래 기존 테스트를 통해 변경이 SDK·Watch·Q 검색까지 전달되는지 확인한다. 현행 결합 소스의 설치 SDK·Watch와 제품 수용은 `NOT_RUN`이다. [E02 검증](../../../plans/oct-22-finalizing/E02-publication-recovery.md#e02-checkpoint-acceptance) · [NC-F 검증 범위](../../../plans/nov-12-native-cutover/tickets/NC-F-integration.md#root-실행과-기존-오너-인수)

| 대상 | 기존 본문 | 인수할 결과 |
| --- | --- | --- |
| S SDK 게시 | `ifx150-native-checked-delta-second-terminal-product-owner` | 실제 FULL G1 → Java 한 파일 변경 G2. Q Published의 operation·view·source와 V5 terminal·V9 ACK 일치. |
| S SDK 검색·복구 | `ifx150-native-delta-q-published-before-ack-product-owner` | Q 게시 후 ACK 유실과 동일 작업 복구. pin한 G1의 `changed` 0건, exact G2의 `src/main/java/demo/App.java` 검색 성공. |
| S Watch | `native-installed-postpublication-recovery-process-owner` | 실제 `app.py` 편집 후 Watch cursor 전진, 새 Source·Attach snapshot, Q Latest에서 marker 정확히 1건. 뒤따르는 Rename은 Watch도 관찰할 수 있으므로 SDK 인과성은 위의 SDK 본문에서 확인한다. |
| Q Literal | `create_edit_delete_old_reader_replay_and_cold_reopen` 및 기존 Literal UDS | Kernel에서 비어 있지 않은 prior의 replace·delete, 이전 pin과 재개방 검색을 확인한다. UDS 본문은 Literal 등록과 재개방 조회를 검증한다. 디렉터리 비용은 INC-02 회귀가 소유한다. [Kernel 본문](../../../../crates/quanta-index-kernel/tests/native_lifecycle.rs#L1199) · [UDS 본문](../../../../crates/quanta-index-searchd-runtime/tests/native_index_uds.rs#L2221) |

S selector의 원본은 `semantica-codegraph-v2/tools/testing/suite-registry-v1.yaml`이다. 세 selector는 Linux·해당 feature에서 각각 한 개의 exact 본문을 선택한다. 영향 소스·기능 구성·설치 바이너리·입력이 일치하고 지정 본문이 실제로 통과한 결과만 인수한다. 현재 확인한 원장에는 SDK 정상 게시가 0/1로 기록돼 있고, 나머지 두 selector도 현 후보에 결속된 통과 기록을 찾지 못했다. selector 해석만 수행한 실행으로 완료 처리하지 않는다. [기존 SDK 실행 원장](../../../plans/nov-12-bugbash-c0-p09/tickets/B05-final-verification.md)

SDK·Watch 제품 본문은 Q Text의 결과를 조회한다. 세 본문과 기존 `OriginalSourceFirstRefusalObserverV3`·`ExactMutationTestCountersV1`에는 경로·단계별 FileText 복사량과 재해시 바이트가 없다. 따라서 INC-01의 **무관한 본문 0 B**를 각 단계에서 직접 측정한다. 기존 SDK 복구 회귀의 두 파일 변경과 fault들은 수용 경로에서 검증한다. 비용 측정은 한 파일 편집만 분리해 동일한 operation으로 successor, Store persist/open, 게시 준비, ACK 포트 반환과 Q 검색 노출을 묶는다. 무관한 파일의 수와 크기를 독립적으로 바꿔 비교한다. ACK 뒤 S literal 포트의 조회·환급은 [INC-01의 포트 검증](INC-01-source-closure.md#완료-판정), Q packed posting 내용은 [INC-02의 논리 key 대조](INC-02-literal-directory.md#완료-판정)에서 인수한다.

**최종 성능 수용은 E07이 소유한다.** Q Large·XL·Mixed는 Text만 등록하고 직접 UDS로 stage한다. 이 실행은 S의 파일 이벤트와 packed Literal Patch를 거치지 않는다. E07은 최종 후보의 원 FULL 제한과 Mixed 부하·freshness 목표를 그대로 판정한다. 과거 Q `824629e4`의 XL은 최초 FULL ACK 전에 실패해 DELTA 시간이 없다. 이 채팅에서 언급된 과거 94초도 두 수리의 전후 기준값으로 쓰지 않는다. [실제 Text 프로필](../../../../crates/quanta-index-searchd-harness/src/scale/native_index.rs#L514) · [E07 DoD](../../../plans/oct-22-finalizing/E07-performance-operations.md#dod) · [기존 결과 원장](../../../plans/nov-5-indexing-bugbash/README.md#현재-판정)

## 한 파일 편집 비용 측정

SDK와 Watch에서 무관 파일의 두 축을 독립으로 비교한다. 각 본문은 무관 파일 **1개 × 256 B**, **32개 × 256 B**, **1개 × 16,384 B**를 사용한다. 각 행은 자체 FULL 기준점과 같은 예산에서 실제 한 파일만 변경한다. 파일 크기 축은 같은 주석을 공백으로 패딩해 검색 토큰과 posting 수를 유지한다. 행마다 FULL 준비와 이전 뷰의 marker 부재 확인을 마친 뒤 시간을 잰다. 이 입력 크기는 경로별 기준 측정용이며 E07 부하의 분모는 E07에서 유지한다.

- SDK 정상 본문은 기존 replace/delete와 fault 회귀를 수행하고 동일 selector에서 별도의 `App.java` 단일 replace를 세 번 실행한다. 각 row는 G1에서 `changed` 0건, Q의 게시된 exact G2에서 `App.java` 1건, V5에 남은 무관 Java 파일의 실제 바이트, 동일 operation으로 Acked인 V9를 확인한다. V9의 Lexical 영속 terminal을 내용 주소와 길이로 확인하고 디코딩한 operation·view·Source cut을 Q의 실제 게시와 대조한다. SDK 호출부터 기존 Runtime successor-pair·required-members·V9 insert·submit 진입, Q stage 반환, pre-ACK 게시, V9 ACK, SDK 반환, Q operation status 관측과 exact 검색 시각을 기록한다. 응답 JSON과 Q 게시 메타데이터 검증은 첫 exact 조회 이후에 수행한다. Runtime hook은 operation으로, stage/ACK cut은 인증된 closeout/attempt로 결속한다. 기존 SQL/UDS observer의 호출 시도 수는 SDK 프로세스 내 실제 dispatch 구간에 한정한다. [SDK 측정 fixture](../../../../../semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-sdk-runtime-executor/src/client/ifx150_restart_recovery_test_support_v1/native_initial_full_recovery_v3/native_checked_delta_v3/single_edit_cost_v3.rs)
- SDK ACK 유실 본문은 기존 replace/delete의 writer dispatch부터 pre-ACK cut까지 측정한다. writer 종료 전에 측정값을 기존 임시 fixture root에 기록하고 부모가 Q 게시와 V9 operation·closeout·attempt를 검증한다. 복구 전후 G2 exact 질의 시간은 각각 기록한다. `App.java` 한 파일 비교에는 정상 SDK의 세 행을 사용한다. [ACK 유실 driver](../../../../../semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-sdk-runtime-executor/src/client/ifx150_restart_recovery_test_support_v1/native_initial_full_recovery_v3/native_checked_delta_v3/durable_cuts_v3.rs)
- Watch 본문은 정상적인 `app.py` 쓰기 직전부터 Q Latest에 marker 1건이 나타난 exact 조회까지 잰다. 관측 시각에는 polling 간격이 포함된다. Q endpoint와 BOOT workspace/epoch는 편집 전에 준비한다. 실제 파일 쓰기부터 Latest polling과 marker 조회까지 기존 30초 마감을 공유한다. Watch cursor 전진, Source/Attach 갱신, 새 Attach 세션과 snapshot의 installed private readback에서 manifest·generation·V9 terminal/ACK를 대조하고 Acked operation을 같은 Q Source cut에 결속한다. 이전 뷰와 실제 관측 뷰에서 무관 sentinel의 전체 경로 집합을 대조한다. rollback 거절, Rename, 복구·재시작은 기존과 같이 기본 행에서 검증한다. [Watch exact 진입점](../../../../../semantica-codegraph-v2/crates/quanta-cli-server/tests/server_socket_daemon_e2e.rs) · [편집·검색 경계](../../../../../semantica-codegraph-v2/crates/quanta-cli-server/tests/server_socket_daemon_e2e/native_offline_recovery_v1/postpublication_v1.rs)

기존 hook의 시각은 dispatch 시작 기준이다. SDK successor, persist/reopen, preflight의 배타적 소요 시간과 물리 I/O는 아래 실제 실행 지점에 operation 결속 계측이 필요하다. SQL 쓰기 문장·UDS 송신 시도 수와 Q 조회의 매치 수로 무관 본문 작업량을 추정할 수 없다.

| 소유자 | 추가 계측 지점과 계수 |
| --- | --- |
| SDK Source | `original_source_successor_v11.rs::prepare_original_source_successor_v11`에서 시작·종료, prior path 방문·복제, flat commitment 입력, 변경 text digest를 기록한다. 실제 입력은 `facts_public_source_preparation_v11.rs::prepare_facts_mutation_source_inner_v11` 및 `runtime_disk_state_attribution_request_v1.rs::prepare_original_source_attribution_parts_for_target_v3`에서 연결된다. |
| Store | `exact_source_revision_authority_v2.rs::persist_generation_source_closure_with_prepared_revisions_v2`, `durable_normal_source_attribution_generation_store_v1.rs::persist_generation_source_member_borrowed_v11`, `rehydrate_source_authority_transition_pair_v1`, `hydrate_source_member_original_into_birth_v3`에서 persist/open 구간과 실제 CAS 읽기·복사·digest·쓰기 바이트, member/path/node 방문을 각각 기록한다. |
| E02 Runtime → Q | `aggregate_prepare.rs::prepare_search_plane_aggregate_members_v1`의 census, `native_source_validation.rs::check_native_source_delta_body_v3`와 `check_delta_changed_set_with_work_v3`의 검증 경로 방문·본문 및 wire 작업, `native_source_publish_v1.rs::stage_checked_native_source_delta_before_seal_v1`과 `submit_bound_native_source_admitted_v3`의 stage 및 submit을 구분한다. Q 수신 쪽은 Kernel `source.rs::apply_source_inner`와 Catalog `native/authority.rs::commit_transition`에서 같은 operation의 로컬 시간을 확인한다. 설치 Watch에는 이 계측을 같은 resident operation으로 내보내는 연결이 필요하다. |

### 2026-11-21 실행 상태

- **정적 VERIFIED.** S의 소유 변경 7파일을 `rustfmt --edition 2021 --emit stdout --config skip_children=true`로 파싱했고 소유 diff의 공백 검사도 exit 0이다. 기존 `tools/testing/run-suite-v1.sh <suite> --resolve-only`는 위 S selector 3개를 각각 exact·expected 1로 해석했다. 세 해석의 종료 코드는 각각 0이다.
- **제품 NOT_RUN · 정상 SDK 0/1, ACK 유실 SDK 0/1, Watch 0/1.** 기준 및 결합 후보의 시간·비용 표본은 0건이다. P09의 고정 입력은 이번 소유 수정 전 Source다. warm attempt journal은 없었고 호스트 여유 48,275,832,832 B는 기존 50 GiB 입장 하한에 미달했다. NC-F 설치 상태의 exit 137은 앞선 공급 시도의 중단이며 설치 서명 영수증은 없다. 동일 Source의 승인 Linux 입력이 공급되면 기존 E02/NC-F에서 각 exact 본문을 한 번 실행하고 원 로그의 `INC-03` 결과를 이 티켓에서 인수한다. [P09 기존 입장과 본문](../../../plans/nov-12-bugbash-c0-p09/tickets/B05-final-verification.md) · [NC-F 기존 설치 큐](../../../plans/nov-20-native-cutover-closeout/tickets/03-signed-published-cp2.md)

### 2026-11-21 20:50 KST 후속 감사

- **정적 VERIFIED.** SDK·Watch 계측 경계를 조정한 뒤 Rust 소스 7/7 파싱, 대상 diff 공백 검사, 기존 exact selector 해석 3/3이 exit 0이다.
- **제품 NOT_RUN · 0/3.** 활성 Linux 설치 컨테이너와 결합 후보의 서명 영수증을 확인하지 못했다. 기존 NC-F 후속 상태가 가리킨 PID 41179와 P09 조정 PID 1114는 현재 존재하지 않는다. 호스트 여유는 약 36.5 GiB다. 기존 실행 입력이 확보되면 소유 본문과 계측 표본을 각각 인수한다.
- 최신 S의 `prepare_original_source_successor_v11`은 prior path census와 flat target identity 생성을 수행한다. `check_delta_changed_set_with_work_v3`도 old/new 집합을 순회한다. Reader/Store의 V2 타입만으로 현재 제품 경로의 무관 본문 0 B 또는 전체 경로 열거 0회를 판정할 수 없다. 계측·수리 지점은 위 소유자 표에 기록했다.

### 2026-11-21 20:57 KST 영속 terminal·입력 감사

- **정적 VERIFIED.** 원 크기·파일 수 두 축에서 본문 패딩의 반복 토큰을 제거하고 SDK·Watch의 ACK 증거를 보강했다. Rust 소스 파싱 10/10, 대상 diff 공백 검사, 기존 exact selector 해석 3/3은 exit 0이다.
- **제품 NOT_RUN · 0/3.** 현행 P09 원장은 canonical supplier 제출 전 입장 시간 초과를 기록했고 기존 PID는 부재다. NC-F 서명 설치 영수증은 0건이며 호스트와 외장 경로는 같은 내부 파일시스템을 보고한다. SDK·Watch의 private raw readback은 V5·Required envelope와 Source 본문에 합계 32 KiB 상한을 그대로 적용한다. 파일 수 확대에서는 fixture 본문만 8,192 B, 크기 확대에서는 16,384 B가 계상된다. V5·Required envelope와 기존 파일이 차지하는 나머지 크기는 현행 실행에서 확인해야 한다.

### 기존 레인 재확인

- `quanta-build-cli status --observational --json`으로 소유 selector의 세 정식 레인이 모두 잠금 해제 상태임을 확인했다. 세 레인의 로컬 상태에는 레인 식별자와 target 경로만 있으며 실행 영수증은 없다. P09·NC-F의 마지막 조정 PID는 부재다.
- **실행 입장 BLOCKED, 제품 본문 NOT_RUN 0/3.** 호스트 여유는 약 32.5 GiB이며 기존 50 GiB 하한에 미달한다. 외장 경로는 내부 APFS와 같은 장치에 있고 별도 외장 장치가 열거되지 않았다. NC-F 서명 단계는 S 4역할 영수증 대기이며 동일 후보의 signed-product receipt는 0건이다. 측정 표본 0건과 본문 0/3을 유지한다.

### SDK·Watch operation 계측 연결

- **SDK 정상.** 기존 pre-Source journal 확인, Source commit 완료, signed SDK checkpoint 확인 훅을 `OperationKey`로 같은 trace에 연결했다. 기존 journal 회귀를 재사용해 실제 SDK target, V9 closeout/attempt, 영속 Source transition·snapshot·digest와 signed M을 맞춘 뒤 출력한다. journal 훅은 확인용 durable readback을 포함한다. 따라서 이 관측 시각들은 개별 Store persist/open 소요 시간이 아니다. ACK 유실 본문은 기존 writer cut과 복구 전후 exact 조회를 사용하며 동일 operation과 Source snapshot·digest·view를 출력한다.
- **Watch.** 파일 쓰기 완료, Q Latest·exact 검색, cursor 전진, durable Source 확인, ACK capture 관측을 실제 편집 시작 기준으로 기록한다. 새 Attach 세션에서 capture의 snapshot·manifest·generation과 V9 closeout/attempt·checkpoint sequence·terminal digest를 확인하고 Q의 같은 operation·Source cut에 묶는다. cursor·Source·ACK capture는 marker 검색 후 확인한 시각이다.
- **생산 카운터 인계.** SDK와 Watch가 공통으로 들어가는 `prepare_facts_mutation_source_inner_v11`과 Watch 호출부 `facts_watch_source_transaction_v11.rs::prepare_facts_watch_source_task_v11`에서 fixture와 같은 `OperationKey`를 유지한다. SDK successor, Store persist/open, E02 preflight, Q stage/submit·ACK의 생산 소유자가 위 표의 함수에서 단계 소요 시간과 무관 본문 read/copy/hash/write 바이트, 경로·노드 방문 수를 제공한다. E02의 warm stage 호출부 `terminal_handoff_v3/stage_v3.rs`와 terminal 재검증 호출부 `terminal_aggregate_readback_v2.rs`는 현재 census 검사 API를 쓴다. V2 증명 경로를 연결할 때 두 호출부의 계측을 함께 확인한다. 취합은 `OperationKey`와 해당 Source 전이, 발급 이후 V9 closeout/attempt로 대조한다. 프로세스마다 자기 단조 시계의 소요 시간을 기록하며 미수집 계수는 0으로 채우지 않는다.
- **정적 VERIFIED.** 수정된 fixture 4파일 Rust 파싱과 diff 공백 검사, 기존 exact selector 해석 3/3이 exit 0이다. P09 입장 시간이 만료됐고 NC-F는 S 4역할 영수증을 기다린다. 호스트 여유는 약 32.4 GiB다. **설치 본문 NOT_RUN 0/3**, 실측 0건이다. 32 KiB readback 한도 초과 여부는 실행에서 확인되지 않았다.
