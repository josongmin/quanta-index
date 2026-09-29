# CS-ENG-02 — Total coverage pipeline cost qualification

Status: `ACTIVE` for total update work and physical heap measurements. Shared
snapshots and bounded immutable page storage are implemented in manifest format
9 and consolidated in
[SEP-27-003](../../../adr/SEP-27-003-code-search-source-and-preview-contract.md).
The accepted owner/lifecycle contract is in SEP-27-003;
[CS-INT-01](CS-INT-01-integration-and-qualification.md) owns remaining
combined-source qualification.

## Remaining mechanism

`plan_batch_coverage` still opens and verifies the entire sealed base.
On the ordinary new-delta publication path, the materializer invokes lexical
preflight before durable intent and repeats it under its operation lock;
lexical `build_batch` plans coverage again. These are three separate base walks
before counting seal/open work, although replay/repair shortcuts differ.
An adapter retains at most one decoded coverage root admitted under an 8 MiB
conservative decode-heap estimate. Identical root bytes authorize immutable row
reuse only after the current root commitment, every page's hash/length and the
directory inventory pass again. Coverage refusal discards the entry. Repeated
preflight/build then records zero decoded rows while keeping the same page and
byte reads. Larger roots use the uncached full-decode path; the estimate is not
a physical memory cap and the cache does not remove any verification boundary.
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

Sep-28 owner measurement on the integrated branch: growing one-file and mixed
deltas at 128/512/2,048 files confirm three full coverage reads. At 2,048
files, each outer preflight, lock-held preflight and build reads 256 pages,
2,048 rows and 883,542 encoded bytes; the three reads total 2,650,626 bytes
before seal/open and other sidecars. Mutation after either preflight is refused
before target creation, old readers remain valid, and retry succeeds after
repair. Separate-process RSS from the lexical diagnostic includes base
construction and is not phase allocator-heap qualification. The port has no
authenticated immutable base token; external file mutation remains possible
under the process-local lock. Removing a recheck would weaken the demonstrated
tamper refusal, so the scan optimization remains open.

Sep-29 local owner update: bounded coverage-row decoding now reserves an
admitted definite CBOR row count once and grows indefinite rows geometrically,
instead of requesting exact capacity for each row. A 4,096-row indefinite page
decodes; the 4,097th row refuses. Focused owner tests and growing 128/512/2,048
file diagnostics passed. The 2,048-file mixed delta still reads 256 pages and
883,542 encoded bytes at each of the three preflight/build phases, with a
separate verified open. This removes avoidable decoder reallocations, not the
authenticated base scans or the unmeasured total-pipeline physical heap.

Current-main RCA rechecked the three call sites and both mutation boundaries.
The 128-file mixed-delta owner probe read the same 100 base pages and 54,479
page bytes in each of outer preflight, lock-held preflight and build; cached
decode reduced the latter two to zero decoded rows. The two owner mutation
tests still refuse changed base pages between those phases and accept a retry
after repair. Since base pages remain externally mutable under the process
lock, removing either later authenticated read without a pinned immutable
base capability would regress that refusal contract. These page-byte counts
exclude other pipeline I/O and do not qualify physical peak heap.

## Remaining acceptance

- [ ] Measure one-file replacement/delete and mixed batches over increasing file
  counts, including index, ranked keys, coverage, text shards and publication.
  Report actual bytes read/fresh bytes, rows/pages visited, time and temporary /
  retained heap. Seal hasher counters exclude decoder reads and are not total I/O.
- [ ] Account separately for outer preflight, lock-held preflight, build, seal
  and open. Reuse an authenticated pinned immutable base handle where its
  lifetime/custody preserves both pre-intent refusal and lock-time ownership
  checks; never replace either check with an unbound cache. The same-build seal
  decode is removed, but seal still re-hashes every effective coverage page.
- [ ] Measure physical peak/resident allocation on the actual total pipeline.
  Conservative charges and fallible encoded/read buffers alone do not qualify
  all persistent-tree and decoder allocations.
- [ ] Execute actual producer/SDK/daemon/crash/restart/retention qualification on
  the combined format-9 source. Keep hosted, installed, Linux and activation
  claims separate from local owner fixtures.

Owner fixtures exercise cached page corruption, orphan/identity/root changes,
refusal eviction and repaired retry, alongside uncached large-root admission.
They compare every effective row against independently constructed
inputs; prove old-reader retention, unchanged-row sharing, page inode inheritance
and actual new coverage bytes; reject forged semantic rows after recomputing
hashes, missing/tampered/symlink/orphan pages and encoded oversize before target
creation. A staged retry tolerates and reclaims orphan pages around the atomic
root rename; the writer syncs orphan removal before sealing. Sealed verification
rejects orphan pages and a missing root. These are correctness/storage
observations, not admitted performance or physical-memory measurements.

Historical local cost probes in the [plan archive](../../ARCHIVE-INDEX.md)
excluded decoder reads/temporary writes from fresh-byte counts and included
base construction/test runtime in RSS. They do not qualify delta-phase physical
peak or total pipeline cost. Re-measure before accepting a cost claim.

Parsing and paired lexical/semantic activation remain unchanged. Optional symbol
coverage does not authorize independently activated lexical publication.
