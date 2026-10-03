# Sourcegraph query bridge

Parses the supported Sourcegraph syntax and lowers it to a typed LQ
candidate. It does not broaden the executable LQ subset or serve results.

Start with [translation](src/translator.rs), [syntax](src/syntax.rs), and
[candidate output](src/candidate.rs). Supported syntax is bounded by the
[JUN-06-001 ADR](../../docs/adr/JUN-06-001-sourcegraph-compatibility-boundary.md).
