# Rust Front Door Fast Path

Canonical Rust commands:
- `just rust-profile dev-fast`
- `just rust-profile dev-daemon`
- `just rust-profile release-daemon`
- `just rust-profile release-daemon-fresh`
- `just rust-profile dev-all-targets`
- `just rust-profile validate-shared-surface`
- `just fmt-check`
- `just rust-clippy`
- `just rust-policy`
- `just rust-profile test-fast`
- `just rust-profile test-integration-fast`
- `just rust-profile test-integration-storage`
- `just rust-profile test-integration-semantic`
- `just rust-profile test-integration`
- `just rust-profile test-daemon-fast`
- `just rust-profile test-daemon`
- `just rust-profile test-daemon-all`
- `./scripts/cargow check -p <crate>`
- `./scripts/cargow test -p <crate>`

Canonical developer shortcuts:
- `just rust-profile-list`
- `just rust-profile dev-fast`
- `just rust-profile release-daemon`
- `just rust-profile test-fast`
- `just rust-profile verify-rust`
- `just verify-rust-heavy`
- `just rust-profile-history-summary`
- `just verify`

`test-integration-fast`, `test-integration-storage`, `test-integration-semantic`,
`test-cli-smoke`, `test-daemon-fast`, and `test-daemon` are declarative
single-process nextest scopes from `tools/ci/test-authority.toml`.
The complete integration profile executes the fast, lexical-storage, and
semantic scopes in one lane; use the owning slice when only one surface changed.
`test-daemon-all` is the exhaustive runtime/harness closeout; use the smaller
`test-daemon` scope for the normal risk-focused loop.
Runtime scenario sources are linked through three explicit suite binaries.
`test-daemon-fast` runs only the fast suite; DSL cold-matrix truth starts at
`test-daemon` or the dedicated `rust-bench-dsl-truth` recipe.
