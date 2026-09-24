# S21-10 — Control Authorization, Readiness, and Observability

Status: historical design record; current implementation and proof state must be read from source and `tools/ci/proof-authority.toml`.

Depends on: S21-00, S21-04, S21-09

## Goal

control socket의 principal capability를 operation 단위로 강제하고, surviving socket이 partial outage를
healthy로 보이지 않도록 process-wide readiness와 request-correlated diagnostics를 만든다.

## Initial audit root cause

- 하나의 control surface에 read-only metrics/status와 activate/rollback/discard mutation이 혼재
- socket-level UID/GID admission 뒤 operation authorization이 없음
- request ID가 dispatcher context로 전달되지 않음
- readiness가 socket connect 가능성과 required plane/backend health를 구분하지 않음
- `engines_touched`를 실제 fanout metric으로 사용해 zero-hit 실행을 누락

## Capability model

- `Observe`: health, readiness, metrics, status
- `Operate`: bounded maintenance and diagnostics
- `MutateGeneration`: activate, rollback, discard, quarantine actions
- `Admin`: migration/repair/credential-sensitive operations

principal mapping은 configured UID/GID/service identity에 명시적으로 결속한다. default deny다.

## Readiness model

- process supervisor state
- query/control/ingest plane alive and accepting
- maintenance heartbeat freshness
- required backend open proof and active candidate integrity
- provider readiness는 profile claim에 따라 required/degraded/disabled로 구분
- partial plane loss는 global ready=false

## Observability model

- request correlation ID를 transport -> dispatcher -> backend/provider -> terminal response까지 전달
- typed outcome, route, repo/revision/generation, read identity, latency, queue, cancellation, close reason
- `engines_executed`와 `lanes_contributed` 분리
- cardinality-bounded labels; source/query payload와 credential은 기록하지 않음
- panic/child-exit/startup rollback/hard-drain escalation counters

## Original work items (recheck current source)

1. control operation capability registry
2. credential-to-principal resolver와 negative authorization
3. read-only and mutation surfaces를 protocol 또는 socket으로 분리할지 S21-00 결정 적용
4. process readiness response schema
5. request context propagation
6. supervisor events와 metrics/status snapshot 연결
7. engine execution/contribution metrics 교정
8. operator diagnostics/runbook contract

## Owner files

- `crates/quanta-index-contract/src/ipc/split.rs`
- `crates/quanta-index-search-plane/src/control_dispatcher.rs`
- `crates/quanta-index-ipc/src/server.rs`
- `crates/quanta-index-searchd/src/app/{runtime,socket_access}.rs`
- metrics/harness/searchctl surfaces

## Negative scenarios

- Observe principal의 activate/rollback/discard 요청
- unknown/mismatched peer credential
- query plane killed while control plane survives
- maintenance heartbeat stale
- backend active artifact quarantined
- zero-hit semantic/hybrid execution
- high-cardinality input and payload/secret leakage scan

## Acceptance

- every control opcode maps to exactly one capability
- unauthorized operation causes mutation 0 and typed audit event
- any required child/backend loss makes all-plane readiness false
- request ID로 queue/dispatch/backend/response/close outcome을 상관 가능
- executed fanout metric equals actual backend invocation even with zero hits
- logs/metrics stay within label and secrecy budget

## Verification

- socket peer credential and capability matrix integration
- real child kill-one-plane readiness test
- metrics scrape and request correlation E2E
- control public API/wire/fuzz gates
- operator negative runbook probe

## No patch-on-patch rule

mutation opcode 몇 개에 ad-hoc UID check를 추가하지 않는다. capability registry, principal context,
readiness and audit event를 공통 control boundary에서 소유한다.

## Final transport and authorization boundary

- IPC admission의 kernel peer credentials를 `Accepted(PeerCredentials)`로 보존한다. 현재처럼 screening 뒤
  버리면 dispatcher가 operation capability를 판정할 수 없다.
- `DispatchContextV1 { request_id, plane, principal, connection_id, deadline, cancellation }`을 transport가
  생성해 dispatcher에 전달한다. payload에 self-asserted principal을 추가하지 않는다.
- control opcode → required capability는 exhaustive total match이며 dispatcher `match` 전에 default-deny로
  실행한다. operation status 조회도 해당 operation principal/관리 capability를 검증한다.

### File-level action list

| Owner | Change | Purpose / DoD |
|---|---|---|
| `crates/quanta-index-ipc/src/socket_access.rs` | admitted credentials와 resolved principal 반환 | credential discard 0 |
| `crates/quanta-index-ipc/src/server.rs::IpcDispatcher` | `DispatchContextV1` 전달 | request/peer/cancel end-to-end 결속 |
| `crates/quanta-index-search-plane/src/control_dispatcher.rs` | total capability map과 pre-dispatch authorize | unauthorized mutation 0 |
| `crates/quanta-index-searchd/src/app/socket_access.rs` 및 config | UID/GID/service identity → principal/capability 명시 | unknown/default deny |
| `crates/quanta-index-searchd/src/app/runtime.rs` | `ProcessReadinessV1`을 supervisor/plane/backend integrity로 합성 | generation status와 구분 |
| searchctl/SDK contract | readiness와 generation status를 별도 command/DTO로 노출 | surviving socket을 healthy로 오판하지 않음 |

### Observability constraints

- request correlation은 bounded diagnostic ring/sink와 trace context를 사용하고 request/repo/generation을
  unbounded metric label로 쓰지 않는다.
- `engines_executed`는 backend invocation 시 증가하고 `lanes_contributed`는 post-filter contribution 시
  증가한다. zero-hit execution도 전자에 포함한다.
- readiness negative proof는 query/control/ingest/maintenance/backend 각각 하나를 죽여 global ready=false를
  확인한다.

### Current-source proof boundary

- `RuntimeReadiness` now keys its physical-proof cache on the catalog's
  generation **and activation token**, plus scrub invalidation epoch. The
  catalog inventory derives both from one locked snapshot. A -> B -> A cannot
  reuse the first A's physical proof merely because its generation repeats.
- The SearchCorpus active-head observation is `Observe`: query-plane active
  resolution already exposes the same token. Activation and rollback remain
  `Admin`; the capability matrix and dispatcher tests cover both boundaries.
- The owner-local `ProvenActive::valid_for` regression rejects a changed
  activation token or scrub epoch for the same generation. A live-daemon
  A -> B -> A physical re-probe counterexample remains a separate proof gap.
- Run `just proof-p09-control-readiness-owner` on the frozen clean source and
  inspect its raw test and fuzz results. A local owner run does not issue a
  registered proof manifest or establish the Linux process/release rail.
- Component-loss readiness and bounded diagnostic closure require their own
  current-source negative and process evidence before release qualification.
