# Quanta Lance 7.0.0 patch

Upstream package: crates.io `lance = 7.0.0`, Apache-2.0. Published source commit: `a15ae30939b9242d74b00aed1fb83abf7d15bf7f`, repository path `rust/lance`. The original `.cargo_vcs_info.json` and package files are retained.

## IVF reader custody

`src/index/vector/ivf/v2.rs` does not store `CachedIndexReaders` in the metadata cache. The cache backend otherwise owns an open `FileReader` whose `LanceCache` retains that same backend, creating a strong reference cycle. With admitted local I/O this retains the actual reader FD lease after its dataset/session is released.

Parsed `CachedFileMetadata`, index state, full index and partition caches remain. Reconstruction opens the original index and auxiliary readers through `open_reader_cached`, using cached footer metadata when available. The cost is two reader opens per IVF state reconstruction, including warm IVF state-cache hits (`Dataset::open_vector_index` reconstructs those hits). Reusing an already-open index object does not reconstruct it; older full-index cache entries use a separate unchanged path. No persisted format, routing, scoring, resource ceiling or caller deadline changes.

The fix lives at the cache producer, so it adds no blocking asynchronous cleanup to object destructors, session registry, global cache flush or forced pin refund. Reevaluate this patch when upgrading Lance against independent real index/query/restart and final reader-retirement tests.

## Packaged documentation

The published `protos/AGENTS.md` and `protos/CLAUDE.md` reference a root guide absent from the crate package. Their links point to the original guide at the exact published source commit. No upstream instructions are copied into a new root guide. All other upstream package files remain unchanged except the IVF production patch above.
