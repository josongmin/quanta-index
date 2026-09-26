# Local build resource budget

`scripts/quanta-index-env.sh`, sourced by `scripts/cargow` and the Just recipes,
sets `CARGO_BUILD_JOBS=4` for local runs when that variable is absent. This
limits the parallel compiler work requested by each invocation.

- An explicitly set `CARGO_BUILD_JOBS` is preserved, including invalid values
  that Cargo must reject. `cargo --jobs` keeps its normal precedence.
- `CI=true` leaves an unset budget unset. CI runner configuration owns its budget.
- This does not change test selection or terminal result checks.

## Shared build and Rust test slot

Local `scripts/cargow` build, check, clippy, test, bench, doc, rustc, rustdoc,
llvm-cov, clean, and nextest run/archive commands acquire one shared slot. A
nextest list also acquires it unless both binaries and Cargo metadata are
supplied. Metadata-only and formatting commands do not acquire the slot.
The lock is `$QUANTA_INDEX_CACHE_ROOT/resource-admission/build-test.lock`, shared
across checkout and target lanes that use the same cache root. Searchd pin
preparation releases its slot before the main test command acquires it.

- `QUANTA_INDEX_RESOURCE_ADMISSION=auto` is the default: enabled locally,
  bypassed with an explicit diagnostic when `CI=true`. `1` enables it in CI;
  `0` explicitly disables it. Any other value fails before Cargo starts.
- Admission waits at most `QUANTA_INDEX_RESOURCE_WAIT_SECONDS` (default 300).
  An admitted command runs at most `QUANTA_INDEX_RESOURCE_TIMEOUT_SECONDS`
  (default 7200). Both require positive integers. Either timeout exits 124;
  setup failures and interrupted commands cannot become successful results.
- The private process guard retains the lock through command-group cleanup,
  including controller termination. Cargo and its children do not inherit the
  lock descriptor. The lock file is never unlinked; replacing it is invalid.
- Only leaf Cargo commands acquire the slot. Wrapping a whole benchmark or
  test orchestrator in the same non-reentrant lock would deadlock when it calls
  `cargow`. Python orchestration remains outside the slot.

This is cooperative admission, not a host resource quota. Direct Cargo calls,
different cache roots, compiler services, unrelated processes, and descendants
that deliberately leave their process group remain outside its guarantee.
It does not establish that a benchmark host is idle or performance-qualified.

For timing comparisons, run native preparation and the Python CI suite
sequentially on the same frozen source and toolchain. Keep target-directory,
cache state, environment, and background load in the measurement record.
A shorter preparation path or fewer subprocesses does not by itself establish
an end-to-end speedup.

## Native preparation preserves each binary's feature graph

The portable proof keeps searchd and retrieval runner builds separate. Combining
the two selected packages can change dependency features in the runner even
when both binary targets retain their default features.

The 2026-09-27 diagnostic on Darwin (Cargo/Rust 1.92.0, lockfile SHA-256
`05044065eae52f79a67378cfa995414c08f77e6910e30bd8bb36dae92ef362d5`)
compared Cargo unit graphs for each binary separately and both together. The
runner's reachable unit count changed from 298 to 339. For example,
`hyper-rustls` gained `http2`, `native-tokio`, and `rustls-native-certs`, while
`cc` gained `parallel`. This rejects feature-equivalent consolidation for that
input tuple; it is not a compiled-binary or performance comparison.

Reproduce the graph comparison with the same locked manifests, toolchain, and
environment using `RUSTC_BOOTSTRAP=1 ./scripts/cargow build --locked --offline
-Z unstable-options --unit-graph`, selecting each package/binary independently,
then selecting both. The bootstrap override is for graph inspection only;
normal proof builds do not set it. Compare each binary's reachable dependency
subgraph, including features and profiles, rather than only the root features.

The admitted optimization is to prepare nextest binaries once, then reuse the
bound binaries metadata and Cargo metadata for collection and execution. Both
reuse consumers revalidate input identity before and after execution; every
selected test still runs and requires terminal evidence.
