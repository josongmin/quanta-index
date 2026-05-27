# E2E-06 - Full Corpus Real-engine CI

Status: `completed`
Priority: `P0`
Depends on: [E2E-01](E2E-01-lexical-full-fidelity-e2e.md), [E2E-02](E2E-02-sourcegraph-parity-e2e.md)

## Purpose

Promote the DSL/Sourcegraph corpus into a real-engine CI rail. The corpus must
write data into indexes and query the runtime; parser-only corpus checks are a
separate lower gate.

## Current live truth (2026-05-27)

- `crates/quanta-index-searchd-runtime/tests/e2e_full_corpus.rs` is live and
  green against the real daemon harness.
- machine-readable runtime corpus fixtures live under
  `crates/quanta-index-searchd-runtime/tests/fixtures/lexical_corpus/`.
- `quanta-index-corpus-smoke` now carries explicit runtime metadata for:
  `syntax`, `classification`, `runtime_route`, `fixture`, `expected_ids`,
  `top_k`, and `runtime_error_code`.
- current corpus classification counts are:
  - `runtime`: 11 rows
  - `typed_unavailable`: 0 rows
  - `parser_only`: 0 rows
  - `deferred_external_producer`: 0 rows
- local/CI command surface exists as `just rust-test-full-corpus`.
- the current tree also wires the rail into
  `.github/workflows/correctness.yml` as `rust-full-corpus`.

## Owner files

- new `crates/quanta-index-searchd-runtime/tests/e2e_full_corpus.rs`
- new `crates/quanta-index-searchd-runtime/tests/fixtures/lexical_corpus/**`
- `crates/quanta-index-corpus-smoke/src/**`
- `Justfile`
- `.github/workflows/**` if CI workflow changes are required
- `docs/plans/may-25-lexical-enhancement/lexical-capability-matrix.md`

## File-level work breakdown

- `crates/quanta-index-searchd-runtime/tests/e2e_full_corpus.rs`: execute the
  corpus through the real runtime harness and report row-level failures.
- `crates/quanta-index-searchd-runtime/tests/fixtures/lexical_corpus/**`:
  store machine-readable query rows plus typed repo-metadata, structural-tree,
  and history authority for the runtime rail.
- `crates/quanta-index-corpus-smoke/src/**`: keep parser-only and runtime corpus
  rails explicitly separate.
- `Justfile` and `.github/workflows/**`: add local and CI entry points for the
  real-engine corpus rail.
- `docs/plans/may-25-lexical-enhancement/lexical-capability-matrix.md`: track
  which rows are promoted into CI and which remain deferred.

## Required scenarios

- import the existing usecase/corpus rows into machine-readable fixtures.
- classify every row with its public runtime route:
  - text
  - structural
  - history
- run real runtime rows through `E2E-00` harness.
- produce failure artifacts with:
  - row id
  - query
  - syntax
  - corpus fixture
  - expected IDs
  - actual IDs
  - typed error code
  - `SearchExplanation`

The current live rail executes text, structural, and history rows through the
real daemon harness. Parser-only, typed-unavailable, and
deferred-producer gaps were retired from this runtime closeout corpus; future
incomplete rows must live in the separate conformance or owner-local proof
rails instead of this file.

## Test plan

- local fast subset for PRs.
- full corpus rail for pre-merge or nightly.
- parser-only conformance stays separate and cannot mark runtime rows green.
- expected-failing rows require an owner ticket and expiration condition.

## DoD

- CI has a command that executes full real-engine corpus tests.
- every corpus row has a classification and owner.
- runtime corpus pass count is reported separately from parser-only pass count.
- no unsupported row is silently dropped from CI.

Current proving commands:

- `cargo test -p quanta-index-searchd-runtime --test e2e_full_corpus -- --nocapture`
- `cargo test -p quanta-index-searchd-runtime --test e2e_matrix_inventory -- --nocapture`
- `just rust-test-full-corpus`

## Failure modes

- counting parser conformance as runtime conformance.
- excluding hard rows without an owner ticket.
- hiding expected failures in test filters.
