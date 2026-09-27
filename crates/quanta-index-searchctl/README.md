# Search CLI usage

Run commands from the repository root. Start `quanta-index-searchd` and publish
an active generation before querying it. Run as the daemon's permitted Unix user.

```sh
./scripts/cargow --lane dev-lane build -p quanta-index-searchctl --locked
./scripts/cargow --lane dev-lane run -p quanta-index-searchctl --locked -- --help
```

The current help path prints usage to stderr and exits 2. Examples below use
the built `quanta-index-searchctl` executable on `PATH`; `scripts/cargow run`
accepts the same arguments after `--`.

## Connect and choose output

Global flags: `--state-root PATH`, `--socket PATH`, and
`--output pretty|json|prometheus`. Use an explicit state root or socket override.
`prometheus` is accepted only for `metrics`.

```sh
quanta-index-searchctl --state-root /absolute/state-root --output json readiness
quanta-index-searchctl --state-root /absolute/state-root --output json generation-status \
  --repo-id my-repo --revision-id pinned-revision
quanta-index-searchctl --state-root /absolute/state-root --output json doctor \
  --repo-id my-repo --revision-id pinned-revision
```

`doctor` exits 0 when diagnosis completes; inspect `serve_ready`, `all_resolvable`
and per-track `resolver_ok` to determine health.

## Search

Use the published `repo-id`, `revision-id` and `manifest-generation`:

```sh
quanta-index-searchctl --state-root /absolute/state-root --output json lexical \
  --repo-id my-repo --revision-id pinned-revision --manifest-generation 1 \
  --syntax sourcegraph --query-text 'file:src main' --top-k 20
quanta-index-searchctl --state-root /absolute/state-root --output json semantic \
  --repo-id my-repo --revision-id pinned-revision --manifest-generation 1 \
  --query-text 'where is shutdown handled' --top-k 20
```

| Command | Query arguments, in addition to the pinned identity |
| --- | --- |
| `lexical`, `symbol`, `runtime-metadata`, `structural` | `--syntax native|sourcegraph --query-text TEXT --top-k N`; optional `--cursor-json PATH|-` |
| `history` | Same query flags plus `--order recency|relevance`; optional cursor |
| `semantic` | `--query-text TEXT --top-k N`; optional `--scope-query TEXT --scope-syntax native|sourcegraph --scope-top-k N` |
| `hybrid` | `--syntax native|sourcegraph --query-text TEXT --semantic-query-text TEXT --top-k N` |
| `hybrid-seed` | `--lexical-query TEXT --lexical-syntax native|sourcegraph --semantic-query TEXT --top-k N` |
| `repomap` | `--query-text TEXT --top-k N --token-budget N`; optional `--focus-subject subject_identity:subject_doc_type` |

`semantic --scope-query` confines dense results to the lexical scope.
`hybrid` searches independent lexical/dense lanes and fuses results.
Pass a returned cursor JSON file or stdin (`-`) to request the next supported page.

For a lexical/symbol candidate explanation, add `--candidate-json PATH|-`
and the original query to `explain`. For a hybrid explanation, use
`--hybrid-candidate-json PATH|-`, both original queries and the original `--top-k`.
Inspect score reconciliation fields in the output.

## Events and metrics

```sh
quanta-index-searchctl --state-root /absolute/state-root --output json events --plane query --limit 100
quanta-index-searchctl --state-root /absolute/state-root --output json metrics
quanta-index-searchctl --state-root /absolute/state-root metrics --output prometheus
```

Event planes: `query`, `control`, `ingest`; limit: 1–1024. Metrics are daemon
process totals. For a node_exporter textfile collector, write a temporary file
and rename it only after a successful scrape:

```sh
quanta-index-searchctl --state-root /absolute/state-root metrics --output prometheus \
  > /absolute/textfile/quanta_index.prom.tmp && \
  mv /absolute/textfile/quanta_index.prom.tmp /absolute/textfile/quanta_index.prom
```

## Quarantine

```sh
quanta-index-searchctl --state-root /absolute/state-root --output json quarantine list
```

To remove an entry, copy its exact identity from the inventory into one form:

```sh
quanta-index-searchctl --state-root /absolute/state-root quarantine discard \
  --track lexical --path /absolute/listed-generation --reason listed-code --detail listed-detail
quanta-index-searchctl --state-root /absolute/state-root quarantine discard \
  --repomap-file listed-name --reason listed-reason
```

`--track` is `lexical|semantic`. Discard deletes the identified entry and refuses
stale or invented identities. Inspect the inventory before invoking it.

## Exit codes

| Code | Meaning |
| --- | --- |
| 0 | Command completed; inspect returned health/verdict fields |
| 1 | Transport or output failure |
| 2 | Invalid usage, including the current help path |
| 3 | Remote typed refusal |
| 4 | Protocol/response mismatch |

For architecture and accepted contracts use [ADRs](../../docs/adr/README.md).
For offline state maintenance use the [state runbook](../../docs/operator/state-cutover-runbook.md).
