# Structural patterns

Owns the structural pattern IR, language set, bindings, and matching
subset. The search plane integrates it with typed query admission and
source authority; this crate does not open a repository.

Start with [pattern IR](src/pattern.rs), [matching](src/matcher.rs), and
[binding](src/binding.rs). See [engine status](../../docs/ssot/engine-status-v1.md)
for currently supported and refused structural paths.
