# quanta-explicit-fd-process

Canonical source for explicit child descriptor mappings used by Quanta Index and Semantica. The crate was relocated from `semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-explicit-fd-process` after the 27-file source cohort was frozen. The Quanta-owned copy received platform and lint corrections and has its own standalone Cargo manifest.

This package contains reviewed platform-specific unsafe process and descriptor code. It is intentionally outside the Quanta Index workspace, whose member crates forbid unsafe code. Keep its source and tests under a dedicated owner rail; workspace-wide checks do not cover it.

Each consuming workspace binds resolved dependencies in its own Cargo.lock. The libc requirement is compatible from 0.2.186 so this shared library does not downgrade another workspace's unrelated dependencies.

Run `just rust-audit-explicit-fd`, `just rust-test-explicit-fd`, and `just rust-clippy-explicit-fd` from the Index root. Run the Rust tests on Linux to cover sealed executable identity and descriptor I/O, and on macOS to cover Darwin process launch.

`audited-source.json` retains the relocation inventory and the former Semantica authority hashes. Its `reviewed_changes` entries record current hashes for the reviewed additions and modifications, including Linux connected-image custody and the optional History descriptor. The audit rejects changes to the relocation inventory and checks the exact resulting Rust inventory and standalone manifest. Review source changes before updating their current hashes.
