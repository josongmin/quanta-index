## Rule Catalog

### Safety

- no silent failure
- no silent fallback
- fail-closed by default
- no error swallowing or error-to-default replacement on production paths
- no heuristic authority when the real authority is absent
- no unverifiable completion claim
- no `ok` status when required inputs, correctness-affecting assumptions, failed checks, or tool errors remain

### Architecture

- shared contract crate is the only producer/search-plane integration surface
- core crate must not import `rusqlite`, `tantivy`, `lancedb`, or raw filesystem layout
- storage/query vendor choices belong to adapters only

### Build hygiene

- no proc-macro derives for serialization: `#[derive(serde::Serialize)]`, `#[derive(serde::Deserialize)]`, `#[derive(Serialize)]`, `#[derive(Deserialize)]` are banned. Write manual `impl serde::Serialize` / `impl serde::Deserialize` instead. Reason: proc-macro expansion is the dominant build-time cost in serde-heavy crates; manual impls keep cold-build seconds bounded and make wire shape auditable.
- the ban applies workspace-wide (contract, core, adapters, searchd, tests, benches). Enforced by semgrep rule `rust-no-serde-derive`.

### Verification

- compile claims require a real `cargo` run
- prompt-manager claims require `pm.py lint`
- behavior changes require tests
- structured agent outputs must validate against `tools/ci/agent/agent_output.schema.json`

### Documentation

- generated docs are artifacts
- edit `tools/prompt-manager/sources/` only
- run `pm.py sync` then `pm.py lint`
