# Jun 4 Sourcegraph Parity Ticket Index

Parent RFC: [../rfc.md](../rfc.md)

Status summary:

- packet active
- comparison baseline is Sourcegraph query/reference/ownership/structural docs
- current goal is not generic DSL widening; it is Sourcegraph gap closure
- scope-lock correction:
  - `SGP-00` landed
- internal substrate extension required:
  - `SGP-01` needs logical external repo keyed commit-recency authority
  - `SGP-05` needs text dispatch revision-selection / pin rebinding semantics
- authority-blocked until producer-side owner is chosen:
  - `SGP-02`, `SGP-03`, `SGP-04`
- repo-local executable queue:
  - `SGP-06`, `SGP-07`
- inventory closeout last:
  - `SGP-08`

## Ticket Table

| ticket | status | target |
| --- | --- | --- |
| [SGP-00](SGP-00-scope-lock-and-comparison-baseline.md) | landed | freeze exact Sourcegraph gap list and owner mapping |
| [SGP-01](SGP-01-repo-commit-recency-predicates.md) | planned | `repo:has.commit.after(...)` and `repo:contains.commit.after(...)` |
| [SGP-05](SGP-05-revision-at-time-filter.md) | planned | `rev:at.time(...)` revision-selection surface on text dispatch / history substrate |
| [SGP-02](SGP-02-repo-meta-and-topic-predicates.md) | planned | `repo:has.meta(...)` first, then `repo:has.topic(...)` after authority owner selection |
| [SGP-03](SGP-03-file-owner-predicate-and-owner-projection.md) | planned | `file:has.owner(...)` first, then `select:file.owners`; people-ownership authority required |
| [SGP-04](SGP-04-file-contributor-predicate.md) | planned | `file:has.contributor(...)`; file-level contributor materialization required |
| [SGP-06](SGP-06-sg-structural-direct-phrase-regex.md) | planned | direct SG structural lexical `Phrase` / `Regex` |
| [SGP-07](SGP-07-sg-structural-non-repo-predicate-siblings.md) | planned | SG structural mixed non-repo predicate family |
| [SGP-08](SGP-08-shared-inventory-and-parity-guard-followthrough.md) | planned | refresh inventory/guard after landed parity work |
