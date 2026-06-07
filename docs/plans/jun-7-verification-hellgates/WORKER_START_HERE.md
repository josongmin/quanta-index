# Worker Start Here

Status: `landed`

This packet does not add new DSL features.

It hardens proof shape:

- fast truth
- structural truth
- daemon lifecycle
- cross-repo ingress
- perf compare

Do first:

1. run `just rust-verify-hellgate-fast`
2. if fast is green, run `just rust-verify-hellgate-broad`
3. only then run `just rust-bench-dsl-warm`, `just rust-bench-dsl-cold 20`,
   `just rust-bench-dsl-compare`
4. cross-repo ingress is separate:
   - `just rust-verify-hellgate-cross-repo`

Current packet facts:

- `SCENARIOS` now carries fast-hellgate lane metadata
- text-route hellgate lives in
  `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/tests/e2e_text_route_hellgate.rs`
- structural-route hellgate lives in
  `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/tests/e2e_structural_hellgate.rs`
- cross-repo ingress target exists as a separate external proof lane
- SG structural direct lexical `Phrase` / `Regex` sibling remains explicit
  unsupported; the owner witness stays in search-plane lowering tests

Current verified snapshot on `2026-06-08`:

- `just rust-bench-dsl-truth` green
- `just rust-verify-hellgate-fast` green
- `just rust-verify-hellgate-broad` green
- `env QUANTA_INDEX_SEARCHD_BIN=/Users/songmin/Library/Caches/quanta-index/target/daemon-lane/debug/quanta-index-searchd just rust-verify-hellgate-cross-repo` red
  - external `semantica-codegraph-v2` boundary guard failure:
    `quanta-sdk.runtime-facade-boundary.v1`
- `just rust-bench-dsl-compare` green
- `just rust-verify-hellgate-all` was not rerun as one aggregate command in
  this snapshot; the component gates above were rerun instead

Preflight truth:

- `scripts/check-persona-target-policy.sh` 없음
- `scripts/cg-agent-session` 없음
- status stays `unverified`

Do not treat:

- `rust-bench-dsl-truth` as daemon lifecycle proof
- `rust-bench-dsl-compare` as correctness proof
- `e2e_full_corpus` as the only DSL support gate
