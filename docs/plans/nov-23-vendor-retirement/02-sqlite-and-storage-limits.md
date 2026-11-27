# SQLite 절체와 저장소 제한

## 구현 경계

| 경계 | 현재 동작 |
| --- | --- |
| Cargo 공급자 | 루트의 patch, SQLite 소스 사본, Runtime private FFI·등록·test hook을 제거했다. 지원 그래프는 crates.io의 `rusqlite 0.41.0`과 번들 `libsqlite3-sys 0.39.1`을 사용한다. [Cargo.toml](../../../Cargo.toml) · [Cargo.lock](../../../Cargo.lock) |
| Linux 운영 쓰기 | Native serve와 쓰기 selection은 quota 설정이 있어야 진행한다. Python 설치와 활성 switch는 selection 게시 전에 동일 제품 ELF로 writer 입장을 확인한다. held StateRoot·mount·XFS project 및 hard byte/inode 검증을 통과해야 stock Catalog·Scratch를 연다. 데몬은 FD 상한, private socket, Root 안의 `sqlite-tmp` 및 쓰기 격리를 사용한다. [기동](../../../crates/quanta-index-searchd-runtime/src/native_product.rs) · [quota authority](../../../crates/quanta-index-searchd/src/app/state_root_quota.rs) · [SQLite I/O](../../../crates/quanta-index-searchd-runtime/src/native_scratch_io.rs) |
| 기존 no-quota 구성 | 쓰기 시작 전에 거절한다. Linux에서 기존 Root의 오프라인 읽기·export로 이관 입력을 확보하고 새 quota Root로 복구한다. 실제 승인 Root와 데이터가 있는 History 이관에는 서명 설치 수용이 남아 있다. [운영 런북](../../operator/state-cutover-runbook.md) |
| 이전 Root Catalog | Linux는 held directory/Main/WAL을 보호 FD VFS로 읽는다. macOS는 오프라인 Root와 Catalog 잠금 아래 held Main/WAL을 private 임시 경로로 복사하고 stock SQLite로 읽는다. 복사와 읽기 앞뒤에서 원본 identity를 대조한다. 두 플랫폼에서 설치용 Catalog 해시는 held 디렉터리와 읽기 예산을 사용한다. [Runtime 이전 Root](../../../crates/quanta-index-searchd-runtime/src/native_boot/legacy_read_factory.rs) · [Linux FD](../../../crates/quanta-index-catalog/src/native/authority/read_only/protected_fd.rs) · [macOS 스냅샷](../../../crates/quanta-index-catalog/src/native/authority/read_only/portable_snapshot.rs) · [Catalog 해시](../../../crates/quanta-index-searchd-runtime/src/native_boot/catalog_digest.rs) |
| 테스트와 출하 | 임시 Root에서 실행하는 무쿼터 통합 테스트는 별도 fixture 바이너리를 쓴다. 제품 쓰기 사전 검사는 실제 데몬의 진입점에서만 허용한다. 제품 normal/build 그래프에서 fixture와 private VFS 기능을 거절한다. [Runtime 진입점](../../../crates/quanta-index-searchd-runtime/src/native_product.rs) · [제품 그래프 검사](../../../tools/ci/check-tantivy-retirement.py) · [XFS 프로세스 테스트](../../../crates/quanta-index-searchd-runtime/tests/native_product_process/native_boot/stock_xfs.rs) |

## 동작 변경과 수용 조건

- SQLite 내부 DB/WAL/SHM syscall마다 수행하던 요청별 선차감을 제거했다. 운영 경계는 작업·메모리 입장, daemon FD 상한, XFS hard block/inode로 구성한다. SQL `FULL`·`IOERR`의 원인 분류, 게시된 head와 ACK, 같은 operation의 정산·재시도는 별도 검증한다. 실제 quota 고갈은 등록 전과 ANN 봉인 이후를 나눠 확인한다.
- Verifier의 연결, Root FD 전송, 응답 수신은 요청 마감과 로컬 5초 상한 중 이른 시각을 공유한다. 응답이 지연되는 동안 도착한 일부 바이트로 마감이 연장되지 않는다. XFS 트리 스캔은 열거가 돌려준 inode를 `statat`과 실제로 연 FD의 메타데이터에 결속한다. 설치·활성 switch는 검증 서브프로세스 중 운영자 인터럽트를 취소로 처리한다. [Verifier IPC](../../../shared/quanta-state-root-quota/src/ipc.rs) · [XFS 순회](../../../crates/quanta-index-searchd/src/app/state_root_quota_tree.rs) · [런처](../../../tools/deployment/launcher.py)
- Catalog lock 및 부모 디렉터리 sync, checked close, 명시적 관리 파일의 예약과 회수는 SQLite 엔진 밖에서 계속 검사한다. 닫힘이 불확실한 원 FD의 charge를 유지하고 같은 숫자 FD를 다시 닫지 않는다. 장기 reader pin 해제와 retire/replay는 실제 stock에서 확인한다.
- Scratch는 할당 시 디렉터리의 device·inode를 기록한다. 정리할 때 held namespace 아래에서 같은 디렉터리를 열어 확인하고, 교체·hardlink·안전하지 않은 권한으로 정리가 거절되면 논리 예약을 유지한다. [할당](../../../crates/quanta-index-storage/src/scratch.rs) · [회수](../../../crates/quanta-index-storage/src/scratch/stock.rs)
- macOS 복구는 오프라인을 전제로 한다. 원 Root에서 DB/WAL/SHM 변경 0건, committed WAL frontier, 입력 교체·잠금 경합·미정산 ingest·예산 거절을 검증한다. 서명 설치의 이전 Root는 실제 History member/instance/cut과 Source V9 및 마지막 확정 ACK까지 검증한다.
- WAL frame pack과 append/checkpoint 배치 최적화는 순정 쓰기로 복귀했다. 1/4/8 KiB 페이지에서 old reader의 PASSIVE backfill, TRUNCATE 후 cold reopen과 integrity를 대조한다. syscall 수와 p95 비교는 같은 WAL profile로 수행한다.
- 물리 한도 64 GiB에는 새 Root와 남아 있는 이관 Root가 사용하는 공간까지 포함한다. 데몬은 각 Root의 project hard quota만 검사한다. D 설치 인수에서 이전·신규 Root의 합산 한도를 확인하고 실제 initial FULL → DELTA → 조회 → History → 재시작을 연결한다. scratch 예약은 4 GiB 이하로 유지한다. [설치 잔여](05-integration-and-release.md#현재-남은-코드)

## macOS 오프라인 잔여

- Catalog Main·WAL 보호 읽기에 이어 macOS의 admitted inventory와 기존 directory metadata를 같은 Root lease, FD custody, WorkBudget에 연결했다. Darwin은 보유 directory FD에서 이름·inode의 bounded batch를 읽는다. 이후 held FD의 자식 메타데이터와 inode를 대조하고 directory identity를 재확인한다. Native Runtime의 Catalog 입장도 보유 Root 아래 기존 보호 reader를 호출한다. [Darwin syscall 경계](../../../shared/quanta-explicit-fd-process/src/darwin_directory.rs) · [Runtime inventory](../../../crates/quanta-index-searchd-runtime/src/publication_inventory/admitted.rs) · [metadata 검증](../../../crates/quanta-index-searchd-runtime/src/publication_file_capacity/read_only_tree.rs) · [Catalog admission](../../../crates/quanta-index-searchd-runtime/src/native_scratch_io.rs)
- macOS 승인 조건은 이전 Native/Legacy Source·History의 CLI 프레임·digest, 원본 변경 0과 경합·교체·예산 초과 거절이다. Linux 제품 writer와 승인된 RepoMap restore-forward는 Linux XFS의 별도 절체 경계에서 판정한다. [실행 판정](10-execution-work-breakdown.md#11월-26일-재감사)

[현재 검증과 이전 후보 결과](10-execution-work-breakdown.md#sqlite) · [서명·제품 최종 인수](05-integration-and-release.md#구신-설치와-복구)
