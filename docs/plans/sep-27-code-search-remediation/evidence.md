# Evidence register

Status: historical diagnostics revalidated for documentation; proposed fixes
NOT_RUN. This document is sufficient to understand the findings without the
temporary scripts. External artifacts provide replay evidence, not doc authority.

The subsequent [engine re-audit](engine-audit.md) adds native/component
counterexamples and corrects the initial implementation inventory. This page
retains the original benchmark evidence rather than relabeling it as new proof.

## Source identities

- Historical clean measurement: `3c6bc0ad3f3dd6d201774255062a509edc2705d6` at
  `/private/tmp/qi-rbr-symbol-boundary-final.x3ddOxQY/checkout`.
- Previous source inspection: dirty `63f09be92fca533bd18d1d71a6464ca30e8073d1`.
- Documentation baseline: clean `66cee47efdda7c5f3886ac58690aa645f44f691f` before
  this packet. On 2026-09-27, the following eleven files were read and compared
  byte-for-byte with the historical snapshot; all were identical:
  `benchmarks/retrieval/src/{main,symbols,batch,sdk}.rs`,
  `crates/quanta-index-lexical/src/searcher/{prepare,port,compile,candidates,paging,restrictions}.rs`,
  `tools/benchmark/retrieval/lexical_file_comparison.py`.
- This does not qualify other source changes or current full-repository behavior.

Runner SHA256:
`2a3fb0dbb40dc716cc465e4605c727381b0d28a68ba5d456466dc3b5dcbcbafc`.
Searchd SHA256:
`faa24fd7e971f585d3c1f6dae656f023edfe418e9b059badad89330455b67db4`.

## Original experiment

Ten frozen repositories, twenty answerable bare-name tasks each: Axios, Black,
ESLint, FastAPI, Gin, ripgrep, Tokio, tRPC, Viper and Vite. These are mechanical
symbol-locator diagnostics, not independently adjudicated general code-search
quality. Quanta used lexical with 1024-byte windows, 256-byte overlap and top-k10;
Semble used native-default hybrid. Other products used their recorded native
profiles. Vite Quanta stopped before queries; its twenty tasks have no score.

| Common 180 tasks | Quanta lexical | Semble hybrid |
| --- | ---: | ---: |
| First returned file contains gold file | 142 | 161 |
| First result fully covers gold span | 83 | 153 |
| Gold file in first ten candidates | 177 | 177 |
| Gold span covered in first ten candidates | 176 | 170 |

The gold defects below limit interpretation. The 59-file/span difference is
not proof that snippet formatting alone can recover every missing definition.
There were 710 Quanta returned chunks, with 393 slots beyond the first chunk
per file within each query. Distinct-file grouping is a different result unit.

## Bounded counterfactual replay

Retained commands:

```sh
python3 /private/tmp/qrca.v5z9/replay.py fastapi
python3 /private/tmp/qrca.v5z9/replay.py ripgrep
python3 /private/tmp/qrca.v5z9/audit.py
```

Each replay identity file retains the full runner command and input/binary
digests. Fresh state was built with the same corpus, binary and chunk profile;
top-k100 and explicit case/file variants plus symbol route were diagnostic
changes. FastAPI emitted 52 result rows; ripgrep emitted 46. Six symbol/file
variants failed. Exit0 for the producer does not make those queries successful.
All forty original lexical queries reproduced their original top-ten path and
indexed-byte-range prefixes exactly.

| Query | Lexical chunk rank | case:yes chunk rank | select:file rank | Both rank | Native symbol rank |
| --- | ---: | ---: | ---: | ---: | ---: |
| Default | 14 | 8 | 7 | 3 | 1 |
| generate_unique_id | 31 | 31 | 3 | 3 | 1 |
| anything | 34 | 34 | 18 | 18 | 1 |

These results rule out missing index entries for these three cases. They do not
establish a global symbol-route gain, file-grouping win or new default ranker.

## Concrete failed invariants and oracle defects

### F01: symbol/file projection

On the symbol endpoint, `select:file Default`, `select:file generate_unique_id`
and `select:file anything`, with and without `case:yes`, return
`INTERNAL: lexical: stored symbol doc missing symbol_kind field`.

`searcher/prepare.rs` starts the domain accumulator at None, allows select:file
to choose Text, then falls back to Symbol only if no explicit domain was set.
`searcher/port.rs` nevertheless decodes the result as SymbolCandidate. Empty Text
results can avoid that decoder; this is why no-match cases need negative tests.

### F05/F06: Vite corpus and parser

All 256 admitted file hashes matched the frozen manifest. Pinned tree-sitter
0.25.10 and tree-sitter-typescript0.23.2 parsed 252 TS and four TSX inputs:
253 had no error; three had errors.

| Path relative to Vite | Location | Trigger |
| --- | --- | --- |
| packages/vite/src/node/__tests__/runnerImport.spec.ts | 26:26, byte968 | Trailing comma after generic call with typeof import |
| packages/vite/src/node/index.ts | 289:8, bytes7591..7595 | export type * as HttpProxy |
| packages/vite/src/node/ssr/__tests__/fixtures/errors/syntax-error.ts | 1:1 | Intentional invalid code fixture |

Minimal forms `runnerImport<typeof import('./basic')>(fixture('cjs.js'),)` and
`export type * as HttpProxy from './basic'` were accepted by independent
TypeScript7.0.2 noEmit probes. This is syntax evidence, not a full Vite typecheck.
The malformed fixture is used by the repository's own syntax-error tests.
Whole-file fail-closed symbol extraction before publication is the current
intentional contract; it is not a partial publication bug.

### F07: gold construction

External recipe:
`/Users/songmin/Documents/code-new/qi-rb-coverage-set-2026-09-25/prepare_symbol_pairs.py`.
Its line regex ignores Python string context, omits Rust const fn and misses
TypeScript object methods. A name counted once by that regex need not be unique.

- FastAPI S05 common_parameters, S08 unicorn_exception_handler and S20
  write_notification occur as docstring code examples, not actual declarations.
  All 46 admitted Python files were hashed and AST-inspected; those names had
  zero real declarations. Of forty Black/FastAPI gold cases, 37 matched AST
  declarations. Native symbol lookup abstained on the three examples.
- Tokio is_readable gold at ready.rs omits the legitimate const fn at
  tokio/src/io/interest.rs:86. A valid alternate definition can be scored wrong.
- tRPC encode has an alternate object-method implementation in wsEncoder.ts.

Examples remain valid content targets. Their classification as unambiguous
actual-definition tasks is the defect.

### F08: native/normalized mismatch

Read the FastAPI cs symbol-only raw capture. In memory, keep S02 native stdout
and its metadata unchanged, replace only normalized paths with the gold path
and file_hit_at_10 with true. Pass those bytes to the comparator's normal read
boundary. It accepts a score change from 19/20 to20/20. Original files remained
unchanged. This proves a rejection gap; it does not show actual capture tampering.

### F09 and exclusions

Sourcegraph requested count:all before truncating scored paths; Quanta requested
ten chunks. SDK, worker, process-spawn and HTTP latency boundaries differ.
Sourcegraph stream and OpenGrok map order are observed order without an attested
relevance contract in this capture. Index universe, updates, quiet-host speed
and a corrected independent holdout remain unqualified.

## Artifact inventory

All four hashes below were rechecked at the documentation baseline.

| Artifact | SHA256 |
| --- | --- |
| /private/tmp/qrca.v5z9/audit.json | d78add1f8f05b33398bbaf8a05d40eb1c3c297294ce2a0e422588023b3caef15 |
| /private/tmp/qrca.v5z9/fastapi/record.json | da462d6bba0ababce5b63a67c73d22d04a346633d47e035ddc5cc18eec29605c |
| /private/tmp/qrca.v5z9/ripgrep/record.json | af43464e49337a668e6fa0f1810d0a01522c55bcf63c98574aa85e1b5345d32b |
| /private/tmp/qi-full-lexical-20260927/full-200-diagnostic.json | 2cabdfa8a4d282b5b3c238bd4732b227627bf6da8ab4cf71699b0a0206584177 |

Full replay commands: `/private/tmp/qrca.v5z9/{fastapi,ripgrep}/identity.json`.
Corpus/input root: `/Users/songmin/Documents/code-new/qi-rb-coverage-set-2026-09-25`.
Frozen clone root: `/private/tmp/qi-full-lexical-20260927/full-repos`.
Original producer roots: q0 Axios, q1 Black, q2 ESLint, q3 FastAPI, q4 Gin,
q5 ripgrep, q6 Tokio, qa tRPC, qb Viper under `/private/tmp`; Vite failure qc.staging.

If these temporary artifacts are unavailable or changed, replay of their
historical claims is BLOCKED. Rebuild the documented fixtures and create new
evidence; do not substitute an unbound file with a similar name. RFC implementation
must not require temporary paths. Large native bytes and corpora stay outside Git.
