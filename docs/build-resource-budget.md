# Local build resource budget

`scripts/quanta-index-env.sh`, sourced by `scripts/cargow` and the Just recipes,
sets `CARGO_BUILD_JOBS=4` for local runs when that variable is absent. Separate
checkout and lane caches can build concurrently; this limits the parallel
compiler work requested by each invocation.

- An explicitly set `CARGO_BUILD_JOBS` is preserved, including invalid values
  that Cargo must reject. `cargo --jobs` keeps its normal precedence.
- `CI=true` leaves an unset budget unset. CI runner configuration owns its budget.
- This does not impose a host-wide concurrency limit. Other Cargo invocations,
  compiler wrappers, linkers, and test processes can still compete for resources.
- It does not change test selection, timeout budgets, or terminal result checks.

For timing comparisons, run native preparation and the Python CI suite
sequentially on the same frozen source and toolchain. Keep target-directory,
cache state, environment, and background load in the measurement record.
A shorter preparation path or fewer subprocesses does not by itself establish
an end-to-end speedup.
