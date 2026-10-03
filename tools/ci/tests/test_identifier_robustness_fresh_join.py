"""Fixed input and route guards for the fresh OSA1 five-product join."""

import copy

import pytest

from tools.benchmark.retrieval import identifier_robustness_fresh_join as fresh


def _profiles():
    native = {
        "candidate_route": "lexical",
        "baseline_route": "semble-lexical-file",
        "execution_profiles": {
            "quanta": {
                "config": {},
                "planning_cost_in_latency": False,
                "policy": "code_search_file",
                "profile_id": "quanta-code-search-file-v1",
            },
            "semble": {"mode": "lexical-file"},
        },
    }
    external = {
        key: {}
        for key in (
            "corpus",
            "cs",
            "opengrok",
            "output_root",
            "query_pack",
            "schema_version",
            "sourcegraph",
            "suite",
        )
    }
    return native, external


def test_fresh_join_accepts_only_default_file_request_profiles():
    native, external = _profiles()
    fresh._require_default_profiles(native, external, "fixture")
    typo = copy.deepcopy(native)
    typo["execution_profiles"]["quanta"]["policy"] = "code_search_typo_file"
    with pytest.raises(fresh.FreshJoinError, match="not default file search"):
        fresh._require_default_profiles(typo, external, "fixture")
    fuzzy = copy.deepcopy(external)
    fuzzy["capability"] = "cs_fuzzy_osa1_file"
    with pytest.raises(fresh.FreshJoinError, match="not default file search"):
        fresh._require_default_profiles(native, fuzzy, "fixture")


def test_fresh_join_repaired_cell_preserves_original_cohort():
    blocked = [{"repository": "repo", "tasks": 5, "status": "BLOCKED"}]
    fingerprints = {
        "blind_pack_sha256": "pack",
        "task_identity_sha256": "tasks",
        "file_universe_sha256": "files",
    }
    custody = [{"source_blocked_originals": {"repo": fingerprints}}]
    assert fresh._reconcile_blocked(blocked, [], custody) == (blocked, [])
    repaired = [{"repository": "repo", **fingerprints}]
    assert fresh._reconcile_blocked(blocked, repaired, custody) == ([], blocked)
    for key in fingerprints:
        changed = [{**repaired[0], key: "drift"}]
        with pytest.raises(fresh.FreshJoinError, match="changes the fixed original query cohort"):
            fresh._reconcile_blocked(blocked, changed, custody)
