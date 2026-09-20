# Copy/paste prompt — P11 Cross-Repo Terminal Receipt Cutover

당신은 S21-12 owner다. P10 migration workflow와 S21-02/04/07 receipts가 DONE인 exact quanta-index source에서
작업한다. Semantica의 과거 SHA/dirty snapshot을 현재 증거로 사용하지 말고 시작 시 두 repo를 재-freeze한다.

읽을 문서: repo instructions, `docs/plans/sep-21-search-plane-sota-hardening/tickets/FINAL-AUDIT.md`,
`docs/plans/sep-21-search-plane-sota-hardening/tickets/S21-12-cross-repo-terminal-receipt-cutover.md`, P10 handoff,
current Semantica repo instructions/producer owner.

시작 freeze:

- quanta-index and Semantica full HEAD, branch/upstream/merge-base
- tracked/untracked dirty paths and deterministic dirty digest
- resolved contract/SDK dependency roots
- toolchain/features and actual daemon binary target

목표: producer source/prepared payload부터 operation journal, sealed candidate, activation, daemon binary까지 하나의
mandatory domain-separated commitment chain과 breaking cutover를 구현한다.

quanta owner surfaces:

- contract/SDK RepoMap ingest-control APIs
- S21-04 journal receipts and S21-02 candidate/activation receipts
- cross-repo hellgate and release receipt schema

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
positive/negative wire matrix, aggregate closeout를 실행한다. external checkout/build 불가면 product shim을 만들지 말고
`BLOCKED`로 남긴다.

최종 보고에 source-pair freeze, dependency roots, receipt chain, compatibility matrix, commands/counts, NOT_RUN/BLOCKED,
P12가 소비할 artifact paths/digests를 남겨라. commit/push는 요청 시에만 한다.
