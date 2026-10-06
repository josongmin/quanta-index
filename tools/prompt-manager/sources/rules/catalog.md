## Rule Catalog

### Safety

- no silent failure
- no silent fallback
- fail-closed by default
- no error swallowing or error-to-default replacement on production paths
- no heuristic authority when the real authority is absent
- no unverifiable completion claim
- no `ok` status when required inputs, correctness-affecting assumptions, failed checks, or tool errors remain

### Architecture

- shared contract crate is the only producer/search-plane integration surface
- core crate must not import vendor storage/index libraries (e.g. `tantivy`) or raw filesystem layout
- storage/query vendor choices belong to adapters only

### Search-plane authority

- `generation_activation_state` is the only query-time serve-head authority; `generation_catalog` may contain future generations and does not become serve-head by itself
- generation resolution order is fixed: explicit request generation -> query-time pin -> active generation; if no ready generation exists, return typed `NOT_READY` / `UNKNOWN_GENERATION` and never fall back to empty hits or a different generation
- `apply_bundle_delta(...)` mutates the target generation staging surface only; never patch the active query indexes in place
- producer is the sole authority for source bytes, git history, parse trees, semantic-source records, and provenance; search-plane code must not read repo source, shell out to `git`, run `tree-sitter`, or fabricate semantic source as fallback
- search-plane is the sole authority for embedding execution, model-contract validation, and embedding cache/storage over validated producer semantic-source records; producers must not emit authoritative vectors or duplicate search-plane embedding policy
- `BundleChannelPublisher::publish` is the only producer -> search ingress path; do not add side-band apply/legacy socket ingress

### Code shape discipline (write-time SOLID, no god code)

These checks apply **while you write**, not as a cleanup pass. Every diff should already satisfy them; if a nearby existing violation is visible, propose the fix in the same PR or flag it explicitly as a named follow-up — never silently leave it.

- **Single Responsibility** — one struct / one fn does one job. If you're tempted to put parsing + I/O + policy validation + response formatting in one function, split it before merging. Heuristic: a fn over ~80 lines, or a struct with > 6 fields of unrelated concerns, is a red flag.
- **Dependency Inversion at every composition seam** — when a struct holds a collaborator, that field's type MUST be a trait object (`Arc<dyn …Port>`) or a generic parameter, NOT a concrete adapter type. The only file allowed to name concrete adapters is the composition root (`searchd::app::runtime`). If you see `field: Arc<TantivyLexicalAdapter>` in dispatcher / query / hybrid code, that is a DIP violation — propose `Arc<dyn LexicalIndexOpenPort + Send + Sync>` immediately.
- **Open/Closed at variant edges** — adding a new IPC variant, channel op, or domain port must not force a sprawling edit across encode + decode + match + factory + test all at once. If you're editing four or more places to add one variant, the surface is closed wrong; lift the dispatch into a trait or registry.
- **Interface Segregation** — a trait with `dispatch(BigEnum) -> BigEnum` for 4+ unrelated payload kinds is fat. Split it into narrow traits (`LexicalQueryPort`, `SemanticQueryPort`, …) and have one composition wrapper implement them all. Composition root does the routing, not the trait.
- **No mirror methods** — `lexical_seal` / `semantic_seal`, `drain_lex` / `drain_sem`, `apply_lex` / `apply_sem` are copy-paste smells. Lift the shape into one generic helper, one delegated type, or one enum-driven dispatch. Acceptable only when the two paths genuinely diverge at the type level (e.g. event types differ irrecoverably); say so in a comment and prove the divergence.
- **No fat enum variants in DTO** — when a struct grows > 8 fields, especially of mixed semantic intent, split it. When an enum variant inflates with `payload: Vec<u8>` "opaque CBOR" plus a side-channel doc comment, the encoding is part of the contract, not a comment — type it or document it precisely.
- **No vendor / transport tokens outside their owning adapter** — `tantivy::*`, `lance::*`, `ciborium::*`, `mmap`, `wal`, `segment`, `fsync`, `crc32` may appear only inside the crate that owns that vendor. Error messages must not leak them either: `CoreError::Storage("lexical: open generation directory: {err}")`, not `"lexical: mmap open …"`.
- **Doc-comments that contradict the code are bugs** — when you change behavior (e.g. payload encoding, fsync cadence, search semantics), update every doc-comment in the same PR. A `///` that lies is worse than no comment.
- **Dead port surface is forbidden** — defining a `pub trait FooPort` in core that no adapter implements and no caller imports is leaving load-bearing scaffolding under a tarp. Either implement it in this PR or delete it.

### Build hygiene

- no proc-macro derives for serialization: `#[derive(serde::Serialize)]`, `#[derive(serde::Deserialize)]`, `#[derive(Serialize)]`, `#[derive(Deserialize)]` are banned. Write manual `impl serde::Serialize` / `impl serde::Deserialize` instead. Reason: proc-macro expansion is the dominant build-time cost in serde-heavy crates; manual impls keep cold-build seconds bounded and make wire shape auditable.
- the ban applies workspace-wide (contract, core, adapters, searchd, tests, benches). The derive allowlist below is its single enforcement owner.
- derive allowlist (script `tools/ci/lint/check-rust-derive-allowlist.py`): every owned Rust source under `crates/` and `benchmarks/` is scanned; only `Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd, Error` (thiserror) are permitted. Any other `#[derive(...)]` arrival — `strum::EnumIter`, `clap::Parser`, `Deserialize_repr`, future proc-macros — must measure cost (cargo llvm-lines / --timings) and extend the allowlist explicitly. Reason: flips the serde rule from denylist to allowlist so new proc-macro deps cannot silently bloat cold-build time.
- monomorphization budget: `tools/ci/lint/check-llvm-lines.py` snapshots LLVM IR line counts for contract + core and fails PRs that exceed the baseline by >15% (and >5k absolute lines). Reason: derive allowlist enforces the rule; this guards the *outcome* — generic blowup or new proc-macros that slip past the allowlist still trip this gate.

### Contract surface

- public API of `quanta-index-contract` and `quanta-index-sdk` is snapshotted by `tools/ci/lint/check-public-api.py` against `tools/ci/lint/baselines/public-api/<crate>.txt`. Any drift fails CI; updating the baseline must be an intentional commit in the same PR as the breaking change. Reason: both crates are typed integration surfaces, so silent shape drift is impossible to review after the fact.

### Fail-closed deserialization

- IPC request/response decoders are fuzzed via cargo-fuzz under `crates/quanta-index-contract/fuzz/` in the explicit CircleCI heavy correctness rail (`just rust-fuzz-smoke`, `.circleci/config.yml` job `heavy-correctness`, 60s/target). Any panic, infinite loop, or non-`Err` exit on malformed bytes is treated as a fail-closed violation.

### Structural integrity (workspace shape)

- **Cargo.toml hygiene** — `tools/ci/lint/check-cargo-toml-hygiene.py` enforces, for every crate in `[workspace.members]`:
  - `[package]` inherits `version`, `edition`, `license`, `publish` from workspace
  - `[lints]` inherits from workspace
  - external deps (non-`quanta-index-*`) use `{ workspace = true }` form — inline-version (`serde = "1.0"`) and inline-path deps are banned
  - internal deps (`quanta-index-*`) use `{ version = "...", path = "../<crate>" }` form
  - no `git = "..."` sources; no path deps escaping `crates/`
  Reason: copy-pasted manifest ad-hoc adds fork the version line across crates and bypass `deny.toml` registry policy. Pre-commit + CI gated.
- **Module discipline** — `tools/ci/lint/check-module-discipline.py` enforces that every `mod.rs` (workspace-wide) and `quanta-index-contract/src/lib.rs` is a re-export facade only: attributes, comments, `use`/`pub use`, `mod`/`pub mod` only. No inline `fn`/`struct`/`enum`/`trait`/`impl`/`const`/`static`/`macro_rules!`/inline `mod {}` blocks. Reason: when implementation slips into mod.rs, the module's surface becomes uneven, renames cascade poorly, and the contract crate loses its DTO-only invariant at the entrypoint. Pre-push + CI gated. Multi-line `pub use { ... }` trees are recognized via brace-depth tracking.
- **Module cycles** — `tools/ci/lint/check-module-cycles.py` builds, per workspace crate, the dependency graph of its production modules (one node per module file; one edge per `crate::`/`super::`/`self::` path a module names another module's item by, facade re-exports resolved to the defining module; the crate root, a module's own submodules, `#[cfg(test)]` code, comments and strings excluded) and fails on every cycle. `tools/ci/lint/baselines/module-cycles.txt` lists the cycles still tolerated, each with why; a listed line whose cycle grew, shrank or disappeared fails too, so the list only shrinks by a reviewed edit (`--update-baseline`). Reason: two modules that reach into each other have no dependency direction — neither can change, be tested or be read alone, and the split is a file split rather than an ownership split (QI-BB-013 supplement 2). Pre-push + CI gated.
- **Error shape** — `tools/ci/lint/check-error-shape.py` enforces that every public `*Error` enum/struct in workspace source either (a) carries `#[derive(... Error ...)]` with `#[error("...")]` on each variant, OR (b) has both `impl std::error::Error for X` and `impl Display for X` in the same file. The manual form is preferred in this repo (consistent with the no-proc-macro-derive build-hygiene rule). Wire-protocol DTOs named `*Error` (currently: `SearchPlaneIpcError`) are allowlisted via `WIRE_DTO_ERRORS`. Reason: a `*Error` that is not `std::error::Error` cannot `?`-propagate, which usually leads to `.ok()` / `unwrap_or` silent fallbacks downstream. Pre-push + CI gated.
- **Module-tree snapshot** — `tools/ci/lint/check-cargo-modules-snapshot.py` snapshots `cargo modules structure --no-fns` output for `quanta-index-contract` and `quanta-index-core` into `tools/ci/lint/baselines/cargo-modules/<crate>.txt`. Any module rename, deletion, or relocation must accompany a baseline update. Complements `check-public-api.py` (external shape) with internal-shape freeze. Heavy correctness rail.
- **Wire-surface inventory** — `tools/ci/lint/check-wire-inventory.py` keeps `tools/ci/inventory/wire-surface.toml` exact against the code: every `SearchPlane{Query,Control,Ingest}Ipc{Request,Response}` variant in `crates/quanta-index-contract/src/ipc/{split,ingest}.rs` must be listed (both directions), and every on-disk format-version constant in workspace `src/` (`*FORMAT_VERSION`, `*_FORMAT`, `*SCHEMA_V<n>`, `*CONTRACT_VERSION`, the text normalizer stamp) must be named by exactly one `[[artifact]]` row with the value the code declares and a reproduction class (`producer-rebuild` / `current-format-backup` / `reject-only` / `cache` / `vendor-native`). Legacy roots and old receipts are refusal-only; backup/restore preserves current-format authority and is not an importer. Reason: plan §11 requires the consumer inventory before any state/wire cutover, and an opcode or format bump that lands without the inventory row is exactly the drift the cutover receipt cannot recover. Pre-push + CI gated.
- **Digest fallibility** — `tools/ci/lint/check-digest-fallibility.py` enforces that every public function returning `[u8; N]` for `N ∈ {16, 20, 32, 48, 64}` either returns `Result<[u8; N], _>` OR carries a doc comment containing the literal phrase `infallible by construction`. Reason: a digest function with a non-Result return forces the implementer into `panic!()`/`unwrap()`/heuristic fallback when the internal codec step fails — exactly the pattern that landed in `lq-ranker::weights_hash` v0. Pre-push + CI gated.
- **Semantic outcome honesty** — `tools/ci/lint/check-semantic-outcomes.py` uses the explicit `semantic-outcome-policy.json` enum inventory to reject negative-to-positive mapping, catch-all success (including an early `Ok` hidden before a terminal error), and the `StructuralReadiness` negative-to-empty/wrong-error projection or reason loss. An identity-preserving bound catch-all is allowed; `ExecutionOutcomeV2` negative-to-`None` remains valid when it means no exhaustion proof. Production `#[cfg(test)]` branches are excluded; parse errors overlapping governed syntax and any unclassified, missing, or renamed registered enum variant fail closed. This is a bounded syntax guard, not a substitute for owner tests or Rust type checking. Pre-push + CI gated.

### Silent-fallback guards (Rust AST lint and Semgrep)

- `tools/ci/lint/check-rust-fallbacks.py` owns seven production Rust syntax guards in one tree-sitter pass: `rust-no-silent-or-else-ok` blocks `.or_else` closures whose tail or explicit return synthesizes `Ok(...)` (including turbofish and standard `Result::Ok` paths); `rust-no-is-ok-as-branch` and `rust-no-is-err-as-branch` block Result predicates with an `else` block or `else if` continuation, including parenthesized conditions; `rust-no-debug-assertions-divergence` rejects actual `cfg!(debug_assertions)` and `#[cfg(debug_assertions)]` nodes in core/contract; the three search-plane authority rules reject direct `ciborium` and producer Git/parser paths regardless of module depth, plus `Command::new` and `std`/`tokio::process` imports. `std::process::id()` is not a process launch. Nested closure returns are not attributed to the outer recovery closure. Opaque macros containing governed tokens fail closed instead of silently passing; string and comment tokens alone do not trigger that refusal. Counterexamples run separately from broad tooling tests. These are syntax guards, not type or flow proofs.
- Clippy's `match_wild_err_arm` owns wildcard `Err(_)` branches; the Semgrep duplicate was removed.
- Clippy's `disallowed-methods` owns `Result::ok`; the Semgrep duplicate was removed. `.err().is_some()` / `.err().is_none()` remain ordinary presence predicates.
- A checked `u8`/`u16`/`u32::try_from(value).is_ok()` width predicate is excluded: canonical CBOR uses it for representability, and Clippy requires that spelling. A general two-branch Result inspection can route errors to an alternate algorithm, default or no-op. Single-branch `if x.is_err() { return Err(...); }` is permitted because it propagates explicitly. Use `?` or a typed `match` with explicit `Err` handling.
- `search-plane-no-process-spawn` / `search-plane-no-producer-parser-import` mechanically protect one part of the producer-owned source-authority boundary; they do not prove that every file read is authority-safe. The Semgrep suite retains workflow and structural-readiness rules, not duplicate search-plane Rust path guards. Its workflow `continue-on-error` guard rejects the YAML key for jobs and steps, including expressions, without matching shell text inside `run` blocks.
- Do not lint public `V<n>` type names as parallel IR by spelling alone: RepoMap layout/evidence types expose versioned artifact contracts. Enforce the single-IR rule against actual duplicate producer/consumer paths, not legitimate contract names.
- *deliberately not enforced automatically:* `if let Ok(x) = ... { ... }` with no else and `Result::map_or(default, ...)` saturation idioms are too common in legitimate best-effort or Option paths to lint accurately without type/intent evidence. Manual code review covers them.

### Verification

- compile claims require a real `cargo` run
- prompt-manager claims require `pm.py lint`
- behavior changes require tests
- structured agent outputs must validate against `tools/ci/agent/agent_output.schema.json`
- verification closeout must state covered vs excluded surface; a green targeted rail is not a repo-wide readiness verdict
- ship-conditional policy findings, runtime/contract greens, and readiness/activation blockers must be reported as separate layers when they differ

### Verification escalation by change surface

- public contract / SDK shape changes: run `just rust-public-api`
- crate/module/facade boundary changes: run `just rust-hexagonal` and `just rust-cargo-modules`
- activation, generation resolution, query pin, state-root, or shared-ingress changes: run `just rust-profile test-daemon` and prove the owning `U/E/C/H-SP` scenario slice
- generated agent-doc source changes: run `python3 tools/prompt-manager/pm.py sync`, `python3 tools/prompt-manager/pm.py lint`, and `python3 -m pytest tools/prompt-manager/tests/test_pm.py -q`

### Fuzz cadence (advisory)

- Fuzz is not required for every IPC decoder, wire DTO, or error-envelope change. Missing fuzz alone does not block routine code closeout.
- Recommend `just rust-fuzz-smoke` after substantial changes to decoders/parsers, wire formats, ingestion, or normalization, or when roughly two weeks have elapsed since the last completed fuzz run. These are guidance triggers, not mandatory gates.
- For small changes, use focused owner regression tests. Explicitly requested fuzz runs and selected proof/CI recipes retain their declared execution scope.

### Documentation

- generated docs are artifacts
- edit `tools/prompt-manager/sources/` only
- run `pm.py sync` then `pm.py lint`
