#!/usr/bin/env python3
"""Snapshot the public API surface of guarded integration crates.

`quanta-index-contract` and `quanta-index-sdk` are typed integration surfaces.
Changes to their public items must appear as a reviewable diff in
`tools/ci/lint/baselines/public-api/<crate>.txt`.

Pairs with the "breaking-first" doctrine — the goal is not to prevent
breakage but to make it impossible to ship breakage without an explicit
baseline update commit.

Usage:
    check-public-api.py                   # CI: fail if surface differs
    check-public-api.py --update-baseline # accept the new surface

Requires:
    cargo install cargo-public-api --locked
"""

from __future__ import annotations

import argparse
import difflib
import hashlib
import os
import platform
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
BASELINE_DIR = ROOT / "tools" / "ci" / "lint" / "baselines" / "public-api"

GUARDED_CRATES: list[str] = ["quanta-index-contract", "quanta-index-sdk"]
PUBLIC_API_TOOLCHAIN = "nightly-2026-08-01"


def default_cache_root() -> Path:
    override = os.environ.get("QUANTA_INDEX_CACHE_ROOT")
    if override:
        return Path(override).expanduser()

    if platform.system() == "Darwin":
        return Path.home() / "Library" / "Caches" / "quanta-index"

    base = os.environ.get("XDG_CACHE_HOME")
    if base:
        return Path(base).expanduser() / "quanta-index"
    return Path.home() / ".cache" / "quanta-index"


def cargo_env(default_lane: str) -> dict[str, str]:
    env = os.environ.copy()
    # Rustdoc JSON rendering changes across nightlies even when the Rust API
    # does not. Keep the baseline tied to the toolchain that produced it.
    env["RUSTUP_TOOLCHAIN"] = PUBLIC_API_TOOLCHAIN
    cache_root = default_cache_root()
    env.setdefault("QUANTA_INDEX_CACHE_ROOT", str(cache_root))
    env.setdefault("QUANTA_INDEX_REPO_ROOT", str(ROOT))
    env.setdefault("QUANTA_INDEX_BUILD_LANE", default_lane)
    env.setdefault(
        "CARGO_TARGET_DIR",
        str(
            cache_root
            / "target"
            / hashlib.sha256(str(ROOT.resolve()).encode()).hexdigest()[:16]
            / env["QUANTA_INDEX_BUILD_LANE"]
        ),
    )
    return env


def render_public_api(package: str) -> str:
    cmd = [
        "cargo",
        "public-api",
        "--package",
        package,
        "--simplified",
    ]
    result = subprocess.run(
        cmd,
        cwd=ROOT,
        env=cargo_env("public-api-lane"),
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        raise RuntimeError(f"cargo public-api failed for {package}: {result.stderr}")
    # Normalize trailing whitespace + force trailing newline so diffs stay
    # stable across editors.
    body = "\n".join(line.rstrip() for line in result.stdout.splitlines())
    expected_root = f"pub mod {package.replace(chr(45), chr(95))}"
    if next((line for line in body.splitlines() if line.strip()), None) != expected_root:
        raise RuntimeError(f"cargo public-api returned missing or wrong crate root for {package}")
    return body + "\n"


def baseline_path(package: str) -> Path:
    return BASELINE_DIR / f"{package}.txt"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--update-baseline", action="store_true")
    parser.add_argument("--packages", nargs="+", default=GUARDED_CRATES)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    # Render every selected API before writing any baseline. A later producer
    # failure must not leave the earlier package's snapshot promoted.
    rendered = {pkg: render_public_api(pkg) for pkg in args.packages}
    if args.update_baseline:
        BASELINE_DIR.mkdir(parents=True, exist_ok=True)
        for pkg, current in rendered.items():
            path = baseline_path(pkg)
            path.write_text(current, encoding="utf-8")
            print(f"baseline updated: {path}")
        return 0

    bad = False
    for pkg in args.packages:
        current = rendered[pkg]
        path = baseline_path(pkg)

        if not path.exists():
            print(
                f"{pkg}: no baseline at {path}. Run with --update-baseline once to seed it.",
                file=sys.stderr,
            )
            bad = True
            continue

        previous = path.read_text(encoding="utf-8")
        if current == previous:
            print(f"{pkg}: public API unchanged.")
            continue

        bad = True
        diff = difflib.unified_diff(
            previous.splitlines(keepends=True),
            current.splitlines(keepends=True),
            fromfile=str(path),
            tofile=f"{pkg} (current)",
        )
        sys.stderr.write(f"\n{pkg}: public API DRIFTED\n")
        sys.stderr.writelines(diff)

    if bad:
        sys.stderr.write(
            "\nThe public API of a guarded integration crate has changed. If this "
            "is intentional, re-run with --update-baseline and commit the "
            "new baseline alongside the breaking change.\n"
        )
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
