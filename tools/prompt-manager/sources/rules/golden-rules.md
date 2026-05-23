## Golden Rules

| ID | Rule | Forbidden | Required |
|----|------|-----------|----------|
| R-SAFE-01 | No silent failure | swallowing `Result` / empty fallback | explicit error propagation |
| R-SAFE-02 | Fail-closed by default | heuristic success path / best-effort continuation / `continue-on-error` | explicit stop, typed failure, or `NotImplemented` |
| R-SAFE-03 | No error swallowing | `ok()`, error-to-default replacement, `except: pass`, `|| true` | preserve error context and abort or return explicit failure |
| R-SAFE-04 | Structured output honesty | `ok` with missing required inputs / hidden correctness assumption / failed check | schema-valid `blocked` or `error` |
| R-ARCH-01 | Hexagonal boundary | vendor type in core ports/services | vendor adapter only |
| R-ARCH-02 | Contract-first | producer/search-plane internal type sharing | shared contract crate |
| R-ENG-01 | No naive coding | unproven heuristic as authority / hidden fallback logic | invariant-driven logic with tests or explicit limitation |
| R-ENG-02 | No quiet degradation | logging-only error handling / partial success claim after dropped failure | fail the operation or mark it partial with proof |
| R-DOC-01 | Generated docs SSOT | hand-edit generated agent docs | edit `tools/prompt-manager/sources/` then sync |
