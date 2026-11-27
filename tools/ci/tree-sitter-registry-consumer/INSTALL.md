# Registry grammar candidate installation

Build the registry-only producer with `./scripts/cargow --lane dev-lane build --manifest-path tools/ci/tree-sitter-registry-consumer/Cargo.toml --locked --offline --bins`. Build `tools/benchmark/retrieval/go_projection/main.go` with Go 1.25.4 and `CGO_ENABLED=0`. The bundle also needs a Node executable and TypeScript 5.9.3 `typescript.js`. The installer rejects macOS Node binaries with absolute non-system dylib dependencies. It checks the dependency closure, compares the Go, JS, TS, and TSX single-file symbol names and byte spans with fixed inputs, then runs the syntax batch binary to verify matching results and a typed parse failure.

Release tooling must prepare a `quanta-symbol-candidate-bundle-v1` descriptor with exact artifact hashes and publish its SHA-256 through an independent trusted channel. Measuring the descriptor hash from the same untrusted bundle is only a local diagnostic. Its `official_binary`, `syntax_batch_binary`, `go_helper`, `node`, and `typescript_js` entries each have a bundle-relative `path` and lowercase `sha256`; `node_libraries` is a list of the same entries. Other fields are `platform` (`macos` or `linux`), `architecture` (`aarch64` or `x86_64`), `grammar_identity` (starting `registry-`), `go_version` (`go1.25.4`), `node_version`, `typescript_version` (`5.9.3`), and the SHA-256 of `tools/benchmark/retrieval/ts_projection.cjs` as `script_sha256`.

```sh
.venv/bin/python tools/ci/install_tree_sitter_candidate.py \
  --bundle /absolute/bundle --descriptor /absolute/descriptor.json \
  --descriptor-sha256 EXTERNALLY_PINNED_SHA256 \
  --destination /absolute/install-directory
```

The command prints the installed `manifest.json` path and SHA-256. Supply both to `quanta-index-retrieval-bench preflight/run` with `--symbol-candidate-manifest PATH --symbol-candidate-manifest-sha256 SHA256`, or in the capture spec as `symbol_candidate_manifest` and `symbol_candidate_manifest_sha256`. Corpora containing Go, JavaScript, TypeScript or TSX require the pair. The destination must be absent. Probe failure removes it. The installer records physical paths so the same manifest also passes the Python oracle's file checks. Qualified pair capture still requires the independent census and the existing admission gates.

## Local preflight verification (2026-11-24)

The final installer output at `/tmp/quanta-parser-candidate-node/installed-racefix-2/manifest.json` had SHA-256 `1e9f0283d63f463805704a4363aba017285594e1a336cf647b60233b967ad1e1`. An existing benchmark binary from the `test-workspace-lane` target consumed that exact manifest:

```sh
quanta-index-retrieval-bench preflight \
  --repo /tmp/quanta-parser-candidate-node/corpus \
  --manifest /tmp/quanta-parser-candidate-node/corpus-manifest.json \
  --out /tmp/quanta-parser-candidate-node/preflight-racefix.json \
  --symbol-timeout-ms 60000 --symbol-total-timeout-ms 300000 \
  --symbol-candidate-manifest /tmp/quanta-parser-candidate-node/installed-racefix-2/manifest.json \
  --symbol-candidate-manifest-sha256 1e9f0283d63f463805704a4363aba017285594e1a336cf647b60233b967ad1e1
```

**VERIFIED:** exit 0, `symbol-preflight-v1`, 4 admitted files, 0 incomplete files, and one symbol each for Go, JS, TS, and TSX. The report's grammar identity contains the installed manifest SHA-256 and its producer policy SHA-256 is `513cfbff3e4cb498a7f3237691ae27b65baec9758124aec6b80254aa11457dfe`. The corpus is a four-file clean temporary Git repository at commit `498ef81071f1b74fd6cb5a3443692c1591b09263`; it is an integration smoke test. The descriptor hash was locally generated, so release provenance, an external admitted corpus, default parser transition, and qualified benchmark comparison remain unverified.

The attempt to rebuild the root benchmark binary from the concurrent checkout with `./scripts/cargow --lane test-workspace-lane build -p quanta-index-retrieval-bench --bin quanta-index-retrieval-bench --locked --offline` **FAILED** (exit 101, no new binary or test body): `quanta-index-contract/src/ipc/ingest/runtime_wire.rs:9` referenced a missing `catalog_admitted` module during concurrent contract work. The successful preflight is proof for the existing binary and installed manifest only; its file modification time does not establish current-source identity. A current-checkout rebuild and preflight must be repeated after the contract source stabilizes.

## One-process syntax batch verification (2026-11-24)

The rebuilt registry-only executables passed the installer with four single-file probes and four syntax batch rows, including the malformed input. The batch used the 10-second per-file and 60-second total limits. The local manifest SHA-256 is `e1d15c4e621e878740561f63db984239bd8c96f6d5b62f735c095ca95c288542`. The local comparison matched 92 records over 16 valid files and one parse rejection. The existing benchmark binary consumed the new manifest at its default deadlines and admitted all four input files. The release channel still needs to supply an independent descriptor pin and the admitted corpus.
