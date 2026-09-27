# J7Q-02 — Snippet and explanation acceptance

Status: `ACTIVE_RESIDUAL`
Parent: [quality index](INDEX.md)
Owner: lexical previews, explanation contract, SDK/CLI consumers

Structured window/highlight and planner/engine/contribution contracts exist;
`snippet.rs` grades emitted phrase/regex/multi-hit/long-line fixtures without
repairing output, and `ui.rs` observes highlight/explanation fields. Current
contracts are in [the code-search ADR](../../../adr/SEP-27-003-code-search-source-and-preview-contract.md).

## Remaining acceptance

- Prove current phrase, regex, repeated-hit, long-line and symbol previews
  against source bytes: exact windows/offsets, UTF-8 boundaries, bounded length,
  deterministic truncation, empty/unavailable states and advertised highlights.
- Reconcile required planner stages, engine ordering and contribution rows with
  actual execution/ranking. Required route-specific rationale must carry useful
  provenance; a populated summary alone is insufficient.
- Execute current-wire contract, SDK and CLI consumer cases. The harness fixture
  cannot replace these or the physical regex work in
  [CS-ENG-04](../../sep-27-code-search-remediation/rfcs/CS-ENG-04-match-anchored-snippets.md)
  and combined-source acceptance in
  [CS-INT-01](../../sep-27-code-search-remediation/rfcs/CS-INT-01-integration-and-qualification.md).

Registered producer: `snippet_matrix`; emitted `summary.json` and
`golden_windows.json` retain the scored native output. Snippet correctness and
human usefulness remain separate from relevance quality; keep failures visible.
