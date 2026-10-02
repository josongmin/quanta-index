"""A declared code-search matrix cannot silently omit or substitute native cells."""

import copy
import hashlib
import json
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "benchmark"))

import code_search_matrix as matrix  # noqa: E402

from tools.benchmark.retrieval import evaluator  # noqa: E402
from tools.ci.tests.test_holdout_c4 import _matrix_fixture  # noqa: E402


def _cell(tmp_path, repository="repo-a", family="symbols"):
    return {
        "repository": repository,
        "view": next(iter(matrix.corpus_release.VIEWS)),
        "query_family": family,
        "query_policy": "native",
        "suite": str(tmp_path / f"{repository}-{family}-suite.json"),
        "query_pack": str(tmp_path / f"{repository}-{family}-pack.json"),
        "captures": {
            mode: {"kind": "pair", "root": str(tmp_path / f"{repository}-{family}-{mode}")}
            for mode in matrix.MODES
        },
    }


def _spec(tmp_path):
    return {
        "schema_version": 2,
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
        lambda value: value["cells"][0].update(query_policy=["native"]),
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
    value["schema_version"] = 1
    with pytest.raises(ValueError, match="closed schema v2"):
        matrix._spec(value, {"repo-a", "repo-b"})


def _c4_matrix_fixture(tmp_path, monkeypatch):
    release = tmp_path / "release"
    release.mkdir()
    (release / "manifest.json").write_bytes(b"manifest")
    admission_root = tmp_path / "c4-admission"
    admission_root.mkdir()
    intent = "declaration_name_exact"
    cell_root = admission_root / "repo-a" / intent
    cell_root.mkdir(parents=True)
    suite = {"tasks": [{"task_id": "q", "query": "Alpha", "split": "eval"}]}
    pack = {"tasks": [{"task_id": "q", "query": "Alpha"}]}
    report = {"status": "diagnostic_unqualified", "selected": 1}
    payloads = (suite, pack, report)
    digests = {}
    for name, payload, field in (
        ("suite.json", suite, "suite_sha256"),
        ("blind-pack.json", pack, "blind_pack_sha256"),
        ("admission.json", report, "admission_sha256"),
    ):
        raw = evaluator.canonical(payload)
        (cell_root / name).write_bytes(raw)
        digests[field] = hashlib.sha256(raw).hexdigest()
    c4_cells = [
        {
            "repository": "repo-a",
            "intent": family,
            "status": "diagnostic_unqualified" if family == intent else "no_admission_diagnostic",
            "suite_sha256": digests["suite_sha256"] if family == intent else None,
            "blind_pack_sha256": digests["blind_pack_sha256"] if family == intent else None,
            "admission_sha256": digests["admission_sha256"] if family == intent else None,
            "reason": None if family == intent else "no_tasks_for_intent",
            "candidate_task_ids": ["q"] if family == intent else [],
            "excluded": [],
        }
        for family in matrix.holdout_c4.MATRIX_INTENTS
    ]
    c4 = {"release_digest": "sha256:" + "a" * 64, "cells": c4_cells}
    (admission_root / "admission-matrix.json").write_text(json.dumps(c4))

    def derive(_release, _capsules, _checkouts, *, expected_repositories, _payloads):
        assert _release == release and expected_repositories == 1
        _payloads[("repo-a", intent)] = payloads
        return c4

    monkeypatch.setattr(matrix.holdout_c4, "derive_matrix", derive)
    monkeypatch.setattr(
        matrix.code_search_workflow, "_source", lambda _repo: {"revision": "a" * 40, "dirty": False}
    )
    monkeypatch.setattr(
        matrix.corpus_release,
        "validate",
        lambda _root: {
            "digest": c4["release_digest"],
            "repositories": [
                {
                    "recipe": {"name": "repo-a"},
                    "views": {"code_only": {"manifest": "manifest.json"}},
                }
            ],
        },
    )
    monkeypatch.setattr(matrix.corpus_binding, "_bind", lambda *_args: {"binding": "fixed"})
    monkeypatch.setattr(matrix, "load_registry", lambda _path: {})
    cells = []
    for family in matrix.holdout_c4.MATRIX_INTENTS:
        admitted = family == intent
        captures = (
            {
                "lexical-only": {"kind": "not_run", "reason": matrix.MISSING_REASON},
                "semantic-only": {"kind": "unsupported", "reason": matrix.UNSUPPORTED_REASON},
                "hybrid": {"kind": "unsupported", "reason": matrix.UNSUPPORTED_REASON},
            }
            if admitted
            else {
                mode: {"kind": "not_applicable", "reason": matrix.NO_ADMISSION_REASON}
                for mode in matrix.MODES
            }
        )
        cells.append(
            {
                "repository": "repo-a",
                "view": "code_only",
                "query_family": family,
                "query_policy": matrix.holdout_c4._execution_policy(family),
                "suite": str(cell_root / "suite.json") if admitted else None,
                "query_pack": str(cell_root / "blind-pack.json") if admitted else None,
                "captures": captures,
            }
        )
    spec = {
        "schema_version": 3,
        "release_path": str(release),
        "release_digest": c4["release_digest"],
        "query_families": list(matrix.holdout_c4.MATRIX_INTENTS),
        "c4_admission_root": str(admission_root),
        "c4_capsules": str(tmp_path / "capsules"),
        "c4_checkouts": str(tmp_path / "checkouts"),
        "cells": cells,
    }
    spec_path = tmp_path / "matrix-v3.json"
    spec_path.write_text(json.dumps(spec))
    return spec, spec_path, admission_root


def test_matrix_v3_replays_c4_no_admission_without_inventing_inputs(tmp_path, monkeypatch):
    spec, spec_path, admission_root = _c4_matrix_fixture(tmp_path, monkeypatch)
    result = matrix.verify(tmp_path, spec_path)
    assert result["schema_version"] == 3
    assert result["no_admission_cells"] == len(matrix.holdout_c4.MATRIX_INTENTS) - 1
    assert result["no_admission_mode_cells"] == 3 * result["no_admission_cells"]
    assert all(row["reason"] == "no_tasks_for_intent" for row in result["no_admission"])
    assert (result["verified_cells"], result["not_run_cells"], result["unsupported_cells"]) == (
        0,
        1,
        2,
    )
    assert result["status"] == "diagnostic_incomplete"
    captured = copy.deepcopy(spec)
    admitted_capture = next(
        cell for cell in captured["cells"] if cell["query_family"] == "declaration_name_exact"
    )
    admitted_capture["captures"]["lexical-only"] = {
        "kind": "pair",
        "root": str(tmp_path / "pair-capture"),
    }
    source = {"revision": "a" * 40, "dirty": False}

    def pair(_repo, root, _registry):
        assert root == tmp_path / "pair-capture"
        baseline = matrix.pair_run.SEMBLE_ROUTE_BY_MODE["lexical-file"]
        return (
            source,
            {
                "scope": "exploratory",
                "claims": {
                    "quality": False,
                    "speed": False,
                    "same_model": False,
                    "incremental": False,
                },
                "routes": ["lexical"],
                "candidate_route": "lexical",
                "baseline_route": baseline,
                "semble_route": baseline,
                "execution_profiles": {
                    "quanta": matrix.query_plan.execution_profile("code_search_file"),
                    "semble": {"mode": "lexical-file"},
                },
                "top_k": 10,
            },
            {
                "manifest": b"manifest",
                "suite": (
                    admission_root / "repo-a" / "declaration_name_exact" / "suite.json"
                ).read_bytes(),
                "query_pack": (
                    admission_root / "repo-a" / "declaration_name_exact" / "blind-pack.json"
                ).read_bytes(),
            },
        )

    monkeypatch.setattr(matrix, "_pair", pair)
    spec_path.write_text(json.dumps(captured))
    captured_result = matrix.verify(tmp_path, spec_path)
    assert captured_result["status"] == "diagnostic_partial_admission"
    assert (captured_result["pairs"], captured_result["not_run_cells"]) == (1, 0)
    spec_path.write_text(json.dumps(spec))
    changed = copy.deepcopy(spec)
    changed["cells"][0]["captures"]["lexical-only"] = {
        "kind": "not_run",
        "reason": matrix.MISSING_REASON,
    }
    with pytest.raises(ValueError, match="no-admission cell"):
        matrix._spec(changed, {"repo-a"})
    changed = copy.deepcopy(spec)
    admitted_cell = next(
        cell for cell in changed["cells"] if cell["query_family"] == "declaration_name_exact"
    )
    admitted_cell["suite"] = admitted_cell["query_pack"] = None
    admitted_cell["captures"] = {
        mode: {"kind": "not_applicable", "reason": matrix.NO_ADMISSION_REASON}
        for mode in matrix.MODES
    }
    spec_path.write_text(json.dumps(changed))
    with pytest.raises(ValueError, match="cell inputs differ"):
        matrix.verify(tmp_path, spec_path)
    spec_path.write_text(json.dumps(spec))
    suite_path = admission_root / "repo-a" / "declaration_name_exact" / "suite.json"
    original_suite = suite_path.read_bytes()
    suite_path.write_bytes(b"{}")
    with pytest.raises(ValueError, match="emitted input differs"):
        matrix.verify(tmp_path, spec_path)
    suite_path.write_bytes(original_suite)
    forged = json.loads((admission_root / "admission-matrix.json").read_text())
    forged["cells"][1]["status"] = "no_admission_diagnostic"
    (admission_root / "admission-matrix.json").write_text(json.dumps(forged))
    with pytest.raises(ValueError, match="C4 admission differs"):
        matrix.verify(tmp_path, spec_path)


def test_matrix_v3_rederives_source_bound_c4_cells(tmp_path, monkeypatch):
    release, capsules, checkouts = _matrix_fixture(tmp_path, monkeypatch)
    output = tmp_path / "c4-output"
    admitted = matrix.holdout_c4.write_matrix(
        release, capsules, checkouts, output, expected_repositories=2, emit_suites=True
    )
    source = {"revision": "a" * 40, "dirty": False}
    monkeypatch.setattr(matrix.code_search_workflow, "_source", lambda _repo: source)
    monkeypatch.setattr(
        matrix.corpus_release,
        "validate",
        lambda _release: matrix.holdout_c4._read(release / "release.json"),
    )
    monkeypatch.setattr(matrix, "load_registry", lambda _path: {})
    cells = []
    for row in admitted["cells"]:
        selected = row["status"] == "diagnostic_unqualified"
        root = output / row["repository"] / row["intent"]
        cells.append(
            {
                "repository": row["repository"],
                "view": "code_only",
                "query_family": row["intent"],
                "query_policy": matrix.holdout_c4._execution_policy(row["intent"]),
                "suite": str(root / "suite.json") if selected else None,
                "query_pack": str(root / "blind-pack.json") if selected else None,
                "captures": (
                    {
                        "lexical-only": {"kind": "not_run", "reason": matrix.MISSING_REASON},
                        "semantic-only": {
                            "kind": "unsupported",
                            "reason": matrix.UNSUPPORTED_REASON,
                        },
                        "hybrid": {"kind": "unsupported", "reason": matrix.UNSUPPORTED_REASON},
                    }
                    if selected
                    else {
                        mode: {"kind": "not_applicable", "reason": matrix.NO_ADMISSION_REASON}
                        for mode in matrix.MODES
                    }
                ),
            }
        )
    spec = {
        "schema_version": 3,
        "release_path": str(release),
        "release_digest": admitted["release_digest"],
        "query_families": list(matrix.holdout_c4.MATRIX_INTENTS),
        "c4_admission_root": str(output),
        "c4_capsules": str(capsules),
        "c4_checkouts": str(checkouts),
        "cells": cells,
    }
    spec_path = tmp_path / "matrix-v3.json"
    spec_path.write_text(json.dumps(spec))
    capture_roots = {
        f"{row['repository']}/{row['intent']}": None
        for row in admitted["cells"]
        if row["status"] == "diagnostic_unqualified"
    }
    assert matrix.build_c4_spec(release, capsules, checkouts, output, capture_roots) == spec
    roots_path = tmp_path / "capture-roots.json"
    roots_path.write_text(json.dumps(capture_roots))
    written_path = tmp_path / "written-matrix-v3.json"
    assert (
        matrix.write_c4_spec(release, capsules, checkouts, output, roots_path, written_path) == spec
    )
    assert json.loads(written_path.read_bytes()) == spec
    with pytest.raises(ValueError, match="fresh and external"):
        matrix.write_c4_spec(release, capsules, checkouts, output, roots_path, written_path)
    capture_key = next(iter(capture_roots))
    captured_spec = matrix.build_c4_spec(
        release, capsules, checkouts, output, {capture_key: str(tmp_path / "new-pair")}
    )
    captured_cell = next(cell for cell in captured_spec["cells"] if cell["suite"] is not None)
    assert captured_cell["captures"]["lexical-only"] == {
        "kind": "pair",
        "root": str(tmp_path / "new-pair"),
    }
    with pytest.raises(ValueError, match="canonical absolute path"):
        matrix.build_c4_spec(release, capsules, checkouts, output, {capture_key: "relative"})
    with pytest.raises(ValueError, match="capture-root inventory differs"):
        matrix.build_c4_spec(release, capsules, checkouts, output, {})
    result = matrix.verify(tmp_path, spec_path)
    assert result["no_admission_cells"] == sum(
        row["status"] == "no_admission_diagnostic" for row in admitted["cells"]
    )
    assert result["not_run_cells"] == sum(
        row["status"] == "diagnostic_unqualified" for row in admitted["cells"]
    )
    assert result["verified_cells"] == 0


@pytest.mark.parametrize("policy", sorted(matrix.LEXICAL_ONLY_FILE_POLICIES))
def test_matrix_file_policy_requires_pair_and_explicit_unsupported_modes(tmp_path, policy):
    value = _spec(tmp_path)
    value["cells"] = [_cell(tmp_path, "repo-a", "symbols")]
    value["query_families"] = ["symbols"]
    cell = value["cells"][0]
    cell["query_policy"] = policy
    for mode in ("semantic-only", "hybrid"):
        cell["captures"][mode] = {
            "kind": "unsupported",
            "reason": matrix.UNSUPPORTED_REASON,
        }
    assert matrix._spec(value, {"repo-a"}) == value
    for mutation in (
        {"kind": "pair", "root": str(tmp_path / "unexpected")},
        {"kind": "not_run", "reason": matrix.MISSING_REASON},
        {"kind": "unsupported", "reason": "unknown"},
    ):
        changed = copy.deepcopy(value)
        changed["cells"][0]["captures"]["semantic-only"] = mutation
        with pytest.raises(ValueError, match="matrix"):
            matrix._spec(changed, {"repo-a"})
    changed = copy.deepcopy(value)
    changed["cells"][0]["query_policy"] = "native"
    with pytest.raises(ValueError, match="matrix"):
        matrix._spec(changed, {"repo-a"})
    changed = copy.deepcopy(value)
    changed["cells"][0]["captures"]["lexical-only"] = {
        "kind": "unsupported",
        "reason": matrix.UNSUPPORTED_REASON,
    }
    with pytest.raises(ValueError, match="supported query policy/mode"):
        matrix._spec(changed, {"repo-a"})


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


def test_matrix_admits_frozen_queries_and_rejects_label_leakage():
    matrix._admit_queries({"tasks": [{"query": "read request body"}]}, "natural_language")
    matrix._admit_queries({"tasks": [{"query": "Handler123"}]}, "code_search_file")
    matrix._admit_queries({"tasks": [{"query": "Handler123"}]}, "code_search_typo_file")
    matrix._admit_queries(
        {"tasks": [{"query": "read request body"}]}, "code_search_exact_content_file"
    )
    for pack, policy in (
        ({"tasks": [{"query": " ".join(f"word{i}" for i in range(33))}]}, "natural_language"),
        ({"tasks": [{"query": "x" * 97}]}, "natural_language"),
        ({"tasks": [{"query": "word!"}]}, "code_search_file"),
        ({"tasks": [{"query": "ab"}]}, "code_search_typo_file"),
        ({"tasks": [{"query": "x" * 257}]}, "code_search_exact_content_file"),
        ({"tasks": [{"query": "word", "gold": []}]}, "native"),
        ({"tasks": [{"query": "word"}], "gold": []}, "native"),
    ):
        with pytest.raises(ValueError, match="matrix"):
            matrix._admit_queries(pack, policy)


@pytest.mark.parametrize("policy", sorted(matrix.LEXICAL_ONLY_FILE_POLICIES))
@pytest.mark.parametrize("failure", [None, "profile", "semble", "missing"])
def test_matrix_code_search_file_pair_is_file_only_and_no_external_workflow(
    tmp_path, monkeypatch, failure, policy
):
    value = _spec(tmp_path)
    value["query_families"] = ["code-search"]
    value["cells"] = [_cell(tmp_path, family="code-search")]
    cell = value["cells"][0]
    cell["query_policy"] = policy
    for mode in ("semantic-only", "hybrid"):
        cell["captures"][mode] = {
            "kind": "unsupported",
            "reason": matrix.UNSUPPORTED_REASON,
        }
    if failure == "missing":
        cell["captures"]["lexical-only"] = {
            "kind": "not_run",
            "reason": matrix.MISSING_REASON,
        }
    spec_path = tmp_path / "matrix.json"
    spec_path.write_text(json.dumps(value))
    release = tmp_path / "release"
    release.mkdir()
    (release / "manifest.json").write_bytes(b"manifest")
    query = "Handler123" if policy == "code_search_typo_file" else "read request"
    suite = json.dumps({"tasks": [{"task_id": "q", "query": query, "split": "eval"}]}).encode()
    pack = json.dumps({"tasks": [{"task_id": "q", "query": query}]}).encode()
    Path(cell["suite"]).write_bytes(suite)
    Path(cell["query_pack"]).write_bytes(pack)
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
                    "views": {cell["view"]: {"manifest": "manifest.json"}},
                }
            ],
        },
    )
    monkeypatch.setattr(matrix, "load_registry", lambda path: {})
    monkeypatch.setattr(matrix.corpus_binding, "_bind", lambda *args: {"binding": "fixed"})

    def pair(_repo, _root, _registry):
        assert _root == Path(cell["captures"]["lexical-only"]["root"])
        semble_mode = "lexical-only" if failure == "semble" else "lexical-file"
        return (
            source,
            {
                "scope": "exploratory",
                "claims": {
                    "quality": False,
                    "speed": False,
                    "same_model": False,
                    "incremental": False,
                },
                "routes": ["lexical"],
                "candidate_route": "lexical",
                "baseline_route": matrix.pair_run.SEMBLE_ROUTE_BY_MODE[semble_mode],
                "semble_route": matrix.pair_run.SEMBLE_ROUTE_BY_MODE[semble_mode],
                "execution_profiles": {
                    "quanta": matrix.query_plan.execution_profile(
                        "native" if failure == "profile" else policy
                    ),
                    "semble": {"mode": semble_mode},
                },
                "top_k": 10,
            },
            {"manifest": b"manifest", "suite": suite, "query_pack": pack},
        )

    monkeypatch.setattr(matrix, "_pair", pair)
    monkeypatch.setattr(
        matrix.code_search_workflow,
        "verify",
        lambda *_: pytest.fail("file policies must not use the five-product workflow"),
    )
    if failure in {"profile", "semble"}:
        with pytest.raises(ValueError, match="route, mode or claim"):
            matrix.verify(tmp_path, spec_path)
    else:
        result = matrix.verify(tmp_path, spec_path)
        assert result["expected_cells"] == 3
        assert result["unsupported_cells"] == 2
        assert result["not_run_cells"] == (1 if failure == "missing" else 0)
        assert result["verified_cells"] == (0 if failure == "missing" else 1)
        assert result["status"] == (
            "diagnostic_incomplete" if failure == "missing" else "diagnostic_unqualified"
        )


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
