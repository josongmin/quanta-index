"""Install a pinned registry-grammar candidate bundle for the benchmark CLI.

The caller supplies a digest from its release channel. A digest measured from
the same untrusted bundle does not establish release provenance.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import re
import shutil
import stat
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))
from tools.benchmark.retrieval.projection_process import (  # noqa: E402
    ProjectionProcessError,
    run_helper,
)

SCRIPT = ROOT / "tools/benchmark/retrieval/ts_projection.cjs"
SCHEMA = "quanta-symbol-candidate-bundle-v1"
OUTPUT_SCHEMA = "quanta-symbol-candidate-artifacts-v1"
MAX_MANIFEST = 1 << 20
MAX_ARTIFACT = 256 << 20
MAX_PROBE_OUTPUT = 8 << 20
PROBE_TIMEOUT_SECONDS = 75
ARTIFACTS = ("official_binary", "syntax_batch_binary", "go_helper", "node", "typescript_js")
PROBES = (
    ("go", "probe.go", "package p\nfunc One() { _ = new(1) }\n", "One"),
    (
        "syntax",
        "probe.js",
        "\ufeff// 한😀\nfunction local() {} export { local as const };\n",
        "local",
    ),
    ("syntax", "probe.ts", "export type * from './other';\nfunction One() {}\n", "One"),
    (
        "syntax",
        "probe.tsx",
        "export type * from './other';\nfunction One() { return <div/> }\n",
        "One",
    ),
)
MALFORMED_PROBE = {"path": "malformed.ts", "source": "function broken( {"}


def _sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _unique(pairs: list[tuple[str, object]]) -> dict[str, object]:
    result = {}
    for name, value in pairs:
        if name in result:
            raise ValueError(f"duplicate manifest key: {name}")
        result[name] = value
    return result


def _regular_bytes(path: Path | str, limit: int, *, dir_fd: int | None = None) -> bytes:
    # O_NONBLOCK prevents a replaced FIFO from blocking open before fstat can
    # reject it. Read from the same descriptor that was checked.
    fd = os.open(path, os.O_RDONLY | os.O_NONBLOCK | os.O_NOFOLLOW, dir_fd=dir_fd)
    try:
        metadata = os.fstat(fd)
        if not stat.S_ISREG(metadata.st_mode) or metadata.st_size > limit:
            raise ValueError(f"artifact is not a bounded regular file: {path}")
        data = bytearray()
        while len(data) <= limit:
            chunk = os.read(fd, min(1 << 20, limit + 1 - len(data)))
            if not chunk:
                break
            data.extend(chunk)
        if len(data) > limit:
            raise ValueError(f"artifact exceeds size limit: {path}")
        return bytes(data)
    finally:
        os.close(fd)


def _bundle_bytes(bundle: Path, relative: str, limit: int) -> bytes:
    # Each directory is opened relative to the already opened parent. A
    # symlink or directory rename cannot redirect a later component lookup.
    fd = os.open(bundle, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        parts = relative.split("/")
        for part in parts[:-1]:
            child = os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=fd)
            os.close(fd)
            fd = child
        return _regular_bytes(parts[-1], limit, dir_fd=fd)
    finally:
        os.close(fd)


def _artifact(bundle: Path, item: object, name: str) -> tuple[str, bytes]:
    if not isinstance(item, dict) or set(item) != {"path", "sha256"}:
        raise ValueError(f"invalid {name} artifact")
    relative, expected = item["path"], item["sha256"]
    if (
        not isinstance(relative, str)
        or not relative
        or Path(relative).is_absolute()
        or any(part in ("", ".", "..") for part in relative.split("/"))
        or "\\" in relative
        or not isinstance(expected, str)
        or not re.fullmatch(r"[0-9a-f]{64}", expected)
    ):
        raise ValueError(f"invalid {name} artifact path or digest")
    try:
        data = _bundle_bytes(bundle, relative, MAX_ARTIFACT)
    except OSError as error:
        raise ValueError(f"linked or unavailable artifact path: {name}") from error
    if _sha(data) != expected:
        raise ValueError(f"artifact digest differs: {name}")
    return Path(relative).name, data


def _check_dynamic_dependencies(path: Path, supplied_libraries: set[str]) -> None:
    """Reject absolute non-system linkage that would escape the staged bundle."""
    if platform.system() == "Linux":
        completed = subprocess.run(
            ["ldd", str(path)], capture_output=True, text=True, check=False, timeout=10
        )
        if completed.returncode:
            if "not a dynamic executable" in completed.stdout + completed.stderr:
                return
            raise ValueError(f"Linux dependency inspection failed: {path}")
        for row in completed.stdout.splitlines():
            if "not found" in row:
                raise ValueError(f"unstaged Linux dynamic dependency: {row.strip()}")
            resolved = row.split("=>", 1)[-1].strip().split(" ", 1)[0]
            if resolved.startswith("/") and not resolved.startswith(
                ("/lib/", "/lib64/", "/usr/lib/", "/usr/lib64/")
            ):
                raise ValueError(f"unstaged Linux dynamic dependency: {resolved}")
        return
    if platform.system() != "Darwin":
        return
    completed = subprocess.run(
        ["otool", "-L", str(path)], capture_output=True, text=True, check=True, timeout=10
    )
    for row in completed.stdout.splitlines()[1:]:
        dependency = row.strip().split(" (", 1)[0]
        if dependency.startswith(("/usr/lib/", "/System/Library/")):
            continue
        if dependency.startswith("@rpath/") and Path(dependency).name in supplied_libraries:
            continue
        raise ValueError(f"unstaged macOS dynamic dependency: {dependency}")


def _probe_output(command: list[str], source: str, label: str) -> object:
    try:
        stdout, stderr, returncode = run_helper(
            command,
            source.encode("utf-8"),
            timeout_seconds=PROBE_TIMEOUT_SECONDS,
            max_stdout=MAX_PROBE_OUTPUT,
        )
    except ProjectionProcessError as error:
        raise ValueError(f"installed candidate probe failed: {label}: {error.kind}") from error
    if returncode or stderr:
        raise ValueError(f"installed candidate probe failed: {label}")
    return json.loads(stdout, object_pairs_hook=_unique)


def _verify_probe_output(output: object, path: str, source: str, name: str) -> None:
    if not isinstance(output, dict) or output.get("status") != "ok":
        raise ValueError(f"installed candidate probe status differs: {path}")
    records = output.get("records")
    names = output.get("names")
    if (
        not isinstance(records, list)
        or len(records) != 1
        or not isinstance(records[0], dict)
        or not isinstance(names, dict)
        or len(names) != 1
    ):
        raise ValueError(f"installed candidate probe inventory differs: {path}")
    record = records[0]
    symbol_id = record.get("symbol_id")
    expected_start = source.encode("utf-8").index(name.encode("utf-8"))
    expected_name = {
        "name": name,
        "start_byte": expected_start,
        "end_byte": expected_start + len(name.encode("utf-8")),
    }
    if (
        record.get("repo_relative_path") != path
        or record.get("local_name") != name
        or record.get("symbol_kind") != "function"
        or not isinstance(symbol_id, str)
        or names.get(symbol_id) != expected_name
    ):
        raise ValueError(f"installed candidate probe source span differs: {path}")


def _probe(manifest: dict[str, object]) -> None:
    official = manifest["official_binary"]["path"]
    common = [
        manifest["node"]["path"],
        manifest["node"]["sha256"],
        manifest["node_version"],
        manifest["typescript_js"]["path"],
        manifest["typescript_js"]["sha256"],
        manifest["script_sha256"],
        manifest["typescript_version"],
    ]
    for library in manifest["node_libraries"]:
        common.extend((library["path"], library["sha256"]))
    syntax_inputs = []
    syntax_outputs = []
    for producer, path, source, name in PROBES:
        args = (
            [
                producer,
                path,
                manifest["go_helper"]["path"],
                manifest["go_helper"]["sha256"],
                manifest["go_version"],
            ]
            if producer == "go"
            else [producer, path, *common]
        )
        output = _probe_output([official, *args, "--inventory"], source, path)
        _verify_probe_output(output, path, source, name)
        if producer == "syntax":
            syntax_inputs.append({"path": path, "source": source})
            syntax_outputs.append({"path": path, **output})
    syntax_inputs.append(MALFORMED_PROBE)
    syntax_outputs.append(
        {
            "path": MALFORMED_PROBE["path"],
            "status": "parse_failure",
            "records": [],
            "names": {},
        }
    )
    batch = _probe_output(
        [manifest["syntax_batch_binary"]["path"], *common, "--inventory", "10000", "60000"],
        json.dumps(syntax_inputs),
        "syntax batch",
    )
    if not isinstance(batch, list) or len(batch) != len(syntax_outputs):
        raise ValueError("installed candidate syntax batch differs in row count")
    for actual, expected in zip(batch, syntax_outputs):
        if actual != expected:
            status = actual.get("status") if isinstance(actual, dict) else None
            raise ValueError(
                f"installed candidate syntax batch differs at {expected['path']}: "
                f"status {status!r}, expected {expected['status']!r} with matching inventory"
            )


def install(
    bundle: Path, descriptor: Path, expected_sha256: str, destination: Path
) -> tuple[Path, str]:
    if not re.fullmatch(r"[0-9a-f]{64}", expected_sha256):
        raise ValueError("bundle descriptor requires an external lowercase SHA-256")
    bundle = bundle.resolve(strict=True)
    destination = destination.absolute()
    if destination.is_symlink():
        raise ValueError("bundle must be a directory and external destination must be absent")
    # The manifest is read back by consumers that reject symlink ancestors.
    # Resolve platform aliases such as macOS /tmp before pinning artifact paths.
    destination = destination.parent.resolve() / destination.name
    if (
        not bundle.is_dir()
        or destination.exists()
        or destination.is_symlink()
        or destination == ROOT
        or ROOT in destination.parents
    ):
        raise ValueError("bundle must be a directory and external destination must be absent")
    descriptor_bytes = _regular_bytes(descriptor, MAX_MANIFEST)
    if _sha(descriptor_bytes) != expected_sha256:
        raise ValueError("bundle descriptor differs from external SHA-256")
    document = json.loads(descriptor_bytes, object_pairs_hook=_unique)
    required = {
        "schema",
        "platform",
        "architecture",
        "grammar_identity",
        "go_version",
        "node_version",
        "typescript_version",
        "script_sha256",
        "node_libraries",
        *ARTIFACTS,
    }
    if (
        not isinstance(document, dict)
        or set(document) != required
        or document["schema"] != SCHEMA
        or document["platform"] != {"Darwin": "macos", "Linux": "linux"}.get(platform.system())
        or document["architecture"]
        != {"arm64": "aarch64", "x86_64": "x86_64"}.get(platform.machine())
        or not isinstance(document["grammar_identity"], str)
        or not document["grammar_identity"].startswith("registry-")
        or document["go_version"] != "go1.25.4"
        or not isinstance(document["node_version"], str)
        or not re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+", document["node_version"])
        or document["typescript_version"] != "5.9.3"
        or not isinstance(document["node_libraries"], list)
        or len(document["node_libraries"]) > 8
        or document["script_sha256"] != _sha(_regular_bytes(SCRIPT, 1 << 20))
    ):
        raise ValueError("candidate bundle descriptor is invalid for this source or platform")
    destination.parent.mkdir(parents=True, exist_ok=True)
    stage = Path(tempfile.mkdtemp(prefix=".candidate-", dir=destination.parent))
    published = False
    try:
        manifest = {
            key: document[key]
            for key in (
                "grammar_identity",
                "go_version",
                "node_version",
                "typescript_version",
                "script_sha256",
            )
        }
        manifest.update(
            schema=OUTPUT_SCHEMA,
            platform=document["platform"],
            architecture={"arm64": "aarch64"}.get(platform.machine(), platform.machine()),
        )
        names = set()
        for key in ARTIFACTS:
            filename, data = _artifact(bundle, document[key], key)
            if filename in names:
                raise ValueError(f"duplicate candidate artifact filename: {filename}")
            names.add(filename)
            path = stage / filename
            path.write_bytes(data)
            path.chmod(
                0o700
                if key in {"official_binary", "syntax_batch_binary", "go_helper", "node"}
                else 0o600
            )
            manifest[key] = {"path": str(destination / filename), "sha256": _sha(data)}
        libraries = []
        for index, item in enumerate(document["node_libraries"]):
            filename, data = _artifact(bundle, item, f"node_libraries[{index}]")
            if filename in names:
                raise ValueError(f"duplicate candidate artifact filename: {filename}")
            names.add(filename)
            path = stage / filename
            path.write_bytes(data)
            path.chmod(0o600)
            libraries.append({"path": str(destination / filename), "sha256": _sha(data)})
        manifest["node_libraries"] = libraries
        library_names = {Path(item["path"]).name for item in libraries}
        for key in ("official_binary", "syntax_batch_binary", "go_helper", "node"):
            _check_dynamic_dependencies(stage / Path(manifest[key]["path"]).name, library_names)
        for item in libraries:
            _check_dynamic_dependencies(stage / Path(item["path"]).name, library_names)
        if destination.exists() or destination.is_symlink():
            raise ValueError("candidate destination appeared during installation")
        stage.rename(destination)
        published = True
        _probe(manifest)
        encoded = (json.dumps(manifest, sort_keys=True, separators=(",", ":")) + "\n").encode()
        output = destination / "manifest.json"
        output.write_bytes(encoded)
        return output, _sha(encoded)
    except BaseException:
        if published and destination.exists():
            shutil.rmtree(destination)
        raise
    finally:
        if stage.exists():
            shutil.rmtree(stage)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bundle", required=True, type=Path)
    parser.add_argument("--descriptor", required=True, type=Path)
    parser.add_argument("--descriptor-sha256", required=True)
    parser.add_argument("--destination", required=True, type=Path)
    args = parser.parse_args()
    try:
        output, digest = install(
            args.bundle, args.descriptor, args.descriptor_sha256, args.destination
        )
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        parser.exit(1, f"candidate install failed: {error}\n")
    print(f"{output} {digest}")


if __name__ == "__main__":
    main()
