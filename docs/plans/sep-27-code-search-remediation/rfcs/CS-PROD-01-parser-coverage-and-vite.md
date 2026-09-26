# CS-PROD-01 — Parser coverage and Vite admission

Latest code audit: [L5_FINAL_CODE_AUDIT](../handoffs/L5_FINAL_CODE_AUDIT.md).
The older execution totals below remain bound to the original completion snapshot.

Status: **VERIFIED for the L5 producer and shared integration scope** on the
frozen dirty snapshot at `98601a66d8cab9c86232b3e62ce490c8b43b71b6`.
Exact source identities, commands and raw evidence are in
[L5_COMPLETION.json](../handoffs/L5_COMPLETION.json) and the
[handoff](../handoffs/L5_HANDOFF.md). Whole-repository CI and clean-source release
qualification are **NOT_RUN**. External production producers are outside this scope.
Category: producer compatibility. Findings: F05/F06.
Parser probes are independent; publication changes depend on ENG-02.

## Purpose and RCA

The pinned producer uses tree-sitter 0.25.10 and tree-sitter-typescript 0.23.2.
Original Vite census: 256 admitted files, 252 TS and four TSX; 253 clean parses and three
failures. All file hashes matched the frozen manifest. Two failures are accepted
by an independent TypeScript 7.0.2 syntax probe; one is an intentional malformed
test fixture. See [evidence](../evidence.md) for file paths and byte positions.

Minimal valid forms to preserve as regressions:

```typescript
runnerImport<typeof import('./basic')>(fixture('cjs.js'),)
export type * as HttpProxy from './basic'
```

The independent probe establishes syntax acceptance, not a full project typecheck.
The malformed fixture should remain malformed and searchable as source text.
Deleting it from the corpus to obtain a green symbol run changes the universe.

## Decision

1. Add a deterministic all-file parser preflight that reports every admitted
   file's language, source hash, capability, parse status and diagnostic spans.
2. Use the census to choose a pinned compatible grammar/runtime combination or
   a narrowly justified upstream-compatible grammar patch. A version upgrade is
   a candidate fix, not proof that either construct becomes correct.
3. Retain strict ERROR/MISSING-node detection for complete-symbol claims. Do not
   suppress errors globally or interpret an error-recovered tree as complete.
4. Publish text with explicit symbol failure/unsupported state only through the
   ENG-02 capability contract. Strict all-symbol profiles still refuse coverage
   gaps before claiming success.

Preflight results are source-bound producer facts. Stable sort diagnostics by
path and byte range. Distinguish unsupported syntax/language, syntax error,
extractor invariant failure, crash, timeout and cancellation. Bounded diagnostic
output must disclose truncation and retain total failure counts; fail-fast CLI
behavior alone is not a full-corpus coverage report.

## Owner and payload

- [Symbol producer](../../../../benchmarks/retrieval/src/symbols.rs): parser setup,
  grammar capability and extraction invariants.
- [Runner](../../../../benchmarks/retrieval/src/main.rs): profile admission and
  preflight invocation; do not hide failures in a successful zero-result record.
- [Batch construction](../../../../benchmarks/retrieval/src/batch.rs): complete
  unit set plus ENG-02 state, all bound into the publication scope digest.
- [Producer dependencies](../../../../benchmarks/retrieval/Cargo.toml) and root
  lockfile: pin runtime/grammar/extractor identity for reproducible receipts.

Production searchd remains a consumer of facts; this RFC does not move a parser
into the engine. Any real external producer must implement the same payload
contract independently. A benchmark-only parser fix is not proof that a sibling
production producer has been upgraded.

Record grammar version/commit, runtime version, lockfile/source digest, enabled
language capabilities and extraction policy. Valid files with no declarations
produce Complete with zero facts. No parse failure may emit that same state.

## Regression strategy

- Store small self-contained valid constructs with independently expected AST
  acceptance and symbol outputs. Avoid copying full Vite source into fixtures.
- Verify both minimal examples and their frozen repository files. Assert no
  spurious symbol identities, offsets or kind changes from parser recovery.
- Keep the intentional malformed fixture as a negative; assert text availability
  and strict symbol refusal according to the selected profile.
- Exercise all supported languages, especially language-specific owner/kind
  cases already protected by SEP-26-001. A TS fix cannot weaken Rust/Python rules.
- Inject a failure in the last file: preflight enumerates the complete census,
  and a strict profile does not boot/publish a supposedly complete index.
- Test grammar identity mismatch, changed source bytes, timeout and malformed
  preflight records. Neither old successful receipts nor a parser version label
  satisfy current-source publication evidence.

## DoD

- [x] Both independently valid forms parse and extract correctly under pinned
  dependencies; their acceptance and expected facts have independent oracles.
- [x] Intentional invalid source is retained and has truthful capability state.
- [x] Full frozen Vite inventory is accounted for exactly once, with no hidden
  exclusions or zero-filled failure rows.
- [x] Supported-language extraction regressions and payload rejection tests pass.
- [x] Lexical Vite queries execute through the actual capability-enabled profile;
  strict symbol coverage remains separately reported, not mislabeled complete.
- [x] Grammar/producer identity, registry and source-closure receipts are renewed
  for the exact dirty snapshot; this does not bypass the clean-source gate.
- [x] Shared publication/lifecycle tests pass after integration with ENG-02.

The final census is 255 Complete files and one intentional ParseFailed file,
with all 256 admitted files retained. Verification covers 132 Rust owner tests,
20 SDK process tests, 64 shared lifecycle tests, 469 Python tests and six real
Vite process probes. Strict Vite preflight still refuses the malformed fixture
after emitting the complete census. Later shared Python consumer changes are
bound to separate compatibility receipts; frozen producer proof is not rebased.

No manual relabeling or legal approval is an automatic coding prerequisite here.
If no compatible grammar can be established, retain the typed failure and report
the exact blocker; do not lower the completeness guarantee. Tree-sitter recovery
semantics: [S06](../references.md). Rollout: [CS-INT-01](CS-INT-01-integration-and-qualification.md).
