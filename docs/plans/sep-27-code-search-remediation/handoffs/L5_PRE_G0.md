# L5_PRE_G0 — Parser and producer preparation

Historical preparation snapshot. Superseded by [L5_HANDOFF](L5_HANDOFF.md).
The statements below describe the pre-G0 snapshot only; its source receipt is
not current implementation evidence.

State: **partial**. Rust producer behavior: **NOT_RUN**. Publication contract:
**BLOCKED on G0_READY**. This is a handoff, not a second execution tracker or
evidence of a parser remedy.

Baseline: `106d7abec2dd3fa03f9db5a19a3de41df2f0afad`, concurrent dirty checkout.
L0 is task `01a0dea8-aa8e-7d73-857f-b174f7be64fe`. L0 retains shared DTOs,
workspace manifests, lockfile and the Rust execution slot. No production source,
dependency, commit or agent creation belongs to this L5 change.

## Changed files

- `benchmarks/retrieval/tests/l5_parser_regressions.rs`: six owner regressions.
- `benchmarks/retrieval/tests/fixtures/l5_parser/*.ts`: two minimal valid forms
  and one intentionally malformed control.

The valid fixtures require exactly the handwritten `sentinel` function and its
source byte/line span. A call or namespace re-export must not invent a definition.
Both TS and TSX are exercised. Controls retain ordinary definitions, distinguish
empty/zero-definition files from parse failure, refuse a final-file failure, and
retain empty-file membership in the existing corpus extraction result.

These tests do not yet prove an all-file preflight: existing extraction remains
fail-fast. Prepared assertions are not a demonstrated behavioral RED.

## Executed evidence

Exact source/artifact hashes, commands and dirty state:
[`L5_PRE_G0.source.json`](L5_PRE_G0.source.json).

- `rustfmt --edition 2024 --check benchmarks/retrieval/tests/l5_parser_regressions.rs`:
  exit 0; syntax/formatting only, no Rust typecheck or behavior execution.
- `git diff --check -- benchmarks/retrieval`: exit 0; tracked whitespace only.
- `python3 /private/tmp/quanta-index-l5-20260926T201839Z/typescript_oracle.py`:
  exit 0. The script invokes TypeScript 7.0.2 with explicit files, `noCheck`,
  `noEmit`, `noResolve`, empty ambient type inclusion and JSX preservation.
  Four valid TS/TSX fixture invocations exit 0. Two malformed fixture invocations
  exit 1 with syntax diagnostics. The whole Vite invocation exits 1 with only
  its intentional `syntax-error.ts` diagnostic. These are the expected outcomes.

Frozen Vite inventory: 256/256 unique paths and source hashes verified, commit
`bc598a6a8a6b7d6e157e9f19c16911cff8d2360c`. Manifest SHA-256:
`87ad16fe0c5626ee8ff625ddccf1645558c0b019d9229c5adcf24ffd0cbf7705`.
All source hashes are checked again around the compiler invocation.

Raw outputs, generated configs, compiler binary identity and per-file inputs are
under `/private/tmp/quanta-index-l5-20260926T201839Z/`. Oracle receipt SHA-256:
`6566d8ce73272bba1623d431ce2f90024c00807623de16260e20d9028b34532b`.
This proves independent syntax acceptance only; it does not prove semantic
typechecking, tree-sitter compatibility, symbol extraction, SDK or publication.

The oracle harness first failed on an incorrect executable path and then an
incorrect assumption that this compiler's syntax-error exit code is 2. The final
script resolves the installed `lib/tsc` and requires the observed native 7.0.2
exit code 1 plus exact diagnostic file identities. Aborted harness runs do not
provide the successful receipt above.

## G0 inputs requested from L0

1. Canonical replacement covering the full admitted file universe, including no
   chunks/no symbols; explicit Complete(0), NotRequested, Unsupported, ParseFailed
   and ProducerFailed. Current `batch.rs` omits empty unit pairs.
2. Source repository/path/revision/full-byte digest/language binding, exact
   producer/runtime/grammar identity, and a shared unit-commitment helper.
3. Source stream/event and expected source base at the existing SDK publication
   boundary. Specify initial import and update semantics; a target generation
   alone cannot synthesize an event identity.
4. Explicit profile admission for text with incomplete symbols versus strict
   symbol coverage; do not guess this policy from route names.
5. Exact shared DTO/SDK names and mismatch rejections. Confirm ownership of
   `symbols.rs`, `main.rs` and `batch.rs` before production edits.

## Remaining implementation and verification

The pinned dependencies remain tree-sitter 0.25.10 and TypeScript grammar 0.23.2.
The installed grammar lacks type-only namespace re-export alternatives. Upstream
[PR 360](https://github.com/tree-sitter/tree-sitter-typescript/pull/360) proposes
those alternatives but was still Draft when inspected; it is a candidate, not
an adopted dependency or proof. No tested grammar candidate for the generic
`typeof import` trailing-comma case has been established here.

After L0 releases the Rust slot, the first behavior command is:

```sh
./scripts/cargow test -p quanta-index-retrieval-bench --test l5_parser_regressions -- --test-threads=1
```

L0 must account for the new target and fixture paths in test authority/source
closure. All-file preflight, bounded diagnostic states, grammar remedy, coverage
publication, empty-file batch membership and shared lifecycle tests remain open.
Do not lower ERROR/MISSING rejection or remove the malformed Vite source to make
a strict symbol profile appear complete. External production producers have not
been changed or exercised.
