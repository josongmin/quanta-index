## Agent change posture

**Default: breaking-first.**

- compatibility preservation is not the default
- do not add long-lived shims
- prefer one canonical contract over dual surfaces
- generated docs must be updated through prompt-manager, not patched by hand
- do not introduce heuristic success paths when an authoritative path is missing
- do not replace errors with defaults, placeholders, or best-effort continuation on production paths
- prefer explicit `NotImplemented`, typed failure, or blocked cutover over partial silent behavior
