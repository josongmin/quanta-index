#!/usr/bin/env python3
"""Run and validate named benchmark evidence profiles through one CLI.

This is the single current benchmark orchestrator. Producers remain the
registered owners (Justfile recipes, cargo bench targets and allowlisted Python
modules); the CLI owns source freeze, host preflight, profile execution,
artifact validation, immutable `BenchmarkEvidenceV1` promotion, verdict
comparison and fresh-process replay. It never accepts arbitrary shell commands,
because that would make a benchmark label independent from its actual
authority path.

The typed evidence contract is defined by the Rust crate
`benchmarks/bench-protocol`; `tools/benchmark/evidence.py` writes the same
canonical bytes. Registration lives in `tools/benchmark/registry.toml`.
"""

from __future__ import annotations

import argparse
import contextlib
import hashlib
import io
import json
import os
import platform
import socket
import stat
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from compare_dsl_bench import (  # noqa: E402
    FULL_HEAD_RE,
    MIN_SAMPLES_FOR_AUTHORITY,
    ArtifactRefused,
    atomically_write_baseline,
    fsync_directory,
    load_artifact,
    require_clean_host_load,
    require_clean_preflight,
    require_complete_baseline_candidate,
    require_no_pending_admission,
)
from evidence import EvidenceError, RunStore, digest_bytes  # noqa: E402
from manifest import DEFAULT_MANIFEST_PATH, ManifestError, load_manifest  # noqa: E402
from registry import load_registry, registry_digest  # noqa: E402

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
    """Full registry profile view: every registered profile is selectable.

    `recipes` names only the Justfile producers of the profile (cargo/Python/
    recorded producers are exposed by `plan`, not silently treated as native
    captures), so a profile with no Just recipe is still listed and resolvable.
    """
    registry = load_registry(path, repo_root=ROOT)
    families = registry["families"]
    producers = registry["producers"]
    profiles: dict[str, dict[str, object]] = {}
    for name, profile in registry["profiles"].items():
        recipes = [
            producers[families[family]["producer"]]["recipe"]
            for family in profile["families"]
            if families[family]["producer"] != "none"
            and producers[families[family]["producer"]]["kind"] == "just-recipe"
        ]
        profiles[name] = {
            "families": list(profile["families"]),
            "recipes": recipes,
            "description": profile["description"],
        }
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
    subparsers.add_parser("list", help="show registered benchmark profiles and the registry digest")
    subparsers.add_parser(
        "plan", help="emit the resolved, digest-bound plan for a profile"
    ).add_argument("profile", choices=sorted(profiles))
    replay = subparsers.add_parser(
        "replay", help="fresh-process re-validation of one immutable evidence run"
    )
    replay.add_argument(
        "reference",
        nargs="?",
        help="run id under <evidence-root>/runs/, or a path to a run directory",
    )
    replay.add_argument("--family", help="select the newest valid run of this registered family")
    replay.add_argument(
        "--evidence-root",
        type=Path,
        help="external benchmark root; defaults to $QUANTA_BENCH_EVIDENCE_ROOT",
    )
    for command, help_text in (
        ("run", "run producer recipes serially, then require their evidence"),
        ("validate", "require existing current-source evidence without running producers"),
        ("compare", "compare profile families that have a committed baseline"),
        ("summarize", "print a diagnostic inventory of declared profile evidence"),
    ):
        child = subparsers.add_parser(command, help=help_text)
        child.add_argument("profile", choices=sorted(profiles))
        if command in {"run", "validate"}:
            child.add_argument(
                "--evidence-root",
                type=Path,
                help=(
                    "external benchmark root; when set, promote (run) or require (validate) "
                    "immutable BenchmarkEvidenceV1 runs for every artifact family"
                ),
            )
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
        family = families.get(name)
        if family is None:
            # A registered family with no BenchArtifactV1 artifact cannot have
            # a declared artifact baseline; it is not silently treated as one.
            continue
        assert isinstance(family, dict)
        baseline = family["baseline"]
        if baseline is None:
            continue
        assert isinstance(baseline, dict)
        path = repo_root / baseline["path"]
        try:
            require_no_pending_admission(path)
        except ArtifactRefused as exc:
            raise RuntimeError(str(exc)) from exc
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


def _file_identity(value: os.stat_result) -> tuple[int, int, int, int, int, int]:
    return (
        value.st_dev,
        value.st_ino,
        value.st_mode,
        value.st_size,
        value.st_mtime_ns,
        value.st_ctime_ns,
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
    artifact_snapshots: list[tuple[Path, tuple[int, int, int, int, int, int]]] = []
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
            descriptor = os.open(artifact_path, os.O_RDONLY | os.O_NOFOLLOW)
            with os.fdopen(descriptor, "r", encoding="utf-8") as handle:
                artifact_stat = os.fstat(handle.fileno())
                if not stat.S_ISREG(artifact_stat.st_mode):
                    raise RuntimeError(f"DSL artifact is not a regular file: {artifact_path}")
                if min(artifact_stat.st_mtime_ns, artifact_stat.st_ctime_ns) < capture_started_ns:
                    raise RuntimeError(f"DSL artifact was not written by this run: {artifact_path}")
                content = handle.read()
                if _file_identity(os.fstat(handle.fileno())) != _file_identity(artifact_stat):
                    raise RuntimeError(f"DSL artifact changed while reading: {artifact_path}")
            current_stat = artifact_path.lstat()
            if _file_identity(current_stat) != _file_identity(artifact_stat):
                raise RuntimeError(f"DSL artifact changed after reading: {artifact_path}")
        except (OSError, UnicodeError) as exc:
            raise RuntimeError(f"fresh DSL artifact unreadable: {artifact_path}: {exc}") from exc
        try:
            artifact = load_artifact(artifact_path, role="baseline candidate", content=content)
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
        prepared.append((destination, content))
        artifact_snapshots.append((artifact_path, _file_identity(artifact_stat)))
    for artifact_path, identity in artifact_snapshots:
        try:
            current_identity = _file_identity(artifact_path.lstat())
        except OSError as exc:
            raise RuntimeError(
                f"DSL artifact disappeared during admission: {artifact_path}: {exc}"
            ) from exc
        if current_identity != identity:
            raise RuntimeError(f"DSL artifact changed during admission: {artifact_path}")
    try:
        final_receipt_digest = hashlib.sha256(receipt.read_bytes()).hexdigest()
    except OSError as exc:
        raise RuntimeError(f"DSL preflight receipt disappeared: {exc}") from exc
    if final_receipt_digest != preflight_digest:
        raise RuntimeError("DSL preflight receipt changed during admission")
    require_frozen_source(repo_root, initial_head)
    publish_dsl_baseline_pair(prepared)


def publish_dsl_baseline_pair(prepared: list[tuple[Path, str]]) -> None:
    """Reject a crash-interrupted pair and restore ordinary write failures."""
    if len(prepared) != 2 or prepared[0][0] == prepared[1][0]:
        raise RuntimeError("DSL baseline publication requires two distinct destinations")
    parent = prepared[0][0].parent
    if prepared[1][0].parent != parent:
        raise RuntimeError("DSL baseline pair must share one directory")
    marker = parent / ".dsl-admission-pending"
    prior: dict[Path, str | None] = {}
    for destination, _ in prepared:
        try:
            require_no_pending_admission(destination)
        except ArtifactRefused as exc:
            raise RuntimeError(str(exc)) from exc
        try:
            destination_stat = destination.lstat()
        except FileNotFoundError:
            prior[destination] = None
            continue
        except OSError as exc:
            raise RuntimeError(f"cannot inspect DSL baseline {destination}: {exc}") from exc
        if not stat.S_ISREG(destination_stat.st_mode):
            raise RuntimeError(f"DSL baseline is not a regular file: {destination}")
        try:
            prior[destination] = destination.read_text(encoding="utf-8")
        except (OSError, UnicodeError) as exc:
            raise RuntimeError(f"cannot preserve DSL baseline {destination}: {exc}") from exc

    try:
        parent.mkdir(parents=True, exist_ok=True)
        fsync_directory(parent.parent)
        descriptor = os.open(marker, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
        with os.fdopen(descriptor, "wb") as handle:
            handle.write(b"dsl-baseline-admission-v1\n")
            handle.flush()
            os.fsync(handle.fileno())
        fsync_directory(parent)
    except OSError as exc:
        raise RuntimeError(f"cannot start DSL baseline pair publication: {exc}") from exc

    written: list[Path] = []
    try:
        for destination, content in prepared:
            atomically_write_baseline(destination, content)
            written.append(destination)
    except OSError as exc:
        rollback_errors: list[str] = []
        for destination in reversed(written):
            try:
                old_content = prior[destination]
                if old_content is None:
                    destination.unlink()
                else:
                    atomically_write_baseline(destination, old_content)
            except OSError as rollback_error:
                rollback_errors.append(f"{destination}: {rollback_error}")
        if rollback_errors:
            raise RuntimeError(
                f"DSL baseline write failed: {exc}; rollback failed: {'; '.join(rollback_errors)}; "
                f"admission marker retained at {marker}"
            ) from exc
        try:
            fsync_directory(parent)
            marker.unlink()
            fsync_directory(parent)
        except OSError as cleanup_error:
            raise RuntimeError(
                f"DSL baseline write failed: {exc}; pair restored but marker cleanup failed: "
                f"{cleanup_error}"
            ) from exc
        raise RuntimeError(f"DSL baseline write failed and pair restored: {exc}") from exc
    try:
        marker.unlink()
        fsync_directory(parent)
    except OSError as exc:
        raise RuntimeError(f"DSL baseline pair written but marker cleanup failed: {exc}") from exc
    for destination in written:
        print(f"baseline candidate written: {destination}")


def require_clean_preflight_receipt(receipt: Path, profile: str) -> None:
    """A diagnostic contention override cannot qualify a benchmark run."""
    try:
        payload = _strict_json_bytes(receipt.read_bytes())
    except (OSError, ValueError) as exc:
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
        family = raw_families.get(name)
        if family is None:
            continue
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
        family = raw_families.get(name)
        if family is None:
            families.append(
                {
                    "family": name,
                    "host_policy": None,
                    "artifacts": [],
                    "status": "registered_without_bench_artifact",
                }
            )
            continue
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


def resolve_evidence_root(explicit: Path | None) -> Path | None:
    """External benchmark root; never inside the checkout."""
    if explicit is not None:
        return explicit.resolve()
    value = os.environ.get("QUANTA_BENCH_EVIDENCE_ROOT")
    return Path(value).resolve() if value else None


def producer_command(repo_root: Path, registry_entry: dict[str, object], family: str) -> list[str]:
    """Resolve one allowlisted producer invocation; no shell fragments."""
    kind = registry_entry.get("kind")
    if kind == "just-recipe":
        return ["just", str(registry_entry["recipe"])]
    if kind == "cargo-bench":
        return [
            str(repo_root / "scripts" / "cargow"),
            "--lane",
            "bench-lane",
            "bench",
            "-p",
            str(registry_entry["package"]),
            "--bench",
            str(registry_entry["target"]),
            "--all-features",
            "--locked",
        ]
    if kind == "python-module":
        argv = registry_entry.get("argv") or []
        assert isinstance(argv, list)
        return [sys.executable, str(repo_root / str(registry_entry["module"])), *map(str, argv)]
    raise RuntimeError(f"family {family!r} has no runnable producer kind {kind!r}")


def plan_command(repo_root: Path, profile_name: str) -> int:
    """Emit the resolved, digest-bound plan for a profile; never mutates."""
    registry = load_registry(repo_root / "tools" / "benchmark" / "registry.toml")
    profiles = registry["profiles"]
    families = registry["families"]
    producers = registry["producers"]
    selected = profiles[profile_name]["families"]
    steps: list[dict[str, object]] = []
    for family in selected:
        entry = families[family]
        producer_id = entry["producer"]
        if producer_id == "none":
            steps.append(
                {
                    "family": family,
                    "kind": "recorded-input",
                    "runnable": False,
                    "reason": "recorded-only family: the CLI cannot manufacture a capture",
                }
            )
            continue
        producer = producers[producer_id]
        steps.append(
            {
                "family": family,
                "kind": producer["kind"],
                "runnable": True,
                "command": producer_command(repo_root, producer, family),
                "outputs": list(producer.get("outputs") or []),
                "validator": entry["validator"],
                "scorer": entry["scorer"],
                "payload": entry["payload"],
                "host_policy": entry["host_policy"],
                "gate_tier": entry["gate_tier"],
                "sample_floor": entry["sample_floor"],
                "baseline": entry["baseline"],
                "closure": registry["closures"][entry["closure"]]["profile"],
            }
        )
    plan = {
        "registry_schema_version": registry["schema_version"],
        "registry_digest": registry_digest(registry),
        "profile": profile_name,
        "description": profiles[profile_name]["description"],
        "families": list(selected),
        "steps": steps,
        "mutates": False,
    }
    print(json.dumps(plan, sort_keys=True, indent=2))
    return 0


def latest_run_for_family(store: object, family: str) -> str | None:
    """Newest promoted run for one family, by evidence creation time."""
    runs = store.runs_dir
    if not runs.is_dir():
        return None
    candidates: list[tuple[str, str]] = []
    for entry in sorted(runs.iterdir()):
        if not entry.is_dir() or entry.is_symlink():
            raise EvidenceError(f"invalid run entry: {entry}")
        try:
            evidence = store.load(entry.name)
        except EvidenceError as exc:
            raise EvidenceError(f"invalid run {entry.name!r}: {exc}") from exc
        if evidence["family"] == family:
            candidates.append((str(evidence["created_utc"]), entry.name))
    if not candidates:
        return None
    candidates.sort()
    return candidates[-1][1]


def replay_command(repo_root: Path, reference: str, evidence_root: Path | None) -> int:
    """Fresh-process re-validation of one immutable run.

    Integrity is recomputed from raw bytes, and for a native `BenchArtifactV1`
    payload the independent artifact checker re-runs against the captured raw
    file. A changed input is refused; nothing is promoted.
    """
    candidate = Path(reference)
    if candidate.is_dir() and (candidate / "evidence.json").is_file():
        run_dir = candidate.resolve()
        store = RunStore(run_dir.parent.parent)
        run_id = candidate.name
    else:
        root = resolve_evidence_root(evidence_root)
        if root is None:
            print(
                "ERROR: replay needs a run directory or an --evidence-root/QUANTA_BENCH_EVIDENCE_ROOT",
                file=sys.stderr,
            )
            return 2
        store = RunStore(root)
        run_id = reference
        run_dir = store.run_dir(run_id)
    try:
        evidence = store.load(run_id)
    except EvidenceError as exc:
        print(f"ERROR: replay refused: {exc}", file=sys.stderr)
        return 2
    registry_path = repo_root / "tools" / "benchmark" / "registry.toml"
    registry = load_registry(registry_path, repo_root=repo_root)
    profile = registry["profiles"].get(evidence["profile"])
    if not isinstance(profile, dict) or evidence["family"] not in profile["families"]:
        print("ERROR: replay run family is not registered in its profile", file=sys.stderr)
        return 2
    native_families = load_manifest(registry_path, repo_root=repo_root)["families"]
    artifact_oracle = "not_applicable"
    if evidence["family"] in native_families:
        checker = _load_lint_module(repo_root)
        family = native_families[evidence["family"]]
        artifacts = []
        for reference in evidence["raw"]:
            native = run_dir / reference["path"]
            if native.suffix != ".json":
                print("ERROR: replay native artifact is not JSON", file=sys.stderr)
                return 2
            try:
                artifact = checker.parse_artifact_bytes(native.read_bytes())
            except (OSError, ValueError, json.JSONDecodeError) as exc:
                print(f"ERROR: replay cannot parse native raw {native}: {exc}", file=sys.stderr)
                return 2
            refusals = checker.check_artifact(
                artifact,
                dimension=evidence["family"],
                head=evidence["source"]["revision"],
                require=True,
                manifest={"families": native_families},
            )
            if refusals:
                print(
                    "ERROR: replay refused: native artifact oracle failed: " + "; ".join(refusals),
                    file=sys.stderr,
                )
                return 2
            artifacts.append(artifact)
        refusals = _check_native_inventory(artifacts, family, checker.CONCURRENCY_COUNTS)
        if refusals:
            print(f"ERROR: replay native inventory refused: {refusals}", file=sys.stderr)
            return 2
        if family["payload"] != evidence["payload"]["kind"]:
            print("ERROR: replay native payload kind differs from registry", file=sys.stderr)
            return 2
        from evidence_bridge import native_payload_from_artifacts

        try:
            derived = native_payload_from_artifacts(artifacts, family["payload"])
        except EvidenceError as exc:
            print(f"ERROR: replay cannot derive native payload: {exc}", file=sys.stderr)
            return 2
        if derived != evidence["payload"]:
            print("ERROR: replay typed payload differs from native artifact", file=sys.stderr)
            return 2
        artifact_oracle = "pass"
    receipt = {
        "run_id": run_id,
        "family": evidence["family"],
        "profile": evidence["profile"],
        "evidence_digest": evidence["digest"],
        "raw_references": len(evidence["raw"]),
        "raw_verified": True,
        "artifact_oracle": artifact_oracle,
        "payload_kind": evidence["payload"]["kind"],
        "verdict": evidence["verdict"],
        "replay": "contract_only" if artifact_oracle == "not_applicable" else "re_derived",
    }
    print(json.dumps(receipt, sort_keys=True, indent=2))
    return 0


def _strict_json_bytes(raw: bytes) -> object:
    return _load_lint_module(ROOT).parse_artifact_bytes(raw)


def _load_lint_module(repo_root: Path):
    import importlib.util

    path = repo_root / "tools" / "ci" / "lint" / "check-bench-artifacts.py"
    spec = importlib.util.spec_from_file_location("check_bench_artifacts", path)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def _check_native_inventory(
    artifacts: list[dict], family: dict, concurrency_counts: tuple[int, ...]
) -> str | None:
    """Enforce fan-out completeness during capture AND detached replay."""
    required = concurrency_counts if family.get("dimension") == "concurrency" else None
    if required is not None:
        from evidence_bridge import concurrency_clients_from_artifact

        try:
            actual = [concurrency_clients_from_artifact(artifact) for artifact in artifacts]
        except EvidenceError as exc:
            return str(exc)
        if any(type(value) is not int for value in actual) or sorted(actual) != sorted(required):
            return f"concurrency set {actual!r} differs from required {required!r}"
    elif len(artifacts) != 1:
        return "family requires exactly one native artifact"
    return None


def _capture_native_family(
    repo_root, family, entry, initial_head, manifest, capture_started_ns, validated_artifacts
):
    from evidence_bridge import native_payload_from_artifacts

    paths = sorted(repo_root.glob(entry["artifact_glob"]))
    if not paths:
        raise EvidenceError(f"family {family!r} has no artifact")
    checker, captures, artifacts = None, [], []
    for path in paths:
        if (
            path.is_symlink()
            or not path.is_file()
            or not path.resolve().is_relative_to(repo_root.resolve())
            or path.stat().st_mtime_ns < capture_started_ns
        ):
            raise EvidenceError(f"artifact is not fresh regular output: {path}")
        raw = path.read_bytes()
        if validated_artifacts.get(path.relative_to(repo_root).as_posix()) != digest_bytes(raw):
            raise EvidenceError(f"artifact changed after validation: {path}")
        if checker is None:
            checker = _load_lint_module(repo_root)
        artifact = checker.parse_artifact_bytes(raw)
        refusals = checker.check_artifact(
            artifact, dimension=family, head=initial_head, require=True, manifest=manifest
        )
        if refusals:
            raise EvidenceError(f"native artifact refused: {'; '.join(refusals)}")
        captures.append((path, raw))
        artifacts.append(artifact)
    refusal = _check_native_inventory(artifacts, entry, checker.CONCURRENCY_COUNTS)
    if refusal:
        raise EvidenceError(refusal)
    return captures, artifacts, native_payload_from_artifacts(artifacts, entry["payload"])


def promote_profile_runs(
    repo_root: Path,
    profile_name: str,
    manifest: dict[str, object],
    evidence_root: Path,
    initial_head: str,
    receipt: Path,
    preflight_digest: str,
    capture_started_ns: int,
    validated_artifacts: dict[str, str],
    execution: dict[str, object] | None = None,
) -> int:
    """Promote each family *of this profile* into an immutable evidence run.

    Only the profile's declared artifact families are promoted; promoting every
    registered family would silently widen what a profile capture claims.
    """
    from evidence import digest_bytes
    from evidence_bridge import (
        host_identity,
        promote_native_run,
        source_identity,
    )

    promotion_started = time.monotonic_ns()

    families = manifest["families"]
    profiles = manifest["profiles"]
    assert isinstance(families, dict) and isinstance(profiles, dict)
    selected = profiles[profile_name]["families"]
    store = RunStore(evidence_root)
    created = datetime.now(timezone.utc).isoformat(timespec="microseconds").replace("+00:00", "Z")
    stamp = f"{time.strftime('%Y%m%dT%H%M%SZ', time.gmtime())}-{time.time_ns()}"
    hostname = socket.gethostname() or "unknown"
    try:
        require_clean_preflight_receipt(receipt, profile_name)
        receipt_bytes = receipt.read_bytes()
    except (RuntimeError, OSError) as exc:
        print(f"ERROR: cannot read clean benchmark preflight receipt: {exc}", file=sys.stderr)
        return 2
    if digest_bytes(receipt_bytes) != preflight_digest:
        print("ERROR: benchmark preflight receipt changed during capture", file=sys.stderr)
        return 2
    lease_mode = "shared"
    lease_samples = 1
    promoted: list[str] = []
    prepared = {}
    try:
        for family in selected:
            prepared[family] = _capture_native_family(
                repo_root,
                family,
                families[family],
                initial_head,
                manifest,
                capture_started_ns,
                validated_artifacts,
            )
            _native_inputs(prepared[family][1], preflight_digest)
    except (EvidenceError, OSError, ValueError) as exc:
        print(f"ERROR: profile native capture refused: {exc}", file=sys.stderr)
        return 2
    for family in selected:
        entry = families[family]
        captures, artifacts, payload = prepared[family]
        path, native_bytes = captures[0]
        artifact = artifacts[0]
        if artifact.get("provenance", {}).get("git_head") != initial_head:
            print(
                f"ERROR: family {family!r} artifact is not from the frozen source", file=sys.stderr
            )
            return 2
        try:
            source = source_identity(repo_root, "benchmark-control-plane")
        except EvidenceError as exc:
            print(f"ERROR: cannot bind source closure: {exc}", file=sys.stderr)
            return 2
        if source.get("dirty") is not False or source.get("revision") != initial_head:
            print("ERROR: benchmark source changed during promotion", file=sys.stderr)
            return 2
        # Promotion occurs only after the native validator and declared
        # comparator have both succeeded in this run.
        verdict_status = "pass"
        verdict_reason = None
        if any(a.get("detail", {}).get("passed") is False for a in artifacts):
            verdict_status = "fail"
            verdict_reason = "rail verdict false"
        run_id = f"{family}-{stamp}-{digest_bytes(b''.join(raw for _, raw in captures))[7:15]}"
        try:
            promotion = promote_native_run(
                evidence_root=evidence_root,
                run_id=run_id,
                family=family,
                profile=profile_name,
                created_utc=created,
                native_path=path,
                native_bytes=native_bytes,
                additional_native=captures[1:],
                payload=payload,
                source=source,
                build={
                    "toolchain": _toolchain_identity(repo_root),
                    "target_triple": f"{sys.platform}-{platform.machine()}",
                    "lockfile_digest": digest_bytes((repo_root / "Cargo.lock").read_bytes()),
                    "profile": "producer-recipe",
                    "flags": [],
                    "binaries": [],
                },
                inputs=_native_inputs(artifacts, preflight_digest),
                host=host_identity(
                    policy=str(entry["host_policy"]),
                    os_name=_host_os(),
                    arch=platform.machine() or "unknown",
                    cpu_count=os.cpu_count() or 1,
                    hostname=hostname,
                    lease_mode=lease_mode,
                    lease_samples=lease_samples,
                ),
                command=execution
                or {
                    "argv": ["benchctl", "promote-native", profile_name],
                    "cwd": ".",
                    "status": "completed",
                    "exit_code": 0,
                    "timeout_seconds": 3600,
                    "wall_ms": (time.monotonic_ns() - promotion_started) // 1_000_000,
                },
                boundary={
                    "clock": "monotonic",
                    "instrumentation": "none",
                    "start_event": "profile_producer_exec"
                    if execution
                    else "native_promotion_start",
                    "end_event": "profile_producers_completed"
                    if execution
                    else "native_promotion_envelope",
                },
                verdict={
                    "scope": "diagnostic",
                    "status": verdict_status,
                    "reason": verdict_reason,
                    "metrics": [],
                },
                case_id=None,
            )
        except EvidenceError as exc:
            print(f"ERROR: family {family!r} promotion refused: {exc}", file=sys.stderr)
            return 2
        promoted.append(promotion["run_id"])
        print(f"promoted run: {promotion['run_dir']}")
    latest = store.read_latest()
    if latest is None or latest.get("run_id") != promoted[-1]:
        print("ERROR: latest pointer was not updated by promotion", file=sys.stderr)
        return 2
    print(json.dumps({"profile": profile_name, "promoted": promoted}, sort_keys=True))
    return 0


def _host_os() -> str:
    if sys.platform.startswith("linux"):
        return "linux"
    if sys.platform == "darwin":
        return "macos"
    return "windows"


def _toolchain_identity(repo_root: Path) -> str:
    pinned = repo_root / "rust-toolchain.toml"
    channel = "unknown"
    try:
        for line in pinned.read_text(encoding="utf-8").splitlines():
            stripped = line.strip()
            if stripped.startswith("channel"):
                channel = stripped.split("=", 1)[1].strip().strip('"')
    except (OSError, IndexError):
        channel = "unknown"
    completed = subprocess.run(["rustc", "--version"], check=False, capture_output=True, text=True)
    if completed.returncode == 0 and completed.stdout.strip():
        return f"{completed.stdout.strip()} (pinned {channel})"
    return f"unresolved rustc (pinned {channel})"


def _declared_inputs(payload: dict[str, object], preflight_digest: str) -> list[dict[str, object]]:
    """Inputs the payload actually binds; `corpus` is unavailable when unbound."""
    preflight = {
        "id": "benchmark-preflight",
        "availability": "present",
        "digest": preflight_digest,
        "reason": None,
    }
    if payload.get("kind") == "retrieval":
        return [
            preflight,
            {
                "id": "corpus",
                "availability": "present",
                "digest": payload["corpus_digest"],
                "reason": None,
            },
            {
                "id": "query-pack",
                "availability": "present",
                "digest": payload["query_pack_digest"],
                "reason": None,
            },
        ]
    return [
        preflight,
        {
            "id": "workspace-fixture",
            "availability": "unavailable",
            "digest": None,
            "reason": "the rail builds its deterministic fixture in-process; no external corpus",
        },
    ]


def _native_inputs(artifacts: list[dict], preflight_digest: str) -> list[dict]:
    inputs = [
        {
            "id": "benchmark-preflight",
            "availability": "present",
            "digest": preflight_digest,
            "reason": None,
        }
    ]
    for key in ("corpus_digest",):
        identities = {artifact["provenance"][key] for artifact in artifacts}
        if len(identities) != 1:
            raise EvidenceError(f"native artifact set mixes {key}")
        inputs.append(
            {
                "id": key.removesuffix("_digest"),
                "availability": "present",
                "digest": identities.pop(),
                "reason": None,
            }
        )
    for index, artifact in enumerate(artifacts):
        inputs.append(
            {
                "id": "config" if len(artifacts) == 1 else f"config-{index}",
                "availability": "present",
                "digest": artifact["provenance"]["config_digest"],
                "reason": None,
            }
        )
    return inputs


def snapshot_profile_artifacts(
    repo_root: Path, profile_name: str, manifest: dict[str, object]
) -> dict[str, str]:
    """Freeze the exact native bytes accepted before a comparator runs."""
    from evidence import digest_bytes

    families = manifest["families"]
    selected = manifest["profiles"][profile_name]["families"]
    assert isinstance(families, dict)
    frozen: dict[str, str] = {}
    for family in selected:
        paths = sorted(repo_root.glob(families[family]["artifact_glob"]))
        if not paths:
            raise RuntimeError(f"family {family!r} has no validated artifact")
        for path in paths:
            if (
                path.is_symlink()
                or not path.is_file()
                or not path.resolve().is_relative_to(repo_root.resolve())
            ):
                raise RuntimeError(f"family {family!r} has non-regular artifact: {path}")
            relative_path = path.relative_to(repo_root).as_posix()
            if relative_path in frozen:
                raise RuntimeError(
                    f"artifact belongs to multiple profile families: {relative_path}"
                )
            try:
                frozen[relative_path] = digest_bytes(path.read_bytes())
            except OSError as exc:
                raise RuntimeError(f"cannot freeze artifact {path}: {exc}") from exc
    return frozen


def validate_promoted_runs(
    evidence_root: Path,
    profile_name: str,
    manifest: dict[str, object],
    expected_source: dict[str, object],
    expected_lock_digest: str,
    repo_root: Path = ROOT,
) -> int:
    """Require a valid promoted run for every artifact family in the profile."""
    store = RunStore(evidence_root)
    families = manifest["families"]
    profiles = manifest["profiles"]
    assert isinstance(families, dict) and isinstance(profiles, dict)
    selected = profiles[profile_name]["families"]
    receipts: list[dict[str, object]] = []
    missing: list[str] = []
    capture_identity: tuple[str, str] | None = None
    for family in selected:
        try:
            run_id = latest_run_for_family(store, family)
        except EvidenceError as exc:
            print(f"ERROR: cannot select promoted run: {exc}", file=sys.stderr)
            return 2
        if run_id is None:
            missing.append(family)
            continue
        try:
            evidence = store.load(run_id)
        except EvidenceError as exc:
            print(f"ERROR: promoted run for {family!r} is invalid: {exc}", file=sys.stderr)
            return 2
        if evidence["profile"] != profile_name or evidence["source"] != expected_source:
            print(f"ERROR: run {run_id!r} has wrong profile or source identity", file=sys.stderr)
            return 2
        if evidence["build"]["lockfile_digest"] != expected_lock_digest:
            print(f"ERROR: run {run_id!r} has wrong lockfile identity", file=sys.stderr)
            return 2
        if evidence["verdict"]["status"] != "pass":
            print(f"ERROR: run {run_id!r} has non-passing verdict", file=sys.stderr)
            return 2
        if evidence["payload"]["kind"] != families[family]["payload"]:
            print(f"ERROR: run {run_id!r} has wrong payload kind", file=sys.stderr)
            return 2
        if evidence["host"]["policy"] != families[family]["host_policy"]:
            print(f"ERROR: run {run_id!r} has wrong host policy", file=sys.stderr)
            return 2
        preflights = [item for item in evidence["inputs"] if item["id"] == "benchmark-preflight"]
        if (
            len(preflights) != 1
            or preflights[0]["availability"] != "present"
            or not isinstance(preflights[0]["digest"], str)
        ):
            print(f"ERROR: run {run_id!r} lacks a unique preflight binding", file=sys.stderr)
            return 2
        identity = (preflights[0]["digest"], evidence["created_utc"])
        if capture_identity is None:
            capture_identity = identity
        elif identity != capture_identity:
            print("ERROR: promoted profile mixes different benchmark captures", file=sys.stderr)
            return 2
        receipts.append(
            {
                "family": family,
                "run_id": run_id,
                "digest": evidence["digest"],
                "verdict": evidence["verdict"]["status"],
                "payload": evidence["payload"]["kind"],
            }
        )
    if missing:
        print(
            f"ERROR: profile {profile_name!r} has no promoted evidence run for: "
            + ", ".join(sorted(missing)),
            file=sys.stderr,
        )
        return 2
    for receipt in receipts:
        with contextlib.redirect_stdout(io.StringIO()):
            if replay_command(repo_root, str(store.run_dir(receipt["run_id"])), evidence_root):
                print(
                    f"ERROR: promoted run {receipt['run_id']!r} failed native replay",
                    file=sys.stderr,
                )
                return 2
    print(json.dumps({"profile": profile_name, "runs": receipts}, sort_keys=True, indent=2))
    return 0


def main(argv: list[str] | None = None) -> int:
    bootstrap = argparse.ArgumentParser(add_help=False)
    bootstrap.add_argument("--repo-root", type=Path, default=ROOT)
    bootstrap_args, _ = bootstrap.parse_known_args(argv)
    repo_root = bootstrap_args.repo_root.resolve()
    try:
        profiles = load_profiles(repo_root / "tools" / "benchmark" / "registry.toml")
    except ManifestError as exc:
        print(f"ERROR: invalid benchmark profile manifest: {exc}", file=sys.stderr)
        return 2
    args = parse_args(argv, profiles, repo_root=repo_root)
    repo_root = args.repo_root.resolve()
    if args.command == "list":
        registry = load_registry(repo_root / "tools" / "benchmark" / "registry.toml")
        for name in sorted(profiles):
            profile = profiles[name]
            print(f"{name}\t{name}\t{profile.get('description', '')}")
        print(f"registry-digest\t-\t{registry_digest(registry)}")
        return 0
    if args.command == "plan":
        return plan_command(repo_root, args.profile)
    if args.command == "replay":
        if bool(args.reference) == bool(args.family):
            print("ERROR: replay requires exactly one run reference or --family", file=sys.stderr)
            return 2
        reference = args.reference
        if args.family:
            evidence_root = resolve_evidence_root(args.evidence_root)
            if evidence_root is None:
                print("ERROR: --family requires an evidence root", file=sys.stderr)
                return 2
            try:
                reference = latest_run_for_family(RunStore(evidence_root), args.family)
            except EvidenceError as exc:
                print(f"ERROR: cannot select replay run: {exc}", file=sys.stderr)
                return 2
            if reference is None:
                print(f"ERROR: no promoted run for family {args.family!r}", file=sys.stderr)
                return 2
        return replay_command(repo_root, reference, args.evidence_root)

    profile = profiles[args.profile]
    artifact_profile = args.profile
    manifest = load_manifest(repo_root / "tools" / "benchmark" / "registry.toml")
    native_profile = manifest["profiles"].get(args.profile)
    native_families = native_profile["families"] if native_profile else []
    fully_native = set(native_families) == set(profile["families"])
    if args.command == "preflight" and not fully_native:
        print(
            f"ERROR: profile {args.profile!r} has no native timing preflight; use `plan`",
            file=sys.stderr,
        )
        return 2
    if args.command == "summarize" and not fully_native:
        print(
            json.dumps(
                {
                    "profile": args.profile,
                    "families": profile["families"],
                    "status": "registered_not_captured",
                    "measurement_count": None,
                },
                sort_keys=True,
            )
        )
        return 0
    if args.command in {"run", "validate", "compare"}:
        if not fully_native:
            print(
                f"ERROR: profile {args.profile!r} needs a non-native capture adapter; "
                "`plan` shows the registered owner commands. No producer was executed.",
                file=sys.stderr,
            )
            return 2
        if args.command == "run" and getattr(args, "evidence_root", None) is not None:
            unsupported = [
                family
                for family in native_families
                if manifest["families"][family]["payload"] not in {"latency", "load", "freshness"}
            ]
            if unsupported:
                print(
                    f"ERROR: profile has no native payload adapter for {unsupported!r}; no producer was executed",
                    file=sys.stderr,
                )
                return 2
    if args.command == "preflight":
        return preflight(repo_root, args.profile, args.receipt, manifest)
    if args.command == "summarize":
        return summarize(repo_root, profile, manifest)
    if args.command == "run":
        requested_root = resolve_evidence_root(args.evidence_root)
        if requested_root is not None and (
            requested_root == repo_root or repo_root in requested_root.parents
        ):
            print(
                "ERROR: --evidence-root must stay outside the checkout; no producer was executed",
                file=sys.stderr,
            )
            return 2
        if args.admit_baseline and requested_root is not None:
            print(
                "ERROR: baseline admission and immutable capture are separate actions; omit --evidence-root for admission",
                file=sys.stderr,
            )
            return 2
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
        execution_started_ns = time.monotonic_ns()
        for recipe in recipes:
            assert isinstance(recipe, str)
            command = ["just", recipe]
            if recipe == "rust-bench-dsl-cold" and cold_samples is not None:
                command.append(str(cold_samples))
            try:
                completed = subprocess.run(command, cwd=repo_root, check=False, timeout=3600)
            except subprocess.TimeoutExpired:
                print(f"ERROR: producer recipe {recipe!r} timed out", file=sys.stderr)
                return 2
            if completed.returncode:
                print(f"ERROR: producer recipe {recipe!r} failed", file=sys.stderr)
                return completed.returncode
            try:
                require_frozen_source(repo_root, initial_head)
            except RuntimeError as exc:
                print(f"ERROR: {exc}", file=sys.stderr)
                return 2
        execution = {
            "argv": [
                sys.executable,
                str(Path(__file__).resolve()),
                *(argv if argv is not None else sys.argv[1:]),
            ],
            "cwd": ".",
            "status": "completed",
            "exit_code": 0,
            "timeout_seconds": 3600 * len(recipes),
            "wall_ms": (time.monotonic_ns() - execution_started_ns) // 1_000_000,
        }
    evidence_root = resolve_evidence_root(getattr(args, "evidence_root", None))
    if args.command == "validate" and evidence_root is not None:
        from evidence import digest_bytes
        from evidence_bridge import source_identity

        try:
            require_clean_worktree(repo_root)
            expected_source = source_identity(repo_root, "benchmark-control-plane")
            expected_lock = digest_bytes((repo_root / "Cargo.lock").read_bytes())
        except (RuntimeError, EvidenceError, OSError) as exc:
            print(f"ERROR: cannot establish current validation identity: {exc}", file=sys.stderr)
            return 2
        return validate_promoted_runs(
            evidence_root, args.profile, manifest, expected_source, expected_lock, repo_root
        )
    if artifact_profile in manifest["profiles"]:
        validation = validate(repo_root, artifact_profile)
        if validation:
            return validation
    else:
        # Crate-local Criterion, retrieval and recorded-only profiles have no
        # BenchArtifactV1 family; they are exercised through plan/replay or an
        # explicit --evidence-root, never through the artifact checker.
        print(
            f"ERROR: profile {args.profile!r} registers no BenchArtifactV1 family; "
            "use `plan` or an explicit --evidence-root",
            file=sys.stderr,
        )
        return 2
    validated_artifacts = None
    if args.command == "run" and evidence_root is not None:
        if evidence_root == repo_root or repo_root in evidence_root.parents:
            print(
                "ERROR: --evidence-root must stay outside the checkout; run artifacts are external",
                file=sys.stderr,
            )
            return 2
        try:
            validated_artifacts = snapshot_profile_artifacts(repo_root, args.profile, manifest)
        except RuntimeError as exc:
            print(f"ERROR: {exc}", file=sys.stderr)
            return 2
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
                if (
                    validated_artifacts is not None
                    and validated_artifacts
                    != snapshot_profile_artifacts(repo_root, args.profile, manifest)
                ):
                    raise RuntimeError("benchmark artifacts changed during comparison")
        except RuntimeError as exc:
            print(f"ERROR: {exc}", file=sys.stderr)
            return 2
    if args.command == "run" and evidence_root is not None:
        assert validated_artifacts is not None
        result = promote_profile_runs(
            repo_root,
            args.profile,
            manifest,
            evidence_root,
            initial_head,
            receipt,
            "sha256:" + preflight_digest,
            capture_started_ns,
            validated_artifacts,
            execution,
        )
        if result:
            return result
        try:
            require_frozen_source(repo_root, initial_head)
        except RuntimeError as exc:
            print(f"ERROR: {exc}", file=sys.stderr)
            return 2
    if args.command in {"run", "compare"}:
        return 0
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
