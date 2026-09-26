# Tickets - May 26 Indexing Residue Tasks

> Archive status: `Historical program record`. Current architecture: [JUN-02-001](../../../adr/JUN-02-001-search-dsl-authority-and-runtime-contract.md). Archive map: [Completed Plan Archive](../../ARCHIVE-INDEX.md).


Parent doc: [../README.md](../README.md)

This pack is now a closeout record for the real residue that was left after the
`may-25` closeout and active lexical lanes.

## 1. Tickets

| Ticket | Priority | Title | Primary owner files | Start condition |
| --- | --- | --- | --- | --- |
| [STR-02](STR-02-authority-matcher-tree-walk-expansion.md) | P0 | Authority matcher tree-walk expansion | `lq-structural`, `searchd/runtime.rs` | shipped |
| [STR-03](STR-03-native-structural-semantics-ast-ir-expansion.md) | P0 | Native structural semantics AST/IR expansion | `lq-norm`, `lq-structural` | shipped |
| [STR-04](STR-04-structural-query-surface-expansion.md) | P1 | Structural query surface expansion | `search-plane/query_dispatcher.rs`, `searchd/runtime.rs` | shipped |
| [BRIDGE-02](BRIDGE-02-sourcegraph-structural-honesty-gate.md) | P0 | Sourcegraph structural honesty gate | `search-plane/lowering.rs`, `core/domains/lexical/service.rs` | shipped |
| [BRIDGE-03](BRIDGE-03-sourcegraph-structural-syntax-and-lowering.md) | P1 | Sourcegraph structural syntax and lowering | `search-plane/lowering.rs`, `sdk_frontdoor.rs` | shipped supported subset |

## 2. Dependency order

1. `STR-02` widened the truthful live authority matcher.
2. `STR-03` gave native syntax/IR a typed carrier for richer semantics.
3. `STR-04` exposed only the structural surface the widened executor honors.
4. `BRIDGE-02` removed the lexical Sourcegraph honesty gap.
5. `BRIDGE-03` landed the supported Sourcegraph structural subset on the
   structural route.

## 3. Collision rules

Historical note:

- `STR-02` and `STR-03` were safe to run together.
- `STR-04` followed once the matcher subset was explicit.
- `BRIDGE-02` and `BRIDGE-03` touched the shared Sourcegraph frontdoor and were
  landed after the overlapping lexical lane stabilized.

## 4. Explicitly out of scope

- `LXE-04` regex/trigram engine work
- `LXE-05` phrase/positions engine work
- `LXE-06` symbol/select/type frontdoor work
- Sourcegraph lexical v1 subset
- structural `repo/file/lang` live subset
- lexical/structural fused result ranking

## 5. Post-pack queue

This ticket pack is closed on the current tree. Remaining closeout work is
owned by `docs/plans/may-25-lexical-enhancement`, especially:

- `LXE-10`
- `E2E-07`

`BRIDGE-03` stays marked as a shipped supported subset by design. That wording
does not imply unfinished implementation inside this pack; it records the
intentional fail-closed boundary for unsupported Sourcegraph structural forms.
