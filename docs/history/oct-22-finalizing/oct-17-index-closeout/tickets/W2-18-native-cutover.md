> HISTORICAL — pre-migration document from `docs/plans/oct-17-index-closeout/tickets/W2-18-native-cutover.md`. Current work: [Oct-22 SSOT](../../../../plans/oct-22-finalizing/README.md). Old TODO/status/counts do not create work or qualify current source.

# W2-18 — 실제 target 인계·Native 전환과 boot 수용

상태는 [SSOT](README.md#w2-18)만 갱신한다. 이전 W2-08 verifier 잔여를 포함한다.

## Remaining scope

- W2-05의 authoritative FULL operation/target issuer 결과를 새-root rebuild→select/receipt→격리 candidate Serve에 인계한다. SDK의 `NativeSourceTargetV1` 입력·전달 API는 재사용한다. API 존재와 실제 FULL 출력·BOOT 인계 수용을 구분하며 새 issuer를 만들지 않는다.
- Legacy default selection→fresh Native root의 전환 경로를 기존 launcher/CAS authority에 연결한다. 기존 Native→Native 수용 결과를 새 Legacy consumer 수용으로 계산하지 않는다. `install`/`install-native`의 같은 root mode 덮어쓰기는 verifier 전에 거절한다. 이 install 거절 수리와 별도 switch/rollback consumer의 구현·실행 수용을 구분한다. 같은 모드 bootstrap/reselect와 기존 Native cross-root CAS는 재사용한다.
- 새 rollback old-root Serve/search·두 차례 restart/receipt 재선택과 receipt final directory-close fault를 실제 source에서 수용한다. 기존 회귀는 재구현하지 않는다.

## Reuse and sources

- [native CLI](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd/src/cli/native.rs): Select/Verify/Serve 계약.
- [native_boot.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/src/native_boot.rs): Catalog candidate 인증·selection·receipt.
- [launcher.py](/Users/songmin/Documents/code-new/quanta-index/tools/deployment/launcher.py): 기존 Native↔Native CAS와 Legacy→fresh Native 및 명시적 `rollback-legacy` consumer. Legacy 전환은 동일 protected deployment 안에 caller가 보존한 기존 Selection과 out-of-band 고정 fingerprint를 요구한다. Native receipt가 없는 기존 root에 pristine 전용 verify-state를 적용하지 않는다.
- [native boot fixture](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/tests/native_product_process/native_boot.rs)·[receipt](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/src/native_boot/receipt.rs): 기존 실제 UDS/rollback·fault oracle.

select/Verify/Serve·two-root rebuild·Native CAS rollback은 구현돼 있다. 새 launcher/target issuer·제자리 추측 migration·Native backup/restore는 범위가 아니다.

## Sequence and acceptance

BOOT는 launcher/CLI/receipt exact hunk, SDK/INT는 원 FULL target 인계를 소유한다. target 계약 동결 뒤 독립 격리 준비가 가능하며 W2-19 A를 기다려 fixture 개발을 막지 않는다.

1. actual producer target으로 same lease/identity·foreign/symlink/alias/incomplete 거절, Verify→Serve/search/reopen·cross-root rollback·crash/fault를 검사한다.
2. W2-19 A가 opt-in candidate의 기능/비용과 격리 리허설을 수용한다.
3. A 후 실제 default routing switch와 경쟁 head/lifetime consumer 퇴역을 수행한다.
4. W2-19 B가 실제 default smoke/rollback·coherent integration·remote CI/release를 판정한다.

기존 Native-root 및 receipt 회귀는 VALIDATION의 해당 source/selector 범위로 소비한다. 현재 상태는 README만 갱신하며 Common close/fault를 재작성하지 않는다.

## Implementation packet

[W2-05 E](W2-05-producer-full-delta.md#sdk-producer-custody)와 [W2-02 terminal](W2-02-pending-recovery.md#terminal-publication-join)의 실제 선택형 FULL 결과를 Root/PUB가 기존 BOOT caller에 인계한다. fixed-target/ACK projection receipt만으로 operation/view/source를 합성하지 않는다. 기존 launcher/selection·보존 predecessor·rollback을 재사용하며 실제 default switch는 [W2-19 A](W2-19-integration-acceptance.md#combined-checkpoints) 뒤다.

- **즉시 준비:** BOOT 담당이 현재 Legacy/Native selection 입력과 launcher의 CAS/receipt 전제, 기존 격리 rebuild/rollback·새 close-fault fixture를 대조한다. SDK 발급 target을 추측하거나 직접 발급하지 않는다.
- **계약 후 구현:** W2-05의 FULL operation/target tuple을 select/receipt/Serve로 전달한다. Legacy→fresh Native의 허용 조건·이전 selection 보존·실패 복구를 기존 CAS authority에 연결한다.
- **정적 완료:** foreign/symlink/incomplete 거절, target/source identity·동시 selection CAS·rollback 원 root/receipt 및 cleanup을 실제 caller에서 대조한다. Native→Native 기존 경로는 재작성하지 않는다.
- **반환물:** SDK target 소비 위치·launcher exact hunk·candidate/rollback 절차·실행할 기존 fixture. 격리 candidate 실행은 W2-19 A 전에 가능하며 실제 default 변경은 A 뒤다. deploy/activate/rollback 실 target/observer 입력은 W2-19의 별도 조건이다.

## SDK result binding and predecessor custody

- 기존 public 선택형 `QuantaClient::ensure_indexed_with_native_source_selection_v3`는 실제 `NativeInitialFullPublishedV3`를 반환한다. cross-repo BOOT caller는 이를 한 번 호출해 retained ACK에 결합된 target operation, 게시 operation/view/source와 이미 계산된 post-FULL source-cut digest를 직접 소비한다. 새 결과 API/target issuer를 만들지 않는다. daemon auto-Attach가 결과를 drop하는 사실만으로 opaque session의 공개 반환형 변경이 필수라고 추론하지 않는다. 같은 auto-Attach 경로 자체를 검증할 경우에만 원 SDK/daemon writer의 인계를 별도로 결합한다.
- Native receipt Verify는 실제 candidate Catalog를 인증한다. SDK가 의도한 rebuild의 결과인지도 확인해야 하므로 W2-05의 actual operation/post-FULL view/source evidence와 receipt의 manifest_operation/source를 대조한다. Legacy selection에는 이를 대체할 workspace/operation 축이 없다. target.expected_source는 게시 전 값이며 opaque projection ACK만으로 post-FULL source를 추정하지 않는다.
- prior Legacy Selection과 out-of-band 고정 fingerprint는 실제 product caller가 전환 전에 보호된 deployment 안에 영속 보존하고 재시작 때 동일 predecessor로 복구한다. launcher mock에서 임시 파일을 만든 것은 이 custody 증거가 아니다. forward 게시 전/후 중단, postrename 불확실 상태, candidate reopen, saved selection 손상/변조와 stale CAS를 기존 atomic publish·선택 조회로 판별한다. 새 migration ledger/issuer를 만들지 않는다.
- genuine SDK 결과의 기존 process owner는 S `quanta-sdk-runtime-executor/src/client/ifx150_restart_recovery_test_support_v1/native_initial_full_recovery_v3.rs`의 `with_configured_native_fixture_v3`/writer caller다. fixed-target `ensure_indexed_with_native_source_socket_v1` 경로를 선택형 FULL→BOOT 수용으로 계산하지 않는다. F/SDK 원 writer가 기존 production client·installed SQL provider·process roots를 재사용하는 exact caller에서 선택형 결과를 인계한 뒤 B가 Q launcher 소비자를 연결한다. S 서버 `server_socket_daemon_e2e`의 opaque Attach 응답은 이 tuple을 제공하지 않으며 이를 위해 별도 client factory·issuer·공개 결과 API를 만들지 않는다.
- Q의 기존 `native_cross_root_sdk_rebuild_switch_and_rollback_preserve_fixed_query` 및 `native_boot_selection_verification_and_selected_serve_preserve_real_publication`은 Q 측 publication/receipt/Serve oracle다. genuine S FULL 인계와 함께 forward/reopen·Legacy rollback Serve/search를 대조한다. Q-only 및 Native↔Native 수용을 cross-repo SDK/Legacy 수용으로 재사용하지 않는다. launcher Python owner 파일은 한 영향 배치로 실행한다.
- launcher의 deployment lock은 협조적 launcher 경로에서 Serve lifetime을 직렬화한다. 직접 실행된 비협조 Rust daemon이 정지했다는 증거로 사용하지 않는다. 실제 전환 caller는 기존 process owner의 stop/join/lease 계약을 소비하고 불확실하면 전환을 거절한다. default 전환은 W2-19 A 뒤다.


## Explicit prior-selection preservation

- `preserve-legacy`는 caller가 지정한 current selection의 expected CAS와 별도 previous-selection 경로를 소비한다. 잠금 아래 실제 현재 Legacy Selection을 canonical bytes로 보존하며 새 predecessor identity·target·issuer·migration ledger를 생성하지 않는다. switch/rollback이 predecessor를 자동 생성하지 않는다.
- 저장 경로는 같은 보호된 deployment 디렉터리의 별도 private file이어야 한다. 기존 파일이 같은 Selection이면 재검증·영속 재게시할 수 있고,다른 Selection/Native·symlink/hardlink·lock alias·다른 부모는 거절한다. caller의 expected fingerprint와 현재 선택은 그대로 유지한다.
- 기존 `_publish_selection_locked`의 file fsync→rename→directory fsync와 `PersistenceUncertain`을 재사용한다. 해당 writer의 CAS는 목적지 파일용이므로 source current CAS를 그대로 넘기지 않는다. rename 직전의 guard가 caller fault hook 이후 source Legacy CAS와 destination absent-or-same 조건을 다시 검사한다. 협조적 deployment lock 범위 이상을 보장한다고 주장하지 않는다.
- rename 전 실패는 saved selection을 노출하지 않고 current 선택을 바꾸지 않는다. rename 이후 불확실 상태는 restart에서 current/saved를 재검증한다. current가 같은 Legacy이고 saved가 같은 canonical Selection인 경우만 재게시로 durability를 확인한다. 무조건 overwrite/retry하지 않는다.
- 이 동작의 코드·CLI/영속 회귀 수용은 predecessor custody 범위다. 실제 SDK의 FULL operation/post-FULL source/target3축 인계,process stop/join/lease,실제 old-root Serve/search와 default 전환은 별도 수용이다.
