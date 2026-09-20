# Copy/paste prompt — P11 Cross-Repo Terminal Receipt Cutover

당신은 S21-12 owner다. 먼저 repo root 기준 `docs/plans/sep-21-search-plane-sota-hardening/tickets/prompts/`
아래 `COMMON-EXECUTION-CONTRACT.md`와 `README.md`를 읽고 그대로 적용한다. immediate P10 handoff/result SHA가 start HEAD와
exact match하고 current tracked source의 candidate/journal/SDK terminal-receipt contract digest를 freeze한 뒤 시작한다. Semantica의
과거 SHA/dirty snapshot을 현재 증거로 사용하지 말고 시작 시 두 repo를 재-freeze한다.

읽을 문서: repo instructions, `docs/plans/sep-21-search-plane-sota-hardening/tickets/FINAL-AUDIT.md`,
`docs/plans/sep-21-search-plane-sota-hardening/tickets/S21-12-cross-repo-terminal-receipt-cutover.md`, P10 handoff,
current Semantica repo instructions/producer owner.

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

구현 요구:

- producer HEAD+dirty digest+payload digest → operation key/body digest+terminal sequence → candidate commitment →
  prior/new activation+epoch → daemon source/binary/features/toolchain binding
- old producer/new daemon과 new producer/old daemon을 mutation 전 typed incompatible로 거부
- identity-only ACK로 strong closeout 불가
- ACK loss exact replay가 rebuild/re-embed/reactivation을 반복하지 않음
- receipt retention과 replay window/floor 결속
- query-only와 mutation-capable SDK profile evidence 분리
- cutover order, deployment freeze, rollback boundary 문서화

negative matrix: same identity/different payload, same generation/different candidate, dependency-root mismatch,
wrong daemon binary, dirty source, missing/zero/reordered commitment, stale activation epoch, unsupported legacy producer.

금지: optional digest extension, dirty alignment을 GREEN으로 승격, mock/in-process binary를 deployed daemon proof로 사용,
past receipt를 new source에 재사용.

DoD: exact clean source pair와 actual built daemon binary hash로 publish-only, publish+activate, restart/replay,
positive/negative wire matrix, aggregate closeout를 실행한다. query-only profile과 mutation-capable profile을 별도
proof로 남긴다. test/disposable activation을 production `ACTIVATED`로 승격하지 않는다. external checkout/build
불가면 product shim을 만들지 말고 `BLOCKED`로 남긴다.

proof node는 독립적인 `p11-cross-repo-cutover`, `p11-deployment`, `p11-activation`, `p11-rollback` 네 개다.
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
남겨라. quanta-index explicit owner path는 checkpoint commit하고 current lane branch에 non-force push한다.
Semantica는 별도 승인이 있을 때만 commit/push한다. 두 repo의 권한과 결과를 분리 기록한다.
