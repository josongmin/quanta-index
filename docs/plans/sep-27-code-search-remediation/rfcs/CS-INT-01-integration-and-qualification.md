# CS-INT-01 — Remaining integration and qualification

Status: `ACTIVE_RESIDUAL`.
Implemented source/preview/capture decisions are in
[SEP-27-003](../../../adr/SEP-27-003-code-search-source-and-preview-contract.md),
[SEP-27-004](../../../adr/SEP-27-004-benchmark-capture-and-resource-custody.md) and
[OCT-05 ADRs](../../../adr/README.md#oct-05-implemented-contracts).
[MISC](../../sep-27-misc/tickets/INDEX.md) owns common execution/CI/producer and
supported-platform qualification; [OCT-04](../../oct-4-parallel-closure/tickets/INDEX.md)
owns current review/capture/runtime/cost residuals. This file retains integration
acceptance, with no duplicate live queue or historical test-total promotion.

## Remaining boundaries

| Scope | Required result / stop rule |
| --- | --- |
| [ENG-02](CS-ENG-02-capability-publication-and-freshness.md) | Measure whole coverage pipeline and temporary/retained physical heap on growing one-file/mixed updates: pre-intent preflight, lock-held preflight, build, seal/hash and independent open. Existing same-build commitment reuse and borrowed row mutation do not remove all base walks. Reuse needs authenticated immutable handles and both refusal boundaries; otherwise retain verification and report measured cost. |
| [ENG-04](CS-ENG-04-match-anchored-snippets.md) | Exact request-wide physical regex allocation admission is conditional/deferred P3, with no observed overrun claim. Per-engine NFA/DFA and logical preview guards do not prove an aggregate allocator ceiling. Reopen only under its stated consumer/measurement condition; do not promote old parser-hook proposals to a selected release gate. |
| [BENCH-01–04](CS-BENCH-01-corpus-gold-and-holdout.md) | Independent source/license/gold/splits/unused holdout, native-derived refusal proof across selected entrypoints/formats, actual indexed view per comparator, admitted statistical units and equal-boundary/resource measurements. A declared matrix or disk/file-view probe cannot qualify those facts. |
| External producer/readers | Actual Semantica issuer/QBC, paired SDK/consumer migration, installed process, publish/activate, crash/restart, retention/rollback and supported Linux paths on one selected source. Older generations use the current explicit rebuild boundary. Local producer inspection is not execution. |
| Adapter I/O | Actual prepare/execute/publish/load/replay paths for large successful/failed stdout/stderr, many-entry bounded archives/JSONL and metadata. Record retained/temporary heap and hosted limits; a fixture for one adapter is not all adapters. |
| Release/CI | Affected registered owner controls then selected full Rust/Python/daemon/hosted inventory on final config/binaries. Resolve actual tests from live collection; selected ignored public process cases require execution, not zero-selected success. |
| Operational P11 | [S21-12](../../sep-21-search-plane-sota-hardening/tickets/S21-12-cross-repo-terminal-receipt-cutover.md) owns missing typed deploy/activate/restore-forward producers/recipes, actual actions, independent observers and authorized target inputs. Existing CAS and staged proof nodes do not implement that operational producer. |

## Integration order

1. Inspect source/dirty ownership and selected local/installed/hosted/benchmark
   claims. One integration owner changes shared DTO/schema/dependencies/registry/
   selectors; coverage, gold and capture owners keep disjoint implementation scope.
2. Complete demonstrated owner defects and focused independent controls. Coverage
   cost optimizations retain pre-intent and lock-time refusal, identity/tamper/
   retry/lineage/old-reader and fresh-rebuild equality. Missing measurement does
   not justify weaker verification or sublinear whole-ingest claims.
3. Prepare independent source/gold and blind native inputs. Apply current request
   profiles, complete repository × family × mode inventory and explicit failed/
   unsupported outcomes. One canonical suite/admission, decoder and report owner
   remain; `QUALITY_DELTA` evidence validity is separate from useful positive
   effect and a frozen critical-stratum/default-decision policy.
4. Integrate selected source once, enroll affected tests/normative ADRs and run
   affected owner rails. Run selected full/installed/issuer/restart/rollback/native
   adapter/Linux/hosted scopes independently; each has its own terminal status.
5. Admit native index/source/output units and host before qualified quality,
   equal-work performance, incremental/recovery or default decisions. Reuse
   compatible captures; relevant source/input/config drift rechecks affected proof.

## Required controls

- Contract/engine: legal domain/projection/empty/count/cursor, strict stored-field
  decoding, same-path federation/global top-k, wrong source, interruption/resource
  and admitted generation lifetime.
- Publication: failed-parse replacement/repair, replay/conflict/reorder, Delta
  inheritance, readers during writes, activation/retention and actual crash cuts.
  Seal/open/tamper/residency and unchanged-segment fixtures use the current format.
- Preview/Explain: original NFC/folded/UTF-8/CRLF bytes, Boolean focus, oversize/
  overlap, optional refusal, score/provenance, restart and mutable-checkout drift.
- Producer: current grammar/parser/ownership, retained malformed source,
  capability-enabled lexical and strict-symbol Vite through actual daemon.
- Native I/O: original bound raw/metadata, large output/failure and every selected
  format/entrypoint; no normalized-only or fixture-only qualification.

Use `Justfile`/`scripts/cargow` and current test authority. Formal release/replay
and qualified benchmarks bind their required source/inputs/dependencies/binaries/
environment. Missing labels/products/host/platform block only dependent claims.
Final-source full/platform proof belongs to MISC-04/05; measurement admission
belongs to BENCH-01/03/04 and MISC-06/07. Exact prior audits and execution bodies
are recoverable through [the plan archive](../../ARCHIVE-INDEX.md#oct-05-benchmark-and-quality-ledger-compaction).
