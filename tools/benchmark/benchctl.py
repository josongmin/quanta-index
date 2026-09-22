#!/usr/bin/env python3
"""Run and validate named benchmark evidence profiles through one CLI.

This is intentionally a thin dispatcher. Producers remain the canonical
Justfile recipes; the CLI owns only the profile-to-recipe mapping and invokes
the artifact validator after production. It never accepts arbitrary shell
commands, because that would make a benchmark label independent from its
actual authority path.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
MANIFEST_PATH = ROOT / "tools" / "benchmark" / "profiles.json"
VALIDATOR_PATH = ROOT / "tools" / "ci" / "lint" / "check-bench-artifacts.py"


def load_profiles(path: Path = MANIFEST_PATH) -> dict[str, dict[str, object]]:
    with path.open("rb") as handle:
        payload = json.load(handle)
    profiles = payload.get("profiles")
    if not isinstance(profiles, dict) or not profiles:
        raise ValueError("profiles.json has no profiles object")
    checked: dict[str, dict[str, object]] = {}
    for name, value in profiles.items():
        if not isinstance(name, str) or not isinstance(value, dict):
            raise ValueError("every profile must be a named table")
        profile = value.get("artifact_profile")
        recipes = value.get("recipes")
        families = value.get("families")
        if not isinstance(profile, str) or not profile:
            raise ValueError(f"profile {name!r} has no artifact_profile")
        if not isinstance(recipes, list) or not all(
            isinstance(recipe, str) and recipe for recipe in recipes
        ):
            raise ValueError(f"profile {name!r} has invalid recipes")
        if (
            not isinstance(families, list)
            or not families
            or not all(isinstance(family, str) and family for family in families)
            or len(set(families)) != len(families)
        ):
            raise ValueError(f"profile {name!r} has invalid families")
        checked[name] = value
    return checked


def parse_args(profiles: dict[str, dict[str, object]]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--repo-root", type=Path, default=ROOT, help="checkout to operate on")
    subparsers = parser.add_subparsers(dest="command", required=True)
    subparsers.add_parser("list", help="show registered benchmark profiles")
    for command, help_text in (
        ("run", "run producer recipes serially, then require their evidence"),
        ("validate", "require existing current-source evidence without running producers"),
    ):
        child = subparsers.add_parser(command, help=help_text)
        child.add_argument("profile", choices=sorted(profiles))
    return parser.parse_args()


def validate(repo_root: Path, artifact_profile: str) -> int:
    return subprocess.run(
        [
            sys.executable,
            str(VALIDATOR_PATH),
            "--repo-root",
            str(repo_root),
            "--profile",
            artifact_profile,
            "--require",
            "--skip-baselines",
        ],
        cwd=repo_root,
        check=False,
    ).returncode


def main() -> int:
    try:
        profiles = load_profiles()
    except (OSError, json.JSONDecodeError, ValueError) as exc:
        print(f"ERROR: invalid benchmark profile manifest: {exc}", file=sys.stderr)
        return 2
    args = parse_args(profiles)
    repo_root = args.repo_root.resolve()
    if args.command == "list":
        for name in sorted(profiles):
            profile = profiles[name]
            print(f"{name}\t{profile['artifact_profile']}\t{profile.get('description', '')}")
        return 0

    profile = profiles[args.profile]
    artifact_profile = profile["artifact_profile"]
    assert isinstance(artifact_profile, str)
    if args.command == "run":
        recipes = profile["recipes"]
        assert isinstance(recipes, list)
        if not recipes:
            print(f"ERROR: profile {args.profile!r} has no registered producer", file=sys.stderr)
            return 2
        for recipe in recipes:
            assert isinstance(recipe, str)
            completed = subprocess.run(["just", recipe], cwd=repo_root, check=False)
            if completed.returncode:
                print(f"ERROR: producer recipe {recipe!r} failed", file=sys.stderr)
                return completed.returncode
    return validate(repo_root, artifact_profile)


if __name__ == "__main__":
    raise SystemExit(main())
