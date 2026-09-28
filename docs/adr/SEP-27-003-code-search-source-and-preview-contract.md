# SEP-27-003 — Code Search Plan, Source and Preview Contract

Status: `Accepted`

Decided: 2026-09-27

Consolidates the implemented L1–L5 contracts and completed owner/RCA records.
It extends [SEP-26-001](SEP-26-001-retrieval-query-publication-and-result-proof.md)
without admitting a ranking experiment, physical heap bound or deployment claim.

## Context

The engine campaign repaired domain/decoder mismatches, false empty/exhaustion
facts, ambiguous file mutation, incomplete symbol authority, source identity,
collection interruption, approximate snippet anchors and parser ownership.
Separate handoffs repeated those decisions alongside changing test counts and
source snapshots. The decisions belong here; remaining implementation and
qualification belong in the [active code-search ledger](../plans/sep-27-code-search-remediation/rfcs/CS-INT-01-integration-and-qualification.md).

## Decision

### Validated query and result window

- Validate endpoint/domain/projection and all admitted primitives before
  result-dependent or language-intersection empty shortcuts. Bind route and
  decoder through one validated plan; a Symbol endpoint cannot decode Text rows.
- Preserve valid generic Text-to-Symbol selection. Unsupported Symbol text
  primitives refuse with their typed planner error; projection conflicts return
  `INVALID_REQUEST`. Do not filter malformed rows into a successful empty result.
- A lane is executed only after its backend is invoked. Valid logical emptiness
  carries logical proof without claiming execution. Capability checks still use
  the selected immutable read view where required.
- `count:N`, returned rows and response clipping do not define the match universe.
  Producer count/continuation facts own exhaustion; a continuation counts the
  remaining matches. Cursor identity binds query/options/constraints, route,
  ranking, caps and snapshot. Wrong context refuses; retained old pins stay valid.

### Canonical file revision and publication

- One source-file key `(source_repo_id, canonical path)` replaces chunks,
  symbols and coverage together. Reject overlapping surface aliases, replace/
  tombstone/clear conflicts, internal path/source mismatches and duplicate or
  cross-kind IDs before mutation. Sealed generations remain immutable.
- Include empty and zero-unit admitted files. `SourceFileCoverage` binds source
  revision/hash, language, producer policy and the complete unit-set commitment.
  Complete with zero symbols is distinct from NotRequested, Unsupported,
  ParseFailed and ProducerFailed. Producer attestation is not independent parser
  completeness or full-file byte verification when source bytes were not supplied.
- Coverage and publication lineage are committed by the existing sealed lexical
  generation and read through the same generation handle. Seal/open/scrub,
  inheritance, recovery and reclaim must include them. Incomplete inherited files
  cannot disappear from a strict gate just because a Delta did not modify them.
- Manifest format 9 binds coverage format 2: one generation root commits sorted
  `(slot, length, SHA-256, row count)` references to immutable coverage pages.
  The logical snapshot shares immutable rows and tree paths; its private derived
  partition index cannot be mutated independently. The canonical coverage owner
  derives changed slots from replacements/tombstones before index mutation.
- Routing is SHA-256 over length-framed source repository/path with fixed u64
  little-endian lengths. Support is bounded to 256 slots, 4096 rows and 1 MiB
  per page, a 32 KiB root, 64 MiB total encoded pages and conservative 256 MiB
  decode residency admission. Skew/oversize refuses typed before target writes;
  these charges do not assert physical allocator or process RSS ceilings.
- Pre-encode all changed pages/root, hard-link untouched authenticated pages,
  atomically replace the root and exclude old coverage artifacts from generic
  generation cloning. Seal compares each measured or inherited page commitment
  to its authenticated root reference, including a change during sealing.
  Scrub quarantines an unauthentic root before expanding child references;
  it hashes every child against the authenticated root thereafter. An unsealed
  target with orphan pages and no root is unbound. A retry can read a valid
  staged root despite orphan pages, but a sealed door refuses them. The writer
  publishes the new root before removing and syncing obsolete pages, so either
  root remains readable across the rename crash cut; retry reclaims orphans.
  Seal commitment accounting and scrub expand child
  commitments from that authenticated root. Missing, duplicate, reordered,
  misrouted, trailing, tampered and uncommitted artifacts refuse; cardinality
  decoding refuses malicious definite and indefinite array lengths.
  Format 8 and earlier require rebuild; there is no legacy coverage reader.
- Shared cloning and bounded page writes remove whole-map deep cloning and flat
  coverage rewriting. Full base verification, seal validation, retry comparison
  and open still read the effective coverage. Total ingest remains O(files) at
  those stages; fresh-page bytes alone do not establish sublinear total work.
- Within one build, seal reuses the verified candidate coverage commitment
  instead of decoding that candidate a second time. It still hashes the actual
  root and every effective page and rejects orphans. Independent preflight/build
  calls and generation open still decode the full base; small page reads allocate
  their encoded length. This is an I/O and allocation reduction, not a total
  pipeline or physical peak-memory bound.
- Check strict symbol authority over the effective pre-result source scope, for
  every plan using that authority. Incomplete coverage is a typed refusal, not
  exact zero. A text-admitted malformed file remains searchable under its profile.
  Independent symbol clears cannot leave false Complete coverage.
- Source stream/event identity, payload and expected parent are distinct from
  generation monotonicity and activation CAS. Replay returns the original
  publication binding; same event/different payload conflicts. A Delta's actual
  sealed base must match its declared source lineage. Do not let another stream
  overtake an unresolved source target for the same paired publication.
- Decode the complete raw/typed operation list before preparing storage or
  reserving source state. Structural Delta authority starts from the complete
  base, applies replacements/deletes and commits the resulting chunk universe
  with its batch identity before retention can retire the base.
- Structural trees and runtime catalog entries that reference lexical chunks
  publish only after the pinned source generation materializes those chunks.
  Orphan references refuse at ingest; they cannot create a ready auxiliary
  generation whose query later fails with a missing source chunk.
- Finalize-only recovery after base retirement requires the exact original
  reservation, completed target transaction and physically exact sealed tracks.
  Missing completion refuses. Queries observe complete old/new snapshots;
  uncertain ACK recovery reconciles committed state instead of inventing an ACK.
- Parsing stays producer-owned. Retain paired lexical/semantic activation.
  Optional symbols do not establish embedding-free or independently activatable
  lexical publication. External producer event issuance/cutover needs its own proof.

### Exact identity and bounded collection

- Explicit local/qualified-name lookup uses dedicated original, NFC and folded
  fields with the requested case semantics. Keep legacy broad keyword behavior
  distinct. Preserve signature/definition/source facts; do not guess names from
  filenames or change ranking weights through an exact-lookup repair.
- Preserve containing index pin and federated source identity separately. Group
  files by source repository/path before top-k; select the global best admissible
  representative with deterministic ties. No fixed overfetch/dedup approximation.
- Native traversal, predicate/restriction whole-set collection, harvest/merge,
  sort and retained carriers share examined/work/byte and interruption authority.
  Refusal returns no falsely exact partial set. Keep errors sticky and observe
  cancellation before allocating more retained resources; release reservations.
  Returned whole-set `BTreeSet` and manual candidate vectors remain examined-
  count bounded, rather than fully charged to that byte ledger. Do not promote
  collection reservations to an aggregate allocator guarantee.
- Required stored text/kind/symbol/line fields use strict unique decoders.
  Missing, malformed or duplicate authority refuses; a snippet or empty string is
  not a substitute. Manual `index:no` cannot infer non-stored language metadata
  from the extension; explicit language constraints remain typed unavailable
  until a coordinated stored-language/rebuild change is admitted.
- Format 8 commits one immutable `ranked-keys-<segment-id>.bin` table per
  Tantivy segment, derived after the final commit from repository, path and
  candidate dictionaries. Deltas hard-link unchanged tables, derive new/merged
  segment tables and remove retired tables. Activation/query open checks the
  exact segment inventory, SHA/length, term counts, offsets, UTF-8 and strict key
  order; missing, extra, malformed or changed tables refuse the generation.
- Grouping keeps segment-local ordinals; ranked pages compare borrowed keys
  before retaining rows and charge retained strings to the request collection
  budget. Queries do not use Tantivy's SSTable ordinal-to-string decoder.
  Resident table bytes count once in the generation estimate; the total table
  size is capped at 64 MiB at seal/open. The snapshot registry separately admits
  the complete generation estimate, including Tantivy and authority data.
- Seal construction uses Tantivy's public dictionary stream and can allocate
  while preparing a segment. Request budgets do not govern seal/open work or
  prove process-wide RSS bounds. Index-segment content remains checked at
  seal/scrub and length at open: same-length fast-field mutation between scrubs
  remains outside the table digest's guarantee.
- Format 7 requires a producer rebuild. The current lockfile uses the registry
  `tantivy-sstable`; no private patch or CI credential is part of the build.
  Retention of the former private repository is an archive decision, not a
  runtime dependency. Remaining combined-source integration proof stays open.

### Canonical match witnesses and original bytes

- Rank/group immutable candidate identities first; render only selected rows.
  Token/phrase/raw/regex witnesses share their canonical matching authority.
  Recompose Boolean truth per request: NOT and false branches produce no positive
  highlight; content-filter leaves contribute their required witnesses.
- Verify indexed NFC against immutable raw authority and map through bounded
  interval provenance, including composition, reordering and expanding case
  mappings. Chunk normalization is not whole-file normalization. No fabricated
  source offset, mutable checkout text or caller-supplied private provenance map.
- Preserve complete focus when it fits the 240-byte window; reduce surrounding
  context first. Oversized/zero-width/no-positive/source-not-provided and optional
  budget refusal have explicit metadata. Paths and synthetic symbol labels do
  not claim source spans. Retain overlapping substring witnesses.
- Integrity failures refuse typed. Optional preview exhaustion preserves selected
  IDs, scores, order and truthful window metadata. Charge witness/map/output
  lifetimes and normalization buffers before work; incomplete witnesses never
  enter a complete-result cache.
- Regex capture removal only erases unobserved explicit captures through the
  canonical AST; original HIR/dialect/estimator and truth/ranges remain authoritative.
  Preserve differential tests against the admitted matcher. The base/state policy
  charge is not aggregate allocator admission; the [open regex ticket](../plans/sep-27-code-search-remediation/rfcs/CS-ENG-04-match-anchored-snippets.md)
  owns that remaining boundary.
- The canonical executor pins a 10 MiB compiled NFA limit and 2 MiB lazy-DFA
  cache limit per engine and maps engine-size refusal to a typed resource error.
  The validated structural state estimate is bounded before literal extraction;
  the default ceiling is 100,000 states. Explicit `require_literal=false` admits
  bounded verify-only plans, while strict policy rejects absent literals and
  empty alternatives. Neither estimate bounds Unicode automaton bytes.
  Manual text/symbol scans and manual dense admission each retain at most four
  compiled patterns per request-local cache; failed compilations are not cached.
  Manual scoped content predicates
  use their supported word-boundary grammar and per-document evaluation, while
  indexed scope collection retains Tantivy's FST grammar.
  Structural negative-universe file regexes compile once per request and retain
  distinct syntax and resource failures. Manual `repo.has.*` gates retain at
  most 64 distinct canonical sets under a separate logical byte account; each
  set is materialized before that charge. None of these limits admits parser or
  compiler temporaries, cumulative collection work, or aggregate request heap.

### Producer syntax and ownership

- Enumerate the complete admitted file universe with source/policy/grammar hashes,
  truthful capability and stable diagnostic spans before strict publication.
  ERROR/MISSING recovery never implies Complete; unsupported, parse failure,
  producer failure, timeout and cancellation are distinct. Disclose truncation.
- Keep intentionally malformed source in the universe. The Vite repair accepts
  the valid generic-call/type-export forms under pinned grammar/runtime policy;
  strict symbol profiles still refuse the intentionally malformed fixture.
- Derive identities and ownership from AST structure: Rust generic impl/direct
  method owners; JS/TS functions versus methods; Python nearest named scope;
  direct TS class/interface/type-alias members versus anonymous nested type
  signatures. Canonicalize dotted namespace segments from AST identifiers while
  preserving source spans. Never emit guessed symbols or charge skipped anonymous
  signatures as successful extracted facts.

## Consequences and verification boundary

Retain owner/public regressions for legal/empty plans, windows/cursors, mutation/
lineage/crash, federated exact lookup/collection refusal, Unicode/Boolean previews
and independent parser inventories. The campaign executed owner and real-daemon
tests on several historical sources; their counts are not one current qualification.

Ranked storage and collection import all five column names from the existing
schema owner; schema construction uses those same constants. This removes the
collector/storage module cycle without changing names or field layout.
Outstanding work is physical regex admission, total coverage pipeline cost/heap,
final combined-source/platform/external-producer qualification and separately
admitted benchmark/default policy. This ADR does not qualify those claims.
Historical bodies are recoverable from `1419f3087f4f09a6ecab4ef39c30a2bf32544d5d`;
see [the plan archive](../plans/ARCHIVE-INDEX.md).
