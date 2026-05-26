# Tickets — Search-owned Semantic Derivation

Parent doc: [../README.md](../README.md)

This ticket pack turns semantic ownership inside out:

- external producer authors raw records
- `quanta-index` derives corpus embeddings
- `quanta-index` embeds query text
- public semantic query surfaces become text-first

Current tree truth before this pack starts:

- public semantic/hybrid query paths are already text-only
- `searchctl` already rejects the old vector/handle flags
- SDK public semantic publish is already removed
- public ingest IPC no longer exposes semantic publish/receipt variants
- remaining work is internal semantic ownership, manifest/readiness, and final
  observability/proof closure

---

## 1. Execution order

| Wave | Ticket | Title | Why first |
|---|---|---|---|
| 0 | [SEM-OWN-00.md](SEM-OWN-00.md) | Semantic ownership inversion and boundary freeze | prevents mixed assumptions across contract, docs, and runtime |
| 1 | [SEM-OWN-01.md](SEM-OWN-01.md) | Raw ingest contract and chunk-text authority | semantic derivation has no stable input without this |
| 2 | [SEM-OWN-02.md](SEM-OWN-02.md) | Search-owned corpus embedding derivation worker | creates the actual semantic corpus |
| 3 | [SEM-OWN-03.md](SEM-OWN-03.md) | Semantic manifest, readiness, and seal gating | makes derived semantic generations query-safe |
| 4 | [SEM-OWN-04.md](SEM-OWN-04.md) | Manifest-guided query text embedding and cache contract | aligns internal query embedding with the already-text-only public surface |
| 5 | [SEM-OWN-05.md](SEM-OWN-05.md) | Legacy vector ingress removal, observability, and final proof | removes old seam and closes proof rails |

## 2. Ticket summary

| Ticket | Primary surface | Main output | Blocking deps |
|---|---|---|---|
| `SEM-OWN-00` | docs + contract boundary | one canonical ownership model | none |
| `SEM-OWN-01` | `quanta-index-contract`, producer ingest | `ChunkRecord { text }` | `SEM-OWN-00` |
| `SEM-OWN-02` | `searchd`, semantic channel, embedder client | derivation worker + job lifecycle + render policy hash | `SEM-OWN-01` |
| `SEM-OWN-03` | manifest + readiness + activation | manifest freeze + seal proof + typed blocked reasons | `SEM-OWN-02` |
| `SEM-OWN-04` | query IPC + dispatcher | manifest-guided internal query embedding + normalization/cache contract | `SEM-OWN-03` |
| `SEM-OWN-05` | cleanup, metrics, proof | observability/final-proof closure after legacy ingress removal | `SEM-OWN-04` |

## 3. Program exit criteria

The program is complete only when all are true:

1. producer no longer needs to publish embeddings
2. chunk text is the canonical external semantic source input
3. every active semantic generation has an internal manifest with provider/model/dim
4. every active semantic generation is fingerprinted by a stable render policy hash
5. semantic seal carries a completeness proof
6. semantic blocked generations expose typed blocked reasons
7. query embeddings are cached against normalized text + manifest identity
8. stable chunk identity is either guaranteed or semantic derivation is
   explicitly full-generation-only
9. semantic and hybrid public queries work from plain text without caller-side embedding
10. chunk delete cascades to semantic delete under `embedding_id == chunk_id`
11. no public semantic/hybrid happy path requires caller-authored vectors
12. restart/replay preserves semantic readiness correctness
13. activation remains fail-closed when semantic derivation is incomplete

## 4. PR slicing guidance

Recommended PR units:

1. `SEM-OWN-00` + `SEM-OWN-01` if the contract break is kept small
2. `SEM-OWN-02`
3. `SEM-OWN-03`
4. `SEM-OWN-04`
5. `SEM-OWN-05`

Do not merge:

- public query text cutover before manifest-guided query embedding exists
- worker before chunk-text authority is frozen
- cleanup before restart/readiness proof is green

Current tree note:

- the public query text cutover is already landed; `SEM-OWN-04` only owns the
  manifest-guided internal query embedding and cache/normalization contract

## 5. Explicitly rejected alternatives

- keep producer-authored embeddings and only add query-time embedding in `searchd`
- make symbols the primary semantic corpus unit
- keep public vector requests as the main semantic API forever
- call the embedder inline in the existing lexical channel-dispatcher ack path
- allow one generation to mix multiple embedding models or dimensions
