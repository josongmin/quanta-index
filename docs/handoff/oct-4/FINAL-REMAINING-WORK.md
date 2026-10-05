# OCT-04 통합 잔여 요약

원본5 handoff와 완료 구현·회귀 이력은 [Accepted ADR 4개](../../adr/README.md#oct-05-implemented-contracts)에 압축했다.
원문과 과거 실행 SHA/명령/실패 이력은 [Git 복구 인덱스](../../ARCHIVE-INDEX.md#oct-05-handoff-compaction)에서 회수한다.
**실제 작업·29개 scope 판정의 단일 기준:** [잔여 인덱스](../../plans/oct-4-parallel-closure/tickets/INDEX.md).
[담당·인계](../../plans/oct-4-parallel-closure/README.md) · [남은 웨이브](../../plans/oct-4-parallel-closure/WAVES.md).

## 코드 잔여

- [I0-03](../../plans/oct-4-parallel-closure/tickets/INDEX.md#o4-i0-03): P11 operational producer/recipes 미구현.
  Deploy/activate/restore-forward actual actions와 독립 pre/post 성공 판정 계약·authorized target 입력이 필요하다.
- [E1-07](../../plans/oct-4-parallel-closure/tickets/INDEX.md#o4-e1-07),
  [E4-02](../../plans/oct-4-parallel-closure/tickets/INDEX.md#o4-e4-02),
  [E4-04](../../plans/oct-4-parallel-closure/tickets/INDEX.md#o4-e4-04),
  [E4-07](../../plans/oct-4-parallel-closure/tickets/INDEX.md#o4-e4-07): 실제 비용/정책 실패 후 채택할 조건부 변경.
- 나머지23개 코드 경로는 구현 존재/수리 완료, E3-02는 현 Accepted 계약에서 비적용이다.
  이 분류는 전체29개 요청 종료나 최신source·품질·성능·배포 qualification이 아니다.

## 실제 후속 작업

| 담당 | 해야 할 일 | 현재 입력/검증 경계 |
| --- | --- | --- |
| E1 | SQL/Zellij 최종 판단·Tailscale rubric, current9 신규742pairs 검수·labels/admissions·재채점 | 최소1,364pairs 잔여; 실제 model quota/rubric 필요. SQL/Zellij는 C3 평가용 source repositories |
| E1 | 다른 name/typo cells·독립 holdout/gold·license/exposure/acceptance | Gin exact1,196 완료 범위 보존; candidate12repo는 source-only 미승인 |
| E2 | 나머지3repo/다른 lanes required captures/replays/joins, actual OpenGrok reader/source/index authority | 기존9repo raw는 bound epoch의 diagnostic; readonly disk/API는 loaded-reader/whole universe가 아님 |
| E4 | causal/full-caller/scanner A/B·default capacity·정식 반복 성능·정책 RCA | 기본large30s timeout·xlarge4M cap gate FAILED 보존; Darwin frequency admission BLOCKED |
| I0 | 최신 source Contract/SDK/CI·exact producer pair·real-provider/Linux/state·P11 actions | 과거 owner/OS-child/2UID component proof와 shipping/release/운영 구분 |

완료된 구현을 재작성하지 않는다. Unknown/unresolved를0점·no-answer로 채우거나
history·focused tests·문서 정리를 새로운 제품 qualification으로 표시하지 않는다.
