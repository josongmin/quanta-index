# SEP-21 Lane Execution Contract

이 문서는 P00~P12 실행 프롬프트의 공통 강제 계약이다. 각 lane prompt와 함께 읽고 적용한다. 이 계약과
lane prompt/ticket이 충돌하면 Accepted ADR과 `docs/adr/SEP-21-DECISION-REGISTRY.md`가 우선한다. stale type/error
이름을 임의 adapter로 맞추지 말고 owner 문서를 고치거나 `BLOCKED`로 종료한다.

## 1. Task mode

- 이 프롬프트를 별도 실행 task에 붙여넣은 경우에만 구현·owner-local 검증·checkpoint commit을 수행한다.
- 프롬프트 작성/정적 감사 task에서는 source edit, build, test, process, provider, migration을 실행하지 않는다.
- 각 lane은 RCA와 owner repair를 수행한다. compatibility shim, dual decoder/read-write, optional transition field,
  route-local 보정, zero/sentinel authority는 금지한다.

## 2. Mandatory preflight

패치 전에 다음을 기록한다.

- full HEAD, branch, upstream 또는 `null`, merge-base 또는 `null`
- staged diff, unstaged diff, untracked path와 scoped untracked-content digest
- lane owner path의 base blob/hash와 기존 dirty owner
- predecessor checkpoint commit, proof ID, manifest path/digest, exported contract
- `tools/ci/proof-authority.toml`의 현재 lane command/profile/target/filter/required host

owner path가 시작 시 이미 dirty이거나 다른 lane이 소유하면 수정하지 말고 exact path/diff를 보고해 `BLOCKED`로
끝낸다. 대화의 `DONE` 문구는 gate가 아니다. predecessor commit과 source-bound handoff/proof가 current source와
일치해야 한다.

## 3. Write and integration discipline

- 한 lane은 명시한 owner allowlist만 수정한다. 추가 owner가 필요하면 먼저 범위와 충돌을 보고하고 중단한다.
- `git add -A`, stash, reset, unrelated formatting/cleanup, 다른 owner의 dirty 수정은 금지한다.
- P02A/P02B만 병렬이다. 동일 P01 base SHA의 별도 worktree/branch에서 수행한다.
- shared contract, public API baseline, wire inventory, generated docs는 integration owner 단일 writer다.
- 병렬 lane 결과는 각각 한 checkpoint commit으로 인계한다. integration owner가 두 commit을 합친 뒤 동일 clean
  integration HEAD에서 양쪽 proof를 재실행하고 `P02I` integration handoff를 만든다.
- P04→P05→P06과 P07→P08은 같은 stack의 순차 checkpoint다. predecessor schema를 downstream이 재설계하지 않는다.

## 4. Handoff authority

각 lane은 source digest에서 제외되는 `artifacts/sep-21/handoffs/<LANE>.json`을
`docs/plans/sep-21-search-plane-sota-hardening/tickets/handoffs/lane-handoff.schema.json`에 맞춰 남긴다. tracked
source 안에 result commit SHA를 자기참조로 기록하지 않는다. 최소 내용은 base/result SHA, dirty digest, exact write set,
exported API/schema, proof ID별 manifest digest, 실행 counts, NOT_RUN, blockers다. 병렬 통합은 별도로
`artifacts/sep-21/handoffs/P02I.json`을 남긴다. handoff와 proof source가 current HEAD와 다르면 downstream 시작을 금지한다.

상태는 다음 네 값만 사용한다.

- `IMPLEMENTATION_DONE`: 구현과 정적 owner audit 완료
- `OWNER_PROOF_GREEN`: 등록된 owner-local proof가 source-bound로 통과
- `RELEASE_PROOF_PENDING`: 구현/owner proof는 끝났으나 요구 Linux/external proof 미실행
- `BLOCKED`: prerequisite, ownership, contract 또는 mandatory proof 결손

mandatory proof가 NOT_RUN이면 ticket을 `done`으로 바꾸지 않는다.

## 5. Verification and evidence

- canonical command는 `Justfile`과 `./scripts/cargow`만 사용한다. bare `cargo`는 repo exception rail만 허용한다.
- 새 Rust test target/suite는 suite module과 `tools/ci/test-authority.toml`에 등록한다.
- static/focused/macOS/in-process proof를 Linux/process/external/deploy/activation proof로 승격하지 않는다.
- proof manifest는 registry의 proof ID, command/profile/target/filter, source, binary, host, counts, artifact digest에
  결속한다. `binary_binding=none`과 upstream 없는 worktree는 schema/validator가 명시적으로 표현해야 한다.
- source dirty digest는 staged+unstaged+scoped untracked content를 포함하고 proof output 자체는 source digest에서
  제외한다. final gate는 `--require-all --bind-source`와 aggregate receipt를 요구한다.

## 6. External and destructive boundaries

다음은 각각 별도 명시 승인 없이는 실행하지 않고 `NOT_RUN` 또는 `BLOCKED`로 기록한다.

- 다른 repository 수정/commit/push
- provider network egress, credential 사용, 비용 발생
- non-disposable state root의 migrate/backup/restore/cutover
- production-like host 예약 또는 shared daemon/process kill
- deploy, activation, rollback, production configuration 변경

로컬 검증은 disposable temp state root와 locally spawned process만 사용한다. 경로가 불명확한 삭제/overwrite는
금지한다.

## 7. Checkpoint and report

proof 직전과 commit 직전에 owner path와 HEAD drift를 재확인한다. explicit owner paths만 stage하고 cached diff를
검토한 뒤 lane checkpoint commit을 만든다. push는 별도 명시 요청이 있을 때만 수행한다. 최종 보고는 다음을
포함한다.

- 상태 값과 exact base/result commit
- dirty ownership과 exact write set
- RCA, 변경한 authority/logic, 제거한 legacy path
- proof ID, command/profile/host, counts, manifest/artifact path와 digest
- NOT_RUN/BLOCKED와 release/deploy/activation 간극
- downstream이 소비할 handoff path/digest
- commit SHA, upstream, push 수행 여부/결과
