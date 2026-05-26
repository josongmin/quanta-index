# MSTR-03 Structural Core V2

Parent packet: [../README.md](../README.md)

## Objective

Widen the native structural route only where the executor can already preserve
authority and deterministic behavior.

## Target Behavior

- accept structural-only boolean trees instead of exactly one top-level
  structural leaf
- keep mixed lexical / structural boolean typed fail-closed
- add structural `AND` / `OR` / bounded `NOT`
- preserve deterministic candidate projection

## Follow-On

Typed holes and richer pattern-internal text predicates remain part of the same
ticket family, but must not ship as parser-only surface.

## Guardrails

- no lexical fallback
- no silent acceptance of mixed trees
- deterministic merge and projection rules are part of the behavior, not test
  incidental
