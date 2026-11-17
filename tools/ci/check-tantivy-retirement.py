#!/usr/bin/env python3
"""Check Tantivy residue in the workspace or the actual release compiler artifacts."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import subprocess
import sys
from pathlib import Path

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - Python < 3.11
    import tomli as tomllib  # type: ignore[no-redef]


ROOT = Path(__file__).resolve().parents[2]
WORKSPACE_PACKAGE = "quanta-index-searchd-runtime"
DAEMON_BINARY = "quanta-index-searchd"


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
    with path.open("rb") as stream:
        return tomllib.load(stream)


def file_sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def release_build_findings(root: Path, evidence: Path) -> tuple[list[str], str]:
    """Check a source-stable SDK60 release build's captured Cargo artifacts."""
    evidence = evidence.resolve(strict=True)
    receipt = json.loads((evidence / "build-result.json").read_bytes())
    if (
        receipt["status"] != "VERIFIED"
        or receipt["exit_code"] != 0
        or receipt["source_before_after_equal"] is not True
    ):
        raise ValueError("release build has no source-stable successful receipt")
    command = receipt["command"]
    if not isinstance(command, list) or not {
        "build", "--release", "--locked", "--offline",
        "--message-format=json-render-diagnostics",
    }.issubset(command):
        raise ValueError("release build has no locked Cargo artifact command")
    before = (evidence / "source-before.json").read_bytes()
    if before != (evidence / "source-after-build.json").read_bytes():
        raise ValueError("release build source captures differ")
    if not json.loads(before)["files"]:
        raise ValueError("release build source capture is empty")

    daemon = receipt["binaries"][DAEMON_BINARY]
    binary = Path(daemon["path"])
    if binary != evidence / DAEMON_BINARY or binary.is_symlink() or not binary.is_file():
        raise ValueError("retained daemon path differs from the release evidence directory")
    if file_sha256(binary) != daemon["sha256"]:
        raise ValueError("retained daemon digest differs from the build receipt")

    return (
        compiler_artifact_findings(root, evidence / "build-artifacts.jsonl", daemon["compiler_artifact"]),
        daemon["sha256"],
    )


def compiler_artifact_findings(root: Path, artifacts: Path, compiler_executable: str) -> list[str]:
    """Check the successful Cargo stream and the exact optimized daemon artifact."""
    metadata = json.loads(cargo_output(root, "metadata", "--format-version", "1", "--all-features"))
    names = {package["id"]: package["name"] for package in metadata["packages"]}
    packages = set()
    daemon_events = 0
    finished = False
    manifest = root / "crates" / WORKSPACE_PACKAGE / "Cargo.toml"
    with artifacts.open("rb") as log:
        for line in log:
            if finished:
                raise ValueError("release build has output after its Cargo terminal event")
            row = json.loads(line)
            if not isinstance(row, dict):
                raise ValueError("release Cargo event is not an object")
            if row.get("reason") == "build-finished":
                if row.get("success") is not True:
                    raise ValueError("release Cargo build failed")
                finished = True
            if row.get("reason") != "compiler-artifact":
                continue
            name = names[row["package_id"]]
            packages.add(name)
            if name == WORKSPACE_PACKAGE and row["target"]["name"] == DAEMON_BINARY:
                if (
                    row["manifest_path"] != str(manifest)
                    or row["target"]["kind"] != ["bin"]
                    or row["profile"]["test"] is not False
                    or row["profile"]["opt_level"] not in ("1", "2", "3", "s", "z")
                    or row["executable"] != compiler_executable
                ):
                    raise ValueError("daemon compiler artifact differs from the release receipt")
                daemon_events += 1
    if not finished or daemon_events != 1:
        raise ValueError("release build lacks one daemon artifact and successful Cargo terminal")
    return [f"Release compiler artifact: {name}" for name in sorted(packages) if tantivy_package(name)]


def retirement_findings(root: Path) -> list[str]:
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

    for entry in sorted((root / "vendor").glob("*")):
        manifest = entry / "Cargo.toml"
        if tantivy_package(entry.name) or (
            manifest.is_file() and tantivy_package(read_toml(manifest)["package"]["name"])
        ):
            findings.append(f"vendor: {entry.relative_to(root)}")
    return findings


def main(
    release_build: Path | None = None,
    expected_daemon_sha256: str | None = None,
    *,
    compiler_artifacts: Path | None = None,
    built_daemon: Path | None = None,
) -> int:
    try:
        if (compiler_artifacts is None) != (built_daemon is None):
            raise ValueError("--compiler-artifacts and --built-daemon must be supplied together")
        if release_build is not None and compiler_artifacts is not None:
            raise ValueError("select either release evidence or the current compiler artifacts")
        if expected_daemon_sha256 is not None:
            if release_build is None and compiler_artifacts is None:
                raise ValueError("expected daemon SHA-256 requires --release-build or --compiler-artifacts")
            if re.fullmatch(r"[0-9a-f]{64}", expected_daemon_sha256) is None:
                raise ValueError("expected daemon SHA-256 must be 64 lowercase hex characters")
        daemon_sha256 = None
        if compiler_artifacts is not None and built_daemon is not None:
            if not built_daemon.is_absolute() or built_daemon.is_symlink() or not built_daemon.is_file():
                raise ValueError("built daemon must be an absolute regular file")
            findings = compiler_artifact_findings(ROOT, compiler_artifacts, str(built_daemon))
            daemon_sha256 = file_sha256(built_daemon)
            output = daemon_sha256
        elif release_build is not None:
            findings, daemon_sha256 = release_build_findings(ROOT, release_build)
            output = f"Observed release build has no Tantivy compiler packages, daemon SHA-256 {daemon_sha256}"
        else:
            findings = retirement_findings(ROOT)
            output = "Tantivy retirement workspace check passed"
        if expected_daemon_sha256 is not None and daemon_sha256 != expected_daemon_sha256:
            raise ValueError(
                f"release daemon SHA-256 {daemon_sha256} differs from expected {expected_daemon_sha256}"
            )
    except (OSError, subprocess.CalledProcessError, ValueError, KeyError, TypeError) as error:
        print(f"Tantivy retirement inputs unavailable: {error}", file=sys.stderr)
        return 2
    if findings:
        print("\n".join(findings), file=sys.stderr)
        return 1
    print(output)
    return 0


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--release-build", type=Path, help="existing SDK60 release evidence directory")
    parser.add_argument(
        "--expected-daemon-sha256",
        help="require the release daemon SHA-256 supplied by the installed candidate verifier",
    )
    parser.add_argument("--compiler-artifacts", type=Path, help="Cargo artifact stream from the current build")
    parser.add_argument("--built-daemon", type=Path, help="current executable whose SHA-256 is printed on success")
    args = parser.parse_args()
    raise SystemExit(main(
        args.release_build, args.expected_daemon_sha256,
        compiler_artifacts=args.compiler_artifacts, built_daemon=args.built_daemon,
    ))
