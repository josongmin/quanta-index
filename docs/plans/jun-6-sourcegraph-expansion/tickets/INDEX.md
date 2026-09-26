# Jun 6 Sourcegraph Expansion Ticket Index

> Archive status: `Historical program record`. Current architecture: [JUN-06-001](../../../adr/JUN-06-001-sourcegraph-compatibility-boundary.md). Archive map: [Completed Plan Archive](../../ARCHIVE-INDEX.md).


Parent RFC: [../rfc.md](../rfc.md)

Status summary:

- packet landed
- `jun-4-sourcegraph-parity` stays landed
- `jun-5-sourcegraph-tail-gaps` stays landed
- this packet no longer owns any active widening backlog on the current tree
- `SGX-01` and `SGX-02` are already landed on the live tree
- `SGX-03` is now landed on the live tree
- `SGX-05` is terminal explicit unsupported and already landed
- `SGX-06` is now landed on the live tree
- `SGX-04` is now fully landed, including semantica producer publish + live ingress roundtrip proof
- `SGX-07` is now landed because guard and packet docs reflect the final verdict set
- no widening backlog remains inside `jun-6` for the current Sourcegraph docs baseline
- no semantica producer proof residue remains for this packet
- follow-on verification packet only:
  - [../../jun-7-verification-hellgates/rfc.md](../../jun-7-verification-hellgates/rfc.md)

Worker read order:

1. [../WORKER_START_HERE.md](../WORKER_START_HERE.md)
2. [../NO-GO-RULES.md](../NO-GO-RULES.md)
3. [../SOURCE_TRUTH_MAP.md](../SOURCE_TRUTH_MAP.md)
4. [../DUMB_LLM_EXECUTION_CHECKLIST.md](../DUMB_LLM_EXECUTION_CHECKLIST.md)

## Ticket Table

| ticket | status | scope |
| --- | --- | --- |
| [SGX-00](SGX-00-scope-lock-and-reopen-rules.md) | landed | freeze reopened cells and keep landed packets closed |
| [SGX-01](SGX-01-repo-file-path-content-support.md) | landed | `repo:has.file(path:... content:...)` now supported on the live tree |
| [SGX-02](SGX-02-repo-description-predicate-support.md) | landed | `repo:has.description(...)` now supported on the live tree |
| [SGX-03](SGX-03-repo-meta-widened-shapes.md) | landed | regex `repo:has.meta` family is executable on the live tree |
| [SGX-04](SGX-04-file-contributor-regex-semantics.md) | landed | contributor regex now matches structured `name` / `email` authority without canonical raw-string fallback; semantica producer publish + live ingress roundtrip proof is green |
| [SGX-05](SGX-05-sg-structural-direct-phrase-regex-support.md) | landed | SG structural direct lexical `Phrase` / `Regex` sibling is permanently explicit unsupported |
| [SGX-06](SGX-06-sg-structural-mixed-non-repo-support.md) | landed | SG structural mixed non-repo predicate siblings are executable on the live tree |
| [SGX-07](SGX-07-guard-and-capability-followthrough.md) | landed | guard and capability followthrough are synced to the final landed verdict set |
