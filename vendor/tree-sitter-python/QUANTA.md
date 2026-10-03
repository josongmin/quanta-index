# Canonical python grammar generation

Upstream crate: tree-sitter-python 0.25.0. The MIT LICENSE is retained.
The grammar and scanner semantics are unchanged. Generated C, grammar.json,
node-types.json and tree-sitter headers use tree-sitter CLI 0.24.4, ABI 14.
CLI archive and binary digests are pinned in
../tree-sitter-typescript/quanta-provenance.json.

ABI 14 allows the existing Python tree-sitter 0.23.2 runtime and Rust 0.25.10
runtime to consume the same parser bytes. It does not change either runtime
or the query/definition contract. Both producers bind actual vendor bytes;
old preflight policy commitments must be reissued after this cutover.
