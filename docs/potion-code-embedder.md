# Pinned local code embeddings

Quanta's `potion-code` profile uses Semble's default
`minishlab/potion-code-16M-v2` Model2Vec weights. The pinned upstream snapshot is
`e9d2a44ca6a05ac6685f3b23709ea57eb7352d5b` (256 dimensions, MIT).
The daemon loads the model locally at boot; it does not download at query time.

`potion-code` is the daemon default when `QUANTA_INDEX_EMBEDDER` is unset.
Its default model directory is
`$QUANTA_INDEX_CACHE_ROOT/models/potion-code-16M-v2-e9d2a44` (or the platform
Quanta cache root when the override is unset). Provision `config.json`,
`tokenizer.json`, and `model.safetensors` from that snapshot there, for example
with the Hugging Face CLI:

```sh
hf download minishlab/potion-code-16M-v2 \
  --revision e9d2a44ca6a05ac6685f3b23709ea57eb7352d5b \
  --local-dir "$(./scripts/quanta-index-env.sh)/models/potion-code-16M-v2-e9d2a44"
```

Then serve without an embedder selector (history retention is a separate,
required daemon policy):

```sh
QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_GENERATIONS=8 \
QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_BYTES=16777216 \
QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_REVISION_PAIRS=128 \
QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_TOTAL_BYTES=268435456 \
quanta-index-searchd serve --state-root /absolute/path/to/state
```

Set `QUANTA_INDEX_EMBED_MODEL_DIR` to an absolute directory only when using a
non-default model location. `hash-dev` remains explicit test/development opt-in.
Boot verifies SHA-256 of all three files and fails on missing/modified assets;
there is no hash fallback.
Query and corpus paths share the same model instance and L2 normalization.
Switching from `hash-dev` or `openai` requires re-ingesting and sealing a new
semantic generation; old vectors cannot be mixed with the new model identity.

`just rust-verify-quality-relevance` uses potion-code by default and gates the
paraphrase fixture as well as the existing lexical and exact-token cases. Use
`just rust-verify-quality-relevance-hash-dev` only for the hermetic mechanical
regression rail; its artifact is isolated under `relevance/hash-dev/latest`.
The in-process E2E test harness still uses an explicit deterministic hash fixture.
The `relevance_matrix --diagnostic` binary runs the same queries without writing
an attributed artifact; it is for dirty-tree investigation only, never a
quality-closure receipt. The canonical artifact validator refuses hash-dev or
an incomplete paraphrase set at `relevance/latest`.
Matching the model alone does not match Semble's chunker, BM25, fusion, reranker,
or published quality numbers.
