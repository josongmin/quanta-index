# Workspace crate ownership

The 25 root `crates/*/Cargo.toml` packages are indexed below. Each README
points to its implementation entry point. Directory size is not a crate
boundary decision; use dependency direction and change coupling when changing
package ownership. Nested fuzz manifests are tooling, not root packages.

## Production contract and core

| Crate | Owner surface |
| --- | --- |
| [quanta-index-contract-base](../../crates/quanta-index-contract-base/README.md) | Stable producer/search-plane identifiers, generation pins and query syntax without IPC envelopes. |
| [quanta-index-contract](../../crates/quanta-index-contract/README.md) | Shared IPC envelopes, requests, responses and wire validation. |
| [quanta-index-core](../../crates/quanta-index-core/README.md) | Domain ports, policies, budgets and validation shared by the search-plane adapters. |

## Production storage, indexing and providers

| Crate | Owner surface |
| --- | --- |
| [quanta-index-catalog](../../crates/quanta-index-catalog/README.md) | SQLite-backed idempotency, generation and lifecycle records used by the search plane. |
| [quanta-index-embed](../../crates/quanta-index-embed/README.md) | Local Model2Vec and opt-in remote embedding providers plus their cache; provider choice and egress remain explicit. |
| [quanta-index-lexical](../../crates/quanta-index-lexical/README.md) | Tantivy-backed generation materialization and lexical querying; also owns source-grounded code-search authority. |
| [quanta-index-semantic](../../crates/quanta-index-semantic/README.md) | LanceDB-backed persisted, generation-scoped vector index and ANN query adapter. |
| [quanta-index-repomap](../../crates/quanta-index-repomap/README.md) | Repository-map snapshots, delta application, persistence and pinned query views. |

## Query language components

| Crate | Owner surface |
| --- | --- |
| [quanta-index-lq-bridge](../../crates/quanta-index-lq-bridge/README.md) | Translates the documented Sourcegraph subset into typed LQ candidates. |
| [quanta-index-lq-norm](../../crates/quanta-index-lq-norm/README.md) | Parser, canonical query normalization, limits and stable query hashing. |
| [quanta-index-lq-obs](../../crates/quanta-index-lq-obs/README.md) | Typed emission surface for query observations; transport/exporters are wired elsewhere. |
| [quanta-index-lq-positions](../../crates/quanta-index-lq-positions/README.md) | Per-generation token-position index and phrase/proximity query primitives. |
| [quanta-index-lq-regex](../../crates/quanta-index-lq-regex/README.md) | Regex dialect validation, execution budgets and trigram prefilter integration. |
| [quanta-index-lq-structural](../../crates/quanta-index-lq-structural/README.md) | Language-scoped structural pattern IR and matching subset. Unsupported grammar remains a typed refusal. |
| [quanta-index-lq-text-normalizer](../../crates/quanta-index-lq-text-normalizer/README.md) | Shared Unicode normalization, case-folding and token-boundary contract for build and query paths. |
| [quanta-index-lq-trigram](../../crates/quanta-index-lq-trigram/README.md) | Generation-scoped byte-trigram postings for substring and regex candidate prefiltering. |

## Runtime and public entry points

| Crate | Owner surface |
| --- | --- |
| [quanta-index-ipc](../../crates/quanta-index-ipc/README.md) | CBOR framing, admission and Unix-domain socket client/server transport. Typed payloads live in quanta-index-contract. |
| [quanta-index-search-plane](../../crates/quanta-index-search-plane/README.md) | Persisted ingest, query and control state transitions after typed admission. |
| [quanta-index-searchd](../../crates/quanta-index-searchd/README.md) | Searchd application assembly and CLI command types. The executable wiring lives in quanta-index-searchd-runtime. |
| [quanta-index-searchd-runtime](../../crates/quanta-index-searchd-runtime/README.md) | Concrete adapter wiring and the quanta-index-searchd executable, including query/control/ingest socket serving. |
| [quanta-index-searchctl](../../crates/quanta-index-searchctl/README.md) | Operator CLI for search, readiness, diagnosis and state inspection. |
| [quanta-index-sdk](../../crates/quanta-index-sdk/README.md) | Public producer and reader facade with typed request/response binding. |

## Test and experiment packages

| Crate | Owner surface |
| --- | --- |
| [quanta-index-corpus-smoke](../../crates/quanta-index-corpus-smoke/README.md) | TOML corpus parsing, conformance runner and JUnit output for DSL fixtures; not a daemon serving path. |
| [quanta-index-searchd-harness](../../crates/quanta-index-searchd-harness/README.md) | Fixtures and benchmark drivers used as runtime dev dependencies; not part of the production daemon graph. |
| [quanta-index-scan-experiment](../../crates/quanta-index-scan-experiment/README.md) | Exploratory keyword scan comparison. Its output is not a qualified daemon or DSL benchmark. |

For current serving behavior, see [engine status](engine-status-v1.md).
For verification status, follow the [active residual ledger](../plans/sep-21-search-plane-sota-hardening/tickets/CURRENT-RESIDUAL-2026-09-26.md) and the selected gate.

## Boundary triage (2026-10-03 working tree)

| Crate | `src/**/*.rs` files / lines | Normal dependencies / reverse dependents |
| --- | ---: | ---: |
| `quanta-index-search-plane` | 144 / 68,241 | 13 / 4, including the retrieval benchmark |
| `quanta-index-lexical` | 83 / 42,146 | 16 / 3 |

`search-plane` has no normal dependency on `lexical`; its lexical dependency
is test-only. The current local splits reduced `ipc/server.rs` from 4,243 to
1,811 lines and `searchctl/src/lib.rs` from 5,057 to 578 lines, moving
behavior to named sibling modules. These numbers describe a concurrent dirty
worktree, not a qualified build measurement; remeasure after integration.
Further crate splits need evidence of change coupling and build cost at the
candidate boundary. Line count alone does not establish one.
