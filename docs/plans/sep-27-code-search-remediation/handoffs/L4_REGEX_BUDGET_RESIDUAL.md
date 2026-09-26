# L4 residual — aggregate regex allocation admission

Status: **BLOCKED for aggregate heap qualification**. This is a source-backed
accounting/proof gap, not a reproduced runtime memory overrun. Native preview behavior and this aggregate allocation claim are separate;
use the latest source-bound receipt in L4_HANDOFF.md for executed behavior.

## Current boundary

- `searcher/candidates.rs::SelectedPreviewContext::prepare_leaf` checks a 64 KiB
  source-pattern cap and reserves 16 MiB per request-local executor before
  compiling. This bounds retained executor count under the shared ledger.
- `RegexExecutor::compile` runs AST validation, HIR parsing, dialect checks and
  the NFA-state estimator, then `regex::bytes::Regex::new`. The estimator runs
  after HIR allocation. Preview compilation repeats the canonical pipeline.
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
  handles. Merely raising the fixed charge would introduce another unproved
  estimate.

Exact dependency/source digests and the limited inspection claim are recorded
in [dependency-audit.json](l4-proof/p02/dependency-audit.json).

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
narrowing query recall, or inventing a global compiled cache. A larger fixed
reservation alone is not a proof. No runtime aggregate memory overrun has been
established by the audit.

## Acceptance evidence still needed

Use valid capture-heavy and Unicode-class/repetition patterns, malformed
patterns, multiple distinct request leaves, a repeated leaf, and interrupted
compilation/search. Assert admission occurs before the expensive allocation,
retained and temporary reservations have the correct lifetime, and optional
refusal leaves hit selection unchanged. Verify ordinary regex range fixtures
and existing truth/recall tests remain unchanged. Bind receipts to the final
dependency graph, flags, source and binaries. Aggregate allocation accounting
and process RSS are separate claims.
