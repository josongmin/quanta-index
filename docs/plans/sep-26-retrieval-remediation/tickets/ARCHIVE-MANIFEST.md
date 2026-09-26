# SEP-26 Retrieval Remediation Archive Manifest

Status: `HISTORICAL`

Archive boundary: clean pre-consolidation revision
`5e6addd5814ce8b71af808ff201ddd0b18fbe6c4`, captured 2026-09-27.

The files below remain at their stable paths so source comments, external notes and historical links continue to
resolve. Git at the archive boundary preserves their exact pre-consolidation bytes. Current architecture lives in the
accepted ADRs; current unfinished work lives in [GAP-REGISTER.md](GAP-REGISTER.md).

| Historical file | Preserved purpose | Current owner |
|---|---|---|
| [AUDIT.md](AUDIT.md) and [audit-evidence.json](audit-evidence.json) | Initial defect inventory and small machine-readable snapshot | Historical only |
| [CURRENT-AUDIT.md](CURRENT-AUDIT.md) | Chronological source, command, failure and receipt ledger | Historical only |
| [IMPLEMENTATION-WAVE.md](IMPLEMENTATION-WAVE.md) | Parallel ownership and serial integration record | Historical only |
| [PROFILE-CONTRACT.md](PROFILE-CONTRACT.md) | Detailed profile/provenance draft used during implementation | SEP-26-001/002/003 |
| [RBR-00](RBR-00-proof-contract.md) | Proof inventory and source closure work packet | SEP-26-003; G-01 |
| [RBR-01](RBR-01-diagnostics.md) | Response and stage observation work packet | SEP-26-002; G-02 |
| [RBR-02](RBR-02-query-policy.md) | Query-policy work packet | SEP-26-001; G-01 |
| [RBR-03](RBR-03-semble-profiles.md) | Comparator-profile work packet | SEP-26-002; G-03 |
| [RBR-04](RBR-04-symbol-producer.md) | Source-symbol producer work packet | SEP-26-001; G-03 |
| [RBR-05](RBR-05-symbol-route-proof.md) | Published-unit and symbol-route proof work packet | SEP-26-001; G-01 |
| [RBR-06](RBR-06-span-chunking.md) | Span-accounting and chunking experiment work packet | SEP-26-001; G-03 |
| [RBR-07](RBR-07-semantic-parity.md) | Encoder parity and ANN decomposition work packet | SEP-26-002; G-03 |
| [RBR-08](RBR-08-symbol-ranking.md) | Symbol routing/ranking work packet | SEP-26-001/002; G-04 |
| [RBR-09](RBR-09-query-performance.md) | Hybrid fetch and query-performance work packet | SEP-26-002; G-02/G-03 |
| [RBR-10](RBR-10-ingest-performance.md) | Ingest observation/performance work packet | SEP-26-002; G-05 |
| [RBR-11](RBR-11-resource-accounting.md) | Process-tree resource accounting work packet | SEP-26-002; G-06 |
| [RBR-12](RBR-12-evaluation-closeout.md) | Evaluation and closeout work packet | SEP-26-003; G-01/G-03 |
| [TEST-PLAN.md](TEST-PLAN.md) | Active oracle, command and qualification contract | Active, not archived |

## Retrieval

Use `git show 5e6addd5814ce8b71af808ff201ddd0b18fbe6c4:<path>` for the exact packet before consolidation. Later edits to a
historical file must state why the archive record itself changed; they cannot silently alter an accepted decision or
close an active gap.
