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


@pytest.mark.parametrize("failure", [None, "source", "inputs", "mode"])
def test_matrix_verification_binds_every_native_cell(tmp_path, monkeypatch, failure):
    value = _spec(tmp_path)
    value["query_families"] = ["prose"]
    value["cells"] = [_cell(tmp_path, family="prose")]
    spec_path = tmp_path / "matrix.json"
    spec_path.write_text(json.dumps(value))
    release = tmp_path / "release"
    release.mkdir()
    (release / "manifest.json").write_bytes(b"manifest")
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

    def pair(_repo, root, _registry):
        mode = next(mode for mode in matrix.MODES if root.name.endswith(mode))
        route = (
            "lexical"
            if failure == "mode" and mode == "semantic-only"
            else matrix.PAIR_MODES[mode][0]
        )
        pair_spec = {
            "scope": "exploratory",
            "claims": {"quality": False, "speed": False, "same_model": False, "incremental": False},
            "routes": [route],
            "candidate_route": matrix.PAIR_MODES[mode][0],
            "execution_profiles": {"semble": {"mode": matrix.PAIR_MODES[mode][1]}},
            "top_k": 10,
        }
        return (
            {"revision": "b" * 40 if failure == "source" else "a" * 40, "dirty": False},
            pair_spec,
            {
                "manifest": b"manifest",
                "suite": suite,
                "query_pack": b"changed" if failure == "inputs" else pack,
            },
        )

    monkeypatch.setattr(matrix, "_pair", pair)
    if failure is None:
        result = matrix.verify(tmp_path, spec_path)
        assert result["verified_cells"] == result["expected_cells"] == 3
        assert result["status"] == "diagnostic_unqualified"
    else:
        with pytest.raises(
            ValueError, match="source or corpus/query binding differs|native inputs|route"
        ):
            matrix.verify(tmp_path, spec_path)


def test_matrix_mode_requires_matching_single_route_and_exploratory_claims():
    spec = {
        "scope": "exploratory",
        "claims": {"quality": False, "speed": False, "same_model": False, "incremental": False},
        "routes": ["hybrid"],
        "candidate_route": "hybrid",
        "execution_profiles": {"semble": {"mode": "hybrid-no-rerank"}},
        "top_k": 10,
    }
    matrix._mode(spec, "hybrid")
    for mutation in ({"routes": ["lexical"]}, {"claims": {"quality": True}}, {"top_k": True}):
        with pytest.raises(ValueError, match="another route, mode or claim"):
            matrix._mode({**spec, **mutation}, "hybrid")
