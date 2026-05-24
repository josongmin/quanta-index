#!/usr/bin/env python3
"""Snapshot the module tree of each workspace crate.

Complements `check-public-api.py`:

  * `cargo public-api` answers "what items are reachable from lib.rs"
  * `cargo modules structure` answers "what is the internal mod tree"

The pair locks both the *external* shape and the *internal* organization so
that silent module renames, deletions, or relocations always appear as a
reviewable diff in the same PR that performs them.

Usage:
    check-cargo-modules-snapshot.py                   # CI: fail on drift
    check-cargo-modules-snapshot.py --update-baseline # accept the new tree

Requires:
    cargo install cargo-modules --locked
"""

from __future__ import annotations

import argparse
import difflib
import subprocess
import sys
from pathlib import Path

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - Python < 3.11
    import tomli as tomllib  # type: ignore[no-redef]


ROOT = Path(__file__).resolve().parents[3]
WORKSPACE_TOML = ROOT / "Cargo.toml"
BASELINE_DIR = ROOT / "tools" / "ci" / "lint" / "baselines" / "cargo-modules"

# Only the contract + core surfaces are structurally load-bearing; adapters can
# evolve their module tree freely. Extend this list deliberately.
GUARDED_CRATES: list[str] = [
    "quanta-index-contract",
    "quanta-index-core",
]


def workspace_member_names() -> set[str]:
    data = tomllib.loads(WORKSPACE_TOML.read_text(encoding="utf-8"))
    members = data.get("workspace", {}).get("members", [])
    return {Path(m).name for m in members}


def render_module_tree(package: str) -> str:
    cmd = ["cargo", "modules", "structure", "--package", package, "--no-fns"]
    result = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True)
    if result.returncode != 0:
        raise RuntimeError(
            f"cargo modules structure failed for {package}: {result.stderr}"
        )
    body = "\n".join(line.rstrip() for line in result.stdout.splitlines())
    return body + "\n"


def baseline_path(package: str) -> Path:
    return BASELINE_DIR / f"{package}.txt"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--update-baseline", action="store_true")
    parser.add_argument("--packages", nargs="*", default=GUARDED_CRATES)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    BASELINE_DIR.mkdir(parents=True, exist_ok=True)

    # Filter packages against current workspace membership so mid-refactor
    # crate removals don't crash this gate.
    present = workspace_member_names()
    packages = [p for p in args.packages if p in present]
    skipped = [p for p in args.packages if p not in present]
    for p in skipped:
        print(
            f"{p}: not in workspace.members (skipped — adjust GUARDED_CRATES if intentional)",
            file=sys.stderr,
        )

    bad = False
    for pkg in packages:
        current = render_module_tree(pkg)
        path = baseline_path(pkg)

        if args.update_baseline:
            path.write_text(current, encoding="utf-8")
            print(f"baseline updated: {path}")
            continue

        if not path.exists():
            print(
                f"{pkg}: no baseline at {path}. Seed with --update-baseline.",
                file=sys.stderr,
            )
            bad = True
            continue

        previous = path.read_text(encoding="utf-8")
        if current == previous:
            print(f"{pkg}: module tree unchanged.")
            continue

        bad = True
        diff = difflib.unified_diff(
            previous.splitlines(keepends=True),
            current.splitlines(keepends=True),
            fromfile=str(path),
            tofile=f"{pkg} (current)",
        )
        sys.stderr.write(f"\n{pkg}: module tree DRIFTED\n")
        sys.stderr.writelines(diff)

    if bad:
        sys.stderr.write(
            "\nA guarded crate's module tree changed. If intentional, re-run "
            "with --update-baseline and commit the new baseline.\n"
        )
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
