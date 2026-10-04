# OCT-04-002 — Effective configuration and generation policy

Status: `Proposed` — no configuration, wire or format change is authorized.

Source audit: clean `main@e43cda8c87b4a06fecac82a266011e30f84a2986` on 2026-10-04.
The Sep-23 RFC and file-level plan predate this source and are recoverable from
Git history. Accepted [SEP-21-001](SEP-21-001-canonical-identity-and-digest-domains.md)
and [SEP-21-003](SEP-21-003-read-view-continuation-and-provider-policy.md)
remain authoritative.

## Current boundary

- `SearchdConfig::from_env` resolves serve policy; the CLI exposes `serve` and
  offline backup/restore/verify, with no `config check/show` command.
- The semantic derive-mode environment knob is retired. The current semantic
  manifest format is 12 (`semantic/src/manifest.rs`), so the old plan's
  proposed format 10-to-11 transition is obsolete.
- Sealed lexical/semantic artifacts and provider identity checks exist, but a
  shared `GenerationCompatibility` / `ResolvedGenerationPolicyV1` gate is not
  present in the current source. A config parse is not a boot, provider or
  historical-generation compatibility proof.

## Open decision

If operators require a stable effective-policy inspection, define one typed,
redacted resolution used by serving and by a side-effect-free `config check/show`.
Enumerate included fields and value origins; exclude credentials and raw paths.
Bind the configured provider endpoint to the actual transport identity before
claiming that an egress grant authorizes that destination.

If generation policy admission is needed, check exact seal-backed evidence at
full-pair activation/restart and only declared tracks at query acquisition,
including resident cache hits, before semantic provider I/O. Preserve physical
corruption versus intact-policy-mismatch outcomes. A new origin/format claim
requires a fresh live-row inventory, candidate-root rebuild decision, producer
proof and explicit old-root refusal. Do not reuse the Sep-23 version numbers or
assume typed origin from a producer-controlled digest.

Required first proof: current CLI/config/provider and seal inventory, an
independent mismatched-generation negative on cold and resident paths, and a
source-bound migration inventory. Implementation, cross-repository producer
compatibility and release qualification are `NOT_RUN`.
