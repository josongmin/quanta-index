# 11월 저장소 감사 후속 티켓

2026-11-08 KST. 최초 감사 기준은 `main@7a16f48a2`와 비교 기준 `c32cd1915`다.
완료한 구현은 [ADR](../../../adr/NOV-07-001-native-index-execution-and-custody.md)에 합쳤다.
실행 결과와 원본 복구는 [11월 8일 정리 기록](../../../ARCHIVE-INDEX.md#2026-11-08-november-audit-ticket-closeout)과
[완료 P10](../../../ARCHIVE-INDEX.md#nov-7-audit-p10)을 따른다.

| 티켓 | 현재 상태 |
| --- | --- |
| [P04 게시 배치 계측](P04-publication-batch-proof.md) | `VERIFIED`. 실제 daemon에서 신규 게시·중단 직전 게시·Pending 복구 각각 세 계산을 1회씩 관측했다. |
| [P08 Sidecar 부모 디렉터리](P08-sidecar-parent.md) | `VERIFIED`. mount 거절, held-FD 작업, bind rollback, 정산 후 lease 거절과 삭제·환급 회귀가 통과했다. |
| [P09 IPC 종료 응답과 관측](P09-native-ipc-terminal.md) | 로컬 IPC·SDK 회귀 `VERIFIED`. 설치 DELTA `BLOCKED`. 현재 양쪽 observer 소스를 포함한 Linux 실행 입력이 필요하다. |

착수 전에 HEAD·worktree·실제 작성자와 겹치는 hunk를 대조한다.
기존 [소유권 표](../../oct-22-finalizing/OWNERSHIP.md)와
[bugbash 경계](../../nov-5-indexing-bugbash/WORKPLAN.md#중복-제외와-소유-경계)를 사용한다.
Rust 검증은 기존 단일 실행 경로에서 수행하고 선택·실행·결과 수와 영향 범위를 해당 티켓에 남긴다.
전체 제품 수용과 성능은 [Oct-22 원장](../../oct-22-finalizing/README.md)과
[Native bugbash](../../nov-5-indexing-bugbash/README.md)에서 관리한다.
