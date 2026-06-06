# Jun 5 Sourcegraph Tail Gaps RFC

Status: `landed`
Date: `2026-06-05`
Scope: document the exact post-`jun-4` Sourcegraph backlog that still remains after the landed parity packet

Primary inventory:

- [README.md](README.md)
- [WORKER_START_HERE.md](WORKER_START_HERE.md)
- [NO-GO-RULES.md](NO-GO-RULES.md)
- [SOURCE_TRUTH_MAP.md](SOURCE_TRUTH_MAP.md)
- [DUMB_LLM_EXECUTION_CHECKLIST.md](DUMB_LLM_EXECUTION_CHECKLIST.md)
- [../../analysis/jun-4-dsl-capabilty.md](../../analysis/jun-4-dsl-capabilty.md)
- [../../../tools/benchmark/SOURCEGRAPH_PARITY.md](../../../tools/benchmark/SOURCEGRAPH_PARITY.md)

External comparison baseline:

- [Sourcegraph Search Query Language Reference](https://sourcegraph.com/docs/code-search/queries/language)
- [Sourcegraph Search Query Syntax](https://sourcegraph.com/docs/code-search/queries)

This packet is a follow-on backlog packet. It does not reopen:

- [../jun-4-sourcegraph-parity/rfc.md](../jun-4-sourcegraph-parity/rfc.md)
- final widening closeout lives in [../jun-6-sourcegraph-expansion/rfc.md](../jun-6-sourcegraph-expansion/rfc.md)

## 1. Scope Lock

This packet covers only cells that are still real gaps on the current tree.

Done and not backlog:

- `repo:has.commit.after(...)`
- `repo:contains.commit.after(...)`
- `repo:has.meta(key:value)`
- `repo:has.topic(...)`
- `file:has.owner(...)`
- `file:has.owner()`
- `file:has.contributor(...)` with current exact-string semantics
- `select:file.owners`
- `rev:at.time(...)`

Tail gaps owned here:

- `repo:contains.file(...)`
- `repo:contains.path(...)`
- `repo:has.file(path:... content:...)`
- `repo:has.description(...)`
- `repo:has.meta(key)`
- `repo:has.meta(tag:)`
- `repo:has.meta(/key/:/value/)` regex key/value semantics
- SG structural direct lexical `Phrase` sibling
- SG structural direct lexical `Regex` sibling
- SG structural mixed non-repo predicate sibling
  - `file.contains(path|file:...)`
  - `file.has.content(path|file:...)`
  - `symbol.has.name(...)`

## 2. Promotion Bar

A tail-gap cell moves to `지원됨` only when all are true:

1. parser or bridge admission exists on the owning route
2. owner seam is explicit
3. exact runtime or front-door proof exists for the claimed shape
4. SG/native parity exists when the surface is dual-syntax
5. neighboring unsupported cells stay explicit

If a cell is not worth implementing now, it must close as explicit `미지원`.

## 3. Ticket Order

1. [SGT-00](tickets/SGT-00-scope-lock-and-tail-baseline.md)
2. [SGT-01](tickets/SGT-01-repo-file-predicate-tail.md)
3. [SGT-02](tickets/SGT-02-repo-description-predicate.md)
4. [SGT-03](tickets/SGT-03-repo-meta-tail-shapes.md)
5. [SGT-04](tickets/SGT-04-file-contributor-regex-semantics.md)
6. [SGT-05](tickets/SGT-05-sg-structural-direct-phrase-regex-siblings.md)
7. [SGT-06](tickets/SGT-06-sg-structural-mixed-non-repo-predicate-siblings.md)
8. [SGT-07](tickets/SGT-07-guard-and-capability-followthrough.md)

Execution ordering rule:

- `repo-file` tail and `repo-meta` tail do not share the same owner seam; they may be worked independently
- structural tickets stay separate from text-route tickets
- `SGT-07` is last because it reflects final verdicts

## 4. Non-Goals

- no reopening `jun-4` landed claims
- no docs-only promotion
- no alias support claims without execution proof
- no regex semantics claimed from exact-string substrate
- no SG structural widening from lowering-only evidence

## 5. Remaining Work Ownership

- `jun-4-sourcegraph-parity` mandatory residue:
  - 없음
- tail-gap mandatory residue:
  - 없음

## 6. Ticket Index

- [tickets/INDEX.md](tickets/INDEX.md)
