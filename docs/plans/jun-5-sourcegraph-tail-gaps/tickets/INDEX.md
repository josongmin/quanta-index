# Jun 5 Sourcegraph Tail Gaps Ticket Index

Parent RFC: [../rfc.md](../rfc.md)

Status summary:

- packet landed
- `jun-4-sourcegraph-parity` stays landed
- this packet is exact-gap backlog only
- done surfaces are intentionally excluded from the backlog
- final verdicts: `repo:contains.file` / `repo:contains.path` supported (alias); all other tail cells closed as explicit typed-fail or permanent demotion. See [../../../../docs/analysis/jun-4-dsl-capabilty.md](../../../analysis/jun-4-dsl-capabilty.md)

Worker read order:

1. [../WORKER_START_HERE.md](../WORKER_START_HERE.md)
2. [../NO-GO-RULES.md](../NO-GO-RULES.md)
3. [../SOURCE_TRUTH_MAP.md](../SOURCE_TRUTH_MAP.md)
4. [../DUMB_LLM_EXECUTION_CHECKLIST.md](../DUMB_LLM_EXECUTION_CHECKLIST.md)

## Ticket Table

| ticket | status | verdict |
| --- | --- | --- |
| [SGT-00](SGT-00-scope-lock-and-tail-baseline.md) | done | scope locked: done vs explicit-unsupported vs supported buckets separated |
| [SGT-01](SGT-01-repo-file-predicate-tail.md) | done | `repo:contains.file` / `repo:contains.path` **supported** (alias → `repo.has.file`); `repo:has.file(path+content)` **typed-fail** (no content matcher seam) |
| [SGT-02](SGT-02-repo-description-predicate.md) | done | `repo:has.description(...)` **typed-fail** (no producer description authority) |
| [SGT-03](SGT-03-repo-meta-tail-shapes.md) | done | key-only stays **typed-fail**; `tag:` and `/key/:/value/` regex converted from silent-admit to **typed-fail** |
| [SGT-04](SGT-04-file-contributor-regex-semantics.md) | done | exact-only pinned; `/<regex>/` contributor **typed-fail** (exact-string set, no name/email split) |
| [SGT-05](SGT-05-sg-structural-direct-phrase-regex-siblings.md) | done | **permanent demotion**: Phrase/Regex are structural body syntax, no distinct sibling surface (witnesses green) |
| [SGT-06](SGT-06-sg-structural-mixed-non-repo-predicate-siblings.md) | done | **demotion frozen**: preserve-only widening ruled out; support requires a new candidate-level structural execution seam (witnesses green) |
| [SGT-07](SGT-07-guard-and-capability-followthrough.md) | done | parity guard regenerated, capability-truth green, analysis doc + packet aligned to final verdicts |
