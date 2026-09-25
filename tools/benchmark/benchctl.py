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
import hashlib
import json
import subprocess
import sys
import time
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from compare_dsl_bench import (  # noqa: E402
    FULL_HEAD_RE,
    MIN_SAMPLES_FOR_AUTHORITY,
    ArtifactRefused,
    atomically_write_baseline,
    load_artifact,
    require_clean_host_load,
    require_clean_preflight,
    require_complete_baseline_candidate,
)
from manifest import DEFAULT_MANIFEST_PATH, ManifestError, load_manifest  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]


def require_clean_worktree(repo_root: Path) -> None:
    """Refuse a capture before it can attribute dirty-source timings to HEAD."""
    completed = subprocess.run(
        ["git", "-C", str(repo_root), "status", "--porcelain", "--untracked-files=normal"],
        check=False,
        capture_output=True,
        text=True,
    )
    if completed.returncode != 0:
        raise RuntimeError(f"git status failed: {completed.stderr.strip()}")
    if completed.stdout:
        raise RuntimeError(
            "worktree is dirty: benchmark producers require a clean checkout before capture"
        )


def resolve_checkout_head(repo_root: Path) -> str:
    completed = subprocess.run(
        ["git", "-C", str(repo_root), "rev-parse", "HEAD"],
        check=False,
        capture_output=True,
        text=True,
    )
    if completed.returncode != 0:
        raise RuntimeError(f"git rev-parse HEAD failed: {completed.stderr.strip()}")
    head = completed.stdout.strip()
    if not FULL_HEAD_RE.fullmatch(head):
        raise RuntimeError(f"invalid checkout HEAD {head!r}")
    return head


def require_frozen_source(repo_root: Path, initial_head: str) -> None:
    require_clean_worktree(repo_root)
    current_head = resolve_checkout_head(repo_root)
    if current_head != initial_head:
        raise RuntimeError(
            f"checkout HEAD changed during benchmark run: {initial_head} -> {current_head}"
        )


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
        if command == "run":
            child.add_argument(
                "--cold-samples",
                type=int,
                help="DSL authority cold samples; accepted only for dsl-authority (minimum 20)",
            )
            child.add_argument(
                "--admit-baseline",
                action="store_true",
                help="capture and admit both DSL baselines in this same guarded run",
            )
    preflight = subparsers.add_parser(
        "preflight", help="capture a host-contention receipt before a local timing run"
    )
    preflight.add_argument("profile", choices=sorted(profiles))
    preflight.add_argument(
        "--receipt",
        type=Path,
        required=True,
        help="where to atomically write the preflight receipt",
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


def require_declared_baselines(
    repo_root: Path, profile: dict[str, object], manifest: dict[str, object]
) -> None:
    """Refuse an expensive comparison run that cannot finish without its baselines."""
    families = manifest["families"]
    names = profile["families"]
    assert isinstance(families, dict) and isinstance(names, list)
    for name in names:
        family = families[name]
        assert isinstance(family, dict)
        baseline = family["baseline"]
        if baseline is None:
            continue
        assert isinstance(baseline, dict)
        path = repo_root / baseline["path"]
        if not path.is_file():
            raise RuntimeError(
                f"missing declared baseline for {name}: {path}; capture and admit a baseline before running this comparison profile"
            )
        try:
            artifact = load_artifact(path, role="baseline")
        except ArtifactRefused as exc:
            raise RuntimeError(f"declared baseline for {name} is invalid: {exc}") from exc
        expected_mode = name.removeprefix("dsl-")
        if artifact.mode != expected_mode:
            raise RuntimeError(
                f"declared baseline for {name} has mode {artifact.mode!r}, expected {expected_mode!r}"
            )
        floor = MIN_SAMPLES_FOR_AUTHORITY[artifact.mode]
        if any(
            row.early_stop_reason is not None or row.samples < floor
            for row in artifact.rows.values()
        ):
            raise RuntimeError(
                f"declared baseline for {name} has unmeasured rows or fewer than {floor} samples"
            )


def admit_dsl_baselines(
    repo_root: Path,
    profile: dict[str, object],
    manifest: dict[str, object],
    receipt: Path,
    initial_head: str,
    capture_started_ns: int,
    preflight_digest: str,
) -> None:
    """Admit only artifacts freshly produced by this guarded DSL run."""
    families = manifest["families"]
    names = profile["families"]
    assert isinstance(families, dict) and isinstance(names, list)
    if names != ["dsl-warm", "dsl-cold"]:
        raise RuntimeError("DSL baseline admission requires the exact warm/cold family pair")
    try:
        current_receipt_digest = hashlib.sha256(receipt.read_bytes()).hexdigest()
    except OSError as exc:
        raise RuntimeError(f"DSL preflight receipt disappeared: {exc}") from exc
    if current_receipt_digest != preflight_digest:
        raise RuntimeError("DSL preflight receipt changed during capture")
    prepared: list[tuple[Path, str]] = []
    for name in names:
        family = families[name]
        assert isinstance(family, dict)
        relative_artifact = family["artifact_glob"]
        baseline = family["baseline"]
        if (
            not isinstance(relative_artifact, str)
            or any(char in relative_artifact for char in "*?[]")
            or not isinstance(baseline, dict)
            or baseline.get("comparator") != "dsl-latency"
        ):
            raise RuntimeError(f"DSL baseline family {name!r} has no exact artifact/baseline pair")
        artifact_path = repo_root / relative_artifact
        try:
            stat = artifact_path.stat()
        except OSError as exc:
            raise RuntimeError(f"fresh DSL artifact missing: {artifact_path}: {exc}") from exc
        if min(stat.st_mtime_ns, stat.st_ctime_ns) < capture_started_ns:
            raise RuntimeError(f"DSL artifact was not written by this run: {artifact_path}")
        try:
            artifact = load_artifact(artifact_path, role="baseline candidate")
            require_complete_baseline_candidate(artifact)
            require_clean_preflight(receipt, artifact)
        except ArtifactRefused as exc:
            raise RuntimeError(f"DSL baseline candidate {name!r} refused: {exc}") from exc
        if artifact.git_head != initial_head or artifact.mode != name.removeprefix("dsl-"):
            raise RuntimeError(f"DSL baseline candidate {name!r} is not from frozen source/mode")
        floor = MIN_SAMPLES_FOR_AUTHORITY[artifact.mode]
        if any(row.samples < floor for row in artifact.rows.values()):
            raise RuntimeError(f"DSL baseline candidate {name!r} has fewer than {floor} samples")
        destination = repo_root / baseline["path"]
        try:
            prepared.append((destination, artifact_path.read_text(encoding="utf-8")))
        except OSError as exc:
            raise RuntimeError(f"DSL artifact disappeared: {artifact_path}: {exc}") from exc
    require_frozen_source(repo_root, initial_head)
    for destination, content in prepared:
        try:
            atomically_write_baseline(destination, content)
        except OSError as exc:
            raise RuntimeError(f"DSL baseline write failed: {destination}: {exc}") from exc
        print(f"baseline candidate written: {destination}")


def require_clean_preflight_receipt(receipt: Path, profile: str) -> None:
    """A diagnostic contention override cannot qualify a benchmark run."""
    try:
        payload = json.loads(receipt.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise RuntimeError(f"cannot read timing preflight receipt {receipt}: {exc}") from exc
    if not isinstance(payload, dict) or (
        payload.get("schema_version") != 1
        or payload.get("kind") != "quanta-index-timing-preflight"
        or payload.get("run_id") != f"benchctl:{profile}"
    ):
        raise RuntimeError(f"timing preflight receipt is not bound to profile {profile!r}")
    if payload.get("status") != "clean" or payload.get("foreign_rust_processes") != []:
        raise RuntimeError(
            f"timing preflight status {payload.get('status')!r} is not clean; diagnostic overrides cannot qualify"
        )
    try:
        require_clean_host_load(payload)
    except ArtifactRefused as exc:
        raise RuntimeError(str(exc)) from exc


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
                artifacts.append(
                    {"path": str(path.relative_to(repo_root)), "error": "not an object"}
                )
                continue
            provenance = payload.get("provenance")
            detail = payload.get("detail")
            artifacts.append(
                {
                    "path": str(path.relative_to(repo_root)),
                    "schema_version": payload.get("schema_version"),
                    "git_head": provenance.get("git_head")
                    if isinstance(provenance, dict)
                    else None,
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
        cold_samples = args.cold_samples
        if cold_samples is not None:
            if args.profile != "dsl-authority":
                print("ERROR: --cold-samples is only valid for dsl-authority", file=sys.stderr)
                return 2
            if cold_samples < 20:
                print(
                    "ERROR: --cold-samples must be at least 20 for authority comparison",
                    file=sys.stderr,
                )
                return 2
        if args.admit_baseline and args.profile != "dsl-authority":
            print("ERROR: --admit-baseline is only valid for dsl-authority", file=sys.stderr)
            return 2
        try:
            if not args.admit_baseline:
                require_declared_baselines(repo_root, profile, manifest)
            require_clean_worktree(repo_root)
            initial_head = resolve_checkout_head(repo_root)
        except RuntimeError as exc:
            print(f"ERROR: {exc}", file=sys.stderr)
            return 2
        receipt = repo_root / "artifacts" / "benchmark-receipts" / args.profile / "preflight.json"
        preflight_result = preflight(repo_root, args.profile, receipt, manifest)
        if preflight_result:
            print(
                f"ERROR: local timing preflight blocked; receipt written to {receipt}",
                file=sys.stderr,
            )
            return preflight_result
        try:
            require_clean_preflight_receipt(receipt, args.profile)
            require_frozen_source(repo_root, initial_head)
            preflight_digest = hashlib.sha256(receipt.read_bytes()).hexdigest()
        except RuntimeError as exc:
            print(f"ERROR: {exc}", file=sys.stderr)
            return 2
        except OSError as exc:
            print(f"ERROR: timing preflight receipt disappeared: {exc}", file=sys.stderr)
            return 2
        recipes = profile["recipes"]
        assert isinstance(recipes, list)
        if not recipes:
            print(f"ERROR: profile {args.profile!r} has no registered producer", file=sys.stderr)
            return 2
        capture_started_ns = time.time_ns()
        for recipe in recipes:
            assert isinstance(recipe, str)
            command = ["just", recipe]
            if recipe == "rust-bench-dsl-cold" and cold_samples is not None:
                command.append(str(cold_samples))
            completed = subprocess.run(command, cwd=repo_root, check=False)
            if completed.returncode:
                print(f"ERROR: producer recipe {recipe!r} failed", file=sys.stderr)
                return completed.returncode
            try:
                require_frozen_source(repo_root, initial_head)
            except RuntimeError as exc:
                print(f"ERROR: {exc}", file=sys.stderr)
                return 2
    validation = validate(repo_root, artifact_profile)
    if validation:
        return validation
    if args.command == "run":
        try:
            require_frozen_source(repo_root, initial_head)
            if args.admit_baseline:
                admit_dsl_baselines(
                    repo_root,
                    profile,
                    manifest,
                    receipt,
                    initial_head,
                    capture_started_ns,
                    preflight_digest,
                )
                return 0
        except RuntimeError as exc:
            print(f"ERROR: {exc}", file=sys.stderr)
            return 2
    if args.command in {"run", "compare"}:
        try:
            result = compare(repo_root, profile, manifest)
            if result:
                return result
            if args.command == "run":
                require_frozen_source(repo_root, initial_head)
            return 0
        except RuntimeError as exc:
            print(f"ERROR: {exc}", file=sys.stderr)
            return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
