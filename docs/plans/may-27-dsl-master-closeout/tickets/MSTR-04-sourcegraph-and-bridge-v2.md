# MSTR-04 Sourcegraph and Bridge V2

Parent packet: [../README.md](../README.md)

## Objective

Widen the Sourcegraph structural frontdoor and bridge surface only after native
structural semantics already exist.

## Scope

- Sourcegraph structural boolean composition
- Sourcegraph structural richer body lowering where native semantics exist
- structural bridge / CodeQL follow-on on the existing bridge path

## Guardrails

- translator never falls back to lexical
- unsupported shapes remain typed failures
- bridge export preserves structural result truth rather than down-casting to
  lexical candidates
