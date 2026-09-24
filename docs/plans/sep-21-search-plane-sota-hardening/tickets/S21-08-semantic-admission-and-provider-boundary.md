# S21-08 — Semantic Admission and Provider Boundary

Status: historical design record; current implementation and proof state must be read from source and `tools/ci/proof-authority.toml`.

Depends on: S21-00, S21-04, S21-06, S21-07

## Goal

provider 호출 전 결정 가능한 validation을 모두 수행하고, external egress/work/cost를 process-global로
bounded하고 auditable하게 만든다.

## Initial audit root cause

- retained generation model mismatch를 embedding 호출 뒤 검사
- hash/provider profile이 empty/punctuation input에 다른 semantics를 가짐
- raw source/query text egress policy가 release authority에 없음
- cancellation 이후 detached OS thread와 HTTP request가 request-local cap 밖에서 지속
- declared model과 provider-observed model/version/usage가 분리되지 않음

## Admission pipeline

1. common query normalization and shape validation
2. provider/tenant egress policy check
3. target generation model/profile compatibility check
4. global provider budget reservation
5. cancellable provider task execution
6. response model/dimension/finite/norm/usage validation
7. reservation reconciliation and redacted audit event

Provider I/O 전에 1-4가 모두 성공해야 한다.

## Original work items (recheck current source)

- common semantic input policy를 core owner에 배치
- known model mismatch를 lexical/dense/provider fanout 전에 거부
- provider task를 supervisor-owned pool로 이동; per-request detached thread 금지
- global concurrent requests/threads/FD/inflight bytes/retry/cost budget
- cancellation과 hard shutdown에서 task ownership/settlement 정의
- egress allowlist, source classification, tenant/profile consent, redaction, region/retention config
- payload를 기록하지 않는 request audit identity
- declared and observed model identity plus usage/cost receipt
- retry는 idempotency와 provider billing semantics를 분리

## Owner files

- `crates/quanta-index-core/src/domains/semantic/`
- `crates/quanta-index-search-plane/src/query_embedder.rs`
- semantic/hybrid routes
- `crates/quanta-index-embed/src/openai.rs`
- `crates/quanta-index-searchd/src/app/runtime.rs`
- configuration and secret/provider profile owners
- `crates/quanta-index-search-plane/src/semantic_derive.rs` source-content egress path

## Negative scenarios

- whitespace/punctuation/unsupported Unicode tokenless input
- old generation vs rotated model
- policy-denied source/query classification
- cancel immediately/after send/during response/retry
- provider reports unexpected model/dimension/NaN
- repeated cancelled requests at global cap
- shutdown with inflight provider requests

## Acceptance

- locally decidable refusal causes provider calls exactly 0
- hash/stub/OpenAI profiles share public input/error semantics
- cancelled/timeout work remains under global configured cap and is supervisor-visible
- no raw source/query/credential appears in log, metric, cache key, receipt
- every external call has model/usage/cost/outcome audit identity
- egress-disabled profile fails closed

## Verification

- deterministic provider spy counting calls and residual tasks
- controlled HTTP stub for cancellation/retry/model mismatch
- child-process FD/thread/request cap probe
- opt-in real-provider X/Q rail with fixed budget and redacted receipt
- secret/log/artifact scanners

## No patch-on-patch rule

route 앞에 빈 문자열 check만 추가하거나 detached thread에 semaphore만 감싸지 않는다. admission policy,
global task ownership, cancellation settlement, egress proof를 한 provider boundary로 만든다.

## Final ownership split and action list

- S21-08은 admission/interface를 소유하고 S21-09는 provider task lifecycle/join을 소유한다. 두 ticket은
  같은 M3에서 원자적으로 닫되 서로의 구현을 중복하지 않는다.
- `crates/quanta-index-core/src/domains/semantic/`: canonical input policy, model/profile compatibility,
  `EmbeddingOutcomeV1`, egress decision과 reservation port.
- semantic/hybrid route: lexical 또는 provider fanout 전에 local admission을 끝내고 refusal이면 호출 0.
- `crates/quanta-index-search-plane/src/query_embedder.rs`: reservation token을 받아 cancellable executor에 제출,
  observed model/dimension/usage/cost와 함께 정산.
- `crates/quanta-index-search-plane/src/semantic_derive.rs:262`와 core semantic outbound path: source content도
  query text와 동일한 classification/consent/redaction boundary를 통과.
- `crates/quanta-index-embed/src/openai.rs`: payload construction, timeout/retry, response model/finite/norm 검증,
  secret-safe error mapping. detached thread 생성 금지.
- config: global requests, worker threads, inflight bytes, retry, token/cost, region/retention cap과 zero/overflow validation.
- telemetry: raw payload/credential 없이 request audit identity, policy decision, provider-observed model, usage/cost/outcome.

### DoD additions

- query와 source derive 각각 policy/model/input refusal에서 provider spy call count와 reserved budget이 0이다.
- cancellation/timeout/retry 뒤 global reservations, live task, FD/thread count가 bounded baseline으로 복귀한다.
- declared model과 provider-observed model 불일치는 success/cache/receipt를 생성하지 않는다.
- log/metric/error/receipt/cache-key artifact scanner가 raw source/query/credential 유출 0을 증명한다.
