#!/usr/bin/env python3
"""Check Tantivy residue in the workspace or the actual release compiler artifacts."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import stat
import struct
import subprocess
import sys
import tempfile
from collections.abc import Iterator
from contextlib import contextmanager
from pathlib import Path, PurePosixPath
from typing import TYPE_CHECKING, BinaryIO
from urllib.parse import unquote, urlsplit

if TYPE_CHECKING:
    from tree_sitter import Node

ROOT = Path(__file__).resolve().parents[2]
WORKSPACE_PACKAGE = "quanta-index-searchd-runtime"
DAEMON_BINARY = "quanta-index-searchd"
NATIVE_RELEASE_LANE = "release-native-daemon-bin-lane"
CLI_RELEASE_LANE = "release-bin-lane"
CLI_BINARY = "quanta-index-searchctl"
SDK_PACKAGE = "quanta-index-sdk"
INSTALLED_LINUX_TARGET = "x86_64-unknown-linux-gnu"
RELEASE_ROLES = {
    "daemon": (NATIVE_RELEASE_LANE, WORKSPACE_PACKAGE, DAEMON_BINARY, "--no-default-features"),
    "cli": (CLI_RELEASE_LANE, CLI_BINARY, CLI_BINARY, "--all-features"),
}
ISOLATED_RELEASE_PREFIX = ".tantivy-release-"
RUST_SOURCE_DIRS = ("crates", "benchmarks", "shared", "tools")
TRUST_ROOT = Path("/etc/semantica-codegraph-v2/trust")
SHIPPED_ROLES = {
    "public-cli": "bin/quanta",
    "server": "libexec/quanta/quanta-cli-server",
    "sql-rulepack-worker": "libexec/quanta/quanta-sql-rulepack-worker",
    "parser-native-worker": "libexec/quanta/quanta_native_worker_v3",
}
INSTALLED_READBACK_STATE = "LOCAL_SIGNED_LINUX_INSTALLED_PRODUCT_PRODUCED_NOT_C01_ACCEPTANCE"

def tantivy_package(name: str) -> bool:
    return name == "tantivy" or name.startswith(("tantivy-", "tantivy_"))


def cargo_output(root: Path, *args: str) -> str:
    return subprocess.check_output(
        [str(root / "scripts/cargow"), *args, "--locked", "--offline"],
        cwd=root,
        text=True,
    )


def package_names(rows: list[dict]) -> set[str]:
    return {row["name"] for row in rows}


def read_toml(path: Path) -> dict:
    try:
        import tomllib
    except ModuleNotFoundError:  # pragma: no cover - Python < 3.11
        import tomli as tomllib  # type: ignore[no-redef]

    with path.open("rb") as stream:
        return tomllib.load(stream)


def require_linux_x86_64_header(header: bytes, size: int, path: Path) -> None:
    if len(header) != 64 or header[:7] != b"\x7fELF\x02\x01\x01" or header[7] not in (0, 3):
        raise ValueError(f"release artifact must be a Linux x86-64 executable: {path}")
    kind, machine, version, entry, program_offset = struct.unpack_from("<HHIQQ", header, 16)
    header_size, program_size, program_count = struct.unpack_from("<HHH", header, 52)
    # ET_EXEC and ET_DYN cover fixed-address and position-independent executables.
    if (
        kind not in (2, 3) or machine != 62 or version != 1 or entry == 0
        or header_size != 64 or program_size != 56 or program_count == 0
        or program_offset < header_size or program_offset + program_count * program_size > size
    ):
        raise ValueError(f"release artifact must be a Linux x86-64 executable: {path}")


def file_sha256(path: Path, *, installed_linux: bool = False) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        if installed_linux:
            header = stream.read(64)
            require_linux_x86_64_header(header, os.fstat(stream.fileno()).st_size, path)
            digest.update(header)
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def require_daemon_digest(actual: str, expected: str | None) -> None:
    if expected is not None and actual != expected:
        raise ValueError(f"release daemon SHA-256 {actual} differs from expected {expected}")


def release_build_args(role: str) -> list[str]:
    lane, package, binary, features = RELEASE_ROLES[role]
    return [
        "--lane", lane, "build", "-p", package, "--bin", binary, features,
        "--locked", "--offline", "--release",
    ]


def installed_release_invocation(command: list[str], role: str) -> tuple[Path, Path]:
    """Validate the producer's command using paths from the archived invocation."""
    prefix = release_build_args(role)
    options = command[len(prefix) + 1:]
    wrapper = Path(command[0])
    if (
        command[1:len(prefix) + 1] != prefix
        or not wrapper.is_absolute() or ".." in wrapper.parts
        or wrapper.parts[-2:] != ("scripts", "cargow")
        or len(options) != 7
        or options[0] != "--config" or not options[1].startswith("build.build-dir=")
        or options[2] != "--target-dir"
        or options[4:] != ["--message-format=json-render-diagnostics", "--target", INSTALLED_LINUX_TARGET]
    ):
        raise ValueError(f"installed {role} receipt differs from the canonical Linux build command")
    target = Path(options[3])
    if (
        not target.is_absolute() or ".." in target.parts
        or not target.name.startswith(ISOLATED_RELEASE_PREFIX)
    ):
        raise ValueError(f"installed {role} receipt lacks an isolated target directory")
    return wrapper.parents[1], target / INSTALLED_LINUX_TARGET / "release"


def require_installed_compiler_path(compiler: Path, output: Path) -> None:
    if ".." in compiler.parts or not compiler.is_relative_to(output):
        raise ValueError("installed release compiler artifact differs from this invocation's output directory")


def release_receipt_and_source(
    evidence: Path, expected_receipt_sha256: str
) -> tuple[dict, bytes]:
    """Validate the existing release receipt and its exact source capture."""
    evidence = evidence.resolve(strict=True)
    receipt_bytes = (evidence / "build-result.json").read_bytes()
    if hashlib.sha256(receipt_bytes).hexdigest() != expected_receipt_sha256:
        raise ValueError("release build receipt SHA-256 differs from the expected digest")
    receipt = json.loads(receipt_bytes)
    if (
        receipt["status"] != "VERIFIED"
        or type(receipt["exit_code"]) is not int
        or receipt["exit_code"] != 0
        or receipt["source_before_after_equal"] is not True
    ):
        raise ValueError("release build has no source-stable successful receipt")
    command = receipt["command"]
    if (
        not isinstance(command, list)
        or not all(isinstance(arg, str) for arg in command)
        or not {
            "build", "--release", "--locked", "--offline",
            "--message-format=json-render-diagnostics",
        }.issubset(command)
    ):
        raise ValueError("release build has no locked Cargo artifact command")
    before = (evidence / "source-before.json").read_bytes()
    if before != (evidence / "source-after-build.json").read_bytes():
        raise ValueError("release build source captures differ")
    if hashlib.sha256(before).hexdigest() != receipt["source_capture_sha256"]:
        raise ValueError("release build source capture differs from the build receipt")
    files = json.loads(before)["files"]
    if not isinstance(files, dict) or not files:
        raise ValueError("release build source capture lacks a nonempty file digest map")
    for name, digest in files.items():
        path = PurePosixPath(name)
        if (
            not path.parts or path.is_absolute() or ".." in path.parts or name != path.as_posix()
            or not isinstance(digest, str) or re.fullmatch(r"[0-9a-f]{64}", digest) is None
        ):
            raise ValueError("release build source capture has an invalid file digest entry")

    expected_stream_sha256 = receipt["compiler_stream_sha256"]
    if re.fullmatch(r"[0-9a-f]{64}", expected_stream_sha256) is None:
        raise ValueError("release build receipt has an invalid compiler stream SHA-256")
    return receipt, before


def release_build_findings(
    evidence: Path, expected_receipt_sha256: str, *, installed_linux: bool = False,
) -> tuple[list[str], str, bytes]:
    """Check a source-stable SDK60 release build's captured Cargo artifacts."""
    receipt, source_capture = release_receipt_and_source(evidence, expected_receipt_sha256)
    daemon = receipt["binaries"][DAEMON_BINARY]
    # The original path carries provenance. The archive member has a fixed name.
    retained_path = Path(daemon["path"])
    if not retained_path.is_absolute() or retained_path.name != DAEMON_BINARY:
        raise ValueError("release build receipt has an invalid retained daemon path")
    compiler_executable = daemon["compiler_artifact"]
    compiler_path = Path(compiler_executable)
    if not compiler_path.is_absolute() or compiler_path.name != DAEMON_BINARY:
        raise ValueError("release build receipt has an invalid compiler artifact path")
    root = None
    if installed_linux:
        root, output = installed_release_invocation(receipt["command"], "daemon")
        require_installed_compiler_path(compiler_path, output)
    compiler_digest = daemon.get("compiler_artifact_sha256")
    if not isinstance(compiler_digest, str) or re.fullmatch(r"[0-9a-f]{64}", compiler_digest) is None:
        raise ValueError("release build receipt lacks a valid compiler artifact SHA-256")
    if compiler_digest != daemon["sha256"]:
        raise ValueError("compiler artifact SHA-256 differs from the retained daemon digest")
    binary = evidence / DAEMON_BINARY
    if binary.is_symlink() or not binary.is_file():
        raise ValueError("release evidence lacks a regular retained daemon")
    if file_sha256(binary, installed_linux=installed_linux) != compiler_digest:
        raise ValueError("retained daemon digest differs from the build receipt")

    with (evidence / "build-artifacts.jsonl").open("rb") as log:
        findings, _ = compiler_artifact_findings(
            root, log, compiler_executable,
            expected_stream_sha256=receipt["compiler_stream_sha256"],
        )
    return findings, compiler_digest, source_capture


def compiler_package_identity(package_id: str) -> tuple[str, Path | None]:
    """Decode the canonical Package ID spec emitted by Cargo's artifact protocol."""
    if not isinstance(package_id, str) or any(c.isspace() for c in package_id):
        raise ValueError("release compiler artifact has a malformed Cargo package ID")
    source = urlsplit(package_id)
    name, separator, version = source.fragment.partition("@")
    if not separator:
        # Cargo omits the name when it equals the URL's last path segment.
        version, name = name, source.path.rsplit("/", 1)[-1]
    if not all(
        part.replace("-", "_").isidentifier() and not part.startswith("-")
        for part in name.split("::")
    ) or not re.fullmatch(
        r"[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?"
        r"(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?",
        version,
    ):
        raise ValueError("release compiler artifact has a malformed Cargo package ID")
    if source.scheme in {"path+file", "file"}:
        directory = Path(unquote(source.path, errors="strict"))
        if source.netloc or source.query or not directory.is_absolute() or ".." in directory.parts:
            raise ValueError("release compiler artifact has an invalid local Cargo package ID")
        return name, directory
    kind, _, scheme = source.scheme.partition("+")
    if (
        kind in {"registry", "sparse", "git"}
        and scheme
        and (source.netloc or (scheme == "file" and source.path.startswith("/")))
    ):
        return name, None
    raise ValueError("release compiler artifact has an unsupported Cargo package ID source")


def compiler_artifact_findings(
    root: Path | None,
    log: BinaryIO,
    compiler_executable: str | None = None,
    *,
    expected_stream_sha256: str | None = None,
    show_diagnostics: bool = False,
    required_daemon: bool = True,
    consumer_artifacts: dict[str, Path] | None = None,
) -> tuple[list[str], Path | None]:
    """Check the successful Cargo stream and the exact optimized daemon artifact."""
    packages = set()
    daemon = None
    finished = False
    digest = hashlib.sha256() if expected_stream_sha256 is not None else None
    for line in log:
        if digest is not None:
            digest.update(line)
        if finished:
            raise ValueError("release build has output after its Cargo terminal event")
        row = json.loads(line)
        if not isinstance(row, dict):
            raise ValueError("release Cargo event is not an object")
        reason = row.get("reason")
        if reason == "compiler-message" and show_diagnostics:
            rendered = row["message"]["rendered"]
            if rendered:
                sys.stderr.write(rendered)
        if reason == "build-finished":
            if row.get("success") is not True:
                raise ValueError("release Cargo build failed")
            finished = True
        if reason != "compiler-artifact":
            continue
        name, package_dir = compiler_package_identity(row["package_id"])
        packages.add(name)
        if consumer_artifacts is not None and name in {"quanta-index-searchctl", "quanta-index-sdk"}:
            target_name, target_kind = (
                ("quanta-index-searchctl", ["bin"]) if name == "quanta-index-searchctl"
                else ("quanta_index_sdk", ["lib"])
            )
            target = row["target"]
            profile = row["profile"]
            if (
                name in consumer_artifacts
                or package_dir is None or package_dir.parts[-2:] != ("crates", name)
                or (root is not None and package_dir != root / "crates" / name)
                or row["manifest_path"] != str(package_dir / "Cargo.toml")
                or target["name"] != target_name or target["kind"] != target_kind
                or profile["test"] is not False
                or profile["opt_level"] not in ("1", "2", "3", "s", "z")
                or "legacy-tantivy" in row["features"]
            ):
                raise ValueError(f"unexpected {name} compiler artifact in the release stream")
            if name == "quanta-index-searchctl":
                artifact = row["executable"]
                if not isinstance(artifact, str) or Path(artifact).name != target_name:
                    raise ValueError("release CLI compiler executable is invalid")
            else:
                filenames = row["filenames"]
                matching = [path for path in filenames if re.fullmatch(r"libquanta_index_sdk-[0-9A-Za-z_-]+\.rlib", Path(path).name)]
                if row.get("executable") is not None or len(matching) != 1:
                    raise ValueError("release SDK compiler artifact is invalid")
                artifact = matching[0]
            if not Path(artifact).is_absolute():
                raise ValueError(f"release {name} compiler artifact must be absolute")
            consumer_artifacts[name] = Path(artifact)
        if not required_daemon or name != WORKSPACE_PACKAGE or row["target"]["name"] != DAEMON_BINARY:
            continue
        if row.get("features") != []:
            raise ValueError("Native daemon compiler artifact requires an empty features list")
        executable = row["executable"]
        if (
            daemon is not None
            or package_dir is None
            or package_dir.parts[-2:] != ("crates", WORKSPACE_PACKAGE)
            or row["manifest_path"] != str(package_dir / "Cargo.toml")
            or (root is not None and package_dir != root / "crates" / WORKSPACE_PACKAGE)
            or row["target"]["kind"] != ["bin"]
            or row["profile"]["test"] is not False
            or row["profile"]["opt_level"] not in ("1", "2", "3", "s", "z")
            or not Path(executable).is_absolute()
            or (compiler_executable is not None and executable != compiler_executable)
        ):
            raise ValueError("unexpected daemon compiler artifact in the release stream")
        daemon = Path(executable)
    if not finished or (required_daemon and daemon is None):
        raise ValueError("release build lacks one daemon artifact and successful Cargo terminal")
    if consumer_artifacts is not None and set(consumer_artifacts) != {"quanta-index-searchctl", "quanta-index-sdk"}:
        raise ValueError("release build lacks CLI or SDK compiler artifacts")
    if digest is not None and digest.hexdigest() != expected_stream_sha256:
        raise ValueError("release compiler stream SHA-256 differs from the build's expected digest")
    findings = [
        f"Release compiler artifact: {name}" for name in sorted(packages) if tantivy_package(name)
    ]
    return findings, daemon


def release_cli_findings(
    evidence: Path, expected_receipt_sha256: str
) -> tuple[list[str], bytes]:
    """Check one actual CLI build stream and its retained CLI and SDK bytes."""
    receipt, source_capture = release_receipt_and_source(evidence, expected_receipt_sha256)
    root, output = installed_release_invocation(receipt["command"], "cli")
    artifacts: dict[str, Path] = {}
    with (evidence / "build-artifacts.jsonl").open("rb") as log:
        findings, _ = compiler_artifact_findings(
            root, log, expected_stream_sha256=receipt["compiler_stream_sha256"],
            required_daemon=False, consumer_artifacts=artifacts,
        )
    for package, compiler in artifacts.items():
        require_installed_compiler_path(compiler, output)
        entry = receipt["binaries"][package]
        original = Path(entry["compiler_artifact"])
        retained = Path(entry["path"])
        digest = entry["compiler_artifact_sha256"]
        if (
            original != compiler or not retained.is_absolute()
            or retained.name != compiler.name
            or not isinstance(digest, str) or re.fullmatch(r"[0-9a-f]{64}", digest) is None
            or entry["sha256"] != digest
        ):
            raise ValueError(f"release {package} compiler and retained artifact binding differs")
        archived = evidence / compiler.name
        if archived.is_symlink() or not archived.is_file():
            raise ValueError(f"release {package} retained artifact bytes differ")
        if file_sha256(archived, installed_linux=package == CLI_BINARY) != digest:
            raise ValueError(f"release {package} retained artifact bytes differ")
    return findings, source_capture


def read_regular_bytes(path: Path, maximum: int, label: str) -> bytes:
    if path.is_symlink() or not path.is_file():
        raise ValueError(f"{label} must be a regular file")
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
    with os.fdopen(descriptor, "rb") as stream:
        info = os.fstat(stream.fileno())
        if not stat.S_ISREG(info.st_mode) or not 0 < info.st_size <= maximum:
            raise ValueError(f"{label} size or type differs")
        data = stream.read(maximum + 1)
    if len(data) != info.st_size:
        raise ValueError(f"{label} changed while reading")
    return data


def require_live_approved_q(request: dict, payload: dict, daemon_sha256: str) -> None:
    from tools.deployment import launcher
    from tools.deployment import native_release_approval as approval

    root, selection_path, policy = approval._request_paths(request)
    selection_sha = request["selection_sha256"]
    policy_sha = request["decision_policy_sha256"]
    try:
        with launcher.deployment_lock(selection_path):
            approval._verify_request_digests(request, selection_path, policy)
            selection = approval._require_native_selection(root, selection_path, selection_sha, daemon_sha256, launcher)
            expected = approval.binding(
                selection, selection_sha, policy_sha,
                payload["active_generation_sha256"], payload["release_manifest_sha256"],
            )
            if approval.signing_bytes(expected) != approval.signing_bytes(payload):
                raise ValueError("signed Q approval differs from the installed selection or policy")
            launcher._check_binary(selection)
    except launcher.SelectionError as error:
        raise ValueError(f"installed Q selection differs: {error}") from error


def shipped_bundle_findings(
    bundle: Path, readback_path: Path, daemon_sha256: str, expected_s_source_head: str,
    *, approval_request_path: Path, trust_root: Path = TRUST_ROOT, paired_checkout: Path | None = None,
) -> tuple[str, str, str]:
    """Bind the installed Q ELF to the authenticated S manifest and its files."""
    from cryptography.exceptions import InvalidSignature
    from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PublicKey

    from tools.benchmark.evidence import CONTROL_DOCUMENT_BYTES
    from tools.ci.proof_json import parse_proof_json
    from tools.ci.proof_operational_result import installation_identity
    from tools.deployment.native_release_approval import (
        PRODUCT_ACTIVE_MAX_BYTES,
        SCHEMA,
        signing_bytes,
        verify_installed_product_generation,
    )

    active_path = trust_root / "active-generation.json"
    active_bytes = read_regular_bytes(active_path, PRODUCT_ACTIVE_MAX_BYTES, "installed active generation")
    key = read_regular_bytes(trust_root / "deployment-public-key.bin", 32, "deployment public key")
    if len(key) != 32:
        raise ValueError("deployment public key length differs")
    active = verify_installed_product_generation(active_bytes, key)
    manifest_sha = active["release_manifest_sha256"]
    install_root = Path(active["install_root"])
    if (
        not isinstance(manifest_sha, str) or re.fullmatch(r"[0-9a-f]{64}", manifest_sha) is None
        or not install_root.is_absolute() or install_root.resolve(strict=True) != install_root
        or bundle.is_symlink() or bundle.resolve(strict=True) != install_root / "releases" / manifest_sha
    ):
        raise ValueError("shipped bundle differs from the installed Product generation")
    readback_bytes = read_regular_bytes(readback_path, CONTROL_DOCUMENT_BYTES, "signed install readback")
    readback = parse_proof_json(readback_bytes)
    active_sha256 = hashlib.sha256(active_bytes).hexdigest()
    if (
        not isinstance(readback, dict)
        or readback.get("state") != INSTALLED_READBACK_STATE
        or readback.get("installed_active_generation_sha256") != active_sha256
        or readback.get("selected") != active
    ):
        raise ValueError("signed install readback differs from the active Product generation")
    approval = readback.get("native_q_approval")
    if not isinstance(approval, dict) or set(approval) != {"payload", "signature_hex"}:
        raise ValueError("installed Q approval envelope differs")
    payload = approval["payload"]
    signature = approval["signature_hex"]
    if (
        not isinstance(payload, dict)
        or payload.get("schema_version") != SCHEMA
        or payload.get("active_generation_sha256") != active_sha256
        or payload.get("release_manifest_sha256") != manifest_sha
        or payload.get("q_elf_sha256") != daemon_sha256
        or not isinstance(signature, str) or re.fullmatch(r"[0-9a-f]{128}", signature) is None
    ):
        raise ValueError("installed Q approval differs from this Q/S release pair")
    try:
        Ed25519PublicKey.from_public_bytes(key).verify(bytes.fromhex(signature), signing_bytes(payload))
    except InvalidSignature as error:
        raise ValueError("installed Q approval signature differs") from error

    manifest_path = bundle / "share/quanta/release-manifest.json"
    manifest_bytes = read_regular_bytes(manifest_path, 4 * 1024 * 1024, "shipped release manifest")
    if hashlib.sha256(manifest_bytes).hexdigest() != manifest_sha:
        raise ValueError("shipped release manifest differs from installed approval")
    manifest = json.loads(manifest_bytes)
    if not isinstance(manifest, dict) or manifest.get("schema_version") != "quanta-release-bundle-v4":
        raise ValueError("shipped release manifest schema differs")
    provenance = manifest["build_provenance"]
    source_digest = manifest["source_snapshot_digest"]
    if (
        not isinstance(provenance, dict)
        or provenance.get("git_sha") != expected_s_source_head
        or not isinstance(source_digest, str)
        or re.fullmatch(r"[0-9a-f]{64}", source_digest) is None
        or provenance.get("source_snapshot_digest") != source_digest
    ):
        raise ValueError("shipped S source provenance differs from the P11 source pair")
    receipt = manifest["build_receipt"]
    if receipt["relative_path"] != "share/quanta/build-receipt.json":
        raise ValueError("shipped build receipt path differs")
    entries = [(receipt["relative_path"], receipt)]
    artifacts = manifest["artifacts"]
    if not isinstance(artifacts, list) or len(artifacts) != len(SHIPPED_ROLES):
        raise ValueError("shipped bundle artifact inventory is incomplete")
    observed = set()
    for row in artifacts:
        role = row["role"]
        if role not in SHIPPED_ROLES or role in observed or row["relative_path"] != SHIPPED_ROLES[role]:
            raise ValueError("shipped bundle artifact role or path differs")
        observed.add(role)
        entries.append((row["relative_path"], row))
    if observed != set(SHIPPED_ROLES):
        raise ValueError("shipped bundle artifact inventory is incomplete")
    for relative, row in entries:
        path = bundle / relative
        size = row["size_bytes"]
        digest = row["sha256"]
        if (
            path.is_symlink() or not path.is_file() or path.resolve(strict=True) != path
            or type(size) is not int or size <= 0
            or not isinstance(digest, str) or re.fullmatch(r"[0-9a-f]{64}", digest) is None
            or path.stat().st_size != size or file_sha256(path) != digest
        ):
            raise ValueError(f"shipped bundle member {relative} differs from signed manifest")
        if relative != "share/quanta/build-receipt.json" and path.stat().st_mode & 0o111 == 0:
            raise ValueError(f"shipped bundle member {relative} is not executable")
    copied_receipt = json.loads(read_regular_bytes(
        bundle / "share/quanta/build-receipt.json", 64 * 1024 * 1024, "shipped build receipt",
    ))
    if copied_receipt["source_snapshot_digest"] != source_digest:
        raise ValueError("shipped S source snapshot differs from its QBC receipt")
    if paired_checkout is not None:
        verify_paired_product_bundle(paired_checkout, bundle, source_digest)
        if manifest_bytes != read_regular_bytes(manifest_path, 4 * 1024 * 1024, "shipped release manifest"):
            raise ValueError("shipped release manifest changed during inventory")
    approval_request = parse_proof_json(
        read_regular_bytes(approval_request_path, CONTROL_DOCUMENT_BYTES, "installed Q approval request")
    )
    require_live_approved_q(approval_request, payload, daemon_sha256)
    if active_bytes != read_regular_bytes(active_path, PRODUCT_ACTIVE_MAX_BYTES, "installed active generation"):
        raise ValueError("installed Product generation changed during inventory")
    if key != read_regular_bytes(trust_root / "deployment-public-key.bin", 32, "deployment public key"):
        raise ValueError("deployment public key changed during inventory")
    if readback_bytes != read_regular_bytes(readback_path, CONTROL_DOCUMENT_BYTES, "signed install readback"):
        raise ValueError("signed install readback changed during inventory")
    return installation_identity(payload)


def verify_paired_product_bundle(paired_checkout: Path, bundle: Path, expected_source_digest: str) -> None:
    paired_checkout = paired_checkout.resolve(strict=True)
    bundle = bundle.resolve(strict=True)
    subprocess.run(
        [
            sys.executable, "-I", "-B", "-c", """
import sys
from pathlib import Path

sys.path.insert(0, sys.argv[1])
from tools.ci.source_snapshot_identity_v1 import source_snapshot_digest_v1
from tools.daemon_release_manifest.product_bundle import (
    LINUX_INSTALLED_PRODUCT_BUILD_PROFILE_V1,
    verify_product_bundle_v1,
)

root, bundle = Path(sys.argv[1]), Path(sys.argv[2])
verify_product_bundle_v1(bundle, expected_profile_id_v1=LINUX_INSTALLED_PRODUCT_BUILD_PROFILE_V1)
if source_snapshot_digest_v1(cwd=root) != sys.argv[3]:
    raise SystemExit("installed Product source snapshot differs from the validated S checkout")
""", str(paired_checkout), str(bundle), expected_source_digest,
        ],
        cwd=paired_checkout,
        check=True,
    )


@contextmanager
def isolated_release_command(
    root: Path, role: str, *, installed_linux: bool = False,
) -> Iterator[tuple[list[str], Path, Path]]:
    if role not in RELEASE_ROLES:
        raise ValueError(f"unknown release build role: {role}")
    lane, _, _, features = RELEASE_ROLES[role]
    metadata = json.loads(cargo_output(
        root, "--lane", lane, "metadata", "--format-version", "1", "--no-deps", features,
    ))
    target = Path(metadata["target_directory"]).resolve()
    target.mkdir(parents=True, exist_ok=True)
    # Keep the existing Cargo intermediates and isolate the final artifact
    # from other builds after the cooperative Cargo lease is released.
    build_dir = json.dumps(metadata["build_directory"], ensure_ascii=False)
    with tempfile.TemporaryDirectory(prefix=ISOLATED_RELEASE_PREFIX, dir=target) as raw:
        output = Path(raw)
        command = [
            str(root / "scripts/cargow"), *release_build_args(role),
            "--config", f"build.build-dir={build_dir}",
            "--target-dir", str(output), "--message-format=json-render-diagnostics",
        ]
        if installed_linux:
            command.extend(["--target", INSTALLED_LINUX_TARGET])
        yield command, output, target


def run_release_compiler(command: list[str], root: Path, log: BinaryIO) -> subprocess.CompletedProcess:
    return subprocess.run(
        command, cwd=root, stdout=log, check=False,
        env={**os.environ, "QUANTA_INDEX_RESOURCE_ADMISSION": "1"},
    )


def build_and_check_native_daemon(
    root: Path, expected_sha256: str | None
) -> tuple[list[str], str, Path | None]:
    """Census one Cargo invocation and retain its daemon by digest."""
    with isolated_release_command(root, "daemon") as (command, output, target):
        with tempfile.TemporaryFile() as log:
            run = run_release_compiler(command, root, log)
            log.seek(0)
            findings, daemon = compiler_artifact_findings(root, log, show_diagnostics=True)
            run.check_returncode()
        resolved = daemon.resolve(strict=True)
        if (
            daemon.is_symlink() or not daemon.is_file() or not os.access(daemon, os.X_OK)
            or resolved.name != DAEMON_BINARY or not resolved.is_relative_to(output)
        ):
            raise ValueError("built daemon must be a regular executable in this invocation's target directory")
        candidate = output / "checked-daemon"
        shutil.copy2(resolved, candidate)
        digest = file_sha256(candidate)
        require_daemon_digest(digest, expected_sha256)
        if findings:
            return findings, digest, None

        published = target / "verified-native" / digest / DAEMON_BINARY
        published.parent.mkdir(parents=True, exist_ok=True)
        try:
            os.link(candidate, published)
        except FileExistsError:
            if (
                published.is_symlink() or not published.is_file() or not os.access(published, os.X_OK)
                or file_sha256(published) != digest
            ):
                raise ValueError("existing verified Native daemon differs from its digest") from None
        return findings, digest, published


def tantivy_identifier(node: Node | None) -> bool:
    return (
        node is not None
        and node.type == "identifier"
        and tantivy_package(node.text.decode("utf-8").removeprefix("r#"))
    )


def rust_imports_tantivy(node: Node | None) -> bool:
    if node is None:
        return False
    path = node.child_by_field_name("path")
    if path is not None:
        return rust_imports_tantivy(path)
    if node.type == "scoped_identifier":
        return tantivy_identifier(node.child_by_field_name("name"))
    if node.type == "scoped_use_list":
        return rust_imports_tantivy(node.child_by_field_name("list"))
    if node.type in {"use_list", "use_wildcard"}:
        return any(rust_imports_tantivy(child) for child in node.named_children)
    return tantivy_identifier(node)


def rust_references_tantivy(root: Node) -> bool:
    pending = [root]
    while pending:
        node = pending.pop()
        if node.type == "extern_crate_declaration":
            if tantivy_identifier(node.child_by_field_name("name")):
                return True
        elif node.type == "use_declaration":
            if rust_imports_tantivy(node.child_by_field_name("argument")):
                return True
            continue
        elif node.type in {"scoped_identifier", "scoped_type_identifier"}:
            path = node.child_by_field_name("path")
            if tantivy_identifier(path if path is not None else node.child_by_field_name("name")):
                return True
        elif node.type in {"token_tree", "token_tree_pattern"}:
            tokens = [
                child for child in node.children if child.type not in {"line_comment", "block_comment"}
            ]
            for previous, token, following in zip(tokens, tokens[1:], tokens[2:]):
                if tantivy_identifier(token) and (
                    following.type == "::" or previous.type in {"use", "crate"}
                ):
                    return True
        pending.extend(node.named_children)
    return False


def rust_source_files(root: Path):
    yield from root.glob("*.rs")
    for directory in RUST_SOURCE_DIRS:
        for parent, dirs, files in os.walk(root / directory):
            dirs[:] = [name for name in dirs if name not in {".git", "target"}]
            yield from (Path(parent) / name for name in files if name.endswith(".rs"))


def retirement_findings(root: Path) -> list[str]:
    try:
        from tree_sitter_language_pack import get_parser
    except ImportError as error:
        raise ValueError("workspace retirement check requires tree-sitter-language-pack") from error

    tree = cargo_output(
        root,
        "tree",
        "--workspace",
        "--all-features",
        "--target",
        "all",
        "--edges",
        "normal,build,dev",
        "--prefix",
        "none",
        "--format",
        "{p}",
    )
    metadata = json.loads(cargo_output(root, "metadata", "--format-version", "1", "--all-features"))
    lockfile = read_toml(root / "Cargo.lock")
    workspace = read_toml(root / "Cargo.toml")
    scopes = {
        "Cargo tree": {line.split()[0] for line in tree.splitlines() if line.strip()},
        "Cargo metadata": package_names(metadata["packages"]),
        "Cargo.lock": package_names(lockfile["package"]),
    }
    if any(WORKSPACE_PACKAGE not in names for names in scopes.values()):
        raise ValueError("Cargo dependency evidence lacks the workspace daemon")

    findings = [
        f"{scope}: {name}"
        for scope, names in scopes.items()
        for name in sorted(names)
        if tantivy_package(name)
    ]
    member_ids = set(metadata["workspace_members"])
    findings.extend(sorted(
        f"Cargo workspace feature: {package['name']}/{feature}"
        for package in metadata["packages"]
        if package["id"] in member_ids
        for feature in package["features"]
        if feature == "legacy-tantivy"
        or (package["name"], feature) == ("quanta-index-searchd-harness", "legacy-bench")
    ))
    dependency_tables = {
        "workspace.dependencies": workspace["workspace"].get("dependencies", {}),
        **{
            f"patch.{registry}": entries for registry, entries in workspace.get("patch", {}).items()
        },
    }
    for scope, entries in dependency_tables.items():
        for name, spec in sorted(entries.items()):
            package = spec.get("package", name) if isinstance(spec, dict) else name
            if tantivy_package(name) or tantivy_package(package):
                findings.append(f"Cargo.toml {scope}: {name}")
    findings.extend(
        f"Cargo.toml workspace.exclude: {path}"
        for path in workspace["workspace"].get("exclude", [])
        if tantivy_package(Path(path).name)
    )

    parser = get_parser("rust")
    for source in sorted(rust_source_files(root)):
        data = source.read_bytes()
        if b"tantivy" in data and rust_references_tantivy(parser.parse(data).root_node):
            findings.append(f"Rust source: {source.relative_to(root)}")

    vendor = root / "vendor"
    vendor_residue = set()
    for entry in vendor.rglob("*"):
        if tantivy_package(entry.name):
            vendor_residue.add(entry)
        manifest = entry
        if entry.is_symlink() and entry.is_dir():
            manifest = entry / "Cargo.toml"
        if manifest.name == "Cargo.toml" and manifest.is_file():
            package = read_toml(manifest).get("package")
            if package is not None and tantivy_package(package["name"]):
                vendor_residue.add(manifest.parent)
    findings.extend(f"vendor: {entry.relative_to(root)}" for entry in sorted(vendor_residue))
    return findings


def _capture_release_source(
    root: Path, expected_head: str | None,
) -> tuple[dict[str, dict[str, str]], dict[str, tuple[int, ...]]]:
    from tools.ci.source_closure import capture_committed_source

    files, states = capture_committed_source(root, expected_head)
    return {"files": files}, states


def capture_release_source(root: Path, expected_head: str | None = None) -> dict[str, dict[str, str]]:
    """Capture the physical bytes of the complete committed Q tree."""
    capture, _ = _capture_release_source(root, expected_head)
    return capture


def require_release_source(root: Path, source_capture: bytes, expected_head: str) -> None:
    from tools.ci.proof_json import parse_proof_json

    observed = parse_proof_json(source_capture)["files"]
    if observed != capture_release_source(root, expected_head)["files"]:
        raise ValueError("release build source files differ from the complete P10 checkout")


def select_captured_daemon(root: Path, evidence: Path, expected_receipt_sha256: str) -> tuple[str, Path]:
    """Admit a recorded Linux release build for a diagnostic on this frozen source."""
    from tools.ci.source_closure import _git

    head = _git(root, "rev-parse", "HEAD")
    evidence = evidence.resolve(strict=True)
    findings, daemon_sha256, source = release_build_findings(evidence, expected_receipt_sha256, installed_linux=True)
    if findings:
        raise ValueError("\n".join(findings))
    require_release_source(root, source, head)
    binary = evidence / DAEMON_BINARY
    if not os.access(binary, os.X_OK):
        raise ValueError("recorded release daemon is not executable")
    return daemon_sha256, binary


def capture_release_build(root: Path, role: str, evidence: Path) -> list[str]:
    """Build on the normal release lane and produce the shipment reader's input."""
    from tools.ci.source_closure import _git

    root = root.resolve(strict=True)
    evidence = evidence.parent.resolve(strict=True) / evidence.name
    if evidence.is_relative_to(root):
        raise ValueError("release evidence must be outside the source checkout")
    head = _git(root, "rev-parse", "HEAD")
    before, before_states = _capture_release_source(root, head)
    before_path = evidence / "source-before.json"
    evidence.mkdir(mode=0o700)
    source_bytes = (json.dumps(before, sort_keys=True, separators=(",", ":")) + "\n").encode("utf-8")
    before_path.write_bytes(source_bytes)

    binaries = {}
    with isolated_release_command(root, role, installed_linux=True) as (command, output, _):
        log_path = evidence / "build-artifacts.jsonl"
        with log_path.open("x+b") as log:
            result = run_release_compiler(command, root, log)
            log.flush()
            log_sha256 = file_sha256(log_path)
            log.seek(0)
            artifacts: dict[str, Path] = {}
            findings, daemon = compiler_artifact_findings(
                root, log, expected_stream_sha256=log_sha256, show_diagnostics=True,
                required_daemon=role == "daemon", consumer_artifacts=artifacts if role == "cli" else None,
            )
            result.check_returncode()
        if findings:
            return findings
        if daemon is not None:
            artifacts[DAEMON_BINARY] = daemon
        for name, compiler in artifacts.items():
            if (
                compiler.resolve(strict=True) != compiler or not compiler.is_file()
                or not compiler.is_relative_to(output)
                or (name != SDK_PACKAGE and not os.access(compiler, os.X_OK))
            ):
                raise ValueError(f"release {name} must come from this invocation's target directory")
            retained = evidence / compiler.name
            compiler_digest = file_sha256(compiler, installed_linux=name != SDK_PACKAGE)
            shutil.copy2(compiler, retained)
            if file_sha256(retained) != compiler_digest:
                raise ValueError(f"release {name} artifact changed during capture")
            binaries[name] = {
                "path": str(retained), "compiler_artifact": str(compiler),
                "sha256": compiler_digest, "compiler_artifact_sha256": compiler_digest,
            }
    if _capture_release_source(root, head) != (before, before_states):
        raise ValueError("release build source changed during compilation")
    (evidence / "source-after-build.json").write_bytes(source_bytes)
    receipt = {
        "status": "VERIFIED", "exit_code": 0, "source_before_after_equal": True,
        "command": command, "binaries": binaries, "compiler_stream_sha256": log_sha256,
        "source_capture_sha256": hashlib.sha256(source_bytes).hexdigest(),
    }
    with tempfile.NamedTemporaryFile(dir=evidence, prefix=".build-result-") as terminal:
        terminal.write((json.dumps(receipt, sort_keys=True) + "\n").encode("utf-8"))
        terminal.flush()
        os.link(terminal.name, evidence / "build-result.json")
    return []


def proof_bound_candidate(
    root: Path, paired_checkout: Path, *, require_aggregate: bool = False,
) -> tuple[tuple[str, str, str], tuple[str, str, str] | None]:
    """Select the release identity from the current, source-validated proof pair."""
    from tools.ci.daemon_execution_evidence import P10_PROOF, R5_PROOF, REGISTRY_PATH
    from tools.ci.lint.handoff_validation import _read_repo_regular_bytes
    from tools.ci.proof_json import parse_proof_json
    from tools.ci.proof_operational_result import ACTIONS, operational_installation_identity

    try:
        import tomllib
    except ModuleNotFoundError:  # pragma: no cover - Python < 3.11
        import tomli as tomllib  # type: ignore[no-redef]

    registry_raw = _read_repo_regular_bytes(root, REGISTRY_PATH, label="proof registry")
    registry = tomllib.loads(registry_raw.decode("utf-8"))
    proofs = {proof["id"]: proof for proof in registry["proofs"]}
    proof_ids = (P10_PROOF, R5_PROOF, *(ACTIONS if require_aggregate else ()))
    proof_paths = tuple(proofs[proof_id]["artifact"] for proof_id in proof_ids)
    inputs = (REGISTRY_PATH, *proof_paths)
    before = (registry_raw, *(
        _read_repo_regular_bytes(root, path, label="registered release proof") for path in proof_paths
    ))
    subprocess.run(
        [
            sys.executable, str(root / "tools/ci/lint/check-proof-authority.py"),
            "--require-all" if require_aggregate else "--require-code-qualified",
            "--bind-source",
            "--paired-checkout", f"{proofs[R5_PROOF]['paired_repository']}={paired_checkout.resolve()}",
        ],
        cwd=root,
        check=True,
    )
    after = tuple(_read_repo_regular_bytes(root, path, label="release proof input") for path in inputs)
    if before != after:
        raise ValueError("release proof inputs changed during source validation")
    payloads = {proof_id: parse_proof_json(raw) for proof_id, raw in zip(proof_ids, before[1:], strict=True)}
    p10_payload, p11_payload = payloads[P10_PROOF], payloads[R5_PROOF]
    daemon_sha256 = p10_payload["daemon_binary"]["sha256"]
    if p11_payload["daemon_binary"]["sha256"] != daemon_sha256:
        raise ValueError("P10 and P11 bind a different daemon")
    identity = daemon_sha256, p11_payload["source_pair"]["source"]["head"], p10_payload["source"]["head"]
    installation = operational_installation_identity(payloads) if require_aggregate else None
    return identity, installation


def publish_qualified_aggregate(
    paired_checkout: Path, identity: tuple[str, str, str], installation: tuple[str, str, str],
) -> None:
    from runpy import run_path

    writer = run_path(str(ROOT / "tools/ci/write-proof-aggregate.py"))
    path, digest, ready = writer["publish_aggregate"](
        root=ROOT,
        registry_path=ROOT / "tools/ci/proof-authority.toml",
        paired_checkout=paired_checkout,
        required_release_identity=identity,
        required_installation=installation,
    )
    if not ready:
        raise ValueError("aggregate did not qualify the verified shipment")
    print(f"WROTE {path.relative_to(ROOT)} sha256:{digest} state=READY")


def check_migration_dispositions(root: Path, revision: str | None = None) -> int:
    committed_input = ["--revision", revision] if revision is not None else []
    status = 0
    for checker, args in (
        ("check-lexical-test-disposition.py", committed_input),
        ("check-runtime-legacy-migration.py", committed_input or ["--require-execution"]),
    ):
        exit_code = subprocess.run(
            [sys.executable, str(root / "tools/ci/lint" / checker), *args],
            cwd=root,
            check=False,
        ).returncode
        if exit_code not in (0, 1):
            print(f"{checker} exited with status {exit_code}", file=sys.stderr)
            status = 2
        else:
            status = max(status, exit_code)
    return status


def main(
    release_build: Path | None = None,
    expected_daemon_sha256: str | None = None,
    *,
    expected_build_receipt_sha256: str | None = None,
    build_native_daemon: bool = False,
    release_cli_build: Path | None = None,
    expected_cli_build_receipt_sha256: str | None = None,
    shipped_bundle: Path | None = None,
    installed_readback: Path | None = None,
    installed_q_approval_request: Path | None = None,
    trust_root: Path = TRUST_ROOT,
    proof_checkout: Path | None = None,
    publish_aggregate: bool = False,
    require_aggregate: bool = False,
    check_migrations: bool = False,
) -> int:
    disposition_exit = 0
    try:
        if (publish_aggregate or require_aggregate) and proof_checkout is None:
            raise ValueError("aggregate qualification requires --proof-checkout")
        if publish_aggregate and require_aggregate:
            raise ValueError("select one aggregate qualification action")
        proof_identity = None
        proof_installation = None
        approved_installation = None
        shipped = any(value is not None for value in (
            release_cli_build, expected_cli_build_receipt_sha256,
            shipped_bundle, installed_readback, installed_q_approval_request, proof_checkout,
        ))
        if check_migrations and (shipped or release_build is not None or build_native_daemon):
            raise ValueError("--check-migrations requires the working-tree inventory mode")
        if shipped:
            if proof_checkout is None:
                raise ValueError("shipped candidate requires --proof-checkout to select committed Q/S and test authority")
            # A shipment either publishes the qualified aggregate or verifies
            # it against the installed candidate's operational proofs.
            require_aggregate = not publish_aggregate
            if expected_daemon_sha256 is not None:
                raise ValueError("proof-bound shipment cannot accept caller-selected Q or S identity")
            if (
                build_native_daemon or release_build is None or expected_build_receipt_sha256 is None
                or release_cli_build is None or expected_cli_build_receipt_sha256 is None
                or shipped_bundle is None or installed_readback is None
                or installed_q_approval_request is None
            ):
                raise ValueError("shipped candidate requires both pinned release builds, signed install readback and Q approval request")
            proof_identity, proof_installation = proof_bound_candidate(
                ROOT, proof_checkout, require_aggregate=require_aggregate,
            )
            expected_daemon_sha256, expected_s_source_head, quanta_source_head = proof_identity
        if release_build is not None and build_native_daemon:
            raise ValueError("select one release observation source")
        if expected_build_receipt_sha256 is not None and release_build is None:
            raise ValueError("expected build receipt SHA-256 requires --release-build")
        if expected_daemon_sha256 is not None:
            if release_build is None and not build_native_daemon:
                raise ValueError("expected daemon SHA-256 requires --release-build or --build-native-daemon")
            if re.fullmatch(r"[0-9a-f]{64}", expected_daemon_sha256) is None:
                raise ValueError("expected daemon SHA-256 must be 64 lowercase hex characters")
        if build_native_daemon:
            findings, daemon_sha256, published = build_and_check_native_daemon(ROOT, expected_daemon_sha256)
            output = (
                f"Native release build has no Tantivy compiler packages, daemon SHA-256 {daemon_sha256}\n"
                f"Verified Native daemon {published}"
            )
        elif release_build is not None:
            if expected_daemon_sha256 is None or expected_build_receipt_sha256 is None:
                raise ValueError("--release-build requires expected daemon and build receipt SHA-256 digests")
            if re.fullmatch(r"[0-9a-f]{64}", expected_build_receipt_sha256) is None:
                raise ValueError("expected build receipt SHA-256 must be 64 lowercase hex characters")
            findings, daemon_sha256, daemon_source = release_build_findings(
                release_build, expected_build_receipt_sha256, installed_linux=shipped,
            )
            require_daemon_digest(daemon_sha256, expected_daemon_sha256)
            if shipped:
                if re.fullmatch(r"[0-9a-f]{64}", expected_cli_build_receipt_sha256) is None:
                    raise ValueError("expected CLI build receipt SHA-256 must be 64 lowercase hex characters")
                if re.fullmatch(r"(?:[0-9a-f]{40}|[0-9a-f]{64})", expected_s_source_head) is None:
                    raise ValueError("expected S source head must be a lowercase Git hash")
                cli_findings, cli_source = release_cli_findings(
                    release_cli_build, expected_cli_build_receipt_sha256
                )
                if cli_source != daemon_source:
                    raise ValueError("release daemon and CLI/SDK source captures differ")
                require_release_source(ROOT, daemon_source, quanta_source_head)
                disposition_exit = check_migration_dispositions(ROOT, quanta_source_head)
                if disposition_exit:
                    return disposition_exit
                approved_installation = shipped_bundle_findings(
                    shipped_bundle, installed_readback, daemon_sha256, expected_s_source_head,
                    approval_request_path=installed_q_approval_request, trust_root=trust_root,
                    paired_checkout=proof_checkout,
                )
                if require_aggregate and approved_installation != proof_installation:
                    raise ValueError("operational proofs differ from the signed installation target")
                findings.extend(cli_findings)
                findings.extend(retirement_findings(ROOT))
                output = f"Shipped Q/S release inventory has no Tantivy compiler packages, daemon SHA-256 {daemon_sha256}"
            else:
                output = f"Observed release build has no Tantivy compiler packages, daemon SHA-256 {daemon_sha256}"
        else:
            if check_migrations:
                disposition_exit = check_migration_dispositions(ROOT)
            findings = retirement_findings(ROOT)
            output = "Tantivy workspace dependency and source inventory passed"
        if publish_aggregate and not findings:
            assert proof_checkout is not None and proof_identity is not None and approved_installation is not None
            publish_qualified_aggregate(proof_checkout, proof_identity, approved_installation)
    except (OSError, subprocess.CalledProcessError, RuntimeError, ValueError, KeyError, TypeError) as error:
        print(f"Tantivy retirement inputs unavailable: {error}", file=sys.stderr)
        return 2
    if findings:
        print("\n".join(findings), file=sys.stderr)
        return max(1, disposition_exit)
    print(output)
    return disposition_exit


if __name__ == "__main__":
    if str(ROOT) not in sys.path:
        sys.path.insert(0, str(ROOT))
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check-migrations", action="store_true", help="check the working-tree test dispositions")
    parser.add_argument("--capture-source", action="store_true", help="emit the clean Q release source inventory")
    parser.add_argument("--select-captured-daemon", action="store_true", help="admit an existing Linux release for staged R5")
    parser.add_argument("--capture-release-build", choices=("daemon", "cli"), help="record an optimized release build")
    parser.add_argument("--evidence-dir", type=Path, help="fresh output directory for a recorded release build")
    parser.add_argument("--build-native-daemon", action="store_true", help="build and check the Native release daemon")
    parser.add_argument("--release-build", type=Path, help="attested SDK60 release evidence directory")
    parser.add_argument("--release-cli-build", type=Path, help="attested CLI build with SDK compiler artifacts")
    parser.add_argument("--shipped-bundle", type=Path, help="installed Semantica Product release root")
    parser.add_argument("--installed-readback", type=Path, help="D's signed installed Product readback")
    parser.add_argument("--installed-q-approval-request", type=Path, help="D's Q approval request with the live selection path")
    parser.add_argument("--proof-checkout", type=Path, help="derive Q/S identity from registered release proofs")
    aggregate = parser.add_mutually_exclusive_group()
    aggregate.add_argument("--publish-aggregate", action="store_true", help="publish the verified release aggregate")
    aggregate.add_argument(
        "--require-aggregate", action="store_true",
        help="verify the existing release aggregate (default for shipment)",
    )
    parser.add_argument(
        "--expected-daemon-sha256",
        help="pin the daemon digest for a compiler-observation check",
    )
    parser.add_argument(
        "--expected-build-receipt-sha256",
        help="require the original build receipt SHA-256 supplied by the release producer",
    )
    parser.add_argument(
        "--expected-cli-build-receipt-sha256",
        help="require the original CLI build receipt SHA-256 supplied by the release producer",
    )
    args = parser.parse_args()
    if args.select_captured_daemon:
        selection_fields = {"select_captured_daemon", "release_build", "expected_build_receipt_sha256"}
        if args.release_build is None or args.expected_build_receipt_sha256 is None or any(
            value for key, value in vars(args).items() if key not in selection_fields
        ):
            parser.error("recorded daemon selection requires --release-build and --expected-build-receipt-sha256")
        try:
            daemon_sha256, binary = select_captured_daemon(ROOT, args.release_build, args.expected_build_receipt_sha256)
        except (OSError, subprocess.CalledProcessError, RuntimeError, ValueError, KeyError, TypeError) as error:
            parser.exit(2, f"recorded release daemon unavailable: {error}\n")
        print(daemon_sha256)
        print(binary)
        raise SystemExit(0)
    if args.capture_release_build is not None or args.evidence_dir is not None:
        if args.capture_release_build is None or args.evidence_dir is None or any(
            value for key, value in vars(args).items() if key not in {"capture_release_build", "evidence_dir"}
        ):
            parser.error("release capture requires --capture-release-build and --evidence-dir without other modes")
        try:
            findings = capture_release_build(ROOT, args.capture_release_build, args.evidence_dir)
        except (OSError, subprocess.CalledProcessError, RuntimeError, ValueError, KeyError, TypeError) as error:
            parser.exit(2, f"Tantivy retirement build capture unavailable: {error}\n")
        if findings:
            parser.exit(1, "\n".join(findings) + "\n")
        receipt = args.evidence_dir / "build-result.json"
        print(f"{receipt} sha256:{file_sha256(receipt)}")
        raise SystemExit(0)
    if args.capture_source:
        if any(value for key, value in vars(args).items() if key != "capture_source"):
            parser.error("--capture-source cannot be combined with an inventory check")
        try:
            print(json.dumps(capture_release_source(ROOT), sort_keys=True, separators=(",", ":")))
        except (OSError, RuntimeError, ValueError) as error:
            parser.exit(2, f"Tantivy retirement source unavailable: {error}\n")
        raise SystemExit(0)
    raise SystemExit(main(
        args.release_build, args.expected_daemon_sha256,
        expected_build_receipt_sha256=args.expected_build_receipt_sha256,
        build_native_daemon=args.build_native_daemon,
        release_cli_build=args.release_cli_build,
        expected_cli_build_receipt_sha256=args.expected_cli_build_receipt_sha256,
        shipped_bundle=args.shipped_bundle, installed_readback=args.installed_readback,
        installed_q_approval_request=args.installed_q_approval_request,
        proof_checkout=args.proof_checkout,
        publish_aggregate=args.publish_aggregate, require_aggregate=args.require_aggregate,
        check_migrations=args.check_migrations,
    ))
