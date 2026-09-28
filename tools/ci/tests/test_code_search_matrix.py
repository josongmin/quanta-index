"""A declared code-search matrix cannot silently omit or substitute native cells."""

import copy
import json
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "benchmark"))

import code_search_matrix as matrix  # noqa: E402


def _cell(tmp_path, repository="repo-a", family="symbols"):
    return {
        "repository": repository,
        "view": next(iter(matrix.corpus_release.VIEWS)),
        "query_family": family,
        "suite": str(tmp_path / f"{repository}-{family}-suite.json"),
        "query_pack": str(tmp_path / f"{repository}-{family}-pack.json"),
        "captures": {
            mode: {"kind": "pair", "root": str(tmp_path / f"{repository}-{family}-{mode}")}
            for mode in matrix.MODES
        },
    }


def _spec(tmp_path):
    return {
        "schema_version": 1,
        "release_path": str(tmp_path / "release"),
        "release_digest": "sha256:" + "a" * 64,
        "query_families": ["symbols", "prose"],
        "cells": [
            _cell(tmp_path, repository, family)
            for repository in ("repo-a", "repo-b")
            for family in ("symbols", "prose")
        ],
    }


@pytest.mark.parametrize(
    "mutation",
    [
        lambda value: value["cells"].pop(),
        lambda value: value["cells"].append(copy.deepcopy(value["cells"][0])),
        lambda value: value["cells"][0]["captures"].pop("semantic-only"),
        lambda value: value["cells"][0]["captures"]["hybrid"].update(
            root=value["cells"][0]["captures"]["lexical-only"]["root"]
        ),
        lambda value: value["cells"][0].update(repository=["repo-a"]),
        lambda value: value["cells"][0]["captures"]["hybrid"].update(kind=[]),
        lambda value: value.update(query_families=["symbols"]),
        lambda value: value["cells"][1].update(
            suite=value["cells"][0]["suite"], query_pack=value["cells"][0]["query_pack"]
        ),
    ],
)
def test_matrix_inventory_refuses_missing_duplicate_or_malformed_cells(tmp_path, mutation):
    value = _spec(tmp_path)
    mutation(value)
    with pytest.raises(ValueError, match="matrix"):
        matrix._spec(value, {"repo-a", "repo-b"})


def test_matrix_inventory_accepts_only_complete_cartesian_product(tmp_path):
    value = _spec(tmp_path)
    assert matrix._spec(value, {"repo-a", "repo-b"}) == value


@pytest.mark.parametrize("cohort", ["prose", "negative-bare"])
@pytest.mark.parametrize("failure", [None, "source", "inputs", "mode", "policy", "kind"])
def test_matrix_verification_binds_every_native_cell(tmp_path, monkeypatch, failure, cohort):
    value = _spec(tmp_path)
    value["query_families"] = [cohort]
    value["cells"] = [_cell(tmp_path, family=cohort)]
    if cohort == "negative-bare":
        value["cells"][0]["captures"]["lexical-only"]["kind"] = "workflow"
    if failure == "kind":
        value["cells"][0]["captures"]["lexical-only"]["kind"] = (
            "pair" if cohort == "negative-bare" else "workflow"
        )
    spec_path = tmp_path / "matrix.json"
    spec_path.write_text(json.dumps(value))
    release = tmp_path / "release"
    release.mkdir()
    (release / "manifest.json").write_bytes(b"manifest")
    if cohort == "negative-bare":
        suite = b'{"tasks":[{"task_id":"q","answerable":false,"gold":[]}]}'
        pack = b'{"tasks":[{"task_id":"q","query":"symbol"}]}'
    else:
        suite, pack = b"{}", b'{"tasks":[{"query":"two words"}]}'
    Path(value["cells"][0]["suite"]).write_bytes(suite)
    Path(value["cells"][0]["query_pack"]).write_bytes(pack)
    source = {"revision": "a" * 40, "dirty": False}
    monkeypatch.setattr(matrix.code_search_workflow, "_source", lambda repo: source)
    monkeypatch.setattr(
        matrix.corpus_release,
        "validate",
        lambda root: {
            "digest": value["release_digest"],
            "repositories": [
                {
                    "recipe": {"name": "repo-a"},
                    "views": {value["cells"][0]["view"]: {"manifest": "manifest.json"}},
                }
            ],
        },
    )
    monkeypatch.setattr(matrix, "load_registry", lambda path: {})
    monkeypatch.setattr(matrix.corpus_binding, "_bind", lambda *args: {"binding": "fixed"})
    if cohort == "negative-bare":
        monkeypatch.setattr(matrix.lexical, "_file_universe", lambda *_: {"answer.go"})
        monkeypatch.setattr(matrix.lexical, "_tasks", lambda *_: {"q": ("symbol", [])})
        monkeypatch.setattr(
            matrix.code_search_workflow,
            "verify",
            lambda *_: {"source": source, "binding": {"binding": "fixed"}},
        )

    def pair_spec(mode):
        route = (
            "lexical"
            if failure == "mode" and mode == "semantic-only"
            else matrix.PAIR_MODES[mode][0]
        )
        return {
            "scope": "exploratory",
            "claims": {"quality": False, "speed": False, "same_model": False, "incremental": False},
            "routes": [route],
            "candidate_route": matrix.PAIR_MODES[mode][0],
            "baseline_route": matrix.pair_run.SEMBLE_ROUTE_BY_MODE[matrix.PAIR_MODES[mode][1]],
            "semble_route": matrix.pair_run.SEMBLE_ROUTE_BY_MODE[matrix.PAIR_MODES[mode][1]],
            "execution_profiles": {
                "quanta": matrix.query_plan.execution_profile(
                    "natural_language"
                    if failure == "policy" and mode == "semantic-only"
                    else "native"
                ),
                "semble": {"mode": matrix.PAIR_MODES[mode][1]},
            },
            "top_k": 10,
        }

    def pair(_repo, root, _registry):
        mode = next(mode for mode in matrix.MODES if root.name.endswith(mode))
        return (
            {"revision": "b" * 40 if failure == "source" else "a" * 40, "dirty": False},
            pair_spec(mode),
            {
                "manifest": b"manifest",
                "suite": suite,
                "query_pack": b"changed" if failure == "inputs" else pack,
            },
        )

    monkeypatch.setattr(matrix, "_pair", pair)
    if cohort == "negative-bare":
        monkeypatch.setattr(matrix.pair_run, "load_spec", lambda *_: pair_spec("lexical-only"))
    if failure is None:
        result = matrix.verify(tmp_path, spec_path)
        assert result["verified_cells"] == result["expected_cells"] == 3
        assert result["status"] == "diagnostic_unqualified"
    else:
        with pytest.raises(
            ValueError,
            match="source or corpus/query binding differs|native inputs|route|query support",
        ):
            matrix.verify(tmp_path, spec_path)


def test_matrix_mode_requires_matching_single_route_and_exploratory_claims():
    spec = {
        "scope": "exploratory",
        "claims": {"quality": False, "speed": False, "same_model": False, "incremental": False},
        "routes": ["hybrid"],
        "candidate_route": "hybrid",
        "baseline_route": "semble-hybrid",
        "semble_route": "semble-hybrid",
        "execution_profiles": {
            "quanta": matrix.query_plan.execution_profile("native"),
            "semble": {"mode": "hybrid-no-rerank"},
        },
        "top_k": 10,
    }
    matrix._mode(spec, "hybrid")
    for mutation in (
        {"routes": ["lexical"]},
        {"claims": {"quality": True}},
        {"top_k": True},
        {"baseline_route": "semble-semantic-only"},
        {
            "execution_profiles": {
                "quanta": matrix.query_plan.execution_profile("natural_language"),
                "semble": {"mode": "hybrid-no-rerank"},
            }
        },
    ):
        with pytest.raises(ValueError, match="another route, mode or claim"):
            matrix._mode({**spec, **mutation}, "hybrid")


def test_matrix_routes_judged_no_answer_bare_symbols_to_five_product_workflow():
    suite = {"tasks": [{"task_id": "q", "gold": [{"path": "answer.go"}]}]}
    pack = {"tasks": [{"task_id": "q", "query": "symbol"}]}
    assert matrix._supports_lexical_workflow(suite, pack)
    suite["tasks"][0]["gold"] = []
    assert matrix._supports_lexical_workflow(suite, pack)
    suite["tasks"][0].pop("gold")
    assert matrix._supports_lexical_workflow(suite, pack)  # scorer rejects malformed gold
    suite["tasks"][0]["gold"] = [{"path": "answer.go"}]
    pack["tasks"][0]["query"] = "two words"
    assert not matrix._supports_lexical_workflow(suite, pack)


@pytest.mark.parametrize("changed_role", [None, "manifest", "suite", "query_pack", "spec"])
def test_matrix_pair_binds_every_validated_run(tmp_path, monkeypatch, changed_role):
    root = tmp_path / "pair"
    runs = ("case-a", "case-b")
    for run_id in runs:
        raw = root / "runs" / run_id / "raw"
        raw.mkdir(parents=True)
        (raw / "original-spec.json").write_text('{"candidate_route":"semantic"}')
        for role in ("manifest", "suite", "query_pack"):
            (raw / f"input-{role}").write_bytes(role.encode())
    second = root / "runs" / runs[1] / "raw"
    if changed_role == "spec":
        (second / "original-spec.json").write_text('{"candidate_route":"lexical"}')
    elif changed_role is not None:
        (second / f"input-{changed_role}").write_bytes(b"different")

    document = {"source": {"revision": "a" * 40}, "runs": [{"run_id": run} for run in runs]}
    monkeypatch.setattr(matrix.pair_capture, "validate", lambda *_: document)
    monkeypatch.setattr(matrix, "load_capture", lambda *_, **__: document)
    monkeypatch.setattr(matrix, "registry_digest", lambda *_: "registry")
    monkeypatch.setattr(matrix.pair_run, "load_spec", lambda path: json.loads(path.read_bytes()))

    class Store:
        def __init__(self, _root):
            pass

        def load(self, _run_id):
            return {}

    monkeypatch.setattr(matrix, "RunStore", Store)
    if changed_role is None:
        source, spec, inputs = matrix._pair(tmp_path, root, {})
        assert source == document["source"]
        assert spec == {"candidate_route": "semantic"}
        assert inputs == {role: role.encode() for role in ("manifest", "suite", "query_pack")}
    else:
        with pytest.raises(ValueError, match="matrix pair runs differ"):
            matrix._pair(tmp_path, root, {})
