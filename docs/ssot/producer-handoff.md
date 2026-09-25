# Producer Handoff — Historical Pre-De-channelize Wire Shapes

> Status: `Historical archive. Not current-tree authority after the 2026-05-27 de-channelize cutover.`
> Owners: producer-side lead (`semantica-codegraph-v2`), search-side lead (this repo).
> Parent SSOT: [channel-architecture.md](channel-architecture.md) (historical parent; this doc extends its archived channel op catalogue).
> Posture: **breaking-first** per [AGENT_CORE.md](../../AGENT_CORE.md) § Design Defaults. No long-lived shims.

Current-tree note (2026-05-27):

- this document describes the old channel-centric handoff model
- the current runtime hot path is de-channelized; do not treat
  `open_lexical_subscriber`, `open_semantic_subscriber`, or `ChannelDispatcher`
  as live source truth for the daemon
- the standalone `quanta-index-channel` crate is retired from the workspace;
  only the contract-level channel DTOs remain live as historical/internal
  carriers
- semantic query/public SDK truth has also changed: public semantic publish is
  removed, semantic query/hybrid are live query surfaces, and semantic corpus
  derivation happens inside `searchd` from typed semantic sources; default
  the intended cutover requires producer-authored typed semantic sources and
  removes the derive-mode environment setting and chunk-text fallback;
  this archive does not establish current runtime or cross-repo qualification

This is a historical archive of the old producer/search channel contract that
was used to reason about [LEX-07](../plans/may-24-lexical-indexing-sourcegraph/tickets/LEX-07.md), [RT-01](../plans/may-24-lexical-indexing-sourcegraph/tickets/RT-01.md), and [STR-01](../plans/may-24-lexical-indexing-sourcegraph/tickets/STR-01.md). It captures the prior 9-op channel framing and the 11 AMB-PROD-* ambiguity resolutions that predated the typed-batch UDS cutover.

If anything here conflicts with live source, live source wins. For the current
tree use `SearchPlaneIngestIpcRequest::{PublishHistoryBatch, PublishDirtyBatch,
PublishStructuralBatch}` plus the persisted authority stores in
`crates/quanta-index-search-plane/src/{ingest_dispatcher,readiness}.rs`.

---

## Table of Contents

1. [Document Identity](#1-document-identity)
2. [Producer Authorship Rule (Verbatim)](#2-producer-authorship-rule-verbatim)
3. [Op Catalogue](#3-op-catalogue)
   1. [3.1 History Track — UpsertCommit / UpsertRef / UpsertTag / DeleteRef / DeleteTag](#31-history-track--upsertcommit--upsertref--upserttag--deleteref--deletetag)
   2. [3.2 Runtime Track — UpsertDirty / EvictDirty](#32-runtime-track--upsertdirty--evictdirty)
   3. [3.3 Structural Track — UpsertParseTree / DeleteParseTree (gated)](#33-structural-track--upsertparsetree--deleteparsetree-gated)
   4. [3.4 SymbolRecord (extends existing UpsertSymbol payload)](#34-symbolrecord-extends-existing-upsertsymbol-payload)
   5. [3.5 Delta contract — producer emission semantics](#35-delta-contract--producer-emission-semantics)
4. [Language Matrix](#4-language-matrix)
5. [Versioning & Cutover Policy](#5-versioning--cutover-policy)
6. [Error Contract](#6-error-contract)
7. [Decision Matrix for AMB-PROD-1..11](#7-decision-matrix-for-amb-prod-111)
8. [Versioning + Integration Handshake Protocol](#8-versioning--integration-handshake-protocol)
9. [Cross-References](#9-cross-references)

---

## 1. Document Identity

| Field | Value |
|---|---|
| Status | Historical archive. Do not treat §1–§8 as pending producer sign-off on the current tree. |
| Owners | Producer lead (`semantica-codegraph-v2` repo) + search-side lead (this repo). |
| Parent SSOT | [channel-architecture.md](channel-architecture.md) — particularly [§0 Scope](channel-architecture.md), [§3.1 Op enums + Authorship rule lock](channel-architecture.md), [§5.2 Generation state authority](channel-architecture.md). |
| Authoritativeness | This doc is the single source of truth for the wire shape, emission ordering, and error semantics of every op listed in §3. Downstream ticket specs ([LEX-05](../plans/may-24-lexical-indexing-sourcegraph/tickets/LEX-05.md), [LEX-07](../plans/may-24-lexical-indexing-sourcegraph/tickets/LEX-07.md), [STR-01](../plans/may-24-lexical-indexing-sourcegraph/tickets/STR-01.md), [RT-01](../plans/may-24-lexical-indexing-sourcegraph/tickets/RT-01.md)) reference this doc; conflicts resolve in favour of this doc. |
| Trigger | The producer-authorship correction in [INDEX.md §3.6](../plans/may-24-lexical-indexing-sourcegraph/tickets/INDEX.md) inverted four tickets to consume producer-emitted records. The 9 ops + 11 AMB-PROD-* questions in [INDEX.md §3.7](../plans/may-24-lexical-indexing-sourcegraph/tickets/INDEX.md) need a single coherent answer. This doc is it. |
| Cutover gate | LEX-07 / RT-01 / STR-01 integration cutover is blocked on producer sign-off of this doc per §8. |
| Posture | Breaking-first per [AGENT_CORE.md](../../AGENT_CORE.md). No long-lived compatibility shims; one canonical wire shape per generation; cross-team cutover is lock-step. |

---

## 2. Producer Authorship Rule

The following rule, locked in [channel-architecture.md §3.1](channel-architecture.md), governs every op below.

> **Authorship rule (typed-only cutover target):** the producer in `semantica-codegraph-v2`
> authors chunk/symbol/commit/parse-tree/structural/history/dirty handoff
> payloads and, under the typed-only cutover, publishes typed semantic-source
> replace/tombstone scopes or intentional empty semantic deltas. The wire
> does not independently attest empty-list coverage; producer proof is required. The
> search plane validates those sources and derives embeddings from their
> rendered text via `semantic_derive`. The target live path has no derive-mode
> environment setting or legacy chunk-text fallback. Producer-authored
> `RawCodeFallback` is a typed source. `EmbeddingRecord`/`UpsertEmbedding`
> language is historical archive material only; it is not the current serving
> contract.

### 2.1 Anti-pattern register (forbidden on the search side)

| Anti-pattern | Where it would land | Why forbidden |
|---|---|---|
| Search plane reads `*.rs` / `*.py` source bytes | any adapter crate | violates [channel-architecture.md §0](channel-architecture.md) out-of-scope; collapses producer/search split. |
| Search plane shells out to `git log`, `git rev-parse`, `git diff` | history adapter | violates authorship rule; producer is git authority. |
| Search plane runs `tree-sitter` against source | structural / symbol adapters | violates authorship rule; producer ships parse trees. |
| Producer ships ready-made semantic vectors for serving | semantic adapter | violates the dense-owner rule; live semantic vectors are derived by the search plane from typed semantic sources. |
| Heuristic best-effort "fill-in" when the producer record is absent | any adapter | violates [AGENT_CORE.md](../../AGENT_CORE.md) "Authority must be typed or explicit"; must surface typed `NotReady` instead. |
| A second producer→search ingress channel (e.g. an `apply_changes` UDS IPC) | any | violates [channel-architecture.md §11 rule 6](channel-architecture.md) — `BundleChannelPublisher::publish` is the only ingress. |

If any of these appear in a PR against this repo, the PR is defective; correct it before merge.

### 2.2 What the search plane DOES

| Step | Action |
|---|---|
| 1 | Subscribe to the lexical / semantic channel via [channel-architecture.md §3.2 factory surface](channel-architecture.md) (`open_lexical_subscriber`, `open_semantic_subscriber`). |
| 2 | Decode each op body via the per-op CBOR decoder. On decode failure, raise typed `*_DECODE_FAIL` per §6 — do not skip. |
| 3 | Validate the decoded record against the wire-shape contract in §3 (required fields, enum domain, monotonic spans). On failure, raise `*_INVALID` per §6 — do not skip. |
| 4 | Insert / delete the addressed document in the per-shard index. |
| 5 | Flip the per-track generation ledger ([channel-architecture.md §5.2](channel-architecture.md)) on `Seal`. |
| 6 | Acknowledge the channel cursor only after successful apply. |

---

## 3. Op Catalogue

For every op below: channel, wire shape, emission ordering guarantee, idempotency, error semantics. Wire shape is normative; producer encodes via CBOR-canonical (`ciborium` or equivalent). Search side decodes via hand-rolled serde per [AGENT_RULE_CATALOG.md](../../AGENT_RULE_CATALOG.md) build hygiene (no `#[derive(Serialize|Deserialize)]`).

### 3.1 History Track — UpsertCommit / UpsertRef / UpsertTag / DeleteRef / DeleteTag

**Channel:** [`LexicalChannelOp`](../../crates/quanta-index-contract/src/channel/ops.rs).
**Consumed by:** [LEX-07](../plans/may-24-lexical-indexing-sourcegraph/tickets/LEX-07.md).

#### 3.1.1 Wire shapes

```text
CommitRecord {
    wire_version:    u32,                    // §5 cutover policy
    sha:             [u8; 20],               // git sha1
    parents:         Vec<[u8; 20]>,          // topological parents; empty for root
    applied_at_ms:   u64,                    // producer monotonic stamp at integration time
    author:          Box<str>,               // "Name <email>" canonical
    committer:       Box<str>,               // "Name <email>" canonical
    message:         Box<str>,               // full commit message, UTF-8
    is_merge:        bool,                   // parents.len() >= 2
    tags:            Vec<Box<str>>,          // tag names pointing at this sha; may be empty
}
```

```text
UpsertCommit { repo, revision, generation, commit: CommitRecord }
UpsertRef    { repo, revision, generation, name: Box<str>, sha: [u8; 20] }
UpsertTag    { repo, revision, generation, name: Box<str>, sha: [u8; 20] }
DeleteRef    { repo, revision, generation, name: Box<str> }
DeleteTag    { repo, revision, generation, name: Box<str> }
```

`name` is a fully-qualified ref name (e.g. `refs/heads/main`, `refs/tags/v1.2.3`) — the producer is the canonical source.

`commit.parents` carries the full producer-authoritative DAG view at integration time. A force-pushed branch that rewrites history is **not** modelled by retro-mutating existing commits; see §3.1.3 (DeleteCommit / force-push) below.

#### 3.1.2 Emission ordering guarantee (AMB-PROD-1)

The producer MUST guarantee, per `(repo, revision, generation)` window:

1. **Topological order** — for every `UpsertCommit { commit }`, all shas in `commit.parents` MUST appear in an earlier `UpsertCommit` in the same `(repo, revision, generation)` window, OR be exactly the empty `parents: []` case (root).
2. **Ref / tag ordering** — `UpsertRef` / `UpsertTag` MUST follow the `UpsertCommit` whose `sha` they reference. A ref pointing at an absent sha is a producer bug; the search side raises `HISTORY_REF_NOT_FOUND` and does not buffer.
3. **Channel seq monotonicity** — already guaranteed by the channel ([channel-architecture.md §4.2 frame format](channel-architecture.md)).

This lets the search side ([LEX-07 §5.0 Step Pre-A](../plans/may-24-lexical-indexing-sourcegraph/tickets/LEX-07.md)) insert each commit into the local `CommitGraph` in arrival order without a separate buffer-and-replay phase.

Search-side enforcement: on `UpsertCommit` whose `commit.parents` references an unknown sha → typed `HISTORY_COMMIT_PARENT_UNKNOWN`; track marked degraded per [channel-architecture.md §4.6](channel-architecture.md). No silent buffering, no heuristic fallback.

#### 3.1.3 DeleteCommit — NOT in v1. Force-push handling.

**Decision (AMB-PROD-3):** there is no `DeleteCommit` op in v1.

Force-pushed branches that rewrite history are handled by emitting a **fresh generation**:

1. Producer detects the history rewrite.
2. Producer assigns a new `manifest_generation = N+1`.
3. Producer emits the new commit DAG as `UpsertCommit` ops under `(repo, revision, generation=N+1)`.
4. Producer emits `Seal { repo, revision, generation=N+1 }` when complete.
5. Search side opens the N+1 generation; the N+1 commit DAG supersedes N.
6. The N generation persists in the search-side ledger until retention reaps it ([channel-architecture.md §4.3 segment GC](channel-architecture.md)); queries pinned to N still serve correctly.

No mutation of an existing commit row is ever permitted on the wire. Generations are append-only; rewrites create a new generation.

#### 3.1.4 Diff hunk authorship (AMB-PROD-4)

[LEX-07 §4.5](../plans/may-24-lexical-indexing-sourcegraph/tickets/LEX-07.md) ships a per-generation `diff_hunks/` sibling shard with fields `{commit_id, repo_relative_path, hunk_id, side, added_text, removed_text, touched_text}`. There is currently no op that carries this payload. Two options:

| Option | Shape | Op count | Per-op size | Streaming behaviour |
|---|---|---|---|---|
| **X — inline** | `UpsertCommit.commit.hunks: Vec<DiffHunk>` field added to `CommitRecord` | 1 per commit | large (proportional to commit diff size) | a large commit blocks the channel until fully encoded |
| **Y — separate op (recommended)** | new op `UpsertDiffHunk { repo, revision, generation, commit_sha: [u8;20], file_path: Box<str>, hunk: DiffHunkRecord }` | 1 per hunk | small (single-hunk granularity) | hunks stream incrementally; large commits do not block other commits |

`DiffHunkRecord` shape (under Option Y):

```text
DiffHunkRecord {
    wire_version:    u32,
    hunk_id:         Box<str>,               // producer-assigned, unique within (commit_sha, file_path)
    side:            DiffSide,               // Added | Removed | Touched
    added_text:      Box<str>,               // UTF-8; empty for Removed-only hunks
    removed_text:    Box<str>,               // UTF-8; empty for Added-only hunks
    touched_text:    Box<str>,               // UTF-8 context; may be empty
    byte_start:      u32,                    // post-edit file offset
    byte_end:        u32,                    // post-edit file offset
}

DiffSide = Added | Removed | Touched
```

**Recommendation: Option Y.** Smaller per-op payloads keep frame size under the 16 MiB cap ([channel-architecture.md §4.2](channel-architecture.md)) for large commits, preserve streaming hygiene, and align with the per-record incremental boundary already locked in [LEX-05 §3.6](../plans/may-24-lexical-indexing-sourcegraph/tickets/LEX-05.md).

Ordering under Option Y: every `UpsertDiffHunk { commit_sha }` MUST follow the corresponding `UpsertCommit { commit.sha == commit_sha }` in the same generation window. Producer decides the final call; this doc records the recommendation. Tracked in §7 as `AMB-PROD-4`.

#### 3.1.5 Idempotency

Re-applying any of `UpsertCommit` / `UpsertRef` / `UpsertTag` / `DeleteRef` / `DeleteTag` for the same `(repo, revision, generation, key)` after a search-side restart MUST be a no-op (the channel guarantees at-least-once; cursor persistence per [channel-architecture.md §4.5](channel-architecture.md) makes duplicates expected). Search side dedupes by `(generation, sha)` for commits and `(generation, name)` for refs/tags.

### 3.2 Runtime Track — UpsertDirty / EvictDirty

**Channel:** [`LexicalChannelOp`](../../crates/quanta-index-contract/src/channel/ops.rs).
**Consumed by:** [RT-01](../plans/may-24-lexical-indexing-sourcegraph/tickets/RT-01.md).

#### 3.2.1 Wire shapes

```text
UpsertDirty {
    repo:             RepoId,
    revision:         RevisionId,
    generation:       ManifestGeneration,
    doc_id:           ChunkId,                // existing contract type
    applied_at_ms:    u64,                    // producer monotonic stamp
    payload_hash:     [u8; 32],               // producer's content hash, opaque to search
}

EvictDirty {
    repo:             RepoId,
    revision:         RevisionId,
    generation:       ManifestGeneration,
    doc_id:           ChunkId,
}
```

`doc_id` is the chunk identity already shared via [`UpsertChunk`](../../crates/quanta-index-contract/src/channel/ops.rs). `generation` MUST be the **currently-active** generation at the time the producer emits the op — see §3.2.4.

#### 3.2.2 Emission cadence (AMB-PROD-7)

The producer chooses cadence. Default proposal:

| Mode | When to use | Behaviour |
|---|---|---|
| Per-edit | low edit rate | one `UpsertDirty` per file save; latency-optimal |
| Debounced | high edit rate (typing) | one `UpsertDirty` per `doc_id` per 100 ms ceiling; reduces channel load |

Either is acceptable; the search side does NOT prescribe. The producer-side ADR is recorded in `semantica-codegraph-v2` and named here for traceability. The 100 ms debounce ceiling is a recommendation, not a hard limit — the channel SSOT does not impose one.

#### 3.2.3 Ordering at the same doc_id (AMB-PROD-8)

The channel's monotonic seq per track ([channel-architecture.md §4.2](channel-architecture.md)) is the sole authority on ordering.

Producer contract: for any `doc_id D`, if the producer emits `EvictDirty { doc_id: D }` at seq `s_evict`, any subsequent edit to `D` MUST be emitted as `UpsertDirty { doc_id: D }` at seq `> s_evict`. This makes the channel-final state deterministic regardless of arrival skew.

Search-side guarantee: the dispatcher applies ops in seq order ([channel-architecture.md §5.3](channel-architecture.md)); the buffer end-state equals the channel-final state.

#### 3.2.4 Generation alignment

`UpsertDirty` MUST carry the currently-active `generation`. If the producer emits an `UpsertDirty` with a `generation` strictly less than the search-side currently-active gen, the search side raises `DIRTY_STALE_GEN` per [RT-01 §4.6](../plans/may-24-lexical-indexing-sourcegraph/tickets/RT-01.md) and drops the entry from the buffer (channel cursor still advances; this is not a corruption).

On `Seal` for `generation = N+1`, the search side evicts all dirty entries pinned to `N` — the producer is expected to re-emit `UpsertDirty` against the new generation if the dirty state persists across the seal.

#### 3.2.5 WAL retention horizon (AMB-PROD-9)

[Channel-architecture.md §4.3](channel-architecture.md) says segment GC happens after subscriber ack at a rotation boundary. [RT-01 §4.6](../plans/may-24-lexical-indexing-sourcegraph/tickets/RT-01.md) sets the search-side TTL default at **300 seconds**.

Rule: the producer-side WAL segment retention horizon MUST be greater than or equal to `max(subscriber_lag, search_side_dirty_ttl)`. With default TTL of 300 s, the producer keeps segments alive for at least 300 s past subscriber ack so a search-side restart within the TTL window can replay the buffer.

This is a producer-side configuration concern; the channel SSOT does not enforce a minimum. Tracked as `AMB-PROD-9` in §7; the producer-side ADR pins the actual horizon.

#### 3.2.6 Validation timing (AMB-PROD-10)

`DIRTY_BAD_IDENTITY` (an `UpsertDirty.doc_id` that does not correspond to any known chunk in the active generation) is enforced **synchronously at decode + apply time** in `DirtyBuffer::apply`, per [RT-01 §8 / §4.6](../plans/may-24-lexical-indexing-sourcegraph/tickets/RT-01.md). This is NOT eventually-consistent.

Rejected ops never enter the buffer; the typed event `runtime.dirty.rejected{reason=DIRTY_BAD_IDENTITY}` surfaces on the observability rail; the channel cursor still advances (the op was structurally valid but semantically rejected).

#### 3.2.7 Idempotency

`EvictDirty` for an absent `doc_id` is a no-op `Ok(None)` (per [RT-01 §5.5](../plans/may-24-lexical-indexing-sourcegraph/tickets/RT-01.md)). Channel at-least-once delivery makes this expected on restart.

### 3.3 Structural Track — UpsertParseTree / DeleteParseTree (gated)

**Channel:** [`LexicalChannelOp`](../../crates/quanta-index-contract/src/channel/ops.rs).
**Consumed by:** [STR-01](../plans/may-24-lexical-indexing-sourcegraph/tickets/STR-01.md).
**Gating:** This op only fires if STR-01 Option A is chosen ([STR-01 §1.1](../plans/may-24-lexical-indexing-sourcegraph/tickets/STR-01.md)). If STR-01 ships under Option B (v2 deferral), this op never appears on the wire. The integrator picks Option A vs B at wave-5 entry; tracked as `AMB-PROD-11` in §7.

#### 3.3.1 Wire shapes

```text
ParseTreeRecord {
    wire_version:    u32,
    lang:            LangId,                 // §4 language matrix
    root:            ParseNode,
    source_hash:     [u8; 32],               // producer's content hash of the chunk source
}

ParseNode {
    kind:            Box<str>,               // grammar node name, producer-authoritative
    byte_start:      u32,                    // post-normalize chunk-local byte offset
    byte_end:        u32,                    // post-normalize chunk-local byte offset
    children:        Vec<ParseNode>,
}

UpsertParseTree { repo, revision, generation, chunk_id: ChunkId, tree: ParseTreeRecord }
DeleteParseTree { repo, revision, generation, chunk_id: ChunkId }
```

`ParseNode` is recursive; the producer encodes the full tree-of-nodes per chunk. The `source_hash` lets the search side detect drift between the parse tree and the chunk text (if both are present); on mismatch, raise `STR_PARSE_TREE_DECODE_FAIL{reason=source_hash_mismatch}`.

#### 3.3.2 LangId set

`LangId` enum is the closed set per §4 (`Rust | Python | TypeScript | JavaScript | Go`). Records with `lang` outside this set MUST NOT be emitted; emission of an out-of-set value is `SYMBOL_RECORD_INVALID` / `STR_PARSE_TREE_DECODE_FAIL` (see §6). Note that the v1 ship set may grow per §4; growth is coordinated via §8 handshake.

#### 3.3.3 Ordering

`UpsertParseTree { chunk_id }` MUST follow the corresponding `UpsertChunk { chunk_id }` in the same `(repo, revision, generation)` window. The chunk identity is the binding key between the lexical-content payload and the parse tree.

`DeleteParseTree { chunk_id }` may arrive independently of `DeleteChunk`; the producer chooses whether to keep parse trees around for chunks that no longer exist. The search side treats `DeleteParseTree` as a pure delete on the structural sibling index.

#### 3.3.4 Capacity

Per-record limits inherited from [STR-01 §3](../plans/may-24-lexical-indexing-sourcegraph/tickets/STR-01.md): there is no producer-side cap on parse tree depth or node count, but search-side query-time limits apply (256 nodes / 16 depth for the **pattern**, not the tree being matched). A pathological tree (e.g. > 1 MiB encoded) approaches the 16 MiB frame cap ([channel-architecture.md §4.2](channel-architecture.md)); the producer is responsible for splitting chunks if needed.

### 3.4 SymbolRecord (extends existing UpsertSymbol payload)

**Channel:** [`LexicalChannelOp::UpsertSymbol`](../../crates/quanta-index-contract/src/channel/ops.rs) (already shipped).
**Consumed by:** [LEX-05](../plans/may-24-lexical-indexing-sourcegraph/tickets/LEX-05.md).

The current shipped `UpsertSymbol` struct in [crates/quanta-index-contract/src/channel/ops.rs](../../crates/quanta-index-contract/src/channel/ops.rs) carries `payload: Vec<u8>` — opaque at the transport layer. This section formalises the CBOR decoded shape.

#### 3.4.1 Wire shape

```text
SymbolRecord {
    wire_version:        u32,                    // §5 cutover policy (AMB-PROD-5)
    name:                Box<str>,               // canonical identifier name (post-normalize)
    kind:                SymbolKind,             // closed enum, §3.4.2
    span:                SymbolSpan,
    lang:                LangId,                 // §4 language matrix
    parent:              Option<Box<str>>,       // canonical name of enclosing symbol; None at top level
    container_name:      Option<Box<str>>,       // module / class / namespace path; producer-assigned
    relationship:        SymbolRelationship,     // Def | Ref
}

SymbolSpan {
    path:                Box<str>,               // repo-relative path, forward-slash
    byte_start:          u32,
    byte_end:            u32,                    // must satisfy byte_end >= byte_start
    line_start:          u32,                    // 1-based inclusive
    line_end:            u32,                    // 1-based inclusive; line_end >= line_start
}
```

The CBOR `payload` bytes inside `UpsertSymbol.payload` decode to a `SymbolRecord` value. The decoder lives at `crates/quanta-index-lexical/src/symbol/decode.rs` per [LEX-05 §4.1](../plans/may-24-lexical-indexing-sourcegraph/tickets/LEX-05.md).

The wire-shape identity carrier of `UpsertSymbol` is `symbol_id` (assigned by the producer). Search-side index identity is the dedup key per §3.5.1; the search side records the wire `symbol_id` in its sidecar so producer-driven `DeleteSymbol { symbol_id }` resolves to the correct shard entry.

#### 3.4.2 SymbolKind (closed enum, v1)

The current tree carries the v1 12-kind policy in planner-facing
[`SymbolKindFilter`](../../crates/quanta-index-lexical/src/symbol.rs), while
the wire carrier is validated by
[`SymbolKindCode`](../../crates/quanta-index-contract/src/lex/symbol.rs).
Producer-emitted symbol kinds are therefore pinned to this 12-code subset:

```text
SymbolKind = function | method | class | struct | enum
           | trait    | interface | variable | constant
           | module   | macro     | type_alias
```

12 variants. The producer emits exactly one per `SymbolRecord`. An out-of-set value is `SYMBOL_RECORD_INVALID{field=kind}` per §6.

Note: [LEX-05 §3.3](../plans/may-24-lexical-indexing-sourcegraph/tickets/LEX-05.md) lists 20 variants for a longer-term landing. This doc pins the **v1 wire set** at 12. Variants 13..20 are reserved; emitting them requires a coordinated `wire_version` bump per §5.

#### 3.4.3 SymbolRelationship

```text
SymbolRelationship = Def | Ref
```

This is the boundary lock from [LEX-05 §3.5](../plans/may-24-lexical-indexing-sourcegraph/tickets/LEX-05.md): the producer distinguishes definition records from reference records. The search side does not resolve cross-file `Def → Ref` edges in v1; that is downstream (SEM-01 / cross-ref planner family).

#### 3.4.4 Versioning policy (AMB-PROD-5)

Every `SymbolRecord` carries `wire_version: u32`. Bumping requires a coordinated cut per §5 and the §8 handshake. Recommended: producer authors a wire-shape ADR in `semantica-codegraph-v2` per revision; search side pins `[min_wire_version, max_wire_version]` in [`quanta-index-contract::channel`](../../crates/quanta-index-contract/src/channel/ops.rs).

### 3.5 Delta contract — producer emission semantics

This section is the authoritative contract for **delta-handling semantics** across every op in §3.1–§3.4 plus the already-shipped `Upsert*` / `Delete*` family. It exists because the channel transport guarantees at-least-once delivery and replay (per [channel-architecture.md §4.5](channel-architecture.md), [§4.7](channel-architecture.md)), but the per-builder application semantics on the search side need an explicit identity / cascade / replay rule. Without this section, two valid-looking producer emissions can drive a search-side builder into accumulated duplicates or orphaned shard entries.

Round-5 builder hardening (`upsert_X`, `remove_X`, `from_prior(...)` APIs across trigram / positions / symbol / scorer) is the search-side implementation of these rules. Code-level surface lives in the LQ builder crates ([SHIPPED.md §7 item 2](../plans/may-24-lexical-indexing-sourcegraph/SHIPPED.md)) and is not normative; this section is.

#### 3.5.1 Identity rules per record type

The producer carries one wire identity per op; the search side has a corresponding shard identity that may be narrower (e.g. symbol identity is `(doc_id, name, kind, span.byte_start)` because the same `symbol_id` can be re-emitted with a normalised name).

| Op | Wire identity (producer-assigned) | Search-side shard identity | Replacement on same wire identity |
|---|---|---|---|
| `UpsertChunk { chunk: ChunkRecord }` | `chunk.chunk_id` | `chunk_id` | overwrite — idempotent via `TrigramIndexBuilder::upsert_doc` + `PositionsBuilder::upsert_doc` + `ScorerBuilder::upsert_doc` |
| `UpsertSymbol { symbol: SymbolRecord }` | `symbol.symbol_id` | `(doc_id, name, kind, span.byte_start)` per `SymbolIndexBuilder::upsert_symbol` | overwrite at the wire-identity slot; the shard identity is the dedup key inside that slot |
| `UpsertCommit { commit: CommitRecord }` | `commit.sha` | `(generation, sha)` per §3.1.5 | overwrite allowed but rare; force-push triggers fresh generation per §3.1.3 (AMB-PROD-3) |
| `UpsertRef { name, sha }` / `UpsertTag { name, sha }` | `(generation, name)` | same | overwrite — last-write-wins per generation |
| `UpsertDirty { doc_id, applied_at_ms, payload_hash }` | `doc_id` | `doc_id` | overwrite — RT-01 dirty buffer is idempotent per [RT-01 §5.5](../plans/may-24-lexical-indexing-sourcegraph/tickets/RT-01.md) |
| `UpsertParseTree { chunk_id, tree }` | `chunk_id` | `chunk_id` | overwrite — replaces the structural sibling entry |
| `UpsertEmbedding { embedding: EmbeddingRecord }` | `embedding.embedding_id` | `embedding_id` | overwrite — HNSW idempotent insert |

Producer contract: re-emitting `UpsertX` with the same wire identity and a different payload body MUST be a deliberate overwrite. Search side never accumulates two records under the same wire identity; the LAST emission wins per channel seq order (per §3.5.4).

#### 3.5.2 Delete semantics and cascade

Deletes are typed at the wire level; some cascade across sibling shards, others do not. The cascade graph below is normative.

| Op | Direct shard | Cascades to |
|---|---|---|
| `DeleteChunk { chunk_id }` | lexical text shard | trigram shard (per-doc trigram set removed) + positions shard (per-doc positions removed) + scorer shard (per-doc length/term-freq contribution removed) + symbol shard via `SymbolIndexBuilder::remove_doc(doc_id)` (every symbol whose `(doc_id, …)` matches is purged) + structural shard (every `ParseTreeRecord { chunk_id == this }` is purged) + semantic shard via `SemanticIndexBuilder::remove_embedding(doc_id)` (every embedding whose `embedding_id == chunk_id` per §3.5.1 wire-identity equivalence is tombstoned; gated on Round 7a HNSW delta API landing per [tickets/RFC-SEM-02.md §12 Q-RFC-SEM-02-3](../plans/may-24-lexical-indexing-sourcegraph/tickets/RFC-SEM-02.md)) |
| `DeleteSymbol { symbol_id }` | symbol shard | **none** — only the named symbol_id is removed; other symbols in the same doc remain |
| `DeleteRef { name }` / `DeleteTag { name }` | ref / tag index | none |
| `EvictDirty { doc_id }` | RT-01 dirty buffer | none |
| `DeleteParseTree { chunk_id }` | structural sibling shard | none — may arrive independently of `DeleteChunk` per §3.3.3 |
| `DeleteEmbedding { embedding_id }` | semantic shard | none — tombstones the HNSW node |

Producer-side commitment: there is **no `DeleteCommit` op in v1**. Force-pushed branches that rewrite history are handled exclusively by emitting a fresh generation per §3.1.3. The search side has no API surface for retro-deletion of a commit row; emitting one would violate the append-only generation invariant.

A `Delete*` whose key is absent on the search side is a no-op (`Ok(None)`), not a typed error. Channel at-least-once delivery makes this expected on subscriber restart.

#### 3.5.3 Replay semantics

The channel guarantees at-least-once delivery and supports replay from any acked cursor per [channel-architecture.md §4.5](channel-architecture.md). The contract below pins the producer + search-side responsibilities under replay.

| Side | Guarantee |
|---|---|
| Producer | Never emit two ops with different effects under the same `(track, channel_seq)`. Once a seq is published, its body is immutable; a retried publish must reuse the same body bytes. |
| Producer | Never reorder ops within a `(repo, revision, generation)` window across a restart. Producer crash recovery reads the last-published seq and resumes from `seq + 1`. |
| Search | Apply ops in strict channel seq order. Every `Upsert*` and `Delete*` MUST be safe to replay (idempotent on the wire identity per §3.5.1). |
| Search | Acknowledge the cursor only **after** a successful apply, never before. The atomic order is: apply succeeds → cursor ack persists → next event polled. |

On subscriber restart, the dispatcher reads the persisted cursor and resumes from `cursor + 1`. Any op whose seq exceeds the in-memory ledger's last-applied seq is replayed against the same builder; the §3.5.1 identity rules guarantee no duplicate accumulation.

#### 3.5.4 In-generation delta semantics

Within ONE generation window `(repo, revision, generation)`, the producer MAY emit any sequence of `Upsert*` / `Delete*` against the same wire identity. The search-side state after a `Seal { generation }` equals the channel-final state under strict seq order.

Three patterns the producer MAY emit, and the resulting search-side state:

| Pattern within one generation | Search-side state at Seal |
|---|---|
| `UpsertX(id, A)` → `UpsertX(id, B)` | id maps to B |
| `UpsertX(id, A)` → `DeleteX(id)` | id is absent |
| `DeleteX(id)` → `UpsertX(id, A)` | id maps to A |

The search-side builder's `upsert_X` / `remove_X` APIs (Round-5 hardening) implement the per-builder mechanics that make these patterns idempotent. The producer never needs to compact in-generation deltas before emission; the search side honours the LAST emission.

#### 3.5.5 Cross-generation delta semantics

When a new generation `N+1` opens, the producer chooses one of two modes:

| Mode | Wire | Search-side build |
|---|---|---|
| **(a) FullBundle reset** | `FullBundle { generation: N+1, payload }` + per-record `Upsert*` + `Seal { generation: N+1 }` | Builder starts from empty; payload is the authoritative full state. |
| **(b) Delta against prior gen** | `Upsert*` / `Delete*` ops referencing the prior gen's records + `Seal { generation: N+1 }` | Builder inherits prior-gen state via the new `from_prior(...)` API (Round-5 hardening across trigram / positions / symbol / scorer), then applies the delta stream. |

Both modes are supported. Mode (b) is the steady-state path; mode (a) is reserved for bootstrap, force-push recovery (§3.1.3), and explicit producer-side rebuild. The producer picks mode per generation; the search side honours whichever appears on the wire.

The producer MUST NOT mix modes within one generation window: emitting a `FullBundle` after one or more `Upsert*` ops in the same generation is a producer-side bug. The search-side dispatcher rejects this with `STATE_GENERATION_REGRESSION` per [channel-architecture.md §5.2](channel-architecture.md).

#### 3.5.6 Ordering

Channel seq monotonicity (per [channel-architecture.md §4.2](channel-architecture.md)) is the sole authority on ordering within a track. The §3.1.2 / §3.2.3 / §3.3.3 per-track ordering rules layer on top of this.

For the history track specifically: the producer SHOULD topologically order `UpsertCommit` ops so that parents precede children within a generation window (per §3.1.2 / AMB-PROD-1). If the producer cannot guarantee topo order (e.g. parallel-walked DAG segments arrive interleaved), the search-side `CommitGraph::with_buffering` mode resolves out-of-order arrivals by buffering until parents land, surfacing `HISTORY_TRACE_INCOMPLETE` as a wait-and-retry code rather than `HISTORY_COMMIT_PARENT_UNKNOWN` as a hard reject. The buffering mode is opt-in at the search-side composition root; topological emission is the recommended producer default.

For every other track (lexical content, runtime dirty, semantic embedding), there is no parent / child ordering constraint beyond seq monotonicity; the search side applies ops as they arrive.

---

## 4. Language Matrix

### 4.1 v1 ship set

```text
LangId = Rust | Python | TypeScript | JavaScript | Go
```

Five langs. This is the closed set the search side ships in v1.

### 4.2 Producer shipping policy

The producer MAY ship records (`SymbolRecord`, `ParseTreeRecord`, future records carrying `lang`) for languages outside the v1 ship set. Behaviour:

| Case | Search-side behaviour |
|---|---|
| `lang ∈ v1 ship set` | indexed normally; queryable via `lang:` filter. |
| `lang ∉ v1 ship set` but is a valid `LangId` variant | indexed if/when the variant is added to the v1 set; until then, dropped at index time with a typed observability counter `lq_symbol_dropped_unknown_lang_total{lang}` / `lq_parse_tree_dropped_unknown_lang_total{lang}`. **NO silent rejection** — every drop emits a typed event. |
| `lang` is not a valid `LangId` value at all | `SYMBOL_RECORD_INVALID{field=lang}` / `STR_PARSE_TREE_DECODE_FAIL{reason=lang_invalid}` per §6. |

### 4.3 LangId expansion policy

Growing the ship set (e.g. adding `Java`) requires:

1. Producer-side ADR landing first (defining grammar, span semantics, identifier normalisation).
2. Coordinated search-side PR adding the variant to `LangId` and the per-language test fixtures.
3. `wire_version` bump per §5.
4. §8 handshake re-run with new conformance fixtures.

The producer CANNOT unilaterally widen the closed set on the wire. Doing so produces `SYMBOL_RECORD_INVALID` until the search side catches up.

---

## 5. Versioning & Cutover Policy

### 5.1 wire_version field

Every wire shape introduced in this doc carries an explicit `wire_version: u32` field:

- `CommitRecord.wire_version`
- `DiffHunkRecord.wire_version` (Option Y)
- `ParseTreeRecord.wire_version`
- `SymbolRecord.wire_version`

`UpsertDirty` / `EvictDirty` payloads are simple enough that they currently do not carry their own `wire_version` field; their shape is pinned by the `LexicalChannelOp` enum tag itself, and any breaking change is signalled by adding a new variant per [channel-architecture.md §3.1](channel-architecture.md).

### 5.2 One version per generation

The producer MUST emit a single `wire_version` per `(repo, revision, generation)` window. Mixing wire versions within a generation surfaces as `*_PAYLOAD_DECODE_FAIL{at_seq, reason=wire_version_drift_within_generation}` per §6. Generation seal locks the wire version for that generation's index.

### 5.3 Breaking changes

Per [AGENT_CORE.md](../../AGENT_CORE.md) breaking-first posture:

| Rule | Action |
|---|---|
| No long-lived shims | when `wire_version` bumps `N → N+1`, the search side accepts only N+1 from the next generation forward; N stays decodable for already-sealed generations until retention reaps them. |
| Lock-step cutover | producer + search side ship coordinated PRs; neither side merges before the other has a green build against the new shape. |
| No dual surface | the search-side decoder pins `[min, max]` accepted `wire_version` range; outside the range → typed `*_DECODE_FAIL{reason=wire_version_out_of_range}`. |

### 5.4 Generation seal locks the wire version

When `Seal { repo, revision, generation }` is observed, the search side records the wire version of every op in that generation in its per-generation ledger. A later op against a sealed generation with a different wire version is rejected with `STATE_GENERATION_REGRESSION` / `*_DECODE_FAIL` per [channel-architecture.md §5.2](channel-architecture.md).

---

## 6. Error Contract

Typed codes the search side raises against producer-emitted ops. Per [channel-architecture.md §3.3](channel-architecture.md), channel-level corruption surfaces as `ChannelError::Corrupted` (existing). Semantic-record errors are the codes below; they flow into the observability rail and (where relevant) mark the affected track degraded.

### 6.1 Symbol track

| Code | Cause | Site |
|---|---|---|
| `SYMBOL_PAYLOAD_DECODE_FAIL{at_seq, reason}` | CBOR decode of `UpsertSymbol.payload` fails (truncated / non-CBOR / type mismatch) | `crates/quanta-index-lexical/src/symbol/decode.rs` per [LEX-05 §8](../plans/may-24-lexical-indexing-sourcegraph/tickets/LEX-05.md). |
| `SYMBOL_RECORD_INVALID{at_seq, field}` | CBOR decode succeeds but a required field is missing / out of range (empty `name`, unknown `kind` enum, non-monotonic `span`, `lang` not in `LangId`, `wire_version` outside accepted range) | post-decode validation. |

### 6.2 History track

| Code | Cause | Site |
|---|---|---|
| `HISTORY_COMMIT_DECODE_FAIL{at_seq, reason}` | CBOR decode of `UpsertCommit.commit` fails | [LEX-07 §5.0](../plans/may-24-lexical-indexing-sourcegraph/tickets/LEX-07.md) decoder. |
| `HISTORY_COMMIT_PARENT_UNKNOWN{at_seq, sha}` | `commit.parents` references a sha not seen in the same generation window — violates §3.1.2 ordering | `CommitGraph::add_commit`. |
| `HISTORY_REF_DECODE_FAIL{at_seq, reason}` | `UpsertRef` / `UpsertTag` / `DeleteRef` / `DeleteTag` payload malformed | ref/tag decoder. |
| `HISTORY_REF_NOT_FOUND{name}` | `UpsertRef` / `UpsertTag` references an unknown sha (producer ordering violation per §3.1.2) | ref index. |

### 6.3 Structural track (Option A only)

| Code | Cause | Site |
|---|---|---|
| `STR_PARSE_TREE_DECODE_FAIL{at_seq, reason}` | `UpsertParseTree.tree` CBOR decode fails or `source_hash` mismatch | `quanta-index-structural` decode adapter per [STR-01 §3.1](../plans/may-24-lexical-indexing-sourcegraph/tickets/STR-01.md). |
| `STR_PRODUCER_PARSE_TREE_UNAVAILABLE` | structural query against a chunk for which no `ParseTreeRecord` was ever emitted; surfaced under Option B as the default failure of `match_pattern` | query path. |

### 6.4 Runtime / dirty track

| Code | Cause | Site |
|---|---|---|
| `DIRTY_PAYLOAD_DECODE_FAIL{at_seq, reason}` | `UpsertDirty` / `EvictDirty` payload malformed | dispatcher decode site per [RT-01 §5.5](../plans/may-24-lexical-indexing-sourcegraph/tickets/RT-01.md). |
| `DIRTY_STALE_GEN{op_gen, active_gen}` | `UpsertDirty` carries `generation < active_gen` — §3.2.4 violation | `DirtyBuffer::apply`. |
| `DIRTY_BUFFER_FULL` | per-tenant per-repo buffer at capacity ceiling; offending op dropped, existing entries preserved | `DirtyBuffer::apply` per [RT-01 §4.6](../plans/may-24-lexical-indexing-sourcegraph/tickets/RT-01.md). |
| `DIRTY_BAD_IDENTITY{doc_id}` | `UpsertDirty.doc_id` does not name a known chunk in the active generation — §3.2.6 enforced sync at apply | `DirtyBuffer::apply`. |
| `DIRTY_TTL_EXPIRED` | (search-side only; not a producer-facing code) sweep evicted a dirty entry past TTL | sweep worker. |

### 6.5 Semantic track

| Code | Cause | Site |
|---|---|---|
| retired | semantic-vector handle resolution was removed with the text-only semantic/hybrid public surface; search-side query embedding is now search-owned and no public handle lookup route remains | historical ADR note only; not part of the live query/error surface |

### 6.6 Channel-level (existing)

| Code | Cause | Site |
|---|---|---|
| `ChannelError::Corrupted{at_seq, reason}` | crc mismatch / non-monotonic seq / length-prefix beyond segment end | [channel-architecture.md §4.6](channel-architecture.md). On any `Corrupted`, the affected track is marked degraded; the daemon rejects further queries against that track with `NOT_READY`. |

### 6.7 No silent skip

Per [AGENT_CORE.md](../../AGENT_CORE.md) and [channel-architecture.md §4.6](channel-architecture.md): there is **no silent skip path** for any code above. Every rejected op surfaces a typed code on the observability rail, and either the channel cursor advances (semantic rejection) or the track is marked degraded (structural corruption). The producer is the single source of truth; correct behaviour is to republish a valid op.

---

## 7. Decision Matrix for AMB-PROD-1..11

| AMB-ID | Producer decision needed | Recommended default | Blocking ticket | This doc § |
|---|---|---|---|---|
| AMB-PROD-1 | Commit emission ordering (topological vs buffered) | Topological per-generation; parents always precede children | [LEX-07](../plans/may-24-lexical-indexing-sourcegraph/tickets/LEX-07.md) | §3.1.2 |
| AMB-PROD-2 | `CommitRecord` full wire shape | Locked here: `{sha, parents, applied_at_ms, author, committer, message, is_merge, tags}` | [LEX-07](../plans/may-24-lexical-indexing-sourcegraph/tickets/LEX-07.md) + contract pin | §3.1.1 |
| AMB-PROD-3 | `DeleteCommit` op — present? force-push handling | NOT in v1. Force-push → fresh generation | [LEX-07](../plans/may-24-lexical-indexing-sourcegraph/tickets/LEX-07.md) | §3.1.3 |
| AMB-PROD-4 | Diff hunk authorship — inline (X) vs separate op (Y) | **Option Y** — new `UpsertDiffHunk` op for streaming hygiene | [LEX-07](../plans/may-24-lexical-indexing-sourcegraph/tickets/LEX-07.md) | §3.1.4 |
| AMB-PROD-5 | `SymbolRecord` wire-shape versioning policy | Embedded `wire_version: u32` per record; producer ADR per revision; search-side pins `[min,max]` range; breaking-first cutover | [LEX-05](../plans/may-24-lexical-indexing-sourcegraph/tickets/LEX-05.md) | §3.4.4, §5 |
| AMB-PROD-6 | `ParseTreeRecord` wire-shape + version field policy | Embedded `wire_version: u32`; recursive `ParseNode` with `source_hash` integrity check | [STR-01](../plans/may-24-lexical-indexing-sourcegraph/tickets/STR-01.md) | §3.3.1, §5 |
| AMB-PROD-7 | Producer `UpsertDirty` emission cadence (per-edit / batched / debounce) | Producer chooses. Recommended default: per-edit with 100 ms debounce ceiling | [RT-01](../plans/may-24-lexical-indexing-sourcegraph/tickets/RT-01.md) | §3.2.2 |
| AMB-PROD-8 | `EvictDirty` vs `UpsertDirty` ordering at same doc_id | Channel seq monotonicity is authority; producer MUST emit new `UpsertDirty` at seq > any prior `EvictDirty` for the same doc | [RT-01](../plans/may-24-lexical-indexing-sourcegraph/tickets/RT-01.md) | §3.2.3 |
| AMB-PROD-9 | WAL retention horizon vs RT-01 TTL (300 s default) | Producer keeps segments alive for at least `max(subscriber_lag, dirty_ttl)`; default ≥ 300 s | [channel-architecture.md §4.3](channel-architecture.md) | §3.2.5 |
| AMB-PROD-10 | `DIRTY_BAD_IDENTITY` validation timing (sync vs async) | Sync at decode + apply in `DirtyBuffer::apply` | [RT-01 §8](../plans/may-24-lexical-indexing-sourcegraph/tickets/RT-01.md) | §3.2.6 |
| AMB-PROD-11 | Q-STR-01-OPTION — Option A (v1 with `UpsertParseTree`) vs Option B (v2 deferral) | Integrator decision at wave-5 entry; recommendation: Option A only if producer commits to shipping parse trees in time, else Option B with `STR_PRODUCER_PARSE_TREE_UNAVAILABLE` typed failure | [STR-01 §1.1](../plans/may-24-lexical-indexing-sourcegraph/tickets/STR-01.md) | §3.3 |

### 7.1 New ambiguity surfaced during authoring

- **AMB-PROD-12 (new): Diff hunk Option Y producer cost.** Switching to per-hunk ops increases op count by `~hunks_per_commit` (typical: 1–10). The 64-entry batched-fsync window in [channel-architecture.md §4.4](channel-architecture.md) absorbs this comfortably, but a producer with bursty large-diff commits should size its publish-side buffer accordingly. Producer ADR should record the choice. Not blocking; flagged for traceability.

---

## 8. Versioning + Integration Handshake Protocol

The cutover from "proposed" ([channel-architecture.md §3.1 status](channel-architecture.md)) to "shipped" follows a fixed handshake.

### 8.1 Handshake steps

| # | Step | Owner | Artefact |
|---|---|---|---|
| 1 | Producer signs off the wire shapes in §3 and the decisions in §7 | producer lead | sign-off on this doc + producer-side ADR per record |
| 2 | Producer commits to `wire_version = 1` for the new ops + amended `UpsertSymbol` shape | producer lead | producer-side release notes |
| 3 | Search side pins `[min_wire_version=1, max_wire_version=1]` consumers in [`quanta-index-contract::channel`](../../crates/quanta-index-contract/src/channel/ops.rs) | search lead | PR landing the contract pin |
| 4 | Joint test corpus: producer publishes a fixture stream covering every op in §3 against a known repo snapshot | producer lead | `tests/fixtures/producer-handoff-v1/*.wal` + `expected.json` |
| 5 | Search side runs the conformance corpus ([PRE-CONF](../plans/may-24-lexical-indexing-sourcegraph/tickets/PRE-CONF.md)) against the producer fixture stream | search lead | conformance run report (junit XML) |
| 6 | Both teams sign off the conformance result | both leads | mutual sign-off recorded in this doc + producer release notes |
| 7 | Cutover locks: producer flips `wire_version = 1` emission on; search side flips the new-op handlers on; old "proposed" status flips to "shipped" in [channel-architecture.md §3.1](channel-architecture.md) | both | coordinated release |

### 8.2 Failure modes during handshake

| Failure | Action |
|---|---|
| Producer fixture stream fails search-side decode | block cutover; producer fixes the encoder; restart step 4 |
| Search side rejects a record that the producer believes valid | escalate to wire-shape ADR; if shape is wrong, bump `wire_version` and restart |
| Conformance run misses a row | block cutover; add the row; rerun |
| Conformance run finds a perf regression on the search side | block cutover until the regression is investigated; perf is a search-side concern, but it can block cutover |

### 8.3 Post-cutover

Once cutover is locked:

- Producer emits `wire_version = 1` for every op in §3.
- Search side accepts `wire_version ∈ [1, 1]`.
- Any breaking change requires a fresh handshake at `wire_version = 2` per §5.
- Sealed generations under `wire_version = 1` persist in the search-side ledger until retention reaps them, regardless of later wire bumps.

---

## 9. Cross-References

### 9.1 Parent SSOT

- [channel-architecture.md](channel-architecture.md) — canonical SSOT. This doc extends [§3.1 op catalogue](channel-architecture.md) and references [§0 Scope](channel-architecture.md), [§4 Default backend](channel-architecture.md), [§5.2 Generation state authority](channel-architecture.md), [§11 No-Resurrection Rules](channel-architecture.md).

### 9.2 Corrected ticket specs (downstream consumers)

- [LEX-05.md](../plans/may-24-lexical-indexing-sourcegraph/tickets/LEX-05.md) — `SymbolRecord` decoder + symbol shard. Consumes §3.4.
- [LEX-07.md](../plans/may-24-lexical-indexing-sourcegraph/tickets/LEX-07.md) — commit / ref / tag callbacks + history sidecar. Consumes §3.1.
- [STR-01.md](../plans/may-24-lexical-indexing-sourcegraph/tickets/STR-01.md) — parse tree consumer (Option A) or scaffold-only (Option B). Consumes §3.3.
- [RT-01.md](../plans/may-24-lexical-indexing-sourcegraph/tickets/RT-01.md) — dirty buffer channel-subscriber callback. Consumes §3.2.

### 9.3 Index of corrections

- [INDEX.md §3.6](../plans/may-24-lexical-indexing-sourcegraph/tickets/INDEX.md) — producer-authorship correction table; this doc is the agreement artefact for the corrected specs.
- [INDEX.md §3.7](../plans/may-24-lexical-indexing-sourcegraph/tickets/INDEX.md) — the 11 `AMB-PROD-*` items resolved in §7 above.

### 9.4 Current shipped surface

- [crates/quanta-index-contract/src/channel/ops.rs](../../crates/quanta-index-contract/src/channel/ops.rs) — the current shipped `LexicalChannelOp` / `SemanticChannelOp` enum. New variants land here as part of the handshake (step 3, §8).

### 9.5 Agent posture and rules

- [AGENT_CORE.md](../../AGENT_CORE.md) — breaking-first, explicit authority, and no silent failure; [AGENT_RULE_CATALOG.md](../../AGENT_RULE_CATALOG.md) — serde derive ban.
- [../../AGENTS.md](../../AGENTS.md) — shared agent router.

### 9.6 Lint and CI

- [../../tools/ci/lint/lint-doc-paths.py](../../tools/ci/lint/lint-doc-paths.py) — doc-link linter; this doc must pass with 0 broken links.
- [../../tools/ci/semgrep/rules.yml](../../tools/ci/semgrep/rules.yml) — `rust-no-serde-derive` enforces D18 across the wire decoders that consume the shapes in §3.
- [../../tools/ci/agent/agent_output.schema.json](../../tools/ci/agent/agent_output.schema.json) — structured agent output schema; integration claims tied to this doc are evidence-bound per [AGENTS.md](../../AGENTS.md) Verification Contract.
