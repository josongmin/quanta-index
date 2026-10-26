# W2-07 — 기존 Common physical capability의 생산 경로·삭제 retry 결합

> HISTORICAL REFERENCE — 2026-10-18 retirement. Current work/status: [Oct-17 SSOT](../../../plans/oct-17-index-closeout/tickets/README.md). Old TODOs, owners, commands and counts below do not authorize new work or qualify current source.


> `HISTORICAL_DESIGN_REFERENCE`. 현재 잔여/상태는 [Oct-17 SSOT](../../../plans/oct-17-index-closeout/tickets/README.md#w2-07)만 따른다. 아래 과거 TODO·진척·실행 순서는 신규 작업을 만들지 않는다. 이 파일에는 상태/담당을 갱신하지 않는다.

기존 요구/큐: R10,R11,R12 / NI-09. 설계 계약은 [RFC](../../../rfc/oct-10-native-incremental-vnext.md), 실행 결과는 [VALIDATION](../../../plans/oct-10-index-closeout/VALIDATION.md)이 소유한다.

## Reuse

- [x] `DONE_IMPL`: 기존 physical/custody·checked close/reaper, History 원 budget→실 writer·merge drain, PreparedStateRoot의 원 parent/staging inode와 NOREPLACE cutover source 결합을 재사용한다. 구현 결합과 운영 수용을 구분하며 실제 실행 범위는 기존 VALIDATION이 소유한다.
- [x] `DONE_IMPL`: offline byte copy·funded Catalog snapshot/normalization·incarnation rotation·manifest write는 하나의 destination binder와 원 request budget을 사용한다. snapshot은 실제 source read transaction의 receipt와 대조하고 checked native close 후 admitted FD·parent sync 및 실제폭을 확인한다. 이 source 결합은 실행 수용이나 실제 OS fault qualification을 뜻하지 않는다.

## Scope

Common physical capability·namespace/incarnation custody·checked retirement·History budget·offline target authority를 생산 I/O의 동일 수명에 묶는다.

기존 Common capability/root를 constructor→grow→write→rename→delete/recursive retry까지 보존한다. Native R10–R12 실제 call inventory와 ordinary production의 미결합만 수리한다. FD/정산·offline destination authority·History caller budget은 기존 owner 안에서 결합한다. 신규 global ledger/queue를 만들지 않는다.

## Acceptance

offline backup/restore 계약은 기존 Legacy 운영 경로에 적용한다. Native는 W2-18의 새-root rebuild/reselection 정책을 유지하며 backup/restore 지원을 추가하지 않는다.

- 실제 Catalog/VFS close·shutdown에서 원 child/close 오류를 함께 보존한다. 마지막 lease는 미정산 custody를 숨기며 해제하지 않는다. clean retry 뒤 독립 inode census와 zero live custody를 확인한다.
- binder/root/cap mismatch·hardlink/inode incarnation·foreign replacement를 독립 fixture로 거절한다. 자원 admission이 거절된 경우 mutable open/create/grow 이전에 멈추는지 actual I/O 경계에서 확인한다.
- offline admitted copy부터 snapshot/rotation/manifest-last/deep-open/cutover까지 동일 destination custody·원 budget을 보존한다. 생성 전 고정한 parent와 생성된 staging 원 FD/inode를 cutover까지 유지하고, target 존재/교체는 NOREPLACE로 거절한다. post-rename 재시도는 같은 원 inode에만 parent sync를 허용한다. 원 root lease를 parent sync 완료까지 유지하며, 초기 source FD 및 native close/cutover 실패의 원 custody를 보존한다. deep-open 이후 실제 Catalog receipt로 manifest를 작성한다. 이 namespace 계약은 파일 복사·snapshot의 byte admission을 대신하지 않는다. tiny-cap/cancel/fault에서 source와 partial destination을 검증한다.
- SQLite가 정규화 중 생성했지만 쓰지 않은 WAL은 actual native FD 종료·원래 inode·0바이트 Create·exclusive custody가 확인된 경우에만 기존 unlink→zero-link→parent sync→checked close→Core 부재 정산을 사용한다. populated WAL·Resize·교체 inode를 이 경로로 삭제하지 않는다. cleanup 조건과 reader claim은 같은 잠금 안에서 검사한다. adapter 경로는 원 lease의 canonical namespace를 사용한다.
- restore의 Catalog 논리 digest·row 수를 유지한다. WAL/DELETE 정규화의 고정 헤더 counter(24..28, 92..96)를 제외한 DB 바이트와 나머지 객체는 동일해야 한다. 각 root manifest는 그 root에서 실제로 publish한 Catalog의 전체 SHA·byte width를 검증하며, source backup 바이트 보존은 별도 oracle로 유지한다.
- History long IO는 caller budget/cancel/deadline과 pending 정산을 보존한다. release-once·uncertain unlink·recursive directory 재시도·foreign incarnation·restart를 검사한다.
- long pin/공간 압박/WAL/object/scratch 경쟁과 실제 fault/crash 후 drain을 검사한다. synthetic EIO routing은 실제 OS close/ENOSPC/fsync fault와 구분한다.
- Catalog WAL이 회수 전이 자체의 공간을 막지 않도록 기존 automatic/manual reclaim의 보호된 funding으로 checkpoint한다. 원 mutex·request budget·IO reservation 상한을 유지한다. 관측한 WAL이 원 IO/남은 work allowance에 못 맞으면 IO 없이 선택적 checkpoint만 건너뛰고 원 GC/세션 복구를 계속한다. 필수 metadata 예산·cancel/deadline·실제 SQLite snapshot/IO 오류는 전파한다. 논리 read ticket을 풀어 WAL을 줄이지 않는다. 고정 SQL 데이터·WAL 실제폭·무예산/IO 경계와 사용량 보존·실제 SQLite snapshot 보존과 혼합 cap 복구를 함께 검증한다. 초과 WAL 자체의 축소나 장시간 자원 수용을 이 skip으로 입증하지 않는다.
- 실제 offline 쓰기 실패는 커널 errno와 실패 경계를 검사한다. 자식 프로세스의 파일 크기 제한(`EFBIG`)과 별도 32–128MiB 볼륨의 공간 부족(`ENOSPC`)을 구분하고, 소스 fingerprint·미게시 staging·lease 재획득·명시적 정리 후 재시도를 확인한다. 일반 호스트 임시 디렉터리를 채우지 않는다. 격리 볼륨 실행은 [physical-enospc.py](../../../../tools/testing/physical-enospc.py)를 사용한다.
- opt-in 확장 압박은 기존 helper·동일 cap을 사용한다. 최소4 completed cycles와36초 실제 작업을 모두 요구하고 유휴 sleep으로 시간을 채우지 않는다. 원120초 pin·10초 phase retry/watchdog·30초 RPC deadline·4MiB spare·128KiB body를 유지하며 pin 취득 뒤90초 협력적 cutoff와 새 cycle용45초 잔여 예산을 검사한다. syscall 선점 상한은 아니다. 고유 body는 kind/cycle/index header로 유일화한다. 각 cycle의 실제 cap 거절·재입장·폐기 object 부재와 old-pin bytes/current serving, 마지막 pin 해제 후 old object 회수·survivor·quiescent census를 검사한다. logical-file 관측 최대·debt·drain 시간은 steady-state SLO/allocated-block peak/물리 write bytes 증거가 아니다.
- 혼합 압박 회귀는 실제 read ticket을 유지한 상태에서 서로 다른 content의 staging을 같은 physical cap까지 반복하고, typed refusal·동일 cap 재입장·마지막 pin 해제 후 회수·재시작 census를 검사한다. 이 bounded 회귀를 장시간 운영 수용이나 aggregate RSS·allocated disk peak 측정으로 승격하지 않는다.
- Linux 원FD fault 회귀는 별도64MiB FUSE backend의 kernel-visible EIO를 실제 binder에 전달한다. `QIDX_REAL_FAULT_MOUNT`는 `fuse.passthrough-fault` mount, `QIDX_REAL_FAULT_CONTROL`은 mount 외부 정규 파일이며 테스트의 `TMPDIR`만 fault mount를 사용한다. control은 `fsync <basename>`, `flush <basename>`, `off none`이다. fsync 실패 뒤 같은 원FD/inode와 전체 예약을 보존하고 fault 해제 후 같은 inode에서 sync·close·삭제·census를 끝내야 한다. checked close 실패는 소비한 numeric FD를 재시도하지 않고 전체 예약·불확실성·root quarantine을 보존해야 한다. FUSE callback EIO는 실제 커널 오류 전달 증거이며 물리 매체 내구성 장애 증거는 아니다. 빌드 임시 파일은 이 제한 볼륨 밖에 둔다.

## Source entry points

- [History publish.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-lexical/src/history_text_index/publish.rs)
- [state_migration.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/src/state_migration.rs)
- [cutover.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd/src/app/state_format/cutover.rs)
- [lib.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/src/lib.rs)
- [canonical_publication_admission.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/src/canonical_publication_admission.rs)
- [store.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-lexical/src/lexical_store/store.rs)
- [offline OS fault tests](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/tests/state_migration_owner_v1/physical_os_faults.rs)
- [Native mixed pressure test](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/tests/native_index_uds/physical_mixed_pressure.rs)
- [Catalog WAL checkpoint](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-catalog/src/connection.rs)
- [Native reclaim pump](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/src/native_pump.rs)
- [original-FD WAL truncate sync](/Users/songmin/Documents/code-new/quanta-index/vendor/libsqlite3-sys-0.38.2/quanta/quanta_capacity_vfs.c)

공용 parent/export/contract/config/registry 변경은 [단일 writer 규칙](../../../plans/oct-13-native-incremental-wave2/tickets/WRITE-OWNERSHIP.md)을 따른다. 검사 미실행을 기능 부재로 바꾸지 않는다. 결과에는 source·명령·범위·VERIFIED/FAILED/BLOCKED/NOT_RUN/NOT_APPLICABLE을 구분한다.
