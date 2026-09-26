# JUN-06-001 — Sourcegraph Compatibility Boundary

Status: `Accepted`

Decided: 2026-06-07

Consolidated: 2026-09-27

Source programs: Jun-4 DSL extension, Jun-4 Sourcegraph parity, Jun-5 tail gaps
and Jun-6 Sourcegraph expansion

## Context

Compatibility work originally tracked parser acceptance, lowering, storage
authority and runtime behavior in separate tickets. That made parse-only or
alias-only work easy to overstate as Sourcegraph support.

## Decision

### Promotion rule

A Sourcegraph-shaped surface is supported only when all applicable conditions
hold:

1. the owning parser or bridge admits the exact shape;
2. lowering targets an existing native semantic authority or a separately
   defined Sourcegraph-only authority;
3. an exact runtime or public front-door rail executes the shape;
4. native and Sourcegraph forms have parity proof when both exist;
5. neighboring unsupported shapes retain explicit typed verdicts;
6. `sourcegraph_parity.py --check` and the DSL capability checker agree with
   the code-owned registry.

Parsing, normalization or lowering alone cannot satisfy the rule. A surface
that fails the rule is explicit unsupported rather than partial support.

### Authority separation

Repository file/content, description, metadata, topic, commit-recency,
ownership and contributor predicates consume distinct producer-published
authorities. They cannot be synthesized from snippets or conflated because
their wire shapes look similar.

Aliases canonicalize onto the same authority and semantics as their canonical
form. Combination predicates preserve same-document or same-record
correlation; they cannot widen into repository-level cross products.

Revision-at-time is a revision-selection and pin-rebinding operation. It is
not implemented as a lexical filter.

Contributor regex matches declared structured identity fields. It cannot use
an undocumented canonical-string fallback.

### Structural boundary

Sourcegraph structural search may combine only leaf kinds admitted by the
code-owned legality matrix. Direct lexical phrase or regex siblings remain
unsupported unless a later decision introduces exact execution semantics and
parity proof. Structural-body syntax and lexical siblings are separate forms.

### Claim boundary

The decision describes compatibility for the checked inventory, not marketing
parity with every current or future Sourcegraph feature. A new upstream syntax
cell starts as unsupported until the promotion rule passes.

## Consequences

- Capability matrices are generated or mechanically checked against code.
- New producer authority requires an ingress round trip in addition to local
  query execution.
- An unsupported verdict is a valid terminal decision and does not reopen a
  completed program.

## Historical record

The four implementation waves are indexed in
[the completed-plan archive](../plans/ARCHIVE-INDEX.md).
