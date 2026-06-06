# J7Q-06 — Ambiguous Intent Handling

Parent RFC: [../rfc.md](../rfc.md)

Status: `planned`

## Goal

Preserve fail-closed query behavior while making common user mistakes easier to
repair.

## Current Code Fact

- fail-closed typed errors are strong
- unsupported and ambiguous query classes already exist
- repair guidance is still limited

## Owner Seam

- bridge error surface
- CLI rendering
- SDK error payload rendering

## Source Truth Anchors

- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-lq-bridge/src/errors.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchctl/src/lib.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-sdk/src/error.rs`

## Preferred Implementation Direction

- add typed repair metadata such as supported shapes, example queries, and
  route hints
- preserve canonical error codes and fail-closed runtime behavior
- make repair guidance available to CLI and SDK from the same source payload
- attach docs anchors from typed payloads instead of CLI-only prose

## Layer Boundary Clarification

- this ticket owns repairability of typed failures
- it does not own ranking or operator diagnosis commands

## Concrete Work Items

1. Add repair hints for common typed failures.
2. Surface supported alternative shapes when safe.
3. Keep ambiguous, unsupported, and wrong-route errors distinct.
4. Avoid any silent rewrite or best-effort fallback.

## First Increment

- identify the top repeated query-shape failures and add repair metadata for
  those only

## Red Rail To Pin First

```bash
./scripts/cargow test -p quanta-index-searchd-runtime --test e2e_filter_execution -- --nocapture
./scripts/cargow test -p quanta-index-searchctl --test cli_smoke -- --nocapture
```

## Worker First Commands

```bash
rg -n "BridgeAmbiguousFilter|unsupported|typed_error|usage|error:" crates -S
sed -n '1,240p' crates/quanta-index-lq-bridge/src/errors.rs
sed -n '1,260p' crates/quanta-index-searchctl/src/lib.rs
```

## No-Go

- do not weaken fail-closed behavior into silent fallback
- do not collapse distinct error classes into one generic help string

## Reviewer Rejection Checklist

- reject if the fix rewrites the query silently
- reject if ambiguous, unsupported, and wrong-route errors still share one
  generic hint
- reject if repair guidance exists only in CLI copy and not in typed payloads

## DoD

- typed query failures remain strict but become easier to repair

## Not Done If

- ambiguity is still only understandable by reading source
- repair hints silently rewrite semantics
