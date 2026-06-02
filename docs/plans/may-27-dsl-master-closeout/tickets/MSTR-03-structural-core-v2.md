# MSTR-03 Structural Core V2

Parent packet: [../README.md](../README.md)

## Objective

Finish the remaining native structural semantics that still require new set
algebra or universe definition, while preserving authority and determinism.

## Target Behavior

- keep the already-landed structural subset (`where`, `inside`, `outside`,
  named/anonymous/variadic holes, typed holes) stable
- add mixed lexical / structural boolean over a shared canonical candidate universe
- add pure-negative structural root semantics
- preserve deterministic candidate projection and binding retention after set algebra

## Current Source Truth

- structural-only boolean trees are already executable
- mixed lexical / structural boolean remains typed fail-closed
- pure-negative structural root remains typed fail-closed
- Sourcegraph structural widening already depends on native truth and must not
  outrun it

## Follow-On

Any widened native algebra must land before Sourcegraph or bridge widening that
depends on it.

## Guardrails

- no lexical fallback
- no silent acceptance of mixed trees
- deterministic merge and projection rules are part of the behavior, not test incidental
