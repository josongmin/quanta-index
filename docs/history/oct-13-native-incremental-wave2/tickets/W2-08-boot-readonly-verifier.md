# W2-08 — Catalog-issued boot selection과 WAL-aware read-only verifier 연결

> HISTORICAL REFERENCE — 2026-10-18 retirement. Current work/status: [Oct-17 SSOT](../../../plans/oct-17-index-closeout/tickets/README.md). Old TODOs, owners, commands and counts below do not authorize new work or qualify current source.


> `HISTORICAL_DESIGN_REFERENCE`. 현재 잔여/상태는 [Oct-17 SSOT](../../../plans/oct-17-index-closeout/tickets/README.md#w2-08)만 따른다. 아래 과거 TODO·진척·실행 순서는 신규 작업을 만들지 않는다. 이 파일에는 상태/담당을 갱신하지 않는다.

기존 요구/큐: R35,R36 / NI-07. 설계 계약은 [RFC](../../../rfc/oct-10-native-incremental-vnext.md), 실행 결과는 [VALIDATION](../../../plans/oct-10-index-closeout/VALIDATION.md)이 소유한다.

## Reuse

[x] `DONE_IMPL` 재사용 경계: Catalog receipt·WAL-aware Verify/Serve·CAS/rollback·SDK two-root. 이 표시의 범위는 명시한 하위 구현이며 티켓 전체 검증 완료를 뜻하지 않는다. 새로운 결함이 확인되지 않으면 재구현·재배정하지 않는다.

## Scope

Catalog-issued selection·protected closure·config receipt를 읽기 전용으로 인증하고 동일 root lease/물리 authority를 Serve에 전달한다.

## Acceptance

- 실제 producer 출력으로 selected request/profile/SourceCut/config와 동일 lease의 Verify→Serve→reopen을 검사한다.
- 기존 fault gold를 재사용하고 빠진 receipt drift/foreign/symlink/alias·취소·postcommit uncertainty·강제 close 오류/launcher interruption만 보강한다.
- receipt의 마지막 directory-FD close 실패는 durable 파일이 남는 postcommit uncertainty일 수 있다. 실패를 무파일 상태나 안전한 재발행으로 해석하지 않으며, 원 receipt·selection·lease와 재시도 자격을 검사한다.
- 기존 root의 bytes/inode/WAL/SHM 무변경과 읽기 전용 거절을 보존한다. Common retirement 수리는 W2-07이 담당한다.

## Source entry points

- [native_boot.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/src/native_boot.rs)
- [boot_manifest.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-catalog/src/native/authority/boot_manifest.rs)
- [capture_format.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-catalog/src/native/authority/capture_format.rs)

공용 parent/export/contract/config/registry 변경은 [단일 writer 규칙](../../../plans/oct-13-native-incremental-wave2/tickets/WRITE-OWNERSHIP.md)을 따른다. 검사 미실행을 기능 부재로 바꾸지 않는다. 결과에는 source·명령·범위·VERIFIED/FAILED/BLOCKED/NOT_RUN/NOT_APPLICABLE을 구분한다.
