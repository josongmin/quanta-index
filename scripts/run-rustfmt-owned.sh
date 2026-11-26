#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

cargo="${QUANTA_INDEX_RUSTFMT_CARGO:-./scripts/cargow}"
packages="$(
  ./scripts/cargow --lane fmt-lane metadata --no-deps --format-version 1 |
    python3 -c '
import json
from pathlib import PurePath
import sys

metadata = json.load(sys.stdin)
root = PurePath(metadata["workspace_root"])
members = set(metadata["workspace_members"])
packages = [
    package["name"]
    for package in metadata["packages"]
    if package["id"] in members
    and PurePath(package["manifest_path"]).relative_to(root).parts[0] != "vendor"
]
if not packages:
    sys.exit("Rustfmt workspace contains no owned packages")
print("\n".join(sorted(packages)))
'
)"

selectors=()
while IFS= read -r package; do
  selectors+=(-p "$package")
done <<< "$packages"

"$cargo" --lane fmt-lane fmt "${selectors[@]}" "$@"
# These authored packages have their own workspaces.
"$cargo" --lane fmt-lane fmt --manifest-path shared/quanta-explicit-fd-process/Cargo.toml "$@"
"$cargo" --lane fmt-lane fmt --manifest-path crates/quanta-index-sqlite-fd-ledger/Cargo.toml "$@"
