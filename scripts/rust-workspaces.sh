#!/usr/bin/env bash

rust_locked_workspace_manifests() {
  printf '%s\n' \
    Cargo.toml \
    shared/quanta-explicit-fd-process/Cargo.toml \
    shared/quanta-state-root-quota/Cargo.toml \
    crates/quanta-index-contract/fuzz/Cargo.toml \
    crates/quanta-index-lq-norm/fuzz/Cargo.toml \
    tools/ci/ciborium-no-default-consumer/Cargo.toml \
    tools/ci/tree-sitter-registry-consumer/Cargo.toml \
    tools/lance-upstream-oracle/Cargo.toml \
    tools/sqlite-upstream-oracle/Cargo.toml \
    crates/quanta-index-sqlite-unix-syscall/Cargo.toml \
    tools/testing/catalog-checked-close/Cargo.toml \
    crates/quanta-index-sqlite-fd-ledger/Cargo.toml
}
