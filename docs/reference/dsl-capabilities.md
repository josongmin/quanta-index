# DSL capability entrypoint

This page is a navigation anchor, not an independent capability inventory or
a test receipt. Use the current code-owned sources:

- [Sourcegraph test inventory](sourcegraph-filter-parity.md) lists recognized
  forms, pinned old-Q and Native query witnesses, and coverage gaps after
  engine retirement. Generate it with `just rust-bench-dsl-parity`.
- `crates/quanta-index-lexical/src/predicate_registry.rs` for executable
  lexical predicate names and argument admission.
- `crates/quanta-index-search-plane/src/lowering.rs` for Sourcegraph structural
  leaf legality.
- [DSL proof inventory](dsl-proof-inventory.md)
  for historical per-surface proof owners. Its status cells are not a
  current-source support verdict without revalidation.
- [Sourcegraph compatibility decision](../adr/JUN-06-001-sourcegraph-compatibility-boundary.md)
  for the promotion and typed-refusal rules.

The generated matrix lists source/test coverage, not a current-source test
receipt or a promise of full upstream Sourcegraph parity. Unknown forms are
unsupported until their exact route and independent proof are admitted.
