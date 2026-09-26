# L5 adversarial follow-up audit

Status: **VERIFIED for the bounded scope below**. Four reproduced P2 defects were
fixed. No confirmed finding remains open in that scope. This is not a proof that
the whole repository has no defects.

Source: `98601a66d8cab9c86232b3e62ce490c8b43b71b6`, dirty. Snapshot: `/Users/songmin/.codex/worktrees/l5-adversarial-proof/quanta-index`.
Three edited source/authority files match the shared checkout exactly. Later shared
ZIP streaming changes in `run.py` and its test fixtures were preserved; the audit's
changed functions remain identical. A second frozen snapshot verifies 18 current
integration cases against those shared changes. The full 477-test proof remains
bound to its first snapshot. No commit or push was made.

| Finding | Counterexample and correction |
| --- | --- |
| L5-AUD-01 / P2 | Extensionless `rs` and `json` were classified by their whole filename. Require an actual dot; lexical coverage/chunks and semantic records now carry `text` with Unsupported symbol coverage. |
| L5-AUD-02 / P2 | Removing diagnostics and recomputing artifact hashes passed despite unused capacity. Enforce exact file/global retention and the single unsupported diagnostic; legitimate budget truncation still passes. |
| L5-AUD-03 / P2 | A raw coverage state of `[]` raised an uncaught TypeError. Reject its type explicitly and produce a failed pair verdict. |
| L5-AUD-04 / P2 | The strict shared reader refused its own temporary authority file via macOS `/tmp`, failing otherwise valid contract/SDK receipts. Resolve the internally allocated directory while preserving rejection of symlinked input evidence. |

## Current evidence

- Rust owner: **132 passed** (87 library, 3 runner, 25 chunking, 17 parser).
- Python consumers: **477 passed**, no failures, errors or skips. The exact
  retrieval inventory of **328** is registered and matches the executed JUnit identities.
- Current consumer replay accepts the prior frozen Vite **256-file** raw preflight.
  Removing its actual syntax diagnostic, with a recomputed digest, is rejected.
- Later shared-consumer integration: **18 passed**, covering the audit regressions,
  valid contract/SDK receipts and command/context/archive tampering refusal.
  Its duplicate-ZIP test initially expected a later rejection message. The archive
  was already safely refused at the central-directory count bound. A concurrent
  writer corrected the test expectation before our edit; that correction was
  preserved and included in the final integration run.
- A further shared input-custody change was checked directly in the live checkout:
  both the required-inventory boundary and valid contract/SDK verdict passed
  (**2 tests**, no source drift during execution). Each snapshot retains its own
  proof scope; these results do not qualify later shared edits automatically.
- Each successful rail has unchanged before/after inputs. The later temporary-path
  fix changes only Python source/tests and Python registration; Rust source and
  Rust test authority remain identical to the successful Rust snapshot.

Commands, source identities, raw failures before fixes, terminal outputs and
SHA-256 digests are in [L5_ADVERSARIAL_AUDIT.json](L5_ADVERSARIAL_AUDIT.json) and
[the evidence directory](l5-proof/adversarial-20260927).

The initial broad Python run was interrupted after exposing the shared temporary-path
regression. It is not successful evidence; the final full selection above was rerun.
The previous [completion](L5_COMPLETION.json) retains its original snapshot meaning.
Fresh SDK/daemon and fresh Vite process runs were **NOT_RUN** in this follow-up;
the existing raw Vite replay is consumer verification. Repository-wide CI, clean
commit/release and performance qualification remain outside this audit.
