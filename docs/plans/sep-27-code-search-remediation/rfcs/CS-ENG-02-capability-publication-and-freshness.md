# CS-ENG-02 — Total coverage pipeline cost qualification

Status: `ACTIVE` for total update work and physical heap measurements. Shared
snapshots and bounded immutable page storage are implemented in manifest format
9 and consolidated in
[SEP-27-003](../../../adr/SEP-27-003-code-search-source-and-preview-contract.md).
Their owner/lifecycle proof is recorded in
[CS-INT-01](CS-INT-01-integration-and-qualification.md).

## Remaining mechanism

`plan_batch_coverage` still opens/verifies/decodes the entire sealed base.
The candidate shares immutable rows and private derived tree indexes, and the
writer serializes only changed bounded partitions plus the fixed-size root.
Untouched pages carry authenticated base commitments and hard-linked inodes.
The same build now passes the verified candidate coverage commitment to seal:
seal re-hashes the actual root/pages and rejects orphan pages without a second
coverage decode. Small page reads allocate their actual encoded length rather
than the 1 MiB page ceiling. Retry comparison and generation open still decode
effective coverage; strict scope checks and generic generation verification
have separate full-scope costs. Paging does not establish sublinear total ingest.

The supported envelope is 256 hash-routed pages, 4096 rows/1 MiB per page,
32 KiB root, 64 MiB encoded total and conservative 256 MiB decode admission.
All changed page/root encoding and envelope checks precede target mutation.
Skew refuses with `INGEST_RESOURCE_BUDGET_EXCEEDED`; it does not silently split,
truncate, drop files or claim complete coverage. The residency estimate is not
an allocator or RSS bound.

Owners: lexical ingestion and sealed coverage/lifecycle, pinned generation read
handles and resident accounting.
[CS-BENCH-04](CS-BENCH-04-comparators-performance-and-incremental.md) owns admitted
measurements; [CS-INT-01](CS-INT-01-integration-and-qualification.md) owns combined
producer/daemon qualification. No stale-result defect has been reproduced.

## Remaining acceptance

- [ ] Measure one-file replacement/delete and mixed batches over increasing file
  counts, including index, ranked keys, coverage, text shards and publication.
  Report actual bytes read/fresh bytes, rows/pages visited, time and temporary /
  retained heap. Seal hasher counters exclude decoder reads and are not total I/O.
- [ ] Reuse an authenticated pinned immutable base handle across independent
  preflight/build calls where lifetime/custody permits. The same-build seal
  decode is removed, but preflight/build and open still scan the full base;
  seal still re-hashes every effective coverage page.
- [ ] Measure physical peak/resident allocation on the actual total pipeline.
  Conservative charges and fallible encoded/read buffers alone do not qualify
  all persistent-tree and decoder allocations.
- [ ] Execute actual producer/SDK/daemon/crash/restart/retention qualification on
  the combined format-9 source. Keep hosted, installed, Linux and activation
  claims separate from local owner fixtures.

Owner fixtures compare every effective row against independently constructed
inputs; prove old-reader retention, unchanged-row sharing, page inode inheritance
and actual new coverage bytes; reject forged semantic rows after recomputing
hashes, missing/tampered/symlink/orphan pages and encoded oversize before target
creation. A staged retry tolerates and reclaims orphan pages around the atomic
root rename; the writer syncs orphan removal before sealing. Sealed verification
rejects orphan pages and a missing root. These are correctness/storage
observations, not admitted performance or physical-memory measurements.

Current owner-local regressions on `3272662c` plus the working-tree changes:
coverage library 13/13, sealed commitment and manifest controls 1/1 each, and
128/512/2048-file mixed-delta diagnostics 1/1 each. The three ignored manual
cost probes were also run separately as fresh test-binary processes under
macOS `/usr/bin/time -l` on the same working tree:

| Files | Preflight ms | Delta build ms | Open ms | Fresh generation bytes | Coverage bytes within fresh generation | Whole-process maximum RSS |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 128 | 20 | 310 | 23 | 138,680 | 9,142 | 57,966,592 |
| 512 | 70 | 560 | 75 | 557,321 | 18,782 | 69,287,936 |
| 2,048 | 256 | 1,418 | 258 | 2,306,639 | 28,786 | 115,867,648 |

Each one-test process passed. Its RSS includes the initial base build and the
test runtime, so it is not the delta phase's temporary heap peak. Fresh bytes
are the new generation's unshared entries, not all bytes read or temporary
disk writes. There is no frozen performance limit, controlled quiet host or
per-phase allocator measurement; these observations do not qualify total
pipeline cost or physical memory admission.

Parsing and paired lexical/semantic activation remain unchanged. Optional symbol
coverage does not authorize independently activated lexical publication.
