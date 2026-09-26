# Local Tantivy SSTable admission extension

Base: the crates.io `tantivy-sstable` 0.3.0 release used by Tantivy 0.22.1.
`quanta-provenance.json` records original file digests. Root Cargo.toml patches
that exact package to this directory; the SSTable wire format is unchanged.

`Dictionary<VoidSSTable>::ord_to_term_budgeted` admits three allocations through
the caller's reservation callback: encoded block IO, decode workspace, and the
exact retained output. The returned guard accompanies the output; temporary
guards remain live through decoding and output allocation. Absent ordinals do
not allocate. Refused reservations propagate as errors before the operation.

The bounds live in the codec, not in the lexical collector. Encoded block sizes
bound IO; uncompressed lengths or zstd's `decompressBound` bound decode buffers;
the sum of decoded block bytes bounds reconstructed key suffixes. Unknown,
overflowing and malformed bounds are rejected. The bulk decoder's argument-free
`ZSTD_estimateDCtxSize` query bounds its one nonstreaming context before creation.
No persisted path/name length limit or new serialized format is introduced.

Checked key-delta varints and suffix lengths reject truncated/overflowing data
instead of indexing outside the block. This changes failure behavior for
malformed input, while retaining valid-key ordering and serialization. Explicit
lifetimes in two signatures silence warnings newly visible with a path package.

The byte contract covers requested logical buffer capacity and the bulk zstd
context. It excludes allocator metadata, allocator realloc internals, index-open
and query preparation allocations, caches, output payloads outside the returned
key, and process RSS. This API assumes an already opened dictionary index; it
does not claim to harden every upstream index parser against arbitrary bytes.

Regression coverage is in `quanta-index-lexical` L3 tests: denied reservation at
each stage, retained-guard lifetime, compressed predecessor workspace, native
collector typed refusal, and malformed key deltas. See the L3 handoff for exact
commands, input snapshots, results and remaining verification limits.

Keep this patch narrow when upgrading Tantivy. Revalidate the bulk decoder's
context/buffer contracts and Rust Vec capacity assumptions at that time. The
original package's MIT license and project authors are included here.
