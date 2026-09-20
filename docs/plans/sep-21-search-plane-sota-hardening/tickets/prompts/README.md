# SEP-21 Implementation Prompt Runbook

이 디렉터리의 파일은 새 Codex task에 그대로 복붙하는 실행 프롬프트다. 각 task는 먼저
[공통 실행 계약](COMMON-EXECUTION-CONTRACT.md)을 읽고 적용한다. 프롬프트 작성/정적 검토 task에서는 이 문서들을
구현 권한으로 해석하지 않는다.

## 절대 실행 순서

| Step | Ticket | Prompt | Required source gate | Output/proof |
|---|---|---|---|---|
| 0 | S21-00/13A | [P00](P00-foundation-gate.md) | current source 재검증 | corrected M0 contract / `p00-authority-freeze` |
| 1 | S21-01A | [P01A](P01-canonical-identity.md) | corrected M0 source-bound handoff | identity/codec checkpoint / `p01-canonical-identity` |
| 2A | S21-03 | [P02A](P02A-repomap-compiler.md) | exact P01A base, isolated worktree | compiler commit / `p02a-repomap-compiler` |
| 2B | S21-04 | [P02B](P02B-operation-journal.md) | exact P01A base, isolated worktree | global sequence/journal commit / `p02b-operation-journal` |
| 2I | S21-03/04 | [P02I](P02I-integration-gate.md) | P02A+P02B commits/handoffs | same-HEAD M1 integration handoff |
| 3 | S21-01B/02 | [P03](P03-candidate-activation.md) | P02I clean integration HEAD | live layout/quarantine/activation M1 closure / `p03-candidate-activation` |
| 4 | S21-05 | [P04](P04-read-view.md) | P03 handoff | M2 stack A / `p04-read-view-lifetime` |
| 5 | S21-06 | [P05](P05-query-truth.md) | P04 stack SHA | M2 stack B / `p05-query-truth` |
| 6 | S21-07 | [P06](P06-sdk-binding.md) | P05 stack SHA | M2 consumer closure / `p06-sdk-binding` |
| 7 | S21-08 | [P07](P07-provider-boundary.md) | M2 handoff | M3 interface checkpoint / `p07-provider-boundary` |
| 8 | S21-09 | [P08](P08-runtime-supervisor.md) | P07 stack SHA | M3 lifecycle checkpoint / `p08-runtime-supervisor` |
| 9 | S21-10 | [P09](P09-control-readiness.md) | P08 + P02B handoffs | M3 closure / `p09-control-readiness` |
| 10 | S21-11 | [P10](P10-state-migration.md) | P03 + P08 lease authority | M4 state workflow / `p10-state-migration` |
| 11 | S21-12 | [P11](P11-cross-repo-cutover.md) | P10 + M2 receipts | M4 source-pair handoff / `p11-cross-repo-cutover` |
| 12 | S21-13B | [P12](P12-final-qualification.md) | all current handoffs | M5 aggregate / `p12-final-qualification` |

## 복붙 방법

각 새 task에는 해당 step의 prompt 파일 전체를 그대로 붙여넣는다. 여러 lane prompt를 한 task에 합치지 않는다.
P02A/P02B만 동일 P01A result SHA의 별도 worktree에서 동시에 시작한다. 그 외 step은 predecessor checkpoint,
handoff JSON, proof manifest가 current source에 결속됐음을 확인한 뒤 하나씩 시작한다.

현재 source에서는 P01A를 먼저 실행하면 안 된다. P00이 canonical encoding, closed error-code set, phase ownership,
quarantine sequence source와 proof selector를 corrected handoff로 재동결해야 한다.

## 구조적 경계

- P01A는 S21-01 전체 완료가 아니다. pure identity/codec/address/security primitive checkpoint만 만든다.
- P02B가 state-root-global sequence allocator와 generic event ledger의 sole owner다.
- P03이 live RepoMap persistence, quarantine projection, filesystem activation 삭제, catalog authority를 한 번에 cutover한다.
- P03이 끝나야 S21-01/S21-02를 함께 닫을 수 있다.
- legacy `activations/` directory의 변환/삭제는 P10 offline importer owner다. P03 runtime은 old root를 mutation 전에
  거부하고 legacy artifact를 수정하지 않는다.

## 공통 운영 규칙

- 시작 시 HEAD, branch/upstream, dirty paths/digest와 prerequisite artifact를 재-freeze한다.
- dirty 파일을 덮어쓰지 않는다. owner가 불명확한 겹침은 구현하지 말고 exact path/diff를 보고한다.
- predecessor가 `planned`이거나 required checkpoint/source-bound handoff/proof가 없으면 우회 구현하지 않고
  `BLOCKED`로 종료한다. 대화의 `DONE`은 gate가 아니다.
- compatibility shim, dual read/write, route-local 보정, optional transitional field를 만들지 않는다.
- ticket 전체 closure와 intermediate checkpoint를 구분한다.
- canonical verification은 `Justfile`과 `./scripts/cargow`를 사용한다.
- static, focused, owner-local, process, external, deploy, activation evidence를 서로 승격하지 않는다.
- local disposable process/state-root를 넘는 external repo write, provider egress, migration/cutover, deploy/activate는
  각각 별도 명시 승인이 필요하다.

## 병렬 lane handoff

P02A와 P02B는 각각 exact source SHA/dirty digest, 변경 파일과 owner symbol, frozen types/invariants, command와
selected/executed/passed/failed/ignored counts, NOT_RUN/blocker, P03 소비 API/fixture를 남긴다. P02I가 두 checkpoint를
같은 clean integration HEAD에 합치고 양쪽 proof를 재실행하기 전 P03을 시작하지 않는다.
