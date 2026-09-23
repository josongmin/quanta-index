# Retrieval benchmark cross-language fixtures

Independent oracle vectors consumed by both the Python evaluator
(`tools/benchmark/retrieval/evaluator.py`) and the Rust runner
(`benchmarks/retrieval`). Neither side may generate its expected values
by calling the other side's helpers: hand-written vectors first,
machine-filled digests only via a plain SHA-256 tool, then each
implementation is tested against the frozen file.

- `canonical-json-vectors.json`: value -> exact canonical bytes (UTF-8).
- `tokenizer-vectors.json`: text -> token list + count (`qi-regex-v1`).
- `span-vectors.json`: file bytes + line span -> byte span, block hash,
  token count.
- `split-leakage-mutants.json`: train/eval span sets + allowlist ->
  accept/refuse.

Regeneration rule: edit vectors by hand, re-verify offsets/counts with
an independent computation, refill digests with SHA-256, and keep this
README next to the files it governs.
