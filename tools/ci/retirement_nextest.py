"""Validate current test source and Nextest terminal evidence for retirement."""

from __future__ import annotations

import fcntl
import hashlib
import json
import os
import platform
import re
import stat
import subprocess
import tempfile
from collections.abc import Callable, Iterator, Mapping
from contextlib import suppress
from functools import cache, lru_cache
from pathlib import Path
from uuid import uuid4

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - Python 3.10 compatibility
    import tomli as tomllib

from tools.benchmark.evidence import RawFile
from tools.ci.nextest_events import parse_nextest, parse_nextest_inventory
from tools.ci.proof_json import parse_proof_json
from tools.ci.rust_test_mount import committed_path_resolver, retirement_event_resolver
from tools.ci.source_closure import (
    ClosureError,
    capture_committed_source,
    require_source_files_unchanged,
)

RUST_PROOF_SOURCE = re.compile(r"crates/[a-z0-9-]+/(?:src|tests)/(?:[a-zA-Z0-9_-]+/)*[a-zA-Z0-9_-]+\.rs")
NEXTEST_RUN_ID = re.compile(r"[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}")
SHA256 = re.compile(r"[0-9a-f]{64}")
GIT_REVISION = re.compile(r"[0-9a-f]{40}|[0-9a-f]{64}")
NEXTEST_EVENT = re.compile(r"[a-z0-9-]+::[a-zA-Z0-9_-]+\$[a-zA-Z0-9_]+(?:::[a-zA-Z0-9_]+)*")
LEGACY_BINARY_ENV = "QUANTA_INDEX_LEGACY_SEARCHD_BIN"
LEGACY_SHA_ENV = "QUANTA_INDEX_LEGACY_SEARCHD_SHA256"
LEGACY_EXECUTABLE = "legacy-searchd"
# The selected Runtime compatibility suites launch the same external daemon
# as the Lexical migration tests, including through their shared SDK fixture.
OLD_Q_TARGETS = {
    "quanta-index-lexical": frozenset({"legacy_existing", "sealed_commitment_cost"}),
    "quanta-index-searchd-runtime": frozenset({"process_readiness_owner_v1", "runtime_fast_suite"}),
}
LINUX_OLD_Q_PLATFORM = "x86_64-unknown-linux-gnu"

# The evidence digests are written after the run, so their two ledgers are
# excluded from source identity. Every other committed path is included.
MIGRATION_LEDGERS = (
    b"tools/ci/inventory/lexical-test-disposition.toml",
    b"tools/ci/inventory/runtime-legacy-migration.toml",
)


def release_event_roster(root: Path, revision: str) -> dict[str, dict[str, str]]:
    """Resolve the exact Nextest events required by both committed retirement ledgers."""
    root = root.resolve(strict=True)
    if not GIT_REVISION.fullmatch(revision):
        raise ValueError("retirement roster requires a full Git revision")

    def git(workspace: Path, *args: str) -> bytes:
        return subprocess.check_output(["git", "--no-replace-objects", *args], cwd=workspace)

    @cache
    def read(relative: str) -> str:
        return git(root, "show", f"{revision}:{relative}").decode("utf-8")

    lexical, runtime = (tomllib.loads(read(path.decode("ascii"))) for path in MIGRATION_LEDGERS)
    candidates = [
        candidate
        for target in lexical["targets"]
        for group in target["dispositions"]
        for candidate in group.get("candidates", [])
    ]
    candidates.extend(
        {"path": target["path"], "tests": group["tests"]}
        for target in lexical["targets"]
        for group in target["dispositions"] if group["disposition"] == "retain"
    )
    candidates.extend(
        candidate
        for group in runtime["current_proofs"] + runtime["exact_oracles"]
        for candidate in group["candidates"]
    )
    declared: set[tuple[str, str]] = set()
    for candidate in candidates:
        relative, tests = candidate["path"], candidate["tests"]
        if not isinstance(relative, str) or not RUST_PROOF_SOURCE.fullmatch(relative):
            raise ValueError(f"invalid retirement roster source: {relative!r}")
        if not isinstance(tests, list) or not tests or any(
            not isinstance(case, str) or not case.isascii() or not case.isidentifier() for case in tests
        ):
            raise ValueError(f"invalid retirement roster test declaration: {relative}")
        declared.update((relative, case) for case in tests)

    resolve = retirement_event_resolver(root, read, resolve_path=committed_path_resolver(git, root, revision))
    events: dict[str, dict[str, str]] = {}
    for relative, case in sorted(declared):
        read(relative)
        event = resolve(root / relative, case)
        if event is None or not NEXTEST_EVENT.fullmatch(event) or event in events:
            raise ValueError(f"retirement roster event is missing or ambiguous: {relative}::{case}")
        events[event] = {"source": relative, "case": case}
    if not events:
        raise ValueError("retirement roster has no events")
    return events


@lru_cache(maxsize=8)
def _committed_event_features(root: Path, revision: str) -> dict[str, frozenset[str]]:
    profiles = {}
    for batch in release_capture_batches(root, revision):
        features = frozenset(batch["features"])
        profiles.update((event, features) for event in batch["expected_events"])
    return profiles


def _require_roster_selection(
    root: Path, revision: str, events: set[str], features: list[str],
) -> None:
    profiles = _committed_event_features(root, revision)
    outside = events - profiles.keys()
    if outside:
        raise ValueError(f"Nextest events outside the committed retirement roster: {sorted(outside)}")
    selected_features = frozenset(features)
    for event in sorted(events):
        if selected_features != profiles[event]:
            raise ValueError(
                f"Nextest Cargo features differ from the committed retirement profile for {event}: "
                f"expected={sorted(profiles[event])} observed={sorted(selected_features)}"
            )


def release_capture_batches(root: Path, revision: str) -> list[dict]:
    """Partition the committed roster into exact Cargo package and feature selections."""
    roster = release_event_roster(root, revision)

    @cache
    def test_features(package: str) -> dict[str, tuple[str, ...]]:
        manifest = tomllib.loads(subprocess.check_output(
            ["git", "--no-replace-objects", "show", f"{revision}:crates/{package}/Cargo.toml"],
            cwd=root,
        ).decode("utf-8"))
        return {
            target["name"]: tuple(sorted(
                feature if "/" in feature else f"{package}/{feature}"
                for feature in target.get("required-features", [])
            ))
            for target in manifest.get("test", [])
        }

    groups: dict[tuple[str, tuple[str, ...]], dict[str, list[str]]] = {}
    for event, source in sorted(roster.items()):
        binary, _, _ = event.partition("$")
        package, target = binary.split("::")
        is_lib = Path(source["source"]).parts[2] == "src"
        features = () if is_lib else test_features(package).get(target, ())
        groups.setdefault((package, features), {}).setdefault(target, []).append(event)

    batches = []
    for (package, features), targets in sorted(groups.items()):
        include_lib = False
        test_targets = []
        filter_parts = []
        events = []
        for target, cases in sorted(targets.items()):
            source_kinds = {Path(roster[event]["source"]).parts[2] for event in cases}
            if len(source_kinds) != 1:
                raise ValueError(f"ambiguous retirement target kind: {package}::{target}")
            if source_kinds == {"src"}:
                include_lib = True
            else:
                test_targets.append(target)
            tests = " | ".join(f"test(={event.partition('$')[2]})" for event in cases)
            filter_parts.append(f"(binary(={target}) & ({tests}))")
            events.extend(cases)
        batches.append({
            "package": package, "include_lib": include_lib, "test_targets": test_targets,
            "features": list(features), "filterset": " | ".join(filter_parts),
            "expected_events": sorted(events),
        })
    return batches


@lru_cache(maxsize=32)
def release_tree_sha256(root: Path, revision: str) -> str:
    if not GIT_REVISION.fullmatch(revision):
        raise ValueError("frozen source manifest requires a full Git revision")
    tree = subprocess.check_output(
        ["git", "--no-replace-objects", "ls-tree", "-rz", "--full-tree", revision],
        cwd=root,
    )
    if not tree:
        raise ValueError("frozen source tree is empty")
    digest = hashlib.sha256()
    for entry in tree.removesuffix(b"\0").split(b"\0"):
        if entry.partition(b"\t")[2] not in MIGRATION_LEDGERS:
            digest.update(entry + b"\0")
    return digest.hexdigest()


def _retained_file_state(path: Path, *, label: str = "input") -> tuple[tuple[int, ...], ...]:
    parents = []
    for parent in reversed(path.parents):
        info = parent.lstat()
        if not stat.S_ISDIR(info.st_mode):
            raise ValueError(f"invalid retained Nextest {label} directory: {parent}")
        parents.append((info.st_dev, info.st_ino, info.st_mode))
    info = path.lstat()
    if not stat.S_ISREG(info.st_mode):
        raise ValueError(f"invalid retained Nextest {label}: {path}")
    return (*parents, (info.st_dev, info.st_ino, info.st_mode, info.st_nlink,
                       info.st_size, info.st_mtime_ns, info.st_ctime_ns))


def retained_executable_digests() -> Callable[[Path], str]:
    """Hash each unchanged executable once while checking a single ledger."""
    verified: dict[Path, tuple[tuple[tuple[int, ...], ...], str]] = {}

    def read(path: Path) -> str:
        path = path.absolute()
        before = _retained_file_state(path, label="executable")
        if (previous := verified.get(path)) is not None and previous[0] == before:
            return previous[1]
        digest = RawFile.capture(path).sha256.removeprefix("sha256:")
        if _retained_file_state(path, label="executable") != before:
            raise ValueError(f"retained Nextest executable changed while hashing: {path}")
        verified[path] = (before, digest)
        return digest

    return read


@lru_cache(maxsize=8)
def _approved_linux_old_q_sha256(root: Path, revision: str) -> str:
    import yaml

    if not GIT_REVISION.fullmatch(revision):
        raise ValueError("old-Q approval requires a full Git revision")
    try:
        raw = subprocess.check_output(
            ["git", "--no-replace-objects", "show", f"{revision}:.circleci/config.yml"], cwd=root,
        )
        digest = yaml.safe_load(raw)["jobs"]["verify-rust-tests"]["environment"][LEGACY_SHA_ENV]
    except (KeyError, TypeError, subprocess.CalledProcessError, yaml.YAMLError) as error:
        raise ValueError("release candidate lacks the committed approved Linux old-Q pin") from error
    if not isinstance(digest, str) or not SHA256.fullmatch(digest):
        raise ValueError("release candidate has an invalid approved Linux old-Q SHA-256")
    return digest


def _uses_old_q(package: str, test_targets: list[str]) -> bool:
    return not OLD_Q_TARGETS.get(package, frozenset()).isdisjoint(test_targets)


def _requires_linux_boot_inventory(package: str, test_targets: list[str]) -> bool:
    return package == "quanta-index-lexical" and "boot_inventory_data" in test_targets


def _require_approved_old_q(
    root: Path, revision: str, package: str, test_targets: list[str], digest: str | None,
) -> None:
    if _uses_old_q(package, test_targets):
        if digest is None:
            raise ValueError("retained old-Q executable is absent")
        if digest != _approved_linux_old_q_sha256(root, revision):
            raise ValueError("retained old-Q executable does not match the committed approved Linux old-Q SHA-256")


def check_frozen_inputs(
    artifact: RawFile, observation: dict, root: Path, release_revision: str,
    *, executable_digest: Callable[[Path], str] | None = None,
) -> None:
    run_id = observation["run_id"]
    if artifact.sha256 != f"sha256:{observation['source_manifest_sha256']}":
        raise ValueError(f"frozen source manifest SHA-256 differs for {run_id}")
    manifest = parse_proof_json(artifact.read_control())
    if not isinstance(manifest, dict) or set(manifest) != {
        "head", "source_tree_sha256", "inventory_sha256", "events_sha256", "execution",
    } or not isinstance(manifest["head"], str):
        raise ValueError(f"frozen source manifest lacks its revision and terminal binding for {run_id}")
    run_tree = release_tree_sha256(root, manifest["head"])
    if run_tree != manifest["source_tree_sha256"]:
        raise ValueError(f"frozen source tree SHA-256 differs for {run_id}")
    if run_tree != release_tree_sha256(root, release_revision):
        raise ValueError(f"frozen source tree differs from the release candidate for {run_id}")
    for key in ("inventory_sha256", "events_sha256"):
        if manifest[key] != observation[key]:
            raise ValueError(f"frozen source manifest {key} differs from the observation for {run_id}")
    execution = manifest["execution"]
    if not isinstance(execution, dict) or set(execution) != {
        "binaries_sha256", "cargo_sha256", "executables", "selection", "commands", "legacy_searchd",
        "workspace_root",
    }:
        raise ValueError(f"frozen source manifest lacks Nextest build and executable bindings for {run_id}")
    folder = artifact.path.parent
    binaries = RawFile.capture(folder / f"{run_id}-binaries.json")
    cargo = RawFile.capture(folder / f"{run_id}-cargo.json")
    inventory = RawFile.capture(folder / f"{run_id}-inventory.json")
    if binaries.sha256 != execution["binaries_sha256"] or cargo.sha256 != execution["cargo_sha256"]:
        raise ValueError(f"retained Nextest build inputs differ for {run_id}")
    if inventory.sha256 != f"sha256:{observation['inventory_sha256']}":
        raise ValueError(f"Nextest inventory SHA-256 differs for {run_id}")
    workspace_root = execution["workspace_root"]
    if (
        not isinstance(workspace_root, str) or not Path(workspace_root).is_absolute()
        or ".." in Path(workspace_root).parts or str(Path(workspace_root)) != workspace_root
    ):
        raise ValueError(f"invalid retirement execution workspace root for {run_id}")
    target_dir, paths = _build_paths(binaries, cargo, root=Path(workspace_root))
    _require_collected_build(binaries, inventory)
    selection = execution["selection"]
    selected = _check_selection(selection, inventory)
    _require_roster_selection(root, release_revision, selected, selection["features"])
    _require_retirement_platforms(binaries, inventory, selection)
    expected_commands = _execution_commands(
        selection, target_dir.parent / binaries.path.name, target_dir.parent / cargo.path.name,
    )
    if execution["commands"] != expected_commands:
        raise ValueError(f"retained Nextest commands differ from the recorded selection for {run_id}")
    digest = executable_digest or retained_executable_digests()
    legacy = execution["legacy_searchd"]
    if legacy is not None and (not isinstance(legacy, dict) or set(legacy) != {"path", "sha256"}):
        raise ValueError(f"invalid retained old-Q executable identity for {run_id}")
    _require_approved_old_q(
        root, release_revision, selection["package"], selection["test_targets"],
        legacy["sha256"] if legacy is not None else None,
    )
    if legacy is not None:
        executed = _legacy_binary_path(legacy["path"], legacy["sha256"])
        if executed != target_dir / LEGACY_EXECUTABLE:
            raise ValueError(f"old-Q execution path differs from the retained executable for {run_id}")
        retained_legacy = folder / f"{run_id}-target" / LEGACY_EXECUTABLE
        if digest(retained_legacy) != legacy["sha256"]:
            raise ValueError("old-Q executable SHA-256 differs from the retirement execution pin")
    fingerprints = execution["executables"]
    if not isinstance(fingerprints, dict) or set(fingerprints) != set(paths) or any(
        not isinstance(digest, str) or not SHA256.fullmatch(digest) for digest in fingerprints.values()
    ):
        raise ValueError(f"retained Nextest executables differ from the built test targets for {run_id}")
    for name, path in paths.items():
        retained = folder / f"{run_id}-target" / path.relative_to(target_dir)
        if digest(retained) != fingerprints[name]:
            raise ValueError(f"retained Nextest executable bytes differ for {run_id}: {name}")


def _require_frozen_checkout(
    root: Path, revision: str, states: dict[str, tuple[int, ...]] | None = None,
) -> None:
    git = ["git", "--no-replace-objects"]
    current_head = subprocess.check_output([*git, "rev-parse", "--verify", "HEAD"], cwd=root, text=True).strip()
    if current_head != revision:
        raise ValueError("frozen source revision differs from retirement checkout HEAD")
    dirty = subprocess.check_output(
        [*git, "status", "--porcelain=v1", "-z", "--untracked-files=all"], cwd=root,
    )
    if dirty:
        raise ValueError("refusing dirty retirement checkout")
    tracked = subprocess.check_output([*git, "ls-files", "-v", "-z"], cwd=root)
    for entry in tracked.split(b"\0"):
        if entry and entry[:1] != b"H":
            raise ValueError(
                f"refusing hidden or unresolved tracked retirement source: {os.fsdecode(entry[2:])}"
            )
    if states is not None:
        require_source_files_unchanged(root, states)


def _capture_frozen_checkout(root: Path, revision: str) -> dict[str, tuple[int, ...]]:
    _require_frozen_checkout(root, revision)
    try:
        _, states = capture_committed_source(root, revision)
    except ClosureError as error:
        raise ValueError(str(error)) from error
    return states


def release_source_manifest(
    root: Path, revision: str, inventory_path: Path, events_path: Path,
) -> dict[str, str]:
    """Bind retained collection and terminal to the runner's frozen revision."""
    states = _capture_frozen_checkout(root, revision)
    return _source_manifest_for_execution(root, revision, inventory_path, events_path, states)


def _source_manifest_for_execution(
    root: Path, revision: str, inventory_path: Path, events_path: Path,
    states: dict[str, tuple[int, ...]],
) -> dict[str, str]:
    _require_frozen_checkout(root, revision, states)
    inventory = RawFile.capture(inventory_path)
    terminal = parse_nextest(events_path, parse_nextest_inventory(inventory))
    source_tree = release_tree_sha256(root, revision)
    _require_frozen_checkout(root, revision, states)
    return {
        "head": revision,
        "source_tree_sha256": source_tree,
        "inventory_sha256": inventory.sha256.removeprefix("sha256:"),
        "events_sha256": terminal.sha256,
    }


def _build_paths(binaries: RawFile, cargo: RawFile, *, root: Path) -> tuple[Path, dict[str, Path]]:
    build = parse_proof_json(binaries.read_control())
    metadata = parse_proof_json(cargo.read_control())
    if not isinstance(build, dict) or not isinstance(metadata, dict):
        raise ValueError("Nextest binary and Cargo metadata must be objects")
    build_meta = build.get("rust-build-meta")
    rows = build.get("rust-binaries")
    target = metadata.get("target_directory")
    if (
        not isinstance(build_meta, dict) or not isinstance(rows, dict) or not rows
        or not isinstance(target, str) or not Path(target).is_absolute()
        or ".." in Path(target).parts or build_meta.get("target-directory") != target
        or metadata.get("workspace_root") != str(root)
    ):
        raise ValueError("Nextest build metadata differs from the workspace or its Cargo target")
    paths = {}
    for name, row in rows.items():
        raw_path = row.get("binary-path") if isinstance(row, dict) else None
        if not isinstance(raw_path, str) or row.get("binary-id") != name:
            raise ValueError("Nextest build inventory lacks executable identity")
        path = Path(raw_path)
        if not path.is_absolute() or ".." in path.parts or not path.is_relative_to(target) or path == Path(target):
            raise ValueError("Nextest test executable is outside its Cargo target")
        paths[name] = path
    if len(set(paths.values())) != len(paths):
        raise ValueError("Nextest build inventory repeats an executable path")
    target_dir = Path(target)
    runtime_rows = build_meta.get("non-test-binaries", {})
    if not isinstance(runtime_rows, dict) or any(not isinstance(rows, list) for rows in runtime_rows.values()):
        raise ValueError("Nextest runtime artifact inventory is malformed")
    bound_paths = set(paths.values())
    for entries in runtime_rows.values():
        for entry in entries:
            raw_path = entry.get("path") if isinstance(entry, dict) else None
            if not isinstance(raw_path, str):
                raise ValueError("Nextest runtime artifact lacks a path")
            relative = Path(raw_path)
            if (
                relative.is_absolute() or not relative.parts or ".." in relative.parts
                or relative.as_posix() != raw_path
            ):
                raise ValueError("Nextest runtime artifact is outside its Cargo target or has a noncanonical path")
            path = target_dir / relative
            if path in bound_paths:
                continue
            name = f"runtime/{raw_path}"
            if name in paths:
                raise ValueError("Nextest runtime artifact repeats a test executable identity")
            paths[name] = path
            bound_paths.add(path)
    return target_dir, paths


def _require_collected_build(binaries: RawFile, inventory: RawFile) -> None:
    built = parse_proof_json(binaries.read_control())["rust-binaries"]
    collection = parse_proof_json(inventory.read_control())
    listed = collection.get("rust-suites") if isinstance(collection, dict) else None
    if not isinstance(listed, dict) or set(built) != set(listed) or any(
        not isinstance(listed[name], dict)
        or any(listed[name].get(key) != value for key, value in row.items())
        for name, row in built.items()
    ):
        raise ValueError("Nextest collection differs from the built executable inventory")


def _nextest_platforms(artifact: RawFile) -> tuple[str, str]:
    try:
        meta = parse_proof_json(artifact.read_control())["rust-build-meta"]
        platforms = meta["platforms"]
        host = platforms["host"]["platform"]["triple"]
        targets = platforms["targets"]
        if not isinstance(targets, list) or len(targets) > 1:
            raise ValueError("Nextest retirement requires one effective Cargo target")
        target = targets[0]["platform"]["triple"] if targets else host
        if not all(isinstance(triple, str) and triple for triple in (host, target)):
            raise ValueError("invalid Nextest host or target triple")
        if "target-platforms" in meta and (
            tuple(row["triple"] for row in meta["target-platforms"]) != (target,)
        ):
            raise ValueError("Nextest target snapshots disagree")
    except (KeyError, TypeError, ValueError) as error:
        raise ValueError("Nextest host and target platform metadata is absent or invalid") from error
    return host, target


def _require_retirement_platforms(binaries: RawFile, inventory: RawFile, selection: dict) -> None:
    host_and_target = _nextest_platforms(binaries)
    if _nextest_platforms(inventory) != host_and_target:
        raise ValueError("Nextest collection platform differs from the build platform")
    package, targets = selection["package"], selection["test_targets"]
    if _uses_old_q(package, targets):
        if host_and_target != (LINUX_OLD_Q_PLATFORM, LINUX_OLD_Q_PLATFORM):
            raise ValueError("retained old-Q coverage requires x86-64 Linux host and target")
    elif _requires_linux_boot_inventory(package, targets) and not all(
        "linux" in triple.split("-") for triple in host_and_target
    ):
        raise ValueError("Lexical inventory coverage requires a Linux host and target")


def _exact_events(values: object) -> set[str]:
    if (
        not isinstance(values, list) or not values
        or any(not isinstance(value, str) or not NEXTEST_EVENT.fullmatch(value) for value in values)
        or len(set(values)) != len(values)
    ):
        raise ValueError("expected Nextest events must be unique exact package::binary$test names")
    return set(values)


def _check_selection(selection: dict, inventory: RawFile) -> set[str]:
    fields = {
        "lane", "package", "include_lib", "test_targets", "filterset", "features",
    }
    if not isinstance(selection, dict) or set(selection) not in (fields, fields | {"expected_events"}):
        raise ValueError("invalid retained Nextest selection")
    if (
        not all(isinstance(selection[key], str) and selection[key] for key in ("lane", "package", "filterset"))
        or type(selection["include_lib"]) is not bool
        or any(
            not isinstance(selection[key], list)
            or not all(isinstance(value, str) and value for value in selection[key])
            for key in ("test_targets", "features")
        )
        or not (selection["include_lib"] or selection["test_targets"])
    ):
        raise ValueError("invalid retained Nextest selection")
    listed = parse_nextest_inventory(inventory)
    suites = listed.suite_counts
    test_targets = set(selection["test_targets"])
    if len(test_targets) != len(selection["test_targets"]):
        raise ValueError("recorded Nextest test targets are duplicated")
    for package, target, kind in suites:
        if package != selection["package"] or not (
            kind == "lib" and selection["include_lib"]
            or kind == "test" and target in test_targets
        ):
            raise ValueError("Nextest collection contains a target outside the recorded selection")
    if missing := test_targets - {target for _, target, kind in suites if kind == "test"}:
        raise ValueError(f"Nextest collection lacks recorded test targets: {sorted(missing)}")
    if selection["include_lib"] and not any(kind == "lib" for _, _, kind in suites):
        raise ValueError("Nextest collection lacks the recorded library target")
    selected = set(listed)
    if "expected_events" in selection:
        expected = _exact_events(selection["expected_events"])
        if selected != expected:
            raise ValueError(
                "Nextest collection differs from the expected event set: "
                f"missing={sorted(expected - selected)} extra={sorted(selected - expected)}"
            )
    return selected


def _execution_commands(selection: dict, binaries: Path, cargo: Path) -> dict[str, list[str]]:
    resolution = ["--locked", "--offline", "--no-default-features"]
    if selection["features"]:
        resolution.extend(["--features", ",".join(selection["features"])])
    wrapper = ["--lane", selection["lane"]]
    build = [*resolution, "-p", selection["package"]]
    if selection["include_lib"]:
        build.append("--lib")
    for target in selection["test_targets"]:
        build.extend(["--test", target])
    reuse = ["--binaries-metadata", str(binaries), "--cargo-metadata", str(cargo), "-E", selection["filterset"]]
    return {
        "binaries.json": [*wrapper, "nextest", "list", *build, "--list-type", "binaries-only", "--message-format", "json"],
        "cargo.json": [*wrapper, "metadata", "--format-version", "1", *resolution],
        "inventory.json": [*wrapper, "nextest", "list", *reuse, "--message-format", "json"],
        "nextest.jsonl": [
            *wrapper, "nextest", "run", *reuse, "--no-tests", "fail",
            "--message-format", "libtest-json-plus", "--message-format-version", "0.1",
        ],
    }


def _legacy_binary_path(path: str, digest: str) -> Path:
    if (
        not isinstance(path, str) or not isinstance(digest, str) or not SHA256.fullmatch(digest)
        or not Path(path).is_absolute() or ".." in Path(path).parts
    ):
        raise ValueError("old-Q retirement execution requires an absolute executable path and SHA-256")
    return Path(path)


def _read_legacy_input(environment: Mapping[str, str]) -> RawFile | None:
    if LEGACY_BINARY_ENV not in environment and LEGACY_SHA_ENV not in environment:
        return None
    original = Path(environment.get(LEGACY_BINARY_ENV, ""))
    if not original.is_absolute():
        raise ValueError("old-Q retirement execution requires an absolute executable path and SHA-256")
    path = str(original.resolve(strict=True))
    digest = environment.get(LEGACY_SHA_ENV, "")
    binary = RawFile.capture(_legacy_binary_path(path, digest))
    if binary.sha256 != f"sha256:{digest}":
        raise ValueError("old-Q executable SHA-256 differs from the retirement execution pin")
    return binary


def _require_capture_host(package: str, test_targets: list[str]) -> None:
    uses_old_q = _uses_old_q(package, test_targets)
    if uses_old_q or _requires_linux_boot_inventory(package, test_targets):
        if platform.system() != "Linux":
            raise ValueError("retirement capture requires a Linux host for the selected targets")
        if uses_old_q and platform.machine() != "x86_64":
            raise ValueError("retained old-Q execution requires an x86-64 Linux host")


def capture_release_execution(
    root: Path, revision: str, evidence_dir: Path, *, lane: str, package: str,
    include_lib: bool, test_targets: list[str], filterset: str, features: list[str],
    expected_events: list[str] | None = None,
    _require_evidence_directory: Callable[[], None] | None = None,
    _manifest_digests: dict[Path, str] | None = None,
) -> Path:
    """Build once, then publish collection and successful execution from the same test binaries."""
    if not include_lib and not test_targets:
        raise ValueError("retirement capture requires --lib or at least one --test")
    root = root.resolve(strict=True)
    evidence_dir = evidence_dir.resolve(strict=True)
    evidence_identity = evidence_dir.lstat()
    if evidence_dir.is_relative_to(root) or not stat.S_ISDIR(evidence_identity.st_mode):
        raise ValueError("retirement evidence must use an existing directory outside the checkout")
    resolved_features = [
        feature if "/" in feature else f"{package}/{feature}"
        for group in features
        for feature in re.split(r"[\s,]+", group)
        if feature
    ]
    selected_events = None
    if expected_events is not None:
        selected_events = _exact_events(expected_events)
        _require_roster_selection(root, revision, selected_events, resolved_features)
    _require_approved_old_q(root, revision, package, test_targets, os.environ.get(LEGACY_SHA_ENV))
    _require_capture_host(package, test_targets)
    source_states = _capture_frozen_checkout(root, revision)
    run_id = str(uuid4())
    target_dir = evidence_dir / f"{run_id}-target"
    environment = {
        **os.environ,
        "CARGO_TARGET_DIR": str(target_dir),
        # A proof target is built once. Collection and execution reuse its binaries.
        "CARGO_INCREMENTAL": "0",
        "QUANTA_INDEX_PRESERVE_CARGO_TARGET_DIR": "1",
        "QUANTA_INDEX_RESOURCE_ADMISSION": "1",
    }
    # Cargo's target belongs to this proof. The lock belongs to the host cache.
    environment.pop("QUANTA_INDEX_CACHE_ROOT", None)
    input_states: dict[Path, tuple[tuple[int, ...], ...]] = {}

    def require_capture_identity() -> None:
        try:
            current = evidence_dir.lstat()
            canonical = evidence_dir.resolve(strict=True)
        except OSError as error:
            raise ValueError("retirement evidence directory changed during capture") from error
        if not os.path.samestat(evidence_identity, current) or canonical != evidence_dir:
            raise ValueError("retirement evidence directory changed during capture")
        if _require_evidence_directory is not None:
            _require_evidence_directory()
        _require_frozen_checkout(root, revision, source_states)

    def pin(path: Path) -> RawFile:
        before = _retained_file_state(path)
        artifact = RawFile.capture(path)
        if _retained_file_state(path) != before:
            raise ValueError(f"Nextest input changed while hashing: {path}")
        input_states[artifact.path] = before
        return artifact

    def require_unchanged(artifacts: list[RawFile], failure: str) -> None:
        for artifact in artifacts:
            path = artifact.path
            expected = input_states[path]
            if (
                _retained_file_state(path) != expected
                or RawFile.capture(path) != artifact
                or _retained_file_state(path) != expected
            ):
                raise ValueError(f"{failure}: {path.name}")

    def capture(suffix: str, *command: str, env: dict[str, str] = environment) -> RawFile:
        require_capture_identity()
        path = evidence_dir / f"{run_id}-{suffix}"
        with path.open("xb") as output:
            subprocess.run([str(root / "scripts/cargow"), *command], cwd=root, env=env, stdout=output, check=True)
        require_capture_identity()
        return pin(path)

    selection = {
        "lane": lane, "package": package, "include_lib": include_lib, "test_targets": test_targets,
        "filterset": filterset, "features": resolved_features,
    }
    if selected_events is not None:
        selection["expected_events"] = sorted(selected_events)
    legacy_binary = None
    if _uses_old_q(package, test_targets):
        legacy_binary = _read_legacy_input(environment)
    else:
        environment.pop(LEGACY_BINARY_ENV, None)
        environment.pop(LEGACY_SHA_ENV, None)
    commands = _execution_commands(
        selection, evidence_dir / f"{run_id}-binaries.json", evidence_dir / f"{run_id}-cargo.json",
    )
    require_capture_identity()
    target_dir.mkdir(mode=0o700)
    legacy = None
    retained_legacy = None
    if legacy_binary is not None:
        retained_legacy = legacy_binary.copy_to(target_dir / LEGACY_EXECUTABLE)
        retained_legacy.path.chmod(0o700)
        input_states[retained_legacy.path] = _retained_file_state(retained_legacy.path)
        execution_path = str(retained_legacy.path)
        environment[LEGACY_BINARY_ENV] = execution_path
        legacy = {"path": execution_path, "sha256": retained_legacy.sha256.removeprefix("sha256:")}
    binaries = capture("binaries.json", *commands["binaries.json"])
    cargo = capture("cargo.json", *commands["cargo.json"])
    observed_target, paths = _build_paths(binaries, cargo, root=root)
    if observed_target != target_dir:
        raise ValueError("Nextest build did not use the isolated execution target")
    pinned = {name: pin(path) for name, path in paths.items()}
    inputs = [binaries, cargo, *pinned.values()]
    if retained_legacy is not None:
        inputs.append(retained_legacy)
    inventory = capture("inventory.json", *commands["inventory.json"])
    _require_collected_build(binaries, inventory)
    selected = _check_selection(selection, inventory)
    _require_retirement_platforms(binaries, inventory, selection)
    if selected_events is None:
        _require_roster_selection(root, revision, selected, resolved_features)
    require_unchanged(inputs, "Nextest build inputs changed during collection")
    events = capture(
        "nextest.jsonl", *commands["nextest.jsonl"],
        env={**environment, "NEXTEST_EXPERIMENTAL_LIBTEST_JSON": "1"},
    )
    require_unchanged([*inputs, inventory, events], "Nextest build inputs or evidence changed during execution")
    source = _source_manifest_for_execution(root, revision, inventory.path, events.path, source_states)
    if source["inventory_sha256"] != inventory.sha256.removeprefix("sha256:") or source["events_sha256"] != events.sha256.removeprefix("sha256:"):
        raise ValueError("Nextest results changed before manifest publication")
    manifest = {
        **source,
        "execution": {
            "workspace_root": str(root),
            "binaries_sha256": binaries.sha256,
            "cargo_sha256": cargo.sha256,
            "executables": {name: blob.sha256.removeprefix("sha256:") for name, blob in pinned.items()},
            "selection": selection,
            "commands": commands,
            "legacy_searchd": legacy,
        },
    }
    path = evidence_dir / f"{run_id}-source-manifest.json"
    manifest_text = json.dumps(manifest, sort_keys=True) + "\n"
    with tempfile.NamedTemporaryFile(mode="w", encoding="utf-8", dir=evidence_dir) as output:
        output.write(manifest_text)
        output.flush()
        os.fsync(output.fileno())
        directory_fd = os.open(evidence_dir, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        try:
            require_capture_identity()
            if not os.path.samestat(evidence_identity, os.fstat(directory_fd)):
                raise ValueError("retirement evidence directory changed during capture")
            temporary_name = Path(output.name).name
            os.link(
                temporary_name, path.name,
                src_dir_fd=directory_fd, dst_dir_fd=directory_fd, follow_symlinks=False,
            )
            linked = os.stat(path.name, dir_fd=directory_fd, follow_symlinks=False)
            owns_link = os.path.samestat(linked, os.fstat(output.fileno()))
            try:
                if not owns_link:
                    # A replaced temporary name can itself have been linked.
                    # A destination with neither inode belongs to another writer.
                    with suppress(FileNotFoundError):
                        named_temporary = os.stat(
                            temporary_name, dir_fd=directory_fd, follow_symlinks=False,
                        )
                        owns_link = os.path.samestat(linked, named_temporary)
                    raise ValueError("published retirement manifest differs from the prepared file")
                require_capture_identity()
                current = os.stat(path.name, dir_fd=directory_fd, follow_symlinks=False)
                if not os.path.samestat(linked, current):
                    raise ValueError("retirement manifest changed during publication")
            except BaseException:
                # The run did not qualify. Remove only the name linked by this
                # capture, even if its evidence directory has been moved.
                if owns_link:
                    with suppress(FileNotFoundError):
                        current = os.stat(path.name, dir_fd=directory_fd, follow_symlinks=False)
                        if os.path.samestat(linked, current):
                            os.unlink(path.name, dir_fd=directory_fd)
                raise
        finally:
            os.close(directory_fd)
    if _manifest_digests is not None:
        digest = hashlib.sha256(manifest_text.encode("utf-8")).hexdigest()
        _manifest_digests[path] = f"sha256:{digest}"
    return path


def _reusable_captures(
    root: Path, revision: str, evidence_dir: Path, selections: list[dict],
    _manifest_digests: dict[Path, str] | None = None,
) -> dict[tuple[str, ...], Path]:
    tree = release_tree_sha256(root, revision)
    retained: dict[tuple[str, ...], Path] = {}
    digest = retained_executable_digests()
    for path in sorted(evidence_dir.glob("*-source-manifest.json")):
        run_id = path.name.removesuffix("-source-manifest.json")
        if not NEXTEST_RUN_ID.fullmatch(run_id):
            continue
        artifact = RawFile.capture(path)
        manifest = parse_proof_json(artifact.read_control())
        if not isinstance(manifest, dict) or manifest.get("source_tree_sha256") != tree:
            continue
        execution = manifest.get("execution")
        selection = execution.get("selection") if isinstance(execution, dict) else None
        if not isinstance(selection, dict) or selection not in selections:
            continue
        key = tuple(selection["expected_events"])
        if key in retained:
            continue
        observation = {
            **{field: manifest[field] for field in ("inventory_sha256", "events_sha256")},
            "run_id": run_id,
            "source_manifest_sha256": artifact.sha256.removeprefix("sha256:"),
        }
        check_frozen_inputs(artifact, observation, root, revision, executable_digest=digest)
        inventory = RawFile.capture(evidence_dir / f"{run_id}-inventory.json")
        if inventory.sha256 != f"sha256:{observation['inventory_sha256']}":
            raise ValueError(f"Nextest inventory SHA-256 differs for {run_id}")
        terminal = parse_nextest(evidence_dir / f"{run_id}-nextest.jsonl", parse_nextest_inventory(inventory))
        if terminal.sha256 != observation["events_sha256"] or set(terminal.passed_names) != set(key):
            raise ValueError(f"Nextest terminal {run_id} differs from the planned passing batch")
        retained[key] = path
        if _manifest_digests is not None:
            _manifest_digests[path] = artifact.sha256
    return retained


def capture_release_roster(
    root: Path, revision: str, evidence_dir: Path, *, lane: str,
    _manifest_digests: dict[Path, str] | None = None,
) -> Iterator[Path]:
    """Capture every committed retirement event serially, reusing verified batches."""
    root = root.resolve(strict=True)
    evidence_dir = evidence_dir.resolve(strict=True)
    if evidence_dir.is_relative_to(root) or not evidence_dir.is_dir():
        raise ValueError("retirement evidence must use an existing directory outside the checkout")
    directory_fd = os.open(evidence_dir, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        try:
            fcntl.flock(directory_fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as error:
            raise ValueError("a retirement roster capture already holds the evidence directory") from error
        locked = os.fstat(directory_fd)

        def require_evidence_directory() -> None:
            try:
                current = evidence_dir.lstat()
                canonical = evidence_dir.resolve(strict=True)
            except OSError as error:
                raise ValueError("retirement evidence directory changed during roster capture") from error
            if (
                not stat.S_ISDIR(current.st_mode)
                or (current.st_dev, current.st_ino) != (locked.st_dev, locked.st_ino)
                or canonical != evidence_dir
            ):
                raise ValueError("retirement evidence directory changed during roster capture")

        require_evidence_directory()
        _require_frozen_checkout(root, revision)
        selections = [{**batch, "lane": lane} for batch in release_capture_batches(root, revision)]
        manifest_digests = _manifest_digests if _manifest_digests is not None else {}
        retained = _reusable_captures(root, revision, evidence_dir, selections, manifest_digests)
        require_evidence_directory()
        pending = [selection for selection in selections if tuple(selection["expected_events"]) not in retained]
        for selection in pending:
            package, targets = selection["package"], selection["test_targets"]
            _require_approved_old_q(root, revision, package, targets, os.environ.get(LEGACY_SHA_ENV))
            _require_capture_host(package, targets)
        if any(_uses_old_q(selection["package"], selection["test_targets"]) for selection in pending):
            _read_legacy_input(os.environ)
        for selection in selections:
            require_evidence_directory()
            key = tuple(selection["expected_events"])
            if key in retained:
                manifest = retained[key]
            else:
                manifest = capture_release_execution(
                    root, revision, evidence_dir,
                    _require_evidence_directory=require_evidence_directory,
                    _manifest_digests=manifest_digests, **selection,
                )
            require_evidence_directory()
            original_digest = manifest_digests.get(manifest)
            if original_digest is None or RawFile.capture(manifest).sha256 != original_digest:
                raise ValueError("retirement manifest changed after validated capture")
            yield manifest
            require_evidence_directory()
    finally:
        os.close(directory_fd)


def check_migration_observations(
    observations: object,
    read: Callable[[Path], str],
    *,
    root: Path,
    source_tests: Callable[[Path, str], list[str]],
    inventory_only: bool,
    evidence_dir: Path | None,
    release_revision: str | None = None,
    event_matches: Callable[[Path, str, str], bool] | None = None,
    executable_digest: Callable[[Path], str] | None = None,
) -> set[tuple[str, str]]:
    """Bind the current test source and independently check its terminal execution."""
    if not isinstance(observations, list) or not observations:
        raise ValueError("VERIFIED disposition requires at least one Nextest body observation")
    source_fields = {"source", "case", "source_sha256", "run_id"}
    event_fields = source_fields | {"event", "events_sha256"}
    evidence_fields = event_fields | {"inventory_sha256"}
    frozen_fields = evidence_fields | {"source_manifest_sha256"}
    observed: set[tuple[str, str]] = set()
    for item in observations:
        if not isinstance(item, dict) or set(item) not in (
            source_fields, event_fields, evidence_fields, frozen_fields,
        ):
            raise ValueError("observation requires source, case, source_sha256 and run_id with optional terminal binding")
        source, case, digest, run_id = (item[key] for key in ("source", "case", "source_sha256", "run_id"))
        if not isinstance(source, str) or not RUST_PROOF_SOURCE.fullmatch(source):
            raise ValueError(f"invalid observation source: {source!r}")
        if not isinstance(case, str) or not case.isascii() or not case.isidentifier():
            raise ValueError(f"invalid observation case: {case!r}")
        if not isinstance(digest, str) or not SHA256.fullmatch(digest):
            raise ValueError(f"invalid observation source SHA-256: {digest!r}")
        if not isinstance(run_id, str) or not NEXTEST_RUN_ID.fullmatch(run_id):
            raise ValueError(f"invalid retirement evidence run ID: {run_id!r}")

        path = root / source
        body = read(path)
        if hashlib.sha256(body.encode("utf-8")).hexdigest() != digest:
            raise ValueError(f"observation source changed since {run_id}: {source}")
        if case not in source_tests(path, body):
            raise ValueError(f"observation case {case!r} is absent from {source}")

        observed.add((source, case))
        if set(item) == source_fields:
            if inventory_only:
                continue
            raise ValueError(f"observation lacks an exact Nextest event and evidence digests: {source}::{case}")
        event = item["event"]
        if not isinstance(event, str) or not NEXTEST_EVENT.fullmatch(event):
            raise ValueError(f"invalid exact Nextest event: {event!r}")
        binary, name = event.split("$")
        parts = Path(source).parts
        _, package, tree, *_ = parts
        direct_target = tree == "tests" and len(parts) == 4
        if (
            binary.split("::")[0] != package
            or (event_matches is None and direct_target and binary != f"{package}::{path.stem}")
            or (event_matches is not None and not event_matches(path, binary, name))
            or name.rsplit("::", 1)[-1] != case
        ):
            raise ValueError(f"Nextest event {event!r} differs from {source}::{case}")
        for key in ("events_sha256", "inventory_sha256", "source_manifest_sha256"):
            if key not in item:
                continue
            value = item[key]
            if not isinstance(value, str) or not SHA256.fullmatch(value):
                raise ValueError(f"invalid Nextest {key}: {value!r}")
        if inventory_only:
            continue
        if "inventory_sha256" not in item:
            raise ValueError(f"observation lacks the Nextest inventory SHA-256: {source}::{case}")
        if release_revision is not None and "source_manifest_sha256" not in item:
            raise ValueError(f"observation lacks the frozen source manifest SHA-256: {source}::{case}")
        if evidence_dir is None:
            raise ValueError("set QUANTA_RETIREMENT_NEXTEST_EVIDENCE_DIR to the captured Nextest evidence directory")

        if release_revision is not None:
            check_frozen_inputs(
                RawFile.capture(evidence_dir / f"{run_id}-source-manifest.json"),
                item, root, release_revision,
                executable_digest=executable_digest,
            )

        inventory = RawFile.capture(evidence_dir / f"{run_id}-inventory.json")
        if inventory.sha256 != f"sha256:{item['inventory_sha256']}":
            raise ValueError(f"Nextest inventory SHA-256 differs for {run_id}")
        terminal = parse_nextest(
            evidence_dir / f"{run_id}-nextest.jsonl", parse_nextest_inventory(inventory)
        )
        if terminal.sha256 != item["events_sha256"]:
            raise ValueError(f"Nextest terminal SHA-256 differs for {run_id}")
        if event not in terminal.passed_names:
            raise ValueError(f"Nextest terminal {run_id} has no PASS for {event}")
    return observed


if __name__ == "__main__":
    import argparse

    parser = argparse.ArgumentParser(description=capture_release_execution.__doc__)
    parser.add_argument("--revision", required=True)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--list-roster", action="store_true", help="Print the committed exact event roster without running Cargo")
    mode.add_argument("--capture-roster", action="store_true", help="Capture every committed retirement event serially")
    parser.add_argument("--evidence-dir", type=Path)
    parser.add_argument("--lane")
    parser.add_argument("--package")
    parser.add_argument("--lib", action="store_true")
    parser.add_argument("--test", dest="test_targets", action="append", default=[])
    parser.add_argument("--filterset")
    parser.add_argument("--expect-event", action="append", help="Require this exact selected package::binary$test event")
    parser.add_argument("--features", action="append", default=[])
    args = parser.parse_args()
    try:
        if args.list_roster:
            roster = release_event_roster(Path.cwd(), args.revision)
            print(json.dumps({"revision": args.revision, "events": roster}, sort_keys=True, indent=2))
            raise SystemExit(0)
        if args.capture_roster:
            if not args.evidence_dir or not args.lane:
                parser.error("--capture-roster requires --evidence-dir and --lane")
            if any((args.package, args.lib, args.test_targets, args.filterset, args.expect_event, args.features)):
                parser.error("--capture-roster uses the committed selections and cannot take manual selectors")
            manifest_digests: dict[Path, str] = {}
            for manifest in capture_release_roster(
                Path.cwd(), args.revision, args.evidence_dir, lane=args.lane,
                _manifest_digests=manifest_digests,
            ):
                print(f"{manifest} {manifest_digests[manifest]}", flush=True)
            raise SystemExit(0)
        if not all((args.evidence_dir, args.lane, args.package, args.filterset)):
            parser.error("execution requires --evidence-dir, --lane, --package and --filterset")
        manifest_digests = {}
        manifest = capture_release_execution(
            Path.cwd(), args.revision, args.evidence_dir, lane=args.lane,
            package=args.package, include_lib=args.lib, test_targets=args.test_targets,
            filterset=args.filterset, features=args.features, expected_events=args.expect_event,
            _manifest_digests=manifest_digests,
        )
    except (OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"Retirement execution evidence unavailable: {error}\n")
    print(f"{manifest} {manifest_digests[manifest]}")
