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
| R-ENG-03 | No god code / SOLID at write-time | one struct/fn that mixes parsing + I/O + business logic + formatting; concrete adapter in a composition partner's field (DIP violation); copy-pasted mirror methods (`lexical_*` / `semantic_*`); fat trait that bundles 4+ unrelated methods | one responsibility per type / fn; depend on traits at composition seams; lift mirrored shape into one generic or one delegated type; split fat traits along their natural axis. If a god-code or SOLID drift is observed while reading any nearby code, **propose a fix in the same PR or call it out explicitly as a deferred follow-up** — never silently leave it. |
| R-DOC-01 | Generated docs SSOT | hand-edit generated agent docs | edit `tools/prompt-manager/sources/` then sync |
