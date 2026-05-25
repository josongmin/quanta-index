# E2E-07 - Performance and Chaos

Status: `proposed`
Priority: `P1`
Depends on: [E2E-00](E2E-00-live-dsl-matrix-harness.md), [LXE-04](LXE-04-regex-trigram-real-execution.md), [LXE-10](LXE-10-observability-and-bridge-sink.md)

## Purpose

Prove the hard DSL paths are bounded, deterministic, and observable under
large candidate sets, degenerate regex, cancellation, and partial shard
availability.

## Owner files

- new `crates/quanta-index-searchd-runtime/tests/e2e_perf_chaos.rs`
- `crates/quanta-index-lq-trigram/src/**`
- `crates/quanta-index-lq-regex/src/**`
- `crates/quanta-index-lexical/src/**`
- `crates/quanta-index-search-plane/src/query_dispatcher.rs`
- `crates/quanta-index-contract/src/results/**`

## File-level work breakdown

- `crates/quanta-index-searchd-runtime/tests/e2e_perf_chaos.rs`: add bounded
  regex, large-candidate trigram, cancellation, and partial-shard rows.
- `crates/quanta-index-lq-trigram/src/**` and
  `crates/quanta-index-lq-regex/src/**`: expose budget and diagnostics needed
  for plan-limit assertions.
- `crates/quanta-index-lexical/src/**`: surface deterministic early-stop and
  bounded candidate behavior for the runtime tests.
- `crates/quanta-index-search-plane/src/query_dispatcher.rs`: preserve typed
  early-stop and unavailable reasons into the response.
- `crates/quanta-index-contract/src/results/**`: keep explanation and error
  carriers stable enough for chaos assertions.

## Required scenarios

- high-frequency trigram produces a typed plan-limit or bounded candidate set.
- regex with no useful literal is rejected or bounded by configured policy.
- regex with useful literals uses prefilter and exact verify.
- fanout cancellation returns typed early stop and does not poison the next
  query.
- partial shard unavailable returns typed unavailable, not empty success.
- deterministic merge remains stable under large tied result sets.
- metrics/explanation report early stop and candidate counts without raw query
  text labels.

## Test plan

- use generated corpus fixtures with stable IDs.
- assert result counts, ordering, and typed error codes.
- assert no unbounded scan path is used unless the plan explicitly permits it
  under a tested budget.
- assert metrics labels are from a closed set.

## DoD

- hard regex/trigram paths have budget tests.
- cancellation cleanup is covered by a follow-up successful query in the same
  test.
- metrics and explanation expose enough detail to debug plan-limit failures.
- no chaos row depends on wall-clock-only assertions.

## Failure modes

- performance tests that only check elapsed time.
- allowing full scans without traceable policy.
- logging raw query text as a metric label.
