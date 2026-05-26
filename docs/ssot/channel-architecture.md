# quanta-index Channel Architecture — Historical Pre-De-channelize SSOT

Status: `Historical design doc. Not current-tree authority after the 2026-05-27 de-channelize cutover.`

Current tree truth:

- `searchd-runtime` no longer runs subscriber replay loops in the hot path
- `ChannelDispatcher` is not the runtime authority path
- readiness and live materialization are driven by direct authority apply plus
  persisted authority stores
- the standalone `quanta-index-channel` crate has been retired from the
  workspace; remaining channel types live under
  `quanta_index_contract::channel::*` and historical prose here is archive-only
- use live source in `crates/quanta-index-searchd/src/app/runtime.rs`,
  `crates/quanta-index-searchd-runtime/src/lib.rs`, and
  `crates/quanta-index-search-plane/src/{ingest_dispatcher,readiness}.rs`
  as the current authority instead of this document

## 0. Scope (narrow)

This repo implements **lexical and semantic index build + open + query**, consumed by a single `searchd` daemon. Bundle and delta delivery from producer (`semantica-codegraph-v2`) flow through an **abstract channel interface** whose default backend is **WAL append + mmap read tail**.

In-scope:

1. lexical index build, open, query
2. semantic index build, open, query
3. hybrid query = lexical + semantic merge via RRF
4. consume producer-written channel (bundle + delta + seal)

Out-of-scope (lives in producer repo `semantica-codegraph-v2`):

- bundle creation, manifest authorship
- generation number assignment
- producer-side prepare / finalize intent
- delta authorship

## 1. Abstraction Principle

> Transport (WAL, mmap, future variants) lives **inside exactly one adapter crate**. Producer code and `searchd` domain code never see WAL, mmap, segment, fsync, or any transport-specific token. They only depend on two traits: `BundleChannelPublisher` and `BundleChannelSubscriber`.

If a future swap replaces WAL+mmap with `io_uring`, `shm-queue`, or `tokio::mpsc` (in-process tests), the following code must change:

| Layer | Change |
|-------|--------|
| Producer code | **0 lines** |
| Searchd domain code | **0 lines** |
| Composition root (`searchd::app`) | 1 line (factory call) |
| Tests using `MockSubscriber` | **0 lines** |

This invariant is the contract.

## 2. Architecture

```text
producer (semantica-codegraph-v2)
  │
  │ 1. publish(ChannelOp)
  ▼
┌──── lexical channel ────┐    ┌──── semantic channel ────┐
│  (transport: WAL + mmap) │    │  (transport: WAL + mmap)  │
└─────────────────────────┘    └──────────────────────────┘
  │                              │
  │ 2. next_event()              │ next_event()
  ▼                              ▼
┌───────────────── searchd (single process) ─────────────────┐
│                                                            │
│   ┌── ChannelDispatcher ──┐                                │
│   │  routes by track       │                                │
│   └────────────┬───────────┘                                │
│                ▼                                             │
│   ┌── lexical module ──┐   ┌── semantic module ──┐          │
│   │ Tantivy adapter     │   │ Lance adapter        │          │
│   │ in-memory readiness │   │ in-memory readiness  │          │
│   └─────────┬──────────┘   └─────────┬───────────┘          │
│             │                          │                     │
│             └────── HybridOrchestrator (RRF) ──┐             │
│                                                ▼             │
│                                         app::query           │
└────────────────────────┬───────────────────────────────────┘
                         │ 3. UDS frame (CBOR envelope)
                         ▼
                  query client
```

Single daemon per `{state_root}`. Single UDS socket per daemon. lexical and semantic are sibling modules inside one process — they communicate only through the daemon's composition root, never directly.

## 3. Channel Abstraction

### 3.1 Traits (in `quanta-index-channel::api`)

```rust
pub trait BundleChannelPublisher: Send + Sync {
    type Op;
    fn publish(&self, op: Self::Op) -> Result<ChannelSeq, ChannelError>;
    fn seal(&self, repo: RepoId, revision: RevisionId, generation: ManifestGeneration)
        -> Result<ChannelSeq, ChannelError>;
    fn flush(&self) -> Result<(), ChannelError>;
}

pub trait BundleChannelSubscriber: Send {
    type Event;
    fn next_event(&mut self) -> Result<Option<Self::Event>, ChannelError>;
    fn ack(&mut self, up_to: ChannelSeq) -> Result<(), ChannelError>;
    fn cursor(&self) -> ChannelSeq;
}
```

Op enums are split by track to make wrong-track publish a compile error:

```rust
pub enum LexicalChannelOp {
    FullBundle    { repo, revision, generation, payload: LexicalBundlePayload },
    UpsertChunk   { repo, revision, generation, chunk: ChunkRecord },
    DeleteChunk   { repo, revision, generation, chunk_id },
    UpsertSymbol  { repo, revision, generation, symbol: SymbolRecord },
    DeleteSymbol  { repo, revision, generation, symbol_id },
    // ── History track (LEX-07) — producer ships commit DAG metadata.
    UpsertCommit  { repo, revision, generation, commit: CommitRecord },
    UpsertRef     { repo, revision, generation, name: Box<str>, sha: CommitSha },
    UpsertTag     { repo, revision, generation, name: Box<str>, sha: CommitSha },
    DeleteRef     { repo, revision, generation, name: Box<str> },
    DeleteTag     { repo, revision, generation, name: Box<str> },
    // ── Runtime track (RT-01) — producer marks docs dirty/clean.
    UpsertDirty   { repo, revision, generation, doc_id, applied_at_ms, payload_hash: [u8;32] },
    EvictDirty    { repo, revision, generation, doc_id },
    // ── Structural track (STR-01) — producer ships pre-parsed trees per chunk.
    // Optional; only emitted when STR-01 is wired in. Search plane consumes
    // CBOR-encoded parse trees, never parses source on the query path.
    UpsertParseTree { repo, revision, generation, chunk_id, tree: ParseTreeRecord },
    DeleteParseTree { repo, revision, generation, chunk_id },
    Seal          { repo, revision, generation },
}

pub enum SemanticChannelOp {
    FullBundle    { repo, revision, generation, payload: SemanticBundlePayload },
    UpsertEmbedding { repo, revision, generation, embedding: EmbeddingRecord },
    DeleteEmbedding { repo, revision, generation, embedding_id },
    Seal          { repo, revision, generation },
}
```

**Authorship rule (locked):** every payload carried by these ops — `ChunkRecord`,
`SymbolRecord` (including `kind`, `name`, `span`, `lang`), `CommitRecord`
(including `parents`, `applied_at_ms`), `ParseTreeRecord`, `EmbeddingRecord` —
is **authored by the producer** in `semantica-codegraph-v2`. Search plane never
parses source bytes, never walks git, never computes embeddings. It decodes
producer-supplied records and indexes them. This is the structural inverse of
the "search engine does its own extraction" pattern in tools like Sourcegraph
Zoekt or Elasticsearch: here, extraction lives upstream so the search plane is
a pure index + query plane.

Status of the op set above:
- Shipped (in `quanta-index-contract::channel`): `FullBundle`, `UpsertChunk`,
  `DeleteChunk`, `UpsertSymbol`, `DeleteSymbol`, `Seal`,
  `UpsertEmbedding`, `DeleteEmbedding`.
- **Proposed, pending producer agreement**: `UpsertCommit`, `UpsertRef`,
  `UpsertTag`, `DeleteRef`, `DeleteTag`, `UpsertDirty`, `EvictDirty`,
  `UpsertParseTree`, `DeleteParseTree`. The corresponding ticket specs
  (LEX-07, RT-01, STR-01) reference these as their authoritative input
  surface; integration cutover blocks on the producer side accepting these
  op shapes.

Delta-handling semantics for every `Upsert*` / `Delete*` op above — identity
rules per record type, cascade graph on delete, replay safety, in-generation
last-write-wins, cross-generation `FullBundle` vs `from_prior(...)` delta
modes, and topological ordering for the history track — are locked in
[producer-handoff.md §3.5](producer-handoff.md). The op enum here is the wire
catalogue; §3.5 is the authoritative producer emission contract that
search-side builders are idempotent under.

### 3.2 Factory surface (in `quanta-index-channel`)

```rust
pub fn open_lexical_publisher(state_root: &Path)
    -> Result<impl BundleChannelPublisher<Op = LexicalChannelOp>, ChannelError>;

pub fn open_lexical_subscriber(state_root: &Path)
    -> Result<impl BundleChannelSubscriber<Event = LexicalChannelEvent>, ChannelError>;

pub fn open_semantic_publisher(state_root: &Path)
    -> Result<impl BundleChannelPublisher<Op = SemanticChannelOp>, ChannelError>;

pub fn open_semantic_subscriber(state_root: &Path)
    -> Result<impl BundleChannelSubscriber<Event = SemanticChannelEvent>, ChannelError>;
```

These factories return the default backend (`wal_mmap`). Backend selection may later be feature-gated.

### 3.3 Error surface

```rust
pub enum ChannelError {
    Closed,
    Corrupted { at_seq: ChannelSeq, reason: String },
    BackpressureFull,
    NotReady,
    Io(std::io::Error),
    Encoding(String),
    State(String),
}
```

No `SegmentFull`, `MmapRemap`, or other backend-specific variant. Backend-leaky errors map into `State(...)` or `Io(...)`.

## 4. Default Backend: WAL + mmap

`quanta-index-channel::backends::wal_mmap`. Hidden from callers.

### 4.1 On-disk layout

```text
{state_root}/channel/
  lexical/
    log.wal.{seg_id}        # append-only segments, producer-written
    cursor                  # subscriber's consumed seq, fsync-owned by subscriber
    publisher.lock          # advisory lock, prevents double-publisher per track
  semantic/
    log.wal.{seg_id}
    cursor
    publisher.lock
```

### 4.2 Frame format (per entry)

```text
[ u32 LE length ]                  # length of body, excluding header
[ u64 LE seq ]                     # monotonic, strictly increasing per track
[ u32 LE op_tag ]                  # discriminator
[ bytes body ]                     # CBOR-encoded op-specific payload
[ u32 LE crc32 ]                   # crc32 over { seq, op_tag, body }
```

Max single entry: 16 MiB. Larger payloads (e.g. FullBundle) are split into multiple ops by the producer.

### 4.3 Segment rotation

Rotate to a new `log.wal.{seg_id+1}` when either:

1. current segment exceeds 64 MiB, or
2. current segment exceeds 10,000 entries

Segment GC: subscriber writes `cursor`; subscriber emits an ack file `cursor.consumed` after each rotation boundary; publisher deletes segments older than the consumed boundary on next rotate.

### 4.4 Fsync policy

| Event | Fsync? |
|-------|--------|
| `Seal` op | yes, before publish returns |
| `flush()` call | yes |
| Other ops | batched, fsync at most every 64 entries or every 100 ms |
| Segment rotate | yes (close old segment, rename new) |

### 4.5 Subscriber polling

- `next_event()` reads frames sequentially from current segment, advances cursor in-memory
- On EOF, stat current segment; if size grew, read more; if next segment exists, advance
- Reader uses `mmap` for the active read window (read-only, MAP_SHARED)
- `ack(seq)` writes `seq` to cursor file with fsync

### 4.6 Corruption policy (fail-closed)

1. crc mismatch → `ChannelError::Corrupted`, subscriber does not skip the entry
2. length-prefix beyond segment end → same
3. seq non-monotonic → same
4. on `Corrupted`, the caller (`ChannelDispatcher`) treats the affected track as **degraded** and rejects future queries against that track with `NOT_READY`

### 4.7 Crash recovery

- subscriber restart: read `cursor`, resume from that seq
- publisher restart: scan tail of latest segment, find max valid seq, set next_seq = max + 1
- partial frame at tail (length prefix written, body truncated): publisher truncates back to last valid frame before appending

## 5. Searchd Internals

### 5.1 Domain layout (in `quanta-index-core::domains`)

```text
domains/
  channel/
    inbound.rs       # ChannelEventHandler trait (driving, used by dispatcher)
    outbound.rs      # (none — channel subscriber is a driving adapter)
    service.rs       # ChannelPolicy: seq monotonicity, generation-scope validation
  lexical/
    inbound.rs       # LexicalQueryPort, LexicalChannelSink
    outbound.rs      # LexicalIndexBuildPort, LexicalIndexOpenPort
    service.rs       # LexicalPolicy: generation pin, readiness gating
  semantic/
    inbound.rs       # SemanticQueryPort, SemanticChannelSink
    outbound.rs      # SemanticIndexBuildPort, SemanticIndexOpenPort
    service.rs       # SemanticPolicy
  hybrid/
    inbound.rs       # HybridQueryPort, ExplainQueryPort
    service.rs       # RRF merge, generation-coherence enforcement
```

Cross-domain rules (hex-lint enforced):

1. `domains::lexical` cannot import `domains::semantic` or vice versa
2. `domains::hybrid` may import **types** from lexical/semantic outbound traits only (not service internals)
3. all four domains import only `contract::*` for external data shapes

### 5.2 Generation state authority

| Authority | Owner |
|-----------|-------|
| Sealed generations (per track) | Reconstructed from channel events on startup, kept in `LexicalGenerationLedger` / `SemanticGenerationLedger` (in-memory) |
| Materialized (per track) | Per-track ledger flips when `Seal` op is observed AND build completes successfully |
| Active (joint) | Hybrid query coordinator: max generation N such that both lex and sem have `materialized=true` for N |
| Query pin | Per-request in-memory pin |

No SQLite. No `quanta-index-control` crate. Recovery cost on restart = re-read channel from last consumed cursor.

### 5.3 Dispatcher loop

```rust
loop {
    select! {
        evt = lex_sub.next_event() => {
            let evt = evt?;
            channel_policy.validate(&evt)?;
            lex_module.apply(evt)?;
            lex_sub.ack(evt.seq)?;
        }
        evt = sem_sub.next_event() => {
            let evt = evt?;
            channel_policy.validate(&evt)?;
            sem_module.apply(evt)?;
            sem_sub.ack(evt.seq)?;
        }
    }
}
```

On `Seal`: module triggers build/open of that generation. On build success: ledger marks `materialized=true`.

### 5.4 Query path (split surface)

- UDS socket `{state_root}/search-plane/query.sock`
- Frame: `[u32 LE length][CBOR body]` carrying `SearchPlaneQueryIpcRequestEnvelope` / `SearchPlaneQueryIpcResponseEnvelope`
- Hybrid query rejects when only one of lex/sem is `materialized` for the requested generation → `NOT_READY`

## 6. Crate Layout

| Crate | Role | Status |
|-------|------|--------|
| `quanta-index-contract` | DTOs only: ids, query, results, ipc, **channel** | revised — bundle/control DTOs removed |
| `quanta-index-channel` | Channel api + WAL+mmap backend | **new** |
| `quanta-index-core` | Domains: channel, lexical, semantic, hybrid + policies | restructured |
| `quanta-index-lexical` | Tantivy adapter, retained | minor rewire |
| `quanta-index-semantic` | Lance adapter, retained | minor rewire |
| `quanta-index-ipc` | UDS / CBOR codec for query envelopes | unchanged surface |
| `quanta-index-searchd` | Composition root, dispatcher, hybrid orchestrator | substantial rewire |
| ~~`quanta-index-control`~~ | — | **deleted** |

## 7. Crate Dependency Matrix (hex-lint)

```text
contract  ◄── (none)
channel   ◄── contract
core      ◄── contract
lexical   ◄── contract, core
semantic  ◄── contract, core
ipc       ◄── contract, core
searchd   ◄── contract, core, channel, lexical, semantic, ipc

producer (cross-repo) ◄── contract, channel  (api+factory only)
```

`lexical` and `semantic` may NOT depend on each other (lint enforced).
`channel::backends::*` may NOT be imported by anything other than `channel::api` (lint enforced).
`channel::api` may NOT import any `channel::backends::*` (lint enforced).

## 8. Runtime Layout

```text
{state_root}/
  channel/
    lexical/
      log.wal.{seg_id}
      cursor
      publisher.lock
    semantic/
      log.wal.{seg_id}
      cursor
      publisher.lock
  search-plane/
    query.sock
    control.sock
  indexes/
    lexical/{repo_id}/{revision_id}/g{manifest_generation}/
    semantic/{repo_id}/{revision_id}/g{manifest_generation}/
```

`bundles/` and `control-plane.sqlite3` are removed.

## 9. Phase Plan

| Phase | Deliverable | Verification gate |
|-------|-------------|-------------------|
| **P0** | `contract::channel` DTOs + `channel::api` traits | `cargo check --workspace`, doc tests |
| **P1** | `channel::backends::wal_mmap` publisher (write, fsync policy, segment rotate) | property tests on frame codec + rotate |
| **P2** | `channel::backends::wal_mmap` subscriber (mmap tail, cursor persistence, crash recovery) | property tests on replay + corruption |
| **P3** | Publisher↔subscriber round-trip integration | seq monotonicity, seal-after-delta, backpressure |
| **P4** | `domains::channel` + `domains::lexical` + `LexicalGenerationLedger` rewire | lexical-only e2e |
| **P5** | `domains::semantic` + `SemanticGenerationLedger` rewire | semantic-only e2e |
| **P6** | `domains::hybrid` + UDS + searchd composition | hybrid e2e, scenario matrix |
| **P7** | Delete `quanta-index-control`, clean up workspace | `cargo machete`, hex-lint green |

## 10. Scenario Matrix

### Usecase

| ID | Given | When | Then |
|----|-------|------|------|
| `U-CH1` | producer writes FullBundle + N Upserts + Seal for generation G | subscriber reads sequentially, build succeeds | track ledger marks G `materialized`, hybrid activation re-evaluates |
| `U-CH2` | both lex and sem have G materialized | client sends hybrid query for G | RRF-fused candidates returned |
| `U-CH3` | searchd restarts after consuming up to seq N | restart with persistent cursor at N | resumes from N+1, no duplicate apply |
| `U-CH4` | producer prepares G+1 while G is active | events stream to subscribers | queries on G unaffected until both tracks seal G+1 |

### Edge

| ID | Given | When | Then |
|----|-------|------|------|
| `E-CH1` | publisher tries to publish wrong-track op | compile time | type error |
| `E-CH2` | duplicate Seal for same (repo, revision, generation) | second seal arrives | rejected by `ChannelPolicy`, idempotent ack |
| `E-CH3` | seq non-monotonic in channel | subscriber reads | `ChannelError::Corrupted`, track marked degraded |

### Corner

| ID | Given | When | Then |
|----|-------|------|------|
| `C-CH1` | no Seal yet for any generation | hybrid query arrives | `NOT_READY` |
| `C-CH2` | lex sealed for G, sem not yet | hybrid query for G | `NOT_READY` (no partial serve) |
| `C-CH3` | lex sealed for G, sem not yet | lexical-only query for G | served |

### Hellgate

| ID | Given | When | Then |
|----|-------|------|------|
| `H-CH1` | crc mismatch mid-segment | subscriber reads | track degraded, queries fail closed |
| `H-CH2` | publisher and subscriber on different machines (shared FS) | concurrent access | publisher.lock prevents double-publisher; subscriber tail works |
| `H-CH3` | segment grows past max | producer attempts publish | rotate succeeds before append; if disk full, `BackpressureFull` |

## 11. No-Resurrection Rules

1. SQLite control-plane is removed — must not return
2. `bundle/`, `control-plane.sqlite3` paths removed — must not return
3. `lexical` and `semantic` crates must not depend on each other
4. `channel::backends::*` must not be referenced outside `channel` crate
5. Backend-specific tokens (`wal`, `mmap`, `segment`, `fsync`, `crc32`) must not appear in `core`, `lexical`, `semantic`, `ipc`, `searchd`, or producer code — semgrep enforced
6. `BundleChannelPublisher::publish` is the only producer→searchd ingress path
7. `unimplemented!()`, `todo!()`, `panic!()`, `unwrap()`, `expect()` remain banned workspace-wide
8. `#[derive(Serialize, Deserialize)]` remains banned — manual impls only

## 12. Abstraction Audit Checklist

When reviewing any PR touching channel or domains:

1. Does the diff introduce any new transport token in non-backend code? (search: `wal`, `mmap`, `segment`, `fsync`, `crc`)
2. Does any domain code import `channel::backends::*`?
3. Does any error variant leak backend internals?
4. Does any trait method on `BundleChannelPublisher` / `BundleChannelSubscriber` take or return a backend-specific type?
5. Could the diff be cleanly replayed against a `MockBackend` for unit testing?

If any answer points the wrong way, the abstraction is leaking.

## 13. Done Definition

1. Producer in `semantica-codegraph-v2` consumes `quanta-index-channel::open_*_publisher` only — no other ingress
2. `searchd` domain code consumes `quanta-index-channel::open_*_subscriber` only
3. `quanta-index-control` crate is deleted from the workspace
4. Hexagonal boundary lint updated and green for new domain/crate set
5. All channel events durably persisted; crash-restart resumes from cursor
6. lexical-only, semantic-only, and hybrid query end-to-end green
7. U/E/C/H-CH scenario matrix green
8. Abstraction audit checklist passes for every PR touching this code
