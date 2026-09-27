# L4_HANDOFF — pre-G0 owner work

> Historical report: one-off evidence files were removed from the repository. This report alone is not current verification.

Historical pre-G0 receipt. **Superseded for current implementation status**:
L0 (`01a0dea8-aa8e-7d73-857f-b174f7be64fe`) has since supplied G0-L4 preview/source
DTOs, integrity policy, separate canonical preview ledger policy, normalizer
exports and caller-wiring ownership. Owner witness/provenance implementation is
in progress; the tests below prove only the earlier regex subset. Final L4
handoff and fresh source receipts will replace this status. This is not L4 completion.

## Source and ownership

- Initial source: `main@66cee47efdda7c5f3886ac58690aa645f44f691f`, dirty.
- A concurrent actor moved HEAD. Final regex proof ran at
  `106d7abec2dd3fa03f9db5a19a3de41df2f0afad`, dirty, with bound inputs unchanged
  across the commands. This lane did not commit, push, reset, or create agents.
- Owned edits: `crates/quanta-index-lq-regex/src/executor.rs` and
  `crates/quanta-index-lexical/src/searcher/snippets.rs` (tests only).
- Existing and concurrent ingest, ranking, paging, benchmark, and tooling changes
  were preserved. No shared DTO, compiler, candidate converter, export, SDK,
  manual scanner, or port was edited.

## Implemented owner primitive

`RegexExecutor::find_ranges_bounded(doc_text, max_source_bytes, max_ranges,
interrupted)` uses the **same `compiled` regex** as `verify`. It never changes
NFC, pattern flags, dialect admission, or retrieval truth. It returns byte ranges
and explicit `exhausted`; hitting a cap does not prove exhaustion. Zero-width
ranges remain zero-width. Source-byte overflow and cancellation return distinct
typed owner errors. Cancellation is observed before and after each engine search,
including a no-match search; partial ranges are discarded.

One regex search is not preemptible. Its input bytes are capped. Aggregate work,
inflight accounting, UTF-8 policy and public optional-preview outcomes remain
caller responsibilities pending G0. This primitive is not yet used by rendering.
No compiled-pattern cache was added. Existing `RegexMatchCache` caches generation-
bound leaf **document bitmaps**, not ranges or whole-query Boolean witnesses.
The planner and verifier currently compile executors separately; request-local
executor reuse needs L0 compiler wiring, not a fictitious existing pattern cache.

## Native proof

See machine receipt for all bound file digests, dirty
state, toolchain, binary digest, commands and raw log digests.

Bound regex/trigram source/config digest:
`2aa111e684fb6dd97fac1d3c8350035e2da5c6ae805d188c2f3e142da8cc4de9`.

| Command | Outcome | Scope |
| --- | --- | --- |
| `./scripts/cargow test -p quanta-index-lq-regex --lib -j 2` | VERIFIED, exit 0, 78 passed | Includes 4 new range owner tests |
| `./scripts/cargow clippy -p quanta-index-lq-regex --all-targets -j 2` | VERIFIED, exit 0 | Regex package targets |
| `./scripts/cargow fmt -p quanta-index-lq-regex -- --check` | VERIFIED, exit 0 | Regex formatting |
| Lexical renderer tests / SDK / workspace gates | NOT_RUN | L0 heavy-run serialization and G0 integration absent |

The four new tests assert independent byte ranges for case-insensitive regex and
UTF-8, non-exhaustive cap 0/1, source overflow, zero-width, immediate cancellation,
cancellation after partial work, and deadline expiry on no-match search. Existing
74 regex unit tests also ran. This is dirty-tree owner proof, not exact-commit,
installed-process, SDK, performance, or repository qualification.

## Permanent renderer regression inputs

Nine `l4_witness_regressions` tests were added under the real renderer owner:
ordinary literal positive control, default folded case, regex, token-prefix
decoy, decomposed NFC source, expanding lowercase, false negated OR branch,
phrase across CRLF, and a fitting 200-byte focus under the existing 240-byte cap.
Expected raw highlights are fixed fixture strings, with UTF-8-checked slicing.

These tests are **NOT_RUN**, not observed RED or GREEN. They intentionally state
the required behavior that the old renderer does not implement. The temporary
test helper still calls the existing literal-list API; migrate it to the agreed
prepared-query witness API after G0, retaining the same fixture oracles. Heavy
lexical builds were not started without coordinator serialization.

Suggested owner command after slot admission:
`./scripts/cargow test -p quanta-index-lexical --lib l4_witness_regressions -j 2`.

## Required L0/L1 connections — proposals, not adopted contracts

1. L0 must freeze witness kinds (source content / path / synthetic symbol label),
   focus/context source coordinates, unavailable/clipped reasons, source identity,
   integrity-error mapping and preview budgets. Oversized focus, zero-width,
   exhausted preview work and missing immutable source must be distinguishable.
2. `candidates.rs::document_to_candidate` currently accepts `&[String]`. It needs
   the actual prepared expression/options and a snapshot-bound source/read context,
   not `snippet_center_terms(query)`. Its raw stored snippet and normalized
   `chunk_text` must be checked against the same immutable text authority. A raw
   source mismatch is an integrity error; it cannot use a checkout fallback.
3. `paging.rs::rows_to_candidates` and `port.rs` must carry that context after
   final ranked/grouped row selection. L3 owns paging, L1 owns port. All-results
   paths need explicit bounded behavior rather than accidental unbounded previews.
4. L1's manual scanner currently renders every matched candidate **before**
   grouping/sorting/truncation. It must defer witness construction until final
   selected rows, including the symbol scanner. Preview refusal must preserve
   candidate identity, ranking, totals and cursor facts.
5. Reuse normalizer token positions and its phrase predicate, exposing a bounded
   range result from the same owner rather than a handwritten snippet matcher.
   Regex leaves use the new executor method with the existing normalized source
   and case flags. Prepared metadata/predicate truth must come from its canonical
   matcher. Recompose Boolean truth per request; false branches and NOT never
   contribute positive highlights.
6. A request-local provenance owner must map the actual indexed **chunk** through
   existing NFC and lowercase semantics, validating transformed bytes. Canonical
   composition/reordering and one-to-many lowercase need covering source intervals.
   Whole-file normalization cannot substitute for chunk normalization. Bound source
   bytes, transformed bytes, map entries, witnesses, aggregate response, deadlines
   and inflight temporaries before allocation/work. No normalization/recall change.
7. L0 owns normalizer module exports and shared/public DTOs. No module/export or
   wire shape has been invented here. L1 was notified of the missing contract and
   owner primitive; exact candidate-call signatures await G0.

## Public-path inputs and remaining tests

Run each renderer fixture through indexed and manual public SDK routes with the
same selected candidate. Assert the fixed source substring appears and has an
exact original-byte highlight. Assert a token `needle` ignores earlier
`needlework`, `NOT blocked OR allow` highlights only the true `allow` branch when
both words occur, and phrase `blue whale` highlights `blue\r\nwhale`.

Still required: oversized focus explicit outcome; pure NOT; false AND inside OR;
path-only and synthetic-label distinction; Unicode canonical reordering and chunk
boundary cases; immutable generation versus mutable-checkout drift; tampered
source/provenance; aggregate budget and cancellation preserving selected hits;
top-k-only work accounting; public SDK wiring and required boundary gates.

Do not label E08 fixed, L4 complete, or public-path VERIFIED from this handoff.
