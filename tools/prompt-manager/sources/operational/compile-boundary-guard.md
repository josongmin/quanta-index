# Compile-Boundary Guidance

이 repo는 아직 별도 boundary guard CLI가 없다.

현재 규칙:
- crate boundary 변경은 `Cargo.toml`, `use`, `pub use`, `mod`를 직접 읽고 검토한다
- facade/export surface를 건드리면 `cargo check --workspace`와 `cargo test --workspace`까지 올린다
- shared contract crate와 core crate 사이에서 vendor import가 core로 새지 않게 유지한다
