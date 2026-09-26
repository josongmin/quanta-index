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

import pytest

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
        "tools/benchmark/native_contracts.py",
        "uv.lock",
        "scripts/quanta-index-env.sh",
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


@pytest.mark.parametrize(
    ("profile", "dependency"),
    [
        ("benchmark-control-plane", "tools/benchmark/native_contracts.py"),
        ("benchmark-control-plane", "uv.lock"),
        ("benchmark-control-plane", "scripts/quanta-index-env.sh"),
        ("retrieval", "tools/ci/lint/rust_attribute_policy.py"),
    ],
)
def test_actual_profile_rejects_dirty_transitive_validation_dependency(
    tmp_path: Path, monkeypatch, profile: str, dependency: str
) -> None:
    module = _closure_module()
    repo = tmp_path / "repo"
    repo.mkdir()
    for path in (*module.PROFILES[profile]["paths"], dependency):
        target = repo / path
        if target.suffix or target.name in {"Justfile", "cargow"}:
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(
                "# source fixture\n" if target.suffix == ".py" else "source fixture\n"
            )
        else:
            target.mkdir(parents=True, exist_ok=True)
            (target / "fixture.txt").write_text("source fixture\n")
    _git(repo, "init", "-q")
    _git(repo, "config", "user.name", "Closure oracle")
    _git(repo, "config", "user.email", "closure@example.invalid")
    _git(repo, "add", "-A")
    _git(repo, "commit", "-qm", "source fixture")
    # Cargo traversal has its own contract. Exercise the actual named profile's
    # explicit validator/dependency boundary using real Git cleanliness checks.
    monkeypatch.setattr(module, "_cargo_roots", lambda *_args: set())
    manifest = module.build_manifest(repo, profile)
    (repo / dependency).write_text(
        "# changed validation semantics\n"
        if dependency.endswith(".py")
        else "changed validation semantics\n"
    )
    with pytest.raises(module.ClosureError, match="dirty relevant source"):
        module.verify_manifest(repo, manifest)


def _python_repo(tmp_path: Path) -> tuple[Path, object]:
    repo, module = _synthetic_repo(tmp_path)
    sources = {
        "tools/entry.py": "from tools.pkg.api import answer\nimport sibling\n",
        "tools/sibling.py": "from tools.ci import junit_events\n",
        "tools/pkg/__init__.py": "from . import boot\n",
        "tools/pkg/boot.py": "BOOT = True\n",
        "tools/pkg/api.py": "from . import helper\nfrom ..ci import junit_events\nanswer = helper.answer\n",
        "tools/pkg/helper.py": "answer = 1\ndef cycle():\n    from tools.pkg.api import answer\n    return answer\n",
        "tools/ci/junit_events.py": "import json\nVALUE = 1\n",
        "tools/unrelated.py": "UNRELATED = 1\n",
    }
    for name, text in sources.items():
        target = repo / name
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(text)
    module.PROFILES["python-synthetic"] = {"cargo_packages": (), "paths": ("tools/entry.py",)}
    _git(repo, "add", "-A")
    _git(repo, "commit", "-qm", "Python dependency graph")
    return repo, module


@pytest.mark.parametrize(
    "helper",
    [
        "tools/sibling.py",
        "tools/pkg/__init__.py",
        "tools/pkg/boot.py",
        "tools/pkg/helper.py",
        "tools/ci/junit_events.py",
    ],
)
@pytest.mark.parametrize("mutation", ["change", "remove"])
def test_real_git_manifest_rejects_static_transitive_python_mutation(tmp_path, helper, mutation):
    repo, module = _python_repo(tmp_path)
    manifest = module.build_manifest(repo, "python-synthetic")
    path = repo / helper
    if mutation == "remove":
        path.unlink()
    else:
        path.write_text("# execution semantics changed\n")
    with pytest.raises(module.ClosureError):
        module.verify_manifest(repo, manifest)


def test_unrelated_python_edit_does_not_invalidate_static_import_closure(tmp_path):
    repo, module = _python_repo(tmp_path)
    manifest = module.build_manifest(repo, "python-synthetic")
    (repo / "tools/unrelated.py").write_text("UNRELATED = 2\n")
    assert module.verify_manifest(repo, manifest) == manifest


def test_adding_previously_absent_namespace_child_invalidates_manifest(tmp_path):
    repo, module = _python_repo(tmp_path)
    source = repo / "tools/entry.py"
    source.write_text("from tools.ci import future_helper\n")
    _git(repo, "add", "-A")
    _git(repo, "commit", "-qm", "namespace child import")
    manifest = module.build_manifest(repo, "python-synthetic")
    (repo / "tools/ci/future_helper.py").write_text("VALUE = 1\n")
    with pytest.raises(module.ClosureError):
        module.verify_manifest(repo, manifest)


def test_ignored_executable_python_helper_cannot_be_omitted_from_manifest(tmp_path):
    repo, module = _python_repo(tmp_path)
    source = repo / "tools/entry.py"
    source.write_text("import ignored_helper\n")
    (repo / ".gitignore").write_text("tools/ignored_helper.py\n")
    _git(repo, "add", "-A")
    _git(repo, "commit", "-qm", "ignored helper declaration")
    (repo / "tools/ignored_helper.py").write_text("VALUE = 1\n")
    with pytest.raises(module.ClosureError, match="contains no files|cannot inventory"):
        module.build_manifest(repo, "python-synthetic")


def test_malformed_or_unreadable_imported_python_is_not_certified(tmp_path, monkeypatch):
    repo, module = _python_repo(tmp_path)
    path = repo / "tools/pkg/helper.py"
    path.write_text("def broken(:\n")
    _git(repo, "add", "-A")
    _git(repo, "commit", "-qm", "malformed Python source")
    with pytest.raises(module.ClosureError, match="cannot parse Python source"):
        module.build_manifest(repo, "python-synthetic")
    path.write_text("VALUE = 1\n")
    _git(repo, "add", "-A")
    _git(repo, "commit", "-qm", "restore Python source")
    original = Path.read_bytes

    def unreadable(candidate):
        if candidate == path:
            raise PermissionError("fixture source read denied")
        return original(candidate)

    monkeypatch.setattr(Path, "read_bytes", unreadable)
    with pytest.raises(module.ClosureError, match="fixture source read denied"):
        module.build_manifest(repo, "python-synthetic")


@pytest.mark.parametrize(
    "field,value",
    [
        ("schema_version", True),
        ("revision", "g" * 40),
        ("roots", [{}]),
        ("files", [{"path": {}, "sha256": "0" * 64}]),
    ],
)
def test_real_manifest_rejects_wrong_types_and_nonhex_revision(tmp_path, field, value):
    repo, module = _synthetic_repo(tmp_path)
    manifest = module.build_manifest(repo, "bm-synthetic")
    manifest[field] = value
    manifest["digest"] = module._digest(
        {key: val for key, val in manifest.items() if key != "digest"}
    )
    with pytest.raises(module.ClosureError):
        module.validate_manifest_shape(manifest)
    with pytest.raises(module.ClosureError):
        module.verify_manifest(repo, manifest)


def test_real_manifest_load_rejects_duplicate_json_keys(tmp_path):
    import json

    repo, module = _synthetic_repo(tmp_path)
    manifest = module.build_manifest(repo, "bm-synthetic")
    raw = json.dumps(manifest)
    path = tmp_path / "manifest.json"
    path.write_text(raw[:-1] + ', "schema_version": 1}')
    with pytest.raises(module.ClosureError, match="duplicate source closure JSON key"):
        module.load_and_verify(path, repo)


def test_symlinked_local_import_is_refused_even_when_target_is_regular(tmp_path):
    repo, module = _python_repo(tmp_path)
    helper = repo / "tools/pkg/helper.py"
    helper.unlink()
    helper.symlink_to(repo / "tools/pkg/boot.py")
    with pytest.raises(module.ClosureError, match="symlink"):
        module.build_manifest(repo, "python-synthetic")


def test_package_initializer_is_bound_when_entry_has_no_imports(tmp_path):
    repo, module = _python_repo(tmp_path)
    source = repo / "tools/pkg/api.py"
    source.write_text("answer = 1\n")
    module.PROFILES["python-synthetic"]["paths"] = ("tools/pkg/api.py",)
    _git(repo, "add", "-A")
    _git(repo, "commit", "-qm", "package entry")
    manifest = module.build_manifest(repo, "python-synthetic")
    (repo / "tools/pkg/__init__.py").write_text("# changed package activation\n")
    with pytest.raises(module.ClosureError):
        module.verify_manifest(repo, manifest)


def test_import_star_runtime_helper_change_invalidates_actual_manifest(tmp_path):
    repo, module = _synthetic_repo(tmp_path)
    package = repo / "pkg"
    package.mkdir()
    (repo / "entry.py").write_text("from pkg import *\nprint(helper.VALUE)\n")
    (package / "__init__.py").write_text('__all__ = ["helper"]\n')
    helper = package / "helper.py"
    helper.write_text("VALUE = 1\n")
    module.PROFILES["star-synthetic"] = {"cargo_packages": (), "paths": ("entry.py",)}
    _git(repo, "add", "-A")
    _git(repo, "commit", "-qm", "wildcard execution")
    manifest = module.build_manifest(repo, "star-synthetic")

    def runtime():
        return subprocess.check_output(
            [sys.executable, "-B", "entry.py"], cwd=repo, text=True
        ).strip()

    assert runtime() == "1"
    helper.write_text("VALUE = 200\n")
    assert runtime() == "200"
    with pytest.raises(module.ClosureError):
        module.verify_manifest(repo, manifest)
