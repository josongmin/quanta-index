# L3 handoff — exact names, source grouping, native collection admission

State: **VERIFIED for the selected L3 owner/consumer scope**. Whole-repository,
installed-product, performance and RSS qualification remain **NOT_RUN**.
HEAD: `98601a66d8cab9c86232b3e62ce490c8b43b71b6` on shared dirty `main`; no commit/push/reset performed.
Bound source SHA256: `386ffd3088c14f7c41314c206e62726804c21764c3b37e910a69d5a20dc44c66`.
The complete dirty inventory, toolchain/environment, source manifests, commands,
raw terminal results and binary/artifact digests are in `L3_HANDOFF.source.json`.

## Audit repairs

1. Exact predicates existed in the compiler but failed earlier in predicate
   preparation. All three entry points now preserve the canonical exact leaf:
   top-level preparation, extraction and Boolean lowering. The manual route
   reads the same sealed document's dedicated normalized name field, refuses
   missing/duplicate/noncanonical authority and uses the existing NFC/case policy.
   Exact local/qualified names preserve overloads and nested definitions. Broad
   `symbol.has.name` and ordinary content search retain their separate behavior.
2. Ranked key decoding could allocate before the byte ledger learned its length.
   A four-byte selected path could reconstruct a 3,000-byte predecessor in its
   compressed SSTable block. The canonical vendored SSTable decoder now reserves
   encoded IO, bounded decode buffers plus the bulk zstd context, and exact output
   before each allocation. Temporary guards overlap retained output creation;
   output guards follow the row. Unknown/overflowing bounds and malformed delta
   lengths fail explicitly. No invented producer key limit or wire-format change.
3. SDK test fixtures still used the old surface-scoped replacement/tombstone
   signature. They now supply canonical `SourceFileCoverage` / `SourceFileKey`,
   explicit symbol completeness, the unit-set digest, and producer event identity.
   Missing-event and unsealed requests are tested for refusal before transport.
   The observation fixture now carries seal timing and rejects missing seal
   timing; the chunk fixture has a byte/line span consistent with its text.
   Their source hashes are
   opaque transport-fixture attestations, not source-byte verification. The CLI
   mock now explicitly rejects the new process-request-events control variant.
   Production SDK/control behavior is unchanged by these fixture repairs.

## Preserved contracts

- Containing repo/revision/generation pin remains separate from source repo and
  source-file revision/hash. Candidate/source/preview identity must agree.
- Shared order: score descending; source repo, path, start line, end line and
  candidate ID ascending. Cursor carries source repo. The containing pin and L1
  query/options/route/order/cap continuation bindings remain authoritative.
- File/path projection groups by `(source_repo,path)`; repo projection means
  source repo. Native exact grouping retains best-hit representatives, stable
  ties, late-segment winners and global merge. Scores are not summed.
- One shared sticky work/byte ledger per native collection covers segment visits,
  group/map admission, heap/row/guard buffers, merge/sort scratch, decoded keys
  and in-flight fruits. Refusal discards partial fruits instead of claiming exact
  exhaustion. Cancellation/deadline behavior remains separate and typed.
- Native TermQuery BlockWAND remains, with conservative `doc_freq + 1` upfront
  work admission. Generic/Boolean weights use bounded scorer traversal: their
  former block-skipping performance is not claimed or measured.

## Current behavioral proof

Command, with `QUANTA_INDEX_RESOURCE_ADMISSION=1 CARGO_BUILD_JOBS=2`:

```sh
./scripts/cargow --lane test-fast-lane test -p quanta-index-lexical -p quanta-index-sdk -p quanta-index-searchctl --lib --test execution_budget --test ranked_pages --test l3_exact_source --test sdk_binding_owner_v1 --test cli_smoke --locked
```

Result: **373 passed, 0 failed**, in the selected target set recorded by
`rr-owner-final-4`. Relevant source/dependency/config inputs were stable during that
run and matched the final snapshot. Unrelated checkout drift, if any, is listed
separately in the machine receipt. Formatting, diff whitespace and vendored
provenance digests also passed their recorded checks.

Independent oracles cover exact-field and Boolean behavior on indexed/manual
routes; fixed source/path/revision identities and paginated cardinality; shuffled
insertion/segment order; late best hit; native scorer/advance counters; forced
work/byte refusal; guard release; decoded fixed keys across blocks; malformed
headers, varints and suffixes; broad/content positive controls. SDK/CLI tests use
stub or mock transports and do not constitute installed searchd integration.

Earlier failed, drifted and passing executions remain archived under
`l3-proof/current/`. They are historical diagnostics and are not added to this
run's pass count. In particular, `rr-decode-2` passed 27 focused cases but had
source drift while waiting; the final stable run supersedes it.

## Files and dependency ownership

The implementation delta is in lexical `symbol.rs`, `ranked_page.rs`,
`ranked_page_tests.rs`, `searcher/predicate_plan.rs`, `searcher/manual_scan.rs`,
`searcher/paging.rs` and `tests/l3_exact_source.rs`, plus the existing shared
source/cursor/budget owners named in the source receipt. The narrow SDK/CLI
fixture migrations are listed separately there. Other concurrent edits were
preserved. Root Cargo.toml/Cargo.lock select the local SSTable 0.3.0 patch;
`vendor/tantivy-sstable/QUANTA.md` explains bounds, provenance and upgrade costs.
The old exact-hook patch files are retained only as historical proposals; their
JSON metadata marks them superseded by the applied implementation.

## Explicit exclusions and remaining integration work

- The collection ledger does not bound complete request preparation, manual scan
  materialization, native index/query caches, required output payload retention,
  allocator metadata/realloc internals or process RSS. Those are outside this
  native-collection L3 claim; no request-wide memory-completion claim is made.
- Selected source-stable tests do not prove every repository test, installed
  daemon/SDK wiring, clean-commit qualification, activation or deployment.
- Ranking weights are unchanged. Ranking quality, holdout comparisons, latency
  and memory-performance promotion require separate measurements.
- The dependency patch increases local maintenance cost. Revalidate codec/zstd
  bounds and the pinned Vec growth assumptions when upgrading Tantivy/Rust.

No unresolved defect reproduced within the final selected L3 scope remains.
This is a scoped correctness closeout, not whole-engine qualification. Inter-task
sending, reading, waiting and subagents were not used during this continuation.
