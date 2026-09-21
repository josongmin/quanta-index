# Copy/paste prompt — P04 QueryReadView V2

당신은 S21-05 owner다. 먼저 repo root 기준 `docs/plans/sep-21-search-plane-sota-hardening/tickets/prompts/`
아래 `COMMON-EXECUTION-CONTRACT.md`와 `README.md`를 읽고 그대로 적용한다. P03가 S21-01B/S21-02를 닫은
checkpoint/handoff와 catalog candidate/activation authority API가 current stack에 있을 때만 시작한다. pinned RepoMap
acquire API의 분리/구현은 이 lane의 산출물이지 선행조건이 아니다. 이 lane은 M2 stacked checkpoint A이며 S21-05를
단독 `done`으로 닫지 않는다. P03 checkpoint/handoff/proof binding이 없거나 current source와 다르면
`BLOCKED`로 종료한다.

읽을 문서: repo instructions, `docs/plans/sep-21-search-plane-sota-hardening/tickets/FINAL-AUDIT.md`,
`docs/plans/sep-21-search-plane-sota-hardening/tickets/S21-05-read-view-v2-and-snapshot-lifetime.md`, P03 handoff.
S21-02/S21-04 의미는 과거 handoff를 재검증하지 않고 current tracked schema/API digest로 소비한다.

목표: 모든 query route가 선언한 실제 immutable handle을 한 번 acquire하고 요청 종료까지 보유하게 하며 ambient
store/ledger latest read와 activation/GC TOCTOU를 제거한다.

owner files/symbols:

- `crates/quanta-index-core/src/domains/repomap/inbound.rs::RepoMapQueryPort`
- `crates/quanta-index-core/src/domains/read_view/{identity,domain,errors}.rs`
- `crates/quanta-index-search-plane/src/query_dispatcher/read_view/{view,snapshots}.rs`
- `crates/quanta-index-search-plane/src/query_dispatcher/{dispatcher,routes/repo_map}.rs`
- `crates/quanta-index-repomap/src/{store,reader}.rs`
- lexical/semantic/auxiliary snapshot registries and retention owners
- exact owner tests/suite modules, ambient-lookup structural guard, `tools/ci/{test-authority,proof-authority}.toml`,
  dedicated Just recipe

구현 요구:

- core port를 snapshot acquisition과 pinned execution으로 분리하고 adapter concrete type을 core에 노출하지 않는다.
- `QueryReadViewV2`에 RepoMap handle과 domain별 `DomainReadEvidenceV2`를 추가한다.
- active identity+candidate commitment+activation epoch+`Arc`를 하나의 critical section에서 acquire한다.
- RepoMap route의 ambient `self.repo_map_query.query`를 제거하고 `view.repo_map()?.query`만 허용한다.
- response/cursor provenance는 실제 handle identity/aux epoch를 운반한다.
- GC/retire/compaction은 pin과 attach fence를 존중한다.
- cancel/panic path는 RAII로 reference를 반환한다.
- lock acquisition order와 deadlock rule을 문서/검증한다.

정확한 invariant: declared domain마다 evidence exactly one이다. physical handle count는 domain count와 같을 필요가
없으며 shared resource group 관계를 evidence가 명시한다.

금지: store 내부 lock 범위만 늘리는 국소 수정, view acquisition 후 ambient registry/ledger lookup, logical pin 아래
physical artifact 교체, debug-string refusal.

DoD:

- barrier: old view acquire 후 activate/retire/GC에도 동일 commitment로 완료
- retire가 먼저 이기면 새 old-generation acquire는 typed refusal
- cancel/panic 뒤 pin/reference count baseline 및 GC progress
- shared physical handle fixture의 evidence cardinality
- 모든 route에서 undeclared accessor refusal과 ambient lookup 0을 정적으로 검증
- production `QueryReadViewV1`/`ReadIdentityV1` live path 0; adapter concrete type의 core import 0
- catalog lock을 잡은 채 disk open/query 0; lock order는 하나의 canonical owner에 고정

owner node expected tuple은 `id=p04-read-view-lifetime-owner`, `family=F`, `required_host=any`,
`dependencies=[p03-candidate-activation-owner]`다. release node는 `id=p04-read-view-lifetime`, `family=F`,
`required_host=linux-production-like`, `dependencies=[p04-read-view-lifetime-owner,p03-candidate-activation]`다.
proof selector는 core read-view owner, activation/retire/GC barrier, panic/cancel reconciliation, shared physical handle
cardinality, ambient lookup/V1-live-path structural guard를 모두 포함한다. `just rust-profile test-daemon` 하나는 subordinate
rail일 뿐이다. dedicated `just rust-proof-p04-read-view`가 owner-local selector와 Linux release subrail을 구분해 실행하며
Linux production-like/release-daemon proof가 없으면 `RELEASE_PROOF_PENDING`이다. 최종 보고에 source/dirty freeze, 변경
route/handles, lock order, proof counts, NOT_RUN, P05가 소비할 internal read identity와
`artifacts/sep-21/handoffs/P04.json`을 남겨라. public cursor schema는 P05 owner다. explicit owner path만 checkpoint commit하고
current lane branch에 non-force push한다.
