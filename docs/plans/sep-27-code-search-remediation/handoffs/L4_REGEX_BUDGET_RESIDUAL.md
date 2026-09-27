# L4 residual — aggregate regex allocation admission

> Historical report: one-off evidence files were removed from the repository. This report alone is not current verification.

Status: **BLOCKED for aggregate heap qualification**. A standalone allocator
probe reproduced a valid regex compilation exceeding the previous 16 MiB
preview reservation. The current source adds a state-proportional logical
charge before automata compilation; it does not enforce a total heap ceiling.
Native preview behavior and this aggregate allocation claim are separate;
use a receipt bound to the current source for executed behavior.

## Current boundary

- `searcher/candidates.rs::SelectedPreviewContext::prepare_leaf` checks a 64 KiB
  source-pattern cap and reserves 16 MiB before AST/HIR planning. It then
  reserves another 256 bytes per estimated NFA state before automata
  compilation. These logical charges limit request-local preview work and
  executor count; they do not measure or cap actual heap allocation.
- `RegexExecutor::prepare` runs AST validation, HIR parsing, dialect checks,
  the NFA-state estimator and capture removal. `compile_prepared` allocates the
  engine automata after the optional preview has admitted the extra charge.
  This preserves the canonical matcher and avoids a second parse in preview.
- In pinned regex 1.12.4, `size_limit` is a 10 MiB limit for each NFA. In pinned
  regex-automata 0.4.14, `meta::strategy::Core` can retain forward and reverse
  NFAs. The limit does not cover their sum or compiler temporaries.
- Search caches are separate allocations. The follow-up audit reproduced 513
  retained capture slots for 512 optional captures around a six-byte match.
  `RegexExecutor` now erases unobserved explicit captures through the canonical
  AST before engine compilation, leaving the whole-match slot. Original HIR,
  pattern and validation remain intact. Differential truth/range regressions
  cover the change. This removes capture-count amplification, but does not
  establish an aggregate bound for compiler temporaries and all engine caches.
- The public `regex::bytes::Regex` wrapper does not expose aggregate retained
  memory, compiler allocation admission, or the lower-level explicit cache
  handles. The state-proportional charge is a mitigation, not a proof that
  every accepted pattern fits it.

The allocator probe
used the pinned pre-remediation regex rlib and a single-thread `System`
allocator high-water counter. `\w{120}` compiled with a 20,033,383-byte peak;
`needle\w{120}` (with a mandatory prefilter literal) peaked at 20,035,301
bytes. Both exceed 16 MiB (16,777,216 bytes). This measures requested
allocation during standalone compilation, not process RSS or a worst-case
bound. The probe's source and raw TSV outputs are archived alongside its
provenance. `\w{150}` exceeds the existing NFA state cap and was rejected.

Exact dependency/source digests and the limited inspection claim are recorded
in dependency-audit.json.

## Required coordinated change

Retain one canonical regex matcher for truth and ranges. Give its compilation
and search-cache owner explicit resource admission covering AST/HIR temporaries,
retained automata, captures, and search caches before allocating them. Keep any
resource refusal distinct from invalid-pattern/integrity errors. An optional
preview refusal must continue to preserve the selected IDs, scores and order.

If a lower-level engine API is used, align its flags and match configuration
with the current bytes Regex builder and retain differential truth/range tests
against that existing matcher. Its memory reporting alone is insufficient:
post-allocation measurement does not enforce admission before allocation.

The user subsequently authorized shared-file edits. File ownership is no longer
an approval blocker. Potential integration surfaces remain the dependency
manifests, canonical prepared execution holders, and the request resource owner.
No cross-task messaging, task polling or coordination is authorized.

Do not resolve this evidence gap by silently disabling all regex previews,
narrowing query recall, or inventing a global compiled cache. Neither a larger
fixed reservation nor the new state-proportional charge is a total heap proof.
No runtime aggregate memory overrun has been established by the audit.

## Acceptance evidence still needed

Use valid capture-heavy and Unicode-class/repetition patterns, malformed
patterns, multiple distinct request leaves, a repeated leaf, and interrupted
compilation/search. Assert admission occurs before the expensive allocation,
retained and temporary reservations have the correct lifetime, and optional
refusal leaves hit selection unchanged. Verify ordinary regex range fixtures
and existing truth/recall tests remain unchanged. Bind receipts to the final
dependency graph, flags, source and binaries. Aggregate allocation accounting
and process RSS are separate claims.
