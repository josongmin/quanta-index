#!/usr/bin/env python3
"""Run one or more declared local test scopes in one cargo-nextest process."""

from __future__ import annotations

import argparse
import json
import os
import re
import secrets
import shlex
import subprocess
import sys
import tempfile
from contextlib import ExitStack, nullcontext
from pathlib import Path
from typing import Any

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - Python < 3.11
    import tomli as tomllib  # type: ignore[no-redef]


ROOT = Path(__file__).resolve().parents[2]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))
from tools.benchmark.evidence import RawFile, write_raw_file  # noqa: E402
from tools.ci.binary_custody import pin as pin_daemon  # noqa: E402
from tools.ci.binary_custody import verify as verify_daemon  # noqa: E402
from tools.ci.daemon_execution_evidence import (  # noqa: E402
    DAEMON_PATH_ENV,
    NATIVE_RELEASE_SCOPES,
    P04_PROOF,
    P04_SCOPE,
    P07_PROOF,
    P07_PROVIDER_PIN_ENV,
    P07_PROVIDER_PIN_NAME,
    P07_PROVIDER_SHA_ENV,
    P10_PROOF,
    WITNESS_DIR_ENV,
    WITNESS_NONCE_ENV,
    daemon_witness_specs,
    live_proof,
    p07_provider_approval_digest,
    read_p07_provider_approval,
    read_witness,
    require_daemon_test_inventory,
    validate_witness,
)
from tools.ci.proof_json import parse_proof_json  # noqa: E402
from tools.ci.proof_run_source import (  # noqa: E402
    BOUND_OWNER_SCOPES,
    FrozenRunCheckout,
    ProofRunSource,
    capture_run_source,
    frozen_run_checkout,
    require_run_source,
    write_run_binding,
)
from tools.ci.rust_test_mount import mounted_test_module  # noqa: E402

CATALOG = ROOT / "tools" / "ci" / "test-authority.toml"
TEST_THREADS_OVERRIDE = "QUANTA_INDEX_TEST_THREADS"
PROOF_RAW_DIR = "QUANTA_PROOF_RAW_DIR"


def load_catalog(path: Path = CATALOG) -> dict[str, Any]:
    return tomllib.loads(path.read_text(encoding="utf-8"))


def resolve_targets(
    data: dict[str, Any], scope_names: list[str], *, root: Path | None = None,
) -> tuple[str, int, bool, list[str], list[dict[str, str]]]:
    root = ROOT if root is None else root
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
    selected: list[tuple[str, bool]] = []
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
        lane = scope.get("lane")
        if not isinstance(lane, str) or not lane:
            raise ValueError(f"local scope {scope_name} has no lane")
        test_threads = scope.get("test_threads")
        if not isinstance(test_threads, int) or isinstance(test_threads, bool) or test_threads < 1:
            raise ValueError(f"local scope {scope_name} has invalid test_threads")
        thread_limits.append(test_threads)
        includes = scope.get("includes", [])
        if not isinstance(includes, list):
            raise ValueError(f"local scope {scope_name}.includes is not a list")
        for included in includes:
            if not isinstance(included, str) or not included:
                raise ValueError(f"local scope {scope_name}.includes has an invalid name")
            expand_scope(included, (*trail, scope_name))
        for field in ("targets", "module_targets"):
            raw_ids = scope.get(field, [])
            if not isinstance(raw_ids, list):
                raise ValueError(f"local scope {scope_name}.{field} is not a list")
            selected.extend((target_id, field == "module_targets") for target_id in raw_ids)
        if "owners" in scope:
            owners = scope["owners"]
            if not isinstance(owners, list):
                raise ValueError(f"local scope {scope_name}.owners is not a list")
            selected.extend(
                (target_id, False) for target_id, entry in by_id.items() if entry["owner"] in owners
            )
        packages = scope.get("packages", [])
        if not isinstance(packages, list):
            raise ValueError(f"local scope {scope_name}.packages is not a list")
        for package in packages:
            if not isinstance(package, str) or not package:
                raise ValueError(f"local scope {scope_name}.packages has an invalid name")
            manifest = root / "crates" / package / "Cargo.toml"
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
        expand_scope(scope_name, ())

    resolved: list[dict[str, str]] = []
    selections: dict[str, bool] = {}
    for target_id, module_only in selected:
        if not isinstance(target_id, str) or target_id not in by_id:
            raise ValueError(f"local scope references unknown integration target: {target_id!r}")
        selections[target_id] = selections.get(target_id, True) and module_only
    for target_id, module_only in selections.items():
        entry = by_id[target_id]
        path = root / entry["path"]
        if not path.is_file():
            raise ValueError(f"local scope target is missing: {entry['path']}")
        cargo_target = entry.get("target", path.stem)
        if not isinstance(cargo_target, str) or not cargo_target:
            raise ValueError(f"local scope target has an invalid Cargo target: {target_id}")
        row = {"id": target_id, "owner": entry["owner"], "path": entry["path"], "target": cargo_target}
        if module_only:
            row["module"] = mounted_test_module(
                root, path, entry["owner"], cargo_target, list(by_id.values())
            )
        resolved.append(row)
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
        pair = (entry["owner"], entry.get("target", path.stem))
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


def selected_by_binary(targets: list[dict[str, str]]) -> dict[tuple[str, str], set[str | None]]:
    by_binary: dict[tuple[str, str], set[str | None]] = {}
    for target in targets:
        key = (target["owner"], target["target"])
        by_binary.setdefault(key, set()).add(target.get("module"))
    return by_binary


def selected_module_filter(
    targets: list[dict[str, str]], *, exact_cases: tuple[str, ...] = ()
) -> str | None:
    by_binary = selected_by_binary(targets)
    by_case: dict[tuple[str, str], list[str]] = {}
    for event in exact_cases:
        identity, _, case = event.partition("$")
        owner, _, binary = identity.partition("::")
        if (owner, binary) not in by_binary or not re.fullmatch(
            r"[A-Za-z_][A-Za-z0-9_]*(?:::[A-Za-z_][A-Za-z0-9_]*)*", case
        ):
            raise ValueError(f"invalid exact nextest case for selected targets: {event}")
        by_case.setdefault((owner, binary), []).append(case)
    if not by_case and all(None in modules for modules in by_binary.values()):
        return None
    clauses = []
    for (owner, binary), modules in by_binary.items():
        if not all(re.fullmatch(r"[A-Za-z0-9_-]+", value) for value in (owner, binary)):
            raise ValueError(f"invalid scoped nextest package or binary: {owner}:{binary}")
        identity = f"package(={owner}) & binary(={binary})"
        if cases := by_case.get((owner, binary)):
            tests = " | ".join(f"test(={case})" for case in cases)
            identity += f" & ({tests})"
        elif None not in modules:
            tests = " | ".join(f"test(/^{module}::/)" for module in sorted(modules))
            identity += f" & ({tests})"
        clauses.append(f"({identity})")
    return " | ".join(clauses)


def build_command(
    lane: str,
    test_threads: int,
    include_lib: bool,
    extra_packages: list[str],
    targets: list[dict[str, str]],
    run_ignored: str = "default",
    *,
    exact_cases: tuple[str, ...] = (),
    exact_lib_cases: dict[str, list[str]] | None = None,
    native_only: bool = False,
    root: Path | None = None,
) -> list[str]:
    if run_ignored not in {"default", "all"}:
        raise ValueError("run_ignored must be default or all")
    packages = list(dict.fromkeys([*(target["owner"] for target in targets), *extra_packages]))
    test_names = list(dict.fromkeys(target["target"] for target in targets))
    command = [
        str((ROOT if root is None else root) / "scripts" / "cargow"),
        "--lane",
        lane,
        "nextest",
        "run",
    ]
    for package in packages:
        command.extend(["-p", package])
    if include_lib:
        command.append("--lib")
    if run_ignored == "all":
        command.extend(["--run-ignored", "all"])
    for test_name in test_names:
        command.extend(["--test", test_name])
    module_filter = selected_module_filter(targets, exact_cases=exact_cases)
    if exact_lib_cases is not None:
        if not include_lib or targets or set(packages) != set(exact_lib_cases):
            raise ValueError("exact library cases require only their registered library packages")
        if not exact_lib_cases or any(
            not re.fullmatch(r"[a-z0-9][a-z0-9-]*", package)
            or not cases
            or any(
                not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*(?:::[A-Za-z_][A-Za-z0-9_]*)*", case)
                for case in cases
            )
            for package, cases in exact_lib_cases.items()
        ):
            raise ValueError("exact library selection requires cases")
        module_filter = " | ".join(
            f"(package(={package}) & ("
            + " | ".join(f"test(={case})" for case in cases)
            + "))"
            for package, cases in exact_lib_cases.items()
        )
    if module_filter:
        command.extend(["-E", module_filter])
    command.extend(
        [
            "--no-default-features" if native_only else "--all-features",
            "--locked",
            "--test-threads",
            str(test_threads),
            "--success-output",
            "never",
            "--failure-output",
            "immediate-final",
            # Keep leaked-output owners visible without printing successful tests.
            "--status-level",
            "leak",
            "--final-status-level",
            "fail",
            "--no-tests",
            "fail",
        ]
    )
    return command


def build_inventory_command(run_command: list[str]) -> list[str]:
    """Use the same package and target selectors for the collection oracle."""
    nextest_index = run_command.index("nextest")
    if run_command[nextest_index + 1] != "run":
        raise ValueError("expected nextest run command")
    runtime_index = run_command.index("--test-threads")
    return [
        *run_command[: nextest_index + 1],
        "list",
        *run_command[nextest_index + 2 : runtime_index],
        "--message-format",
        "json",
    ]


def _release_daemon_path(label: str) -> Path:
    path = Path(os.environ.get(DAEMON_PATH_ENV, ""))
    if not (path.is_absolute() and path.is_file() and os.access(path, os.X_OK)):
        raise ValueError(f"{label} requires an absolute executable release daemon in {DAEMON_PATH_ENV}")
    return path


def run_with_proof_evidence(
    command: list[str], *, scopes: list[str], lane: str, raw_dir: Path,
    targets: list[dict[str, str]],
    witness_specs: dict[str, tuple[str, int]] | None = None,
    source_binding: tuple[dict[str, Any], ProofRunSource] | None = None,
    checkout: FrozenRunCheckout | None = None,
    exact_lib_cases: dict[str, list[str]] | None = None,
) -> int:
    """Capture complete machine evidence when a proof owner explicitly opts in."""
    env = os.environ.copy()
    execution_root = ROOT if checkout is None else checkout.root
    if execution_root != ROOT:
        # A private source projection keeps using the caller's existing Cargo lane.
        # Its detached Git admin directory cannot inventory the caller's worktrees.
        env["QUANTA_INDEX_CACHE_CHECKOUT_ROOT"] = str(ROOT)
        env["QUANTA_INDEX_TARGET_GC"] = "0"
    if checkout is not None:
        env["PYTHONPATH"] = str(execution_root)
        env["PYTHONDONTWRITEBYTECODE"] = "1"
    if (checkout is None) != (source_binding is None):
        raise ValueError("release evidence requires both its source record and frozen checkout")
    release_proof = NATIVE_RELEASE_SCOPES.get(scopes[0]) if len(scopes) == 1 else None
    p10_release = release_proof == P10_PROOF
    if release_proof is not None and (
        source_binding is None or source_binding[1].proof_id != release_proof or checkout is None
    ):
        raise ValueError("release evidence requires source capture before selection")
    provider_approval = None
    provider_approval_sha256 = None
    if release_proof == P07_PROOF:
        authority, _captured = source_binding
        provider_approval = read_p07_provider_approval(authority, env)
        provider_approval_sha256 = p07_provider_approval_digest(authority)
    witness_specs = witness_specs or {}
    witness_names = set(witness_specs)
    release_binary = None
    proof = None
    if release_proof is not None or P04_SCOPE in scopes:
        proof = (release_proof or P04_PROOF).partition("-")[0].upper()
        release_binary = _release_daemon_path(proof)
        if not witness_names:
            raise ValueError(f"{proof} requires registered daemon observations")
    try:
        from tools.ci.nextest_events import (
            NextestEvidenceError,
            parse_nextest,
            parse_nextest_inventory,
        )
    except ModuleNotFoundError:
        sys.path.insert(0, str(ROOT))
        from tools.ci.nextest_events import (
            NextestEvidenceError,
            parse_nextest,
            parse_nextest_inventory,
        )

    label = "-".join((*scopes, lane))
    if not label or any(
        character not in "abcdefghijklmnopqrstuvwxyz0123456789-_" for character in label
    ):
        raise ValueError("proof scope/lane label is not filesystem-safe")
    raw_dir.mkdir(parents=True, exist_ok=True)
    inventory = raw_dir / f"{label}-inventory.json"
    events = raw_dir / f"{label}-nextest.jsonl"
    observations_path = raw_dir / f"{label}-daemon-observations.json"
    binding_path = raw_dir / f"{label}-run-binding.json"
    list_stderr = raw_dir / f"{label}-list.stderr.log"
    run_stderr = raw_dir / f"{label}-run.stderr.log"
    paths = (inventory, events, list_stderr, run_stderr, observations_path, binding_path)
    if any(os.path.lexists(path) for path in paths):
        raise ValueError(f"proof evidence already exists for {label}; choose a fresh output root")
    list_command = build_inventory_command(command)
    run_command = [
        *command,
        "--message-format",
        "libtest-json-plus",
        "--message-format-version",
        "0.1",
    ]
    env.pop(WITNESS_DIR_ENV, None)
    env.pop(WITNESS_NONCE_ENV, None)
    env.pop(P07_PROVIDER_PIN_ENV, None)
    env.pop(P07_PROVIDER_SHA_ENV, None)
    witness_directory = (
        tempfile.TemporaryDirectory(prefix="quanta-daemon-observations-")
        if witness_names
        else nullcontext()
    )
    with witness_directory as witness_dir:
        pinned_digest = None
        if release_binary is not None:
            pinned_binary = Path(witness_dir).resolve(strict=True) / "quanta-index-searchd"
            # P04 accepts a launch alias. Fix its initial bytes before collection.
            source_binary = release_binary if p10_release else release_binary.resolve(strict=True)
            pinned_digest = pin_daemon(source_binary, source_binary, pinned_binary)
            env[DAEMON_PATH_ENV] = str(pinned_binary)
        if checkout is not None and source_binding is not None:
            authority, captured = source_binding
            source_exclusions = (*paths, release_binary) if release_binary is not None else paths
            require_run_source(
                ROOT, authority, captured, phase="test selection", outputs=source_exclusions,
            )
            checkout.require_unchanged("test selection")
        with inventory.open("xb") as output, list_stderr.open("xb") as errors:
            listed = subprocess.run(
                list_command, cwd=execution_root, env=env, stdout=output, stderr=errors,
            )
        if listed.returncode != 0:
            return listed.returncode
        inventory_capture = RawFile.capture(inventory)
        expected = parse_nextest_inventory(inventory_capture)
        for (owner, binary), modules in selected_by_binary(targets).items():
            if None in modules:
                continue
            for module in sorted(modules):
                prefix = f"{owner}::{binary}${module}::"
                if not any(
                    name.startswith(prefix) and identity[:2] == (owner, binary)
                    for name, identity in expected.items()
                ):
                    raise ValueError(
                        f"nextest inventory selected no tests in {owner}:{binary}:{module}"
                    )
        require_daemon_test_inventory(expected, witness_names)
        if release_proof is not None and set(expected) != witness_names:
            raise ValueError(f"{proof} requires only the registered daemon witness tests in nextest inventory")
        if exact_lib_cases is not None:
            selected = {
                (package, name.partition("$")[2])
                for name, (package, _binary, kind) in expected.items()
                if kind == "lib"
            }
            required = {
                (package, case)
                for package, cases in exact_lib_cases.items()
                for case in cases
            }
            if selected != required or len(expected) != len(required):
                raise ValueError("Native Catalog owner inventory differs from exact cases")
        if checkout is not None and source_binding is not None:
            checkout.require_unchanged("nextest collection")
            require_run_source(
                ROOT, authority, captured, phase="nextest collection", outputs=source_exclusions,
            )

        env["NEXTEST_EXPERIMENTAL_LIBTEST_JSON"] = "1"
        if witness_dir is not None:
            nonce = secrets.token_hex(16)
            env[WITNESS_DIR_ENV] = witness_dir
            env[WITNESS_NONCE_ENV] = nonce
            if provider_approval is not None and provider_approval_sha256 is not None:
                provider_pin = Path(witness_dir).resolve(strict=True) / P07_PROVIDER_PIN_NAME
                write_raw_file(provider_pin, [provider_approval])
                env[P07_PROVIDER_PIN_ENV] = str(provider_pin)
                env[P07_PROVIDER_SHA_ENV] = provider_approval_sha256
        with events.open("xb") as output, run_stderr.open("xb") as errors:
            ran = subprocess.run(
                run_command, cwd=execution_root, env=env, stdout=output, stderr=errors,
            )
        if ran.returncode != 0:
            return ran.returncode
        if checkout is not None and source_binding is not None:
            checkout.require_unchanged("nextest execution")
            require_run_source(
                ROOT, authority, captured, phase="nextest execution", outputs=source_exclusions,
            )
        if pinned_digest is not None:
            current_binary = release_binary if p10_release else release_binary.resolve(strict=True)
            verify_daemon(pinned_digest, [current_binary, pinned_binary])
        events_capture = RawFile.capture(events)
        try:
            parsed = parse_nextest(events_capture, expected)
        except NextestEvidenceError as error:
            raise ValueError(f"nextest machine result is not authoritative: {error}") from error
        if witness_dir is not None:
            witnesses = {}
            for name in sorted(witness_names):
                filename, launches = witness_specs[name]
                row = parse_proof_json(read_witness(Path(witness_dir), filename))
                observed = validate_witness(
                    row, test=name, nonce=nonce, launches=launches,
                    provider_approval_sha256=provider_approval_sha256,
                )
                if pinned_digest is not None and observed != pinned_digest:
                    raise ValueError(f"{proof} executed daemon differs from the pinned release binary")
                witnesses[name] = row
            if any(RawFile.capture(raw.path) != raw for raw in (inventory_capture, events_capture)):
                raise ValueError("Nextest evidence changed after validation")
            with observations_path.open("x", encoding="utf-8") as output:
                json.dump(
                    {
                        "nonce": nonce,
                        "events_sha256": parsed.sha256,
                        "inventory_sha256": inventory_capture.sha256.removeprefix("sha256:"),
                        "observations": witnesses,
                    },
                    output,
                    sort_keys=True,
                )
            if source_binding is not None:
                write_run_binding(
                    binding_path, captured, format="nextest-jsonl",
                    digests={
                        "events": parsed.sha256,
                        "inventory": inventory_capture.sha256.removeprefix("sha256:"),
                        "daemon_observations": RawFile.capture(observations_path).sha256.removeprefix(
                            "sha256:"
                        ),
                    },
                    daemon_sha256=pinned_digest,
                )
                print(f"run_binding={binding_path}", flush=True)
            print(f"daemon_observations={observations_path}", flush=True)
        elif source_binding is not None:
            if any(RawFile.capture(raw.path) != raw for raw in (inventory_capture, events_capture)):
                raise ValueError("Nextest evidence changed after validation")
            write_run_binding(
                binding_path, captured, format="nextest-jsonl",
                digests={
                    "events": parsed.sha256,
                    "inventory": inventory_capture.sha256.removeprefix("sha256:"),
                },
            )
            print(f"run_binding={binding_path}", flush=True)
    print(
        f"proof runner evidence: scopes={','.join(scopes)}; selected={parsed.selected}; "
        f"passed={parsed.passed}; inventory={inventory}; events={events}",
        flush=True,
    )
    return 0


def effective_test_threads(declared: int, override: str | None) -> int:
    """Allow an explicit local reduction without exceeding catalog authority."""
    if override is None:
        return declared
    if not override.isascii() or not override.isdecimal():
        raise ValueError(f"{TEST_THREADS_OVERRIDE} must be a positive decimal integer")
    requested = int(override)
    if not 1 <= requested <= declared:
        raise ValueError(
            f"{TEST_THREADS_OVERRIDE} must be between 1 and the declared cap {declared}"
        )
    return requested


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("scopes", nargs="+", help="local scope names from test-authority.toml")
    parser.add_argument("--lane", help="override the declared lane (for composite validation)")
    parser.add_argument(
        "--run-ignored",
        choices=("default", "all"),
        default="default",
        help="include ignored tests when the owner proof supplies their required inputs",
    )
    parser.add_argument("--dry-run", action="store_true", help="print the exact command only")
    args = parser.parse_args(argv)
    try:
        with ExitStack() as resources:
            return _run_requested_scopes(args, resources)
    except (OSError, ValueError) as error:
        print(f"local test scope error: {error}", file=sys.stderr)
        return 2


def _run_requested_scopes(args: argparse.Namespace, resources: ExitStack) -> int:
    raw_dir = os.environ.get(PROOF_RAW_DIR)
    release_proof = NATIVE_RELEASE_SCOPES.get(args.scopes[0]) if len(args.scopes) == 1 else None
    owner_proof = BOUND_OWNER_SCOPES.get(args.scopes[0]) if len(args.scopes) == 1 else None
    source_binding = None
    checkout = None
    if release_proof is None and any(
        scope != P04_SCOPE and scope in NATIVE_RELEASE_SCOPES for scope in args.scopes
    ):
        raise ValueError("release observations require their own exact test scope")
    if owner_proof is None and any(scope in BOUND_OWNER_SCOPES for scope in args.scopes):
        raise ValueError("Native Catalog owner proof requires its own exact test scope")
    proof_id = release_proof or owner_proof or (P04_PROOF if P04_SCOPE in args.scopes else None)
    proof = live_proof(ROOT, proof_id) if proof_id is not None else None
    if owner_proof is not None and proof.get("authority_state") != "executable":
        raise ValueError(f"{owner_proof} is not executable")
    bound_proof = release_proof or owner_proof
    if release_proof is not None and not args.dry_run and release_proof != P04_PROOF and not raw_dir:
        raise ValueError(f"{release_proof} requires {PROOF_RAW_DIR}")
    if proof is not None and bound_proof is not None and raw_dir and not args.dry_run:
        outputs = (_release_daemon_path(proof["id"]),) if release_proof is not None else ()
        source = capture_run_source(ROOT, proof, outputs=outputs)
        checkout = resources.enter_context(
            frozen_run_checkout(ROOT, proof, source, outputs=outputs),
        )
        if live_proof(checkout.root, proof["id"]) != proof:
            raise ValueError("proof registry changed during source capture")
        source_binding = (proof, source)
    execution_root = ROOT if checkout is None else checkout.root
    catalog = load_catalog() if checkout is None else load_catalog(
        execution_root / "tools" / "ci" / "test-authority.toml",
    )
    default_lane, test_threads, include_lib, extra_packages, targets = resolve_targets(
        catalog, args.scopes, root=execution_root,
    )
    witnesses = daemon_witness_specs(proof) if proof is not None else {}
    native_lib_scope = owner_proof is not None and (
        args.scopes[0] != "p02b-operation-journal"
    )
    exact_lib_cases = proof.get("required_nextest_lib_cases") if native_lib_scope else None
    if native_lib_scope and not isinstance(exact_lib_cases, dict):
        raise ValueError(f"{owner_proof} has no exact Native Catalog cases")
    test_threads = effective_test_threads(test_threads, os.environ.get(TEST_THREADS_OVERRIDE))
    command = build_command(
        args.lane or default_lane,
        test_threads,
        include_lib,
        extra_packages,
        targets,
        args.run_ignored,
        exact_cases=tuple(witnesses) if release_proof is not None else (),
        exact_lib_cases=exact_lib_cases,
        native_only=release_proof is not None,
        root=execution_root,
    )

    cargo_test_binaries = len({(target["owner"], target["target"]) for target in targets})
    print(
        f"local test scopes: {','.join(args.scopes)}; "
        f"catalog_rows={len(targets)}; cargo_test_binaries={cargo_test_binaries}; "
        f"cargo_processes=1; test_threads={test_threads}; "
        f"lane={args.lane or default_lane}",
        flush=True,
    )
    if args.dry_run:
        print(shlex.join(command))
        return 0
    if raw_dir:
        if args.run_ignored != "default":
            print("proof runner evidence for --run-ignored all is not supported", file=sys.stderr)
            return 2
        try:
            return run_with_proof_evidence(
                command,
                scopes=args.scopes,
                lane=args.lane or default_lane,
                raw_dir=Path(raw_dir).expanduser(),
                targets=targets,
                witness_specs=witnesses,
                source_binding=source_binding,
                checkout=checkout,
                exact_lib_cases=exact_lib_cases,
            )
        except (OSError, ValueError) as error:
            print(f"proof runner evidence refused: {error}", file=sys.stderr)
            return 2
    os.chdir(ROOT)
    os.execv(command[0], command)
    return 127  # pragma: no cover - os.execv does not return


if __name__ == "__main__":
    raise SystemExit(main())
