# P10 adversarial follow-up — 2026-09-26

Scope: offline manifest admission, source custody, inventory and verification.
Base: `604149ed3f6033e24a834ebaa86a596f7b8ed82d` on local `main`.
This is a local code/test closeout, not a P10 release or operational receipt.
The shared worktree has other writers; their dirty changes are not included in
this implementation's commit or qualified by these checks.

## Findings and structural changes

| Boundary | RCA / counterexample | Owner change |
| --- | --- | --- |
| Manifest byte authority | Duplicate singleton headers used last-field-wins; decoding reconstructed reordered/noncanonical bytes into a different receipt digest. Self-signed empty/non-hex digests and repeated object paths were admitted. | `searchd/src/app/state_format.rs`: one `validate_authority` for read and write; canonical SHA-256 and unique object/directory identities; decode requires exact canonical re-encoding. Valid writer format is unchanged. |
| Publication before refusal | Only root-format was checked before creation; unsupported format versions, invalid payload identity and arbitrary file names could be written. | Same owner validates all authority before any file or fault boundary and accepts only the two owned top-level manifest names. Existing `create_new` and handle-based fsync remain. |
| Verification versus backup freeze | A current session's backup freeze intentionally excludes catalog bytes. Reusing it as the verification oracle missed catalog changes after the logical catalog read. | `searchd/src/app/state_migration.rs`: current verification includes catalog bytes; backup verification reuses its existing full freeze. Start/end freezes retain root custody. Existing non-catalog session identities are compared, not reset by taking the extended freeze. |
| Vendor exception widened by filename | A global `.sqlite-wal/shm/journal` suffix skipped ordinary payload files or entire subtrees. Excluded sidecars also bypassed the hard-link check. | One exact relative-path predicate for the owned catalog, shared by inventory/freeze. Skip only regular vendor files after security checks; all other suffixes are ordinary payload. |
| Filesystem identity relabeling | Lossy UTF-8 conversion and Unix backslash-to-slash rewriting could change the filename identity recorded in a manifest; control characters could create an unreadable published manifest. | One `relative_state_path_v1` admission for object inventory, directory inventory and custody freeze. Reject non-UTF-8, controls and noncanonical separators; do not replace or normalize required identity. |

No parallel manifest IR, legacy decoder, compatibility shim, suppression or
consumer repair path was introduced. Existing persisted format/version stays
valid; previously admitted malformed/noncanonical authority is now refused.

## Counterexamples and independent oracles

The owner target uses a fixed textual golden and independently hashes its raw
body for forged-manifest cases; refusal expectations come from the canonical
manifest contract, not a reported `passed` flag. Other controls use actual
disposable files, SQLite backup/restore and mutation after the catalog read.

- Duplicate/reordered headers, leading-zero numbers, missing final newline,
  CRLF transport, malformed catalog/object digests, duplicate object paths,
  file/directory collision and control characters.
- Invalid-version/digest/path publication leaves zero created authority;
  absolute, traversing and unowned output names are refused.
- Catalog-byte mutation after logical verification; root replacement after
  lease acquisition; payload+manifest resealing after custody opens.
- Non-catalog sidecar-named files and subtrees must appear in the manifest and
  copied backup; owned sidecar hard links/symlinks must be refused without
  modifying the aliased bytes.
- Object/directory inventory and source custody reject backslash/control
  filenames using real disposable roots.
- Non-UTF-8 admission is a direct unit invariant. APFS refuses minting such a
  filename; that OS error is not counted as product verification. The raw
  non-UTF-8 filesystem fixture is `NOT_APPLICABLE` on this local filesystem;
  it is not a Linux filesystem counterexample.

The four initial audit tests failed against the pre-fix product code: invalid
digest decode, duplicate-header decode, invalid-version publication and
post-read catalog drift all returned success. Subsequent regression failures
were investigated rather than changing the expected acceptance contract.

## SQLite / concurrency boundary

Read-only WAL inspection can create native WAL/SHM files. On APFS this changes
the catalog directory's link count as well as its size and mtime (observed
`3 -> 5` with unchanged inode/mode/owner). Only those three directory metadata
fields are outside the verification comparison. The directory's identity,
advertised inventory and every payload file's bytes/metadata remain checked.
No guessed zero/default replaces those fields.

Backup custody is read-only, not exclusive writer exclusion. Keep sources
quiescent. These start/end checks detect covered mutations; they do not provide
an atomic filesystem snapshot against an arbitrary same-UID writer that
continues changing the root during or after the checks. A successful receipt
does not authorize a changed root, deployment or activation.

## Verification

Observed local runner outcomes (all exit 0):

| Command | Covered result | Excluded |
| --- | --- | --- |
| `./scripts/cargow test -p quanta-index-searchd-runtime --test state_migration_owner_v1` | 49 passed, 0 failed/ignored/filtered | Other integration targets, real roots |
| `./scripts/cargow test -p quanta-index-searchd --lib non_utf8_path_identity_is_refused_without_lossy_conversion` | 1 passed, 90 filtered | Remaining unit tests, raw non-UTF-8 Linux filesystem fixture |
| `./scripts/cargow clippy -p quanta-index-searchd --lib -- -D warnings` | Exit 0 | Other packages/targets |
| `./scripts/cargow clippy -p quanta-index-searchd-runtime --test state_migration_owner_v1 -- -D warnings` | Exit 0 | Other runtime tests |

Targeted rustfmt and diff-whitespace checks also returned exit 0. Clippy found
and fixed one existing identical-arm duplication in
`searchd/src/app/ipc_dispatcher.rs`; explicit exhaustiveness was retained,
not replaced by a wildcard or lint suppression.

Environment: `rustc 1.92.0 (ded5c06cf)`, `aarch64-apple-darwin`, local macOS/APFS,
wrapper-selected `test-daemon-lane` (owner) and `test-workspace-lane` (unit).
The four implementation/test source digests were unchanged across the final
execution and source recheck:

| Input | SHA-256 |
| --- | --- |
| `searchd/src/app/state_format.rs` | `b849156d7af54b8bbe62a8ba8690879d568cf0f6972ea1c255d6b74ca21c9a32` |
| `searchd/src/app/state_migration.rs` | `dd0d8cc08f902a0f4f2a018afee8227bafeecb4d5b6e20e9fd50c697e2cd9da7` |
| `searchd/src/app/ipc_dispatcher.rs` | `2ff50981a7ee10b9d4e4e7b98aca9d1334a6556f7b88bfdd94629d3e1e4f8115` |
| `searchd-runtime/tests/state_migration_owner_v1.rs` | `62807779e2a222bc5279bc650cbc9a3a176b980b8061f25450c9c3675a257df0` |
| `Cargo.lock` | `ff40c97922eb0c81ec1c46ca6bcc3e40e4b9d2d51ea497ec1a288cbb2b314837` |
| `scripts/cargow` | `488fcd15987f5cd273ccbb831ab22d1c54d9c0f35eef459663594cfdbe35ed8a` |
| `scripts/quanta-index-env.sh` | `26e171db52fa9d85d8065d57ff4424d19c766e8a4af46607b8c057513662ac26` |
| `.cargo/config.toml` | `f6308580477cf746b7a3fa03750293049dd4fe07b78fbed3f98b125663686234` |

Raw local artifacts under `/tmp/quanta-p10-audit-final.EJ3ILL/` (temporary,
not retained release custody):

| Artifact | SHA-256 |
| --- | --- |
| `owner-final.log` | `2208dbbac7a7888b22ea8349dbab885d11b2d446d07e96bb3087f3d7fb235f86` |
| `path-unit.log` | `ac8c3b40f890b155b903ee9992c0daf5e471b1fe1bc3236e6012f5df86b49565` |
| `clippy-searchd.log` | `01329f9530f74ae886bbdbb75ce6e1235ec6000e5f703ca386bba166b006b86b` |
| `clippy-owner.log` | `7be1cf4ff5d091d2d34d21f0ef8f69aa9e2426d1ea264ad17e82cfde8ded32c4` |

The owner binary from the logged path
`~/Library/Caches/quanta-index/target/e385f4e6b4fe8e9b/test-daemon-lane/debug/deps/state_migration_owner_v1-f6949a7d7779a1d4`
digests to `7fa89195dd0fd96892fad20dd4ad19d82c3d54562cddf0cdb45601c6e3cf152c`.
The unit binary in `test-workspace-lane/debug/deps/quanta_index_searchd-14f31c4c4f57e11b`
digests to `437549bd663e3057f055cd8bf4bc4bfd63769513902ce7beb0608236056f1c55`.

Qualification status: `NOT_RUN`. Another writer changed the dirty harness
`freshness.rs` during the shared session (observed digest changed from
`ff9a66bf64ddc719f86134cc3c3afdb3dfd36a8015cb68ea5217a8eade1e055b` to
`880b3527c5c839706029525bd84b4c32bd2d2ab165e90e429183dedec1f9fd52`).
Do not promote the observed runner successes to an exact-source owner receipt
or frozen-source qualification. Reissue that proof from a stable final source
and complete dependency identity; the other writer's changes were preserved.

Excluded: frozen-source qualification, whole-repository tests, Linux release
process proof, authorized real-root restore/rollback, deployment and activation.
Those remain `NOT_RUN`; P09/P10 release dependencies and registry stage are
unchanged.
