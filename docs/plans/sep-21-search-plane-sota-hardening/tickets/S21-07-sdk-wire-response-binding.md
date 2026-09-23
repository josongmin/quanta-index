# S21-07 — SDK and Wire Response Binding

Status: `planned`

Depends on: S21-00, S21-01, S21-02, S21-04, S21-06

## Goal

모든 public SDK query/mutation이 response variant뿐 아니라 원 요청과 semantic authority를 검증하도록
shared validator layer를 도입한다.

## Root cause

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
alternative single-call contract must explicitly weaken the acceptance claim
to request-ID/domain/self-consistency and must not claim detection of a
producer that resolved the wrong same-domain generation. The user decision
on latency versus independent validation is pending; do not add response-only
`resolution_proof` fields as a cosmetic substitute.
