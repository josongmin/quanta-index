# Jun 5 Sourcegraph Tail Gaps

Canonical packet for the exact Sourcegraph tail-gap backlog after the landed
`jun-4-sourcegraph-parity` packet.

Read order:

1. [WORKER_START_HERE.md](WORKER_START_HERE.md)
2. [NO-GO-RULES.md](NO-GO-RULES.md)
3. [SOURCE_TRUTH_MAP.md](SOURCE_TRUTH_MAP.md)
4. [DUMB_LLM_EXECUTION_CHECKLIST.md](DUMB_LLM_EXECUTION_CHECKLIST.md)
5. [rfc.md](rfc.md)
6. [tickets/INDEX.md](tickets/INDEX.md)

This packet is intentionally narrow:

- it does not reopen `jun-4-sourcegraph-parity`
- it does not relitigate landed surfaces
- it owns only exact tail gaps still visible on the current tree

Tail-gap families:

- repo-file predicate tail
- repo-description predicate
- repo-meta tail shapes
- contributor regex semantics
- SG structural direct lexical sibling gap
- SG structural mixed non-repo predicate sibling gap
- guard / capability followthrough

Out of scope:

- generic DSL widening
- docs-only promotion
- proving a new supported cell without runtime/front-door/parity evidence
