# Jun 6 Sourcegraph Expansion

> Archive status: `Historical program record`. Current architecture: [JUN-06-001](../../adr/JUN-06-001-sourcegraph-compatibility-boundary.md). Archive map: [Completed Plan Archive](../ARCHIVE-INDEX.md).


Historical closeout packet for the post-`jun-5` Sourcegraph expansion work
that landed after `jun-4-sourcegraph-parity` and
`jun-5-sourcegraph-tail-gaps`.

Read order:

1. [WORKER_START_HERE.md](WORKER_START_HERE.md)
2. [NO-GO-RULES.md](NO-GO-RULES.md)
3. [SOURCE_TRUTH_MAP.md](SOURCE_TRUTH_MAP.md)
4. [DUMB_LLM_EXECUTION_CHECKLIST.md](DUMB_LLM_EXECUTION_CHECKLIST.md)
5. [rfc.md](rfc.md)
6. [tickets/INDEX.md](tickets/INDEX.md)

This packet remains intentionally narrow:

- it does not reopen `jun-4-sourcegraph-parity` as incomplete
- it does not reopen `jun-5-sourcegraph-tail-gaps` as incomplete
- it records the last widening work that this packet owned on the current tree

Already landed on the current tree and not backlog here:

- repo-file `path + content` support
- repo-description predicate support
- repo-meta key-existence / `tag:` widened shapes

Final closeout state:

- no feature residue remains inside quanta-index owner seams
- no external semantica producer proof residue remains for this packet
- verification follow-on moved to
  [../jun-7-verification-hellgates/rfc.md](../jun-7-verification-hellgates/rfc.md)
- preflight is still repo-global `unverified` because
  `scripts/check-persona-target-policy.sh` and `scripts/cg-agent-session` are
  absent in this checkout

Out of scope:

- relitigating already supported surfaces
- docs-only promotion
- parser-only or lowering-only proof
- heuristic regex or authority widening
