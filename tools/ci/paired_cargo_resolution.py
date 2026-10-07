"""Reject cross-repository proofs resolving Quanta crates from another checkout.

Input is Cargo metadata with a resolve graph, not declared dependency paths.
This guard is execution preflight; its JSON is not a qualification receipt.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import secrets
import stat
import subprocess
import sys
from pathlib import Path
from typing import Any

try:
    import tomllib
except ModuleNotFoundError:  # Python 3.10 compatibility, matching proof tooling.
    import tomli as tomllib


REQUIRED_QUANTA_PACKAGES = frozenset(
    {"quanta-index-contract", "quanta-index-ipc", "quanta-index-sdk"}
)
QBC_STATUS_MAX_BYTES = 4 * 1024 * 1024
QBC_METADATA_MAX_BYTES = 64 * 1024 * 1024


def _raw_exit_acceptable(value: Any) -> bool:
    # Auxiliary metadata omits raw exit when it equals the effective status.
    # Absence cannot qualify a test; this path only consumes Cargo resolver JSON.
    return value is None or (type(value) is int and value == 0)


def _frozen_head(root: Path) -> str:
    head = subprocess.check_output(["git", "-C", str(root), "rev-parse", "HEAD"], text=True).strip()
    dirty = subprocess.check_output(
        ["git", "-C", str(root), "status", "--porcelain=v1", "--untracked-files=all"],
        text=True,
    )
    if dirty or len(head) != 40:
        raise ValueError(f"QBC resolver preflight requires a clean source: {root}")
    return head


def _bound_inputs(quanta_root: Path, paired_root: Path, consumer: str) -> dict[str, str]:
    paths = {
        "quanta_workspace": quanta_root / "Cargo.toml",
        "paired_workspace": paired_root / "packages/analysis/quanta-v2/Cargo.toml",
        "paired_lock": paired_root / "packages/analysis/quanta-v2/Cargo.lock",
        "consumer": paired_root / f"packages/analysis/quanta-v2/crates/{consumer}/Cargo.toml",
    }
    for name in REQUIRED_QUANTA_PACKAGES:
        paths[name] = quanta_root / "crates" / name / "Cargo.toml"
    return {name: _digest(path.resolve(strict=True)) for name, path in paths.items()}


def _read_bounded_regular(path: Path) -> tuple[bytes, tuple[int, ...]]:
    if path.resolve(strict=True) != path:
        raise ValueError("QBC metadata output path contains an alias")
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
    with os.fdopen(descriptor, "rb") as handle:
        before = os.fstat(handle.fileno())
        if (
            not stat.S_ISREG(before.st_mode)
            or before.st_nlink != 1
            or before.st_size <= 0
            or before.st_size > QBC_METADATA_MAX_BYTES
        ):
            raise ValueError("QBC metadata output is not a bounded regular file")
        content = handle.read(QBC_METADATA_MAX_BYTES + 1)
        after = os.fstat(handle.fileno())

    def identity(value: os.stat_result) -> tuple[int, ...]:
        return (value.st_dev, value.st_ino, value.st_size, value.st_mtime_ns, value.st_ctime_ns)

    if identity(before) != identity(after) or len(content) != before.st_size:
        raise ValueError("QBC metadata output changed during read")
    return content, identity(after)


def _status(
    source: Path, lane: str, environment: dict[str, str]
) -> tuple[dict[str, Any], dict[str, Any]]:
    result = subprocess.run(
        [
            str(source / "scripts/quanta-build-cli"),
            "status",
            "--lane",
            lane,
            "--observational",
            "--json",
        ],
        cwd=source,
        env=environment,
        capture_output=True,
        check=False,
    )
    if result.returncode != 0:
        raise ValueError(f"QBC observational status failed: exit {result.returncode}")
    if not result.stdout or len(result.stdout) > QBC_STATUS_MAX_BYTES:
        raise ValueError("QBC observational status has no bounded JSON output")
    try:
        payload = json.loads(result.stdout, object_pairs_hook=_unique_json)
    except (ValueError, UnicodeDecodeError) as error:
        raise ValueError(f"QBC observational status parse failed: {error}") from error
    if (
        not isinstance(payload, dict)
        or payload.get("observational_v1") is not True
        or payload.get("execution_root") != str(source)
        or not isinstance(payload.get("lanes"), list)
        or len(payload["lanes"]) != 1
    ):
        raise ValueError("QBC observational status belongs to another source or lane")
    item = payload["lanes"][0]
    if (
        not isinstance(item, dict)
        or item.get("lane") != lane
        or item.get("execution_root") != str(source)
    ):
        raise ValueError("QBC observational status lane identity changed")
    if item.get("registered") is not True:
        raise ValueError(f"QBC lane registration missing: {lane}")
    return payload, item


def resolve_from_qbc(
    *, quanta_root: Path, paired_root: Path, consumer: str, feature: str, lane: str
) -> dict[str, Any]:
    """Consume this auxiliary QBC run's stdout as resolver preflight only.

    This does not turn metadata into Rust qualification or override QBC's
    effective exit code. Metadata has no immutable verification-result receipt;
    the own lane's mutable output must remain stable across both status reads.
    """
    quanta_root = quanta_root.resolve(strict=True)
    paired_root = paired_root.resolve(strict=True)
    if not lane or "/" in lane or lane in {".", ".."}:
        raise ValueError("QBC resolver lane is invalid")
    roots = {root: _frozen_head(root) for root in (quanta_root, paired_root)}
    inputs = _bound_inputs(quanta_root, paired_root, consumer)
    environment = os.environ.copy()
    environment["CODEGRAPH_PERSONA"] = "agent"
    before, prior = _status(paired_root, lane, environment)
    prior_id = (prior.get("receipt") or {}).get("last_run_id_v1")
    command = [
        "cargo",
        "metadata",
        "--locked",
        "--format-version",
        "1",
        "--no-default-features",
        "--manifest-path",
        f"packages/analysis/quanta-v2/crates/{consumer}/Cargo.toml",
        "--features",
        feature,
    ]
    nonce = secrets.token_hex(16)
    result = subprocess.run(
        [
            str(paired_root / "scripts/quanta-build-cli"),
            "cargo",
            "--lane",
            lane,
            "--meta",
            f"paired_r5_resolution_nonce={nonce}",
            "--",
            *command[1:],
        ],
        cwd=paired_root,
        env=environment,
        capture_output=True,
        check=False,
    )
    if result.returncode != 0:
        raise ValueError(f"QBC cargo metadata refused: effective exit {result.returncode}")
    after, current = _status(paired_root, lane, environment)
    receipt = current.get("receipt")
    meta = current.get("meta")
    if not isinstance(receipt, dict) or not isinstance(meta, dict):
        raise ValueError("QBC metadata run has no observed receipt/meta")
    run_id = receipt.get("last_run_id_v1")
    if (
        not isinstance(run_id, str)
        or not run_id
        or run_id == prior_id
        or receipt.get("run_state") != "finished"
        or receipt.get("command") != command
        or receipt.get("command_cwd_v1") != str(paired_root)
        or type(receipt.get("last_exit_code")) is not int
        or receipt["last_exit_code"] != 0
        or not _raw_exit_acceptable(receipt.get("command_exit_code_v1"))
        or (meta.get("user_meta") or {}).get("paired_r5_resolution_nonce") != nonce
        or (meta.get("command_context") or {}).get("last_run_id_v1") != run_id
    ):
        raise ValueError("QBC metadata run is stale, failed, or belongs to another invocation")
    if before.get("state_root") != after.get("state_root") or before.get(
        "execution_root_key"
    ) != after.get("execution_root_key"):
        raise ValueError("QBC metadata state namespace changed")
    state_root = Path(str(after["state_root"])).resolve(strict=True)
    key = after["execution_root_key"]
    lane_key = current.get("lane_key")
    if any(
        not isinstance(token, str) or not token or token in {".", ".."} or "/" in token
        for token in (key, lane_key, run_id)
    ):
        raise ValueError("QBC metadata run locator is invalid")
    stdout_path = (
        state_root
        / "execution-roots"
        / key
        / "lanes"
        / lane_key
        / "last-completed-run"
        / "stdout.log"
    )
    if receipt.get("last_stdout_path_v1") != str(stdout_path):
        raise ValueError("QBC metadata stdout path differs from own lane")
    try:
        output, identity = _read_bounded_regular(stdout_path)
    except OSError as error:
        raise ValueError(f"QBC metadata output is unavailable: {error}") from error
    try:
        metadata = json.loads(output, object_pairs_hook=_unique_json)
    except (ValueError, UnicodeDecodeError) as error:
        raise ValueError(f"Cargo metadata output parse failed: {error}") from error
    validated = validate_resolution(
        metadata, quanta_root=quanta_root, paired_root=paired_root, consumer=consumer
    )
    try:
        repeated_output, repeated_identity = _read_bounded_regular(stdout_path)
    except OSError as error:
        raise ValueError(f"QBC metadata output disappeared: {error}") from error
    final, latest = _status(paired_root, lane, environment)
    latest_receipt = latest.get("receipt") or {}
    latest_meta = latest.get("meta") or {}
    if (
        output != repeated_output
        or identity != repeated_identity
        or final.get("state_root") != after.get("state_root")
        or final.get("execution_root_key") != after.get("execution_root_key")
        or latest.get("lane_key") != lane_key
        or any(
            latest_receipt.get(field) != receipt.get(field)
            for field in (
                "last_run_id_v1",
                "run_state",
                "command",
                "command_cwd_v1",
                "last_exit_code",
                "command_exit_code_v1",
                "last_stdout_path_v1",
            )
        )
        or (latest_meta.get("user_meta") or {}).get("paired_r5_resolution_nonce") != nonce
        or (latest_meta.get("command_context") or {}).get("last_run_id_v1") != run_id
    ):
        raise ValueError("QBC metadata owner or output changed during resolver preflight")
    if any(_frozen_head(root) != head for root, head in roots.items()):
        raise ValueError("source HEAD changed during QBC resolver preflight")
    if _bound_inputs(quanta_root, paired_root, consumer) != inputs:
        raise ValueError("resolver manifests or nested lock changed during QBC preflight")
    return validated


def _unique_json(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def _digest(path: Path) -> str:
    return "sha256:" + hashlib.sha256(path.read_bytes()).hexdigest()


def _path(value: Any, label: str) -> Path:
    if not isinstance(value, str) or not value or not Path(value).is_absolute():
        raise ValueError(f"{label} must be an absolute path")
    return Path(value).resolve(strict=True)


def validate_resolution(
    metadata: Any, *, quanta_root: Path, paired_root: Path, consumer: str
) -> dict[str, Any]:
    """Return stable relative identities only after validating actual resolution."""

    quanta_root = quanta_root.resolve(strict=True)
    paired_root = paired_root.resolve(strict=True)
    if (
        not isinstance(metadata, dict)
        or type(metadata.get("version")) is not int
        or metadata["version"] != 1
    ):
        raise ValueError("Cargo metadata format version 1 is required")
    workspace = _path(metadata.get("workspace_root"), "workspace root")
    expected_workspace = paired_root / "packages/analysis/quanta-v2"
    if not workspace.is_relative_to(paired_root):
        raise ValueError("Cargo workspace escapes the paired checkout")
    if workspace != expected_workspace.resolve(strict=True):
        raise ValueError("Cargo resolver belongs to another paired workspace")
    lock = workspace / "Cargo.lock"
    lock = lock.resolve(strict=True)
    if lock.parent != workspace:
        raise ValueError("Cargo resolver lock escapes its workspace")

    packages_raw = metadata.get("packages")
    resolve = metadata.get("resolve")
    if not isinstance(packages_raw, list) or not packages_raw:
        raise ValueError("Cargo package inventory is missing")
    if not isinstance(resolve, dict) or not isinstance(resolve.get("nodes"), list):
        raise ValueError("Cargo resolve graph is required (do not use --no-deps)")
    packages: dict[str, dict[str, Any]] = {}
    for package in packages_raw:
        if not isinstance(package, dict) or not isinstance(package.get("id"), str):
            raise ValueError("malformed Cargo package")
        identity = package["id"]
        if identity in packages:
            raise ValueError("duplicate Cargo package identity")
        packages[identity] = package
    nodes: dict[str, dict[str, Any]] = {}
    for node in resolve["nodes"]:
        if not isinstance(node, dict) or node.get("id") not in packages:
            raise ValueError("unknown Cargo resolve node")
        if node["id"] in nodes:
            raise ValueError("duplicate Cargo resolve node")
        dependencies = node.get("dependencies")
        if (
            not isinstance(dependencies, list)
            or any(
                not isinstance(identity, str) or identity not in packages
                for identity in dependencies
            )
            or len(set(dependencies)) != len(dependencies)
        ):
            raise ValueError("malformed Cargo dependency edge")
        nodes[node["id"]] = node
    roots = [identity for identity, package in packages.items() if package.get("name") == consumer]
    if len(roots) != 1:
        raise ValueError("selected consumer must resolve exactly once")
    consumer_manifest = _path(packages[roots[0]].get("manifest_path"), "consumer manifest")
    if not consumer_manifest.is_relative_to(workspace):
        raise ValueError("selected consumer escapes the paired workspace")
    expected_consumer = workspace / "crates" / consumer / "Cargo.toml"
    if consumer_manifest != expected_consumer.resolve(strict=True):
        raise ValueError("selected consumer comes from another checkout")
    reachable: set[str] = set()
    pending = roots.copy()
    while pending:
        identity = pending.pop()
        if identity in reachable:
            continue
        if identity not in nodes:
            raise ValueError("reachable Cargo dependency lacks a resolve node")
        reachable.add(identity)
        pending.extend(nodes[identity]["dependencies"])
    required_feature = {
        "quanta-runtime": "index-sdk-ingress",
        "quanta-runtime-retrieval-kernel": "index-sdk-ingress-surface",
    }.get(consumer)
    consumer_features = nodes[roots[0]].get("features")
    if (
        required_feature is None
        or not isinstance(consumer_features, list)
        or any(not isinstance(feature, str) for feature in consumer_features)
        or len(set(consumer_features)) != len(consumer_features)
        or required_feature not in consumer_features
    ):
        raise ValueError("selected consumer feature profile is not enabled")

    # Derive the expected manifest owners from this workspace, not from a
    # second hard-coded package-to-path registry or a sibling-directory guess.
    workspace_manifest = quanta_root / "Cargo.toml"
    workspace_config = tomllib.loads(workspace_manifest.read_text())
    expected: dict[str, Path] = {}
    for member in workspace_config["workspace"]["members"]:
        manifest = (quanta_root / member / "Cargo.toml").resolve(strict=True)
        if not manifest.is_relative_to(quanta_root):
            raise ValueError("Quanta workspace member escapes the expected checkout")
        config = tomllib.loads(manifest.read_text())
        name = config["package"]["name"]
        if name in expected:
            raise ValueError("duplicate expected Quanta workspace package")
        expected[name] = manifest
    resolved_packages: list[dict[str, Any]] = []
    seen_names: set[str] = set()
    for identity in sorted(reachable):
        package = packages[identity]
        name = package.get("name")
        if not isinstance(name, str):
            raise ValueError("Cargo package name is missing")
        if not name.startswith("quanta-index-"):
            continue
        if name in seen_names or name not in expected:
            raise ValueError(f"ambiguous or unknown resolved Quanta package: {name}")
        seen_names.add(name)
        manifest = _path(package.get("manifest_path"), "Quanta dependency manifest")
        if "source" not in package or package["source"] is not None or manifest != expected[name]:
            raise ValueError(f"resolved Quanta dependency belongs to another source: {name}")
        if not isinstance(package.get("version"), str) or not package["version"]:
            raise ValueError("Cargo package version is missing")
        features = nodes[identity].get("features")
        if (
            not isinstance(features, list)
            or any(not isinstance(feature, str) for feature in features)
            or len(set(features)) != len(features)
        ):
            raise ValueError("malformed resolved Quanta features")
        resolved_packages.append(
            {
                "name": name,
                "version": package["version"],
                "manifest": manifest.relative_to(quanta_root).as_posix(),
                "manifest_sha256": _digest(manifest),
                "features": sorted(features),
            }
        )
    if not REQUIRED_QUANTA_PACKAGES.issubset(seen_names):
        raise ValueError(
            "selected feature profile does not resolve all required Quanta dependencies"
        )
    return {
        "version": 1,
        "consumer": consumer,
        "consumer_features": sorted(consumer_features),
        "workspace": workspace.relative_to(paired_root).as_posix(),
        "dependency_lock": {
            "path": lock.relative_to(paired_root).as_posix(),
            "sha256": _digest(lock),
        },
        "quanta_workspace_manifest_sha256": _digest(workspace_manifest),
        "packages": sorted(resolved_packages, key=lambda item: item["name"]),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--quanta-root", type=Path, required=True)
    parser.add_argument("--paired-root", type=Path, required=True)
    parser.add_argument(
        "--consumer", choices=["quanta-runtime", "quanta-runtime-retrieval-kernel"], required=True
    )
    parser.add_argument(
        "--qbc-lane", help="registered QBC lane for source-bound metadata preflight"
    )
    parser.add_argument("--feature", help="exact selected Cargo feature profile")
    args = parser.parse_args()
    try:
        if args.qbc_lane:
            if not args.feature:
                raise ValueError("QBC resolver requires an explicit selected feature profile")
            result = resolve_from_qbc(
                quanta_root=args.quanta_root,
                paired_root=args.paired_root,
                consumer=args.consumer,
                feature=args.feature,
                lane=args.qbc_lane,
            )
        else:
            metadata = json.load(sys.stdin, object_pairs_hook=_unique_json)
            result = validate_resolution(
                metadata,
                quanta_root=args.quanta_root,
                paired_root=args.paired_root,
                consumer=args.consumer,
            )
    except (ValueError, OSError, KeyError, TypeError) as error:
        print(f"paired Cargo dependency resolution refused: {error}", file=sys.stderr)
        return 1
    print(json.dumps(result, sort_keys=True, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
