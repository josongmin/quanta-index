"""The benchmark registry is the single control plane and must be reachable.

Every producer, validator and scorer the registry names must exist in the
checkout, every registered artifact family must project into the artifact
checker's family view without drift, and a malformed registry must fail closed.
"""

from __future__ import annotations

import importlib.util
import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[3]
BENCHMARK_DIR = REPO_ROOT / "tools" / "benchmark"
REGISTRY_PATH = BENCHMARK_DIR / "registry.toml"
MANIFEST_PATH = BENCHMARK_DIR / "manifest.py"
JUSTFILE = REPO_ROOT / "Justfile"
RECIPE = re.compile(r"^([a-z0-9][a-z0-9-]*)(?:\s+[^:]*)?:", re.MULTILINE)


def _load(name: str, path: Path):
    if str(BENCHMARK_DIR) not in sys.path:
        sys.path.insert(0, str(BENCHMARK_DIR))
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def _registry_module():
    return _load("benchmark_registry", BENCHMARK_DIR / "registry.py")


def _manifest_module():
    _registry_module()
    return _load("benchmark_manifest", MANIFEST_PATH)


def test_every_registered_producer_is_reachable() -> None:
    registry = _registry_module().load_registry()
    recipes = set(RECIPE.findall(JUSTFILE.read_text(encoding="utf-8")))
    missing = sorted(
        entry["recipe"]
        for entry in registry["producers"].values()
        if entry["kind"] == "just-recipe" and entry["recipe"] not in recipes
    )
    assert missing == [], f"registry names missing Just recipe(s): {missing}"


def test_registry_has_no_second_manifest_data_source() -> None:
    assert not (BENCHMARK_DIR / "manifest.json").exists(), (
        "manifest.json must not return: registry.toml is the single data authority"
    )


def test_artifact_projection_matches_the_registry() -> None:
    registry = _registry_module().load_registry()
    manifest = _manifest_module().load_manifest()
    expected = {
        name: registry["producers"][family["producer"]]["outputs"][0]
        for name, family in registry["families"].items()
        if family["producer"] != "none"
        and family["native_schema"] == "BenchArtifactV1:2"
        and registry["producers"][family["producer"]]["outputs"]
    }
    projected = {name: family["artifact_glob"] for name, family in manifest["families"].items()}
    assert projected == expected
    assert "dsl-warm-criterion" not in projected


def test_profiles_project_without_inventing_membership() -> None:
    registry = _registry_module().load_registry()
    manifest = _manifest_module().load_manifest()
    for name, profile in manifest["profiles"].items():
        assert name in registry["profiles"], name
        assert set(profile["families"]) <= set(registry["profiles"][name]["families"]), name


def test_quality_all_delegates_to_the_registry_control_plane() -> None:
    justfile = JUSTFILE.read_text(encoding="utf-8")
    match = re.search(
        r"^rust-verify-quality-all:\n(?P<body>.*?)(?=^[a-z0-9][a-z0-9-]*(?:\s+[^:]*)?:|\Z)",
        justfile,
        re.MULTILINE | re.DOTALL,
    )
    assert match is not None
    body = match.group("body")
    assert "python3 tools/benchmark/benchctl.py run quality-full" in body
    assert "@just rust-verify-quality-" not in body


def test_dsl_refresh_delegates_to_the_registry_control_plane() -> None:
    justfile = JUSTFILE.read_text(encoding="utf-8")
    match = re.search(
        r"^rust-bench-dsl-refresh\s+samples=\"20\":\n(?P<body>.*?)(?=^[a-z0-9][a-z0-9-]*(?:\s+[^:]*)?:|\Z)",
        justfile,
        re.MULTILINE | re.DOTALL,
    )
    assert match is not None
    body = match.group("body")
    assert (
        "python3 tools/benchmark/benchctl.py run dsl-authority --cold-samples {{samples}}" in body
    )
    assert "@just rust-bench-dsl-" not in body


def test_registry_refuses_a_family_without_a_required_key(tmp_path: Path) -> None:
    module = _registry_module()
    text = REGISTRY_PATH.read_text(encoding="utf-8")
    mutated, count = re.subn(r"\ngate_tier = \"[a-z]+\"", "", text, count=1)
    assert count == 1, "fixture did not remove a gate_tier line"
    path = tmp_path / "registry.toml"
    path.write_text(mutated, encoding="utf-8")
    try:
        module.load_registry(path, repo_root=REPO_ROOT)
    except module.RegistryError as error:
        assert "gate_tier" in str(error)
    else:
        raise AssertionError("registry accepted a family without a gate tier")


def test_registry_refuses_an_unreachable_producer(tmp_path: Path) -> None:
    module = _registry_module()
    text = REGISTRY_PATH.read_text(encoding="utf-8")
    mutated = text.replace(
        'recipe = "rust-bench-dsl-warm"', 'recipe = "rust-bench-dsl-nonexistent"'
    )
    assert mutated != text
    path = tmp_path / "registry.toml"
    path.write_text(mutated, encoding="utf-8")
    try:
        module.load_registry(path, repo_root=REPO_ROOT)
    except module.RegistryError as error:
        assert "missing Justfile recipe" in str(error)
    else:
        raise AssertionError("registry accepted an unreachable producer")


def test_registry_refuses_ambiguous_profile_membership(tmp_path: Path) -> None:
    module = _registry_module()
    text = REGISTRY_PATH.read_text(encoding="utf-8")
    mutated = text.replace(
        'families = ["dsl-warm", "dsl-cold"]',
        'families = ["dsl-warm", "dsl-cold", "dsl-warm"]',
    )
    assert mutated != text
    path = tmp_path / "registry.toml"
    path.write_text(mutated, encoding="utf-8")
    try:
        module.load_registry(path, repo_root=REPO_ROOT)
    except module.RegistryError as error:
        assert "twice" in str(error)
    else:
        raise AssertionError("registry accepted ambiguous profile membership")
