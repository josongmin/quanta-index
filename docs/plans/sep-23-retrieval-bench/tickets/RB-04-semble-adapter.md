# RB-04 — Semble Same-Corpus Comparison Adapter

Status: `planned`

Depends on: RB-00 stage B and RB-01 record contract (environment/data audit may start after stage A)

Owner: Quanta repository's optional competitor adapter

## Goal

Run a pinned Semble release/commit locally against the exact same admitted repository bytes and blind query pack, then normalize its actual results for the single Quanta-owned evaluator.

## Work

1. Pin Semble source/version, Python/runtime dependencies (recorded as a dependency lockfile with digest, a mandatory manifest field), embedding model revision and benchmark repository SHA. Keep installed tool/model cache outside Quanta. Verify dataset/code license and attribution before committing any imported labels.
2. Index the RB-00 frozen file universe. Use an explicit common file manifest/filter; if Semble's API cannot honor it exactly, construct an isolated corpus with identical canonical paths+bytes and prove the mapping, or mark the attempted comparison ineligible. Never silently accept differing ignores, size limits or file types. A matching file count is not evidence of matching input. Emit the path-mapping proof artifact: the explicit path map (Semble-visible path → canonical repo path for every admitted file) plus the both-side path+SHA diff (Quanta-side list vs Semble-side list, per-file SHA). Any mismatch fails the common-universe pair; the diff digest is a mandatory manifest field.
3. Call Semble's real library/CLI search with identical query text and declared `top_k`; record native chunk path/line range, rank, raw latency/errors and version. Normalize spans and hashes from the pinned checkout, not from a guessed snippet. Do not implement or imitate Semble ranking in this repository.
4. Record Semble index time and warm query latency separately. A subprocess CLI call includes startup overhead and cannot be labeled library warm-query latency. Use the same measurement layer on both sides, or report the layers separately.
5. Keep Semble optional for fast CI. A full head-to-head claim requires a real Semble result record on the same source/host, not its published README numbers.
6. If reporting a model-matched control, compare fixed code/query embeddings, tokenizer behavior, dimensions and normalization against Quanta's clean-source `potion-code` profile with a frozen tolerance. Without [TEST-PLAN.md](TEST-PLAN.md) T15, report only an end-to-end system pair with disclosed models, not model parity. T15 gates only the same-model claim, never `PAIR_VALID`.

## Planned files

- `tools/benchmark/retrieval/semble.py`
- `tools/benchmark/retrieval/run.py`
- `tools/benchmark/retrieval/README.md`
- `tools/ci/tests/test_retrieval_benchmark.py` (adapter contract fixtures)

## Acceptance / verification

- A one-repo pilot emits a full recorded result for every eligible eval query with Semble revision/model and exact candidate spans.
- Intentional file-universe mismatch, missing model, partial query output, unstable path mapping and invalid spans fail with explicit reason.
- [TEST-PLAN.md](TEST-PLAN.md) T00/T11/T12 include excluded-but-tracked files, truncated snippets, missing query rows and library-vs-CLI timing mismatch.
- Any published comparison uses fresh paired local runs. Semble's public table is linked only as methodology/context, not imported as the measured opponent.
