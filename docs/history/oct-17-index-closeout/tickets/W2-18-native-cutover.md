# W2-18 — 실제 target 인계·Native 전환과 boot 수용

> HISTORICAL REFERENCE — 2026-10-18 retirement. Current work/status: [Oct-17 SSOT](../../../plans/oct-17-index-closeout/tickets/README.md). Old TODOs, owners, commands and counts below do not authorize new work or qualify current source.


상태는 [SSOT](../../../plans/oct-17-index-closeout/tickets/README.md#w2-18)만 갱신한다. 이전 W2-08 verifier 잔여를 포함한다.

## Remaining scope

- W2-05의 authoritative FULL operation/target issuer 결과를 새-root rebuild→select/receipt→격리 candidate Serve에 인계한다.
- Legacy default selection→fresh Native root의 전환 경로를 기존 launcher/CAS authority에 연결한다. 현재 Native→Native switch를 Legacy 지원으로 계산하지 않는다.
- 새 rollback old-root Serve/search·두 차례 restart/receipt 재선택과 receipt final directory-close fault를 실제 source에서 수용한다. 기존 회귀는 재구현하지 않는다.

## Reuse and sources

- [native CLI](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd/src/cli/native.rs): Select/Verify/Serve 계약.
- [native_boot.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/src/native_boot.rs): Catalog candidate 인증·selection·receipt.
- [launcher.py](/Users/songmin/Documents/code-new/quanta-index/tools/deployment/launcher.py): 두 Native root CAS switch/rollback. current/candidate Native receipt 요건과 Legacy caller를 함께 대조한다.
- [native boot fixture](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/tests/native_product_process/native_boot.rs)·[receipt](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/src/native_boot/receipt.rs): 기존 실제 UDS/rollback·fault oracle.

select/Verify/Serve·two-root rebuild·Native CAS rollback은 구현돼 있다. 새 launcher/target issuer·제자리 추측 migration·Native backup/restore는 범위가 아니다.

## Sequence and acceptance

BOOT는 launcher/CLI/receipt exact hunk, SDK/INT는 원 FULL target 인계를 소유한다. target 계약 동결 뒤 독립 격리 준비가 가능하며 W2-19 A를 기다려 fixture 개발을 막지 않는다.

1. actual producer target으로 same lease/identity·foreign/symlink/alias/incomplete 거절, Verify→Serve/search/reopen·cross-root rollback·crash/fault를 검사한다.
2. W2-19 A가 opt-in candidate의 기능/비용과 격리 리허설을 수용한다.
3. A 후 실제 default routing switch와 경쟁 head/lifetime consumer 퇴역을 수행한다.
4. W2-19 B가 실제 default smoke/rollback·coherent integration·remote CI/release를 판정한다.

기존2/2 Native-root 이력과 이후 추가 회귀 NOT_RUN은 서로 다른 source/scope다. Common close/fault 수리는 기존 PHYSICAL 결과를 소비하고 재작성하지 않는다.

## Implementation packet

- **즉시 준비:** BOOT 담당이 현재 Legacy/Native selection 입력과 launcher의 CAS/receipt 전제, 기존 격리 rebuild/rollback·새 close-fault fixture를 대조한다. SDK 발급 target을 추측하거나 직접 발급하지 않는다.
- **계약 후 구현:** W2-05의 FULL operation/target tuple을 select/receipt/Serve로 전달한다. Legacy→fresh Native의 허용 조건·이전 selection 보존·실패 복구를 기존 CAS authority에 연결한다.
- **정적 완료:** foreign/symlink/incomplete 거절, target/source identity·동시 selection CAS·rollback 원 root/receipt 및 cleanup을 실제 caller에서 대조한다. Native→Native 기존 경로는 재작성하지 않는다.
- **반환물:** SDK target 소비 위치·launcher exact hunk·candidate/rollback 절차·실행할 기존 fixture. 격리 candidate 실행은 W2-19 A 전에 가능하며 실제 default 변경은 A 뒤다. deploy/activate/rollback 실 target/observer 입력은 W2-19의 별도 조건이다.
