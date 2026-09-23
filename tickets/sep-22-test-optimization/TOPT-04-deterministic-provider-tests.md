# TOPT-04 — Deterministic Provider Retry and Concurrency Tests

Status: `code-landed; qualification pending`

Depends on: TOPT-00

Findings: R2, R3

Aligned S21 owner: S21-08

## Goal

Keep production full-jitter retry and bounded concurrency unchanged while unit
tests control delay and prove overlap structurally.

## Design

- Inject a provider-local retry policy/sleeper capability. It returns the delay
  and performs budget-aware sleep in production; tests use a deterministic
  recorder/zero sleeper.
- Keep jitter generation as a pure function with explicit entropy input or an
  equivalent deterministic test seam.
- Replace `ConcurrencyProbeTransport` sleep-based overlap with a first-wave
  barrier/latch. Exactly the configured concurrency cohort rendezvous; later
  calls proceed without a barrier.
- Preserve output-order, maximum-in-flight, retry classification, budget, and
  attempt-count assertions.

## Required tests

1. Delay bounds and attempt progression for production policy.
2. Retryable status invokes the injected sleeper with the expected attempt;
   non-retryable status never sleeps.
3. Budget shorter than proposed delay returns the existing interruption class.
4. Four-worker first wave reaches the barrier; peak never exceeds configured
   concurrency; ten outputs preserve input order.
5. No wall-clock sleep appears in provider unit doubles.

## Owned paths

- `crates/quanta-index-embed/src/openai.rs`
- `crates/quanta-index-embed/src/openai/retry.rs`

## Verification

- `./scripts/cargow test -p quanta-index-embed --lib openai::tests::`
- `./scripts/cargow test -p quanta-index-embed --lib`
- `just rust-profile test-provider-boundary-owner`
- `just rust-profile test-fast`

## Done

Focused results are deterministic across repeated nextest scheduling, ordinary
unit tests perform zero real backoff, and production retry distributions and
concurrency limits remain covered by pure/owner tests.
