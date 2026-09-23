# Compile-Boundary Guidance

This repository has no dedicated boundary-guard CLI yet.

Current rules:
- For crate-boundary changes, inspect `Cargo.toml`, `use`, `pub use`, and `mod` directly.
- Facade/export changes require workspace-wide compile and test escalation through the canonical wrapper.
- Keep vendor imports out of shared contract and core crates.
