#!/usr/bin/env python3
"""Run one or more declared local test scopes in one cargo-nextest process."""

from __future__ import annotations

import argparse
import os
import shlex
import sys
from pathlib import Path
from typing import Any

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - Python < 3.11
    import tomli as tomllib  # type: ignore[no-redef]


ROOT = Path(__file__).resolve().parents[2]
CATALOG = ROOT / "tools" / "ci" / "test-authority.toml"


def load_catalog(path: Path = CATALOG) -> dict[str, Any]:
    return tomllib.loads(path.read_text(encoding="utf-8"))


def resolve_targets(
    data: dict[str, Any], scope_names: list[str]
) -> tuple[str, int, bool, list[str], list[dict[str, str]]]:
    scopes = data.get("local_scopes")
    entries = data.get("integration_targets")
    if not isinstance(scopes, dict) or not isinstance(entries, list):
        raise ValueError("test authority lacks local_scopes or integration_targets")

    by_id = {
        entry["id"]: entry
        for entry in entries
        if isinstance(entry, dict)
        and isinstance(entry.get("id"), str)
        and isinstance(entry.get("path"), str)
        and isinstance(entry.get("owner"), str)
    }
    selected_ids: list[str] = []
    extra_packages: list[str] = []
    default_lanes: list[str] = []
    thread_limits: list[int] = []
    include_lib = False

    def expand_scope(scope_name: str, trail: tuple[str, ...]) -> None:
        nonlocal include_lib
        if scope_name in trail:
            cycle = " -> ".join((*trail, scope_name))
            raise ValueError(f"local scope include cycle: {cycle}")
        scope = scopes.get(scope_name)
        if not isinstance(scope, dict):
            raise ValueError(f"unknown local test scope: {scope_name}")
        includes = scope.get("includes", [])
        if not isinstance(includes, list):
            raise ValueError(f"local scope {scope_name}.includes is not a list")
        for included in includes:
            if not isinstance(included, str) or not included:
                raise ValueError(f"local scope {scope_name}.includes has an invalid name")
            expand_scope(included, (*trail, scope_name))
        if "targets" in scope:
            raw_ids = scope["targets"]
            if not isinstance(raw_ids, list):
                raise ValueError(f"local scope {scope_name}.targets is not a list")
            selected_ids.extend(raw_ids)
        if "owners" in scope:
            owners = scope["owners"]
            if not isinstance(owners, list):
                raise ValueError(f"local scope {scope_name}.owners is not a list")
            selected_ids.extend(
                target_id for target_id, entry in by_id.items() if entry["owner"] in owners
            )
        packages = scope.get("packages", [])
        if not isinstance(packages, list):
            raise ValueError(f"local scope {scope_name}.packages is not a list")
        for package in packages:
            if not isinstance(package, str) or not package:
                raise ValueError(f"local scope {scope_name}.packages has an invalid name")
            manifest = ROOT / "crates" / package / "Cargo.toml"
            if not manifest.is_file():
                raise ValueError(f"local scope {scope_name} references unknown package {package}")
            extra_packages.append(package)
        lib = scope.get("lib", False)
        if not isinstance(lib, bool):
            raise ValueError(f"local scope {scope_name}.lib is not boolean")
        include_lib = include_lib or lib

    for scope_name in scope_names:
        scope = scopes.get(scope_name)
        if not isinstance(scope, dict):
            raise ValueError(f"unknown local test scope: {scope_name}")
        lane = scope.get("lane")
        if not isinstance(lane, str) or not lane:
            raise ValueError(f"local scope {scope_name} has no lane")
        default_lanes.append(lane)
        test_threads = scope.get("test_threads")
        if not isinstance(test_threads, int) or isinstance(test_threads, bool) or test_threads < 1:
            raise ValueError(f"local scope {scope_name} has invalid test_threads")
        thread_limits.append(test_threads)
        expand_scope(scope_name, ())

    resolved: list[dict[str, str]] = []
    seen: set[str] = set()
    for target_id in selected_ids:
        if not isinstance(target_id, str) or target_id not in by_id:
            raise ValueError(f"local scope references unknown integration target: {target_id!r}")
        if target_id in seen:
            continue
        seen.add(target_id)
        entry = by_id[target_id]
        path = ROOT / entry["path"]
        if not path.is_file():
            raise ValueError(f"local scope target is missing: {entry['path']}")
        cargo_target = entry.get("target", path.stem)
        if not isinstance(cargo_target, str) or not cargo_target:
            raise ValueError(f"local scope target has an invalid Cargo target: {target_id}")
        resolved.append(
            {
                "id": target_id,
                "owner": entry["owner"],
                "path": entry["path"],
                "target": cargo_target,
            }
        )
    if not resolved and not (include_lib and extra_packages):
        raise ValueError("local test scope selected no test targets")
    if include_lib and resolved:
        raise ValueError("library and integration selectors must use separate nextest processes")

    # Cargo combines -p and --test selectors. Refuse a declaration that would
    # accidentally run an unselected same-name target from another package.
    selected_pairs = {(entry["owner"], entry["target"]) for entry in resolved}
    packages = {entry["owner"] for entry in resolved} | set(extra_packages)
    target_names = {entry["target"] for entry in resolved}
    for entry in by_id.values():
        path = Path(entry["path"])
        pair = (entry["owner"], path.stem)
        if pair[0] in packages and pair[1] in target_names and pair not in selected_pairs:
            raise ValueError(
                f"scope selectors would run an undeclared package/target pair: {pair[0]}::{pair[1]}"
            )

    default_lane = default_lanes[0]
    if len(set(default_lanes)) != 1 and len(scope_names) > 1:
        default_lane = "local-validation-lane"
    return (
        default_lane,
        min(thread_limits),
        include_lib,
        list(dict.fromkeys(extra_packages)),
        resolved,
    )


def build_command(
    lane: str,
    test_threads: int,
    include_lib: bool,
    extra_packages: list[str],
    targets: list[dict[str, str]],
) -> list[str]:
    packages = list(dict.fromkeys([*(target["owner"] for target in targets), *extra_packages]))
    test_names = list(dict.fromkeys(target["target"] for target in targets))
    command = [
        str(ROOT / "scripts" / "cargow"),
        "--lane",
        lane,
        "nextest",
        "run",
    ]
    for package in packages:
        command.extend(["-p", package])
    if include_lib:
        command.append("--lib")
    for test_name in test_names:
        command.extend(["--test", test_name])
    command.extend(
        [
            "--all-features",
            "--locked",
            "--test-threads",
            str(test_threads),
            "--success-output",
            "never",
            "--failure-output",
            "immediate-final",
            "--status-level",
            "fail",
            "--final-status-level",
            "fail",
        ]
    )
    return command


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("scopes", nargs="+", help="local scope names from test-authority.toml")
    parser.add_argument("--lane", help="override the declared lane (for composite validation)")
    parser.add_argument("--dry-run", action="store_true", help="print the exact command only")
    args = parser.parse_args(argv)
    try:
        default_lane, test_threads, include_lib, extra_packages, targets = resolve_targets(
            load_catalog(), args.scopes
        )
        command = build_command(
            args.lane or default_lane,
            test_threads,
            include_lib,
            extra_packages,
            targets,
        )
    except (OSError, tomllib.TOMLDecodeError, ValueError) as error:
        print(f"local test scope error: {error}", file=sys.stderr)
        return 2

    print(
        f"local test scopes: {','.join(args.scopes)}; "
        f"targets={len(targets)}; cargo_processes=1; test_threads={test_threads}; "
        f"lane={args.lane or default_lane}"
    )
    if args.dry_run:
        print(shlex.join(command))
        return 0
    os.chdir(ROOT)
    os.execv(command[0], command)
    return 127  # pragma: no cover - os.execv does not return


if __name__ == "__main__":
    raise SystemExit(main())
