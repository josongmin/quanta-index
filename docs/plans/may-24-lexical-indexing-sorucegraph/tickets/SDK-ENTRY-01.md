# SDK-ENTRY-01 — SDK-Only Entry + Source-Authority Ingest Cutover

Status: `planning`

Scope:
- force every external producer / caller entrypoint through [`quanta-index-sdk`](../../../../crates/quanta-index-sdk/)
- land the missing history / runtime-metadata / structural source-authority inputs without adding raw socket / channel entry shims
- keep query-time evaluation local to `quanta-index`; no request-time git / source / tree-sitter / producer RPC

Parent docs:
- [../closeout-plan.md](../closeout-plan.md)
- [../feature-scope.md](../feature-scope.md)
- [../usecase.md](../usecase.md)
- [../../may-25-sdk-cutover-wave-plan.md](../../may-25-sdk-cutover-wave-plan.md)
- [../../../ssot/producer-handoff.md](../../../ssot/producer-handoff.md)

---

## 1. Locked decisions

1. External entry is SDK-only.
   - Producers in `semantica-codegraph-v2` do not publish channel ops directly.
   - Callers do not assemble split-IPC envelopes or socket paths directly.
   - Raw `SearchPlane*IpcRequest`, `LexicalChannelOp`, UDS socket strings remain internal surfaces.

2. Query front doors stay text-first.
   - Native LQ goes through `quanta-index-sdk::LexicalNamespace::query()`.
   - Sourcegraph text goes through `quanta-index-sdk::SourcegraphNamespace::query()`.
   - History / runtime-metadata / structural are expressed through the same text-query builders after AST expansion.
   - Do not add public `HistoryNamespace` / `RuntimeNamespace` / `StructuralNamespace` unless a non-text typed API is proven necessary.

3. Producer source-authority for lexical-track extensions enters through one SDK publish surface.
   - Extend [`LexicalBatch`](../../../../crates/quanta-index-sdk/src/lexical.rs) rather than adding a second external ingest namespace.
   - New lexical-track input families:
     - commit / ref / tag / diff-hunk
     - dirty upsert / evict
     - parse-tree upsert / delete

4. Search-side authority remains local.
   - Query-time `changed:` / `stale:` / `snapshot:` / `affected:` / `invalidated_by:` read local catalogs only.
   - `dirty:` reads a local dirty buffer fed by producer-authored ops.
   - Search side never walks git, never reads source bytes, never runs tree-sitter to fill missing authority.

5. Missing authority fails closed.
   - No producer record -> typed `NotReady` / `NotImplemented`
   - No hidden fallback to direct channel, raw socket, source scan, or tree-sitter.

---

## 2. Why this ticket exists

Current repo truth is split:

- query-side SDK surfaces exist for lexical / sourcegraph / semantic / hybrid / symbol / generations / repomap
- ingest-side SDK surface exists, but [`LexicalBatch`](../../../../crates/quanta-index-sdk/src/lexical.rs) only carries chunks and symbols
- remaining feature families that still need real source authority are:
  - history filters (`author:` / `message:` / `before:` / `after:` / `diff.*`)
  - runtime metadata (`dirty:` producer feed; local catalogs for `changed:` / `stale:` / `snapshot:`)
  - structural operators (`inside` / `outside` / `where` / typed hole)

Without this ticket, the likely failure mode is predictable:

- query grammar expands
- producer/source-authority payloads arrive through ad hoc direct channel code or raw split IPC
- SDK becomes an optional wrapper instead of the single external entry surface

This ticket prevents that.

---

## 3. Feature-to-source matrix

| Feature family | User-facing entry | Producer/search-side authority input | Source owner | Required SDK change |
|---|---|---|---|---|
| History filters | `lexical().query().native(...)` / `sourcegraph().query()` | commit, ref, tag, diff-hunk records | `semantica-codegraph-v2` producer | extend `LexicalBatch` with history mutations |
| Runtime metadata | same text-query builders | local catalogs for `changed/stale/snapshot`; dirty buffer for `dirty:` | mixed: local runtime catalogs + producer dirty ops | extend `LexicalBatch` with dirty mutations; no new query namespace |
| Structural | same text-query builders | parse-tree records keyed by chunk | `semantica-codegraph-v2` producer | extend `LexicalBatch` with parse-tree mutations |
| `boost/timeout/index` | same text-query builders | none; planner/executor-local | `quanta-index` only | AST/options + SDK builder passthrough only |

---

## 4. Ticket set

### QI-SDK-02 | quanta-index | External Entry Freeze

Force all external query / control / ingest examples and supported call paths through `quanta-index-sdk`.

**Owner files**
- [../../../../crates/quanta-index-sdk/src/lib.rs](../../../../crates/quanta-index-sdk/src/lib.rs)
- [../../../../crates/quanta-index-sdk/src/{lexical,sourcegraph,search,semantic,repomap,generations,transport}.rs](../../../../crates/quanta-index-sdk/src/lib.rs)
- [../../may-25-sdk-cutover-wave-plan.md](../../may-25-sdk-cutover-wave-plan.md)
- docs / examples that currently show raw contract or socket usage

**Acceptance**
- public docs point to SDK calls only for producer / caller integration
- new features in this packet do not introduce raw external socket examples
- SDK remains the only supported external composition surface

**Blockers**
- none

**Proof**
- `rg "SearchPlane(Query|Control|Ingest)IpcRequest" docs crates -g '*.md' -g '*.rs'`
  - expected: non-test external usage lives only in internal runtime / dispatcher code, not consumer examples

---

### QI-LXB-01 | quanta-index | Extended `LexicalBatch` Contract

Make `LexicalBatch` the single SDK ingress for all lexical-track producer authority.

**Owner files**
- [../../../../crates/quanta-index-sdk/src/lexical.rs](../../../../crates/quanta-index-sdk/src/lexical.rs)
- [../../../../crates/quanta-index-contract/src/ipc/ingest.rs](../../../../crates/quanta-index-contract/src/ipc/ingest.rs)
- [../../../../crates/quanta-index-contract/src/channel/{ops,records}.rs](../../../../crates/quanta-index-contract/src/channel/ops.rs)

**Acceptance**
- `LexicalBatch` grows typed mutation families for:
  - commit
  - ref / tag
  - diff-hunk
  - dirty upsert / evict
  - parse-tree upsert / delete
- SDK `lexical().publish(batch)` remains the only external publish entry for those families
- no new external ingest namespace is introduced for history/runtime/structural

**Blockers**
- source-record shape freeze in `producer-handoff.md`

**Proof**
- `cargo check -p quanta-index-contract -p quanta-index-sdk`
- SDK unit tests cover batch assembly for every new mutation family

---

### QI-HIST-01 | quanta-index | History Source-Authority Ingest + Query Closeout

Land history filters against producer-authored commit / diff inputs.

**Owner files**
- [../../../../crates/quanta-index-contract/src/channel/{ops,records}.rs](../../../../crates/quanta-index-contract/src/channel/ops.rs)
- [../../../../crates/quanta-index-lq-norm/src/{ast.rs,parser/mod.rs}](../../../../crates/quanta-index-lq-norm/src/ast.rs)
- history materializer / executor crates
- SDK lexical batch extensions from `QI-LXB-01`

**Acceptance**
- query carriers exist for:
  - `author`
  - `committer`
  - `message`
  - `before/after/since/until`
  - `diff.added/diff.removed/diff.touched`
- producer publishes commit/ref/tag/diff-hunk via SDK lexical batch only
- query-time history search uses indexed local state only

**Blockers**
- `QI-LXB-01`
- producer agreement on timestamp semantics and diff-hunk wire shape

**Proof**
- `UC-HIST-*` rows in [../usecase.md](../usecase.md) have live E2E coverage

---

### QI-RT-02 | quanta-index | Runtime Metadata Source-Authority Cutover

Land runtime metadata filters with the correct split between producer-fed dirty state and local runtime catalogs.

**Owner files**
- runtime metadata AST/parser surface
- local runtime catalog / dirty buffer owners
- SDK lexical batch extensions from `QI-LXB-01`

**Acceptance**
- query carriers exist for:
  - `changed`
  - `dirty`
  - `stale`
  - `snapshot`
  - gated `affected`
  - gated `invalidated_by`
- `dirty` enters only through SDK lexical batch -> ingest IPC -> local dirty buffer
- `changed/stale/snapshot` are answered from local catalogs only
- `affected/invalidated_by` stay typed-gated until invalidation catalog exists

**Blockers**
- `QI-LXB-01`
- runtime catalog materializers

**Proof**
- `UC-RT-*` rows in [../usecase.md](../usecase.md) have focused tests or typed-gate tests

---

### QI-STR-02 | quanta-index | Structural Source-Authority Cutover

Land rich structural operators against producer-authored parse trees.

**Owner files**
- structural AST / parser
- parse-tree store / matcher
- SDK lexical batch extensions from `QI-LXB-01`
- [../../../ssot/producer-handoff.md](../../../ssot/producer-handoff.md)

**Acceptance**
- query carriers exist for:
  - `inside`
  - `outside`
  - `where`
  - typed hole
- parse-tree records enter only through SDK lexical batch
- no search-side tree-sitter extraction fallback
- typed hole support is blocked on producer-side semantic-role coverage if raw node kind is insufficient

**Blockers**
- `QI-LXB-01`
- parse-tree record shape freeze

**Proof**
- `UC-STR-*` rows in [../usecase.md](../usecase.md) are either live or typed-gated with explicit reasons

---

### QI-QRY-02 | quanta-index | Text Query Builders Remain the Public Front Door

Keep the public query entry surface narrow while feature coverage grows.

**Owner files**
- [../../../../crates/quanta-index-sdk/src/{lexical,sourcegraph,search}.rs](../../../../crates/quanta-index-sdk/src/lib.rs)
- query contract DTOs

**Acceptance**
- history / runtime-metadata / structural text queries are reachable through:
  - `lexical().query().native(...)`
  - `sourcegraph().query()`
- no public raw query-envelope builders leak out of SDK
- public result types are updated if history / structural need richer carriers than `LexicalCandidate`

**Blockers**
- `QI-HIST-01`
- `QI-RT-02`
- `QI-STR-02`

**Proof**
- SDK examples compile against the new public query/result surface

---

### SM-SDK-02 | semantica | Producer Authority via SDK Only

The producer repo emits every new lexical-track authority family through `quanta-index-sdk`.

**Owner repo**
- `semantica-codegraph-v2`

**Acceptance**
- no direct `quanta-index-channel` publisher in producer call sites
- no raw split-IPC envelope assembly for publish paths
- commit/diff/dirty/parse-tree emissions flow through SDK lexical batch only

**Blockers**
- `QI-SDK-02`
- `QI-LXB-01`

**Proof**
- tracked in semantica repo; quanta-index side only records the dependency

---

### QI-VRF-02 | quanta-index | Delete Raw Entry Residue + Full Proof

Close the packet with SDK-only external entry and real source-authority proof.

**Acceptance**
- docs/examples point to SDK only
- producer-input families are proven through SDK publish -> ingest -> local materialize -> query
- no external raw socket / contract examples survive for supported paths

**Proof**
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- `just rust-profile verify-rust`

---

## 5. Execution order

1. `QI-SDK-02`
2. `QI-LXB-01`
3. `QI-HIST-01` ∥ `QI-RT-02` ∥ `QI-STR-02`
4. `QI-QRY-02`
5. `SM-SDK-02`
6. `QI-VRF-02`

Parallelism is safe only inside step 3 once the SDK lexical-batch contract is frozen.

---

## 6. Non-goals

- adding a second public ingest namespace for history/runtime/structural
- letting semantica publish raw channel ops directly "temporarily"
- request-time producer callbacks for `dirty:` / history / structural
- search-side git / source / tree-sitter fallback
- claiming full Sourcegraph parity from this ticket alone
