# Copy/paste prompt — P03 Candidate and Activation Authority

당신은 S21-02 owner다. 먼저 repo root 기준 `docs/plans/sep-21-search-plane-sota-hardening/tickets/prompts/`
아래 `COMMON-EXECUTION-CONTRACT.md`와 `README.md`를 읽고 그대로 적용한다. P02I integration commit,
`artifacts/sep-21/handoffs/P02I.json`, 동일 clean HEAD에서 재생성한 P02A/P02B proof가
모두 일치할 때만 시작한다. 하나라도 없으면 adapter/shim을 만들지 말고 `BLOCKED`로 종료한다.

읽을 문서: repo instructions, `docs/plans/sep-21-search-plane-sota-hardening/tickets/FINAL-AUDIT.md`,
`docs/plans/sep-21-search-plane-sota-hardening/tickets/INDEX.md`,
`docs/plans/sep-21-search-plane-sota-hardening/tickets/S21-02-sealed-candidate-activation-and-recovery.md`,
P02A/P02B handoff.

목표: SQLite catalog를 RepoMap candidate/activation visibility의 유일 authority로 만들고 publish/activate/recover를
exact candidate commitment에 결속한다.

owner files/surfaces:

- `crates/quanta-index-repomap/src/{materializer,model,persistence,store}.rs`
- catalog candidate/activation schema and transaction adapters
- `crates/quanta-index-searchd/src/app/runtime.rs`
- search-plane ingest/control RepoMap routes
- `crates/quanta-index-contract/src/repomap.rs`
- `crates/quanta-index-sdk/src/repomap.rs`

구현 순서:

1. `repomap_candidate_v1`과 `repomap_activation_v1` schema/row digest/CAS/state transitions를 구현한다.
2. `CompiledRepoMapCandidateV1`만 seal하고 raw bundle 재해석/commitment 재계산을 금지한다.
3. object store는 immutable bytes write/fsync/verify만 담당한다.
4. runtime `activations/` file reader/writer와 boot scan authority를 삭제한다.
5. catalog commit 뒤에만 memory registry를 publish한다. registry는 cache다.
6. same logical generation/same commitment는 original receipt replay, 다른 commitment는 conflict다.
7. candidate loss/corruption을 durable invalidation하고 파일 재등장으로 재활성화하지 않는다. conflict code는
   `CANDIDATE_COMMITMENT_CONFLICT`만 사용한다.
8. publish/activate/rollback ACK에 prior/new commitment, epoch, terminal sequence, replay status를 결속한다.

금지: file/catalog dual-write, open-time winner selection, publish implicit activation, repair implicit reactivation,
optional digest ACK, partial materializer input.

DoD:

- compiler refusal이면 object/catalog/registry mutation 0
- publish-only active query truth 변화 0
- fault matrix의 temp write/fsync/rename/catalog commit/registry publish 각 지점에서 restart 후 old 또는 new
  committed activation 하나만 관찰
- missing/corrupt candidate → durable invalidation → restart/repair/restart에서도 자동 활성화 0
- ACK loss exact replay가 object build/write/activation을 반복하지 않음
- runtime activation file read/write symbol과 path 0

proof node는 `p03-candidate-activation`; release proof는 registry가 요구하는 Linux production-like host와 release
daemon binding 없이는 GREEN이 아니다. canonical verification과 허용된 disposable filesystem fault/restart/SDK
consumer proof를 실행하고 count를 남겨라. 최종 보고에 공통 상태, source freeze, authority deletion evidence,
schema/state table, commands/counts, NOT_RUN, P04 입력 API와 `artifacts/sep-21/handoffs/P03.json`을 포함하라. explicit owner
path만 checkpoint commit하고 push는 별도 요청 시에만 한다.
