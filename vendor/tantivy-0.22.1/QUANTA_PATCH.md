# Quanta Index Tantivy patch

- Upstream crate: `tantivy` 0.22.1, <https://crates.io/crates/tantivy/0.22.1>.
- Registry archive SHA-256: `96599ea6fccd844fc833fed21d2eecac2e6a7c1afd9e044057391d78b1feb141` (the pre-patch `Cargo.lock` checksum).
- License: MIT; the upstream `LICENSE` file is preserved here.
- Local change: `src/indexer/merger.rs` counts surviving posting frequencies for deleted segments of string fields indexed with frequencies. It preserves the upstream no-delete shortcut and the upstream Basic/JSON approximation.
- Reason: upstream reconstructs a deleted segment's `total_num_tokens` from quantized fieldnorms, which changes BM25 collection averages after a lexical delta or tombstone even when the final live source is identical to a fresh rebuild.
- Qualification: the focused upstream regression and the Quanta Index lexical lifecycle regression must pass under the patched dependency. The extra streaming pass on stale segments requires seal-time cost measurement at scale.

This copy remains pinned to 0.22.1. Review this patch and the on-disk lexical manifest format when upgrading Tantivy.
