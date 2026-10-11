# Paged lexical store integration contract

Status: implementation in progress; runtime acceptance pending. 2026-10-10.

This contract fixes the next implementation boundary of
[LEXICAL-QUERY-PLAN.md](LEXICAL-QUERY-PLAN.md). The validated F16 core remains a
separate baseline; source implementation and executed acceptance are distinct.
The breaking admission decision is recorded in
[OCT-10-002](../../adr/OCT-10-002-shared-paged-lexical-authorities.md).

## Ownership

- Common storage: one private `lexical_store` module owns authenticated immutable
  objects, bounded COW map pages, body blocks and object-adoption custody.
- File authority: source/current maps, file segments, path/content gram routing
  and the fallible CodeSearch consumer.
- Native authority: source membership/current maps, independently readable text,
  trigram/positions segments, routing and the u64 query bridge.
- Publication: existing ingest materializer, generation writer, seal, activation,
  snapshot registry, scrub, reclaim and offline migration remain the owners.
  New store state is a component of these owners, not another publication service.

## Common interfaces

The Rust names below are the integration seam. Final wire layouts must be fixed
tuples with strict lengths, ordering, bounds and canonical encoding checks.

```rust,ignore
struct ObjectRef { kind: ObjectKind, bytes: u64, sha256: [u8; 32] }
struct BlobRef { root: ObjectRef, bytes: u64, sha256: [u8; 32] }
struct MapRoot { page: Option<PageRef>, entries: u64 }
struct PageRef {
    object: ObjectRef, first: Vec<u8>, last: Vec<u8>, entries: u64, height: u8,
}
struct MapEdit { key: Vec<u8>, value: Option<BlobRef> }

impl ObjectStore {
    fn open(state_root: &Path) -> Result<Self, CoreError>;
    fn begin_attempt(&self, binding: &AttemptBinding,
                     inherited: &[ObjectRef]) -> Result<AttemptLease, CoreError>;
    fn put_blob(&self, lease: &AttemptLease, kind: ObjectKind,
                dependencies: &[ObjectRef], bytes: &[u8]) -> Result<BlobRef, CoreError>;
    fn read_blob(&self, blob: &BlobRef, max_bytes: u64,
                 budget: Option<&RequestBudgetV1>) -> Result<Vec<u8>, CoreError>;
    fn map_get(&self, root: &MapRoot, key: &[u8],
               budget: Option<&RequestBudgetV1>) -> Result<Option<BlobRef>, CoreError>;
    fn map_scan(&self, root: &MapRoot, lower: &[u8], upper: Option<&[u8]>,
                budget: Option<&RequestBudgetV1>,
                visitor: &mut dyn FnMut(&[u8], &BlobRef) -> Result<(), CoreError>)
                -> Result<(), CoreError>;
    fn map_apply(&self, lease: &AttemptLease, base: &MapRoot,
                 sorted_edits: impl Iterator<Item = Result<MapEdit, CoreError>>)
                 -> Result<MapRoot, CoreError>;
}
```

- Object digests bind kind, format, dependency references and payload. Object
  paths are derived from validated digests beneath the pinned store directory;
  arbitrary paths and symlinks cannot become references.
- A read hashes the owned buffer subsequently decoded or matched. Page fences,
  subtree counts/heights and ordered inventories prove both presence and absence.
- QLMAP002 leaf values use either inline full references or a sorted dictionary
  of complete BlobRef values with bounded u16 row indices. Dictionary mode is
  canonical only when `2 + 81*unique_values + 2*rows < 81*rows`; ties use inline.
  Every dictionary entry must be used, sorted and unique. Missing/out-of-range
  indices, nonminimal modes and legacy page magic are refused. This changes
  stored bytes, not the decoded map IR or the exact object dependency closure.
  The coupled epoch is sealed format 19, File root 18 and Native root 6. Attempt
  identity uses the same sealed format authority; older roots require rebuild.
- Blob payloads use bounded physical blocks and a bounded reference tree. A whole
  admitted logical document may be reconstructed for NFC/regex verification;
  physical block boundaries never become independent regex inputs.
- Map keys are bounded internal identities, not unbounded source paths. Source
  digest keys retain and check the complete source identity in the value; a
  collision cannot overwrite a distinct source. Route membership uses one
  `(typed surface, gram-or-term, segment)` key per entry, avoiding a growing Vec
  value for a common gram.
- Sorted batch edits rewrite each affected page once. Full bootstrap builds
  ordered pages in a stream. Repeating a point insert for every member while
  rewriting the same page is not the required bulk implementation.
- Native empty-root bootstrap stages each bounded cohort's five map edit sets
  into private disk scratch with a 2 MiB SQLite cache. It rejects duplicate keys
  and deletions, streams each namespace in key order, and builds each final map
  once. Scratch is neither publication custody nor recoverable authority: all
  referenced objects must already be adopted by the durable attempt. Failure
  publishes no Native root. Nonempty roots retain the COW delta path.
- Page/blob reads and map traversal propagate I/O, integrity, allocation and
  cancellation errors. Missing referenced objects are errors; a missing map key
  is a verified query result.
- No new source admission ceiling is inferred from physical page/block sizes.
  Existing 8 MiB file, 32,768 file, 128 MiB raw corpus and 20M membership limits
  remain until a separately validated policy change.

## Authority roots and mutation

Each small authority root binds format/normalizer/strategy/policy, allocation
scope, checked summaries and these map roots:

- source identity to its complete current source/coverage/derived-input binding;
- logical document-version u64 to exactly one live segment/local-u32 locator;
- source identity plus logical ID to live unit membership;
- immutable segment ID to segment descriptor;
- typed gram/term plus segment ID to routing membership.

A segment keeps one logical document's postings together. Body, trigram and
positions are separate authenticated ranges. Local IDs increase with logical
u64 IDs. File and chunk universes remain distinct. The reader exposes fallible
point lookup, ordered scan and candidate iteration; it does not recreate all
source rows, bodies or metadata maps during ordinary open.

Changed effective bundles retire old visible IDs and obtain new IDs. Exact
derived-input inheritance and compaction retain IDs. The materializer passes
the original validated `SourceEventAllocationV1.root_incarnation` through the
internal publication owner; no public ingest-wire change or path inference is
needed. The existing ActivationCatalog repository envelope reserves checked
u64 ranges per `(incarnation, repository, namespace)` under its existing mutation
lock and atomically persists the original event binding and both namespace ranges.
There is no separate lexical counter registry. Durable
attempt binding fixes event, base/target, operation digest, format and allocation
range. Retry reuses that binding; missing/corrupt custody refuses rather than
recovering from the live maximum. Existing C5 keeps generation ownership.

The outer lexical manifest commits the small file/Native roots rather than every
shared leaf. Normal open authenticates these controls, with leaf faults found by
selected reads or scrub. This deliberately also changes the immediate detection
timing of missing/truncated leaves from F16 and requires an ADR and paired tests.
Tantivy, BM25 and other providers retain their own artifact contracts.

Point lookup may reuse an authenticated decoded map page only within the same
request/proof meter and with the exact full `PageRef`. Its existing 4 MiB retained
LRU accounts for both raw and decoded allocations. A fresh seal/scrub proof gets
a fresh meter and authenticates the underlying objects again. Caller-held evicted
Arcs are bounded traversal scratch, not retained-cache capacity or a process RSS
guarantee.

File seal proof compares source-derived grams and exact posting membership with
each complete term/posting-page stream. It derives expected routing independently
from this oracle for new segments and from authenticated sealed-base term pages
for removed segments. The global ordered route diff must match the exact segment
reference and per-segment/surface domain-separated SHA-256 plus count of fixed-width
route keys in both directions. This avoids per-route rereads and retains witness
state proportional to changed segments; it does not trust producer-supplied route
digests. Complete scrub retains its independent lookup checks.

## Maintenance and lifetime

The pool is local to the lexical state root. A durable attempt lease protects
inherited roots and newly prepared objects before adoption. Adoption and unlink
share a lifecycle gate. Query generation pins remain owned by the snapshot
registry; shared-object GC additionally marks all retained, pending, recovery,
maintenance and adopted roots. An unreadable or incomplete protected closure
aborts sweep. A captured durable adoption/publication epoch fences every sweep
batch; a changed epoch aborts collection. An exact temporary SQLite mark set
bounds heap independently of the reachable object count. A process-local counter
alone is insufficient. The namespace fence is held only for the epoch and retained
directory snapshot, so marking does not hold up generation retirement.

Compaction prepares bounded outputs from a pinned root and installs them only in
the next unsealed target. It preserves logical IDs and independently compares
input/output live membership, then changes locators and routing together. Input
drift discards/revalidates the result. Admission evaluates the post-merge target,
with merge reserve and bounded debt; it cannot deadlock the publication that
would reduce the run count. No sealed root is rewritten in place.

Scrub follows the committed object closure and checks logical membership.
Shared corruption quarantines every referencing generation through the existing
owner. Backup/restore includes the reachable pool closure, counter and attempt
custody; restore uses the existing new-incarnation fence. These are required
integration work, not promises supplied by the standalone page codec.

## Acceptance

No new store is activated before independent source/posting/routing/current-map
oracles, malformed canonical-block tests, u64 retirement/bridge tests,
split/merge/absence checks, retry/crash cuts, pinned-reader/adopt/GC races,
compaction debt limits, SDK/producer bootstrap and backup/restore pass. Benchmark
claims bind the complete coupled source and keep foreground and maintenance
costs separate. The existing F16 tests and XL result do not qualify this design.
