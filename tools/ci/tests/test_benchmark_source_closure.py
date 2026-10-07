"""Source closure: the benchmark control plane binds normative files only.

The closure engine is exercised on a synthetic profile so the mutation
semantics (changed/added/removed file invalidates; an unrelated planning edit
does not) are proven independently of the live checkout's dirty state.
"""

from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[3]
CLOSURE_PATH = REPO_ROOT / "tools" / "ci" / "source_closure.py"
PROFILE = "benchmark-control-plane"


@pytest.mark.parametrize("profile", ["retrieval", "benchmark-control-plane"])
def test_live_workflow_and_owner_tests_are_bound(profile: str) -> None:
    module = _closure_module()
    paths = set(module.PROFILES[profile]["paths"])
    assert {
        "tools/benchmark/code_search_workflow.py",
        "tools/benchmark/code_search_matrix.py",
        "tools/ci/tests/test_live_lexical_external.py",
        "tools/ci/tests/test_code_search_workflow.py",
        "tools/ci/tests/test_code_search_matrix.py",
        "tools/ci/lane-handoff.schema.json",
    } <= paths


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
    (repo / "docs" / "plans" / "unrelated-history").mkdir(parents=True)
    (repo / "tools" / "benchmark").mkdir(parents=True)
    (repo / "tools" / "benchmark" / "registry.toml").write_text("x\n", encoding="utf-8")
    (repo / "docs" / "plans" / "unrelated-history" / "INDEX.md").write_text(
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
        "docs/adr/JUN-08-001-verification-hellgate-and-benchmark-separation.md",
        "docs/adr/SEP-26-003-retrieval-evidence-custody-and-qualification.md",
        "docs/adr/SEP-27-002-single-benchmark-orchestrator-and-typed-evidence.md",
        "docs/adr/SEP-27-004-benchmark-capture-and-resource-custody.md",
        "docs/adr/SEP-27-005-catalog-recovery-supervision-and-proof-custody.md",
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


def test_semantic_decision_is_bound_by_retrieval_closure() -> None:
    module = _closure_module()
    assert (
        "docs/adr/MAY-31-001-lancedb-semantic-generation-authority.md"
        in module.PROFILES["retrieval"]["paths"]
    )


@pytest.mark.parametrize(
    "profile,decision",
    [
        ("retrieval", "OCT-05-001-review-admission-and-result-identity.md"),
        ("retrieval", "OCT-05-002-native-capture-clock-and-index-scope.md"),
        ("retrieval", "OCT-05-003-active-query-and-runtime-lifecycle.md"),
        ("retrieval", "OCT-05-004-cost-capacity-and-qualification-boundaries.md"),
        ("benchmark-control-plane", "OCT-05-001-review-admission-and-result-identity.md"),
        ("benchmark-control-plane", "OCT-05-002-native-capture-clock-and-index-scope.md"),
        ("benchmark-control-plane", "OCT-05-004-cost-capacity-and-qualification-boundaries.md"),
    ],
)
def test_consolidated_adr_mutation_invalidates_bound_closure(tmp_path, profile, decision):
    repo, module = _synthetic_repo(tmp_path)
    relative = f"docs/adr/{decision}"
    assert relative in module.PROFILES[profile]["paths"]
    path = repo / relative
    path.parent.mkdir(parents=True)
    original = (REPO_ROOT / relative).read_text()
    assert "Status: `Accepted`" in original
    path.write_text(original)
    module.PROFILES["bm-synthetic"]["paths"] += (relative,)
    _git(repo, "add", "-A")
    _git(repo, "commit", "-qm", "bind adopted decision")
    manifest = module.build_manifest(repo, "bm-synthetic")
    module.verify_manifest(repo, manifest)
    path.write_text(original.replace("Status: `Accepted`", "Status: `Proposed`", 1))
    with pytest.raises(module.ClosureError, match="dirty|changed"):
        module.verify_manifest(repo, manifest)


def test_portable_execution_transitively_binds_file_custody_owner() -> None:
    module = _closure_module()
    imports = module._python_import_roots(
        REPO_ROOT, ["tools/benchmark/retrieval/portable_proof.py"]
    )
    assert {
        "tools/benchmark/producer_execution.py",
        "tools/benchmark/evidence.py",
        "tools/ci/lint/handoff_validation.py",
    } <= imports


def test_retrieval_closure_binds_the_resolved_typescript_grammar() -> None:
    module = _closure_module()
    # A producer-side hash of an unused vendor directory is insufficient. The
    # actual Cargo dependency graph must select and bind those source files.
    roots = module._cargo_roots(REPO_ROOT, ("quanta-index-retrieval-bench",))
    assert "vendor/tree-sitter-typescript" in roots


def test_local_patch_below_registry_dependency_remains_in_source_closure(tmp_path, monkeypatch):
    module = _closure_module()
    metadata = {
        "packages": [
            {
                "id": "app",
                "name": "app",
                "source": None,
                "manifest_path": str(tmp_path / "app/Cargo.toml"),
            },
            {
                "id": "registry",
                "name": "registry",
                "source": "registry+https://example.invalid",
                "manifest_path": "/external/registry/Cargo.toml",
            },
            {
                "id": "patch",
                "name": "patch",
                "source": None,
                "manifest_path": str(tmp_path / "vendor/patch/Cargo.toml"),
            },
        ],
        "resolve": {
            "nodes": [
                {"id": "app", "dependencies": ["registry"]},
                {"id": "registry", "dependencies": ["patch"]},
                {"id": "patch", "dependencies": []},
            ]
        },
    }
    monkeypatch.setattr(module, "_metadata", lambda _repo: metadata)
    assert module._cargo_roots(tmp_path, ("app",)) == {"app", "vendor/patch"}
    metadata["resolve"]["nodes"].pop(1)
    with pytest.raises(module.ClosureError, match="resolve node missing"):
        module._cargo_roots(tmp_path, ("app",))


def test_execution_ledgers_are_not_normative_source_roots() -> None:
    module = _closure_module()
    roots = module.resolve_roots(REPO_ROOT, PROFILE)
    assert not [root for root in roots if root.startswith("docs/plans/")]


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
        assert "dirty relevant source" in str(error)
        assert "tools/benchmark/registry.toml" in str(error)
    else:
        raise AssertionError("a changed normative file was captured into a closure")


def test_reused_closure_preflight_requires_same_clean_revision_and_inventory(
    tmp_path: Path,
) -> None:
    repo, module = _synthetic_repo(tmp_path)
    module.PROFILES["bm-synthetic"]["paths"] = ("tools/benchmark",)
    manifest = module.build_manifest(repo, "bm-synthetic")
    assert module.preflight_reused_manifest(repo, manifest) == manifest

    source = repo / "tools/benchmark/registry.toml"
    source.write_text("changed\n", encoding="utf-8")
    with pytest.raises(module.ClosureError, match="dirty relevant source"):
        module.preflight_reused_manifest(repo, manifest)
    source.write_text("x\n", encoding="utf-8")

    added = repo / "tools/benchmark/new.toml"
    added.write_text("new\n", encoding="utf-8")
    with pytest.raises(module.ClosureError, match="dirty relevant source|file set changed"):
        module.preflight_reused_manifest(repo, manifest)
    added.unlink()

    source.write_text("new committed content\n", encoding="utf-8")
    _git(repo, "add", "-A")
    _git(repo, "commit", "-qm", "new revision")
    with pytest.raises(module.ClosureError, match="revision changed"):
        module.preflight_reused_manifest(repo, manifest)


def test_reuse_cli_writes_prior_closure_only_for_matching_source(tmp_path, monkeypatch) -> None:
    repo, module = _synthetic_repo(tmp_path)
    manifest = module.build_manifest(repo, "bm-synthetic")
    prior = tmp_path / "prior.json"
    prior.write_text(json.dumps(manifest) + "\n", encoding="utf-8")
    out = tmp_path / "reused.json"
    monkeypatch.setattr(module, "_repo_root", lambda: repo)
    monkeypatch.setattr(
        sys,
        "argv",
        ["source_closure.py", "reuse", "--manifest", str(prior), "--out", str(out)],
    )
    assert module.main() == 0
    assert module.load_and_verify(out, repo) == manifest

    changed = tmp_path / "changed.json"
    (repo / "tools/benchmark/registry.toml").write_text("dirty\n", encoding="utf-8")
    monkeypatch.setattr(
        sys,
        "argv",
        ["source_closure.py", "reuse", "--manifest", str(prior), "--out", str(changed)],
    )
    with pytest.raises(SystemExit, match="dirty relevant source"):
        module.main()
    assert not changed.exists()


def test_incomplete_import_traversal_refuses_python_inventory(tmp_path, monkeypatch):
    repo, module = _synthetic_repo(tmp_path)
    entry = repo / "tools/benchmark/entry.py"
    entry.write_text("VALUE = 1\n", encoding="utf-8")
    module.PROFILES["bm-synthetic"]["paths"] = ("tools/benchmark/entry.py",)
    _git(repo, "add", "-A")
    _git(repo, "commit", "-qm", "add Python entry")
    monkeypatch.setattr(module, "_python_import_roots", lambda *_args, **_kwargs: set())
    with pytest.raises(
        module.ClosureError, match="Python inventory changed: tools/benchmark/entry.py"
    ):
        module.build_manifest(repo, "bm-synthetic")


def test_import_appearing_outside_roots_during_capture_is_rejected(tmp_path, monkeypatch):
    repo, module = _synthetic_repo(tmp_path)
    entry = repo / "tools/benchmark/entry.py"
    entry.write_text("import late_module\n", encoding="utf-8")
    module.PROFILES["bm-synthetic"]["paths"] = ("tools/benchmark/entry.py",)
    _git(repo, "add", "-A")
    _git(repo, "commit", "-qm", "add Python entry")
    original_clean = module._assert_clean
    late = repo / "late_module.py"

    def appear_after_traversal(source, roots):
        if not late.exists():
            late.write_text("VALUE = 1\n", encoding="utf-8")
        original_clean(source, roots)

    monkeypatch.setattr(module, "_assert_clean", appear_after_traversal)
    with pytest.raises(module.ClosureError, match="source closure imports changed"):
        module.build_manifest(repo, "bm-synthetic")


@pytest.mark.parametrize(
    "filename",
    [
        "test_producer_notifications.py",
        "test_bootstrap_cache.py",
        "test_proof_command_timings.py",
        "test_resource_admission.py",
        "test_cargow_resource_admission.py",
        "test_pair_replay_workspace.py",
        "test_cargo_preparation.py",
    ],
)
def test_execution_owner_test_mutation_invalidates_its_bound_closure(tmp_path, filename):
    repo, module = _synthetic_repo(tmp_path)
    relative = f"tools/ci/tests/{filename}"
    assert relative in module.PROFILES["benchmark-control-plane"]["paths"]
    path = repo / relative
    path.parent.mkdir(parents=True)
    path.write_bytes((REPO_ROOT / relative).read_bytes())
    module.PROFILES["bm-synthetic"]["paths"] += (relative,)
    _git(repo, "add", "-A")
    _git(repo, "commit", "-qm", "bind owner test")
    manifest = module.build_manifest(repo, "bm-synthetic")
    path.write_text(path.read_text() + "\n# changed owner test\n")
    with pytest.raises(module.ClosureError, match="dirty|changed"):
        module.verify_manifest(repo, manifest)


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
    (repo / "docs" / "plans" / "unrelated-history" / "INDEX.md").write_text(
        "plan v2\n", encoding="utf-8"
    )
    module.verify_manifest(repo, manifest)


@pytest.mark.parametrize(
    ("profile", "dependency"),
    [
        ("benchmark-control-plane", "tools/benchmark/native_contracts.py"),
        ("benchmark-control-plane", "uv.lock"),
        ("benchmark-control-plane", "scripts/quanta-index-env.sh"),
        (
            "benchmark-control-plane",
            "docs/adr/JUN-08-001-verification-hellgate-and-benchmark-separation.md",
        ),
        (
            "benchmark-control-plane",
            "docs/adr/SEP-26-003-retrieval-evidence-custody-and-qualification.md",
        ),
        (
            "benchmark-control-plane",
            "docs/adr/SEP-27-002-single-benchmark-orchestrator-and-typed-evidence.md",
        ),
        ("retrieval", "docs/adr/MAY-31-001-lancedb-semantic-generation-authority.md"),
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
    with pytest.raises(
        module.ClosureError, match="contains no files|cannot inventory|not a committed"
    ):
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


@pytest.mark.parametrize("flag", ["--assume-unchanged", "--skip-worktree"])
def test_git_visibility_flags_cannot_conceal_wrong_source_bytes(tmp_path, flag):
    repo, module = _synthetic_repo(tmp_path)
    manifest = module.build_manifest(repo, "bm-synthetic")
    path = repo / "tools/benchmark/registry.toml"
    _git(repo, "update-index", flag, "--", "tools/benchmark/registry.toml")
    path.write_text("tampered source\n")
    assert subprocess.check_output(["git", "status", "--porcelain"], cwd=repo, text=True) == ""
    with pytest.raises(module.ClosureError, match="differs from committed HEAD"):
        module.build_manifest(repo, "bm-synthetic")
    with pytest.raises(module.ClosureError, match="differs from committed HEAD"):
        module.verify_manifest(repo, manifest)


def test_capture_rejects_hidden_source_change_after_read(tmp_path, monkeypatch):
    repo, module = _synthetic_repo(tmp_path)
    _git(repo, "update-index", "--assume-unchanged", "--", "tools/benchmark/registry.toml")
    original = module._SourceFrame.read
    changed = False

    def concurrent_read(frame, path):
        nonlocal changed
        data = original(frame, path)
        if path.name == "registry.toml" and not changed:
            changed = True
            path.write_text("changed after capture read\n")
        return data

    monkeypatch.setattr(module._SourceFrame, "read", concurrent_read)
    with pytest.raises(module.ClosureError, match="source changed during closure operation"):
        module.build_manifest(repo, "bm-synthetic")


def test_deleted_hidden_import_cannot_disappear_from_committed_dependency_graph(tmp_path):
    repo, module = _python_repo(tmp_path)
    module.build_manifest(repo, "python-synthetic")
    _git(repo, "update-index", "--assume-unchanged", "--", "tools/ci/junit_events.py")
    (repo / "tools/ci/junit_events.py").unlink()
    with pytest.raises(module.ClosureError, match="cannot read committed source"):
        module.build_manifest(repo, "python-synthetic")


def test_metadata_input_restoration_cannot_hide_capture_epoch_change(tmp_path, monkeypatch):
    repo, module = _synthetic_repo(tmp_path)
    manifest = repo / "Cargo.toml"
    manifest.write_text("[workspace]\n")
    module.PROFILES["bm-synthetic"]["paths"] += ("Cargo.toml",)
    module.PROFILES["bm-synthetic"]["cargo_packages"] = ("fixture",)
    _git(repo, "add", "-A")
    _git(repo, "commit", "-qm", "metadata source")
    _git(repo, "update-index", "--assume-unchanged", "--", "Cargo.toml")

    def racing_metadata(_repo, _packages):
        original = manifest.read_bytes()
        manifest.write_text("[workspace]\n# different dependency selection\n")
        manifest.write_bytes(original)
        return set()

    monkeypatch.setattr(module, "_cargo_roots", racing_metadata)
    with pytest.raises(module.ClosureError, match="source changed during closure operation"):
        module.build_manifest(repo, "bm-synthetic")


def test_missing_local_resolve_node_cannot_hide_declared_package_source(tmp_path, monkeypatch):
    repo, module = _synthetic_repo(tmp_path)
    package = repo / "crates/fixture"
    (package / "src").mkdir(parents=True)
    manifest_path = package / "Cargo.toml"
    manifest_path.write_text('[package]\nname = "fixture"\nversion = "0.1.0"\n')
    source = package / "src/lib.rs"
    source.write_text("pub const VALUE: usize = 1;\n")
    module.PROFILES["bm-synthetic"]["cargo_packages"] = ("fixture",)
    _git(repo, "add", "-A")
    _git(repo, "commit", "-qm", "declared local package")
    metadata = {
        "packages": [
            {
                "id": "fixture-id",
                "name": "fixture",
                "source": None,
                "manifest_path": str(manifest_path),
            }
        ],
        "resolve": {"nodes": [{"id": "fixture-id", "dependencies": []}]},
    }
    monkeypatch.setattr(module, "_metadata", lambda _repo: metadata)
    manifest = module.build_manifest(repo, "bm-synthetic")
    metadata["resolve"]["nodes"] = []
    with pytest.raises(module.ClosureError, match="resolve node missing for local package"):
        module.build_manifest(repo, "bm-synthetic")
    source.write_text("pub const VALUE: usize = 200;\n")
    with pytest.raises(module.ClosureError, match="resolve node missing for local package"):
        module.verify_manifest(repo, manifest)


@pytest.mark.parametrize("duplicate", ["package", "resolve node"])
def test_duplicate_cargo_identity_cannot_replace_source_dependencies(
    tmp_path, monkeypatch, duplicate
):
    repo, module = _synthetic_repo(tmp_path)
    packages = []
    for name in ("fixture", "helper"):
        package = repo / "crates" / name
        (package / "src").mkdir(parents=True)
        manifest_path = package / "Cargo.toml"
        manifest_path.write_text(f'[package]\nname = "{name}"\nversion = "0.1.0"\n')
        (package / "src/lib.rs").write_text("pub const VALUE: usize = 1;\n")
        packages.append(
            {"id": name, "name": name, "source": None, "manifest_path": str(manifest_path)}
        )
    module.PROFILES["bm-synthetic"]["cargo_packages"] = ("fixture",)
    _git(repo, "add", "-A")
    _git(repo, "commit", "-qm", "local dependency graph")
    metadata = {
        "packages": packages,
        "resolve": {
            "nodes": [
                {"id": "fixture", "dependencies": ["helper"]},
                {"id": "helper", "dependencies": []},
            ]
        },
    }
    monkeypatch.setattr(module, "_metadata", lambda _repo: metadata)
    manifest = module.build_manifest(repo, "bm-synthetic")
    if duplicate == "resolve node":
        metadata["resolve"]["nodes"].append({"id": "fixture", "dependencies": []})
        omitted_source = repo / "crates/helper/src/lib.rs"
    else:
        replacement = dict(packages[0], manifest_path=packages[1]["manifest_path"])
        packages.append(replacement)
        omitted_source = repo / "crates/fixture/src/lib.rs"
    with pytest.raises(module.ClosureError, match=f"duplicate {duplicate} id"):
        module.build_manifest(repo, "bm-synthetic")
    omitted_source.write_text("pub const VALUE: usize = 200;\n")
    with pytest.raises(module.ClosureError, match=f"duplicate {duplicate} id"):
        module.verify_manifest(repo, manifest)


@pytest.mark.parametrize("alias", ["file", "ancestor"])
def test_normative_source_alias_cannot_hide_selected_dirty_source(tmp_path, alias):
    repo, module = _synthetic_repo(tmp_path)
    replacement = repo / "replacement"
    replacement.mkdir()
    (replacement / "registry.toml").write_text("different behavior\n")
    _git(repo, "add", "-A")
    _git(repo, "commit", "-qm", "alternate committed source")
    manifest = module.build_manifest(repo, "bm-synthetic")
    if alias == "file":
        original = repo / "tools/benchmark/registry.toml"
        original.unlink()
        original.symlink_to(replacement / "registry.toml")
    else:
        original = repo / "tools/benchmark"
        (original / "registry.toml").unlink()
        original.rmdir()
        original.symlink_to(replacement, target_is_directory=True)
    assert (repo / "tools/benchmark/registry.toml").read_text() == "different behavior\n"
    with pytest.raises(module.ClosureError, match="aliased source root"):
        module.build_manifest(repo, "bm-synthetic")
    with pytest.raises(module.ClosureError, match="aliased source root"):
        module.verify_manifest(repo, manifest)


@pytest.mark.parametrize("alias", ["file", "ancestor"])
def test_cargo_manifest_alias_cannot_replace_selected_package(tmp_path, monkeypatch, alias):
    repo, module = _synthetic_repo(tmp_path)
    for name in ("fixture", "other"):
        package = repo / "crates" / name
        (package / "src").mkdir(parents=True)
        (package / "Cargo.toml").write_text('[package]\nname = "fixture"\nversion = "0.1.0"\n')
        (package / "src/lib.rs").write_text(f'pub const NAME: &str = "{name}";\n')
    _git(repo, "add", "-A")
    _git(repo, "commit", "-qm", "packages with equal manifest bytes")
    module.PROFILES["bm-synthetic"]["cargo_packages"] = ("fixture",)
    selected = repo / "crates/fixture/Cargo.toml"
    metadata = {
        "packages": [
            {"id": "fixture", "name": "fixture", "source": None, "manifest_path": str(selected)}
        ],
        "resolve": {"nodes": [{"id": "fixture", "dependencies": []}]},
    }
    monkeypatch.setattr(module, "_metadata", lambda _repo: metadata)
    manifest = module.build_manifest(repo, "bm-synthetic")
    if alias == "file":
        selected.unlink()
        selected.symlink_to(repo / "crates/other/Cargo.toml")
    else:
        package = selected.parent
        selected.unlink()
        (package / "src/lib.rs").unlink()
        (package / "src").rmdir()
        package.rmdir()
        package.symlink_to(repo / "crates/other", target_is_directory=True)
    with pytest.raises(module.ClosureError, match="aliased cargo manifest"):
        module.build_manifest(repo, "bm-synthetic")
    with pytest.raises(module.ClosureError, match="aliased cargo manifest"):
        module.verify_manifest(repo, manifest)
