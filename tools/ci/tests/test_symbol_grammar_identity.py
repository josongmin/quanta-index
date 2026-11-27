"""Grammar identity binds one registry source for every production grammar."""

from __future__ import annotations

from pathlib import Path

import pytest
import tomllib

from tools.benchmark.retrieval import symbol_coverage

VERSIONS = {
    "tree-sitter": "0.25.10",
    "tree-sitter-rust": "0.24.2",
    "tree-sitter-go": "0.25.0",
    "tree-sitter-javascript": "0.25.0",
    "tree-sitter-python": "0.25.0",
    "tree-sitter-typescript": "0.23.2",
}
GRAMMARS = {name for name in VERSIONS if name != "tree-sitter"}
ROOT = Path(__file__).resolve().parents[3]


def _fixture(tmp_path: Path, *, alias: set[str] | None = None):
    aliases = set() if alias is None else alias
    workspace = ["[workspace.dependencies]"]
    dependencies = ["[dependencies]"]
    dev_dependencies = ["[dev-dependencies]"]
    packages = []
    for name, version in VERSIONS.items():
        workspace.append(f'{name} = "={version}"')
        packages.append(
            {
                "name": name,
                "version": version,
                "source": "registry+https://github.com/rust-lang/crates.io-index",
                "checksum": "a" * 64,
            }
        )
        if name in aliases:
            workspace.append(f'{name}-official = {{ package = "{name}", version = "={version}" }}')
            dev_dependencies.append(f"{name}-official = {{ workspace = true }}")
        if name != "tree-sitter":
            dependencies.append(f"{name} = {{ workspace = true }}")
    return packages, workspace, dependencies, dev_dependencies


def _write(tmp_path: Path, packages, workspace, dependencies, dev_dependencies):
    (tmp_path / "benchmarks/retrieval").mkdir(parents=True)
    (tmp_path / "Cargo.toml").write_text("\n".join(workspace) + "\n")
    (tmp_path / "benchmarks/retrieval/Cargo.toml").write_text(
        "\n".join(dependencies + dev_dependencies) + "\n"
    )
    lines = []
    for package in packages:
        lines.append("[[package]]")
        for key, value in package.items():
            lines.append(f'{key} = "{value}"')
    (tmp_path / "Cargo.lock").write_text("\n".join(lines) + "\n")


def test_product_identity_uses_only_registry_grammars(tmp_path):
    args = _fixture(tmp_path)
    _write(tmp_path, *args)
    identity = symbol_coverage.grammar_identity(tmp_path)
    for tag, version in (("go", "0.25.0"), ("javascript", "0.25.0"), ("typescript", "0.23.2")):
        assert f"{tag}@{version};registry-checksum={'a' * 64}" in identity
    assert "vendored-source-sha256" not in identity


@pytest.mark.parametrize("name", sorted(GRAMMARS))
def test_official_dev_alias_is_rejected_due_to_native_symbol_collision(tmp_path, name):
    _write(tmp_path, *_fixture(tmp_path, alias={name}))
    with pytest.raises(ValueError, match="separate executable"):
        symbol_coverage.grammar_identity(tmp_path)


def test_registry_candidate_is_separate_and_registered_in_ci():
    candidate = ROOT / "tools/ci/tree-sitter-registry-consumer"
    manifest = tomllib.loads((candidate / "Cargo.toml").read_text())
    lock = tomllib.loads((candidate / "Cargo.lock").read_text())
    assert "workspace" in manifest
    for name in {"tree-sitter-go", "tree-sitter-javascript", "tree-sitter-typescript"}:
        rows = [entry for entry in lock["package"] if entry["name"] == name]
        assert len(rows) == 1
        assert rows[0]["source"] == "registry+https://github.com/rust-lang/crates.io-index"
        assert len(rows[0]["checksum"]) == 64
        assert manifest["dependencies"][name] == "=" + VERSIONS[name]
    config = (ROOT / ".circleci/config.yml").read_text()
    recipe = (ROOT / "Justfile").read_text()
    assert "tree-sitter-registry-consumer/Cargo.toml --locked --lib --no-tests fail" in config
    assert "tree-sitter-registry-consumer/Cargo.toml --locked --lib --no-tests fail" in recipe


@pytest.mark.parametrize(
    "mutation",
    [
        "duplicate_registry",
        "missing_registry",
        "extra_path",
        "extra_registry",
        "wrong_prod_manifest",
    ],
)
def test_grammar_identity_rejects_ambiguous_or_forged_lock(tmp_path, mutation):
    packages, workspace, dependencies, dev_dependencies = _fixture(tmp_path)
    go_row = next(p for p in packages if p["name"] == "tree-sitter-go")
    if mutation == "duplicate_registry":
        packages.append(go_row.copy())
    elif mutation == "missing_registry":
        packages.remove(go_row)
    elif mutation == "extra_path":
        packages.append({"name": "tree-sitter-go", "version": "0.25.0"})
    elif mutation == "extra_registry":
        packages.append(
            {
                "name": "tree-sitter-go",
                "version": "0.25.0",
                "source": "registry+https://github.com/rust-lang/crates.io-index",
                "checksum": "a" * 64,
            }
        )
    elif mutation == "wrong_prod_manifest":
        dependencies[dependencies.index("tree-sitter-go = { workspace = true }")] = (
            'tree-sitter-go = { version = "=0.25.0" }'
        )
    _write(tmp_path, packages, workspace, dependencies, dev_dependencies)
    with pytest.raises(ValueError):
        symbol_coverage.grammar_identity(tmp_path)
