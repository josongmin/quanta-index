# SEP-21 Implementation Prompt Runbook

이 디렉터리의 파일은 새 Codex task에 그대로 복붙하는 실행 프롬프트다.

## 실행 순서

| Step | Prompt | 병렬성 | Merge/checkpoint gate |
|---|---|---|---|
| 0 | [P00](P00-foundation-gate.md) | 단독 | M0: authority 결정 + proof skeleton |
| 1 | [P01](P01-canonical-identity.md) | 단독 | M1a: canonical identity/layout |
| 2A | [P02A](P02A-repomap-compiler.md) | P02B와 병렬 | compiled candidate contract |
| 2B | [P02B](P02B-operation-journal.md) | P02A와 병렬 | prepared mutation/journal contract |
| 3 | [P03](P03-candidate-activation.md) | 2A와 2B 모두 merge 뒤 | M1: sole RepoMap authority |
| 4 | [P04](P04-read-view.md) | 단독 | pinned resource lifetime |
| 5 | [P05](P05-query-truth.md) | P04 뒤 | completeness/cursor/provenance |
| 6 | [P06](P06-sdk-binding.md) | P05 뒤 | M2: consumer-bound query truth |
| 7 | [P07](P07-provider-boundary.md) | 단독 | provider admission/executor interface |
| 8 | [P08](P08-runtime-supervisor.md) | P07 뒤 | supervised task/process ownership |
| 9 | [P09](P09-control-readiness.md) | P08 뒤 | M3: auth/readiness/observability |
| 10 | [P10](P10-state-migration.md) | 단독 | offline migration/restore authority |
| 11 | [P11](P11-cross-repo-cutover.md) | P10 뒤 | M4: producer/daemon terminal receipt |
| 12 | [P12](P12-final-qualification.md) | 마지막 | M5: final proof graph |

## 운영 규칙

- P02A/P02B만 기본 병렬 허용한다. shared contract/baseline 파일은 시작 전에 한 lane에 독점 배정한다.
- 각 prompt는 시작 시 HEAD, branch/upstream, dirty paths/digest와 prerequisite artifact를 재-freeze한다.
- dirty 파일을 덮어쓰지 않는다. owner가 불명확한 겹침은 구현하지 말고 exact path와 diff를 보고한다.
- predecessor가 `planned`이거나 required contract/handoff가 없으면 우회 구현하지 않고 `BLOCKED`로 종료한다.
- compatibility shim, dual read/write, route-local 보정, optional transitional field를 만들지 않는다.
- 각 lane은 자기 ticket의 상태를 proof 없이 `done`으로 바꾸지 않는다.
- canonical verification은 `Justfile`과 `./scripts/cargow`를 사용한다. bare `cargo`는 repo exception rail만 허용한다.
- static, focused, owner-local, process, external, deploy, activation evidence를 서로 승격하지 않는다.

## 병렬 lane handoff

P02A와 P02B는 각각 다음을 남긴다.

- exact source SHA와 dirty digest
- 변경 파일 목록과 owner symbol
- frozen public/internal types와 semantic invariants
- 실행한 command, selected/executed/passed/failed/ignored counts
- 미실행 proof와 남은 blocker
- P03이 소비할 API/fixture 목록

두 lane의 handoff를 통합 검토하기 전 P03을 시작하지 않는다.
