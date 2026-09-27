# L4_HANDOFF — matcher-aligned source previews

Latest audit: [L4_REAUDIT_20260927.md](L4_REAUDIT_20260927.md) records the
observed regex preview admission undercount, its policy-charge mitigation,
and a real daemon-process restart SDK regression. Read its exact receipts and
remaining claims before treating a selected test as closure.
The earlier [L4_FURTHER_AUDIT.md](L4_FURTHER_AUDIT.md) records cancellation,
multi-hit preview, verify-only regex candidate and cache-admission repairs.
The earlier [L4_AUTHORITY_ID_AUDIT.md](L4_AUTHORITY_ID_AUDIT.md) and
[L4_ADDITIONAL_AUDIT.md](L4_ADDITIONAL_AUDIT.md) remain historical receipts.
Aggregate regex heap proof remains BLOCKED; whole-workspace and installed
deployment qualification are NOT_RUN. A locally built real daemon binary
was exercised twice in the latest audit, below installed/release proof.

## Historical p02 source and scope

- Shared dirty `main@98601a66d8cab9c86232b3e62ce490c8b43b71b6`; preserve concurrent
  work. No agents, task messages/reads/polling, commit, push or reset were used.
- L4 source: lexical `searcher/{snippets,match_sets,candidates}.rs`, permanent
  `l4_match_anchored_preview` tests, normalizer `provenance.rs`/`tokens.rs`, regex
  `executor.rs`. Shared schema/coverage/callers are inspected as consumers.
- Follow-up lint fixes also touch `budgeted_search.rs`, `ranked_rows.rs`,
  `planner.rs`, `adapter_ingest.rs`, `sealed_generation/coverage.rs`, `symbol.rs`.
- Canonical resource-admission flock serializes heavy Rust runs. The proof binds
  the patched `vendor/tantivy-sstable` compilation inputs as well as workspace
  dependencies, configuration, toolchain, dirty source and executed binaries.

## Follow-up audit and fixes

1. **P2 — unobserved regex capture storage amplification.** A valid six-byte
   match with 512 optional captures retained 513 capture slots. The focused RED
   executed and failed (expected 1, actual 513), then the fix passed. Captures
   are converted to noncapturing groups through `regex_syntax` AST before the
   same bytes Regex engine compiles them. Original pattern, HIR, dialect and
   estimator remain authoritative; no capture values are exposed by this API.
   A differential regression compares truth and every match range with the
   original bytes Regex for alternation, repetition, scoped Unicode flags,
   named captures, extended comments, empty matches and case equivalents.
2. **P2 — normalization buffer reservation undercount.** Pinned normalization
   uses `(u8, char)` decomposition entries plus `char` recomposition entries,
   both with spare capacity, plus a heap-backed stable-sort scratch buffer.
   The bound now includes both layouts, growth and sort scratch
   before admitting owner mapping work. This is requested-capacity accounting,
   not an allocator/RSS measurement. A fixed 8,193-scalar
   combining-sequence oracle covers library sorting and non-monotonic original
   ranges. Existing budget tests cover refusal; no formula-mirroring test was added.
3. **Verification repair — 16 lexical lint diagnostics.** Borrow the
   unchanged probe, preserve integer-conversion errors instead of `.ok()`, use
   a typed query coercion, retain planner error mapping, and repair doc markup.
   A later frozen lint run found one added wildcard enum arm in `symbol.rs`;
   it now enumerates the unchanged fallback variants. These are not sixteen
   independently classified correctness defects.
4. That historical source pass covered Boolean rollback/NOT, original/NFC/folded byte
   coordinates, 240-byte focus, source binding, optional refusal, request leases
   and cancellation. No additional reachable P0–P2 was identified in that
   reviewed scope. Aggregate regex allocation remains the explicit proof gap
   below, not a reproduced runtime overrun.

## Resulting behavior

- Identity-only rows are ranked/grouped first. Selected immutable document
  addresses alone receive previews; preview refusal preserves IDs/scores/order.
- Token/phrase range extraction shares the canonical phrase predicate. Raw
  substring truth and mapped ranges share the same normalizer helper. Regex
  ranges use the same compiled executor as verification. Recall semantics,
  case policy and chunk NFC normalization remain unchanged.
- Bounded private provenance maps verified indexed NFC back to original byte
  intervals through composition, reordering and expanding lowercase. Maps cannot
  be deserialized or paired with caller-supplied source intervals.
- Boolean truth is recomposed per request. False branches and NOT retain no
   positive highlights. Content-filter leaves contribute mandatory AND witnesses.
  The legacy center-term collector/window renderer was removed; fixed regression oracles remain.
- A complete focus fitting 240 bytes is preserved before context. Larger focus,
  zero-width, no-positive, source-not-provided and optional resource refusal have
  explicit metadata. Path and synthetic symbol labels claim no source spans.
- Stored source owner is separate from the containing generation pin. Bound
  revision/hash must match sealed coverage; required missing/partial/duplicate
  source fields fail integrity. Explicitly unbound generations return
  SourceNotProvided without inventing revision/hash.
- Selected raw bytes, chunk extent/digest and indexed text are tied to the same
  immutable stored row and its text-authority member. Rendering opens no checkout
  files. Source whole-file SHA remains producer attestation; raw chunk SHA is
  independently computed at ingest and sealed.
- One request preview ledger, separate from mandatory collection: 10,000,000
  logical work units and 64 MiB reservations; 64 KiB source, 128 KiB transformed,
  262,144 map entries, 32 positive witnesses. Cancellation/deadline are mandatory.
- Historical p02 request-local compiled regex reuse charged 16 MiB per distinct
  executor; the latest audit adds a state-proportional charge before engine
  compilation. Both are logical admission charges, not an established
  aggregate compiler/HIR heap ceiling: regex 1.12.4 limits each NFA independently
  and regex-automata 0.4.14 may retain multiple NFAs. Aggregate compiler/retained
  heap-bound qualification is NOT_RUN. Existing leaf document-bitmap cache is
  unchanged; there is no global compiled or whole-query witness cache.
- Output guard-vector capacity is precharged. Request output-group slots are
  reserved only when rendering produced an output with a live allocation lease.
  Empty or unavailable-only pages consume no slot. Exhaustion degrades preview without losing hits;
  transfer into the request lifetime allocates nothing. Native direct callers
  must keep RequestBudget alive while retaining outputs. DTO clones are excluded.

## Historical p02 proof

[Final receipt](l4-proof/p02/receipt.json) binds exact commands/environment,
source/config/dependency inputs, toolchain, raw logs, binary hashes, dirty state
and historical revalidation. The verification copy is a filesystem snapshot,
not a new branch or commit. All 338 bound inputs were unchanged during those
runs and matched the original checkout at that closeout. Subsequent changes
mean this is not current shared-tree evidence; use the additional audit above.

All commands below ran with `CARGO_BUILD_JOBS=2`, the canonical resource-admission
lock, an explicit 900-second admission wait, and the recorded preserved target
directory. Full environment overrides and cwd are in each linked run receipt.

| Command | Outcome | Executed scope |
| --- | --- | --- |
| `./scripts/cargow test -p quanta-index-lexical -p quanta-index-lq-text-normalizer -p quanta-index-lq-regex --lib --test l4_match_anchored_preview --locked` | VERIFIED, exit 0 | 168 lexical + 4 native adapter + 80 regex + 20 normalizer tests |
| `./scripts/cargow clippy -p quanta-index-lexical --lib --test l4_match_anchored_preview --locked --no-deps` | VERIFIED, exit 0 | lexical library and selected native integration target |
| `./scripts/cargow clippy -p quanta-index-lq-text-normalizer -p quanta-index-lq-regex --all-targets --locked --no-deps` | VERIFIED, exit 0 | both owner packages, all targets |
| Canonical-env `rustfmt --edition 2024 --check` on eight follow-up source files | VERIFIED, exit 0 | exact paths, command, hashes and output in format-final-receipt.json |

**272 passed, 0 failed, 0 ignored.** This is owner/native evidence. Independent
Python NFC validation also agrees with the long combining-sequence golden;
its Unicode version, input hashes and exact action are recorded separately.

- Source SHA-256: `4d13be90b5f7329bd826ab73d4648775c446580017112c8771a3c98903ae941d`.
- Final receipt SHA-256: `e3288564821a429b9ac8c58d1384755ddb009cd692b00a813d67eb98a35fdfcd`.

The focused capture RED executed and failed with 513 slots versus expected 1;
its binary and log were captured, but no full pre-run dependency freeze exists.
It is diagnostic RED, not repository qualification. The corrected permanent
regression and reference-engine comparisons pass in the final native run.

Historical shared-tree and round1/round2 results are retained. The initial
shared native run passed 271 tests but changed inputs made it stale. Frozen
round1 found a Symbol wildcard lint; round2 found a non-NFC Kelvin test literal.
The Symbol fallback is now exhaustive, and the fixture uses an escape preserving
the same runtime bytes. Final round3 includes the stable-sort scratch refinement
and the new long-sequence regression. No historical result is promoted as final.

## Explicit limits

- Residual metadata/structural leaves without a bounded per-row truth API return
  UnsupportedRange; their truth is not guessed or rescanned unboundedly.
- Required identity/extent/text-authority fields are checked before optional
  refusal. Raw SHA/NFC comparison runs only when bounded preview work runs. A
  refused excerpt makes no positive raw-byte integrity claim.
- Full-file SHA is producer attestation, not independent whole-file byte proof.
- One regex engine search / normalization segment sort is not internally
  preemptible; input bounds and surrounding interruption checks are explicit.
- Regex compilation/retained heap has component limits, but neither the
  historical fixed 16 MiB charge nor the current state-proportional charge
  has a demonstrated aggregate upper-bound proof. Source-pattern bytes
  and retained executor count are bounded; this does not qualify a 64 MiB total
  preview heap ceiling. The source inspection and exact dependency-file digests are in
  [dependency-audit.json](l4-proof/p02/dependency-audit.json); no
  runtime memory overrun was reproduced by that inspection.
- Logical reservations do not prove process RSS, latency, ranking quality,
  candidate identity memory accounting, arbitrary DTO clone lifetime, release,
  deployment or activation.

## Public-path oracle packet for L0

Use a coverage-bound immutable chunk with source offset 0. Let `P` be exactly
`"context "` repeated 80 times (640 ASCII bytes). These are fixed expected raw
ranges, not ranges generated by the new mapper. Native owner and adapter cases executed successfully;
public SDK/daemon execution is **NOT_RUN**.

| Query AST / source | Expected preview |
| --- | --- |
| Keyword `needle`; `P + NEEDLE` | SourceChunk raw/NFC `[640,646)` |
| Regex `needle[0-9]+`; `P + needle42` | SourceChunk raw/NFC `[640,648)` |
| Keyword `needle`; `needlework ` + `P + needle` | raw/NFC `[651,657)`; prefix decoy never focus |
| Keyword `café`; `P + cafe\u{0301}` | raw `[640,646)`, NFC `[640,645)`, equivalence true |
| RawString `i\u{0307}`; `P + İ` | raw/NFC `[640,642)`; folded offsets are not raw offsets |
| Any(Not(Keyword `blocked`), Keyword `allow`); `blocked ` + `P + allow` | raw/NFC `[648,653)`; no blocked highlight |
| Phrase `blue whale`; `P + blue\r\nwhale` | raw/NFC `[640,651)` |
| Keyword `needle`; unrelated content at `needle.rs` | Path kind, path label, no content coordinates |
| RawString 200 `n` bytes; `P + 200 n` | complete `[640,840)` focus, 240-byte context `[600,840)` |
| RawString 241 `n` bytes; 241 `n` bytes | retained hit, empty snippet, FocusExceedsBudget |
| Regex `^`; `needle` | retained hit, UnsupportedRange, no fabricated one-byte span |
| Symbol local name `needle` | SyntheticSymbolLabel, no source excerpt coordinates |

For source integrity, mutate raw bytes while retaining the independently stored
chunk SHA (including NFC-equivalent mutation), remove/duplicate required source
fields, or disagree with generation coverage: expect SEARCH_PREVIEW_INTEGRITY.
For optional memory/work/group-slot refusal: preserve IDs, scores and order;
preview becomes WorkBudget. Cancellation and deadline retain mandatory error
codes. A real SDK drift proof must mutate the producer's configured checkout
while reading the old pinned generation; the native fixture-file mutation alone
does not prove producer/SDK checkout routing.

## Remaining qualification

- Additional wire tests and fuzz escalation were executed in the deeper audit;
  public API escalation failed on baseline drift. Exact source/status receipts
  are in [L4_ADDITIONAL_AUDIT.md](L4_ADDITIONAL_AUDIT.md).
- Public SDK/daemon and workspace qualification: **NOT_RUN by L4**. Native
  owner/adapter and wire evidence does not prove those rails.
- Aggregate regex compilation/cache heap upper bound: **BLOCKED on missing
  allocation proof**, distinct from the fixed capture amplification defect.
  See [L4_REGEX_BUDGET_RESIDUAL.md](L4_REGEX_BUDGET_RESIDUAL.md).
- Ranking quality, latency/RSS, release, deployment and activation: **NOT_RUN**.
