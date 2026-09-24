# Copy/paste prompt — P11 Cross-Repo Terminal Receipt Cutover

> Historical lane prompt. Recheck current source, registry, and [residual plan](../FINAL-RESIDUAL-EXECUTION-PLAN.md) before use. Do not treat this text as a current execution order or proof receipt.

당신은 S21-12 owner다. 먼저 repo root 기준 `docs/plans/sep-21-search-plane-sota-hardening/tickets/prompts/`
아래 `COMMON-EXECUTION-CONTRACT.md`와 `README.md`를 읽고 그대로 적용한다. immediate P10 handoff/result SHA가 start HEAD와
exact match하고 current tracked source의 candidate/journal/SDK terminal-receipt contract digest를 freeze한 뒤 시작한다. Semantica의
과거 SHA/dirty snapshot을 현재 증거로 사용하지 말고 시작 시 두 repo를 재-freeze한다.

읽을 문서: repo instructions, `docs/plans/sep-21-search-plane-sota-hardening/tickets/FINAL-AUDIT.md`,
`docs/plans/sep-21-search-plane-sota-hardening/tickets/S21-12-cross-repo-terminal-receipt-cutover.md`, P10 handoff,
current Semantica repo instructions/producer owner.

## REQUIRED INPUTS

- `SEMANTICA_ROOT=/Users/songmin/Documents/code-new/semantica-codegraph-v2`; 이 절대경로가 canonical repo identity
  `github:josongmin/semantica-codegraph-v2`로 resolve되는지 검증한다. 다르면 추정 탐색하지 말고 `BLOCKED`다.
- `SEMANTICA_START_SHA`: 작업 시작 시 위 checkout에서 재-freeze한 full clean SHA. 과거 값 사용 금지.
- Semantica `read`, `edit`, `commit`, `push` 승인: 네 권한을 각각 명시적으로 확인한다. clean source-pair proof에는 coherent
  Semantica commit이 필수이므로 edit만 승인되고 commit이 승인되지 않으면 closure는 `BLOCKED`다.
- Linux production-like host 사용 승인.
- deployment, activation, rollback 승인: 각각 독립 값. 누락된 동작은 실행하지 않고 해당 proof를 `NOT_RUN`으로 둔다.

값을 대화나 과거 artifact에서 추론하지 않는다. 필수 구현 입력이 없으면 product shim이나 quanta-only partial closure를
만들지 않는다.

시작 freeze:

- quanta-index and Semantica full HEAD, branch/upstream/merge-base
- tracked/untracked dirty paths and deterministic dirty digest
- resolved contract/SDK dependency roots
- toolchain/features and actual daemon binary target
- Semantica read/edit/commit/push 권한을 각각 분리 기록. 이 lane은 producer 변경이 필수이므로 edit 권한이 없으면
  cross-repo 구현 전에 `BLOCKED`; quanta-only partial checkpoint를 closure로 남기지 않는다.

목표: producer source/prepared payload부터 operation journal, sealed candidate, activation, daemon binary까지 하나의
mandatory domain-separated commitment chain과 breaking cutover를 구현한다.

quanta owner surfaces:

- contract/SDK RepoMap ingest-control APIs
- S21-04 journal receipts and S21-02 candidate/activation receipts
- cross-repo hellgate and release receipt schema
- cross-repo proof target/fixture, `tools/ci/{test-authority,proof-authority}.toml`, Just recipe

Semantica owner surfaces:

- RepoMap preparation manifest/canonical payload digest
- handoff SDK request context
- aggregate terminal closeout validator

위 surface 이름은 write permission이 아니다. 패치 전 두 repo 각각에 exact path/symbol/base blob/purpose를 가진
owner-freeze table을 만들고, root allowlist 밖 경로 또는 기존 dirty overlap이 필요하면 `BLOCKED`다.

구현 요구:

- producer HEAD+dirty digest+payload digest → operation key/body digest+terminal sequence → candidate commitment →
  prior/new activation+epoch → daemon source/binary/features/toolchain binding
- old producer/new daemon과 new producer/old daemon을 mutation 전 typed incompatible로 거부
- identity-only ACK로 strong closeout 불가
- ACK loss exact replay가 rebuild/re-embed/reactivation을 반복하지 않음
- receipt retention과 replay window/floor 결속
- query-only와 mutation-capable SDK profile evidence 분리
- cutover order, deployment freeze, rollback boundary 문서화

구현과 두 repo coherent checkpoint commit이 끝난 뒤 quanta result SHA에서 `release-daemon-fresh`를 빌드한다. 그때
생성된 absolute path/SHA-256을 `QUANTA_RELEASE_DAEMON`으로 freeze하고 cross-repo/process proof에 주입한다. 시작 HEAD의
binary나 source 변경 전 binary는 stale이므로 사용하지 않는다.

negative matrix: same identity/different payload, same generation/different candidate, dependency-root mismatch,
wrong daemon binary, dirty source, missing/zero/reordered commitment, stale activation epoch, unsupported legacy producer.

금지: optional digest extension, dirty alignment을 GREEN으로 승격, mock/in-process binary를 deployed daemon proof로 사용,
past receipt를 new source에 재사용.

DoD: exact clean source pair와 actual built daemon binary hash로 publish-only, publish+activate, restart/replay,
positive/negative wire matrix, aggregate closeout를 실행한다. query-only profile과 mutation-capable profile을 별도
proof로 남긴다. test/disposable activation을 production `ACTIVATED`로 승격하지 않는다. external checkout/build
불가면 product shim을 만들지 말고 `BLOCKED`로 남긴다.

checkpoint/proof 순서는 두 repo에 대해 원자적으로 고정한다.

1. 두 repo에서 provisional owner checks를 수행한다.
2. 각 repo explicit allowlist만 stage하고 cached diff를 검토한 뒤 coherent checkpoint commits를 만든다. 아직 push하지 않는다.
3. clean quanta result SHA에서 `release-daemon-fresh`를 build하고 path/SHA-256을 freeze한다.
4. clean source pair와 그 binary로 protocol proof를 실행하고 manifests를 발급한다.
5. provisional P11 handoff를 schema validation한다.
6. 승인된 repo branch만 non-force push하고 remote SHA를 확인한다.
7. handoff의 push fields를 실제 결과로 finalize하고 schema/semantic validation과 digest를 다시 수행한다.

proof node는 독립적인 `p11-cross-repo-cutover`, `p11-deployment`, `p11-activation`, `p11-rollback` 네 개다. expected
family는 모두 `X`이며 dependency chain은 `p10-state-migration → p11-cross-repo-cutover → p11-deployment →
p11-activation → p11-rollback`이다. transitive proof를 각 node의 direct dependency로 중복 열거하지 않는다.
첫 node의 pass로 나머지 세 verdict를 암시하지 않는다. quanta와 Semantica 양쪽 exact target/fixture가 registry에
없거나 command가 둘 중 하나를 선택하지 않으면 product shim 없이 `BLOCKED`다. protocol recipe는 producer root를
필수 인자로 받고,
publish-only/publish+activate/restart-replay/positive-negative matrix/query-only+mutation SDK profiles/aggregate closeout를
모두 실행하며 supplied release daemon path/hash를 검증해야 한다. deployment/activation/rollback recipe는 각각
독립 terminal receipt을 생성한다. 현재 `just rust-verify-hellgate-cross-repo`가 단일
positive test만 실행하면 recipe/registry를 이 lane에서 고치기 전 proof를 주장하지 않는다. Linux
production-like/release-daemon proof가 필요하다. Semantica 수정/commit/push, provider egress, deploy/activation은
각각 별도 명시 승인 없이는 수행하지 않는다. 최종 보고에 source-pair freeze, dependency roots, receipt chain,
compatibility matrix, 네 proof별 commands/counts/NOT_RUN/BLOCKED, P12 artifact paths/digests와
`artifacts/sep-21/handoffs/P11.json`을
남겨라. handoff의 `paired_repositories`에는 quanta/Semantica identity, absolute root, base/result SHA, dirty digest,
dependency-root digest, exact write set, branch/upstream, edit/commit/push approval과 실제 push 결과를 각각 기록한다.
Semantica는 별도 승인이 있을 때만 commit/push한다. 두 repo의 권한과 결과를 분리 기록한다.
