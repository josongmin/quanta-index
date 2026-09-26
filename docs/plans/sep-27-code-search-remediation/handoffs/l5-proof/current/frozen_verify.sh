#!/bin/bash
set -euo pipefail
export QUANTA_INDEX_PRESERVE_CARGO_TARGET_DIR=1
export CARGO_TARGET_DIR=/Users/songmin/Library/Caches/quanta-index/target/e385f4e6b4fe8e9b/test-daemon-lane
export QUANTA_INDEX_RESOURCE_WAIT_SECONDS=900
export CARGO_NET_OFFLINE=true
proof=/private/tmp/quanta-index-l5-completion
python3 "$proof/run_check.py" frozen-searchd-cached ./scripts/cargow --lane test-daemon-lane build -p quanta-index-searchd-runtime --bin quanta-index-searchd --locked
python3 "$proof/run_check.py" frozen-rust ./scripts/cargow --lane test-daemon-lane test -p quanta-index-retrieval-bench --lib --bins --test chunking_contract --test l5_parser_regressions --all-features --locked
python3 "$proof/run_check.py" frozen-sdk /Users/songmin/Documents/code-new/quanta-index/.venv/bin/python "$proof/sdk_final.py"
python3 "$proof/run_check.py" frozen-lifecycle ./scripts/cargow --lane test-daemon-lane test -p quanta-index-search-plane --lib --all-features --locked search_corpus
