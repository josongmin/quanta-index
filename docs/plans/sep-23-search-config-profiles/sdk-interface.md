# SDK interface target for search configuration/profile V1

> Archive status: `Superseded proposal`. Accepted provider policy: [SEP-21-003](../../adr/SEP-21-003-read-view-continuation-and-provider-policy.md). Current proposed SDK shape: [Sep-24 SDK DSL RFC](../sep-24-sdk-dsl-rfc.md). The parent packet remains active. Archive map: [Completed Plan Archive](../ARCHIVE-INDEX.md).


- Status: superseded for SDK public-shape and source-compatibility decisions by [Sep-24 SDK DSL RFC](../sep-24-sdk-dsl-rfc.md). This earlier proposal remains background for the server/client configuration boundary; no SDK or wire implementation is claimed.
- Source audit: local `1306325e81189a8c0df5d38c56567664a6f20c1d` on 2026-09-24. Concurrent benchmark/CI files were dirty; re-freeze source and owners before implementation.
- Parent: [search configuration RFC](rfc.md). The server remains the authority for effective policy and sealed-generation compatibility.
- Semantic cutover target: one producer-authored typed-source live path, with no
  derive-mode environment setting or chunk-text fallback. This document does
  not qualify the server binary or cross-repo producer.

## 1. Decision

Keep `QuantaIndex` as the one transport-backed client and preserve the existing `reader()`, `producer()`, `control()` and typed namespace routes. The SDK owns transport connection, request construction, client-side validation and exact response binding. The daemon owns embedding, lexical/semantic build recipes, query execution profiles, provider grants, resource budgets and activation policy. **Do not mirror daemon environment variables or quality-profile parameters into `ConnectOptions`.**

`ClientProfile::{Full,QueryOnly}` currently means *transport access*. Keep that meaning; call the server concepts `IndexBuildRecipeV1`, `QueryExecutionProfileV1` and `EffectivePolicyDigestV1`. Do not overload `ClientProfile` with search-quality semantics. A query-only connection must continue constructing only the query transport; a method requiring an absent plane retains typed `PlaneUnavailable`.

### Intended call shape (illustrative, existing lexical methods)

```rust
let index = QuantaIndex::connect_query_only(
    ConnectOptions::from_state_root(root).with_request_io_timeout(timeout),
)?;

let page = index.reader()
    .lexical()
    .native("symbol:Parser")
    .active(repo_id, revision_id)
    .top_k(20)
    .execute()?;
```

The builder selects request semantics and bounds; it does not choose an embedding model or claim that a generation is compatible. The returned page's actual pinned generation and ranking/trace data are the result authority. The SDK still validates the response against the sent request before returning it.

## 2. Four separate configuration planes

| Plane | Owner and exposed surface | Examples | Change rule |
|---|---|---|---|
| Client transport | SDK `ConnectOptions` | state-root/socket selection, I/O timeout/deadline | Local client choice; no daemon or index rewrite. Prefer private fields and additive setters. |
| Request intent | Typed query/batch builders or exact request DTOs | active/pinned selector, syntax, constraints, `top_k`, bounded scope, continuation | Permit only values already represented and validated by the wire contract. No untyped option map. |
| Server execution policy | `SearchdConfig` and immutable named recipes/profiles | typed semantic-source contract, model identity, RRF/ANN recipe, memory/provider budgets | Resolve once at boot; a changed profile requires the RFC's generation compatibility and release proof. SDK may display a **server-reported** effective identity, never predict or set it. |
| Artifact/read identity | Daemon's sealed generation, read view and response | generation pin, manifest/content roots, model/normalizer/ANN evidence | Query/activation checks remain server-side. The SDK binds the response to the request; it does not recalculate seal compatibility. |

V1 does **not** need a public SDK profile selector or new wire field solely to mirror `index-build-v1`/`query-execution-v1`: both are compiled server choices. If an operator needs to inspect them remotely later, add a typed **read-only** `effective_policy()`/capability response with its own privilege, version, redaction and freshness contract. Do not infer it from `ConnectOptions`, `ClientProfile` or a previous query. A `config show` CLI response is not automatically a remote SDK endpoint.

## 3. Stability boundary

1. **Stable facade:** preserve `QuantaIndex::connect`, `connect_query_only`, `reader`, `producer`, `control`, existing namespaces and their route-specific methods. `ConnectOptions` already has private fields and additive `with_*` methods; extend it only for client transport concerns.
2. **Typed intent:** retain focused builders for common calls and `*_request` methods for exact replay. Do not grow a single `search(options: HashMap<String, Value>)` or a giant generic request builder. A new bounded option can add a setter when the server already has a typed, stable request field and an independent acceptance test.
3. **Wire truth:** current `SemanticQueryRequest` is all-public and has an explicit closed serializer; query request/response IPC enums are exhaustive. Adding a field or route can break source/wire consumers and requires a deliberate versioned contract review. Do not claim that `#[non_exhaustive]` can be retrofitted without a source break. Add a V2 request/route when semantics or serialization cannot be extended compatibly; preserve V1 behavior during a stated migration window or make an explicit breaking-first release decision.
4. **No fake extensibility:** model selection, ANN build parameters, arbitrary provider endpoint, ranking weights and per-request execution profile are not SDK knobs in V1. Each can change persisted/query meaning, authorization or cost. A future opt-in profile requires a server-supported name, capability/compatibility rule, visibility in response and continuation, measured quality/cost proof, and a versioned request contract.
5. **Response authority:** keep the existing request ID check, exact expected response variant, read-identity/candidate/window/order/receipt binding and typed remote errors. `SDK_WIRE_ROUTES_V1` and its owner test must include each new wire entrypoint; never accept an unknown response variant through a generic fallback. A remote-reported policy digest is diagnostic, not an authenticated deployment identity or a substitute for the generation pin.
6. **Sync first:** keep the current synchronous UDS API as canonical. An async facade should be considered only for demonstrated concurrent-client demand and must share request validation, transport framing and response binding. Do not introduce an unqualified async/streaming twin for interface symmetry.

The expected stable unit is the **semantic operation**, not its exact Rust builder generic parameters. Existing typestate builders catch missing required fields at compile time; additional optional features should not multiply generic flags or make every combination a separate implementation. Keep optional validated setters on the existing builder where they do not alter the operation's meaning. For a genuinely different operation, add a focused builder/route.

## 4. API growth protocol

| Change | SDK shape | Server/wire obligation |
|---|---|---|
| New transport timeout or connection path | Add `ConnectOptions::with_*`; preserve default/precedence | None if framing and privilege are unchanged. |
| Existing request's safe optional bound/filter | Add a typed builder setter and exact-request field only if source/wire compatible | Validate on both sides; prove old omission has identical behavior. Otherwise create V2. |
| New search operation, e.g. distinct rerank or batch query | New focused namespace method/builder and request/response variant | Exact response binder, route inventory, limits, error codes and semantic E2E. No generic escape hatch. |
| New model/index/query recipe | No automatic SDK setter | New server profile identity, generation gate, candidate-root/quality proof; add request selection only if the product explicitly supports multiple profiles per daemon. |
| Remote effective-policy inspection | New read-only typed endpoint only if needed | Privilege/redaction, schema version, source freshness, no secret or root leakage. Query-only access requires an explicit threat-model decision. |

For each new wire route, one owner must update `quanta-index-contract` request/response codec, daemon dispatcher, `quanta-index-sdk/src/binding.rs` expected variant and contextual axes, `client.rs` exhaustive dispatch/kind/active-pin handling, one focused namespace method, `SDK_WIRE_ROUTES_V1`, and the route owner test. This explicit closure is a safety property. Centralize duplicated plumbing only if the exact binder and compile-time exhaustive accounting remain visible; do not replace it with string-dispatched plugins.

## 5. File-level blast radius and integration

| Existing file | V1 decision / future touch trigger |
|---|---|
| `crates/quanta-index-sdk/src/config.rs` | Preserve client-only `ConnectOptions` and `ClientProfile`; fix only a proven transport-resolution gap. Do not import `SearchdConfig`. |
| `crates/quanta-index-sdk/src/client.rs` | Preserve the facade and privilege planes. A new route updates exact pin/dispatch/kind handling; a remote policy read is a separate additive method after wire ownership is fixed. |
| `crates/quanta-index-sdk/src/lexical.rs`, `crates/quanta-index-sdk/src/semantic.rs`, `crates/quanta-index-sdk/src/search.rs`, `crates/quanta-index-sdk/src/text_query_builder.rs` | Keep current typed route builders; add a setter only for a typed wire-supported option. New operation gets its own focused builder. |
| `crates/quanta-index-sdk/src/binding.rs`, `crates/quanta-index-sdk/tests/sdk_binding_owner_v1.rs` | Required exact response binding and route coverage for any new API. Inject wrong ID, variant, pin, order, stale/duplicate/partial rows and wrong receipt in negative oracles as applicable. |
| `crates/quanta-index-sdk/src/error.rs` | Keep transport, protocol/binding, usage, absent-plane and typed remote refusal distinct. A new error meaning must not be hidden as `Usage` or success-shaped fallback. |
| `crates/quanta-index-contract/src/query/requests.rs`, `crates/quanta-index-contract/src/ipc/split.rs` | Conditional only for an actual new wire operation/selector. All-public DTO and exhaustive-enum compatibility must be reviewed before adding fields/variants. |
| `crates/quanta-index-searchd/src/app/config.rs`, `crates/quanta-index-searchd/src/app/runtime.rs` | Server policy stays here; SDK should not duplicate its default/profile/knob resolution. |
| `tools/ci/lint/baselines/public-api/quanta-index-sdk.txt` | Update only after reviewed public API change; baseline churn is not a proof of compatibility. |

Integration order: (1) freeze source and classify the proposal as transport, request, server policy or artifact identity; (2) freeze old-call behavior and negative response-binding oracle; (3) implement server/wire authority first if needed; (4) add SDK typed facade and binder; (5) run SDK owner tests, exact route-coverage test, `just rust-public-api`, and for wire/error changes `just rust-fuzz-smoke`; (6) run the affected daemon E2E and one old-client/new-server compatibility scenario if compatibility is claimed. Record exact binary/source/config/generation identity. An SDK compile or unit test alone is not server compatibility proof.

## 6. Current assessment and limits

- Existing foundation is already close to the target: one `QuantaIndex` facade, privilege-separated query-only transport, typed namespaces/builders, canonical publish digest, exact response binding and route inventory.
- Main interface risk is **public DTO and closed enum evolution**, not a shortage of configuration knobs. New wire APIs currently fan out through the explicit SDK and daemon matches; retain exhaustive correctness while reducing only demonstrably duplicated plumbing.
- The configuration RFC changes daemon policy and semantic format, but does not itself require a new SDK request or response field. Adding one preemptively would widen migration and public-API blast radius without a V1 user need.
- No current source or runtime test in this design pass proves cross-version client/server compatibility or future async/streaming performance. Qualify those only when requested by an actual release contract.

## References

- [Cargo SemVer compatibility](https://doc.rust-lang.org/cargo/reference/semver.html): all-public struct fields and exhaustive enums constrain additive API evolution; `non_exhaustive` should be chosen when introducing a type.
- [AWS SDK behavior versions](https://docs.rs/aws-config/latest/aws_config/struct.ConfigLoader.html): explicit behavior versioning separates stable client configuration from later default-behavior changes. Used here as a design analogy, not a requirement to copy its feature set.
- [Elasticsearch Rust client overview](https://www.elastic.co/guide/en/elasticsearch/client/rust-api/current/overview.html): typed endpoint builders and transport configuration are precedent for a focused client facade; Quanta's generation pin and sealed response binding remain its own contract.
