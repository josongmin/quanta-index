# Test Optimization Bugbash — Plan (Sep 22)

Goal: cut test time + remove cheesy/weak/duplicate tests without losing signal.

## Repo test layout (verified by listing files)

- Workspace: 25 crates under `crates/`.
- Integration tests: **195 files** under `crates/*/tests/**/*.rs`.
- Files containing `#[test]`/`#[tokio::test]`: **350** (integration + inline `src/` unit tests).
- Inline `mod tests` in src: **172 files**.
- Per-crate `tests/*.rs` file counts:
  - searchd-runtime: 54 | lexical: 17 | contract: 11 | semantic: 9 | repomap: 8
  - core: 7 | lq-positions: 6 | ipc: 5 | lq-trigram: 5
  - catalog: 3 | lq-bridge: 3 | lq-regex: 3 | lq-structural: 3
  - contract-base: 2 | lq-obs: 2 | search-plane: 2 (+2 inline suites: `src/readiness/tests`, `src/query_dispatcher/tests`, `src/ingest_dispatcher/tests`)
  - lq-norm: 1 | sdk: 1 | searchctl: 1 (`cli_smoke.rs`) | searchd-harness: 1 | corpus-smoke: 1
  - no `tests/` dir: embed, lq-text-normalizer, scan-experiment, searchd, searchd-harness helpers (verify per crate if touched)
- Heaviest dir: `crates/quanta-index-searchd-runtime/tests/` (54 files, many `e2e_*`; has `tests/common/` helpers: `searchd_binary_process.rs`, `e2e_corpus.rs`, `frontdoor_scenarios.rs`, `searchd_lease_probe.rs`).
- Runner: `just <test-*>` / `./scripts/cargow` (per `AGENT_CORE.md`; no bare `cargo`).

## Parallel audit tracks

| # | File | Objective |
|---|------|-----------|
| 1 | `01-duplication.md` | Find overlapping/redundant test coverage to merge or delete |
| 2 | `02-slow-inefficient.md` | Find slowest / most wasteful tests and how to speed them up |
| 3 | `03-weak-assertions.md` | Find tests that pass without proving anything |
| 4 | `04-cheesy-test-code.md` | Find cheesy test code (sleep, unwrap-spam, magic numbers, copy-paste scaffolding) |
| 5 | `05-cheesy-src-code.md` | Find cheesy src code test-only paths hide (test hooks, prod `unwrap`, dead cfg) |

## Track briefs

### Track 1 — Duplication → `01-duplication.md`
- Objective: list test pairs/files covering the same behavior where one can go.
- Search method: cluster `tests/` files by name prefix (`e2e_*`, `runtime_*`) and by helper (`tests/common/*`); diff setup blocks; `rg` for duplicated fixture names / repeated assertion strings across files.
- Done when: every merge/delete candidate names both files + lines, shared behavior, and which file keeps it.

### Track 2 — Slow/inefficient → `02-slow-inefficient.md`
- Objective: rank the worst time sinks and give a concrete speedup per item.
- Search method: look for process spawn (`searchd_binary_process.rs`), `sleep`/`timeout`, full-corpus fixtures (`e2e_full_corpus.rs`, `e2e_corpus.rs`), repeated rebuild/reingest loops, sync sleeps in async tests; confirm with `cargo test -- --list` counts + one timed run if cheap.
- Done when: each item has measured or clearly bounded cost (what it waits/spawns) + fix (shared fixture, smaller corpus, poll-with-deadline).

### Track 3 — Weak assertions → `03-weak-assertions.md`
- Objective: flag tests that cannot fail on real regressions.
- Search method: `rg` for `assert!(true)`, `is_ok()` without value check, no-assert smoke tests, snapshot tests that never compare, `e2e` tests that only check boot/exit code; read full test body for each hit.
- Done when: each item quotes the weak assertion + states what regression it misses + stronger assertion.

### Track 4 — Cheesy test code → `04-cheesy-test-code.md`
- Objective: flag test-only sloppiness that hides flakes and slows edits.
- Search method: `rg` for `thread::sleep`, `unwrap()` chains, `panic!("todo")`, hardcoded ports/paths, magic numbers, >50-line copy-pasted setup fns; read each body.
- Done when: each item has file+line, cheesy pattern, and small cleanup (helper, const, retry-with-timeout).

### Track 5 — Cheesy src code → `05-cheesy-src-code.md`
- Objective: flag prod-code smells the test suite exposes or excuses.
- Search method: follow test failures/helps into `src/`: `rg` for `#[cfg(test)]` shims in prod, `unwrap`/`expect` on prod paths, `allow(...)` attributes, dead `pub` helpers only tests use; read the src body.
- Done when: each item names src file+line, why prod code is at fault (not the test), and the prod-side fix.

## Finding format (all tracks)
- `path:line` — symptom — why it is bad (slow/cheesy/weak/duplicate) — concrete fix.
- Open every file body you cite; search output alone is not evidence.
- Keep it short: one bullet per finding, no essays.
