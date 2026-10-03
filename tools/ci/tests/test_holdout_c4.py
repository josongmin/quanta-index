"""Independent fixed-source admission checks for the holdout C4 adapter."""

from __future__ import annotations

import hashlib
import os
import shutil
import subprocess
import sys
from pathlib import Path

import pytest

from tools.benchmark.retrieval import evaluator, gold_oracle, holdout_c4, query_plan, source_oracle
from tools.ci.tests.test_corpus_binding import split_releases  # noqa: F401


def _fixture(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    language: str = "go",
    *,
    parser_refusal: bool = False,
    checker_disagreement: bool = False,
    disputed_name: str = "Other",
    near_alternative: bool = False,
):
    checkout = tmp_path / "checkout"
    checkout.mkdir()
    syntax = {
        "go": ("main.go", "package demo\nfunc Alpha() {}\nfunc AlphaBeta() {}\n"),
        "rust": ("main.rs", "fn Alpha() {}\nfn AlphaBeta() {}\n"),
        "python": ("main.py", "def Alpha():\n    pass\ndef AlphaBeta():\n    pass\n"),
        "typescript": ("main.ts", "function Alpha() {}\nfunction AlphaBeta() {}\n"),
        "javascript": ("main.js", "function Alpha() {}\nfunction AlphaBeta() {}\n"),
    }
    file_name, body = syntax[language]
    if near_alternative:
        assert language == "go"
        body += "func Alphi() {}\n"
    (checkout / file_name).write_text(body, encoding="utf-8")
    if parser_refusal:
        assert language == "go"
        (checkout / "broken.go").write_text("package demo\nfunc Broken(", encoding="utf-8")
    if checker_disagreement:
        assert language == "go"
        (checkout / "disputed.go").write_text(
            f"package demo\nfunc {disputed_name}() {{}}\n", encoding="utf-8"
        )
    subprocess.run(["git", "init", "-q", str(checkout)], check=True)
    subprocess.run(["git", "-C", str(checkout), "add", "."], check=True)
    subprocess.run(
        [
            "git",
            "-C",
            str(checkout),
            "-c",
            "user.name=Test",
            "-c",
            "user.email=t@example.com",
            "commit",
            "-qm",
            "fixture",
        ],
        check=True,
    )
    commit = (
        subprocess.check_output(["git", "-C", str(checkout), "rev-parse", "HEAD"]).decode().strip()
    )
    source = (checkout / file_name).read_bytes()
    sha = hashlib.sha256(source).hexdigest()
    universe = [{"path": file_name, "file_sha256": sha}]
    if parser_refusal:
        universe.append(
            {
                "path": "broken.go",
                "file_sha256": hashlib.sha256((checkout / "broken.go").read_bytes()).hexdigest(),
            }
        )
    if checker_disagreement:
        universe.append(
            {
                "path": "disputed.go",
                "file_sha256": hashlib.sha256((checkout / "disputed.go").read_bytes()).hexdigest(),
            }
        )
    universe.sort(key=lambda row: row["path"])
    manifest = {"repository_commit": commit, "files": universe}
    release = tmp_path / "release"
    (release / "manifests" / "toy").mkdir(parents=True)
    manifest_path = release / "manifests" / "toy" / "code_only.json"
    manifest_path.write_bytes(holdout_c4._raw(manifest))
    release_digest = "sha256:" + "a" * 64
    document = {
        "digest": release_digest,
        "repositories": [
            {
                "recipe": {"name": "toy", "language": language, "revision": commit},
                "views": {
                    "code_only": {
                        "manifest": "manifests/toy/code_only.json",
                        "manifest_digest": "sha256:"
                        + hashlib.sha256(manifest_path.read_bytes()).hexdigest(),
                        "file_universe_digest": "sha256:" + evaluator.universe_digest(universe),
                    }
                },
            }
        ],
    }
    (release / "release.json").write_bytes(holdout_c4._raw(document))
    capsule = tmp_path / "capsule"
    capsule.mkdir()
    common = {
        "task_id": "toy.def.001",
        "query_family_id": "toy.name.Alpha",
        "intent": "declaration_name_exact",
        "query": "Alpha",
        "scope_prefix": "",
        "language": language,
        "case_semantics": "sensitive",
        "normalization": "none_raw_utf8",
    }
    label = {
        "path": file_name,
        "file_sha256": sha,
        "start_byte": source.index(b"Alpha"),
        "end_byte": source.index(b"Alpha") + len(b"Alpha"),
        "kind": "function_declaration",
        "local_name": "Alpha",
    }
    gold_task = {
        **common,
        "answerable": True,
        "census_text_excluded": (
            [{"path": "broken.go", "reason": "census_refused"}] if parser_refusal else []
        )
        + (
            [{"path": "disputed.go", "reason": "census_disagreement"}]
            if checker_disagreement
            else []
        ),
        "label_state": "mechanical_unreviewed",
        "labels": [label],
        "split": "holdout",
        "unsupported": [],
    }
    metadata = {
        "release_digest": release_digest,
        "repository": "toy",
        "repository_commit": commit,
        "manifest_digest": document["repositories"][0]["views"]["code_only"]["manifest_digest"],
    }
    payloads = {
        "selection.json": {
            "release_path": str(release),
            "release_digest": release_digest,
            "repository": "toy",
            "view": "code_only",
        },
        "recipe.json": {"schema_version": 2},
        "blind.json": {**metadata, "schema_version": 1, "tasks": [common]},
        "gold.json": {
            **metadata,
            "schema_version": 2,
            "census_audits": {
                language: {
                    "status": "unsupported"
                    if parser_refusal or checker_disagreement
                    else "admitted",
                    "refused_paths": ["broken.go"] if parser_refusal else [],
                    "disagreement_paths": ["disputed.go"] if checker_disagreement else [],
                }
            },
            "tasks": [gold_task],
        },
    }
    for name, payload in payloads.items():
        (capsule / name).write_bytes(holdout_c4._raw(payload))
    identity = {
        "files": {
            name: "sha256:" + hashlib.sha256((capsule / name).read_bytes()).hexdigest()
            for name in payloads
        },
        "producer_source_digests": holdout_c4.corpus_binding._gold_producer_source_digests(),
    }
    (capsule / "identity.json").write_bytes(holdout_c4._raw(identity))
    monkeypatch.setattr(
        holdout_c4.corpus_binding,
        "validate_gold",
        lambda path: holdout_c4._read(path / "identity.json"),
    )
    return release, capsule, checkout


def _resign(capsule: Path, name: str, payload: dict) -> None:
    (capsule / name).write_bytes(holdout_c4._raw(payload))
    identity = holdout_c4._read(capsule / "identity.json")
    identity["files"][name] = "sha256:" + hashlib.sha256((capsule / name).read_bytes()).hexdigest()
    (capsule / "identity.json").write_bytes(holdout_c4._raw(identity))


def test_c4_reads_large_gold_only_under_its_scoped_limit(tmp_path, monkeypatch):
    from tools.benchmark import evidence

    monkeypatch.setattr(evidence, "CONTROL_DOCUMENT_BYTES", 16)
    monkeypatch.setattr(holdout_c4.corpus_binding, "CONTROL_DOCUMENT_BYTES", 16)
    monkeypatch.setattr(holdout_c4.corpus_binding, "GOLD_DOCUMENT_BYTES", 32)
    gold = tmp_path / "gold.json"
    blind = tmp_path / "blind.json"
    payload = b'{"ok":true}' + b" " * 10
    gold.write_bytes(payload)
    blind.write_bytes(payload)
    assert holdout_c4._read(gold) == {"ok": True}
    with pytest.raises(evidence.EvidenceError, match="16-byte limit"):
        holdout_c4._read(blind)


def test_c4_derives_existing_suite_and_blind_pack(tmp_path, monkeypatch):
    release, capsule, checkout = _fixture(tmp_path, monkeypatch)
    suite, pack, report = holdout_c4.derive(release, capsule, checkout, "declaration_name_exact")
    assert report["selected"] == 1
    assert report["case_semantics_equivalent"] is False
    assert report["qualified_default_search_conformance"] is False
    assert report["binding"]["index_universe_attested"] is False
    assert suite["tasks"][0]["file_judgments"] == [
        {
            "path": "main.go",
            "file_sha256": hashlib.sha256((checkout / "main.go").read_bytes()).hexdigest(),
            "grade": 3,
        }
    ]
    assert "answerable" not in pack["tasks"][0]
    assert "gold" not in pack["tasks"][0]
    assert pack["suite_commitment_sha256"] == hashlib.sha256(evaluator.canonical(suite)).hexdigest()


@pytest.mark.parametrize("language", ["go", "rust", "python", "typescript", "javascript"])
def test_c4_uses_existing_language_contract(tmp_path, monkeypatch, language):
    release, capsule, checkout = _fixture(tmp_path, monkeypatch, language)
    suite, pack, report = holdout_c4.derive(release, capsule, checkout, "declaration_name_exact")
    assert source_oracle.NAME_CONTRACTS[report["relevance_contract"]] == (language, "exact")
    assert len(suite["tasks"]) == len(pack["tasks"]) == 1
    assert report["semantic_relation"] == "declaration_target_file_diagnostic_only"


@pytest.mark.parametrize(
    ("intent", "query", "names", "variant"),
    [
        ("declaration_name_exact", "Alpha", ("Alpha",), "exact"),
        ("declaration_name_prefix", "Alph", ("Alpha", "AlphaBeta"), "prefix"),
        ("declaration_name_infix", "lph", ("Alpha", "AlphaBeta"), "infix"),
        ("declaration_name_components", "alpha beta", ("AlphaBeta",), "components"),
        ("declaration_name_osa1", "Alphb", ("Alpha",), "osa1"),
    ],
)
def test_c4_independent_name_variants(tmp_path, monkeypatch, intent, query, names, variant):
    release, capsule, checkout = _fixture(tmp_path, monkeypatch)
    source = (checkout / "main.go").read_bytes()
    gold = holdout_c4._read(capsule / "gold.json")
    blind = holdout_c4._read(capsule / "blind.json")
    for payload in (gold, blind):
        payload["tasks"][0].update(intent=intent, query=query)
        if intent == "declaration_name_components":
            payload["tasks"][0]["case_semantics"] = "casefold"
    label = gold["tasks"][0]["labels"][0]
    gold["tasks"][0]["labels"] = [
        {
            **label,
            "start_byte": source.index(name.encode()),
            "end_byte": source.index(name.encode()) + len(name),
            "local_name": name,
        }
        for name in names
    ]
    _resign(capsule, "gold.json", gold)
    _resign(capsule, "blind.json", blind)
    suite, pack, report = holdout_c4.derive(release, capsule, checkout, intent)
    assert report["selected"] == 1
    assert source_oracle.NAME_CONTRACTS[report["relevance_contract"]] == ("go", variant)
    assert suite["tasks"][0]["query"] == pack["tasks"][0]["query"] == query
    expected_policy = (
        "code_search_components_file"
        if intent == "declaration_name_components"
        else "code_search_file"
    )
    assert report["execution_policy"] == expected_policy
    assert suite["routes"] == (
        ["lexical"]
        if intent == "declaration_name_components"
        else ["lexical", "semble-lexical-file"]
    )
    assert suite["tasks"][0]["evaluation_contract"] == {
        "request_mode": (
            "explicit_symbol_components"
            if intent == "declaration_name_components"
            else "default_file_search"
        ),
        "gold_unit": "distinct_file",
        "result_unit": "distinct_file",
    }
    assert suite["suite_id"].endswith(expected_policy.replace("_", "-"))
    assert query_plan.plan_lexical_request(expected_policy, query) == (
        f'components:"{query}"' if intent == "declaration_name_components" else query
    )


def test_c4_casefold_typo_binds_intended_name_and_request_mode(tmp_path, monkeypatch):
    release, capsule, checkout = _fixture(tmp_path, monkeypatch)
    gold = holdout_c4._read(capsule / "gold.json")
    blind = holdout_c4._read(capsule / "blind.json")
    for payload in (gold, blind):
        payload["tasks"][0].update(
            intent="declaration_name_osa1_casefold",
            query="Alphb",
            case_semantics="casefold",
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
    suite, pack, report = holdout_c4.derive(
        release, capsule, checkout, "declaration_name_osa1_casefold"
    )
    task = suite["tasks"][0]
    assert task["query"] == pack["tasks"][0]["query"] == "Alphb"
    assert task["intended_name"] == "Alpha"
    assert task["source_oracle"]["contract"] == "go_exact_local_name_v3"
    assert task["evaluation_contract"]["request_mode"] == "explicit_osa1_typo"
    assert suite["routes"] == ["lexical"]
    assert report["execution_policy"] == "code_search_typo_file"
    assert (
        query_plan.plan_lexical_request(report["execution_policy"], task["query"]) == "typo:Alphb"
    )


def test_c4_casefold_typo_rejects_false_near_name_metadata(tmp_path, monkeypatch):
    release, capsule, checkout = _fixture(tmp_path, monkeypatch)
    gold = holdout_c4._read(capsule / "gold.json")
    blind = holdout_c4._read(capsule / "blind.json")
    for payload in (gold, blind):
        payload["tasks"][0].update(
            intent="declaration_name_osa1_casefold",
            query="Alphb",
            case_semantics="casefold",
        )
    gold["tasks"][0].update(
        intended_name="Alpha",
        near_declaration_state="complete",
        near_declaration_names=["Alpha", "Alphi"],
        exact_collision_names=[],
    )
    _resign(capsule, "gold.json", gold)
    _resign(capsule, "blind.json", blind)
    with pytest.raises(ValueError, match="ambiguity metadata differs from source oracle"):
        holdout_c4.derive(release, capsule, checkout, "declaration_name_osa1_casefold")


def test_c4_excludes_true_alternative_near_name(tmp_path, monkeypatch):
    release, capsule, checkout = _fixture(tmp_path, monkeypatch, near_alternative=True)
    gold = holdout_c4._read(capsule / "gold.json")
    blind = holdout_c4._read(capsule / "blind.json")
    for payload in (gold, blind):
        payload["tasks"][0].update(
            intent="declaration_name_osa1_casefold",
            query="Alphb",
            case_semantics="casefold",
        )
    gold["tasks"][0].update(
        intended_name="Alpha",
        near_declaration_state="complete",
        near_declaration_names=["Alpha", "Alphi"],
        near_declaration_files=["main.go"],
        exact_collision_names=[],
        exact_collision_files=[],
    )
    _resign(capsule, "gold.json", gold)
    _resign(capsule, "blind.json", blind)
    suite, pack, report = holdout_c4._derive_prepared(
        holdout_c4._prepare(release, capsule, checkout),
        "declaration_name_osa1_casefold",
        allow_empty=True,
    )
    assert suite is pack is None
    assert report["selected"] == 0
    assert report["excluded"] == [{"task_id": "toy.def.001", "reason": "ambiguous_typo_target"}]


def test_c4_partial_typo_excludes_before_parsing_refused_file(tmp_path, monkeypatch):
    release, capsule, checkout = _fixture(tmp_path, monkeypatch, parser_refusal=True)
    gold = holdout_c4._read(capsule / "gold.json")
    blind = holdout_c4._read(capsule / "blind.json")
    for payload in (gold, blind):
        payload["tasks"][0].update(
            intent="declaration_name_osa1_casefold",
            query="Alphb",
            case_semantics="casefold",
        )
    gold["tasks"][0].update(
        intended_name="Alpha",
        near_declaration_state="partial",
        near_declaration_names=["Alpha"],
        near_declaration_files=["main.go"],
        exact_collision_names=[],
        exact_collision_files=[],
    )
    _resign(capsule, "gold.json", gold)
    _resign(capsule, "blind.json", blind)
    prepared = holdout_c4._prepare(release, capsule, checkout)
    suite, pack, report = holdout_c4._derive_prepared(
        prepared, "declaration_name_osa1_casefold", allow_empty=True
    )
    assert suite is None and pack is None
    assert report["excluded"] == [
        {"task_id": "toy.def.001", "reason": "incomplete_near_declaration_census"}
    ]


def test_c4_admits_typo_with_query_proven_refused_file(tmp_path, monkeypatch):
    release, capsule, checkout = _fixture(tmp_path, monkeypatch, parser_refusal=True)
    gold = holdout_c4._read(capsule / "gold.json")
    blind = holdout_c4._read(capsule / "blind.json")
    for payload in (gold, blind):
        payload["tasks"][0].update(
            intent="declaration_name_osa1_casefold",
            query="Alphb",
            case_semantics="casefold",
        )
    gold["tasks"][0].update(
        intended_name="Alpha",
        near_declaration_state="complete",
        near_census_text_excluded=[{"path": "broken.go", "reason": "census_refused"}],
        near_declaration_names=["Alpha"],
        near_declaration_files=["main.go"],
        exact_collision_names=[],
        exact_collision_files=[],
    )
    _resign(capsule, "gold.json", gold)
    _resign(capsule, "blind.json", blind)
    suite, _pack, report = holdout_c4.derive(
        release, capsule, checkout, "declaration_name_osa1_casefold"
    )
    assert report["selected"] == 1
    assert suite["tasks"][0]["intended_name"] == "Alpha"
    assert suite["tasks"][0]["query"] == "Alphb"


def test_c4_reuses_one_intended_name_exclusion_for_distinct_typos(tmp_path, monkeypatch):
    release, capsule, checkout = _fixture(tmp_path, monkeypatch, parser_refusal=True)
    gold = holdout_c4._read(capsule / "gold.json")
    blind = holdout_c4._read(capsule / "blind.json")
    for payload in (gold, blind):
        task = payload["tasks"][0]
        task.update(
            intent="declaration_name_osa1_casefold",
            query="Alphb",
            case_semantics="casefold",
        )
        second = {**task, "task_id": "toy.osa.002", "query": "Alphx"}
        payload["tasks"].append(second)
        payload["tasks"].append({**task, "task_id": "toy.osa.003", "query": "Alhpa"})
    gold["tasks"][0].update(
        intended_name="Alpha",
        near_declaration_state="complete",
        near_census_text_excluded=[{"path": "broken.go", "reason": "census_refused"}],
        near_declaration_names=["Alpha"],
        near_declaration_files=["main.go"],
        exact_collision_names=[],
        exact_collision_files=[],
    )
    gold["tasks"][1].update(gold["tasks"][0])
    gold["tasks"][1]["task_id"] = "toy.osa.002"
    gold["tasks"][1]["query"] = "Alphx"
    gold["tasks"][2].update(gold["tasks"][0])
    gold["tasks"][2]["task_id"] = "toy.osa.003"
    gold["tasks"][2]["query"] = "Alhpa"
    _resign(capsule, "gold.json", gold)
    _resign(capsule, "blind.json", blind)

    original = holdout_c4.evaluator.check_query_near_duplicates
    checked_sizes = []

    def checked(rows):
        checked_sizes.append(len(rows))
        return original(rows)

    monkeypatch.setattr(holdout_c4.evaluator, "check_query_near_duplicates", checked)
    suite, pack, report = holdout_c4._derive_prepared(
        holdout_c4._prepare(release, capsule, checkout),
        "declaration_name_osa1_casefold",
        filter_query_duplicates=True,
    )
    assert report["selected"] == 3
    assert [task["query"] for task in pack["tasks"]] == ["Alphb", "Alphx", "Alhpa"]
    assert [task["intended_name"] for task in suite["tasks"]] == ["Alpha"] * 3
    # C4 checks only the new pairs; final suite validation checks all tasks once.
    assert checked_sizes == [2, 2, 2, 3]


@pytest.mark.parametrize("checker_reason", ["census_disagreement", "census_refused"])
def test_c4_admits_typo_with_query_proven_checker_disagreement(
    tmp_path, monkeypatch, checker_reason
):
    release, capsule, checkout = _fixture(tmp_path, monkeypatch, checker_disagreement=True)
    gold = holdout_c4._read(capsule / "gold.json")
    blind = holdout_c4._read(capsule / "blind.json")
    if checker_reason == "census_refused":
        audit = gold["census_audits"]["go"]
        audit["refused_paths"] = ["disputed.go"]
        audit["disagreement_paths"] = []
    for payload in (gold, blind):
        payload["tasks"][0].update(
            intent="declaration_name_osa1_casefold",
            query="Alphb",
            case_semantics="casefold",
        )
    gold["tasks"][0].update(
        intended_name="Alpha",
        near_declaration_state="complete",
        near_census_text_excluded=[{"path": "disputed.go", "reason": checker_reason}],
        near_declaration_names=["Alpha"],
        near_declaration_files=["main.go"],
        exact_collision_names=[],
        exact_collision_files=[],
    )
    gold["tasks"][0]["census_text_excluded"] = [{"path": "disputed.go", "reason": checker_reason}]
    _resign(capsule, "gold.json", gold)
    _resign(capsule, "blind.json", blind)
    suite, _pack, report = holdout_c4.derive(
        release, capsule, checkout, "declaration_name_osa1_casefold"
    )
    assert report["selected"] == 1
    assert "near_declaration_exclusions" not in suite["tasks"][0]["source_oracle"]


def test_c4_excludes_negative_with_default_content_or_path_match(tmp_path, monkeypatch):
    release, capsule, checkout = _fixture(tmp_path, monkeypatch)
    gold = holdout_c4._read(capsule / "gold.json")
    blind = holdout_c4._read(capsule / "blind.json")
    for query, task_id in (("package", "toy.negative.content"), ("main", "toy.negative.path")):
        candidate = dict(gold["tasks"][0])
        candidate.update(task_id=task_id, query=query, answerable=False, labels=[])
        gold["tasks"].append(candidate)
        public = dict(blind["tasks"][0])
        public.update(task_id=task_id, query=query)
        blind["tasks"].append(public)
    absent = dict(gold["tasks"][0])
    absent.update(task_id="toy.negative.absent", query="Zeta", answerable=False, labels=[])
    gold["tasks"].append(absent)
    public = dict(blind["tasks"][0])
    public.update(task_id="toy.negative.absent", query="Zeta")
    blind["tasks"].append(public)
    _resign(capsule, "gold.json", gold)
    _resign(capsule, "blind.json", blind)
    suite, _pack, report = holdout_c4.derive(release, capsule, checkout, "declaration_name_exact")
    assert [task["task_id"] for task in suite["tasks"]] == [
        "toy.def.001",
        "toy.negative.absent",
    ]
    assert suite["tasks"][1]["source_oracle"] == {
        "contract": source_oracle.ASCII_CODE_SEARCH_ABSENT_CASEFOLD,
        "unit": "distinct_file",
    }
    assert suite["tasks"][1]["file_judgments"] == []
    assert suite["tasks"][1]["gold"] == []
    assert report["excluded"] == [
        {"task_id": "toy.negative.content", "reason": "negative_not_default_search_absent"},
        {"task_id": "toy.negative.path", "reason": "negative_not_default_search_absent"},
    ]

    for query, reason in (("package", "content absent"), ("main", "path match")):
        tampered = {**suite, "tasks": [dict(task) for task in suite["tasks"]]}
        negative = tampered["tasks"][1]
        negative["query"] = query
        negative["query_sha256"] = evaluator.digest(query.encode())
        with pytest.raises(evaluator.EvidenceError, match=reason):
            evaluator.validate_suite(checkout, tampered)


def test_c4_refuses_unattested_census_and_capsule(tmp_path, monkeypatch):
    release, capsule, checkout = _fixture(tmp_path, monkeypatch)
    gold = holdout_c4._read(capsule / "gold.json")
    gold["census_audits"]["go"]["status"] = "unsupported"
    _resign(capsule, "gold.json", gold)
    with pytest.raises(ValueError, match="independent census is not admitted"):
        holdout_c4.derive(release, capsule, checkout, "declaration_name_exact")
    monkeypatch.setattr(
        holdout_c4.corpus_binding,
        "validate_gold",
        lambda _path: (_ for _ in ()).throw(ValueError("source-derived capsule mismatch")),
    )
    with pytest.raises(ValueError, match="source-derived capsule mismatch"):
        holdout_c4.derive(release, capsule, checkout, "declaration_name_exact")


def test_c4_admits_query_specific_parser_refusal(tmp_path, monkeypatch):
    release, capsule, checkout = _fixture(tmp_path, monkeypatch, parser_refusal=True)
    suite, _pack, report = holdout_c4.derive(release, capsule, checkout, "declaration_name_exact")
    assert report["selected"] == 1
    assert report["census_excluded_source_paths"] == [
        {"path": "broken.go", "reason": "census_refused"}
    ]
    assert report["selected_tasks_with_proved_exclusions"] == 1
    assert suite["tasks"][0]["source_oracle"]["declaration_exclusions"] == ["broken.go"]


def test_c4_admits_query_specific_checker_disagreement(tmp_path, monkeypatch):
    release, capsule, checkout = _fixture(tmp_path, monkeypatch, checker_disagreement=True)
    suite, _pack, report = holdout_c4.derive(release, capsule, checkout, "declaration_name_exact")
    assert report["selected"] == 1
    assert report["census_excluded_source_paths"] == [
        {"path": "disputed.go", "reason": "census_disagreement"}
    ]
    assert "declaration_exclusions" not in suite["tasks"][0]["source_oracle"]

    gold = holdout_c4._read(capsule / "gold.json")
    gold["tasks"][0]["census_text_excluded"] = []
    _resign(capsule, "gold.json", gold)
    with pytest.raises(ValueError, match="no admitted declaration task"):
        holdout_c4.derive(release, capsule, checkout, "declaration_name_exact")


def test_c4_checker_refusal_with_complete_primary_census(tmp_path, monkeypatch):
    release, capsule, checkout = _fixture(tmp_path, monkeypatch, checker_disagreement=True)
    gold = holdout_c4._read(capsule / "gold.json")
    audit = gold["census_audits"]["go"]
    audit["refused_paths"] = ["disputed.go"]
    audit["disagreement_paths"] = []
    gold["tasks"][0]["census_text_excluded"] = [{"path": "disputed.go", "reason": "census_refused"}]
    _resign(capsule, "gold.json", gold)

    suite, _pack, report = holdout_c4.derive(release, capsule, checkout, "declaration_name_exact")
    assert report["selected"] == 1
    assert report["census_excluded_source_paths"] == [
        {"path": "disputed.go", "reason": "census_refused"}
    ]
    assert "declaration_exclusions" not in suite["tasks"][0]["source_oracle"]


def test_c4_refuses_disputed_file_with_possible_answer(tmp_path, monkeypatch):
    release, capsule, checkout = _fixture(
        tmp_path, monkeypatch, checker_disagreement=True, disputed_name="Alpha"
    )
    with pytest.raises((ValueError, evaluator.EvidenceError), match="query match|labels differ"):
        holdout_c4.derive(release, capsule, checkout, "declaration_name_exact")


@pytest.mark.parametrize("kind", ["missing", "disagreement", "forged_parser_refusal"])
def test_c4_refuses_unproved_partial_census(tmp_path, monkeypatch, kind):
    release, capsule, checkout = _fixture(tmp_path, monkeypatch, parser_refusal=True)
    gold = holdout_c4._read(capsule / "gold.json")
    if kind == "missing":
        gold["tasks"][0]["census_text_excluded"] = []
    elif kind == "disagreement":
        gold["census_audits"]["go"]["disagreement_paths"] = ["broken.go"]
    else:
        gold["tasks"][0]["census_text_excluded"] = [{"path": "main.go", "reason": "census_refused"}]
        gold["census_audits"]["go"]["refused_paths"] = ["main.go"]
    _resign(capsule, "gold.json", gold)
    if kind == "disagreement":
        with pytest.raises(ValueError, match="not admitted or query-provable"):
            holdout_c4.derive(release, capsule, checkout, "declaration_name_exact")
    elif kind == "missing":
        with pytest.raises(ValueError, match="no admitted declaration task"):
            holdout_c4.derive(release, capsule, checkout, "declaration_name_exact")
    else:
        with pytest.raises(
            (ValueError, evaluator.EvidenceError), match="excluded|refuse|parse error"
        ):
            holdout_c4.derive(release, capsule, checkout, "declaration_name_exact")


def test_c4_refuses_release_document_race(tmp_path, monkeypatch):
    release, capsule, checkout = _fixture(tmp_path, monkeypatch)

    def mutate_release(path: Path) -> dict:
        (release / "release.json").write_bytes(b"{}\n")
        return holdout_c4._read(path / "identity.json")

    monkeypatch.setattr(holdout_c4.corpus_binding, "validate_gold", mutate_release)
    with pytest.raises(ValueError, match="release document changed during validation"):
        holdout_c4.derive(release, capsule, checkout, "declaration_name_exact")


@pytest.mark.parametrize("kind", ["identity", "blind", "blind_label", "label", "source"])
def test_c4_refuses_tampering(tmp_path, monkeypatch, kind):
    release, capsule, checkout = _fixture(tmp_path, monkeypatch)
    if kind == "identity":
        (capsule / "blind.json").write_bytes((capsule / "blind.json").read_bytes() + b" ")
    elif kind in {"blind", "blind_label", "label"}:
        name = "blind.json" if kind in {"blind", "blind_label"} else "gold.json"
        payload = holdout_c4._read(capsule / name)
        if kind == "blind":
            payload["tasks"][0]["query"] = "Beta"
        elif kind == "blind_label":
            payload["tasks"][0]["answerable"] = True
        else:
            payload["tasks"][0]["labels"][0]["start_byte"] += 1
        _resign(capsule, name, payload)
    else:
        (checkout / "main.go").write_text("package demo\nfunc Beta() {}\n")
    with pytest.raises((ValueError, evaluator.EvidenceError)):
        holdout_c4.derive(release, capsule, checkout, "declaration_name_exact")


def test_c4_keeps_other_intent_out_of_one_lane(tmp_path, monkeypatch):
    release, capsule, checkout = _fixture(tmp_path, monkeypatch)
    gold = holdout_c4._read(capsule / "gold.json")
    blind = holdout_c4._read(capsule / "blind.json")
    extra_gold = dict(gold["tasks"][0])
    extra_gold.update(task_id="toy.prefix.001", intent="declaration_name_prefix", query="Alph")
    extra_blind = dict(blind["tasks"][0])
    extra_blind.update(task_id="toy.prefix.001", intent="declaration_name_prefix", query="Alph")
    gold["tasks"].append(extra_gold)
    blind["tasks"].append(extra_blind)
    _resign(capsule, "gold.json", gold)
    _resign(capsule, "blind.json", blind)
    suite, _pack, report = holdout_c4.derive(release, capsule, checkout, "declaration_name_exact")
    assert len(suite["tasks"]) == 1
    assert report["excluded"] == [
        {"task_id": "toy.prefix.001", "reason": "outside_selected_intent"}
    ]


def test_c4_refuses_unsupported_selected_intent(tmp_path, monkeypatch):
    release, capsule, checkout = _fixture(tmp_path, monkeypatch)
    with pytest.raises(ValueError, match="supported declaration-name intent"):
        holdout_c4.derive(release, capsule, checkout, "literal_utf8_exact")


def test_c4_never_overwrites_existing_output(tmp_path, monkeypatch):
    release, capsule, checkout = _fixture(tmp_path, monkeypatch)
    output = tmp_path / "out"
    output.mkdir()
    with pytest.raises(ValueError, match="fresh and absolute"):
        holdout_c4.write(release, capsule, checkout, "declaration_name_exact", output)


def test_c4_requires_external_output(tmp_path, monkeypatch):
    release, capsule, checkout = _fixture(tmp_path, monkeypatch)
    with pytest.raises(ValueError, match="fresh and absolute"):
        holdout_c4.write(release, capsule, checkout, "declaration_name_exact", Path("relative"))
    with pytest.raises(ValueError, match="external and disjoint"):
        holdout_c4.write(release, capsule, checkout, "declaration_name_exact", checkout / "results")


def test_c4_removes_partial_root_after_write_failure(tmp_path, monkeypatch):
    release, capsule, checkout = _fixture(tmp_path, monkeypatch)
    output = tmp_path / "out"
    real_write = Path.write_bytes

    def fail_second_file(path: Path, data: bytes) -> int:
        if path == output / "blind-pack.json":
            raise OSError("injected write failure")
        return real_write(path, data)

    monkeypatch.setattr(Path, "write_bytes", fail_second_file)
    with pytest.raises(OSError, match="injected write failure"):
        holdout_c4.write(release, capsule, checkout, "declaration_name_exact", output)
    assert not output.exists()


def _matrix_fixture(tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
    release = tmp_path / "matrix-release"
    capsules = tmp_path / "matrix-capsules"
    checkouts = tmp_path / "matrix-checkouts"
    capsules.mkdir()
    checkouts.mkdir()
    rows = []
    for index, name in enumerate(("repo_a", "repo_b")):
        fixture_root = tmp_path / f"fixture-{index}"
        fixture_root.mkdir()
        original_release, original_capsule, checkout = _fixture(fixture_root, monkeypatch)
        shutil.copytree(checkout, checkouts / name)
        shutil.copytree(original_capsule, capsules / name)
        original_row = holdout_c4._read(original_release / "release.json")["repositories"][0]
        row = dict(original_row)
        row["recipe"] = {**original_row["recipe"], "name": name}
        row["views"] = {
            "code_only": {
                **original_row["views"]["code_only"],
                "manifest": f"manifests/{name}/code_only.json",
            }
        }
        rows.append(row)
        manifest = release / "manifests" / name / "code_only.json"
        manifest.parent.mkdir(parents=True)
        manifest.write_bytes(
            (original_release / "manifests" / "toy" / "code_only.json").read_bytes()
        )
        selection = holdout_c4._read(capsules / name / "selection.json")
        selection.update(repository=name, release_path=str(release))
        _resign(capsules / name, "selection.json", selection)
        for file_name in ("gold.json", "blind.json"):
            payload = holdout_c4._read(capsules / name / file_name)
            payload["repository"] = name
            payload["tasks"][0]["task_id"] = f"{name}.exact.001"
            if index == 1 and file_name == "gold.json":
                payload["tasks"][0].update(
                    answerable=None,
                    label_state="unjudged",
                    labels=[],
                    unsupported=["census_refused"],
                )
            _resign(capsules / name, file_name, payload)
    (release / "release.json").write_bytes(
        holdout_c4._raw({"digest": "sha256:" + "a" * 64, "repositories": rows})
    )
    split_raw = holdout_c4._raw({"fixture": "split"})
    releases_raw = holdout_c4._raw({"sha256:" + "a" * 64: str(release)})
    for name in ("repo_a", "repo_b"):
        for file_name, raw in (
            ("split-manifest.json", split_raw),
            ("split-releases.json", releases_raw),
        ):
            (capsules / name / file_name).write_bytes(raw)
            identity = holdout_c4._read(capsules / name / "identity.json")
            identity["files"][file_name] = "sha256:" + hashlib.sha256(raw).hexdigest()
            (capsules / name / "identity.json").write_bytes(holdout_c4._raw(identity))
    manifest = {"fixture": "validated"}

    def validated(_raw, _releases):
        document = holdout_c4._read(release / "release.json")
        return manifest, {document["digest"]: document}

    def gold_material(_release, selection, _recipe, _split, **_kwargs):
        capsule = capsules / selection["repository"]
        return {path.name: path.read_bytes() for path in capsule.iterdir()}

    monkeypatch.setattr(holdout_c4.corpus_binding, "_validated_split_manifest", validated)
    monkeypatch.setattr(holdout_c4.corpus_binding, "validate_split_manifest", lambda *_: manifest)
    monkeypatch.setattr(holdout_c4.corpus_binding, "_gold_material", gold_material)
    return release, capsules, checkouts


def test_c4_matrix_refuses_stale_gold_source_before_split_replay(tmp_path, monkeypatch):
    release, capsules, checkouts = _matrix_fixture(tmp_path, monkeypatch)
    identity_path = capsules / "repo_b" / "identity.json"
    identity = holdout_c4._read(identity_path)
    identity["producer_source_digests"]["gold_oracle"] = "sha256:" + "0" * 64
    identity_path.write_bytes(holdout_c4._raw(identity))

    def unexpected_replay(*_args, **_kwargs):
        raise AssertionError("split replay must not start for a stale gold producer")

    monkeypatch.setattr(holdout_c4.corpus_binding, "_validated_split_manifest", unexpected_replay)
    with pytest.raises(ValueError, match="C4 gold producer source differs: repo_b"):
        holdout_c4.derive_matrix(release, capsules, checkouts, expected_repositories=2)


def test_c4_matrix_has_independent_cell_inventory_and_validates_once(tmp_path, monkeypatch):
    release, capsules, checkouts = _matrix_fixture(tmp_path, monkeypatch)
    calls = []
    original = holdout_c4.corpus_binding._gold_material

    def derive_one(root, selection, recipe, split, **kwargs):
        calls.append(selection["repository"])
        return original(root, selection, recipe, split, **kwargs)

    monkeypatch.setattr(holdout_c4.corpus_binding, "_gold_material", derive_one)
    matrix = holdout_c4.derive_matrix(release, capsules, checkouts, expected_repositories=2)
    assert calls == ["repo_a", "repo_b"]
    assert [(cell["repository"], cell["intent"]) for cell in matrix["cells"]] == [
        (name, intent)
        for name in ("repo_a", "repo_b")
        for intent in sorted(gold_oracle.DECLARATION_INTENTS)
    ]
    by_key = {(cell["repository"], cell["intent"]): cell for cell in matrix["cells"]}
    admitted = by_key["repo_a", "declaration_name_exact"]
    assert admitted["status"] == "diagnostic_unqualified"
    assert admitted["selected_task_ids"] == ["repo_a.exact.001"]
    assert admitted["suite_sha256"] and admitted["blind_pack_sha256"]
    refused = by_key["repo_b", "declaration_name_exact"]
    assert refused["status"] == "no_admission_diagnostic"
    assert refused["reason"] == "all_tasks_excluded"
    assert refused["excluded"] == [
        {"task_id": "repo_b.exact.001", "reason": "unjudged_or_unsupported"}
    ]
    assert by_key["repo_a", "declaration_name_prefix"]["reason"] == "no_tasks_for_intent"
    assert by_key["repo_a", "declaration_name_osa1_casefold"]["status"] == "no_admission_diagnostic"
    assert matrix["product_capture"] is False
    assert matrix["qualified_default_search_conformance"] is False
    assert matrix["release_document_sha256"] == holdout_c4.digest_bytes(
        (release / "release.json").read_bytes()
    )
    assert matrix["tool_source_sha256"]["holdout_c4"] == holdout_c4.digest_bytes(
        Path(holdout_c4.__file__).read_bytes()
    )
    assert admitted["manifest_sha256"] == holdout_c4.digest_bytes(
        (release / "manifests" / "repo_a" / "code_only.json").read_bytes()
    )
    output = tmp_path / "external-matrix"
    holdout_c4.write_matrix(release, capsules, checkouts, output, expected_repositories=2)
    assert {path.name for path in output.iterdir()} == {"admission-matrix.json"}
    bundle = tmp_path / "external-bundle"
    bound = holdout_c4.write_matrix(
        release, capsules, checkouts, bundle, expected_repositories=2, emit_suites=True
    )
    cell = bundle / "repo_a" / "declaration_name_exact"
    assert {path.name for path in cell.iterdir()} == {
        "suite.json",
        "blind-pack.json",
        "admission.json",
    }
    selected = next(
        row
        for row in bound["cells"]
        if row["repository"] == "repo_a" and row["intent"] == "declaration_name_exact"
    )
    assert (
        hashlib.sha256((cell / "suite.json").read_bytes()).hexdigest() == selected["suite_sha256"]
    )
    assert (
        hashlib.sha256((cell / "blind-pack.json").read_bytes()).hexdigest()
        == selected["blind_pack_sha256"]
    )
    assert (
        hashlib.sha256((cell / "admission.json").read_bytes()).hexdigest()
        == selected["admission_sha256"]
    )
    assert not (bundle / "repo_b").exists()


def test_c4_matrix_refuses_missing_or_changed_inputs(tmp_path, monkeypatch):
    release, capsules, checkouts = _matrix_fixture(tmp_path, monkeypatch)
    with pytest.raises(ValueError, match="roster differs from expected count"):
        holdout_c4.derive_matrix(release, capsules, checkouts)
    shutil.rmtree(capsules / "repo_b")
    with pytest.raises(ValueError, match="capsule roster differs"):
        holdout_c4.derive_matrix(release, capsules, checkouts, expected_repositories=2)


def test_c4_matrix_refuses_source_change_during_admission(tmp_path, monkeypatch):
    release, capsules, checkouts = _matrix_fixture(tmp_path, monkeypatch)
    original = holdout_c4._derive_prepared
    changed = False

    def mutate(prepared, intent, *, allow_empty=False, filter_query_duplicates=False):
        nonlocal changed
        result = original(
            prepared,
            intent,
            allow_empty=allow_empty,
            filter_query_duplicates=filter_query_duplicates,
        )
        if not changed:
            (checkouts / "repo_a" / "main.go").write_text("package demo\nfunc Beta() {}\n")
            changed = True
        return result

    monkeypatch.setattr(holdout_c4, "_derive_prepared", mutate)
    with pytest.raises((ValueError, evaluator.EvidenceError)):
        holdout_c4.derive_matrix(release, capsules, checkouts, expected_repositories=2)


@pytest.mark.parametrize("file_name", ["split-manifest.json", "split-releases.json"])
def test_c4_matrix_rechecks_split_control_bytes(tmp_path, monkeypatch, file_name):
    release, capsules, checkouts = _matrix_fixture(tmp_path, monkeypatch)
    original = holdout_c4._derive_prepared
    changed = False

    def mutate(prepared, intent, *, allow_empty=False, filter_query_duplicates=False):
        nonlocal changed
        result = original(
            prepared,
            intent,
            allow_empty=allow_empty,
            filter_query_duplicates=filter_query_duplicates,
        )
        if not changed:
            control = capsules / "repo_a" / file_name
            control.write_bytes(control.read_bytes() + b" ")
            changed = True
        return result

    monkeypatch.setattr(holdout_c4, "_derive_prepared", mutate)
    with pytest.raises(ValueError, match="input changed during admission"):
        holdout_c4.derive_matrix(release, capsules, checkouts, expected_repositories=2)


def test_c4_matrix_records_unsupported_language_cells(tmp_path, monkeypatch):
    release, capsules, checkouts = _matrix_fixture(tmp_path, monkeypatch)
    document = holdout_c4._read(release / "release.json")
    document["repositories"][1]["recipe"]["language"] = "unsupported-language"
    (release / "release.json").write_bytes(holdout_c4._raw(document))
    matrix = holdout_c4.derive_matrix(release, capsules, checkouts, expected_repositories=2)
    rows = [cell for cell in matrix["cells"] if cell["repository"] == "repo_b"]
    assert len(rows) == len(gold_oracle.DECLARATION_INTENTS)
    assert {cell["reason"] for cell in rows} == {"unsupported_language_intent"}
    assert all(cell["status"] == "no_admission_diagnostic" for cell in rows)
    (capsules / "repo_b" / "blind.json").write_bytes(b"{}\n")
    with pytest.raises(ValueError, match="capsule identity differs"):
        holdout_c4.derive_matrix(release, capsules, checkouts, expected_repositories=2)


@pytest.mark.parametrize("owner", ["holdout_c4", "corpus_release"])
def test_c4_matrix_refuses_tool_source_drift(tmp_path, monkeypatch, owner):
    release, capsules, checkouts = _matrix_fixture(tmp_path, monkeypatch)
    original = Path.read_bytes
    tool = holdout_c4._batch_tool_sources()[owner]
    reads = 0

    def changed(path):
        nonlocal reads
        if path == tool:
            reads += 1
            if reads == 2:
                return b"changed tool source"
        return original(path)

    monkeypatch.setattr(Path, "read_bytes", changed)
    with pytest.raises(ValueError, match="tool source changed"):
        holdout_c4.derive_matrix(release, capsules, checkouts, expected_repositories=2)


def test_c4_matrix_optimized_replay_matches_canonical_validator(
    split_releases,  # noqa: F811
    tmp_path,
    monkeypatch,
):
    from tools.benchmark.retrieval import holdout_sampling

    release, document = split_releases["hold_only"]
    other_release, other_document = split_releases["dev_only"]
    _ledger, recipes, split_raw, _manifest = holdout_sampling.build(release, other_release, 11)
    releases = {document["digest"]: release, other_document["digest"]: other_release}
    selection = {
        "release_path": str(release),
        "release_digest": document["digest"],
        "repository": "beta",
        "view": "code_only",
    }
    capsules = tmp_path / "capsules"
    capsules.mkdir()
    capsule = capsules / "beta"
    holdout_c4.corpus_binding.capture_gold(
        release,
        selection,
        holdout_c4.corpus_binding.canonical_json(recipes["beta"]).encode() + b"\n",
        capsule,
        (split_raw, releases),
    )
    expected_identity = holdout_c4.corpus_binding.validate_gold(capsule)
    validated_manifest, documents = holdout_c4.corpus_binding._validated_split_manifest(
        split_raw, releases
    )
    checkout_root = release.parents[1] / "checkouts"
    prepared = holdout_c4._prepare(
        release,
        capsule,
        checkout_root / "beta",
        verified_split=(split_raw, releases, validated_manifest, documents),
    )
    assert prepared.identity == expected_identity
    assert prepared.gold == holdout_c4._read(capsule / "gold.json")
    original = holdout_c4.corpus_binding._validated_split_manifest
    calls = []

    def counted(raw, paths):
        calls.append(True)
        return original(raw, paths)

    monkeypatch.setattr(holdout_c4.corpus_binding, "_validated_split_manifest", counted)
    matrix = holdout_c4.derive_matrix(release, capsules, checkout_root, expected_repositories=1)
    assert len(calls) == 2  # Initial validation and final drift check.
    assert len(matrix["cells"]) == len(gold_oracle.DECLARATION_INTENTS)
    assert {cell["repository"] for cell in matrix["cells"]} == {"beta"}
    exact = next(cell for cell in matrix["cells"] if cell["intent"] == "declaration_name_exact")
    assert {row["reason"] for row in exact["excluded"]} >= {"query_near_duplicate"}


@pytest.mark.parametrize("name", ["holdout_c4.py", "holdout_literal.py"])
def test_holdout_cli_help_runs_from_external_cwd_without_pythonpath(tmp_path, name):
    repository = Path(__file__).resolve().parents[3]
    command = repository / "tools" / "benchmark" / "retrieval" / name
    environment = os.environ.copy()
    environment.pop("PYTHONPATH", None)
    result = subprocess.run(
        [sys.executable, str(command), "--help"],
        cwd=tmp_path,
        env=environment,
        capture_output=True,
        text=True,
        timeout=30,
        check=False,
    )
    assert result.returncode == 0, result.stderr
    assert "--capsules" in result.stdout
