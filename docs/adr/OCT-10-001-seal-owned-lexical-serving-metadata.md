# OCT-10-001 — Seal-owned lexical serving metadata

Status: `Implemented; verification in progress`

Decided: 2026-10-10

## Problem

F15 source packs and Native text shards already reuse unchanged objects. Publication
and activation nevertheless read and reconstruct the complete source/posting
relation repeatedly. Faster mutation alone does not remove that cost.

## Decision

1. Sealed manifest format 16, file-authority root format 16 and Native manifest
   format 2 form one coupled contract. Earlier generations require a producer
   rebuild. There is one current reader and no legacy fallback.
2. The F16 root commits source body offsets, normalized content lengths and
   posting-page fences/digests. The producer writes these values; an independent
   seal verifier reconstructs source normalization and complete gram membership
   and compares the persisted metadata to decoded objects before identity publication.
3. A bucket inherits proof only from a non-quarantined sealed base whose bound
   root, complete source rows and three object descriptors match exactly, and
   whose pack/path/content objects are the same regular inodes at committed lengths.
   Changed buckets receive the full independent proof. Missing proof does not
   become successful empty verification.
4. Native manifests commit ordered document IDs, candidate IDs and indexed-text
   digests. At seal, changed shards are hashed/decoded and their four derived
   indexes are reconstructed from normalized document rows and compared. Exact
   sealed-base entries and same-inode/length checks authorize unchanged shards.
5. Open, publication and restart authenticate control metadata, complete inventories,
   coverage and regular-file lengths. They retain metadata and pinned identities,
   not source bodies. Selected F16 ranges and Native shards authenticate their
   owned read buffers before decoding or matching. Full semantic verification
   remains the door-finding oracle; the bounded resumable scrub hashes all sealed
   bytes and durably quarantines mismatches.
6. This deliberately moves detection of unchanged, same-length leaf corruption
   from open to selected read or scrub. Missing, truncated, redirected, orphaned
   and corrupt control metadata still refuse at the metadata door. Content from
   the live working tree never repairs a sealed generation.

## Trust and limits

The trust boundary is the existing local seal/identity construction, also used
by Tantivy's content-at-seal-and-scrub policy. The manifest digest supplied by
the producer is not an externally signed cryptographic anchor. This decision
does not defend against an actor rewriting the complete sealed manifest, identity
and all commitments. It does not treat inode/mtime as a fresh content hash.
Inheritance establishes object provenance from a previously sealed generation;
it does not prove that writable bytes remained unchanged. A same-length in-place
overwrite can therefore be inherited and will be refused when the affected range
is selected or the object is scrubbed.

F16 range readers depend on the generation directory while a query handle is
held. The existing snapshot registry fences retirement and defers physical
reclaim until those handles are released. Direct physical reclaim while a query
handle is pinned violates the reclaim port's caller contract.

The F16 root remains flat, with 256 source-key buckets. Its 16 MiB encoded root,
32,768 files, 128 MiB source corpus and other admission ceilings remain explicit.
Persisted directory metadata consumes root capacity and retained heap. Metadata
walks remain proportional to corpus size, and a changed bucket can contain many
unchanged files. Paged routing, shared-object GC and sustained compaction are
separate work; this change does not establish worst-case O(delta) publication.

## Verification

Required oracles cover full versus delta/no-op/tombstone/reopen, forged metadata
and derived postings, exact inherited object identities, selected-read refusal,
control-file refusal, scrub quarantine and interrupted publication. Executed
commands and their current scope are recorded in
[the implementation ledger](../plans/oct-10-index-closeout/LEXICAL-QUERY-PLAN.md#101-이전-f16-검색-핵심-경로-구현과-검증).
No XL latency improvement or competitor ranking follows from compilation or tests.
