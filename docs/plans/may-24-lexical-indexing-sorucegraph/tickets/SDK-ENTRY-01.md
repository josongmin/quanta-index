# SDK-ENTRY-01 — SDK-Only Entry + Source-Authority Ingest Cutover

Status: `partial-execution-live`

Scope:
- force every external producer / caller entrypoint through [`quanta-index-sdk`](../../../../crates/quanta-index-sdk/)
- land the missing history / runtime-metadata / structural source-authority inputs through dedicated typed SDK namespaces and batches, not raw socket / channel entry shims
- keep query-time evaluation local to `quanta-index`; no request-time git / source / tree-sitter / producer RPC

## 0. Current source-truth status (2026-05-27)

Landed on the current tree:

- `searchctl` production query entry is SDK-only
- public SDK happy-path proof now lives in `sdk_frontdoor` for lexical,
  semantic, hybrid, explain, repo-map, history, runtime, and structural paths
- dedicated typed SDK publish/query namespaces exist for history, runtime, and
  structural authority alongside lexical/semantic/repomap
- history source-authority query paths now fail closed with exact typed codes
  and no lexical fallback

Residue that keeps this ticket partial:

- `searchd` composition still uses internal legacy channel publishers,
  subscribers, and mirror paths
- richer history/runtime/structural feature families in this ticket are not all
  closed on the current tree
- semantic/hybrid public query surface still preserves the current
  vector/handle contract; the text-only semantic follow-on is deferred to
  `SEM-OWN`

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

2. Public query routes are split by audience, not by engine.
   - Primary public route for the new families is typed SDK query: `history().query()`, `runtime().query()`, `structural().query()`.
   - Raw text power-user route remains public: `lexical().query().native(...)` and `sourcegraph().query()`.
   - Typed builders own the canonical query DTO / AST surface.
   - Raw text routes lower into that same canonical engine. No parallel "typed engine" vs "text engine" implementation split.

3. Producer source-authority enters through dedicated SDK publish families.
   - [`LexicalBatch`](../../../../crates/quanta-index-sdk/src/lexical.rs) is lexical chunk / symbol publish only.
   - [`HistoryBatch`](../../../../crates/quanta-index-sdk/src/history.rs), [`DirtyBatch`](../../../../crates/quanta-index-sdk/src/runtime.rs), and [`StructuralBatch`](../../../../crates/quanta-index-sdk/src/structural.rs) own commit/diff, dirty, and parse-tree ingress respectively.
   - No external raw ingest DTO or ad hoc side namespace is exposed outside the SDK.

4. Search-side authority remains local.
   - Query-time `changed:` / `stale:` / `snapshot:` / `affected:` / `invalidated_by:` read local catalogs only.
   - `dirty:` reads a local dirty buffer fed by producer-authored ops.
   - Search side never walks git, never reads source bytes, never runs tree-sitter to fill missing authority.

5. Readiness authority is family-local.
   - `GenerationSelector` chooses coordinates only; it is not a readiness proof.
   - History / runtime / structural each gate on their own local materialization or catalog watermark.
   - Lexical seal is not reused as a readiness proxy for history / runtime / structural.

6. Public result carriers are family-specific.
   - History query returns `CommitCandidate` / `DiffCandidate`, not recycled `LexicalCandidate`.
   - `CommitCandidate` exposes `author_time_ms` and `committer_time_ms`; `applied_at_ms` stays operational ordering metadata, not query-time time authority.
   - Runtime query returns `RuntimeMetadataCandidate`, not `LexicalCandidate`.
   - Structural query returns `StructuralCandidate { spans, bindings, role_tags }`.

7. Missing authority and reserved features fail closed.
   - No producer record -> typed `NotReady` / `NotImplemented`
   - No hidden fallback to direct channel, raw socket, source scan, or tree-sitter.
   - `timeout` ships in this packet and is enforced in executor.
   - `index:only` is the only executable mode.
   - `index:no` is accepted on the canonical route and fails closed with typed `NotImplemented`.
   - `boost` is parser / carrier / planner-gate only; active ranking semantics stay out of scope for this packet.

---

## 2. Why this ticket exists

Current repo truth is split:

- query-side SDK surfaces already exist for lexical / sourcegraph / semantic / hybrid / symbol / generations / repomap / history / runtime / structural, but the new family semantics are still partial or fail-closed
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

| Feature family | Primary public entry | Secondary power-user entry | Producer/search-side authority input | Source owner | Required SDK / contract change |
|---|---|---|---|---|---|
| History filters | `history().query()` | `lexical().query().native(...)` / `sourcegraph().query()` | commit, ref, tag, diff-hunk records | `semantica-codegraph-v2` producer | dedicated `HistoryBatch` + typed history query DTO / result carrier |
| Runtime metadata | `runtime().query()` | `lexical().query().native(...)` / `sourcegraph().query()` | local catalogs for `changed/stale/snapshot`; dirty buffer for `dirty:` | mixed: local runtime catalogs + producer dirty ops | dedicated `DirtyBatch` + `RuntimeMetadataCandidate` query/result surface |
| Structural | `structural().query()` | `lexical().query().native(...)` / `sourcegraph().query()` | parse-tree records keyed by chunk | `semantica-codegraph-v2` producer | dedicated `StructuralBatch` + typed structural query DTO / richer structural result carrier |
| `timeout/index/boost` | typed query builders | raw text routes that lower to the same AST where syntax exists | none; planner/executor-local | `quanta-index` only | canonical option carrier: `timeout` executes, `index:no` typed-`NotImplemented`, `boost` planner-gated |

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

### QI-LXB-01 | quanta-index | Dedicated Source-Authority SDK Namespaces

Freeze the dedicated SDK publish split for history / runtime / structural authority.

**Owner files**
- [../../../../crates/quanta-index-sdk/src/lexical.rs](../../../../crates/quanta-index-sdk/src/lexical.rs)
- [../../../../crates/quanta-index-sdk/src/history.rs](../../../../crates/quanta-index-sdk/src/history.rs)
- [../../../../crates/quanta-index-sdk/src/runtime.rs](../../../../crates/quanta-index-sdk/src/runtime.rs)
- [../../../../crates/quanta-index-sdk/src/structural.rs](../../../../crates/quanta-index-sdk/src/structural.rs)
- [../../../../crates/quanta-index-contract/src/ipc/ingest.rs](../../../../crates/quanta-index-contract/src/ipc/ingest.rs)
- [../../../../crates/quanta-index-contract/src/channel/{ops,records}.rs](../../../../crates/quanta-index-contract/src/channel/ops.rs)

**Acceptance**
- `lexical().publish(batch)` is lexical chunk / symbol ingest only
- `history().publish(HistoryBatch)` owns commit / ref / tag / diff-hunk ingress
- `runtime().publish_dirty(DirtyBatch)` owns dirty ingress
- `structural().publish(StructuralBatch)` owns parse-tree ingress
- no external raw ingest namespace or raw DTO leaks out of the SDK

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
- dedicated SDK history publish surface from `QI-LXB-01`

**Acceptance**
- typed query DTO / builder carriers exist for:
  - `author`
  - `committer`
  - `message`
  - `before/after/since/until`
  - `diff.added/diff.removed/diff.touched`
- producer publishes commit/ref/tag/diff-hunk via `history().publish()` only
- query-time history search uses indexed local state only
- `CommitCandidate` exposes `author_time_ms` and `committer_time_ms`
- `applied_at_ms` remains ingest / ordering metadata only; it does not become the public time filter authority

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
- dedicated SDK runtime publish surface from `QI-LXB-01`

**Acceptance**
- typed query DTO / builder carriers exist for:
  - `changed`
  - `dirty`
  - `stale`
  - `snapshot`
  - gated `affected`
  - gated `invalidated_by`
- `dirty` enters only through `runtime().publish_dirty()` -> ingest IPC -> local dirty buffer
- `changed/stale/snapshot` are answered from local catalogs only
- `affected/invalidated_by` stay typed-gated until invalidation catalog exists
- runtime query response uses `RuntimeMetadataCandidate`, not `LexicalCandidate`

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
- dedicated SDK structural publish surface from `QI-LXB-01`
- [../../../ssot/producer-handoff.md](../../../ssot/producer-handoff.md)

**Acceptance**
- typed query DTO / builder carriers exist for:
  - `inside`
  - `outside`
  - `where`
  - typed hole
- parse-tree records enter only through `structural().publish()`
- no search-side tree-sitter extraction fallback
- typed hole support is blocked on producer-side semantic-role coverage if raw node kind is insufficient
- public structural results carry `spans`, `bindings`, and `role_tags`

**Blockers**
- `QI-LXB-01`
- parse-tree record shape freeze

**Proof**
- `UC-STR-*` rows in [../usecase.md](../usecase.md) are either live or typed-gated with explicit reasons

---

### QI-QRY-02 | quanta-index | Canonical Query Engine + Raw Text Parity

Keep one canonical query engine while preserving raw text power-user routes.

**Owner files**
- [../../../../crates/quanta-index-sdk/src/{history,runtime,structural,lexical,sourcegraph}.rs](../../../../crates/quanta-index-sdk/src/lib.rs)
- [../../../../crates/quanta-index-contract/src/query/requests.rs](../../../../crates/quanta-index-contract/src/query/requests.rs)
- [../../../../crates/quanta-index-contract/src/results/query_responses.rs](../../../../crates/quanta-index-contract/src/results/query_responses.rs)
- [../../../../crates/quanta-index-search-plane/src/query_dispatcher.rs](../../../../crates/quanta-index-search-plane/src/query_dispatcher.rs)

**Acceptance**
- typed public route exists and is primary for the new families:
  - `history().query()`
  - `runtime().query()`
  - `structural().query()`
- raw text routes remain public and lower into the same canonical engine:
  - `lexical().query().native(...)`
  - `sourcegraph().query()`
- no public raw query-envelope builders leak out of SDK
- `index:no` lowers onto the canonical route and fails closed with typed `NotImplemented`
- `boost` lowers into the canonical carrier but planner refuses active ranking semantics in this packet
- `timeout` lowers and is enforced in executor

**Blockers**
- `QI-HIST-01`
- `QI-RT-02`
- `QI-STR-02`

**Proof**
- SDK examples compile against the new public query/result surface

---

### SM-SDK-02 | semantica | Producer Authority via SDK Only

The producer repo emits every new source-authority family through `quanta-index-sdk`.

**Owner repo**
- `semantica-codegraph-v2`

**Acceptance**
- no direct `quanta-index-channel` publisher in producer call sites
- no raw split-IPC envelope assembly for publish paths
- commit/diff emissions flow through `history().publish()`
- dirty emissions flow through `runtime().publish_dirty()`
- parse-tree emissions flow through `structural().publish()`

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

Parallelism is safe only inside step 3 once the dedicated SDK publish split is frozen.

---

## 6. Non-goals

- overloading `LexicalBatch` with history / runtime / structural authority families
- letting semantica publish raw channel ops directly "temporarily"
- request-time producer callbacks for `dirty:` / history / structural
- search-side git / source / tree-sitter fallback
- lexical seal reused as history / runtime / structural readiness proof
- a separate raw-text execution engine that diverges from the typed query engine
- claiming full Sourcegraph parity from this ticket alone
