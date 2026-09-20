"""The parity report must read the live typed predicate registry, not old literals."""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[3]
SCRIPT = REPO_ROOT / "tools" / "benchmark" / "sourcegraph_parity.py"


def _module():
    spec = importlib.util.spec_from_file_location("sourcegraph_parity_inventory", SCRIPT)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def test_typed_registry_inventory_is_complete():
    module = _module()
    canonical, aliases, route_owned = module.ours_predicates()

    assert set(canonical) == {
        "file.contains",
        "file.has.content",
        "file.has.contributor",
        "file.has.owner",
        "repo.has.commit.after",
        "repo.has.content",
        "repo.has.description",
        "repo.has.file",
        "repo.has.meta",
        "repo.has.topic",
    }
    assert set(aliases) == {
        "file.contains.content",
        "repo.contains.commit.after",
        "repo.contains.content",
        "repo.contains.file",
        "repo.contains.path",
        "repo.has.path",
    }
    assert route_owned == ["symbol.has.name"]


def test_typed_registry_inventory_refuses_missing_rows_or_names():
    module = _module()
    registry = module.read(module.PREDICATE_REGISTRY_RS)
    core = module.read(module.CORE_PREDICATE_RS)

    missing_row = registry.replace(
        "predicate: LexicalPredicateV1::FileContains,",
        "// predicate: LexicalPredicateV1::FileContains,",
        1,
    )
    with pytest.raises(ValueError, match="rows are missing or ambiguous"):
        module._registry_variants(
            missing_row, "PREDICATE_REGISTRY", "PredicateSpec", "LexicalPredicateV1", "predicate"
        )

    missing_name = core.replace(
        'Self::RepoHasMeta => "repo.has.meta",',
        '// Self::RepoHasMeta => "repo.has.meta",',
        1,
    )
    with pytest.raises(ValueError, match="ALL/name tables are incomplete"):
        module._typed_names(missing_name, "LexicalPredicateV1")
