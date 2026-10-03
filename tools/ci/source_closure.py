#!/usr/bin/env python3
"""Capture and verify an exact, profile-scoped Git source closure."""

from __future__ import annotations

import argparse
import ast
import hashlib
import json
import os
import stat
import subprocess
from pathlib import Path

SCHEMA_VERSION = 1
PROFILES = {
    "retrieval": {
        "cargo_packages": (
            "quanta-index-retrieval-bench",
            "quanta-index-searchd-runtime",
        ),
        "paths": (
            ".cargo/config.toml",
            "Cargo.lock",
            "Cargo.toml",
            "Justfile",
            "pyproject.toml",
            "uv.lock",
            "benchmarks/retrieval/proof-required-tests.json",
            "docs/adr/MAY-31-001-lancedb-semantic-generation-authority.md",
            "docs/adr/SEP-26-001-retrieval-query-publication-and-result-proof.md",
            "docs/adr/SEP-26-002-retrieval-observation-experiment-and-default-policy.md",
            "docs/adr/SEP-26-003-retrieval-evidence-custody-and-qualification.md",
            "docs/adr/SEP-26-DECISION-REGISTRY.md",
            "docs/adr/SEP-27-003-code-search-source-and-preview-contract.md",
            "docs/adr/SEP-27-004-benchmark-capture-and-resource-custody.md",
            "docs/adr/SEP-27-005-catalog-recovery-supervision-and-proof-custody.md",
            "docs/plans/sep-27-misc/tickets",
            "scripts/cargow",
            "tools/ci/resource_admission.py",
            "tools/ci/tests/test_resource_admission.py",
            "tools/ci/tests/test_cargow_resource_admission.py",
            "scripts/quanta-index-env.sh",
            "rust-toolchain.toml",
            "tools/benchmark/retrieval",
            "tools/benchmark/code_search_workflow.py",
            "tools/benchmark/code_search_matrix.py",
            "tools/ci/tests/test_live_lexical_external.py",
            "tools/ci/tests/test_code_search_workflow.py",
            "tools/ci/tests/test_code_search_matrix.py",
            "tools/ci/lint/check-rust-derive-allowlist.py",
            "tools/ci/lint/rust_attribute_policy.py",
            "tools/ci/lint/check-test-authority.py",
            "tools/ci/nextest_events.py",
            "tools/ci/junit_events.py",
            "tools/ci/source_closure.py",
            "tools/ci/timing/rust_profile_history.py",
            "tools/ci/tests/test_portable_proof.py",
            "tools/ci/tests/test_cargo_preparation.py",
            "tools/ci/tests/test_bootstrap_cache.py",
            "tools/ci/tests/test_proof_command_timings.py",
            "tools/ci/tests/test_lexical_file_comparison.py",
            "tools/ci/tests/test_lexical_five_product_oracle.py",
            "tools/ci/tests/test_retrieval_benchmark.py",
            "tools/ci/tests/test_source_oracle_suite.py",
            "tools/ci/tests/test_codesearchnet_qrels.py",
            "tools/ci/tests/test_codesearchnet_materialize.py",
            "tools/ci/tests/test_clarc_adapter.py",
            "tools/ci/tests/test_external_snippet_benchmark.py",
            "tools/ci/tests/test_holdout_c4_projection.py",
            "tools/ci/tests/test_identifier_robustness_fresh_join.py",
            "tools/ci/tests/test_identifier_robustness_multiproduct_report.py",
            "tools/ci/tests/test_identifier_robustness_strata.py",
            "tools/ci/tests/test_retrieval_contract_proof.py",
            "tools/ci/tests/test_retrieval_sdk_proof.py",
            "tools/ci/tests/test_nextest_ignored_inventory.py",
            "tools/ci/tests/test_write_verification_receipt.py",
            "tools/ci/verification-receipt.schema.json",
            "tools/ci/write-verification-receipt.py",
        ),
    },
    "benchmark-control-plane": {
        # Normative benchmark registration/evidence/CLI surface only. Product
        # source is bound separately through the envelope's Git revision and
        # measured binary digests; the retrieval rail keeps its own profile.
        # The consolidated packet contains normative acceptance contracts.
        # Bind it; unrelated planning/history remains outside this closure.
        "cargo_packages": ("quanta-index-bench-protocol",),
        "paths": (
            ".circleci/config.yml",
            "docs/adr/JUN-08-001-verification-hellgate-and-benchmark-separation.md",
            "docs/adr/SEP-26-003-retrieval-evidence-custody-and-qualification.md",
            "docs/adr/SEP-27-002-single-benchmark-orchestrator-and-typed-evidence.md",
            "docs/adr/SEP-27-004-benchmark-capture-and-resource-custody.md",
            "docs/adr/SEP-27-005-catalog-recovery-supervision-and-proof-custody.md",
            "docs/plans/sep-27-misc/tickets",
            ".cargo/config.toml",
            "Cargo.lock",
            "Cargo.toml",
            "Justfile",
            "pyproject.toml",
            "uv.lock",
            "rust-toolchain.toml",
            "scripts/cargow",
            "tools/ci/resource_admission.py",
            "tools/ci/tests/test_resource_admission.py",
            "tools/ci/tests/test_cargow_resource_admission.py",
            "scripts/quanta-index-env.sh",
            "benchmarks/bench-protocol",
            "tools/benchmark/registry.toml",
            "tools/benchmark/registry.py",
            "tools/benchmark/manifest.py",
            "tools/benchmark/evidence.py",
            "tools/ci/lint/handoff_validation.py",
            "tools/ci/proof_json.py",
            "tools/benchmark/evidence.schema.json",
            "tools/benchmark/evidence_bridge.py",
            "tools/benchmark/native_contracts.py",
            "tools/benchmark/profile_capture.py",
            "tools/benchmark/custody.py",
            "tools/benchmark/corpus_release.py",
            "tools/benchmark/corpus_binding.py",
            "tools/benchmark/code_search_workflow.py",
            "tools/benchmark/code_search_matrix.py",
            "tools/ci/tests/test_live_lexical_external.py",
            "tools/ci/tests/test_code_search_workflow.py",
            "tools/ci/tests/test_code_search_matrix.py",
            "tools/benchmark/retrieval",
            "tools/benchmark/criterion_capture.py",
            "tools/benchmark/producer_execution.py",
            "tools/benchmark/host_monitor.py",
            "tools/benchmark/retrieval_capture.py",
            "tools/benchmark/lexical_capture.py",
            "tools/benchmark/pair_capture.py",
            "tools/benchmark/recorded_capture.py",
            "tools/benchmark/agent_outcome",
            "tools/benchmark/benchctl.py",
            "tools/benchmark/compare_dsl_bench.py",
            "tools/benchmark/quality_integration_summary.py",
            "tools/ci/lint/check-bench-artifacts.py",
            "tools/ci/lint/check-benchmark-policy.py",
            "tools/ci/test-authority.toml",
            "tools/ci/source_closure.py",
            "tools/ci/timing/check_host_contention.py",
            "tools/ci/tests/test_bench_protocol_conformance.py",
            "tools/ci/tests/test_benchmark_evidence_bridge.py",
            "tools/ci/tests/test_check_bench_artifacts.py",
            "tools/ci/tests/test_concurrency_sample_contract.py",
            "tools/ci/tests/test_benchmark_profile_capture.py",
            "tools/ci/tests/test_codesearchnet_qrels.py",
            "tools/ci/tests/test_codesearchnet_materialize.py",
            "tools/ci/tests/test_clarc_adapter.py",
            "tools/ci/tests/test_external_snippet_benchmark.py",
            "tools/ci/tests/test_holdout_c4_projection.py",
            "tools/ci/tests/test_identifier_robustness_fresh_join.py",
            "tools/ci/tests/test_identifier_robustness_multiproduct_report.py",
            "tools/ci/tests/test_identifier_robustness_strata.py",
            "tools/ci/tests/test_producer_notifications.py",
            "tools/ci/tests/test_bootstrap_cache.py",
            "tools/ci/tests/test_proof_command_timings.py",
            "tools/ci/tests/test_criterion_capture.py",
            "tools/ci/tests/test_recorded_capture.py",
            "tools/ci/tests/test_retrieval_capture.py",
            "tools/ci/tests/test_lexical_capture.py",
            "tools/ci/tests/test_pair_capture.py",
            "tools/ci/tests/test_pair_replay_workspace.py",
            "tools/ci/tests/test_cargo_preparation.py",
            "tools/ci/tests/test_corpus_release.py",
            "tools/ci/tests/test_corpus_binding.py",
            "tools/ci/nextest_events.py",
            "tools/ci/tests/test_nextest_ignored_inventory.py",
            "tools/ci/tests/test_tool_custody.py",
            "tools/ci/tests/test_portable_tool_execution.py",
            "tools/ci/tests/test_conditional_window_operations.py",
            "tools/ci/tests/test_conditional_tool_execution.py",
            "tools/ci/tests/test_agent_outcome_benchmark.py",
            "tools/ci/tests/test_benchmark_manifest.py",
            "tools/ci/tests/test_benchmark_policy.py",
            "tools/ci/tests/test_benchmark_source_closure.py",
            "tools/ci/tests/test_benchctl.py",
        ),
    },
}

PROFILES["benchmark-micro"] = {
    "cargo_packages": (
        "quanta-index-bench-protocol",
        "quanta-index-lq-norm",
        "quanta-index-searchd-runtime",
    ),
    "paths": PROFILES["benchmark-control-plane"]["paths"],
}

PROFILES["benchmark-retrieval"] = {
    "cargo_packages": tuple(
        sorted(
            set(PROFILES["retrieval"]["cargo_packages"])
            | set(PROFILES["benchmark-control-plane"]["cargo_packages"])
        )
    ),
    "paths": tuple(
        sorted(
            set(PROFILES["retrieval"]["paths"]) | set(PROFILES["benchmark-control-plane"]["paths"])
        )
    ),
}


class ClosureError(RuntimeError):
    """A source closure cannot be captured or verified."""


class _SourceFrame:
    """One frozen Git tree with bytes independently checked against its blobs."""

    def __init__(self, repo: Path, revision: str):
        self.repo = repo
        self.revision = revision
        try:
            output = subprocess.check_output(
                ["git", "ls-tree", "-rz", "--full-tree", revision], cwd=repo
            )
        except (OSError, subprocess.CalledProcessError) as error:
            raise ClosureError("cannot inspect committed source tree") from error
        self.blobs: dict[str, str] = {}
        self.directories: set[str] = {"."}
        self.bytes: dict[str, bytes] = {}
        self.observations: dict[str, tuple[int, int, int, int]] = {}
        for record in output.split(b"\0"):
            if not record:
                continue
            try:
                header, raw_path = record.split(b"\t", 1)
                mode, kind, digest = header.decode().split()
                path = raw_path.decode()
            except (ValueError, UnicodeError) as error:
                raise ClosureError("malformed committed source tree inventory") from error
            if kind != "blob" or mode not in {"100644", "100755"}:
                continue
            self.blobs[path] = digest
            self.directories.update(parent.as_posix() for parent in Path(path).parents)

    def read(self, path: Path) -> bytes:
        relative = path.relative_to(self.repo).as_posix()
        if relative not in self.bytes:
            expected = self.blobs.get(relative)
            if expected is None:
                raise ClosureError(f"source dependency is not a committed regular file: {relative}")
            try:
                before = self._state(path)
                data = path.read_bytes()
                after = self._state(path)
            except OSError as error:
                raise ClosureError(
                    f"cannot read committed source dependency {relative}: {error}"
                ) from error
            if before != after:
                raise ClosureError(f"source changed while reading: {relative}")
            header = b"blob " + str(len(data)).encode() + b"\0"
            if hashlib.sha1(header + data).hexdigest() != expected:
                raise ClosureError(
                    f"refusing dirty relevant source: differs from committed HEAD: {relative}"
                )
            self.bytes[relative] = data
            self.observations[relative] = after
        return self.bytes[relative]

    @staticmethod
    def _state(path: Path) -> tuple[int, int, int, int]:
        info = path.stat()
        return info.st_ino, info.st_size, info.st_mtime_ns, info.st_ctime_ns

    def recheck(self) -> None:
        for relative, observed in self.observations.items():
            try:
                current = self._state(self.repo / relative)
            except OSError as error:
                raise ClosureError(
                    f"cannot recheck source dependency {relative}: {error}"
                ) from error
            if current != observed:
                raise ClosureError(f"source changed during closure operation: {relative}")


def _git(repo: Path, *args: str) -> str:
    try:
        return subprocess.check_output(
            ["git", *args], cwd=repo, text=True, stderr=subprocess.STDOUT
        ).strip()
    except subprocess.CalledProcessError as error:
        raise ClosureError(error.output.strip() or f"git {' '.join(args)} failed") from error


def _repo_root(start: Path | None = None) -> Path:
    return Path(_git((start or Path.cwd()).resolve(), "rev-parse", "--show-toplevel")).resolve()


def _canonical(payload: object) -> bytes:
    return json.dumps(payload, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()


def _digest(payload: object) -> str:
    return hashlib.sha256(_canonical(payload)).hexdigest()


def _metadata(repo: Path) -> dict:
    wrapper = repo / "scripts" / "cargow"
    try:
        raw = subprocess.check_output(
            [
                str(wrapper),
                "--lane",
                "metadata-lane",
                "metadata",
                "--format-version",
                "1",
                "--locked",
            ],
            cwd=repo,
        )
    except subprocess.CalledProcessError as error:
        raise ClosureError("cargo metadata failed while resolving source closure") from error
    try:
        payload = json.loads(raw)
    except json.JSONDecodeError as error:
        raise ClosureError("cargo metadata returned invalid JSON") from error
    if not isinstance(payload, dict):
        raise ClosureError("cargo metadata must return an object")
    return payload


def _cargo_roots(repo: Path, package_names: tuple[str, ...]) -> set[str]:
    if not package_names:
        return set()
    metadata = _metadata(repo)
    packages = metadata.get("packages")
    resolve = metadata.get("resolve")
    nodes = resolve.get("nodes") if isinstance(resolve, dict) else None
    if not isinstance(packages, list) or not isinstance(nodes, list):
        raise ClosureError("cargo metadata omitted packages or resolve nodes")

    def unique_records(records: list, label: str) -> dict[str, dict]:
        indexed = {}
        for record in records:
            if (
                not isinstance(record, dict)
                or not isinstance(record.get("id"), str)
                or not record["id"]
            ):
                raise ClosureError(f"cargo metadata has invalid {label} id")
            identity = record["id"]
            if identity in indexed:
                raise ClosureError(f"cargo metadata has duplicate {label} id: {identity}")
            indexed[identity] = record
        return indexed

    by_id = unique_records(packages, "package")
    node_by_id = unique_records(nodes, "resolve node")
    if any(not isinstance(package.get("name"), str) or not package["name"] for package in packages):
        raise ClosureError("cargo metadata has invalid package name")
    wanted = {
        package["id"]
        for package in packages
        if isinstance(package, dict)
        and package.get("name") in package_names
        and package.get("source") is None
    }
    found_names = {by_id[package_id]["name"] for package_id in wanted}
    missing = sorted(set(package_names) - found_names)
    if missing:
        raise ClosureError(f"cargo packages missing from workspace: {', '.join(missing)}")
    pending = list(wanted)
    closure: set[str] = set()
    visited: set[str] = set()
    while pending:
        package_id = pending.pop()
        if package_id in visited:
            continue
        package = by_id.get(package_id)
        node = node_by_id.get(package_id)
        if package is None:
            raise ClosureError(f"cargo dependency package missing from metadata: {package_id}")
        if node is None:
            kind = "local" if package.get("source") is None else "external"
            raise ClosureError(f"cargo resolve node missing for {kind} package: {package_id}")
        visited.add(package_id)
        # A registry dependency may resolve through a local [patch] package.
        # Traverse every edge, but bind only repository-owned source roots.
        if package.get("source") is None:
            closure.add(package_id)
        dependencies = node.get("dependencies")
        if not isinstance(dependencies, list):
            raise ClosureError(f"cargo resolve node has invalid dependencies: {package_id}")
        pending.extend(dependencies)

    roots: set[str] = set()
    for package_id in closure:
        manifest_path = Path(by_id[package_id]["manifest_path"])
        manifest = manifest_path.resolve()
        if manifest_path != manifest:
            raise ClosureError(f"source closure refuses aliased cargo manifest: {manifest_path}")
        try:
            relative = manifest.parent.relative_to(repo).as_posix()
        except ValueError as error:
            raise ClosureError(f"local cargo package escaped repository: {manifest}") from error
        roots.add(relative or ".")
    return roots


def resolve_roots(
    repo: Path,
    profile: str,
    extra_paths: tuple[str, ...] = (),
    *,
    frame: _SourceFrame | None = None,
) -> list[str]:
    if profile not in PROFILES:
        raise ClosureError(f"unknown source closure profile: {profile}")
    config = PROFILES[profile]
    roots = set(config["paths"])
    roots.update(extra_paths)
    normalized: set[str] = set()
    for value in roots:
        lexical = repo / value
        candidate = lexical.resolve()
        if lexical != candidate:
            raise ClosureError(f"source closure refuses aliased source root: {value}")
        try:
            relative = candidate.relative_to(repo).as_posix()
        except ValueError as error:
            raise ClosureError(f"source root escaped repository: {value}") from error
        if not candidate.exists() and not candidate.is_symlink():
            raise ClosureError(f"source root does not exist: {relative}")
        normalized.add(relative)
        if frame is not None and candidate.is_file():
            frame.read(candidate)
    if frame is not None and config["cargo_packages"]:
        # Cargo metadata reads workspace manifests before choosing the local
        # dependency graph. Freeze those inputs before invoking the resolver.
        for path in frame.blobs:
            if path == "Cargo.toml" or path.endswith("/Cargo.toml"):
                frame.read(repo / path)
    for value in _cargo_roots(repo, config["cargo_packages"]):
        lexical = repo / value
        candidate = lexical.resolve()
        if lexical != candidate:
            raise ClosureError(f"source closure refuses aliased source root: {value}")
        try:
            relative = candidate.relative_to(repo).as_posix()
        except ValueError as error:
            raise ClosureError(f"source root escaped repository: {value}") from error
        if not candidate.exists() and not candidate.is_symlink():
            raise ClosureError(f"source root does not exist: {relative}")
        normalized.add(relative)
    normalized.update(_python_import_roots(repo, sorted(normalized), frame=frame))
    return sorted(normalized)


def _python_import_roots(
    repo: Path, roots: list[str], *, frame: _SourceFrame | None = None
) -> set[str]:
    """Close static local imports without executing Python or importing packages.

    External dependencies remain bound by the declared lockfile. Dynamic
    importlib/__import__ paths remain the caller's explicit normative roots.
    Both repository and sibling candidates are bound for bare script imports;
    this covers the repository's script and package invocation front doors.
    """
    pending = [
        repo / path for path in _files(repo, roots, validate_files=False) if path.endswith(".py")
    ]
    visited: set[str] = set()
    wildcard_packages: set[str] = set()

    def kind(path: Path, wanted: str) -> bool:
        if frame is not None:
            relative = path.relative_to(repo).as_posix()
            if relative in (frame.blobs if wanted == "file" else frame.directories):
                return True
        try:
            mode = path.stat().st_mode
        except FileNotFoundError:
            return False
        except OSError as error:
            raise ClosureError(
                f"cannot inspect Python source dependency {path}: {error}"
            ) from error
        return stat.S_ISREG(mode) if wanted == "file" else stat.S_ISDIR(mode)

    def local_files(base: Path, parts: tuple[str, ...]) -> set[Path]:
        candidates: set[Path] = set()
        package = base
        for part in parts:
            child = package / part
            if not kind(child, "directory") and not kind(child.with_suffix(".py"), "file"):
                break
            init = package / "__init__.py"
            if package != repo and kind(init, "file"):
                candidates.add(init)
            package = child
            init = package / "__init__.py"
            if kind(init, "file"):
                candidates.add(init)
            if kind(package.with_suffix(".py"), "file"):
                candidates.add(package.with_suffix(".py"))
        return candidates

    def enqueue(paths: set[Path]) -> None:
        for candidate in paths:
            try:
                relative = candidate.relative_to(repo)
                resolved = candidate.resolve().relative_to(repo)
            except ValueError as error:
                raise ClosureError(f"Python import escaped repository: {candidate}") from error
            if relative != resolved or candidate.is_symlink():
                raise ClosureError(f"source closure refuses symlinked Python import: {relative}")
            if relative.as_posix() not in visited:
                pending.append(candidate)

    def wildcard_package(package: Path) -> None:
        if not kind(package, "directory"):
            return
        try:
            relative = package.relative_to(repo).as_posix()
            resolved = package.resolve().relative_to(repo).as_posix()
        except ValueError as error:
            raise ClosureError(f"wildcard Python import escaped repository: {package}") from error
        if relative != resolved or package.is_symlink():
            raise ClosureError(f"source closure refuses symlinked Python package: {package}")
        if relative in wildcard_packages:
            return
        wildcard_packages.add(relative)

        def fail(error: OSError) -> None:
            raise ClosureError(f"cannot inventory wildcard Python package: {error}") from error

        for directory, directories, names in os.walk(package, onerror=fail):
            if any((Path(directory) / name).is_symlink() for name in directories):
                raise ClosureError(
                    f"source closure refuses symlinked Python package directory: {directory}"
                )
            enqueue({Path(directory) / name for name in names if name.endswith(".py")})

    while pending:
        source = pending.pop()
        relative = source.relative_to(repo).as_posix()
        if relative in visited:
            continue
        visited.add(relative)
        package = source.parent
        while package != repo:
            init = package / "__init__.py"
            if kind(init, "file"):
                enqueue({init})
            package = package.parent
        try:
            data = frame.read(source) if frame is not None else source.read_bytes()
            tree = ast.parse(data, filename=relative)
        except (OSError, SyntaxError, UnicodeError, ValueError) as error:
            raise ClosureError(
                f"cannot parse Python source dependency {relative}: {error}"
            ) from error
        for node in ast.walk(tree):
            if isinstance(node, ast.Import):
                for alias in node.names:
                    parts = tuple(alias.name.split("."))
                    enqueue(local_files(repo, parts) | local_files(source.parent, parts))
            elif isinstance(node, ast.ImportFrom):
                parts = tuple(node.module.split(".")) if node.module else ()
                if node.level:
                    base = source.parent
                    for _ in range(node.level - 1):
                        base = base.parent
                    try:
                        base.relative_to(repo)
                    except ValueError as error:
                        raise ClosureError(
                            f"relative Python import escaped repository: {relative}"
                        ) from error
                    bases = {base}
                else:
                    bases = {repo, source.parent}
                for base in bases:
                    enqueue(local_files(base, parts))
                    for alias in node.names:
                        if alias.name == "*":
                            # __all__ can ask import-star to load a submodule
                            # that __init__ never explicitly imports. Bind the
                            # local package subtree instead of trusting exports.
                            wildcard_package(base.joinpath(*parts))
                        else:
                            enqueue(local_files(base, (*parts, alias.name)))

    # Git's exclude-standard can hide a Python helper that is still executable.
    # Refuse that evidence rather than recording an incomplete source closure.
    if visited:
        inventoried = set(_files(repo, sorted(visited)))
        missing = visited - inventoried
        if missing:
            raise ClosureError(
                f"cannot inventory imported Python source: {', '.join(sorted(missing))}"
            )
    return visited | wildcard_packages


def _files(repo: Path, roots: list[str], *, validate_files: bool = True) -> list[str]:
    output = subprocess.check_output(
        ["git", "ls-files", "-z", "--cached", "--others", "--exclude-standard", "--", *roots],
        cwd=repo,
    )
    paths = sorted({part.decode() for part in output.split(b"\0") if part})
    if not paths:
        raise ClosureError("source closure contains no files")
    if not validate_files:
        return paths
    for path in paths:
        candidate = repo / path
        if candidate.is_symlink():
            raise ClosureError(f"source closure refuses symlink: {path}")
        if not candidate.is_file():
            raise ClosureError(f"source closure path is not a regular file: {path}")
    return paths


def _assert_clean(repo: Path, roots: list[str]) -> None:
    status = _git(repo, "status", "--porcelain=v1", "--untracked-files=all", "--", *roots)
    if status:
        sample = ", ".join(status.splitlines()[:8])
        raise ClosureError(f"refusing dirty relevant source: {sample}")


def build_manifest(repo: Path, profile: str) -> dict:
    revision = _git(repo, "rev-parse", "HEAD")
    frame = _SourceFrame(repo, revision)
    roots = resolve_roots(repo, profile, frame=frame)
    _assert_clean(repo, roots)
    paths = _files(repo, roots)
    entries = [
        {"path": path, "sha256": hashlib.sha256(frame.read(repo / path)).hexdigest()}
        for path in paths
    ]
    _assert_clean(repo, roots)
    if _git(repo, "rev-parse", "HEAD") != revision:
        raise ClosureError("source closure revision changed during capture")
    if _files(repo, roots) != paths:
        raise ClosureError("source closure file set changed during capture")
    if not _python_import_roots(repo, roots, frame=frame) <= set(roots):
        raise ClosureError("source closure imports changed during capture")
    frame.recheck()
    core = {
        "schema_version": SCHEMA_VERSION,
        "profile": profile,
        "revision": revision,
        "roots": roots,
        "files": entries,
    }
    return {**core, "digest": _digest(core)}


def validate_manifest_shape(payload: object) -> dict:
    keys = {"schema_version", "profile", "revision", "roots", "files", "digest"}
    if not isinstance(payload, dict) or set(payload) != keys:
        raise ClosureError(f"source closure must hold exactly {sorted(keys)}")
    if type(payload["schema_version"]) is not int or payload["schema_version"] != SCHEMA_VERSION:
        raise ClosureError(f"source closure schema_version must be {SCHEMA_VERSION}")
    if not isinstance(payload["profile"], str) or payload["profile"] not in PROFILES:
        raise ClosureError("source closure has unknown profile")
    if (
        not isinstance(payload["revision"], str)
        or len(payload["revision"]) != 40
        or any(ch not in "0123456789abcdef" for ch in payload["revision"])
    ):
        raise ClosureError("source closure revision must be a full Git SHA")
    roots = payload["roots"]
    if (
        not isinstance(roots, list)
        or not roots
        or any(not isinstance(root, str) or not root for root in roots)
        or roots != sorted(set(roots))
    ):
        raise ClosureError("source closure roots must be a nonempty sorted unique list")
    files = payload["files"]
    if not isinstance(files, list) or not files:
        raise ClosureError("source closure files must be a nonempty list")
    paths: list[str] = []
    for index, entry in enumerate(files):
        if not isinstance(entry, dict) or set(entry) != {"path", "sha256"}:
            raise ClosureError(f"source closure files[{index}] has invalid shape")
        if not isinstance(entry["path"], str) or not entry["path"]:
            raise ClosureError(f"source closure files[{index}].path is invalid")
        if (
            not isinstance(entry["sha256"], str)
            or len(entry["sha256"]) != 64
            or any(ch not in "0123456789abcdef" for ch in entry["sha256"])
        ):
            raise ClosureError(f"source closure files[{index}].sha256 is invalid")
        paths.append(entry["path"])
    if paths != sorted(set(paths)):
        raise ClosureError("source closure files must be sorted and unique")
    core = {
        key: payload[key] for key in ("schema_version", "profile", "revision", "roots", "files")
    }
    if payload["digest"] != _digest(core):
        raise ClosureError("source closure digest mismatch")
    return payload


def verify_manifest(repo: Path, payload: object) -> dict:
    manifest = validate_manifest_shape(payload)
    revision = _git(repo, "rev-parse", "HEAD")
    if revision != manifest["revision"]:
        raise ClosureError("source closure revision changed")
    frame = _SourceFrame(repo, revision)
    current_roots = resolve_roots(repo, manifest["profile"], frame=frame)
    if current_roots != manifest["roots"]:
        raise ClosureError("source closure roots changed")
    _assert_clean(repo, current_roots)
    current_paths = _files(repo, current_roots)
    expected_paths = [entry["path"] for entry in manifest["files"]]
    if current_paths != expected_paths:
        raise ClosureError("source closure file set changed")
    for entry in manifest["files"]:
        actual = hashlib.sha256(frame.read(repo / entry["path"])).hexdigest()
        if actual != entry["sha256"]:
            raise ClosureError(f"source closure file digest changed: {entry['path']}")
    _assert_clean(repo, current_roots)
    if _git(repo, "rev-parse", "HEAD") != revision:
        raise ClosureError("source closure revision changed during verification")
    if _files(repo, current_roots) != current_paths:
        raise ClosureError("source closure file set changed during verification")
    if not _python_import_roots(repo, current_roots, frame=frame) <= set(current_roots):
        raise ClosureError("source closure imports changed during verification")
    frame.recheck()
    return manifest


def load_and_verify(path: Path, repo: Path | None = None) -> dict:
    def object_pairs(pairs: list[tuple[str, object]]) -> dict:
        result = {}
        for key, value in pairs:
            if key in result:
                raise ClosureError(f"duplicate source closure JSON key: {key}")
            result[key] = value
        return result

    def reject_constant(value: str) -> object:
        raise ClosureError(f"non-finite source closure JSON value: {value}")

    try:
        payload = json.loads(
            path.read_bytes(), object_pairs_hook=object_pairs, parse_constant=reject_constant
        )
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ClosureError(f"cannot read source closure {path}: {error}") from error
    return verify_manifest(repo or _repo_root(), payload)


def _write_exclusive(path: Path, payload: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    data = json.dumps(payload, sort_keys=True, indent=2) + "\n"
    try:
        with path.open("x", encoding="utf-8") as stream:
            stream.write(data)
            stream.flush()
            os.fsync(stream.fileno())
    except FileExistsError as error:
        raise ClosureError(f"refusing existing source closure: {path}") from error


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)
    for name in ("check", "capture"):
        child = subparsers.add_parser(name)
        child.add_argument("--profile", required=True, choices=sorted(PROFILES))
        if name == "capture":
            child.add_argument("--out", required=True, type=Path)
    verify = subparsers.add_parser("verify")
    verify.add_argument("--manifest", required=True, type=Path)
    args = parser.parse_args()
    try:
        repo = _repo_root()
        if args.command == "verify":
            manifest = load_and_verify(args.manifest, repo)
        else:
            manifest = build_manifest(repo, args.profile)
            if args.command == "capture":
                _write_exclusive(args.out, manifest)
    except ClosureError as error:
        raise SystemExit(str(error)) from error
    print(
        f"source closure {args.command} ok: {manifest['profile']} "
        f"{len(manifest['files'])} files {manifest['digest']}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
