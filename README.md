# quanta-index

External search plane for Semantica/Quanta. Producers publish typed search
records through `quanta-index-sdk`; this workspace owns indexing, generations,
activation and lexical/semantic/hybrid serving over Unix sockets.

## Usage

- [Search CLI](crates/quanta-index-searchctl/README.md): query, diagnose and scrape the daemon.
- [Code-search benchmark results and reading guide](tools/benchmark/CODE_SEARCH_RUNBOOK.md#required-result-table): five products in one table, with mode input scope and the current diagnostic snapshot.
- [Other benchmark profiles](tools/benchmark/README.md): corpus releases, Criterion, DSL gates and recording imports.
- [State operations](docs/operator/state-cutover-runbook.md): verify, backup, restore and rebuild.
- [Embedding provider](docs/potion-code-embedder.md): local model preparation.

Build and start the daemon against an external current-format state root:

```sh
./scripts/cargow --lane daemon-lane build -p quanta-index-searchd-runtime --bin quanta-index-searchd --locked
./scripts/cargow --lane daemon-lane run -p quanta-index-searchd-runtime --bin quanta-index-searchd --locked -- serve --state-root /absolute/state-root
```

Publish typed producer input through the SDK before querying an active generation.
Legacy generations require a rebuild; see the state operations guide.

## Build and verification


- use `./scripts/cargow ...` for raw Cargo commands
- use `just ...` for repo recipes
- agent/default entrypoint: `just rust-profile <name>`
- both route `target/` and related local caches to the shared external cache root instead of the repo working tree
- when installed locally, `sccache` is enabled on a repository-isolated server
  for cacheable clean rebuild work; set `QUANTA_INDEX_SCCACHE=0` to disable it

Recommended Rust profiles:

- `just rust-profile dev-fast` — default local edit loop
- `just rust-profile dev-daemon` — daemon/runtime-only loop
- `just rust-profile dev-all-targets` — widest compile rail after shared-surface edits
- `just rust-profile validate-shared-surface` — one test-profile compile plus bounded contract/core/sdk/search-plane validation
- `just rust-profile test-fast` — default local test loop
- `just rust-profile test-integration-fast` — bounded integration loop
- `just rust-profile test-integration-storage` — text-authority shard persistence slice
- `just rust-profile test-integration-semantic` — Lance/DataFusion-backed semantic targets
- `just rust-profile test-integration` — complete fast + storage + semantic integration rail
- `just rust-profile test-daemon-fast` — bounded daemon edit loop; excludes DSL cold-matrix truth
- `just rust-profile test-daemon` — runtime scenario suite plus DSL truth
- `just rust-profile test-daemon-all` — extended runtime scenario suite plus DSL truth
- `just rust-profile verify-rust` — standard merge gate
- `just rust-profile verify-rust-heavy` — nightly/heavy correctness rail
- `just rust-profile timings-fast` / `timings-daemon` — build-regression capture rails

Rule:

- prefer a named profile over synthesizing raw `cargo` feature/target sets
- only drop to raw `cargo` when the profile catalog does not cover the task

Build profile history:

- `scripts/cargow` appends lane-level JSONL history under `{state_root}/build-profile/history.jsonl`
- `just rust-profile <name>` appends high-level profile selection history to the same file
- entries are compact by default: timestamp, lane/profile key, subcommand/recipe, duration, and exit code only
- `just rust-profile-history-summary` renders the accumulated profile/lane/failure summary

Quality gates:

- lint front door: `just rust-check`, `just rust-clippy`, `just python-lint`, `just semgrep`
- test pyramid:
  - unit: `just rust-test-unit`
  - bounded integration/component: `just rust-test-integration-fast`
  - lexical storage integration: `just rust-test-integration-storage`
  - semantic storage integration: `just rust-test-integration-semantic`
  - complete integration/component: `just rust-test-integration`
  - fast e2e: `just rust-test-e2e-fast`
  - risk-focused e2e: `just rust-test-e2e`
  - exhaustive daemon e2e: `just rust-test-e2e-all`
  - full workspace: `just rust-test`

The integration, CLI-smoke, and daemon recipes resolve target IDs from
`tools/ci/test-authority.toml` and run one nextest process per scope. Timing
rails refuse to start while unrelated Cargo/rustc processes are active; set
`QUANTA_INDEX_ALLOW_CONTENDED_TIMINGS=1` only for non-authoritative diagnosis.

### Local resource options

The wrapper serializes leaf build/test work using the shared cache-root slot.
Inspect `wait_ns`/`held_ns` diagnostics for queue versus command time. These
observations do not qualify benchmark-host idleness or performance.

| Variable | Default / usage |
| --- | --- |
| `CARGO_BUILD_JOBS` | 4 locally when unset; explicit values and Cargo `--jobs` keep precedence. `CI=true` leaves an unset budget unset. |
| `QUANTA_INDEX_RESOURCE_ADMISSION` | `auto`: local admission, diagnostic bypass in CI; `1` enables in CI; `0` disables; other values refuse. |
| `QUANTA_INDEX_RESOURCE_WAIT_SECONDS` | 300, positive integer; admission timeout exits 124. |
| `QUANTA_INDEX_RESOURCE_TIMEOUT_SECONDS` | 7200, positive integer; command timeout exits 124. |

Use one shared `QUANTA_INDEX_CACHE_ROOT` when coordinating checkout/lane work.
Do not wrap a whole orchestrator in the leaf lock: nested wrapper calls acquire it.
A quiet performance host and explicit bound-binary reuse still need their own
admission. [Capture/resource decisions](docs/adr/SEP-27-004-benchmark-capture-and-resource-custody.md)
own the guarantees; this section is operator usage.

## Source and architecture

| Package | Role |
| --- | --- |
| `quanta-index-contract` | Bundle, query, control and IPC DTOs |
| `quanta-index-core` | Vendor-neutral ports and validation |
| `quanta-index-search-plane` | Persisted ingest/query/control authority |
| `quanta-index-lexical`, `quanta-index-semantic` | Tantivy and LanceDB adapters |
| `quanta-index-ipc` | CBOR transport framing |
| `quanta-index-searchd`, `quanta-index-searchd-runtime` | Composition and daemon executable |
| `quanta-index-searchctl` | Operator CLI |
| `quanta-index-sdk` | External producer/client facade |

Accepted contracts are in [ADRs](docs/adr/README.md). Generated references:
[DSL capabilities](docs/reference/dsl-capabilities.md) and
[Sourcegraph filter coverage](docs/reference/sourcegraph-filter-parity.md).

## Active work

- [Production/state/cross-repository residuals](docs/plans/sep-21-search-plane-sota-hardening/tickets/FINAL-RESIDUAL-EXECUTION-PLAN.md)
- [Code-search engine and benchmark acceptance](docs/plans/sep-27-code-search-remediation/readme.md)
- [Benchmark execution and measurement ledger](docs/plans/sep-27-misc/tickets/INDEX.md)
- [Semantic ownership proof](docs/plans/may-25-search-owned-semantic-derivation/README.md)
- [Search quality acceptance](docs/plans/jun-7-search-product-quality/tickets-wave2/INDEX.md)
- [Test hardening acceptance](docs/plans/jul-15-sota-test-hardening/tickets/00-ticket-status-board.md)

Completed tickets and historical reports are indexed for Git recovery in the
[documentation archive](docs/ARCHIVE-INDEX.md) and [plan archive](docs/plans/ARCHIVE-INDEX.md).
Use fresh source-bound results for verification; README command lists are not
qualification evidence.
