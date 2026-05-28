# Build

- default workspace check: `./scripts/cargow check --workspace`
- default lint rail: `just rust-clippy`
- default format rail: `just fmt-check`
- default MSRV pin: 1.92.0 (verified by the `rust-msrv` CI job and `just rust-msrv`)
- bench compile guard: `just rust-bench-build` (criterion)
- unused-dep guard: `just rust-machete` (cargo-machete) — workspace-level only, not per-crate
- supply-chain guard: `just rust-deny` (cargo-deny) — advisories `yanked=deny`, `unmaintained=all`, `unsound=all`
- build/test claims must name the exact command
