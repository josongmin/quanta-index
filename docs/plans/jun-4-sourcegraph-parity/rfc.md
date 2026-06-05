# Jun 4 Sourcegraph Parity RFC

Status: `landed`
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

## 1. Final Verdict Set

Supported in this packet:

- `repo:has.commit.after(...)`
- `repo:contains.commit.after(...)`
- `rev:at.time(...)`
- `repo:has.meta(key:value)`
- `repo:has.topic(...)`
- `file:has.owner(...)`
- `file:has.contributor(...)`
- `select:file.owners`

Closed in this packet as explicit unsupported:

- SG structural direct lexical `Phrase` sibling
- SG structural direct lexical `Regex` sibling
- SG structural mixed non-repo predicate siblings
  - `file.contains(path|file:...)`
  - `file.has.content(path|file:...)`
  - `symbol.has.name(...)`

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
  - `repo:has.commit.after(...)`는 quanta-index 내부 contract/sdk/runtime/front-door/parity substrate가 landed 상태고, Semantica ingress owner의 repo commit recency auto-emission도 live ingress roundtrip proof까지 green이다.
  - `rev:at.time(...)`도 이제 text dispatch 단계의 revision-selection / pin rebinding surface까지 landed다.
  - 따라서 `SGP-01`과 `SGP-05`는 모두 landed이고, 남은 history-side residue는 capability gap이 아니라 shared inventory/guard followthrough다.
- `file:has.owner(...)`와 `select:file.owners`는 같은 ownership authority를 쓰더라도 owner seam이 다를 수 있다.
  - query-side filter를 먼저 닫고 projection은 second increment로 둔다.
- `repo:has.meta(...)`와 `repo:has.topic(...)`는 같은 티켓 안에 있어도 같은 authority라고 가정하지 않는다.
  - 현재 tree는 `has.meta`와 `has.topic`을 distinct authority batch로 분리해 landed 했다.
- `rev:at.time(...)`는 predicate widening이 아니라 revision-selection / history route 문제다.
  - lexical-only patch로 취급하면 안 되고, text dispatch의 pin-resolution seam까지 같이 다뤄야 한다.
- `repo:has.meta(key:value)`와 `repo:has.topic(...)`는 현재 tree에서 executable authority가 있다.
  - 둘 다 owner-local/runtime/front-door/parity/corpus rail이 닫혔다.
- `file:has.owner(...)`는 현재 tree에서 executable authority가 있다.
- `file:has.contributor(...)`는 현재 tree에서 executable authority가 있다.
  - source-repo keyed file-contributor authority batch, runtime/front-door/parity/corpus proof까지 landed다.

Execution queue:

1. [SGP-00](tickets/SGP-00-scope-lock-and-comparison-baseline.md)
2. [SGP-06](tickets/SGP-06-sg-structural-direct-phrase-regex.md) — landed
3. [SGP-07](tickets/SGP-07-sg-structural-non-repo-predicate-siblings.md) — landed
4. [SGP-01](tickets/SGP-01-repo-commit-recency-predicates.md) — landed
5. [SGP-05](tickets/SGP-05-revision-at-time-filter.md) — landed
6. [SGP-02](tickets/SGP-02-repo-meta-and-topic-predicates.md) — landed for `repo:has.meta(key:value)` + `repo:has.topic(...)` promotion
7. [SGP-08](tickets/SGP-08-shared-inventory-and-parity-guard-followthrough.md) — landed

Execution note:

- `SGP-00`는 scope-lock correction이다.
- `SGP-01` / `SGP-05`는 같은 bucket이 아니다.
  - `SGP-01`: quanta-index contract/sdk/runtime/front-door/parity support와 Semantica ingress live proof까지 landed다.
  - `SGP-05`: text dispatch revision-selection / pin rebinding seam이 landed다. 남은 followthrough는 `SGP-08` inventory/guard sync다.
- `SGP-02`는 두 surface 모두 landed로 닫혔다.
  - `repo:has.meta(key:value)`: landed
  - `repo:has.topic(...)`: landed
- `SGP-03`는 split verdict로 닫혔다.
  - `file:has.owner(...)` / `file:has.owner()` query-side filter는 landed
  - `select:file.owners` explicit result contract도 landed
- `SGP-04`는 contributor authority batch와 query-side execution seam까지 landed다.
  - external producer auto-emission은 packet closeout의 필수 조건이 아니라 separate integration seam이다.
- 현재 repo 안에서 바로 executable closeout이 가능한 structural demotion lane은 `SGP-06` / `SGP-07`에서 이미 닫혔다.
- `SGP-08`까지 닫힌 현재 상태에서 이 packet의 mandatory residue는 없다.

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

## 6. Remaining Work

- mandatory residue:
  - 없음
- optional new-scope only:
  - follow-on backlog lives in [../jun-5-sourcegraph-tail-gaps/rfc.md](../jun-5-sourcegraph-tail-gaps/rfc.md)

## 7. Ticket Index

- [tickets/INDEX.md](tickets/INDEX.md)
