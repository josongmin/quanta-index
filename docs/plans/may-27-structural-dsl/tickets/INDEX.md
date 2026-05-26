# Structural DSL Tickets — Final Packet Index

Parent packet: [../README.md](../README.md)

This directory is the execution-facing decomposition for the `may-27` structural
DSL packet.

Historical inputs are not deleted or renamed. They are re-anchored here through
[HISTORICAL-MAP.md](HISTORICAL-MAP.md) so the current plan has one entrypoint
without breaking old references.

---

## 1. Active successor tickets

| Ticket | Priority | Depends on | Outcome |
| --- | --- | --- | --- |
| [SDL-01](SDL-01-structural-boolean-composition.md) | `P0` | shipped `STR-02/03/04` | native structural boolean composition |
| [SDL-02](SDL-02-typed-hole-semantics.md) | `P0` | `SDL-01` pattern/binding model | typed holes on the ship language set |
| [SDL-03](SDL-03-sourcegraph-structural-v2-lowering.md) | `P1` | `SDL-01`, relevant `SDL-02` semantics | richer SG structural lowering with parity proof |
| [SDL-04](SDL-04-language-set-expansion.md) | `P1` | `SDL-01`, `SDL-02` | Java first, then post-ship grammar expansion |
| [SDL-05](SDL-05-structural-codeql-bridge.md) | `P2` | `SDL-01`, `SDL-03` | structural `into:codeql` candidate export |
| [SDL-E2E-01](SDL-E2E-01-structural-proof-and-observability.md) | `P0` | all owning behavior tickets | structural proof matrix and bounded-label metrics |

## 2. Intentional boundary

This packet owns structural semantics and their direct frontdoor/proof follow-on.
It does not own:

- history/runtime truth sources
- semantic/hybrid redesign
- generic lexical boolean/fusion behavior
- exporter productization beyond the structural metrics sink

## 3. Historical attachment

Use [HISTORICAL-MAP.md](HISTORICAL-MAP.md) first when a prior ticket number
appears in discussion. That file is the canonical "old ticket -> new owner"
mapping for this packet.
