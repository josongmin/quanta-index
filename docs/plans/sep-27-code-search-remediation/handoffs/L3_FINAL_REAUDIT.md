# L3 final adversarial code audit — 2026-09-27

**VERIFIED for the selected lexical source and seven test suites only.** HEAD
`2102966246866398f01833bebf71396831377149` on dirty `main`; no commit,
push, reset, agent dispatch, or inter-task message. The exact dirty-path list,
source/config/dependency hashes, command, environment, test binaries, and raw
results are in [source-closure-current.json](l3-proof/final-reaudit/source-closure-current.json).
Older receipts in this directory refer to earlier source and do not qualify the
current checkout.

## Root causes and code repairs

1. **Request cancellation bypassed retained-buffer admission.** The native
   traversal checked interruption, but segment-fruit, ranked-row, and guard
   carriers reserved against the raw resource ledger. A cancelled request
   could allocate or surface a byte refusal instead of `REQUEST_CANCELLED`.
   `CollectionBudget` now shares a request interruption authority across
   traversal and all those carriers; native visit charging uses the same
   checkpoint. Deterministic tests cover cancellation before fruit and
   retained-buffer reservation, no subsequent work/retention, and an
   unpoisoned resource ledger. Cancellation remains cooperative at checks.
2. **Stored authority decoded first values or skipped malformed rows.** Manual
   `index:no` matching could replace missing canonical `chunk_text` with a
   snippet/empty string; candidate/symbol, predicate, restriction, and text
   authority readers could select first duplicate or skip a missing identity.
   Required text, document kind, symbol fields, and numeric lines now reject
   missing, malformed, duplicate, unknown, or out-of-range storage as
   appropriate. Owner/contributor scans restrict to text docs and check that
   stored kind agrees with the indexed kind. Negative tests cover the
   ambiguous storage and exact-source cases.
3. **`index:no` language matching inferred metadata from the filename.** The
   schema indexes `language` but does not store it, so a manual stored-document
   scan cannot reproduce an explicit language filter. Explicit `lang` filters
   and scoped content predicates with a language constraint now return typed
   `NotImplemented` before scan or early empty-plan return, including dense
   admission and candidate explanation. Indexed language queries retain their
   indexed behavior. A public regression uses `.rs` with `python` metadata and
   `.py` with `rust` metadata; it also checks an empty path scope. Supporting
   this on `index:no` would require a coordinated stored-language schema change
   and rebuild, which this repair does not claim.
4. **Whole-set paths bypassed native work/byte accounting.** Seven
   predicate/restriction preludes used `TopDocs::with_limit(num_docs)`, and the
   `index:no` scan used the full corpus size. Dense admission used the input
   candidate count without the lexical examined cap. These paths now use the
   bounded whole-set collector, check cancellation while decoding, and refuse
   a dense input set above the examined limit. Regressions cover five matching
   documents against an examined cap of four, an `index:no` collection byte
   cap, and over-cap dense admission. A refusal returns no partial exact set.

The production stored-field reads in the inspected lexical paths use strict
unique decoders; remaining `get_first` uses found by source search are test
oracles. Inspection covered the modified collector, manual, predicate,
authority, candidate, and dense-admission call paths. No further reachable
P0–P2 defect was identified in that inspected scope. This is a bounded audit,
not a universal defect-free claim.

## Current-source proof

The direct wrapper command exited 0 with **260 passed, 0 failed, 0 ignored**:
193 library, 3 cancellation, 7 execution-budget, 6 exact-source, 5 ranked-page,
38 Tantivy smoke, and 8 Unicode golden tests.

```sh
QUANTA_INDEX_RESOURCE_WAIT_SECONDS=3600 QUANTA_INDEX_RESOURCE_ADMISSION=1 CARGO_BUILD_JOBS=2 \
./scripts/cargow --lane test-fast-lane test -p quanta-index-lexical \
  --lib --test execution_budget --test ranked_pages --test l3_exact_source \
  --test cancellation_inside_search --test unicode_normalization_goldens \
  --test tantivy_smoke --locked
```

The execution used `rustc 1.92.0` on `aarch64-apple-darwin`. The last snapshot
before the resource-admission marker, before/after command snapshots, post-run,
closeout, and post-report captures agree on all **369 selected source/config/dependency
files**: canonical SHA-256
`9d1e99ab7a7aa022fc4753193be9e6583a37350763661fca1b4b79a998edea55`.
The broader captured 1,052-file map also remained unchanged over the test run.
After closeout, two excluded sibling test files changed concurrently; their
paths and the later stable selected-input check are recorded in the closure.
The selected closure covers nine resolved workspace dependency crates, pinned
`tantivy-sstable` source, Cargo manifests/lock, scripts, Cargo config, timing
and admission code, and toolchain. `Justfile` is excluded because this command
invokes `scripts/cargow` directly. The external checkout identity and dirty
state are recorded in the closure JSON.

- [Raw terminal log](l3-proof/final-reaudit/final-audit-current-final5.log): SHA-256
  `1725e41bc4e7ff29a70f3d1d91b6f97f48803fac2f32099e860ebfb7862fa5be`.
- [Result and binary hashes](l3-proof/final-reaudit/final-audit-current-final5-result.json),
  [pre-admission boundary](l3-proof/final-reaudit/final-audit-current-final5-waiting-boundary.json),
  and [source closure](l3-proof/final-reaudit/source-closure-current.json).
- [Static checks](l3-proof/final-reaudit/final-audit-current-final5-static.log):
  `rustfmt --check --edition 2024` on the modified lexical files and
  `git diff --check`, both exit 0.

Earlier attempts are preserved as failed or superseded evidence: a concurrent
witness-test missing import blocked compilation; a language regression exposed
the non-stored metadata defect; a later run failed on one unused test import;
and the first 260-pass run preceded a formatting-only edit. None is promoted
as current-source proof.

**NOT_RUN:** whole-repository CI, unselected integration tests, installed daemon
E2E, peak RSS/performance qualification, release, and deployment. The native
collection byte ledger is not a whole-process memory/RSS ceiling: returned
`BTreeSet` and manual candidate vectors remain bounded by the examined-count
policy but are not charged to that ledger. Changes to shared
`documents.rs`, `searcher/candidates.rs`, `searcher/manual_scan.rs`, and
`searcher/snippets.rs` cross the original lane ownership; serial integration
review is still required before merging this dirty checkout.
