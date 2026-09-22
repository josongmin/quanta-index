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

SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from manifest import DEFAULT_MANIFEST_PATH, ManifestError, load_manifest

ROOT = Path(__file__).resolve().parents[2]


def load_profiles(path: Path = DEFAULT_MANIFEST_PATH) -> dict[str, dict[str, object]]:
    """Compatibility-sized profile view backed by the canonical manifest."""
    manifest = load_manifest(path)
    profiles = manifest["profiles"]
    assert isinstance(profiles, dict)
    return profiles


def parse_args(
    argv: list[str] | None,
    profiles: dict[str, dict[str, object]],
    *,
    repo_root: Path,
) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--repo-root", type=Path, default=repo_root, help="checkout to operate on")
    subparsers = parser.add_subparsers(dest="command", required=True)
    subparsers.add_parser("list", help="show registered benchmark profiles")
    for command, help_text in (
        ("run", "run producer recipes serially, then require their evidence"),
        ("validate", "require existing current-source evidence without running producers"),
        ("compare", "compare profile families that have a committed baseline"),
        ("summarize", "print a diagnostic inventory of declared profile evidence"),
    ):
        child = subparsers.add_parser(command, help=help_text)
        child.add_argument("profile", choices=sorted(profiles))
    preflight = subparsers.add_parser(
        "preflight", help="capture a host-contention receipt before a local timing run"
    )
    preflight.add_argument("profile", choices=sorted(profiles))
    preflight.add_argument(
        "--receipt", type=Path, required=True, help="where to atomically write the preflight receipt"
    )
    return parser.parse_args(argv)


def validate(repo_root: Path, artifact_profile: str) -> int:
    validator_path = repo_root / "tools" / "ci" / "lint" / "check-bench-artifacts.py"
    return subprocess.run(
        [
            sys.executable,
            str(validator_path),
            "--repo-root",
            str(repo_root),
            "--profile",
            artifact_profile,
            "--require",
            "--require-clean-worktree",
            "--skip-baselines",
        ],
        cwd=repo_root,
        check=False,
    ).returncode


def preflight(
    repo_root: Path,
    profile: str,
    receipt: Path,
    manifest: dict[str, object],
) -> int:
    """Capture a fail-closed local contention receipt for one profile run."""
    checker = repo_root / "tools" / "ci" / "timing" / "check_host_contention.py"
    profiles = manifest["profiles"]
    families = manifest["families"]
    assert isinstance(profiles, dict) and isinstance(families, dict)
    selected = profiles[profile]
    assert isinstance(selected, dict)
    family_names = selected["families"]
    assert isinstance(family_names, list)
    canonical_linux = any(
        isinstance(families[name], dict) and families[name]["host_policy"] == "canonical-linux"
        for name in family_names
    )
    command = [
        sys.executable,
        str(checker),
        "--receipt",
        str(receipt),
        "--run-id",
        f"benchctl:{profile}",
    ]
    if canonical_linux:
        command.extend(("--expected-os", "linux"))
    return subprocess.run(
        command,
        cwd=repo_root,
        check=False,
    ).returncode


def compare(repo_root: Path, profile: dict[str, object], manifest: dict[str, object]) -> int:
    """Run each explicitly declared comparator; profiles with no baseline are diagnostic-only."""
    raw_families = manifest["families"]
    assert isinstance(raw_families, dict)
    family_names = profile["families"]
    assert isinstance(family_names, list)
    comparator = repo_root / "tools" / "benchmark" / "compare_dsl_bench.py"
    for name in family_names:
        assert isinstance(name, str)
        family = raw_families[name]
        assert isinstance(family, dict)
        baseline = family["baseline"]
        if baseline is None:
            continue
        assert isinstance(baseline, dict)
        if baseline["comparator"] != "dsl-latency":
            raise RuntimeError(f"unregistered comparator for {name!r}")
        completed = subprocess.run(
            [
                sys.executable,
                str(comparator),
                str(repo_root / baseline["path"]),
                str(repo_root / family["artifact_glob"]),
            ],
            cwd=repo_root,
            check=False,
        )
        if completed.returncode:
            return completed.returncode
    return 0


def summarize(repo_root: Path, profile: dict[str, object], manifest: dict[str, object]) -> int:
    """Emit a read-only inventory without upgrading absent/stale evidence to a pass."""
    raw_families = manifest["families"]
    family_names = profile["families"]
    assert isinstance(raw_families, dict) and isinstance(family_names, list)
    families: list[dict[str, object]] = []
    for name in family_names:
        assert isinstance(name, str)
        family = raw_families[name]
        assert isinstance(family, dict)
        pattern = family["artifact_glob"]
        assert isinstance(pattern, str)
        paths = sorted(repo_root.glob(pattern))
        artifacts: list[dict[str, object]] = []
        for path in paths:
            try:
                payload = json.loads(path.read_text(encoding="utf-8"))
            except (OSError, json.JSONDecodeError) as exc:
                artifacts.append({"path": str(path.relative_to(repo_root)), "error": str(exc)})
                continue
            if not isinstance(payload, dict):
                artifacts.append({"path": str(path.relative_to(repo_root)), "error": "not an object"})
                continue
            provenance = payload.get("provenance")
            detail = payload.get("detail")
            artifacts.append(
                {
                    "path": str(path.relative_to(repo_root)),
                    "schema_version": payload.get("schema_version"),
                    "git_head": provenance.get("git_head") if isinstance(provenance, dict) else None,
                    "host_os": payload.get("host", {}).get("os")
                    if isinstance(payload.get("host"), dict)
                    else None,
                    "passed": detail.get("passed") if isinstance(detail, dict) else None,
                }
            )
        families.append(
            {
                "family": name,
                "host_policy": family["host_policy"],
                "artifacts": artifacts,
                "status": "absent" if not artifacts else "present_unvalidated",
            }
        )
    print(json.dumps({"profile": profile, "families": families}, sort_keys=True, indent=2))
    return 0


def main(argv: list[str] | None = None) -> int:
    bootstrap = argparse.ArgumentParser(add_help=False)
    bootstrap.add_argument("--repo-root", type=Path, default=ROOT)
    bootstrap_args, _ = bootstrap.parse_known_args(argv)
    repo_root = bootstrap_args.repo_root.resolve()
    try:
        profiles = load_profiles(repo_root / "tools" / "benchmark" / "manifest.json")
    except ManifestError as exc:
        print(f"ERROR: invalid benchmark profile manifest: {exc}", file=sys.stderr)
        return 2
    args = parse_args(argv, profiles, repo_root=repo_root)
    repo_root = args.repo_root.resolve()
    if args.command == "list":
        for name in sorted(profiles):
            profile = profiles[name]
            print(f"{name}\t{name}\t{profile.get('description', '')}")
        return 0

    profile = profiles[args.profile]
    artifact_profile = args.profile
    manifest = load_manifest(repo_root / "tools" / "benchmark" / "manifest.json")
    if args.command == "preflight":
        return preflight(repo_root, args.profile, args.receipt, manifest)
    if args.command == "summarize":
        return summarize(repo_root, profile, manifest)
    if args.command == "run":
        receipt = repo_root / "artifacts" / "benchmark-receipts" / args.profile / "preflight.json"
        preflight_result = preflight(repo_root, args.profile, receipt, manifest)
        if preflight_result:
            print(
                f"ERROR: local timing preflight blocked; receipt written to {receipt}",
                file=sys.stderr,
            )
            return preflight_result
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
    validation = validate(repo_root, artifact_profile)
    if validation:
        return validation
    if args.command in {"run", "compare"}:
        try:
            return compare(repo_root, profile, manifest)
        except RuntimeError as exc:
            print(f"ERROR: {exc}", file=sys.stderr)
            return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
