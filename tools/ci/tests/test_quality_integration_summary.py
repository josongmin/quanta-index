"""Current-source ownership tests for the quality integration aggregate."""

from __future__ import annotations

import importlib.util
import json
import subprocess
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
    assert (
        MODULE.rail_verdict(
            _schema_two("tail", head="fedcba9876543210fedcba9876543210fedcba98"),
            dimension="tail",
            head=HEAD,
        )
        is None
    )
    assert (
        MODULE.rail_verdict(_schema_two("tail", passed="false"), dimension="tail", head=HEAD)
        is None
    )


def test_schema_one_verdict_is_never_quality_authority() -> None:
    summary = {"schema_version": 1, "dimension": "ops", "git_rev": HEAD, "passed": True}
    assert MODULE.rail_verdict(summary, dimension="ops", head=HEAD) is None
    summary["git_rev"] = HEAD[:12]
    assert MODULE.rail_verdict(summary, dimension="ops", head=HEAD) is None


def test_quality_dimensions_follow_the_canonical_quality_full_profile() -> None:
    manifest = MODULE.load_manifest(REPO_ROOT / "tools" / "benchmark" / "manifest.json")
    profile = manifest["profiles"]["quality-full"]
    dimensions = MODULE.quality_dimensions()

    assert [dimension for dimension, *_ in dimensions] == profile["families"]
    assert [glob for *_, glob in dimensions] == [
        manifest["families"][name]["artifact_glob"] for name in profile["families"]
    ]


def test_build_fails_closed_when_a_live_artifact_is_stale(tmp_path: Path, monkeypatch) -> None:
    monkeypatch.setattr(MODULE, "ROOT", tmp_path)
    monkeypatch.setattr(MODULE, "resolve_head", lambda: HEAD)
    monkeypatch.setattr(
        MODULE,
        "quality_dimensions",
        lambda: [("tail", "J7Q-04", True, "live", "artifacts/search-quality/tail/latest/summary.json")],
    )
    path = tmp_path / "artifacts" / "search-quality" / "tail" / "latest" / "summary.json"
    path.parent.mkdir(parents=True)
    path.write_text(
        json.dumps(_schema_two("tail", head="fedcba9876543210fedcba9876543210fedcba98"))
    )
    doc, passed = MODULE.build()
    assert not passed
    assert doc["dimensions"][0]["passed"] is False
    assert "missing 'passed' verdict" in doc["dimensions"][0]["error"]


def test_build_requires_every_concurrency_level(tmp_path: Path, monkeypatch) -> None:
    monkeypatch.setattr(MODULE, "ROOT", tmp_path)
    monkeypatch.setattr(MODULE, "resolve_head", lambda: HEAD)
    monkeypatch.setattr(
        MODULE,
        "quality_dimensions",
        lambda: [
            (
                "concurrency",
                "QI-BB-010",
                True,
                "live",
                "artifacts/search-quality/concurrency/latest/summary-c*.json",
            )
        ],
    )
    path = tmp_path / "artifacts" / "search-quality" / "concurrency" / "latest" / "summary-c8.json"
    path.parent.mkdir(parents=True)
    path.write_text(json.dumps(_schema_two("concurrency")))
    doc, passed = MODULE.build()
    assert not passed
    assert "c1/c8/c32" in doc["dimensions"][0]["error"]


def test_cli_refuses_invalid_evidence_before_writing_green(
    tmp_path: Path, monkeypatch, capsys
) -> None:
    subprocess.run(["git", "init", "-q", str(tmp_path)], check=True)
    manifest = tmp_path / "tools" / "benchmark" / "manifest.json"
    manifest.parent.mkdir(parents=True)
    manifest.write_text(
        (REPO_ROOT / "tools" / "benchmark" / "manifest.json").read_text(encoding="utf-8"),
        encoding="utf-8",
    )
    (tmp_path / ".gitignore").write_text("artifacts/\n", encoding="utf-8")
    subprocess.run(["git", "-C", str(tmp_path), "add", "tools/benchmark/manifest.json", ".gitignore"], check=True)
    subprocess.run(
        ["git", "-C", str(tmp_path), "-c", "user.name=Bench Test", "-c", "user.email=bench@example.invalid", "commit", "-qm", "fixture"],
        check=True,
    )
    head = subprocess.run(
        ["git", "-C", str(tmp_path), "rev-parse", "HEAD"],
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()
    tail = tmp_path / "artifacts" / "search-quality" / "tail" / "latest" / "summary.json"
    tail.parent.mkdir(parents=True)
    tail.write_text(json.dumps(_schema_two("tail", head=head)), encoding="utf-8")
    monkeypatch.setattr(MODULE, "ROOT", tmp_path)
    monkeypatch.setattr(sys, "argv", ["quality_integration_summary.py", "--out", str(tmp_path / "summary.json")])
    monkeypatch.setattr(MODULE, "resolve_head", lambda: head)
    monkeypatch.setattr(
        MODULE,
        "quality_dimensions",
        lambda: [("tail", "J7Q-04", True, "live", "artifacts/search-quality/tail/latest/summary.json")],
    )

    assert MODULE.main() == 1
    assert not (tmp_path / "summary.json").exists()
