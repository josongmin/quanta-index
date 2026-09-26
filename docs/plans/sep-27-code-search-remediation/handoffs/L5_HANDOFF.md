# L5 — Parser coverage and source-fact producer

State: **partial**. Native grammar component: **VERIFIED** within the bounds
below. Rust producer, SDK publication, daemon query and integrated qualification:
**NOT_RUN**. G0 APIs are present; Rust execution and workspace dependency changes
remain serialized through L0.

Baseline HEAD: `106d7abec2dd3fa03f9db5a19a3de41df2f0afad`, concurrent dirty
checkout. [Current implementation snapshot](L5_HANDOFF.source.json) records
owned-file hashes and observed shared interfaces. It is an implementation
inventory, not a Rust test receipt. [PRE_G0](L5_PRE_G0.md) is historical only.

## Implementation

- `symbols.rs` and `symbols/preflight.rs`: one all-file census, strict native
  ERROR/MISSING rejection, deterministic bounded diagnostics, per-file/total
  deadlines, cancellation and source/symbol limits. Every admitted file retains
  a row, including empty files and failures. Parser/extractor failure cannot
  become Complete(0). Existing strict corpus extraction uses this census.
- `main.rs`: standalone `preflight` command and pre-publication census in `run`.
  Reports use create-new external output before profile admission. Strict is
  default; `allow-incomplete` accepts only Unsupported/ParseFailed. Producer
  invariants, resource limits, cancellation and timeouts remain fatal.
- `batch.rs`: every source file gets one shared SourceFileCoverage replacement,
  including zero-unit files. Full source SHA, language, producer policy and the
  canonical shared unit-set digest are bound. Chunk bytes and spans are checked
  against source. Lexical/semantic paired publication remains in place.
- Caller source stream/event defaults are the existing runner-name/run-id,
  independently of target generation. Explicit source-stream-id, source-event-id
  and source-base-event-id are available. The shared SDK computes the final
  logical payload digest before transport hashing.
- `l5_parser_regressions.rs`, three small fixtures, batch tests and migrated
  `sdk_roundtrip.rs` callers cover valid TS/TSX forms, source spans, malformed and
  empty files, all-file census, resource/cancellation failures and forged input.
  Two process tests require actual preflight output and actual SDK publication
  of malformed text with strict symbol refusal. These assertions are unexecuted.

In-process native aborts or OS termination yield no successful completed census;
they are not converted into per-file syntax failures. Parent process evidence is
required to distinguish a process crash from a timeout. No crash-recovery or
external producer upgrade is claimed here.

## Grammar component evidence

Root: `/private/tmp/quanta-index-l5-20260926T201839Z/`.

Independent syntax oracle:
`python3 /private/tmp/quanta-index-l5-20260926T201839Z/typescript_oracle.py`
exited 0. TypeScript 7.0.2 accepts both valid forms in TS/TSX and reports only the
intentional malformed file over all 256 frozen Vite sources. Syntax only;
semantic/project typechecking is excluded. Oracle receipt SHA-256:
`6566d8ce73272bba1623d431ce2f90024c00807623de16260e20d9028b34532b`.

Native candidate verification:
`python3 /private/tmp/quanta-index-l5-20260926T201839Z/grammar-probe/verify_candidate.py`
exited 0. It verifies each manifest/source hash before and after parsing, builds
each grammar once, and binds the native library hash across the full census.
Exact component commands, parser hashes, library hashes and terminal outputs are
in `grammar-probe/candidate-receipt.json`, SHA-256
`06ec6220e2a2c0af7a1609e5d07ff676b21a50521ee559d62c993f92a2413930`.

- Upstream 0.23.2: 252 TS executed, 249 successful/3 failed; four TSX successful.
- Candidate: 252 TS executed, 251 successful/one intentionally malformed;
  four TSX successful. No file was excluded.
- Candidate TS/TSX minimal fixtures accept both valid forms and still reject the
  malformed control. Symbol extraction is not established by this component rail.
- Candidate `tree-sitter test --overview-only`: exit 0, 112 terminal passed case
  lines against the upstream corpus. This is not a claim of 336 dialect cases.

Frozen Vite commit: `bc598a6a8a6b7d6e157e9f19c16911cff8d2360c`.
Manifest SHA-256:
`87ad16fe0c5626ee8ff625ddccf1645558c0b019d9229c5adcf24ffd0cbf7705`.
Expected remaining failure:
`packages/vite/src/node/ssr/__tests__/fixtures/errors/syntax-error.ts`.
Host: macOS 15.6/24G84 arm64; Apple clang 17.0.0 (clang-1700.6.4.2).
Generator: tree-sitter CLI 0.24.4, ABI 14. The Rust runtime remains 0.25.10 and
has not executed this candidate yet.

The first full-census harness attempts timed out: CLI 0.24.4's grammar.json
subdirectory fallback omits current-language caching, so `--rebuild` recompiles
C for each path. The corrected same census uses one explicit build followed by
all files with immutable library hashes. Timed-out attempts are not pass evidence.

## Dependency and integration handoff

The candidate is staged outside the repository in
`grammar-probe/vendor-stage`: upstream 0.23.2 commit
`f975a621f4e7f532fe322e13c4f79495e0a7b2e7`, MIT license, Rust bindings, grammar
source, generated TS/TSX C, upstream regression corpus and provenance.
The narrow patch moves the generic-call/type-query ambiguity from static
precedence to an explicit conflict and adds type-only star/namespace re-exports.
Upstream re-export discussion: [PR 360](https://github.com/tree-sitter/tree-sitter-typescript/pull/360).
The generic-call correction is a locally tested patch, not an upstream merge.

Pending L0 decisions/actions:

1. Lease `vendor/tree-sitter-typescript` to L5; root path dependency/lockfile and
   workspace membership remain L0-owned. Bind actual vendored grammar bytes into
   producer identity, then re-freeze the source closure.
2. Release the Rust slot for producer regression, library/bin tests and actual
   preflight over frozen Vite. Compiler failures are not behavioral RED.
3. Update `proof-required-tests.json` for renamed batch tests and new regressions;
   `test-authority.toml` already contains the new L5 target.
4. Build the final daemon and execute SDK tests and the actual Vite capability
   profile after ENG-02 is stable. Text publication is distinct from strict
   symbol completeness. Broad symbol coverage should still refuse the malformed
   file, while lexical source remains searchable.

No commit, push, agent creation or root dependency change was performed by L5.
No performance, clean-commit, release or external production qualification is
claimed.
