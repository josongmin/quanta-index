# Jun 6 Sourcegraph Expansion RFC

> Archive status: `Historical program record`. Current architecture: [JUN-06-001](../../adr/JUN-06-001-sourcegraph-compatibility-boundary.md). Archive map: [Completed Plan Archive](../ARCHIVE-INDEX.md).


Status: `landed`
Date: `2026-06-07`
Scope: document the post-`jun-5` Sourcegraph expansion work that this packet
closed after the landed `jun-4-sourcegraph-parity` and
`jun-5-sourcegraph-tail-gaps` packets

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

This packet is historical closeout state. It does not reopen:

- [../jun-4-sourcegraph-parity/rfc.md](../jun-4-sourcegraph-parity/rfc.md)
- [../jun-5-sourcegraph-tail-gaps/rfc.md](../jun-5-sourcegraph-tail-gaps/rfc.md)
- verification follow-on only:
  [../jun-7-verification-hellgates/rfc.md](../jun-7-verification-hellgates/rfc.md)
  (`landed`)

## 1. Scope Lock

This packet closed the quanta-index owner seams and the external semantica
producer proof for the last remaining post-`jun-5` widening cells.

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
- `repo:contains.file(...)`
- `repo:contains.path(...)`
- `repo:has.file(path:... content:...)`
- `repo:has.description(...)`
- `repo:has.meta(key)`
- `repo:has.meta(tag:)`
- `repo:has.meta(/key/)`
- `repo:has.meta(/key/:)`
- `repo:has.meta(key:/value/)`
- `repo:has.meta(/key/:value)`
- `repo:has.meta(/key/:/value/)`

Packet residue:

- 없음

## 2. Promotion Bar

An expansion cell moves to `지원됨` only when all are true:

1. parser or bridge admission exists on the owning route
2. owner seam is explicit
3. exact runtime or front-door proof exists for the claimed shape
4. SG/native parity exists when the surface is dual-syntax
5. neighboring unsupported shapes stay explicit
6. `sourcegraph_parity.py --check` classifies the new supported or unsupported
   verdict machine-checkably

If a cell is not worth implementing now, it stays explicit `미지원`.

## 3. Ticket Order

1. [SGX-00](tickets/SGX-00-scope-lock-and-reopen-rules.md)
2. [SGX-03](tickets/SGX-03-repo-meta-widened-shapes.md)
3. [SGX-04](tickets/SGX-04-file-contributor-regex-semantics.md)
4. [SGX-05](tickets/SGX-05-sg-structural-direct-phrase-regex-support.md)
5. [SGX-06](tickets/SGX-06-sg-structural-mixed-non-repo-support.md)
6. [SGX-07](tickets/SGX-07-guard-and-capability-followthrough.md)

Execution ordering rule:

- text-route widening tickets may run independently
- structural tickets stay separate from text-route tickets
- `SGX-07` is last because it reflects final verdicts

## 4. Non-Goals

- no reopening landed packets as incomplete
- no docs-only promotion
- no alias-only, parser-only, or lowering-only support claim
- no regex semantics claimed from exact-string substrate
- no structural widening without a real execution seam

## 5. Final Closeout State

- `jun-4-sourcegraph-parity` mandatory residue:
  - 없음
- `jun-5-sourcegraph-tail-gaps` mandatory residue:
  - 없음
- future widening residue:
  - 없음 for the current Sourcegraph docs baseline covered by `jun-4` + `jun-5` + `jun-6`
- packet closeout residue:
  - 없음
- external proof residue:
  - 없음
- verification architecture follow-on:
  - [../jun-7-verification-hellgates/rfc.md](../jun-7-verification-hellgates/rfc.md)
    is landed and owns fast hellgates, broad daemon lifecycle gates,
    cross-repo ingress proof routing, and perf compare naming
- preflight:
  - still repo-global `unverified`
  - `scripts/check-persona-target-policy.sh` 없음
  - `scripts/cg-agent-session` 없음

## 6. Ticket Index

- [tickets/INDEX.md](tickets/INDEX.md)
