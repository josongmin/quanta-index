# L4 further adversarial audit — cancellation, regex memory, SDK preview

Status: **partial**. The source-bound selected checks below passed. Aggregate
regex compiler/retained heap admission remains **BLOCKED**; a fresh installed
daemon/process, whole-workspace qualification, latency and RSS are **NOT_RUN**.
This audit does not promote a logical reservation or an in-process SDK route to
those claims.

## Source and execution boundary

- Live checkout base: `2102966246866398f01833bebf71396831377149` with concurrent dirty
  L1/L2/L3/L5 and runtime changes. No commit, push, reset, outbound task
  message, task polling or cross-agent coordination was performed.
- Final copied source: SHA-256
  `fd2bef1682589161f7688e81c6a5d7fa8b244da1ab234796f2ed599e097e0e4c`
  over 1,172 tracked and untracked non-plan input files. The 25 pinned external
  SSTable input files are hashed separately. The exact manifest, toolchain,
  before/after inputs, commands, binaries and raw logs are in
  [further-audit-20260927](l4-proof/further-audit-20260927/).
- The copied tree is immutable during each recorded run. It is an owner-local
  verification snapshot, not a clean Git commit or whole-repository receipt.
- At closeout, all 11 selected L4/SDK/harness files still matched the copied
  source and the 25 pinned external inputs were unchanged. Five other live
  checkout inputs had changed after the copy, including a lexical execution
  budget integration test and L1/search-plane files. Thus this receipt does
  not qualify the entire current shared checkout. The per-file comparison,
  exact drift list, artifact hashes and `git diff --check` result are in
  [closeout.json](l4-proof/further-audit-20260927/closeout.json).

## Reproduced findings and changes

1. **P2 — final-candidate cancellation leaked a complete result.**
   `RegexExecutor::execute_interruptible` checked before each candidate but
   returned without checking after the last resolver/verification. A resolver
   that canceled the request while returning the final document yielded
   `Ok([DocId(1)])` in the focused RED. The executor now checks interruption
   before publishing, including an empty candidate list. The regression
   requires `Interrupted` and discards the verified prefix.
2. **P2 — prefilter size amplified regex result memory.** Exact verification
   reserved `candidates.len()` output slots before any candidate was examined.
   A broad, entirely false prefilter therefore held an unnecessary result
   allocation and performed it before the first interruption check. Output
   starts empty and uses fallible reservation only for verified IDs; allocation
   refusal remains typed and no partial result escapes. The four-document
   all-unmatched regression checks zero retained result capacity.
3. **P2 — regex match-cache charge understated reachable sparse sets.** With
   pinned roaring 0.11.4, a diagnostic `System` allocator probe measured
   45,064 live bytes after 513 sparse containers while the prior cache charge
   was 24,656 bytes. The same undercount occurs for 3 and 1,025 containers;
   raw probe source, output and hashes are retained. The per-container charge
   now includes conservative growth headroom. Run stores are refused for cache
   retention because the dependency reports serialized run length but not the
   retained interval-vector capacity. The caller still returns its verified
   match set after a cache refusal. The old serialized-size unit oracle was
   relabeled; it was not a retained-heap oracle. This closes the reproduced
   sparse-vector undercount, **not** an exact total-heap/RSS guarantee for
   cache map nodes, in-flight `Arc` references or compiler allocations.
4. The shared harness source-publication refactor initially failed the SDK
   target's compile due to denied unused mutation results. Explicit ignored
   bindings restore compilation without changing mutation control flow.
5. A dedicated SDK integration target verifies selected source-bound previews
   through the SDK and query socket: folded keyword, regex range, decomposed
   NFC source, 200-byte fitting focus, 241-byte explicit refusal, and path-only
   result with no fabricated content coordinates. Expected byte spans and
   emitted raw slices are independent fixture oracles.
6. **P2 — incomplete typed multi-hit highlights.** The public
   `LexicalCandidate.highlights` contract requires every matched-hit range
   within the emitted snippet. The selected witness evaluator returned only
   the first range per keyword/phrase, raw substring and regex leaf. The
   seeded UI rail's three-occurrence case therefore received one span.
   Each leaf now uses its canonical matcher to gather a bounded range set;
   Boolean false branches and NOT still discard their witnesses. A 33rd
   positive witness produces explicit `WorkBudget` unavailability instead of
   a misleading partial span list. Native regressions cover all four leaf
   classes and the cap; the SDK target asserts all three typed spans on a
   separate multi-hit source.
7. **P2 — verify-only regex fallback bypassed candidate admission.** A usable
   trigram prefilter refuses more than 100,000 candidates, but its unusable
   fallback eagerly collected every authority document ID without that cap or
   an enumeration-time cancellation check. The fallback now applies the same
   candidate limit, fallible slot reservation and a request checkpoint before
   advancing the iterator. The boundary regression covers exactly 100,000,
   100,001 and cancellation before the first iterator step. The regex cache-hit
   and completed-verification paths also recheck the request before returning;
   the interval probe alone cannot see cancellation in its final few candidates.
8. **P2 — cache-entry overhead omitted a second B-tree key.** The 128-byte
   per-entry charge was smaller than the inline key/value items stored across
   the resident and recency indexes, before node slack or `Arc` metadata.
   Admission now charges 1,024 bytes per entry, and a regression checks the
   minimum inline-item floor against the actual compiled types. This repairs
   the definite structural undercount; a measured total allocation ceiling
   for both B-trees remains outside this source-local proof.
9. **Verification repair.** The owner lint run exposed long first doc
   paragraphs and an uninlined format argument in the new code. It also found
   two private manual-scan functions with nine parameters and a wildcard
   `LqFilter` arm introduced by concurrent checkout work. Narrow lint
   expectations document the existing signatures; the arm now enumerates its
   unaffected variants so a new filter cannot silently take that path.

## Verification

| Scope | Exact command | Result |
| --- | --- | --- |
| Owner | `./scripts/cargow test -p quanta-index-lexical -p quanta-index-lq-regex -p quanta-index-lq-text-normalizer --lib --test l4_match_anchored_preview --locked` | **VERIFIED**, exit 0: 193 lexical + 9 L4 integration + 82 regex + 21 normalizer = 305 tests |
| Cache/cancellation integration | `./scripts/cargow test -p quanta-index-lexical --test regex_cache_bounds --test cancellation_inside_search --locked` | **VERIFIED**, exit 0: 7 integration tests |
| UI rail | `./scripts/cargow test -p quanta-index-searchd-harness --lib seeded_ui_rail_runs_and_anchors_every_probe --locked` | **VERIFIED**, exit 0: 1 seeded three-hit rail |
| SDK query socket | `./scripts/cargow test -p quanta-index-searchd-runtime --test l4_preview_sdk --locked` | **VERIFIED**, exit 0: 1 integration test, seven query scenarios |
| Regex/normalizer lint | `./scripts/cargow clippy -p quanta-index-lq-regex -p quanta-index-lq-text-normalizer --all-targets --locked --no-deps` | **VERIFIED**, exit 0 |
| Owner lint | `./scripts/cargow clippy -p quanta-index-lexical -p quanta-index-lq-regex -p quanta-index-lq-text-normalizer --lib --test l4_match_anchored_preview --locked --no-deps` | **VERIFIED**, exit 0 |
| SDK lint | `./scripts/cargow clippy -p quanta-index-searchd-runtime --test l4_preview_sdk --locked --no-deps` | **VERIFIED**, exit 0 |
| Format | `rustfmt --edition 2024 --check` on changed Rust files | **VERIFIED**, exit 0 on final source |

Every row above with `VERIFIED` binds the copied source hash and unchanged
inputs to its raw log and executed binary when applicable. Resource admission
used the repository lock, `CARGO_BUILD_JOBS=2` and a recorded wait limit.

## Remaining claims

- **BLOCKED:** The fixed preview executor charge does not prove an aggregate
  upper bound for regex AST/HIR/compiler temporaries, retained forward/reverse
  automata and engine caches. See [L4_REGEX_BUDGET_RESIDUAL.md](L4_REGEX_BUDGET_RESIDUAL.md).
- **NOT_RUN:** A fresh installed daemon/process, whole-workspace CI, benchmark
  p95/context-utility/RSS, release and deployment. The SDK test starts an
  in-process daemon driver with real sockets; it does not prove a restart or
  installed binary.
- The regex cache's `resident_bytes` is an admission charge. The sparse
  undercount above is corrected for the pinned observed layout, but total
  allocation of map nodes and externally retained entries is not established
  by that counter. Do not present it as process RSS.
