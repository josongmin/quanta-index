# G0-L — Tantivy snapshot reuse

**Status: PASS.** Lexical generations may be materialized by hard-linking a base
generation's files instead of byte-copying the directory.

- Decided: 2026-09-16
- Gate owner: W0, blocking W3 lexical lane and QI-BB-006 (lexical half)
- Probe: [`crates/quanta-index-lexical/tests/g0l_tantivy_snapshot_probe.rs`](../../../../crates/quanta-index-lexical/tests/g0l_tantivy_snapshot_probe.rs)
- Command: `just rust-w0-storage-gates`, or
  `./scripts/cargow --lane test-integration-lane test --all-features --locked -p quanta-index-lexical --test g0l_tantivy_snapshot_probe -- --nocapture`
- Vendor: `tantivy = "0.22"` (workspace pin), resolved `tantivy 0.22.1`
- Toolchain: rustc 1.92.0, darwin aarch64

## Question

The plan may only move the lexical lane off `copy_generation_directory` if
Tantivy itself guarantees the properties reuse depends on. Four questions, each
answered against a real index rather than from documentation.

## Evidence

Raw probe output (`G0L-EVIDENCE` lines, 2026-09-16):

```
G0L-EVIDENCE immutability files_before=10 files_after=16 retained_segment_files=6 rewritten_segment_files=0 introduced_files=6
G0L-EVIDENCE hardlink_delta base_total_bytes=55595 linked_bytes=55595 delta_total_bytes=57508 delta_fresh_bytes=2955 delta_fresh_files=9 delta_files_shared_with_base=8 delta_fresh_ratio_pct=5
G0L-EVIDENCE pinned_reader pinned_num_docs=2000 pinned_num_docs_after_gc=2000 gc_deleted_files=0
G0L-EVIDENCE incremental_vs_full_rebuild delta_num_docs=2000 oracle_num_docs=2000 ranked_rows=25 max_abs_score_delta=0.000000 ranking_identical=true
```

| Question | Result |
| --- | --- |
| Are committed segment files immutable? | **Yes.** 6 segment files survived a second commit with identical inode, length and SHA-256; 0 rewritten. Only `meta.json` and `.managed.json` are rewritten — both are Tantivy's own bookkeeping, never referenced as segment data. |
| Can a hard-linked base take a delta without touching base bytes? | **Yes.** g2 hard-linked all 55,595 base bytes, then deleted one document and added one. 8 of g2's files remained hard links into g1; g2 wrote **2,955 fresh bytes (5% of the base)**. Every g1 file was byte-identical afterwards, g1 still served the deleted document, and g1's ranking was unchanged. |
| Does a pinned searcher survive delete + commit + GC? | **Yes**, with a caveat — see Limitations. The searcher pinned before the mutation kept all 2,000 documents and an identical top-10 ranking. |
| Does incremental match an independent full rebuild? | **Yes.** Hard-linked base + delta vs a single build of the identical final corpus: same document count, identical top-25 ranking, `max_abs_score_delta = 0.000000`. |

## Decision

1. W3 may replace `copy_generation_directory` with hard-link materialization for
   the lexical lane. QI-BB-006's lexical half is unblocked.
2. Reuse must treat `meta.json` and `.managed.json` as generation-local: they
   are copied, never linked, because Tantivy rewrites them in place.
3. The incremental commitment obligation (DA-06) is dischargeable for this lane:
   the probe's rebuild oracle is the pattern W3's tests must follow.

## Rejected alternatives

- **Custom `Directory` unioning a read-only base with a writable delta.** Not
  probed and not needed: hard links deliver O(delta) write cost while leaving
  each generation a standalone, independently openable Tantivy index. Choosing
  the custom layout would require its own gate.
- **Keeping the full copy.** Measured at 100% of base bytes per delta against
  5% for links, with no correctness benefit.

## Limitations

- `gc_deleted_files=0`: the GC arm ran but the merge policy had produced no
  garbage, so the probe did **not** demonstrate a pinned reader surviving actual
  file deletion. The pin-vs-GC interaction is therefore only partially evidenced
  and W3 must add a case that forces real segment deletion. This does not affect
  the reuse decision, which rests on immutability plus the hard-link result.
- Single-host macOS APFS. Hard links are POSIX; a deployment on a filesystem
  without them (or across devices) would need `copy_file_range`/reflink or a
  fallback, and that choice is W3's to record.
- Corpus is 2,000 synthetic documents. The 5% figure is a shape, not a
  production ratio.
