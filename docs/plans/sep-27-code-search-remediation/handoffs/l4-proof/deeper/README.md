# L4 deeper audit proof

Start with [receipt.json](receipt.json) and [the audit](../../L4_ADDITIONAL_AUDIT.md).

- `wire-native-fuzz-1f7467d3304e/`: frozen contract/native/fuzz phase. The final wire/fuzz results are authoritative only for their listed input sets.
- `core-native-fc88f5f79f57/`: final core/native/lint phase after stable/nightly counter compatibility and lint repairs. Public API rendering completes but baseline comparison fails.
- `phase-boundary.json`: exact core-only delta and disjoint wire/fuzz dependency closure.
- `live-closeout.json`: current shared-tree differences; no current-tree qualification.
- `diagnostic/` and `invalidated-shared-target.json`: RED failures, superseded attempts and cache-reuse invalidation. No success promotion.
- `source.tar.gz` in each phase: complete bound source snapshot. Manifests and each run's before/after hashes identify the tested inputs; unpack into a dedicated directory and use canonical checkout-scoped caches to rerun.
- `l4-additional.patch`: reviewed source delta against the recorded pre-audit files and core base. Shared files may include concurrent edits; it is not an ownership-exclusive Git commit.
