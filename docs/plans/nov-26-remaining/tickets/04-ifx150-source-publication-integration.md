# 04 · IFX150 Source 게시와 응답 유실 통합

2026-11-26 KST 대조 기준은 S main `b9ca7382747`, Q main `28784da26`과 각 작업 트리다. 기존 `증분 SDK · 변경 경로로 다음 세대 생성`의 컴파일·제품 결과는 INC-01과 INC-03에서 인수한다. [현행 코드·영수증](../../../bugbash/nov-20-native-delta-cost/tickets/INC-01-source-closure.md#현재-남은-제품-코드--2026-11-26)

## 현재 판정

| 범위 | 현재 코드와 증거 | 남은 조건 |
| --- | --- | --- |
| **IFX150 격리 `FAILED`** | `20261125T125219.058705Z-7b0d87a742a4`는 **exit 101 · Runtime 컴파일 오류 19건 · 본문 0건**이다. 이 실행의 수정은 S `191565e26d8`과 `f7cdc36ded7`에 합류했다. [격리 종료 결과](/Users/songmin/Library/Caches/semantica-codegraph-v2/ifx150-response-loss-runtime.a1KjDTpw/result.json) | 수정 커밋을 인수하고 실제 제품 회귀를 같은 S·Q 후보에 합류한다. |
| **SDK 선행 QBC `FAILED`** | Runtime 호출부 4곳 수정 S `077bc3cdf56`은 private type 컴파일 오류로 **exit 101 · 본문 0건**이다. `f7cdc36ded7` 요청은 PTA visibility 수리로 봉인 전에 대체됐다. [선행 종료](/Users/songmin/Library/Caches/semantica-codegraph-v2/inc01-v2-sdk-runtime-prereqs.fmoqysn4/sdk-five-result.json) · [대체 단계](/Users/songmin/Library/Caches/semantica-codegraph-v2/inc01-v2-sdk-runtime-replay-repaired.o9kfx6b4/stage) | 현재의 수리된 S를 호환 Q와 결속한다. |
| **후속 SDK `FAILED`** | S `7626200803b` 실행 `20261125T193459.486122Z-285f7bad22a6`은 Runtime 컴파일 **exit 101 · SDK 0/5**다. 디스크 0/1과 V2 readback은 `NOT_RUN`이다. S의 IFDS·Java export·Original factory 경계는 후속 `1f92b477b3d`·`62e93cd647e`를 인수한다. Q Regex의 `engine_config`·`engine_construction_bytes`는 Q 작업 트리에 있으며 Q HEAD에는 없다. [종료 영수증](/Users/songmin/Library/Caches/semantica-codegraph-v2-target/quanta-build-cli/_state/execution-roots/semantica-codegraph-v2-7dceafca41ec/lanes/inc01-sdk-source-owner-snapshot-artifact/verification-results/20261125T193459.486122Z-285f7bad22a6/receipt.json) · [Q API 내보내기](../../../../crates/quanta-index-lq-regex/src/lib.rs#L53) | Q 담당의 API 구현을 포함한 호환 Q 공급을 기존 SDK 레인에서 사용한다. 현행 S·Q 컴파일과 제품 본문은 `NOT_RUN`이다. |
| Q 응답 유실 훅 | Q 작업 트리의 `native_response_loss_test_support_v1.rs`는 opt-in이며 아직 untracked다. 클라이언트는 실제 요청 프레임을 전송한 뒤 응답이 읽기 가능해질 때까지 기다리고, 디코딩 전 동일 연결을 닫는다. Linux 실제 peer/EOF 집중 본문 2/2는 기존 동결 입력에서 통과했다. | 훅과 기능 조건을 현행 Q 후보에 포함하고 정확한 `SubmitStagedSourceV1` 요청 하나만 선택했는지, 상대 PID·원 기한·취소를 유지했는지 확인한다. Q IPC 통과는 S 제품 게시의 증거가 아니다. |
| IFX 제품 회귀 | `native_delta_reconciles_lost_q_submit_reply_before_terminal_receipt_v1`과 suite 등록은 S `1f4ec3739b8` 기반 별도 dirty 작업 트리 `semantica-codegraph-v2-response-loss-current`에만 있다. S main에는 해당 exact가 없다. `--resolve-only`는 1건을 선택했으나 Linux 설치 본문은 `NOT_RUN`이다. | 기존 IFX 오너가 최신 S main과 Q 후보에 이 fixture를 재결속하고, 동일 Source·signed SQL worker·실제 Q 서버로 본문을 실행한다. |
| 기존 ACK 절단 | `QPublishedBeforeTerminalAck`와 pre-V9 동일 attempt·pending carrier 회귀는 S main에 등록돼 있다. Q가 게시된 뒤 terminal ACK가 기록되기 전의 절단이다. | 응답 자체가 전송 후 유실되는 새 절단과 별개로 같은 코호트에서 실행한다. 기존 ACK 절단의 존재나 selector 해석을 새 유실 절단의 성공으로 세지 않는다. |

남은 코드 합류는 호환 Q Regex API 공급, 실제 Q IPC 훅과 S 제품 selector 결속이다. Q 공급에는 `InitialHistoryCutV1`, `DslRuntimeCatalogPageV1`, `DslStructuralChunksPageV1` 응답과 현행 wire ID가 함께 필요하다. 이 조건을 기존 Q 공급 담당에서 확인한다. [호환 입력 대조](../../../bugbash/nov-20-native-delta-cost/tickets/INC-01-source-closure.md#남은-실행-검증)

**기존 복구 담당 인수.** S `86ecd07699d`에서 Watch V16은 설치된 복구 오너가 같은 Source의 Target을 대여하면 `restore_delta_v16`으로 전달한다. 해당 SDK trait 구현은 작업 트리에 있다. 서명된 SDK V7의 `read_retained_sdk_predecessor_v7`는 journal·Source pointer·예약·Q peer를 대조하며, cold Apply·Rollback 제품 호출 연결은 기존 SDK 복구 담당이 맡는다. 이 흐름의 소스 합류와 IFX 실제 응답 유실의 설치 본문은 각각 기존 원장에서 판정한다. [Watch DELTA 호출](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/coordination/change_event_recompute_invalidator/incremental_rail/native_cold_checkpoint_v16.rs:525) · [SDK predecessor 계약](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/sdk/search_builder/index_owner_env_authority/source_only_sdk_predecessor_v7.rs:30)

## 기존 오너의 실행 순서와 합격 조건

1. 기존 `inc01-sdk-source-owner`에서 현행 S/Q sibling, Cargo lock, feature와 exact를 결속해 Runtime 컴파일을 확인한다. 기존 요청의 source digest와 종료 결과를 먼저 확인한다. 설치 본문 실행에는 같은 후보의 Q ELF와 signed SQL worker까지 고정한다.
2. Q IPC의 opt-in 훅이 **G2 `SubmitStagedSourceV1` 실제 요청 한 건**에서 발화했음을 확인한다. 전송 전 실패, 가짜 응답, 디코딩 후 연결 종료, G1 요청 선택은 오라클에 맞지 않는다. IFX writer는 불확실한 결과의 typed 오류를 받고, 독립 Q 조회는 writer 종료 전에 같은 operation의 게시를 확인해야 한다.
3. 같은 closeout·attempt·Q operation으로 새 SDK PID를 재개한다. 저장된 ACK와 G2 `PublishedViewMetadata` 및 공개 검색을 읽고, Source CAS 변화량 **writer 1회·복구 reader 0회**, 재제출 Q Submit **0회**, 새 FULL·V3 쓰기 **0회**를 확인한다. Source pointer·Q Source root·view가 동일해야 한다. 다른 attempt, operation, workspace, peer, epoch 또는 stale fence가 섞이면 거절하고 부작용 0회를 확인한다.
4. 기존 `QPublishedBeforeTerminalAck`, pre-V9 재무장, pending carrier와 페이지 ACK·seal 응답 유실은 각각 원 selector·기능·실제 본문으로 판정한다. 동일 입력의 이미 끝난 영수증만 재사용한다. 정적 선택, 컴파일 통과, Q IPC 단위 2/2를 설치 E2E로 합산하지 않는다.
5. Java `EquivalentSnapshot` FULL을 기준 세대로 설치한 뒤 **첫 V2 DELTA**를 실행한다. 같은 Source의 독립 FULL과 G2 공개 Text 결과를 경로·순위·점수 비트·삭제 부재까지 비교하고 Source CAS 변화량 1회 및 정확한 ACK/view를 확인한다. 현재 등록된 `java_equivalent_full_then_first_native_v2_delta_product_owner_v3`는 Linux 컴파일·본문 `NOT_RUN`이다.

## 비용과 설치 선행 조건

- Warm 단일 편집의 **무관 본문 read/copy/hash/write 0 B**, Store 전체 검증 0회, 변경 본문 읽기 1건은 별도 비용 조건이다. S Store `source_tree_manifest_v4.rs`에는 전체 본문 검증 호출이 남아 있어 현재 소스가 이 조건을 만족하지 않는다. Store 재사용 객체의 불변성·재개방 권위는 01, ItemIndex·Runtime 전체 경로/본문 비용은 02의 수리와 같은 operation에서 확인한다. 04의 ACK/G2 통과만으로 warm DELTA 0 B를 선언하지 않는다.
- P07의 `/Volumes/Extreme SSD 1`가 다시 마운트됐고, 활성 명세의 QGL 경로는 열린다. 활성 명세와 컨트롤러 intake 사본의 S·Q 동결 경로는 없다. 정본 Product intake도 없어 Source 소실 검사는 exit 1이다. 서명은 S 4역할 영수증 대기이고, 설치는 기존 Root 큐다. 기존 P07에서 소스 복구 또는 검토된 후계 입력과 서명을 인수한다. [현행 설치 판정](../../../bugbash/nov-20-native-delta-cost/tickets/INC-03-product-acceptance.md#현행-판정--2026-11-26)

**현재 판정.** 코드 통합 미완료, IFX 응답 유실·Java 첫 DELTA 설치 본문 `NOT_RUN`, warm 0 B 소스상 미충족, P07 설치 `BLOCKED`. 기존 INC-01에 최종 source tuple, QBC receipt, 실명 본문·종료 코드, 같은 attempt/operation과 Q/Source 효과 계수를 기록한 뒤 이 티켓을 닫는다.
