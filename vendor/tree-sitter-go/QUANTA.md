# Quanta Go grammar compatibility patch

Upstream crate: tree-sitter-go 0.25.0. The MIT LICENSE is retained.
The handwritten delta in quanta-compatibility.patch adds the Go 1.26
new(expression) operand while retaining new(type) and make(type, expressions).
Built-in names may be shadowed by ordinary or variadic functions. Their calls
share one argument rule; operand types and arity belong to Go type checking,
not declaration parsing. Malformed syntax remains a parse error.

Regenerate grammar.js with tree-sitter CLI 0.24.4 and ABI 14. The CLI archive
and binary are pinned by ../tree-sitter-typescript/quanta-provenance.json.
The source-bound path dependency is shared by the benchmark Rust producer and
Python declaration oracles. Reported policy binds actual vendor bytes.
