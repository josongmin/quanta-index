"""Current-source ownership tests for the quality integration aggregate."""

from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[3]
SCRIPT_PATH = REPO_ROOT / "tools" / "benchmark" / "quality_integration_summary.py"
HEAD = "0123456789abcdef0123456789abcdef01234567"


def _load_module():
    spec = importlib.util.spec_from_file_location("quality_integration_summary", SCRIPT_PATH)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules["quality_integration_summary"] = module
    spec.loader.exec_module(module)
    return module


MODULE = _load_module()


def _schema_two(dimension: str, *, head: str = HEAD, passed: bool = True) -> dict:
    return {
        "schema_version": 2,
        "dimension": dimension,
        "provenance": {"git_head": head},
        "detail": {"passed": passed},
    }


def test_schema_two_verdict_requires_matching_dimension_and_head() -> None:
    assert MODULE.rail_verdict(_schema_two("tail"), dimension="tail", head=HEAD) is True
    assert MODULE.rail_verdict(_schema_two("scale"), dimension="tail", head=HEAD) is None
    assert MODULE.rail_verdict(
        _schema_two("tail", head="fedcba9876543210fedcba9876543210fedcba98"),
        dimension="tail",
        head=HEAD,
    ) is None


def test_schema_one_verdict_requires_matching_full_head() -> None:
    summary = {"schema_version": 1, "dimension": "ops", "git_rev": HEAD, "passed": True}
    assert MODULE.rail_verdict(summary, dimension="ops", head=HEAD) is True
    summary["git_rev"] = HEAD[:12]
    assert MODULE.rail_verdict(summary, dimension="ops", head=HEAD) is None


def test_build_fails_closed_when_a_live_artifact_is_stale(tmp_path: Path, monkeypatch) -> None:
    monkeypatch.setattr(MODULE, "ARTIFACT_ROOT", tmp_path / "artifacts" / "search-quality")
    monkeypatch.setattr(MODULE, "resolve_head", lambda: HEAD)
    monkeypatch.setattr(MODULE, "DIMENSIONS", [("tail", "J7Q-04", True, "live", "summary.json")])
    path = MODULE.ARTIFACT_ROOT / "tail" / "latest" / "summary.json"
    path.parent.mkdir(parents=True)
    path.write_text(json.dumps(_schema_two("tail", head="fedcba9876543210fedcba9876543210fedcba98")))
    doc, passed = MODULE.build()
    assert not passed
    assert doc["dimensions"][0]["passed"] is False
    assert "missing 'passed' verdict" in doc["dimensions"][0]["error"]
