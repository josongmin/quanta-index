# S21-07 — SDK and Wire Response Binding

Status: historical design record; current implementation and proof state must be read from source and `tools/ci/proof-authority.toml`.

Depends on: S21-00, S21-01, S21-02, S21-04, S21-06

## Goal

모든 public SDK query/mutation이 response variant뿐 아니라 원 요청과 semantic authority를 검증하도록
shared validator layer를 도입한다.

## Initial audit root cause

generic client는 request ID와 error envelope만 검증하고 route별 SDK는 대부분 enum variant만 match한다.
contract decoder도 candidate/projection/pin/order/receipt invariant를 충분히 검증하지 않는다.

## Target design

- request dispatch가 plane별 closed enum `ExpectedQueryResponseV1`, `ExpectedControlResponseV1`,
  `ExpectedIngestResponseV1`을 payload move 전에 생성·보존
- route별 검증은 closed enum exhaustive match를 사용하며 dynamic registry 누락 가능성을 만들지 않음
- decoder shape validation과 SDK request binding을 분리
- candidate/projection/manifest/sequence/forbidden-field invariant를 typed error로 반환
- cluster membership의 기존 exact binding pattern을 공통화

## Query validation

- pinned selector는 response full pin/read identity와 exact match
- active selector는 response의 resolution proof, activation epoch, read identity를 검증; unresolved selector와 resolved pin을 단순 equality하지 않음
- cursor/window identity equals canonical request
- every candidate belongs to declared repo/revision/generation/domain
- paired owner/file projection has exact candidate/order cardinality
- manifest/content/provenance digest is present and valid
- result order and count satisfy route contract

## Mutation validation

- response operation key/body digest/target identity equals request
- durable sequence positive이고 receipt 내부 일관성을 만족; global monotonic/uniqueness authority는 S21-04 catalog가 소유
- applied/replayed/terminal status fields are internally consistent
- RepoMap activation ACK includes exact prior/new candidate commitment
- same variant with wrong identity is rejected

## Owner files

- `crates/quanta-index-sdk/src/client.rs`
- SDK lexical/semantic/search/history/runtime/structural/repomap modules
- contract query response and receipt decoders
- IPC envelope tests and public API baselines

## Negative matrix

- correct request ID and variant, wrong repo/revision/generation
- stale response from previous request with reused logical ID
- swapped candidate projections
- wrong count/order and duplicate typed identity
- zero/empty manifest digest
- wrong operation body digest or durable sequence
- successful variant carrying forbidden/contradictory fields

## Acceptance

- every public SDK method declares one semantic response validator
- no public method returns a same-variant response without request binding
- decoder rejects intrinsic malformed shape; SDK rejects context mismatch
- validation code is shared, not copied route-by-route
- error exposes route and typed mismatch axis without payload leakage

## Verification

- contract decoder unit/property tests
- SDK mock transport wrong-but-same-variant matrix
- real UDS SDK frontdoor for positive consumer proof
- `just rust-public-api`, `just rust-fuzz-smoke`
- producer SDK compatibility runs in S21-12

## No patch-on-patch rule

각 method에 `if response.repo_id != request.repo_id`를 반복 추가하지 않는다. expected context와
validator registry를 공통 client architecture로 만든다.

## Final file/symbol plan

- `crates/quanta-index-sdk/src/client.rs::{dispatch_query,dispatch_control,dispatch_ingest}`: payload move 전에
  expected enum 생성, request ID 확인 뒤 intrinsic decoder → contextual binding 순으로 실행한다.
- `crates/quanta-index-contract/src/results/query_responses.rs`: success variant의 intrinsic schema/key/order/cardinality validator.
- `crates/quanta-index-contract/src/repomap.rs`: candidate commitment, activation epoch, prior/new identity의 mandatory validator.
- cursor intrinsic validator: version/integrity/typed key 형상만 contract가 검증하고 original request 결속은 SDK가 검증한다.
- SDK error: route, expected/actual mismatch axis, stable code를 typed하게 노출하고 source/query payload는 숨긴다.
- `crates/quanta-index-sdk/src/config.rs:81-123`: query-only client profile이 control/ingest endpoint를 요구하지 않게 분리한다.

### DoD additions

- public SDK method 전수가 closed expected enum의 한 variant에 매핑되고 wildcard success branch가 없다.
- same response variant지만 repo/revision/generation/commitment/order가 다른 mock wire fixture를 모두 거부한다.
- active selector는 resolution proof 누락·stale epoch·다른 activation identity를 거부한다.
- query-only SDK 구성은 control/ingest socket 부재에서도 query contract만으로 동작한다.

## 2026-09-23 active-selector RCA and decision gate

`crates/quanta-index-sdk/src/binding.rs::check_pin` currently knows only the
repo/revision from an `Active` request and accepts any response generation in
that domain. The request contains no expected activation epoch or candidate
commitment. Therefore adding an epoch to the response and checking its shape
cannot let the SDK independently reject a wrong but same-domain generation:
the response would be its own only authority. Do not call such a self-check
the negative oracle above.

The correctness-first contract is a server-authoritative active-resolution
operation returning an exact pin, activation epoch and candidate commitment,
followed by a pinned query whose response is bound to that frozen resolution.
This costs one additional IPC round trip for an uncached active read. An
implementation must keep `QuantaIndex::connect_query_only` functional:
`GenerationNamespace::current` uses the control plane and is unavailable to
query-only clients. The resolution opcode therefore belongs on the query
plane (or an explicitly revised query-only profile), not an SDK-only call to
the existing control operation. Centralize request rewriting and response
binding in `client.rs::dispatch_query` / `binding.rs`, covering every active
query variant and the legal `rev:at.time` rebind; do not patch routes one by
one. Freeze the resolution identity for the second request and reject a
server response with a different pin/epoch/commitment. Add a query-only
transport fixture and a wrong-same-domain-generation negative oracle. An
alternative single-call contract must explicitly weaken the acceptance claim
to request-ID/domain/self-consistency and must not claim detection of a
producer that resolved the wrong same-domain generation. The user decision
on latency versus independent validation is pending; do not add response-only
`resolution_proof` fields as a cosmetic substitute.

### Remaining structural closeout after query-plane resolution checkpoint

The query-plane `ResolveActiveGeneration` → pinned-query path binds a returned
generation number, but it is not yet the complete target contract:

The SDK now resolves `GenerationSelector::Active` through the query plane and
submits `ResolvedActive` with the catalog activation token beside the explicit
pin. The server rechecks that token at dispatch while the SDK exact-binds the
response. Semantic reads also carry the catalog manifest digest into the read
view. These are dirty-source implementation observations, not a P06 receipt.

1. `crates/quanta-index-search-plane/src/readiness/{activation_catalog,search_corpus_generation}.rs`
   must own a durable activation epoch/commitment, including rollback and
   reopen semantics. A manifest generation is not an activation epoch: the
   same generation may be reactivated. The query resolution response must
   derive this identity from the catalog, not from the query response itself.
2. `crates/quanta-index-contract/src/ipc/{split,control}.rs` and the query
   request DTOs must carry the resolved authority identity into the pinned
   request. `query_dispatcher/{dispatcher,selection,read_view}*` must check
   it against the acquired view, so a newer/different activation cannot
   satisfy an older resolution merely by repeating its generation number.
   Add producer/consumer negatives for same generation but wrong epoch and
   wrong content commitment.
3. `crates/quanta-index-search-plane/src/query_dispatcher/{planning,rev_at_time,dispatcher}.rs`
   and `crates/quanta-index-sdk/src/{client,binding}.rs` now use a lexical
   planner preflight that selects the ancestor before the actual query; the
   SDK binds the final response to that result. Focused production-planner
   tests cover the legal ancestor, invalid timeref and before-history empty
   case; SDK mock negatives reject a wrong same-repository final pin and a
   foreign-repository preflight result. Re-run these at the frozen final
   source as part of P06 owner/release proof.
   Explain has its own server-side candidate-pin equality guard and must
   keep exact SDK response binding. Non-lexical routes cannot rebind and
   retain exact pin matching even if their text contains the token.
4. Hybrid/semantic-with-lexical-scope requests should resolve the composite
   lexical+semantic active root atomically from one catalog read rather than
   two independently timed lookups. A concurrent activation/rollback test
   must show either one coherent pair or a typed retry/failure, never a
   mixed successful response.
5. Re-run the closed SDK route inventory, real UDS query-only positive and
   same-variant negative matrix, public API/wire/fuzz rails, P06 owner proof,
   then Linux release proof at one frozen source. Until then the P06 status is
   open even if focused query tests pass.

### Composite active selection checkpoint (2026-09-23)

The server-side hybrid, hybrid-seed and semantic-with-lexical-scope paths now
share one `ActivationCatalog::active_search_corpus_v1` read when both selectors
are `Active`. They reject a stale explicit lexical pin or divergent active
repo/revision before opening a lane. The owner test checks the selected pin
and manifest digest across activation advancement and exercises both route
selection functions. This narrows item 4 but does not close its concurrent
activation/rollback oracle or the request-level epoch/commitment design in
items 1–2. The SDK still performs independent lexical and semantic preflight
resolution calls, so the full end-to-end composite-resolution contract remains
open.

### Dirty-source implementation checkpoint (2026-09-24; not a closure receipt)

- `ActivationCatalog` now persists one positive per-pair activation sequence and
  one root incarnation, rotates the incarnation on controlled offline restore,
  and returns generation plus typed token from one catalog read. The token is
  transport data, not a second catalog or an SDK epoch cache.
- Query-plane active resolution now returns the full composite head and token.
  The SDK rewrites an `Active` selector into `ResolvedActive` for the second
  request; lexical, semantic and joint selectors compare that token against
  the catalog. An A→B→A rollback regression rejects the original token even
  when the generation number and content identity repeat. Query-only UDS
  binding remains on the query transport.
- Activation/rollback CAS requests and ACKs now carry the typed
  `SearchCorpusActiveHeadV1`; the catalog compares the whole generation and
  token and mints the next sequence. The SDK binds target, prior head, and
  sequence advance. The producer freezes the optional head through one
  catalog-owned control read. That read is `Observe` because query active
  resolution already exposes the same token; CAS mutations remain `Admin`.
  No separate producer token authority was added.
- The Semantica generation-bound query view now obtains its captured identity
  from that same active-head API; it still submits immutable generation-pinned
  queries. The previous `GenerationStatusReport` identity assembler was
  removed. A catalog read error or uncertain durability remains an error,
  never an absent head.
- `control_dispatcher::generation_status` now projects track rows and semantic
  roots from one `active_search_corpus_with_token_v1` catalog read. This removes
  its former mixed-head read, but the admin report remains a derived view and
  must not become the producer's CAS authority. These cross-repository changes
  have not received a frozen-source owner, UDS, Linux release, or exact-pair
  proof; P06 remains open.

At Quanta `563da185` with dirty P06/P09 changes, `./scripts/cargow check
--workspace --all-targets --message-format short` passed. Focused executed
tests passed for stale A→B→A activation/rollback CAS (1), control active-head
observation (1), capability matrix (3), SDK ACK roots/token binding (2), SDK
active-head failure semantics (1), and real child-process restart/rollback
(2, including cross-repository rollback). These are dirty-source local checks,
not a P06 owner manifest, frozen
cross-repository proof, or Linux release receipt. The old-type runtime test
fixtures were migrated to typed heads; the full runtime suite remains unrun.

The same dirty-source snapshot passed `just proof-p06-sdk-binding-owner`
(exit 0): SDK owner profile 13/13, SDK library profile 650/650,
hexagonal ownership, wire inventory, public API drift, and four 60-second
fuzz smoke targets. The owner run first exposed missing
`SearchCorpusActiveHead` request / `SearchCorpusActiveHeadObservation` response
inventory rows and the intentional P06/P11 contract + SDK public API delta.
Those rows and the two package baselines were synchronized, then the complete
recipe was rerun to exit 0. This is local dirty-source owner execution only:
no registered owner manifest, Linux release receipt, frozen exact-pair
Semantica consumer proof, or deployment/activation proof exists from it.
