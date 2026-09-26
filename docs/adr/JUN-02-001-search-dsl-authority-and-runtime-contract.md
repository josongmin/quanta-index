# JUN-02-001 — Search DSL Authority and Runtime Contract

Status: `Accepted`

Decided: 2026-06-02

Consolidated: 2026-09-27

Source programs: May-26 indexing residue, May-27 structural and master closeout,
Jun-2 final cut, hardening and advanced widening

## Context

The DSL implementation grew through lexical, history, runtime-catalog,
structural and bridge work packets. Those packets mixed architecture,
implementation order, transient capability tables and owner-local receipts.
Later widening changed individual supported cells, so the durable decision is
the authority model and promotion rule rather than one old capability snapshot.

## Decision

### Capability truth

Executable capability is code-owned. The predicate registry, Sourcegraph
legality table, route validators and machine-checked capability inventory are
the authorities. Planning prose cannot promote a surface.

Every surface has exactly one observable state:

- executable on a named route with an owning authority and proof;
- an intentional non-result carrier;
- explicit typed refusal;
- parser-only or unimplemented and therefore ineligible for a support claim.

Ambiguous or partial support is not a terminal state. A family is either
promoted with exact execution proof or demoted to an explicit unsupported
verdict. Later widening updates code-owned truth and its generated/checkable
inventory together.

### Route authority

- lexical text and predicates execute against lexical generation authority;
- history filters execute only on admitted commit or diff routes and use
  materialized history authority;
- runtime metadata reads a generation-pinned runtime catalog;
- structural search executes against producer parse-tree authority and a
  generation-pinned candidate universe;
- bridge directives such as external-analysis carriers remain typed packets,
  not fabricated search-result rows.

A route cannot silently reinterpret an unsupported leaf, return exact-empty
for missing authority, or fall back to a different engine. Unsupported
combinations fail before execution with stable typed diagnostics.

### Runtime catalog

Runtime-catalog publication is a validated authoritative replacement for its
declared generation and scope. Admission binds repository, revision,
generation, ordering evidence, batch digest and referenced document IDs.
Stale, conflicting or referentially invalid batches fail before mutation.

Runtime-only filters consume deterministic catalog-owned seed sets where the
catalog already owns the relation. Request-time repository scans and repair
fallbacks are not authority.

### Structural composition

Mixed lexical and structural boolean execution uses explicit set algebra over
one pinned candidate universe before projection. Pure-negative structural
queries require an explicit universe. Stable ordering, duplicate rejection and
typed unsupported boundaries apply before SDK projection.

## Consequences

- Current support must be read from executable registries and checked
  inventories, not from the archived ticket status.
- New syntax, aliases or predicate argument shapes need an owner seam, typed
  failure boundary and runtime proof in the same change.
- Owner-local proof does not establish repository-wide or cross-repository
  qualification.

## Historical record

The absorbed packets and exact pre-consolidation revision are listed in
[the completed-plan archive](../plans/ARCHIVE-INDEX.md).
