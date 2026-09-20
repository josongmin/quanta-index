# SEP-21 Implementation Prompt Runbook

이 디렉터리의 파일은 새 Codex task에 그대로 복붙하는 실행 프롬프트다. 모든 task는 먼저
[공통 실행 계약](COMMON-EXECUTION-CONTRACT.md)을 읽고 적용한다. 프롬프트 작성/정적 검토 task에서는 이 문서들을
실행 권한으로 해석하지 않는다.

## 실행 순서

| Step | Ticket | Prompt | Required source gate | Output/proof |
|---|---|---|---|---|
| 0 | S21-00/13A | [P00](P00-foundation-gate.md) | current source 재검증 | M0 / `p00-authority-freeze` |
| 1 | S21-01 | [P01](P01-canonical-identity.md) | M0 source-bound handoff | pre-M1 identity checkpoint / `p01-canonical-identity` |
| 2A | S21-03 | [P02A](P02A-repomap-compiler.md) | exact P01 base, isolated worktree | compiler commit / `p02a-repomap-compiler` |
| 2B | S21-04 | [P02B](P02B-operation-journal.md) | exact P01 base, isolated worktree | journal commit / `p02b-operation-journal` |
| 2I | S21-03/04 | [P02I](P02I-integration-gate.md) | 2A+2B commits/handoffs | same-HEAD M1 integration handoff |
| 3 | S21-02 | [P03](P03-candidate-activation.md) | P02I clean integration HEAD | M1 authority / `p03-candidate-activation` |
| 4 | S21-05 | [P04](P04-read-view.md) | P03 handoff | M2 stack A / `p04-read-view-lifetime` |
| 5 | S21-06 | [P05](P05-query-truth.md) | P04 stack SHA | M2 stack B / `p05-query-truth` |
| 6 | S21-07 | [P06](P06-sdk-binding.md) | P05 stack SHA | M2 consumer closure / `p06-sdk-binding` |
| 7 | S21-08 | [P07](P07-provider-boundary.md) | M2 handoff | M3 interface checkpoint / `p07-provider-boundary` |
| 8 | S21-09 | [P08](P08-runtime-supervisor.md) | P07 stack SHA | M3 lifecycle checkpoint / `p08-runtime-supervisor` |
| 9 | S21-10 | [P09](P09-control-readiness.md) | P08 + P02B handoffs | M3 closure / `p09-control-readiness` |
| 10 | S21-11 | [P10](P10-state-migration.md) | M1 + P08 lease authority | M4 state workflow / `p10-state-migration` |
| 11 | S21-12 | [P11](P11-cross-repo-cutover.md) | P10 + M2 receipts | M4 source-pair handoff / `p11-cross-repo-cutover` |
| 12 | S21-13B | [P12](P12-final-qualification.md) | all current handoffs | M5 aggregate / `p12-final-qualification` |

## 운영 규칙

- P02A/P02B만 기본 병렬 허용한다. 같은 P01 base의 별도 worktree/branch를 사용하며 shared
  contract/baseline/inventory/generated docs는 P02I owner만 쓴다.
- 각 prompt는 시작 시 HEAD, branch/upstream, dirty paths/digest와 prerequisite artifact를 재-freeze한다.
- dirty 파일을 덮어쓰지 않는다. owner가 불명확한 겹침은 구현하지 말고 exact path와 diff를 보고한다.
- predecessor가 `planned`이거나 required checkpoint commit/source-bound handoff/proof가 없으면 우회 구현하지 않고
  `BLOCKED`로 종료한다. 대화의 `DONE`은 gate가 아니다.
- compatibility shim, dual read/write, route-local 보정, optional transitional field를 만들지 않는다.
- 각 lane은 자기 ticket의 상태를 proof 없이 `done`으로 바꾸지 않는다. 상태 vocabulary는 공통 실행 계약의 네 값만
  사용한다.
- canonical verification은 `Justfile`과 `./scripts/cargow`를 사용한다. bare `cargo`는 repo exception rail만 허용한다.
- static, focused, owner-local, process, external, deploy, activation evidence를 서로 승격하지 않는다.
- local disposable process/state-root를 넘는 external repo write, provider egress, migration/cutover, deploy/activate는
  각각 별도 명시 승인이 필요하다.

## 병렬 lane handoff

P02A와 P02B는 각각 다음을 남긴다.

- exact source SHA와 dirty digest
- 변경 파일 목록과 owner symbol
- frozen public/internal types와 semantic invariants
- 실행한 command, selected/executed/passed/failed/ignored counts
- 미실행 proof와 남은 blocker
- P03이 소비할 API/fixture 목록

두 lane의 handoff와 checkpoint commit을 P02I가 같은 clean integration HEAD에 합치고 양쪽 proof를 재실행하기 전
P03을 시작하지 않는다.
