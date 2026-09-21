# SEP-21 Lane Execution Contract

이 문서는 P00~P12Q 실행 프롬프트의 공통 강제 계약이다. 각 lane prompt와 함께 읽고 적용한다. 이 계약과
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

`START_SHA := predecessor.result_sha`로 해석한다. 현재 checkout을 reset/switch/stash해서 맞추지 않는다. 현재 HEAD가
`START_SHA`가 아니면 그 SHA에서 새 격리 worktree/branch를 만든다. P02A/P02B는 동일 P01 result에서 fork하고 P02I만
P01 result를 integration base로 두 checkpoint를 병합한다. 동일 lane branch가 이미 존재하지만 tip/base가
다르거나 owner가 불명확하면 재사용·force 이동하지 말고 `BLOCKED`다.

`main`, `master`, remote default/protected branch에서는 source를 수정하거나 push하지 않는다. task가 이미 격리
worktree/branch를 받지 않았다면 패치 전 exact start HEAD에서 `codex/sep21-<lane>` branch를 만들고
upstream을 그 branch로만 설정한다. default branch에 direct commit/push를 요구하는 환경이면 `BLOCKED`다.

ticket 전체 closure와 intermediate checkpoint를 혼동하지 않는다. `P01A`는 S21-01 전체 closure가 아니며 `P02I`는
두 병렬 lane의 same-HEAD integration checkpoint다. S21-01/S21-02 closure는 P03 handoff와 proof가 current source에
결속된 뒤에만 선언한다.

## 3. Write and integration discipline

- 한 lane은 명시한 owner allowlist만 수정한다. 추가 owner가 필요하면 먼저 범위와 충돌을 보고하고 중단한다.
- edit 전에 owner-freeze table을 만든다. 각 row는 exact path, exact symbol/section, base blob SHA, 허용 변경 목적을
  가진다. glob, 디렉터리 전체, `all modules`, `owner` 같은 표현은 write permission이 아니다.
- `git add -A`, stash, reset, unrelated formatting/cleanup, 다른 owner의 dirty 수정은 금지한다.
- P02A/P02B만 병렬이다. 동일 P01A result SHA의 별도 worktree/branch에서 수행한다.
- P00, P01A, P02I, P03~P11, P12A, P12Q는 순차다. predecessor가 실행 중이면 다음 lane을 선행 구현하지 않는다.
- P02A/P02B는 P01 handoff/runbook이 미리 배정한 disjoint contract files/symbols만 수정할 수 있다. P02A는
  `contract/src/repomap.rs` compiler DTO section, P02B는 `contract/src/ipc/ingest.rs` operation journal/status section이다.
  P02A는 `tools/ci/proof-authority.toml`의 `p02a-repomap-compiler` row와 P02A 전용 test-authority target/recipe section만,
  P02B는 `p02b-operation-journal` row와 P02B 전용 section만 소유한다. 서로의 row, 공통 profile, CI workflow는 수정하지
  않는다. public re-export, SDK facade, P02A/P02B가 발생시킨 public API baseline/wire inventory/generated-doc delta와
  공통 profile delta는 P02I integration owner 단일 writer다. P01A 자체 변경으로 발생한 baseline/inventory는 P01A가
  같은 checkpoint에서 소유한다.
- 병렬 lane 결과는 각각 한 checkpoint commit으로 인계한다. integration owner가 두 commit을 합친 뒤 동일 clean
  integration HEAD에서 양쪽 proof를 재실행하고 `P02I` integration handoff를 만든다.
- P04→P05→P06과 P07→P08은 같은 stack의 순차 checkpoint다. predecessor schema를 downstream이 재설계하지 않는다.

## 4. Handoff authority

각 lane은 source digest에서 제외되는 `artifacts/sep-21/handoffs/<LANE>.json`을
`docs/plans/sep-21-search-plane-sota-hardening/tickets/handoffs/lane-handoff.schema.json`에 맞춰 남긴다. tracked
source 안에 result commit SHA를 자기참조로 기록하지 않는다. 최소 내용은 base/result SHA, dirty digest, exact write set,
exported API/schema, proof ID별 manifest digest, 실행 counts, NOT_RUN, blockers다. 병렬 통합은 별도로
`artifacts/sep-21/handoffs/P02I.json`을 남긴다. immediate predecessor handoff의 result SHA는 lane start HEAD와 exact
match해야 한다. 이전 ancestor의 commit ancestry는 유지되어야 하지만 매 lane이 과거 handoff/proof artifact를 다시
검증하지 않는다. downstream이 필요한 transitive contract는 current tracked path/schema digest로 소비한다. P12Q만
complete transitive handoff/proof DAG를 final HEAD/source pair에 다시 결속한다. release-only external proof가
`NOT_RUN`인 handoff는 다음 구현 lane의 contract consumption을 자동 차단하지 않지만, registered owner-local proof가
요구된 lane에서 그 proof까지 없으면 차단한다. release closure와 `PRODUCTION_READY`에는 모든 mandatory external proof가
필수다.

execution label과 handoff lane ID의 canonical mapping은 `P01A → P01`, `P12Q → P12`다. 나머지는 execution label과
handoff ID가 같다. 기존 artifact compatibility를 위해 P01A/P12Q를 schema lane 값으로 새로 쓰지 않는다.

상태는 다음 네 값만 사용한다.

- `IMPLEMENTATION_DONE`: 구현과 정적 owner audit 완료
- `OWNER_PROOF_GREEN`: 등록된 owner-local proof가 source-bound로 통과
- `RELEASE_PROOF_PENDING`: 구현/owner proof는 끝났으나 요구 Linux/external proof 미실행
- `BLOCKED`: prerequisite, ownership, contract 또는 mandatory proof 결손

mandatory proof가 NOT_RUN이면 ticket을 `done`으로 바꾸지 않는다.

Linux/external release proof가 뒤 lane 구현을 영구 차단하지 않도록 P03~P10은 proof를 둘로 나눈다.

- `<release-proof-id>-owner`: `required_host=any`, immediate predecessor의 owner node에만 의존하며 다음 구현 lane을 연다.
- 기존 `<release-proof-id>`: 해당 owner node와 immediate predecessor release node에 의존하며 P12Q release closure를 연다.

owner node를 Linux/process/external/release evidence로 승격하지 않는다. release node `NOT_RUN`이면 handoff는
`RELEASE_PROOF_PENDING`이고 다음 lane은 owner node와 current tracked contract를 소비해 진행할 수 있다. P11/P12Q의
exact-pair/deploy/activate/rollback proof는 별도 승인 경계이므로 owner-node 대체가 없다.

## 5. Verification and evidence

- canonical command는 `Justfile`과 `./scripts/cargow`만 사용한다. bare `cargo`는 repo exception rail만 허용한다.
- 새 Rust test target/suite는 suite module과 `tools/ci/test-authority.toml`에 등록한다.
- static/focused/macOS/in-process proof를 Linux/process/external/deploy/activation proof로 승격하지 않는다.
- proof manifest는 registry의 proof ID, command/profile/target/filter, source, binary, host, counts, artifact digest에
  결속한다. `binary_binding=none`과 upstream 없는 worktree는 schema/validator가 명시적으로 표현해야 한다.
- source-bound receipt의 archive key는 domain/version이 고정된 canonical JSON
  `{"domain":"quanta-proof-source-binding-v1","source":<source>,"source_pair":<source_pair-or-null>}`의 SHA-256인
  `source-binding-digest`다. primary source만 hash하거나 paired source를 생략하지 않는다. canonical archive 경로는
  `artifacts/proof-authority/archive/<proof-id>/<source-binding-digest>/<manifest-digest>.json`이며
  `manifest-digest`는 최종 manifest canonical bytes의 SHA-256이다. content-addressed exact-byte idempotent 재발행만
  허용하고, 같은 경로의 다른 bytes/overwrite/delete는 거부한다.
- handoff와 manifest의 `dependency_receipts`는 current alias가 아니라 dependency의 exact archive path와 manifest digest를
  기록한다. historical 검증은 archive path/digest와 transitive dependency archive DAG만 검증하며 registry current alias
  equality를 요구하거나 허용하지 않는다. `artifacts/proof-authority/<proof-id>.json` current alias는 편의용이며
  historical handoff authority가 아니다. 같은 source binding의 retry도 새 manifest digest leaf를 추가할 수 있지만 과거
  archive leaf와 dependency edge를 변경하지 않는다.
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

각 구현 lane은 아래 순서를 바꾸지 않는다.

1. dirty tree에서 provisional owner checks를 실행한다. 이것은 terminal source-bound receipt가 아니다.
2. owner path와 HEAD drift를 재확인하고 explicit allowlist만 stage한다. cached diff를 검토한다.
3. checkpoint commit을 만든다.
4. source-affecting dirty가 없는 committed result HEAD를 다시 freeze한다.
5. registered owner-local terminal proof를 result HEAD에서 재실행하고 manifest를 `--bind-source`로 검증한다.
   release-only proof는 승인/host 부재 시 `NOT_RUN`으로 남긴다.
6. result SHA와 manifest digest를 담은 provisional handoff JSON을 만들고 schema를 검증한다. push field가 required인
   paired-repo handoff는 이 시점에 `attempted=false,result=NOT_RUN`이다.
7. current lane upstream으로 최초 non-force push하고 remote ref가 result SHA인지 확인한다.
8. push field가 있는 handoff를 실제 결과로 finalize하고 다시 schema validation/digest를 수행한다. handoff는 source
   digest 제외 artifact이므로 source-bound proof는 변하지 않는다. push 결과가 없는 일반 lane은 최종 보고에 기록한다.

terminal proof 실패 후 source를 고치면 새 checkpoint commit을 추가하고 source-bound proof/manifest/handoff를 전부
재발급한다. stale receipt를 재사용하거나 이미 발급한 handoff의 SHA를 손으로 바꾸지 않는다. 이전 result SHA의
immutable archive receipt는 보존하고 새 result SHA용 archive만 추가한다. terminal proof가 green이
아니면 downstream용 handoff를 발급하거나 push하지 않는다. 여기서 green은 현재 lane의 required owner-local proof를
뜻하며 release-only `NOT_RUN`은 `RELEASE_PROOF_PENDING`으로 분리한다. remote drift/rejection은 rebase/force-push로 숨기지 말고
`BLOCKED`로 보고한다. P12Q qualification-only task에 tracked 변경이 없으면 empty commit은 만들지 않는다. 최종 보고는
다음을 포함한다.

- 상태 값과 exact base/result commit
- dirty ownership과 exact write set
- RCA, 변경한 authority/logic, 제거한 legacy path
- proof ID, command/profile/host, counts, manifest/artifact path와 digest
- NOT_RUN/BLOCKED와 release/deploy/activation 간극
- downstream이 소비할 handoff path/digest
- commit SHA, upstream, push 수행 여부/결과
