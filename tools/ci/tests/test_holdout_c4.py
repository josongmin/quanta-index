"""Independent fixed-source admission checks for the holdout C4 adapter."""

from __future__ import annotations

import hashlib
import subprocess
from pathlib import Path

import pytest

from tools.benchmark.retrieval import evaluator, holdout_c4, source_oracle


def _fixture(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    language: str = "go",
    *,
    parser_refusal: bool = False,
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
    (checkout / file_name).write_text(body, encoding="utf-8")
    if parser_refusal:
        assert language == "go"
        (checkout / "broken.go").write_text("package demo\nfunc Broken(", encoding="utf-8")
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
                    "status": "unsupported" if parser_refusal else "admitted",
                    "refused_paths": ["broken.go"] if parser_refusal else [],
                    "disagreement_paths": [],
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
        }
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
    assert report["excluded"] == [
        {"task_id": "toy.negative.content", "reason": "negative_not_default_search_absent"},
        {"task_id": "toy.negative.path", "reason": "negative_not_default_search_absent"},
    ]


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
    assert report["parser_refused_source_paths"] == ["broken.go"]
    assert report["selected_tasks_with_proved_exclusions"] == 1
    assert suite["tasks"][0]["source_oracle"]["declaration_exclusions"] == ["broken.go"]


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
        with pytest.raises(ValueError, match="checker disagreement"):
            holdout_c4.derive(release, capsule, checkout, "declaration_name_exact")
    elif kind == "missing":
        with pytest.raises(ValueError, match="no admitted declaration task"):
            holdout_c4.derive(release, capsule, checkout, "declaration_name_exact")
    else:
        with pytest.raises((ValueError, evaluator.EvidenceError), match="excluded|refuse"):
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
