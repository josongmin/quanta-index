from __future__ import annotations

import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
LINT = ROOT / "tools" / "ci" / "lint" / "lint-hexagonal-boundaries.py"


def test_hexagonal_boundary_lint_passes_on_repo() -> None:
    completed = subprocess.run(
        [sys.executable, str(LINT)],
        cwd=ROOT,
        check=False,
        capture_output=True,
        text=True,
    )
    assert completed.returncode == 0, completed.stderr or completed.stdout


def _load_lint():
    import importlib.util

    spec = importlib.util.spec_from_file_location("lint_hexagonal_boundaries", LINT)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    # The lint's dataclasses resolve their module through `sys.modules`.
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def test_path_dependencies_are_named_as_cargo_names_them(tmp_path: Path) -> None:
    """The table key (or `package`) is the name, never the path's basename.

    Deriving the name from the path, underscored, once made every internal
    dependency invisible to the allowlist check.
    """
    lint = _load_lint()
    cargo_toml = tmp_path / "Cargo.toml"
    cargo_toml.write_text(
        """
[package]
name = "quanta-index-example"

[dependencies]
quanta-index-core = { version = "0.1.0", path = "../quanta-index-core" }
alias = { package = "quanta-index-contract", version = "0.1.0", path = "../somewhere-else" }
serde = { workspace = true }

[dev-dependencies]
quanta-index-searchd-harness = { version = "0.1.0", path = "../quanta-index-searchd-harness" }
""",
        encoding="utf-8",
    )
    assert lint.path_dependencies(cargo_toml, lint.PRODUCTION_SECTIONS) == {
        "quanta-index-core",
        "quanta-index-contract",
    }
    assert lint.path_dependencies(cargo_toml, lint.DEV_SECTIONS) == {
        "quanta-index-searchd-harness",
    }


def test_an_unlisted_internal_dependency_is_a_violation(tmp_path: Path, monkeypatch) -> None:
    """A production edge outside the allowlist fails; a test-support crate
    is admitted as a dev-dependency only."""
    lint = _load_lint()
    crates = tmp_path / "crates"
    core = crates / "quanta-index-core"
    core.mkdir(parents=True)
    (core / "Cargo.toml").write_text(
        """
[package]
name = "quanta-index-core"

[dependencies]
quanta-index-contract = { version = "0.1.0", path = "../quanta-index-contract" }
quanta-index-lexical = { version = "0.1.0", path = "../quanta-index-lexical" }

[dev-dependencies]
quanta-index-corpus-smoke = { version = "0.1.0", path = "../quanta-index-corpus-smoke" }
""",
        encoding="utf-8",
    )
    monkeypatch.setattr(lint, "CRATES", crates)
    messages = [violation.message for violation in lint.check_crate_dependency_matrix()]
    assert messages == [
        "quanta-index-core must not depend on quanta-index-lexical "
        "(allowed: ['quanta-index-contract'])"
    ]


def test_core_vendor_alias_cannot_bypass_dependency_boundary(tmp_path: Path, monkeypatch) -> None:
    lint = _load_lint()
    core = tmp_path / "crates" / "quanta-index-core"
    core.mkdir(parents=True)
    (core / "Cargo.toml").write_text(
        """
[package]
name = "quanta-index-core"

[dependencies]
storage = { package = "rusqlite", version = "0.40" }
""",
        encoding="utf-8",
    )
    monkeypatch.setattr(lint, "CRATES", tmp_path / "crates")
    messages = [violation.message for violation in lint.check_crate_dependency_matrix()]
    assert messages == ["quanta-index-core must not depend on vendor crate 'rusqlite'"]
