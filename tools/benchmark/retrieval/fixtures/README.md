# Retrieval fixture usage

`just retrieval-contract-local` consumes these fixed inputs in the Python
evaluator and Rust runner tests:

| File | Input / expected output |
| --- | --- |
| `canonical-json-vectors.json` | JSON value → exact canonical UTF-8 bytes |
| `tokenizer-vectors.json` | Text → `qi-regex-v1` tokens and count |
| `span-vectors.json` | File/line span → byte span, block hash and token count |
| `split-leakage-mutants.json` | Train/eval spans and allowlist → accept/refuse |

When editing vectors, independently check offsets/counts and recompute hashes
with SHA-256. The independent oracle policy is in
[SEP-26-003](../../../../docs/adr/SEP-26-003-retrieval-evidence-custody-and-qualification.md).
