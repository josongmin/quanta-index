# Workspace crate ownership

The root `crates/*/Cargo.toml` packages are indexed below. Links point to the
crate README or implementation entry point. Directory size is not a crate
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
| [quanta-index-catalog](../../crates/quanta-index-catalog/README.md) | SQLite-backed durable publication, operation and lifetime authority; legacy and Native have distinct formats. |
| [quanta-index-embed](../../crates/quanta-index-embed/README.md) | Local Model2Vec and opt-in remote embedding providers plus their cache; provider choice and egress remain explicit. |
| [quanta-index-kernel](../../crates/quanta-index-kernel/src/lib.rs) | Native incremental publication and query coordination; the injected Catalog owns durable state. |
| [quanta-index-storage](../../crates/quanta-index-storage/src/lib.rs) | Native immutable candidate objects and bounded persistent maps; Catalog owns visibility and reclamation. |
| [quanta-index-scratch-sqlite](../../crates/quanta-index-scratch-sqlite/src/lib.rs) | Shared admission for disposable SQLite main databases; distinct from durable Catalog state. |
| [quanta-index-lexical](../../crates/quanta-index-lexical/README.md) | Legacy Tantivy and Native literal/text adapters; source-grounded code-search authority. |
| [quanta-index-semantic](../../crates/quanta-index-semantic/README.md) | Legacy LanceDB and Native semantic index/query adapters. |
| [quanta-index-repomap](../../crates/quanta-index-repomap/README.md) | Legacy RepoMap snapshots and the Native graph adapter. |

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
| [quanta-index-search-plane](../../crates/quanta-index-search-plane/README.md) | Legacy ingest, query and control transitions; Native coordination is in Kernel and Runtime. |
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
For verification status, follow the [current epics](../plans/oct-22-finalizing/README.md)
and the [source-scoped results](../plans/oct-22-finalizing/VALIDATION.md).

## Boundary triage

`search-plane` has no normal dependency on `lexical`; its lexical dependency
is test-only. The IPC server and searchctl entry points delegate to named
sibling modules. Recount files, lines and dependency edges from the selected
source revision before using them to justify a crate split. Require evidence
of change coupling and build cost at the proposed boundary; line count alone
does not establish one.
