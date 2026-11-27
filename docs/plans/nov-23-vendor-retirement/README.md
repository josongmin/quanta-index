# 외부 라이브러리 포크 퇴역

2026-11-26 KST. **포크 소스 13/13 퇴역.** 루트 lock과 데몬은 crates.io의 `rusqlite 0.41.0`·`libsqlite3-sys 0.39.1`을 사용한다. Linux 운영 쓰기는 XFS project quota 계약을 적용한다. [소스·검증 기록](10-execution-work-breakdown.md)

Q의 writer preflight, 제품 XFS 하네스, macOS 오프라인 inventory와 Cargo 정책을 반영했다. Linux 실제 XFS에서 preflight, P03·P04·P10, RepoMap 구 wire, Small scale을 집중 검증했다. macOS 로컬 Source·History CLI도 통과했다. 승인된 Q/S의 서명 설치·실제 이전 Root 이관, Product4K·E07·품질 및 전체 커버리지는 각 원장에서 판정한다. S warm DELTA는 기존 INC‑01/S02가 담당한다. [작업 상태와 수용 조건](05-integration-and-release.md#현재-남은-코드)

| 경계 | 문서 |
| --- | --- |
| SQLite 절체 | Linux 쓰기와 no-quota 이관, macOS 이전 Root의 Catalog 읽기·export 경계, 작업 예약과 물리 제한 계약. [SQLite와 저장소 제한](02-sqlite-and-storage-limits.md) |
| 제품 인수 | 구·신 서명 설치와 실제 History, 초기 게시·Product4K, warm DELTA, 품질과 E07의 담당 원장 및 합격 조건. [출하 인수](05-integration-and-release.md) |
| 검증 | 최종 소스에서 실행한 검사와 이전 고정 후보의 제품·XFS 결과. [실행 기록](10-execution-work-breakdown.md) |
