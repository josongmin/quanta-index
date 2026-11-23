"""Grammar identity selects product paths despite official dev-only aliases."""

from __future__ import annotations

from pathlib import Path

import pytest

from tools.benchmark.retrieval import symbol_coverage

VERSIONS = {
    "tree-sitter": "0.25.10",
    "tree-sitter-rust": "0.24.2",
    "tree-sitter-go": "0.25.0",
    "tree-sitter-javascript": "0.25.0",
    "tree-sitter-python": "0.25.0",
    "tree-sitter-typescript": "0.23.2",
}
VENDOR = {"tree-sitter-go", "tree-sitter-javascript", "tree-sitter-typescript"}


def _fixture(tmp_path: Path, *, alias: set[str] | None = None):
    aliases = {"tree-sitter-go"} if alias is None else alias
    workspace = ["[workspace.dependencies]"]
    dependencies = ["[dependencies]"]
    dev_dependencies = ["[dev-dependencies]"]
    packages = []
    for name, version in VERSIONS.items():
        if name in VENDOR:
            workspace.append(f'{name} = {{ version = "={version}", path = "vendor/{name}" }}')
            packages.append({"name": name, "version": version})
            if name in aliases:
                workspace.append(
                    f'{name}-official = {{ package = "{name}", version = "={version}" }}'
                )
                dev_dependencies.append(f"{name}-official = {{ workspace = true }}")
                packages.append(
                    {
                        "name": name,
                        "version": version,
                        "source": "registry+https://github.com/rust-lang/crates.io-index",
                        "checksum": "a" * 64,
                    }
                )
        else:
            workspace.append(f'{name} = "={version}"')
            packages.append(
                {
                    "name": name,
                    "version": version,
                    "source": "registry+https://github.com/rust-lang/crates.io-index",
                    "checksum": "a" * 64,
                }
            )
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


def test_product_vendor_identity_survives_official_dev_aliases(tmp_path, monkeypatch):
    monkeypatch.setattr(symbol_coverage, "_vendor_digest", lambda *_args: "d" * 64)
    args = _fixture(tmp_path, alias=VENDOR)
    _write(tmp_path, *args)
    identity = symbol_coverage.grammar_identity(tmp_path)
    assert "go@0.25.0+quanta-go-compatibility-1;vendored-source-sha256=" in identity
    assert "javascript@0.25.0+quanta-javascript-export-names-1" in identity
    assert "typescript@0.23.2+quanta-typescript-compatibility-2" in identity


@pytest.mark.parametrize(
    "mutation",
    [
        "duplicate_path",
        "missing_path",
        "bad_registry_source",
        "missing_registry_checksum",
        "extra_registry",
        "wrong_prod_manifest",
        "alias_in_product",
        "wrong_alias_pin",
    ],
)
def test_grammar_identity_rejects_ambiguous_or_forged_lock(tmp_path, monkeypatch, mutation):
    monkeypatch.setattr(symbol_coverage, "_vendor_digest", lambda *_args: "d" * 64)
    packages, workspace, dependencies, dev_dependencies = _fixture(tmp_path)
    go_path = next(p for p in packages if p["name"] == "tree-sitter-go" and "source" not in p)
    go_registry = next(p for p in packages if p["name"] == "tree-sitter-go" and "source" in p)
    if mutation == "duplicate_path":
        packages.append(go_path.copy())
    elif mutation == "missing_path":
        packages.remove(go_path)
    elif mutation == "bad_registry_source":
        go_registry["source"] = "registry+https://example.invalid/index"
    elif mutation == "missing_registry_checksum":
        del go_registry["checksum"]
    elif mutation == "extra_registry":
        packages.append(go_registry.copy())
    elif mutation == "wrong_prod_manifest":
        dependencies[dependencies.index("tree-sitter-go = { workspace = true }")] = (
            'tree-sitter-go = { version = "=0.25.0" }'
        )
    elif mutation == "alias_in_product":
        dependencies.append("tree-sitter-go-official = { workspace = true }")
        dev_dependencies.remove("tree-sitter-go-official = { workspace = true }")
    elif mutation == "wrong_alias_pin":
        workspace[
            workspace.index(
                'tree-sitter-go-official = { package = "tree-sitter-go", version = "=0.25.0" }'
            )
        ] = 'tree-sitter-go-official = { package = "tree-sitter-go", version = "=0.25.1" }'
    _write(tmp_path, packages, workspace, dependencies, dev_dependencies)
    with pytest.raises(ValueError):
        symbol_coverage.grammar_identity(tmp_path)
