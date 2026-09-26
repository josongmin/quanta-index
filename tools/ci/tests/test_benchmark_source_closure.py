"""Source closure: the benchmark control plane binds normative files only.

The closure engine is exercised on a synthetic profile so the mutation
semantics (changed/added/removed file invalidates; an unrelated planning edit
does not) are proven independently of the live checkout's dirty state.
"""

from __future__ import annotations

import importlib.util
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[3]
CLOSURE_PATH = REPO_ROOT / "tools" / "ci" / "source_closure.py"
PROFILE = "benchmark-control-plane"


def _closure_module():
    spec = importlib.util.spec_from_file_location("benchmark_source_closure", CLOSURE_PATH)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def _git(repo: Path, *args: str) -> None:
    subprocess.run(["git", "-C", str(repo), *args], check=True, capture_output=True)


def _synthetic_repo(tmp_path: Path) -> tuple[Path, object]:
    module = _closure_module()
    repo = tmp_path / "repo"
    (repo / "docs" / "plans" / "sep-26-bench-migration" / "tickets").mkdir(parents=True)
    (repo / "tools" / "benchmark").mkdir(parents=True)
    (repo / "tools" / "benchmark" / "registry.toml").write_text("x\n", encoding="utf-8")
    (repo / "docs" / "plans" / "sep-26-bench-migration" / "tickets" / "INDEX.md").write_text(
        "plan\n", encoding="utf-8"
    )
    module.PROFILES["bm-synthetic"] = {
        "cargo_packages": (),
        "paths": ("tools/benchmark/registry.toml",),
    }
    _git(repo, "init", "-q")
    _git(repo, "config", "user.name", "Closure Test")
    _git(repo, "config", "user.email", "closure@example.invalid")
    _git(repo, "add", "-A")
    _git(repo, "commit", "-qm", "fixture")
    return repo, module


def test_normative_benchmark_files_are_bound() -> None:
    module = _closure_module()
    profile = module.PROFILES[PROFILE]
    paths = set(profile["paths"])
    for required in (
        "tools/benchmark/registry.toml",
        "tools/benchmark/registry.py",
        "tools/benchmark/evidence.py",
        "tools/benchmark/evidence.schema.json",
        "tools/benchmark/evidence_bridge.py",
        "tools/benchmark/benchctl.py",
        "tools/ci/lint/check-benchmark-policy.py",
    ):
        assert required in paths, required
    assert "quanta-index-bench-protocol" in profile["cargo_packages"]


def test_planning_history_is_excluded_from_the_normative_closure() -> None:
    module = _closure_module()
    roots = module.resolve_roots(REPO_ROOT, PROFILE)
    assert not any(root.startswith("docs/plans/sep-26-bench-migration") for root in roots), (
        "a planning-status edit must not invalidate benchmark proof"
    )


def test_every_declared_normative_path_exists() -> None:
    module = _closure_module()
    roots = module.resolve_roots(REPO_ROOT, PROFILE)
    assert "tools/benchmark/registry.toml" in roots
    assert "benchmarks/bench-protocol" in roots
    assert roots == sorted(set(roots))


def test_changed_file_invalidates_the_closure(tmp_path: Path) -> None:
    repo, module = _synthetic_repo(tmp_path)
    module.build_manifest(repo, "bm-synthetic")
    (repo / "tools" / "benchmark" / "registry.toml").write_text("changed\n", encoding="utf-8")
    try:
        module.build_manifest(repo, "bm-synthetic")
    except module.ClosureError as error:
        assert "refusing dirty relevant source" in str(error)
    else:
        raise AssertionError("a changed normative file was captured into a closure")


def test_committed_source_change_invalidates_the_closure(tmp_path: Path) -> None:
    repo, module = _synthetic_repo(tmp_path)
    manifest = module.build_manifest(repo, "bm-synthetic")
    (repo / "tools" / "benchmark" / "registry.toml").write_text("changed\n", encoding="utf-8")
    _git(repo, "add", "-A")
    _git(repo, "commit", "-qm", "change")
    try:
        module.verify_manifest(repo, manifest)
    except module.ClosureError as error:
        assert "revision changed" in str(error)
    else:
        raise AssertionError("a committed source change did not invalidate the closure")


def test_tampered_manifest_digest_is_refused(tmp_path: Path) -> None:
    repo, module = _synthetic_repo(tmp_path)
    manifest = module.build_manifest(repo, "bm-synthetic")
    manifest["files"][0]["sha256"] = "0" * 64
    try:
        module.validate_manifest_shape(manifest)
    except module.ClosureError as error:
        assert "digest mismatch" in str(error)
    else:
        raise AssertionError("a tampered closure digest was accepted")


def test_added_file_invalidates_the_closure(tmp_path: Path) -> None:
    repo, module = _synthetic_repo(tmp_path)
    manifest = module.build_manifest(repo, "bm-synthetic")
    (repo / "tools" / "benchmark" / "extra.py").write_text("new\n", encoding="utf-8")
    _git(repo, "add", "-A")
    _git(repo, "commit", "-qm", "add")
    try:
        module.verify_manifest(repo, manifest)
    except module.ClosureError as error:
        assert "revision changed" in str(error)
    else:
        raise AssertionError("an added normative file did not invalidate the closure")


def test_removed_file_invalidates_the_closure(tmp_path: Path) -> None:
    repo, module = _synthetic_repo(tmp_path)
    manifest = module.build_manifest(repo, "bm-synthetic")
    (repo / "tools" / "benchmark" / "registry.toml").unlink()
    _git(repo, "add", "-A")
    _git(repo, "commit", "-qm", "remove")
    try:
        module.verify_manifest(repo, manifest)
    except module.ClosureError:
        pass
    else:
        raise AssertionError("a removed normative file did not invalidate the closure")


def test_dirty_normative_file_is_refused(tmp_path: Path) -> None:
    repo, module = _synthetic_repo(tmp_path)
    (repo / "tools" / "benchmark" / "registry.toml").write_text("dirty\n", encoding="utf-8")
    try:
        module.build_manifest(repo, "bm-synthetic")
    except module.ClosureError as error:
        assert "refusing dirty relevant source" in str(error)
    else:
        raise AssertionError("a dirty normative file was captured into a closure")


def test_unrelated_planning_edit_does_not_invalidate(tmp_path: Path) -> None:
    repo, module = _synthetic_repo(tmp_path)
    manifest = module.build_manifest(repo, "bm-synthetic")
    (repo / "docs" / "plans" / "sep-26-bench-migration" / "tickets" / "INDEX.md").write_text(
        "plan v2\n", encoding="utf-8"
    )
    module.verify_manifest(repo, manifest)
