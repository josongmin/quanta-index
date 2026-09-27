# L1 final code-first audit — semantic lexical scope

> Historical report: one-off evidence files were removed from the repository. This report alone is not current verification.

Status: **VERIFIED** for the exercised L1 implementation and actual daemon/SDK
path at the frozen process source. This does not qualify the concurrent shared
checkout, whole-repository CI, learned semantic relevance, performance, or
release/deployment.

## Source boundary

- HEAD: `2102966246866398f01833bebf71396831377149` in both the shared
  checkout and isolated worktree
  `/Users/songmin/.codex/worktrees/l1-final-audit/quanta-index`.
- The eight L1-owned product/test files in the isolated input manifests match
  the shared checkout byte-for-byte. The isolated worktree also carries
  minimal clean-HEAD compile/lint repairs outside L1; these are listed in
  `l1-proof/final-semantic-scope-20260927/isolated-inputs-final.json`. They are
  not a qualification of the concurrent shared tree.
- The 470-test dispatcher run, daemon build, and actual SDK process run used
  `isolated-inputs-process.json`. Subsequent edits to two test files replace
  direct indexing/closure syntax for Clippy; they do not change product code
  or test assertions. Later isolated-only baseline lint repairs are separately
  identified in `isolated-inputs-lint.json`.

## Reproduced defects and repair

| Root cause | Before | Repair |
| --- | --- | --- |
| Empty lexical allowlist still dispatched dense search | A contradictory scope invoked the semantic adapter and claimed executed zero-hit instead of logical empty. A valid lexical zero-hit scope also issued an unnecessary dense call. | Embed and validate the query/model/vector, then return before dense invocation. Derive the window and explanation from actual lexical/dense calls: logical empty for contradiction, lexical-only executed zero-hit for a searched empty scope. |
| Scoped adapter result was trusted | A recording semantic adapter could return an ID outside the lexical allowlist and the route would publish it. | Reject any scoped dense row whose ID is not in the lexical allowlist before building the response. |
| Empty-scope shortcut could bypass generation vector validation | Native dense search previously validated dimension, finiteness and normalization; an early return would skip that validation. | Make `SemanticSearcher::validate_query_vector` mandatory, implement it using the opened native generation's validator, and invoke it in the common embed/model gate. |
| Semantic window omitted or misstated scope execution | A live scoped query reported only the dense lane in window coverage. A nonempty lexical scope with zero dense rows could mark lexical as contributing despite an empty response. | Emit both scope and dense lane traces, bind the exact selected scope-ID count, and mark scope contribution only when a final row survives. Contradictory and lexical-zero windows also name both lanes without claiming unexecuted work. |

The two behavioral RED receipts remain in the proof directory: empty scope
`0/1` (`lexical=0 semantic=1`) and live scope lane omission `0/1` (only
`semantic.dense` reported). Recording-adapter regressions cover valid zero-hit,
logical contradiction, dense zero-hit after nonempty lexical scope, invalid
embedding/vector, and out-of-allowlist rows.

Final caller census: `semantic.rs` is the only dispatcher caller of scoped
constrained dense search and the only caller of `semantic_window_v2`.
`embed_and_gate_query` is shared by semantic, hybrid, hybrid seed, and hybrid
explain. The required `SemanticSearcher` method has exactly three repository
implementations: native persisted search, the dispatcher recording searcher,
and the searchd resident-only test double.

## Verification

| Rail | Exact command or action | Result | Scope |
| --- | --- | --- | --- |
| Native L1 control | `./scripts/cargow test --locked -j 2 -p quanta-index-lexical --test l1_query_domain_window --message-format=json` | **VERIFIED**: 18/18 | Earlier shared source; unchanged by this semantic repair. |
| Dispatcher whole lib | `QUANTA_INDEX_RESOURCE_WAIT_SECONDS=3600 ./scripts/cargow test --locked -j 2 -p quanta-index-search-plane --lib --message-format=json` | **VERIFIED**: 470/470 | Isolated process-source input; includes new regression cases. |
| Daemon build | `QUANTA_INDEX_RESOURCE_WAIT_SECONDS=3600 ./scripts/cargow build --locked -j 2 -p quanta-index-searchd-runtime --bin quanta-index-searchd --message-format=json` | **VERIFIED**: exit 0 | Frozen binary SHA-256 `d4005f208060189689d189858d3fbe3161c09cda7f6311893f5eebd40d4677df`. |
| Real daemon/SDK | Start that binary with `serve --state-root` on a fresh private root, then `./scripts/cargow test --locked -j 2 -p quanta-index-sdk --test l1_daemon_query_contract --message-format=json -- --ignored` | **VERIFIED**: 1/1, zero ignored | SDK publish/activate and actual Unix IPC query; Native/Sourcegraph, indexed/manual, scoped semantic truth and symbol windows. |
| Product lib Clippy | `./scripts/cargow clippy --locked -j 2 -p quanta-index-core -p quanta-index-semantic -p quanta-index-search-plane --lib --no-deps --message-format=json -- -D warnings` | **VERIFIED**: exit 0 | Isolated lint input, including separately identified baseline repairs. |
| L1 SDK test Clippy | `./scripts/cargow clippy --locked -j 2 -p quanta-index-sdk --lib --test l1_daemon_query_contract --no-deps --message-format=json -- -D warnings` | **VERIFIED**: exit 0 | Only SDK lib and the selected L1 process test. |
| Searchd lib | `./scripts/cargow test --locked -j 2 -p quanta-index-searchd --lib --message-format=json` | **VERIFIED**: 92 passed, 0 failed, 1 ignored | Compiles and runs the searchd test-only `SemanticSearcher` implementation; the one ignored test is outside this invocation. |
| Broad `--tests` Clippy | `./scripts/cargow clippy --locked -j 2 -p quanta-index-core -p quanta-index-semantic -p quanta-index-search-plane -p quanta-index-sdk --lib --tests --no-deps --message-format=json -- -D warnings` | **FAILED**: exit 101 | Unrelated SDK L2 publication test has six lint failures; core `ingest_resource_policy` uses stale fields, and unrelated ingest tests have lint failures. Raw failed logs retained. |
| Static checks | `rustfmt --check --edition 2024` on the eight L1 files; scoped `git diff --check` | **VERIFIED** | Formatting and whitespace only. |

The real SDK test uses explicit `QUANTA_INDEX_EMBEDDER=hash-dev`, a fresh
`QUANTA_INDEX_L1_TEST_STATE_ROOT`, and explicit search-corpus retention caps.
The fixture publishes both lexical chunks and semantic source scopes under the
same IDs. An earlier SDK RED was a missing semantic-source test fixture, not an
observed product defect; its failure remains separate from the successful run.

Raw JSONL/stderr, input manifests, isolated patch, process receipt, executable
hash, and deterministic compressed evidence are in
`l1-proof/final-semantic-scope-20260927/`.
The machine-readable command/status and artifact-digest index is
`L1_FINAL_AUDIT.json`.
The 342 MiB executable remains at `/tmp/qi-l1-final-process3/searchd-proof-bin`;
it is not copied into the repository.

## Exclusions and integration boundary

- The shared checkout contains concurrent L2–L5 changes. Its complete dirty
  source and whole-repository CI are **NOT_RUN** as a frozen aggregate.
- One searchd lib test remained ignored by the selected command; its behavior
  is outside this L1 proof.
- End-to-end relevance quality, ANN performance, fault-injection matrices,
  clean-commit integration, release and deployment are **NOT_RUN**.
- The required trait method is a pre-release breaking API change for any
  external `SemanticSearcher` implementer.
