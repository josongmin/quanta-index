# Quanta TypeScript grammar compatibility patch

Upstream package: tree-sitter-typescript 0.23.2, commit
`f975a621f4e7f532fe322e13c4f79495e0a7b2e7`. The package version remains 0.23.2;
workspace path pinning and the complete vendored-source digest identify this
patch. Upstream MIT [LICENSE](LICENSE) is unchanged.

`quanta-compatibility.patch` contains the complete handwritten grammar delta:

1. Preserve the generic-call and type-query-call alternatives as a GLR conflict.
   Static precedence prematurely chooses an expression interpretation for
   `runnerImport<typeof import('./basic')>(fixture('cjs.js'),)`.
2. Recognize `export type *` and `export type * as Name` re-exports. The latter
   matches [upstream PR 360](https://github.com/tree-sitter/tree-sitter-typescript/pull/360);
   this vendored patch does not claim that PR was merged upstream.

The TS and TSX generated parser, grammar JSON and node types are kept with their
source grammar, shared scanner, C headers, Rust bindings and queries. Build-time
source hashing in the retrieval producer binds actual vendored bytes into its
reported policy. `quanta-provenance.json` records original inputs and component
receipt identity; it is documentation, not an authority to skip verification.
The Rust binding additionally exports `QUANTA_COMPATIBILITY_PATCH_ID`; the
producer references it so an accidentally retained registry dependency cannot
compile while claiming the vendored grammar identity. Existing binding APIs
remain unchanged.

## Regeneration

Use tree-sitter CLI **0.24.4**, commit
`fc8c1863e2e5724a0c40bb6e6cfc8631bfe5908b`, ABI **14**. The exact official CLI
archive, upstream source archive and npm dependency archive URLs/hashes are in
`quanta-provenance.json`. Grammar generation needs the unmodified npm package
`tree-sitter-javascript` **0.23.1** extracted into
`node_modules/tree-sitter-javascript`. Do not resolve its floating package range.

After verifying those input hashes, run from this directory (CLI on PATH):

```sh
(cd typescript && tree-sitter generate --abi 14)
(cd tsx && tree-sitter generate --abi 14)
```

Rust compilation uses the checked-in generated C and does not invoke npm or the
generator. Regeneration changes must repeat both supported-language producer
tests and grammar component validation. The upstream corpus, Vite checkout and
benchmark data are not vendored here.

## Component validation boundary

The native candidate passed both valid minimal forms in TS/TSX, kept malformed
syntax erroneous, and passed 112 terminal upstream corpus cases. On the frozen
Vite universe all 256 files were examined: only the intentional syntax-error
fixture remained erroneous, compared with three failures in upstream 0.23.2.
See `docs/plans/sep-27-code-search-remediation/handoffs/L5_HANDOFF.md` for exact
source, commands and receipts. These component results do not establish Rust
producer extraction, SDK publication, daemon activation or performance.
