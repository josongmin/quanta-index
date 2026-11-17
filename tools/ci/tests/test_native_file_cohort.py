"""CLI admission for independently replayed Native file families."""

from __future__ import annotations

import copy
import json
from pathlib import Path

import pytest

from tools.benchmark.retrieval import evaluator as ev
from tools.ci.tests.test_native_file_evidence import _native_family_cohort


def _manifest(tmp_path: Path) -> tuple[Path, list[dict]]:
    replayed, repositories, strata = _native_family_cohort()
    manifest = {
        "schema_version": 1,
        "profile": "native_file_family_cohort",
        "repositories": repositories,
        "repository_strata": strata,
        "families": [
            {
                "repo": f"inputs/{index}/repo",
                "suite": f"inputs/{index}/suite.json",
                "query_pack": f"inputs/{index}/pack.json",
                "capture": f"inputs/{index}/capture.json",
                "source_universe": f"inputs/{index}/universe.json",
                "source_binding": f"inputs/{index}/binding.json",
                "base_release": f"inputs/{index}/base.json",
                "target_releases": f"inputs/{index}/targets.json",
                "family_id": item["report"]["composite_source_binding"]["family_id"],
            }
            for index, item in enumerate(replayed)
        ],
    }
    path = tmp_path / "cohort.json"
    path.write_text(json.dumps(manifest), encoding="utf-8")
    return path, replayed


def test_native_file_cohort_cli_replays_and_publishes_repository_equal_gold(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    manifest_path, replayed = _manifest(tmp_path)
    by_family = {
        item["report"]["composite_source_binding"]["family_id"]: item
        for item in replayed
    }
    calls = []

    def replay(repo, suite, query_pack, capture, source_inputs):
        calls.append((repo, suite, query_pack, capture, source_inputs))
        return copy.deepcopy(by_family[source_inputs[-1]])

    monkeypatch.setattr(ev, "load_native_file_replay", replay, raising=False)
    output = tmp_path / "result.json"
    assert ev.main([
        "evaluate-native-file-cohort", "--manifest", str(manifest_path),
        "--output", str(output),
    ]) == 0
    value = json.loads(output.read_text(encoding="utf-8"))
    assert len(calls) == 13
    assert calls[0][0] == tmp_path / "inputs/0/repo"
    assert calls[0][-1][0] == tmp_path / "inputs/0/universe.json"
    assert value["manifest_file_sha256"] == ev.digest(manifest_path.read_bytes())
    assert value["status"] == "native_repository_cluster_diagnostic_unqualified"
    assert value["product_default_decision"] is False
    assert value["runtime_qualified"] is False
    ci = value["repository_cluster_ci"]
    assert (ci["repository_count"], ci["family_count"], ci["sample_count"]) == (12, 13, 31)
    assert ci["mean"] == pytest.approx(-11 / 12)
    assert ci["mean"] != pytest.approx(7 / 31)


@pytest.mark.parametrize(
    "mutation",
    [
        "injected_report", "injected_qualification", "missing_input", "duplicate_family",
        "duplicate_capture", "wrong_profile", "duplicate_json_key",
    ],
)
def test_native_file_cohort_cli_rejects_invalid_manifest_before_replay(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, mutation: str
) -> None:
    path, _ = _manifest(tmp_path)
    manifest = json.loads(path.read_text(encoding="utf-8"))
    if mutation == "injected_report":
        manifest["families"][0]["report"] = {"status": "qualified"}
    elif mutation == "injected_qualification":
        manifest["qualification"] = "verified"
    elif mutation == "missing_input":
        del manifest["families"][0]["source_binding"]
    elif mutation == "duplicate_family":
        manifest["families"][1]["family_id"] = manifest["families"][0]["family_id"]
    elif mutation == "duplicate_capture":
        manifest["families"][1]["capture"] = manifest["families"][0]["capture"]
    elif mutation == "wrong_profile":
        manifest["profile"] = "qualified_native_file"
    if mutation == "duplicate_json_key":
        path.write_text(path.read_text(encoding="utf-8").replace(
            '"schema_version": 1', '"schema_version": 1, "schema_version": 1', 1
        ), encoding="utf-8")
    else:
        path.write_text(json.dumps(manifest), encoding="utf-8")
    monkeypatch.setattr(
        ev, "load_native_file_replay", lambda *_: pytest.fail("invalid manifest reached replay")
    )
    output = tmp_path / "result.json"
    assert ev.main([
        "evaluate-native-file-cohort", "--manifest", str(path),
        "--output", str(output),
    ]) == 2
    assert not output.exists()


def test_native_file_cohort_cli_rejects_replay_from_wrong_family(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    path, replayed = _manifest(tmp_path)
    monkeypatch.setattr(ev, "load_native_file_replay", lambda *_: replayed[1])
    assert ev.main(["evaluate-native-file-cohort", "--manifest", str(path)]) == 2
