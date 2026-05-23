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

### Verification

- compile claims require a real `cargo` run
- prompt-manager claims require `pm.py lint`
- behavior changes require tests
- structured agent outputs must validate against `tools/ci/agent/agent_output.schema.json`

### Documentation

- generated docs are artifacts
- edit `tools/prompt-manager/sources/` only
- run `pm.py sync` then `pm.py lint`
