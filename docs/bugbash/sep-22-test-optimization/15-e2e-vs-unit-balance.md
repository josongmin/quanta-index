# E2E-vs-Unit Balance Audit

Scope: behaviors currently proven only through heavy e2e (real `searchd` binary boot,
multi-socket `E2eRuntime`, shell-spawned daemon, CLI subprocess) that contain a pure
policy/arithmetic core deserving a fast unit test; plus unit-missing error/policy-denial
branches. Every cited file body was opened and inspected.

Fast-unit model already in repo: `crates/quanta-index-core/tests/ingest_resource_policy.rs:1`
(pure arithmetic oracle, no sockets) and
`crates/quanta-index-core/tests/generation_serving_policy.rs:1` (pin table, no daemon).
Heavy model: `crates/quanta-index-searchd-runtime/tests/common/searchd_binary_process.rs:23`
(`Command::spawn` + 30 s `wait_for_sockets` over query/control/ingest sockets).

## Findings

### F1. Ingest preflight fault matrix proven only over the real socket
- File: `crates/quanta-index-searchd-runtime/tests/e2e_ingest_preflight.rs:1` (2 `#[test]`s cover ~6 raw-IPC refusal shapes + 2 SDK shapes; oracles read catalog file + disk tree walk).
- Symptom: digest/shape/delta-base validation logic is exercised exclusively via full daemon boot + ingest socket round-trip.
- Why-bad: each refusal shape (invalid mode/base, unsealed delta base, empty/wrong-shape digest, body/digest mismatch) is a pure function of the batch; e2e failure cannot localize which validator broke, and the matrix costs a daemon boot per run.
- Severity: H
- Concrete fix: add unit tests calling the preflight validator directly. Proposed location: `crates/quanta-index-core/tests/ingest_preflight_shape.rs` (new; mirrors `ingest_resource_policy.rs`). Sketch:
  ```rust
  #[test] fn delta_on_unsealed_base_refused() { /* build SearchCorpusIngestBatch, call preflight fn, assert DELTA_BASE_NOT_SEALED code */ }
  #[test] fn digest_mismatch_refused() { /* stamp_batch_digest_v1 then flip one body byte, assert BATCH_DIGEST_MISMATCH_CODE, zero side effects */ }
  ```
  Keep one e2e case as the wiring proof; move the other 5 shapes to unit.

### F2. Ingest resource envelope re-proven through a tight-envelope daemon
- File: `crates/quanta-index-searchd-runtime/tests/e2e_ingest_resource_envelope.rs:1` (boots daemon under tight envelope, sends over-limit batch, walks disk for absence of generation dir).
- Symptom: `IngestResourcePolicy::admit_search_corpus_batch` (`crates/quanta-index-core/src/ingest_resource.rs:45`) already has a fast oracle in `crates/quanta-index-core/tests/ingest_resource_policy.rs:1` (boundary at one-over/bound), but the e2e file re-proves the same boundary through sockets.
- Why-bad: duplication — the e2e adds only "refusal leaves no generation dir", the rest is policy arithmetic already covered cheaply. Slow signal for a one-line policy regression.
- Severity: M
- Concrete fix: shrink e2e to a single over-limit + single fits case (the no-side-effect wiring); move per-ceiling (records/text/vector) boundary rows to `ingest_resource_policy.rs`. Sketch: `#[test] fn vector_bytes_boundary() { policy with vector_bytes=N; batch of N+1 bytes refused, N admitted }`.

### F3. Umask hardening proven only by spawning the real binary under `sh`
- File: `crates/quanta-index-searchd-runtime/tests/e2e_umask_hardening.rs:32` (`sh -c 'umask "$1" && exec ... serve'`, 30 s socket + exit timeouts, `stat` oracles).
- Symptom: the only unit-adjacent code, `harden_umask()` in `crates/quanta-index-searchd/src/app/umask.rs:25`, has no `mod tests` (verified: grep finds `mod tests` only in `process_memory.rs`, not `umask.rs`).
- Why-bad: umask is process-global so it cannot be unit-tested in-process in parallel, but the *constants and mode arithmetic* (`DAEMON_UMASK=0o077`, expected `0700`/`0600` outcomes, `STATE_ROOT_INSECURE` refusal predicate) are pure and currently only covered by a shell-spawned binary.
- Severity: M
- Concrete fix: fast test at `crates/quanta-index-searchd/src/app/umask.rs` (`#[cfg(test)] mod tests`, same file as `process_memory.rs:104` pattern) asserting constants and a pure `mode_after_umask(mode, mask)` helper if extracted; plus a unit test for the insecure-root predicate (mode `0777` → refuse) without booting. Sketch:
  ```rust
  #[test] fn daemon_mask_is_owner_only() { assert_eq!(DAEMON_UMASK, 0o077); }
  #[test] fn world_writable_root_refused() { assert!(is_state_root_insecure(0o777)); }
  ```
  Keep the shell-spawn e2e as the single kernel-truth proof.

### F4. Socket-access policy matrix split across two heavy harnesses
- Files: `crates/quanta-index-searchd-runtime/tests/e2e_socket_access.rs:1` (real daemon via `E2eRuntime`, `stat` + metrics-scrape oracles) and `crates/quanta-index-ipc/tests/socket_access.rs:1` (real `UdsServer::run` + stub dispatcher + real socket files under `/tmp`).
- Symptom: mode/gid arithmetic (`PRIVATE_SOCKET_MODE`, `GROUP_SOCKET_MODE`, non-member-gid selection at `e2e_socket_access.rs:55`) and the bind-refusal for untraversable paths are proven through bound sockets in both places.
- Why-bad: the e2e half duplicates the IPC-crate half's accept-path proof (the e2e header itself admits at `e2e_socket_access.rs:10` that stranger-refusal is proven in the IPC crate). Two heavy suites for one policy.
- Severity: M
- Concrete fix: keep IPC-crate tests as the canonical policy suite; shrink e2e to boot-report gauge check only. Add fast unit tests in `crates/quanta-index-ipc/src/` (next to policy definition) for mode constants and `SharedSocketAccess::new` validation (e.g., empty-path / untraversable-path refusal without binding). Sketch: `#[test] fn shared_policy_requires_traversable_dir() { ... }`.

### F5. Process-memory envelope gate driven through the daemon timer
- File: `crates/quanta-index-searchd-runtime/tests/e2e_process_envelope.rs:1` (scripted probe moved above/below ceiling, metrics scrape, idle-sweep on daemon timer; 4 `#[test]`s).
- Symptom: the gate decision (probe value vs ceiling → typed `PROCESS_MEMORY_ENVELOPE_EXCEEDED` / writer refusal) is pure, and the VmRSS parser already has in-file unit tests (`crates/quanta-index-searchd/src/app/process_memory.rs:104`), but the *gate predicate + boot-refusal for inconsistent envelope* has no unit test — only e2e.
- Why-bad: timer-driven e2e is slow/flaky by construction (idle sweep on daemon clock); a ceiling-comparison regression waits for the slowest suite.
- Severity: M
- Concrete fix: fast test at `crates/quanta-index-core/tests/process_memory_gate.rs` (or extend existing envelope policy test) with the scripted probe pattern already in the e2e (`ScriptedProbe` at `e2e_process_envelope.rs:32` is trivially portable): probe above ceiling → refused with code; below → admitted; envelope inconsistent with ceiling → boot refusal. Sketch:
  ```rust
  #[test] fn probe_above_ceiling_refuses() { /* ScriptedProbe(u64::MAX) vs small ceiling → PROCESS_MEMORY_ENVELOPE_EXCEEDED_CODE */ }
  ```

### F6. Metrics-scrape delta proven only by generating live traffic
- File: `crates/quanta-index-searchd-runtime/tests/e2e_metrics_scrape.rs:1` (sends 5 queries + 1 typed error between two scrapes, asserts exact counter/histogram deltas incl. `SAMPLES_PER_SERVED_LEXICAL_QUERY` constants).
- Symptom: the counter-arithmetic (which events each query shape emits) is a pure function of the route outcome, tested only via socket traffic + double scrape.
- Why-bad: exact-delta assertions over live traffic are order- and timing-sensitive; a miscounted emission maps to a 7-vs-5 sample diff with no unit pointer to the emitting call site.
- Severity: L
- Concrete fix: unit test the emission function in the observability domain (`crates/quanta-index-core/src/domains/observability.rs` — policy file found in inventory) asserting per-outcome counter sets; keep one e2e scrape as end-to-end wiring. Sketch: `#[test] fn served_query_emits_seven_samples() { ... }`.

### F7. Boot-quarantine decisions proven only via corrupted generation dirs
- File: `crates/quanta-index-searchd-runtime/tests/e2e_boot_quarantine.rs:1` (corrupts manifests/identities on disk, reboots daemon, asserts start/refuse + typed `NOT_READY`/`UNKNOWN` codes + boot report listing).
- Symptom: the quarantine *decision* (which damage → quarantine vs refuse-boot) embeds validator + policy logic with no fast table test; contrast `generation_serving_policy.rs:1` which does exactly this pattern for the serving boundary.
- Why-bad: each damage shape requires a fresh daemon + disk corruption + reboot; the decision table (inactive-damaged → quarantine+serve; active-damaged → refuse-boot) deserves the same table-oracle treatment as the pin policy.
- Severity: H
- Concrete fix: new fast table test `crates/quanta-index-core/tests/generation_quarantine_policy.rs`: rows of (damage location × active/inactive) → (quarantine | refuse-boot | serve), asserting `GenerationQuarantineReasonV1` codes directly. Sketch:
  ```rust
  #[test] fn inactive_content_damage_quarantines() { ... }
  #[test] fn active_semantic_damage_refuses_boot() { ... }
  ```
  Keep e2e for the two headline cases (start vs refuse + report listing).

### F8. CLI smoke spins a real UDS server per scenario
- File: `crates/quanta-index-searchctl/tests/cli_smoke.rs:73` (`lexical_json_roundtrip`, `explain_pretty_roundtrip`, … each boots `UdsServer` + `ScenarioDispatcher` + temp socket; `NEXT_SOCKET_ID` at line 52).
- Symptom: what is actually asserted is CLI output formatting (JSON/pretty rendering of contract types) — pure serialization — but each case pays for a server bind, dispatch, and socket round-trip.
- Why-bad: 7 scenarios × server setup for string formatting; a rendering regression (field rename, pretty indent) is slowest to catch where it is cheapest to test.
- Severity: M
- Concrete fix: fast tests in `crates/quanta-index-searchctl/src/` (formatter module `#[cfg(test)]`) calling the render function on fixture responses directly, no sockets. Sketch: `#[test] fn lexical_pretty_contains_digest() { assert!(render_pretty(&fixture()).contains("digest")) }`. Keep one socket round-trip (lexical JSON) as the IPC wiring proof.

## Unit-missing branches (error paths / policy denials with no fast test)

- U1. `crates/quanta-index-searchd/src/app/umask.rs:25` — no `mod tests`; denial branch (`STATE_ROOT_INSECURE` on `0777` root, `e2e_umask_hardening.rs:11`) has zero unit coverage. See F3.
- U2. Crash-point coverage table (`crates/quanta-index-searchd-runtime/tests/e2e_crash_matrix.rs:46`, `SearchdBinaryProcess` + `QUANTA_INDEX_CRASH_POINT` + exit 86): the 4 crash tests each boot, crash, and restart a real binary. The *table-completeness* check (every declared point has a row) is already a fast test inside that file — keep it — but per-point disk-state predicates have no unit test; candidate for in-process state-machine tests if the seal state transitions are factored pure. Severity L (genuinely needs the binary for crash semantics; only the predicate extraction is actionable).
- U3. Supervisor lifecycle (`crates/quanta-index-searchd-runtime/tests/runtime_supervisor_owner_v1.rs:1`) is already the good pattern: synthetic threads + helper processes from the test binary, no release daemon — no change needed; cited as the model for F5/F7 extraction.
- U4. Admission denials (`SERVER_OVERLOADED`, connection-cap close, deadline-exceeded in `crates/quanta-index-ipc/tests/admission.rs:9`, per-repo cap + slowloris in `crates/quanta-index-ipc/tests/repo_admission_and_slowloris.rs:4`) run real `UdsServer::run` loops with barrier handshakes — appropriately heavy (concurrency semantics need the loop), but the *budget-deadline computation* (`RequestBudgetV1` from policy) is extractable to a unit test asserting deadline = policy value without threads. Severity L.

## Proposed priority
1. F1 + F7 (H): preflight shape matrix and quarantine decision table — biggest boot-count savings.
2. F3 + F5 (M, cheap): umask constants/predicate and memory-gate predicate — small pure extractions.
3. F2 + F4 + F8 (M): shrink duplicated e2e to one wiring case each; move boundary/format rows to unit.
4. F6 + U2 + U4 (L): emission-count and predicate extractions when touching those files.
