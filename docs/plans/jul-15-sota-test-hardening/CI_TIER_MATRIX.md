# CI Tier Matrix

The current workflows are evidence inputs, not yet the completed hierarchy.
`PR` and scheduled correctness exist; merge receipts and portions of the
authority/receipt design are in-flight. Weekly/release is planned.

| Tier | Trigger | Target budget | Required proof | Current state |
| --- | --- | ---: | --- | --- |
| PR | pull request | 15 min | fmt/clippy, impacted+workspace nextest, P0 regression, wire golden, small lifecycle/differential corpus, changed-line coverage, touched-fuzz smoke | partial; heavy correctness also runs on PR today |
| Merge | `merge_group` | 30–40 min | workspace all-features nextest, fast daemon, SDK conformance, deterministic repeat, inventory guard | merge trigger/receipt in-flight |
| Nightly | scheduled | bounded by artifact | Miri/careful/TSan/ASan, expanded lifecycle/crash, critical mutation, broader fuzz, medium corpus quality/perf, authority receipt | daily schedule exists; scope incomplete |
| Weekly/release | weekly tag/dispatch | long | full corpus, long fuzz, large/XL scale, cold start/RSS/index/ingest, release receipt | planned |

## Quantitative promotion targets

- P0 owner changed-line coverage: >=95%; repository changed production Rust:
  >=90%.
- critical pure-logic mutation: >=85%; other selected target: >=75%; P0
  semantic survivors: zero or an explicit, time-bounded accepted exception.
- property test PR floor: 256 cases per property; nightly lifecycle total:
  100,000 transitions.
- fuzz: PR 30–60 seconds for changed target; nightly 15 minutes per target;
  weekly 2 hours per target. Every crash must preserve seed, minimized input,
  command, toolchain, and owner.
- nightly deterministic burn-in: 100 critical repetitions; daemon E2E: 20.

Coverage is a signal, never a replacement for negative/recovery proof or killed
mutants. Performance evidence is separately classified from correctness.

## Receipt contract

A promotable receipt must include schema version, commit SHA, event/tier, rail,
exact command, toolchain/platform, test execution summary, artifact digest,
start/end time, and failure classification. A prior green receipt cannot cover a
different SHA, target catalog, workflow command, or platform class.
