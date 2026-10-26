# Quanta Index SQLite capacity patch

- Upstream: `libsqlite3-sys` 0.38.2 from the crates.io registry.
- Registry archive SHA-256: `f1d20bef17f513b9b3004532233187769cd072d790971f4e4da0e346eb6401e8`.
- License: MIT. The upstream `LICENSE` is retained. Bundled SQLite is public domain.
- Local feature: `quanta-capacity-vfs`, requiring bundled SQLite on Unix. One
  `links = "sqlite3"` dependency remains in the graph; no second SQLite engine.
- Local files: `quanta/quanta_capacity_vfs.c`, its header and native Unix witness
  include, and `src/quanta_capacity_vfs.rs`. Build integration compiles the shim
  with the existing amalgamation. The Rust facade contains the FFI boundary;
  application crates continue to forbid unsafe code.
- The named VFS admits create/growth before lower I/O, forwards native locking,
  observes native DB/WAL/SHM inode widths without reopening those files, and
  records actual descriptor retirement including deferred POSIX closes.
  Configured refusal has a private extended FULL code. The facade validates
  the compiled C/Rust event layout before registering the VFS.
- `close_owned_file` consumes ordinary owned writer/reader FDs and returns the
  actual close result. An ambiguous close is never retried against a possibly
  reused descriptor. Composition must retain the root lease and charge.
- Native witness and close hooks depend on this exact amalgamation. Re-audit
  both the Unix implementation and callback ABI when upgrading SQLite.
- Scope: logical regular-file length and file count. This does not measure
  allocated blocks, free-space headroom or RSS. Registration selects the native
  `unix` VFS and rejects incompatible SQLite build modes.
- Qualification requires real ABI, WAL/SHM/locking, pre-growth refusal, deferred
  close, fault and restart tests. A path-parent retarget between preflight and
  native Unix open remains a separate custody seam; post-open mismatch fails
  closed and does not prove zero escaped I/O.

No default runtime activation is implied by the dependency feature.
