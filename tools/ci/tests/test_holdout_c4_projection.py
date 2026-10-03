"""Fixed-cohort C4 projection uses current source truth and a pinned old selector."""

from __future__ import annotations

import copy
import hashlib

import pytest

from tools.benchmark.retrieval import evaluator, holdout_c4, query_plan
from tools.ci.tests.test_holdout_c4 import _fixture, _resign


def _two_task_suites(tmp_path, monkeypatch):
    release, capsule, checkout = _fixture(
        tmp_path,
        monkeypatch,
        additional_files={"other.go": "package demo\nfunc Zeta() {}\n"},
    )
    raw = (checkout / "other.go").read_bytes()
    gold = holdout_c4._read(capsule / "gold.json")
    blind = holdout_c4._read(capsule / "blind.json")
    for payload in (gold, blind):
        second = copy.deepcopy(payload["tasks"][0])
        second.update(task_id="toy.def.002", query="Zeta", query_family_id="toy.name.Zeta")
        payload["tasks"].append(second)
    gold["tasks"][1]["labels"] = [
        {
            **gold["tasks"][0]["labels"][0],
            "path": "other.go",
            "file_sha256": hashlib.sha256(raw).hexdigest(),
            "start_byte": raw.index(b"Zeta"),
            "end_byte": raw.index(b"Zeta") + 4,
            "local_name": "Zeta",
        }
    ]
    _resign(capsule, "gold.json", gold)
    _resign(capsule, "blind.json", blind)
    fresh, _pack, _report = holdout_c4.derive(release, capsule, checkout, "declaration_name_exact")
    legacy = copy.deepcopy(fresh)
    legacy["tasks"] = legacy["tasks"][:1]
    legacy["suite_id"] += "-legacy"
    legacy["tasks"][0]["source_oracle"]["declaration_exclusions"] = ["other.go"]
    return checkout, fresh, legacy


def _project(checkout, fresh, legacy, *, default_file_typo=False):
    raw = evaluator.canonical(legacy)
    return holdout_c4.project_fixed_cohort(
        checkout,
        fresh,
        raw,
        hashlib.sha256(raw).hexdigest(),
        suite_id="toy-new-fixed-cohort",
        default_file_typo=default_file_typo,
    )


def test_fixed_cohort_selects_old_ids_with_fresh_oracle_metadata(tmp_path, monkeypatch):
    checkout, fresh, legacy = _two_task_suites(tmp_path, monkeypatch)
    suite, pack, lineage = _project(checkout, fresh, legacy)
    assert [task["task_id"] for task in suite["tasks"]] == ["toy.def.001"]
    assert suite["tasks"][0]["source_oracle"] == fresh["tasks"][0]["source_oracle"]
    assert [task["task_id"] for task in pack["tasks"]] == ["toy.def.001"]
    assert pack["suite_commitment_sha256"] == evaluator.digest(evaluator.canonical(suite))
    assert lineage["fresh_selected"] == 2
    assert lineage["fixed_selected"] == 1


@pytest.mark.parametrize("field", ["query", "query_sha256", "answerable", "file_judgments", "gold"])
def test_fixed_cohort_rejects_changed_selected_truth(tmp_path, monkeypatch, field):
    checkout, fresh, legacy = _two_task_suites(tmp_path, monkeypatch)
    legacy["tasks"][0][field] = None
    with pytest.raises(ValueError, match="selected query or truth changed"):
        _project(checkout, fresh, legacy)


def test_fixed_cohort_rejects_unknown_reordered_and_source_change(tmp_path, monkeypatch):
    checkout, fresh, legacy = _two_task_suites(tmp_path, monkeypatch)
    unknown = copy.deepcopy(legacy)
    unknown["tasks"][0]["task_id"] = "unknown"
    with pytest.raises(ValueError, match="unknown, duplicated, or reordered"):
        _project(checkout, fresh, unknown)
    reordered = copy.deepcopy(fresh)
    reordered["tasks"].reverse()
    reordered["suite_id"] += "-legacy"
    with pytest.raises(ValueError, match="unknown, duplicated, or reordered"):
        _project(checkout, fresh, reordered)
    wrong_source = copy.deepcopy(legacy)
    wrong_source["repository_commit"] = "0" * 40
    with pytest.raises(ValueError, match="source or contract differs"):
        _project(checkout, fresh, wrong_source)


def test_fixed_cohort_rejects_wrong_digest_and_invalid_fresh(tmp_path, monkeypatch):
    checkout, fresh, legacy = _two_task_suites(tmp_path, monkeypatch)
    raw = evaluator.canonical(legacy)
    with pytest.raises(ValueError, match="byte commitment differs"):
        holdout_c4.project_fixed_cohort(
            checkout, fresh, raw, "0" * 64, suite_id="toy-new-fixed-cohort"
        )
    invalid = copy.deepcopy(fresh)
    invalid["file_universe_digest"] = "0" * 64
    with pytest.raises(evaluator.EvidenceError, match="file universe digest mismatch"):
        _project(checkout, invalid, legacy)


def test_fixed_cohort_typo_default_request_is_explicit(tmp_path, monkeypatch):
    checkout, fresh, legacy = _two_task_suites(tmp_path, monkeypatch)
    with pytest.raises(ValueError, match="requires explicit OSA1 tasks"):
        _project(checkout, fresh, legacy, default_file_typo=True)


def test_fixed_cohort_osa1_default_projection_regenerates_pack(tmp_path, monkeypatch):
    release, capsule, checkout = _fixture(tmp_path, monkeypatch)
    gold = holdout_c4._read(capsule / "gold.json")
    blind = holdout_c4._read(capsule / "blind.json")
    for payload in (gold, blind):
        payload["tasks"][0].update(
            intent="declaration_name_osa1_casefold", query="Alphb", case_semantics="casefold"
        )
    gold["tasks"][0].update(
        intended_name="Alpha",
        near_declaration_state="complete",
        near_declaration_names=["Alpha"],
        near_declaration_files=["main.go"],
        exact_collision_names=[],
        exact_collision_files=[],
    )
    _resign(capsule, "gold.json", gold)
    _resign(capsule, "blind.json", blind)
    fresh, _pack, _report = holdout_c4.derive(
        release, capsule, checkout, "declaration_name_osa1_casefold"
    )
    legacy = copy.deepcopy(fresh)
    legacy["suite_id"] += "-legacy"
    legacy["routes"] = ["lexical", "semble-lexical-file"]
    legacy["tasks"][0]["evaluation_contract"]["request_mode"] = query_plan.DEFAULT_FILE_SEARCH
    suite, pack, lineage = _project(checkout, fresh, legacy, default_file_typo=True)
    assert suite["routes"] == ["lexical", "semble-lexical-file"]
    assert (
        suite["tasks"][0]["evaluation_contract"]["request_mode"] == query_plan.DEFAULT_FILE_SEARCH
    )
    assert pack["suite_commitment_sha256"] == evaluator.digest(evaluator.canonical(suite))
    assert lineage["default_file_typo"] is True
    wrong_selector = copy.deepcopy(legacy)
    wrong_selector["tasks"][0]["evaluation_contract"]["request_mode"] = query_plan.EXPLICIT_OSA1_TYPO
    with pytest.raises(ValueError, match="legacy ordinary-file typo mode differs"):
        _project(checkout, fresh, wrong_selector, default_file_typo=True)
