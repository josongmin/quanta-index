# Jun 4 Sourcegraph Parity RFC

Status: `active`
Date: `2026-06-04`
Scope: close the highest-signal Sourcegraph parity gaps that remain after `jun-4-dsl-extension`

This packet starts from live code truth, not from old closeout prose.

Primary inventory:

- [../../analysis/jun-4-dsl-capabilty.md](../../analysis/jun-4-dsl-capabilty.md)
- [../../../tools/benchmark/SOURCEGRAPH_PARITY.md](../../../tools/benchmark/SOURCEGRAPH_PARITY.md)

External comparison baseline:

- [Sourcegraph Search Query Syntax](https://sourcegraph.com/docs/code-search/queries)
- [Sourcegraph Search Language Reference](https://sourcegraph.com/docs/code-search/queries/language)
- [Sourcegraph Code Ownership](https://sourcegraph.com/docs/own/configuration_reference)
- [Sourcegraph Structural Search](https://sourcegraph.com/docs/code-search/types/structural)

This RFC is a **new-scope parity program**. It does not reopen:

- [../jun-2-dsl-final-cut/README.md](../jun-2-dsl-final-cut/README.md)
- [../jun-2-dsl-hardening/README.md](../jun-2-dsl-hardening/README.md)
- [../jun-2-dsl-advanced/README.md](../jun-2-dsl-advanced/README.md)
- [../jun-4-dsl-extension/rfc.md](../jun-4-dsl-extension/rfc.md)

---

## 1. Current Gap Set

Highest-signal Sourcegraph comparison gaps still open:

1. `repo:has.commit.after(...)`
2. `repo:contains.commit.after(...)`
3. `repo:has.meta(...)`
4. `repo:has.topic(...)`
5. `file:has.owner(...)`
6. `select:file.owners`
7. `file:has.contributor(...)`
8. `rev:at.time(...)`
9. SG structural direct lexical `Phrase` sibling
10. SG structural direct lexical `Regex` sibling
11. SG structural mixed non-repo predicate sibling

## 2. Promotion Bar

A Sourcegraph surface moves from gap to `지원됨` only when all are true:

1. parser / syntax admission exists on the owning route
2. lowering or execution owner seam is explicit
3. exact runtime or front-door rail exists for the claimed shape
4. SG/native parity exists when the surface is dual-syntax
5. unsupported neighboring cells remain explicit

If a surface is not worth implementing now, it must be closed as explicit
`미지원`, not left ambiguous.

## 3. Ticket Order

Critical review corrections:

- SG structural parity는 Sourcegraph official docs에서도 experimental / disabled-by-default surface다.
  - 따라서 structural gap은 text/history/ownership gap보다 뒤에 둔다.
- `repo:has.commit.after(...)`와 `rev:at.time(...)`는 raw history substrate가 일부 이미 있지만, 기존 RFC가 가정한 것처럼 바로 executable lane은 아니다.
  - `committer_time_ms`, refs/tags, RFC3339/duration timeref parsing은 이미 materialized 되어 있다.
  - 하지만 `repo:has.commit.after(...)`는 현재 history authority에 logical external repo 축(`source_repo_id`)이 없어 Sourcegraph-style repo gate를 바로 계산할 수 없다.
  - `rev:at.time(...)`는 기존 lexical text route에서 `rev:` 자체가 fail-closed이고, text dispatch 단계에 revision-selection / pin rebinding surface가 없다.
  - 따라서 둘 다 "existing history substrate reuse 가능"이 아니라 **history substrate extension required** ticket으로 취급해야 한다.
- `file:has.owner(...)`와 `select:file.owners`는 같은 ownership authority를 쓰더라도 owner seam이 다를 수 있다.
  - query-side filter를 먼저 닫고 projection은 second increment로 둔다.
- `repo:has.meta(...)`와 `repo:has.topic(...)`는 같은 티켓 안에 있어도 같은 authority라고 가정하지 않는다.
  - `has.meta` 먼저, `has.topic`은 별도 second increment다.
- `rev:at.time(...)`는 predicate widening이 아니라 revision-selection / history route 문제다.
  - lexical-only patch로 취급하면 안 되고, text dispatch의 pin-resolution seam까지 같이 다뤄야 한다.
- `repo:has.meta(...)`, `repo:has.topic(...)`, `file:has.owner(...)`, `file:has.contributor(...)`는 현재 search-plane 안에 executable authority가 없다.
  - 이 셋은 producer-side ingestion/materialization owner가 확정되기 전까지 authority-blocked로 유지한다.

Execution queue:

1. [SGP-00](tickets/SGP-00-scope-lock-and-comparison-baseline.md)
2. [SGP-06](tickets/SGP-06-sg-structural-direct-phrase-regex.md)
3. [SGP-07](tickets/SGP-07-sg-structural-non-repo-predicate-siblings.md)
4. [SGP-08](tickets/SGP-08-shared-inventory-and-parity-guard-followthrough.md)

Execution note:

- `SGP-00`는 scope-lock correction이다.
- `SGP-01` / `SGP-05`는 history substrate extension required ticket이다.
  - `SGP-01`: logical external repo keyed commit-recency authority 필요
  - `SGP-05`: text dispatch revision-selection / pin rebinding seam 필요
- `SGP-02` / `SGP-03` / `SGP-04`는 external producer authority-blocked ticket이다.
  - producer-side owner가 정해지기 전에는 parser/bridge-only progress를 landed로 취급하지 않는다.
- 현재 repo 안에서 바로 executable surface를 닫을 수 있는 lane은 `SGP-06` / `SGP-07`뿐이다.

## 4. Non-Goals

- no docs-only promotion
- no Sourcegraph marketing-surface parity claims
- no ownership or contributor support without real authority ingestion
- no SG structural widening without exact runtime/parity proof
- no reopening already-landed shipped DSL closure claims

## 5. Residue Rule

Before this RFC lands:

- [../../analysis/jun-4-dsl-capabilty.md](../../analysis/jun-4-dsl-capabilty.md)
  must stop listing the chosen surfaces as generic unsupported leftovers when a
  stronger `지원됨` or explicit `미지원` verdict has been proven.

If a ticket lands with a demotion instead of a promotion, the analysis doc must
say that directly.

## 6. Ticket Index

- [tickets/INDEX.md](tickets/INDEX.md)
