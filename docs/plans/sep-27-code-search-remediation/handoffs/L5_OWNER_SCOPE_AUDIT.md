# L5 owner and namespace audit

Status: **VERIFIED for the bounded scope** on `2102966246866398f01833bebf71396831377149` plus a three-file
L5 overlay at `/Users/songmin/.codex/worktrees/l5-owner-scope-audit/quanta-index`. Those three files match the shared checkout at closeout.
Other shared dirty inputs are not part of this snapshot's qualification.

Two further P2 defects were corrected:

- TS method signatures inside parameter, field and nested object types were
  incorrectly emitted as methods of a nearby named function, class or alias.
  Direct interface/class members and alias-owned direct, intersection and
  parenthesized object methods remain. Anonymous signatures are skipped before
  the symbol budget is charged.
- Dotted TS namespaces used raw whitespace/comments in qualified names and
  treated the full chain as a local name. The producer now derives each segment
  from AST identifiers, preserving source spans while canonicalizing identity.

Handwritten TS/TSX inventories assert exact name, kind, owner, source slice,
multiplicity and Complete count. Focused runs reproduced both false-owner and
namespace defects before repair. A focused alias-composition red run was queued
but interrupted before admission; its final regression passes in the full owner
run and is not presented as red proof.

- **Rust VERIFIED:** 141 passed (87 library, 3 binary, 25 chunking, 26 parser).
  All 138 registered owner identities match executed tests.
- **Python VERIFIED:** 16 selected preflight/receipt checks passed; 323 deselected.
- **Vite CLI VERIFIED:** 256 files retained, 255 Complete and one intentional
  ParseFailed; 2239 symbols. Allow-incomplete exits 0; strict
  exits 2 with the same census. The consumer recomputed the binary producer
  policy, grammar identity, source hashes and manifest universe.
- **Format VERIFIED:** rustfmt and changed-file whitespace checks passed.

The exact commands, environment, source and raw output digests are in
[L5_OWNER_SCOPE_AUDIT.json](L5_OWNER_SCOPE_AUDIT.json), SHA-256 `2cb968a25136cb39ebf162ada0e6368619a978537fda0442245e5c5b4a9ba6f2`.
Raw logs are stored as `.raw.txt` under
[l5-proof/owner-scope-audit-20260927](l5-proof/owner-scope-audit-20260927).

Whole-repository CI, fresh daemon/SDK queries, the full Python consumer suite,
clean-source release and deployment are **NOT_RUN** on this snapshot. The earlier
L5 receipts keep their original source identities and are not combined with
these results. This audit does not claim a complete semantic symbol table for
all language forms.
