# JFC-00 Truth Freeze and Scope Lock

> Archive status: `Historical program record`. Current architecture: [JUN-02-001](../../../adr/JUN-02-001-search-dsl-authority-and-runtime-contract.md). Archive map: [Completed Plan Archive](../../ARCHIVE-INDEX.md).


Parent packet: [../README.md](../README.md)

Status: `closed`

## Objective

Freeze one authoritative current-source description of the remaining DSL work and eliminate packet drift between proof ledger, capability matrix, and active owner tickets.

## Current Source Truth

- `docs/plans/may-25-lexical-enhancement/dsl-proof-ledger.toml` is the closest machine-readable proof inventory, but it is not the active whole-program packet
- `docs/plans/may-27-dsl-master-closeout/README.md` already regroups residue, but it is a previous packet and should no longer be treated as the active owner packet
- bridge carriers, parser-only shapes, owner-local proof rails, and active runtime rows are easy to blur if they are not explicitly separated in docs

## 핵심 로직

- one surface maps to one current status
- one current status maps to one authority owner
- packet prose may summarize, but must not override code-backed proof or typed fail-closed behavior
- carrier kind must stay explicit:
  - `search_result`
  - `bridge_packet`
  - `parser_shape`

## 건드릴 파일

- `docs/plans/jun-2-dsl-final-cut/README.md`
- `docs/plans/jun-2-dsl-final-cut/tickets/*.md`
- `docs/plans/may-25-lexical-enhancement/dsl-proof-ledger.toml`
- `docs/plans/may-25-lexical-enhancement/lexical-capability-matrix.md`
- `docs/plans/may-25-lexical-enhancement/README.md`
- superseded packet headers only if an explicit forward pointer is needed

## 건드리지 말 것

- product code
- runtime fixtures
- bridge/runtime carrier behavior
- proof status to make docs read cleaner

## TODO

- [x] declare this packet as the active whole-DSL owner packet
- [x] keep a historical map from prior packets/tickets to current owner tickets
- [x] ensure every DSL surface has exactly one current status in the proof ledger
- [x] ensure packet prose and matrix prose do not overclaim beyond executed rails
- [x] keep route-specific nuances explicit, especially executable `dirty:yes` / `dirty:no` vs typed-reject `dirty:only`, and bridge-only carriers

## NOT TODO

- no semantic widening
- no parser/executor changes
- no new proof claims without executed rails
- no collapsing owner-local proof into runtime proof

## Test Plan

- parse `dsl-proof-ledger.toml`
- read-check the packet index and historical map
- spot-check every newly edited claim against the owning code path and proof rail

## DoD

- the active packet for remaining DSL work is unambiguous
- the proof ledger, capability matrix, and active packet do not contradict each other
- bridge-packet carriers are not misrepresented as missing runtime rows
- parser-only and typed fail-closed surfaces are explicitly called out rather than silently omitted

## Failure Modes

- stale packet prose becomes more authoritative than code
- owner-local proof is mislabeled as runtime proof
- bridge carriers are treated as search-result residue
- code-backed but unproved surfaces disappear from docs instead of being called open
