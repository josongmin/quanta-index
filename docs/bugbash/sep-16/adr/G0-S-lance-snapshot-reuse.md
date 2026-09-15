# G0-S — LanceDB snapshot reuse

**Status: PASS**, with one alternative empirically closed off.

- Decided: 2026-09-16
- Gate owner: W0, blocking W3 semantic lane and QI-BB-006 (semantic half) / QI-BB-027
- Probe: [`crates/quanta-index-semantic/tests/g0s_lance_snapshot_probe.rs`](../../../../crates/quanta-index-semantic/tests/g0s_lance_snapshot_probe.rs)
- Command: `just rust-w0-storage-gates`, or
  `./scripts/cargow --lane test-integration-lane test --all-features --locked -p quanta-index-semantic --test g0s_lance_snapshot_probe -- --nocapture`
- Vendor: `lancedb = { version = "0.30", default-features = false }`, resolved `lancedb 0.30.0`
- Toolchain: rustc 1.92.0, darwin aarch64

## Question

The semantic lane materializes a delta generation with
`prepare_staging_dataset` → `copy_dir`, a full byte copy of the base dataset.
Reuse is only admissible if LanceDB's own guarantees support it.

## Evidence

Raw probe output (`G0S-EVIDENCE` lines, 2026-09-16):

```
G0S-EVIDENCE immutability first_version=2 second_version=4 files_before=9 files_after=15 retained_files=7 rewritten_files=0
G0S-EVIDENCE hardlink_delta base_total_bytes=24170 linked_bytes=24170 delta_total_bytes=26776 delta_fresh_bytes=2619 delta_fresh_files=7 delta_files_shared_with_base=8
G0S-EVIDENCE old_version_branch base_version=2 latest_version=3 checked_out_row_count=512 branch_write_rejected=true branch_write_error=Invalid_input,_table_cannot_be_modified_when_a_specific_version_is_checked_out
G0S-EVIDENCE prune_fail_closed base_version=2 base_row_count=512 pruned_old_versions=2 checkout_after_prune_ok=false post_prune_row_count=0
```

| Question | Result |
| --- | --- |
| Are dataset files immutable across versions? | **Yes.** 7 files retained across an add plus a delete (version 2 → 4) with identical inode, length and SHA-256; 0 rewritten. Only `_versions/latest_version_hint.json` is rewritten — the pointer to the newest manifest, not content. |
| Can a hard-linked base take a delta without touching base bytes? | **Yes.** g2 hard-linked all 24,170 base bytes, then deleted one row and added one. 8 of g2's files stayed hard links into g1; g2 wrote **2,619 fresh bytes**. Every g1 file was byte-identical afterwards and g1's row set was unchanged. |
| Can writes branch from an older-than-latest version in place? | **No.** `checkout(2)` reproduced the base row set exactly (512 rows), and the subsequent `add` was refused: *"Invalid input, table cannot be modified when a specific version is checked out."* |
| Does a pruned version fail closed? | **Yes.** After `Prune` removed 2 old versions, `checkout(2)` returned an error rather than serving a different row set. |

## Decision

1. W3 may replace `copy_dir` with hard-link materialization for the semantic
   lane, on the same terms as G0-L. QI-BB-006's semantic half is unblocked.
2. `_versions/latest_version_hint.json` is generation-local: copy, never link.
3. **One dataset directory per generation stays the layout.** In-place version
   branching is not available (question 3), so generation identity cannot be
   carried by a Lance version number alone.
4. Pruning is safe to use for reclamation because a pruned pin fails closed.
   W3 still owns the pin-before-prune ordering; this result only establishes
   that the failure mode is an error, not silent row-set drift.

## Rejected alternatives

- **One dataset with one Lance version per generation.** Empirically closed:
  LanceDB refuses writes against a checked-out older version, so a delta could
  not branch from a pinned base. The probe test
  `lance_refuses_to_mutate_a_checked_out_older_version` is kept as a regression
  so this decision cannot drift silently if upstream behavior changes.

## Limitations

- ANN behavior under delete/refill is deliberately **not** covered here. Mixing
  a recall threshold into a storage-capability gate would let either mask the
  other; QI-BB-027 / IT-11 own that, against the exhaustive exact-cosine oracle
  the semantic owner tests already hold. G0-S therefore does **not** discharge
  the ANN half of the semantic W3 work.
- 512 rows, 8-dimensional vectors, no vector index built. File-level
  immutability was measured on data and manifest files only.
- Single-host macOS APFS; the hard-link portability note from G0-L applies.
