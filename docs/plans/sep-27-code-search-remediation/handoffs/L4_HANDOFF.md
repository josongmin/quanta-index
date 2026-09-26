# L4 handoff — implementation in progress, native lexical proof pending

Status: **NOT_RUN** for lexical behavior and public adapter regressions. This is
an interim handoff, not L4 completion. Initial four lexical commands stopped
before test execution on compile errors; they are not behavioral RED evidence.

## Source and ownership

- Shared dirty `main`, HEAD `106d7abec2dd3fa03f9db5a19a3de41df2f0afad`.
- Original owned files: lexical `searcher/snippets.rs`, `searcher/match_sets.rs`,
  normalizer `provenance.rs`/`tokens.rs`, regex `executor.rs`, lane tests.
- L0 explicitly granted the entire `searcher/candidates.rs` conversion owner.
- Public DTO, source schema/writers/exports/request-budget carrier are L0/L3
  changes. Manual selection is L1; indexed paging is L3. No new agents, commits,
  resets, or pushes were created by L4.

## Implemented behavior awaiting native validation

- Bounded regex ranges use the same compiled executor as membership verification.
- Token/phrase ranges share `phrase_ranges` with existing phrase truth; raw
  substring truth and `MappedText.find_substring` share the canonical normalizer
  helper. NFC/case matching semantics are preserved.
- Private original/NFC/folded interval provenance verifies the immutable raw chunk
  against indexed NFC. Composition, reordering, Hangul, expansion and chunk
  normalization boundaries have fixed source-byte regression oracles.
- Request-local Boolean recomposition keeps positive witnesses only from matched
  branches. NOT and false branches roll back witnesses and optional-span flags.
- Selection/ranking uses identity-only candidates. Only selected stored document
  addresses are rendered. Content-filter leaves participate as mandatory ANDs.
- Raw bytes come from the selected stored snippet, indexed text from that stored
  row bound to its immutable text-authority member. No checkout file is opened.
- Source owner is the stored repository; containing snapshot pin stays separate.
  Bound source revision/hash must match sealed coverage. Missing/partial/duplicate
  identity fields fail integrity. An explicitly unbound generation may return
  `SourceNotProvided`; no revision/hash is fabricated.
- Short fitting focus takes precedence over context within 240 bytes. Oversized,
  zero-width, no-positive, missing-source and budget outcomes are explicit.
  Path and synthetic labels carry no source excerpt coordinates.
- A separate canonical preview ledger is shared by the served request: 10,000,000
  work units, 64 MiB logical reservations. Per-source 64 KiB, transformed 128 KiB,
  map entries 262,144, positive witnesses 32. This is not an RSS/latency claim.
- Request-local regex compilation uses canonical `RegexExecutor` and retains a
  conservative 16 MiB logical compiler/cache/HIR allowance per distinct pattern.
  Existing regex document-bitmap cache remains unchanged; no global compiled or
  whole-query witness cache was added.
- Output leases include guard-vector capacity and transfer to RequestBudgetV1
  through L1/L3 helpers. Root reserves output-group slots before rendering;
  exhausted slots produce optional WorkBudget without losing hits. Direct callers
  must retain the budget while using outputs; arbitrary DTO clones are excluded.

## Explicit limitations

- Residual metadata/structural leaves without a bounded per-row truth API return
  optional `UnsupportedRange`; they are not guessed or rescanned unboundedly.
- Full-file source SHA is producer attestation sealed with the generation.
  Chunk raw SHA is computed independently at ingest. This does not prove the
  producer's whole-file bytes independently.
- Required identity/extent/text-authority fields are checked before optional
  preview refusal. Raw SHA/NFC comparison happens only when bounded preview work
  runs; a budget-refused excerpt carries no positive integrity claim.
- Regex engine searches and one normalization sort are not preemptible internally;
  their inputs are bounded and cancellation is checked around them.
- Candidate identity storage, transport DTO cloning, process RSS, public SDK,
  fresh-process E2E, workspace qualification and deployment are not proven here.

## Evidence status

`l4-proof/standalone-final/receipt.json` records earlier 78 regex + 18 normalizer
unit tests, package clippy and format checks, with bound source/binary digests.
Normalizer changed afterward to share substring truth/range extraction; this
receipt is historical and must be regenerated (97 expected unit tests).

Initial lexical command in each `l4-proof/lexical/red*.log`:

`CARGO_BUILD_JOBS=2 ./scripts/cargow test -p quanta-index-lexical --lib l4_witness_regressions --locked`

All four commands exited 101, executed 0 tests: respectively unused owned import,
shared coverage dead-code wiring, shared cursor unused validation results, and
shared predicate lookup name mismatch. Owner fixes are now written. Native
execution is paused while L0 holds the serialized Rust slot for its shared gate.

Pending proof: meaningful old-renderer RED, same nine fixed cases on the new
renderer GREEN, six renderer cases, stored-field corruption case, normalizer/regex
revalidation and `tests/l4_match_anchored_preview.rs` sealed adapter tests.
The latter covers indexed/manual Unicode and regex focus, mutable checkout drift,
path-only and oversized focus, synthetic labels, and the 257th request-output
slot preserving the hit. Native adapter proof is not public SDK proof.
