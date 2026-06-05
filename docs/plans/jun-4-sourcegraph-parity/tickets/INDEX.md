# Jun 4 Sourcegraph Parity Ticket Index

Parent RFC: [../rfc.md](../rfc.md)

Status summary:

- packet landed
- comparison baseline is Sourcegraph query/reference/ownership/structural docs
- current goal is not generic DSL widening; it is Sourcegraph gap closure
- scope-lock correction:
  - `SGP-00` landed
- landed cross-repo parity slice:
  - `SGP-01` has quanta-index contract/sdk/runtime/front-door/parity landed, and Semantica ingress live roundtrip proof is green
  - `SGP-05` has quanta-index text-dispatch revision selection landed with owner-local + targeted runtime + SDK/front-door proof
- landed explicit unsupported verdict:
  - `SGP-06` landed as explicit unsupported direct-surface demotion
  - `SGP-07` landed as explicit unsupported non-repo predicate sibling matrix
- repo-local executable queue:
  - `SGP-03` lands `file:has.owner(...)` / `select:file.owners` on ownership authority
  - `SGP-04` lands `file:has.contributor(...)` query-side support
- inventory closeout:
  - `SGP-08` landed
- mandatory follow-up remains:
  - none
- follow-on backlog lives in:
  - [../../jun-5-sourcegraph-tail-gaps/rfc.md](../../jun-5-sourcegraph-tail-gaps/rfc.md)

## Ticket Table

| ticket | status | target |
| --- | --- | --- |
| [SGP-00](SGP-00-scope-lock-and-comparison-baseline.md) | landed | freeze exact Sourcegraph gap list and owner mapping |
| [SGP-01](SGP-01-repo-commit-recency-predicates.md) | landed | `repo:has.commit.after(...)` and `repo:contains.commit.after(...)` |
| [SGP-05](SGP-05-revision-at-time-filter.md) | landed | `rev:at.time(...)` revision-selection surface on text dispatch / history substrate |
| [SGP-02](SGP-02-repo-meta-and-topic-predicates.md) | landed | `repo:has.meta(key:value)` supported; `repo:has.topic(...)` supported |
| [SGP-03](SGP-03-file-owner-predicate-and-owner-projection.md) | landed | `file:has.owner(...)` / `file:has.owner()` / `select:file.owners` supported |
| [SGP-04](SGP-04-file-contributor-predicate.md) | landed | `file:has.contributor(...)` supported on source-repo keyed contributor authority; external producer auto-emission remains separate integration work |
| [SGP-06](SGP-06-sg-structural-direct-phrase-regex.md) | landed | direct SG structural lexical `Phrase` / `Regex` explicit unsupported demotion |
| [SGP-07](SGP-07-sg-structural-non-repo-predicate-siblings.md) | landed | SG structural mixed non-repo predicate explicit unsupported matrix |
| [SGP-08](SGP-08-shared-inventory-and-parity-guard-followthrough.md) | landed | final inventory/guard synced to supported + explicit unsupported verdict set |
