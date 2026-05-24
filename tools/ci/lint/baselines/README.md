# CI lint baselines

Committed snapshots used by the regression gates under `tools/ci/lint/`.

- `llvm-lines.json` — per-crate monomorphization line counts measured by
  `cargo llvm-lines --release -p <pkg> --lib`. Used by
  `check-llvm-lines.py`. Update with:

      python3 tools/ci/lint/check-llvm-lines.py --update-baseline

- `public-api/<crate>.txt` — `cargo public-api --simplified` output for
  contract-tier crates. Used by `check-public-api.py`. Update with:

      python3 tools/ci/lint/check-public-api.py --update-baseline

Both gates fail the build when current measurements diverge from the
baseline. Updating a baseline must be an intentional commit, ideally in the
same PR as the change that caused the drift.
