# Text normalization

Shared Unicode normalization, case folding, token boundaries, and
provenance mapping for indexed and query text. Build/query callers must use
the same versioned normalization behavior.

Start with [text provenance](src/provenance.rs), [case rules](src/case.rs),
[tokens](src/tokens.rs), and [version](src/version.rs). The executable
behavior is in this crate; current routing is in the
[living documentation index](../../docs/ssot/README.md).
