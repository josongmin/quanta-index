"""Fixed independent expectations for exact-content holdout admission."""

from __future__ import annotations

import copy
import hashlib
import json
import shutil
from pathlib import Path

import jsonschema
import pytest

from tools.benchmark.retrieval import evaluator, holdout_c4, holdout_literal, literal_source_oracle
from tools.ci.tests.test_holdout_c4 import _fixture, _resign
from tools.ci.tests.test_source_oracle_suite import _source_repo


def test_literal_oracle_counts_overlaps_case_utf8_and_ignores_paths():
    files = {
        "Needle.go": (b"nothing\n", "a" * 64),
        "a.go": (b"banana ABA aba \xc3\xa9\n", "b" * 64),
    }
    oracle = literal_source_oracle.LiteralSourceOracleIndex(files, {"ana", "aba", "é", "Needle"})
    assert oracle.spans("ana") == [("a.go", 1, 4), ("a.go", 3, 6)]
    assert oracle.spans("aba") == [("a.go", 11, 14)]
    assert oracle.spans("é") == [("a.go", 15, 17)]
    assert oracle.spans("Needle") == []
    assert oracle.expected_rows(
        literal_source_oracle.CONTENT_LITERAL_UTF8_EXACT, "ana", "distinct_file"
    ) == [{"path": "a.go", "file_sha256": "b" * 64, "grade": 3}]
    assert oracle.first_match(literal_source_oracle.CONTENT_LITERAL_UTF8_EXACT, "ana") == (
        "a.go",
        1,
        4,
    )


@pytest.mark.parametrize("query", ["", "e\u0301", "a\n", "a\tb", "a\x00", "x" * 257, "\ud800"])
def test_literal_oracle_rejects_non_product_query_forms(query):
    with pytest.raises(literal_source_oracle.LiteralOracleError):
        literal_source_oracle.LiteralSourceOracleIndex({"a": (b"body", "a" * 64)}, {query})


def test_literal_oracle_rejects_normalization_created_file_membership():
    oracle = literal_source_oracle.LiteralSourceOracleIndex(
        {"decomposed.go": (b"e\xcc\x81", "a" * 64)}, {"é"}
    )
    assert oracle.spans("é") == []
    assert oracle.indexed_nfc_membership_matches("é") is False


def test_literal_oracle_refuses_unknown_invalid_utf8_index_semantics():
    oracle = literal_source_oracle.LiteralSourceOracleIndex(
        {"bad.go": (b"needle\xff", "a" * 64)}, {"needle"}
    )
    assert oracle.spans("needle") == [("bad.go", 0, 6)]
    assert oracle.indexed_nfc_membership_matches("needle") is False


def _suite(tmp_path: Path):
    repo, commit, raw_files = _source_repo(tmp_path)
    source = evaluator.SourceSnapshot(repo, commit)
    query = "type Param"
    oracle = literal_source_oracle.LiteralSourceOracleIndex(
        {path: (raw, evaluator.digest(raw)) for path, raw in raw_files.items()}, {query}
    )
    task = {
        "task_id": "exact.001",
        "query": query,
        "query_sha256": evaluator.digest(query.encode()),
        "query_family_id": "exact.001",
        "split": "eval",
        "category": "literal_utf8_exact",
        "query_intent": "exact_content",
        "source_oracle": {
            "contract": literal_source_oracle.CONTENT_LITERAL_UTF8_EXACT,
            "unit": "distinct_file",
        },
        "judgment_policy": evaluator.SOURCE_ORACLE_JUDGMENT_POLICY,
        "file_judgments": oracle.expected_rows(
            literal_source_oracle.CONTENT_LITERAL_UTF8_EXACT, query, "distinct_file"
        ),
        "gold": evaluator.source_oracle_gold(
            source, oracle, literal_source_oracle.CONTENT_LITERAL_UTF8_EXACT, query
        ),
        "answerable": True,
    }
    universe = [
        {"path": path, "file_sha256": evaluator.digest(raw)}
        for path, raw in sorted(raw_files.items())
    ]
    suite = {
        "schema_version": evaluator.SCHEMA_VERSION,
        "suite_id": "exact-content-source-fixture",
        "repository_commit": commit,
        "comparison_contract": {
            "top_k": 10,
            "tokenizer": evaluator.TOKENIZER,
            "tokenizer_budget_version": evaluator.TOKENIZER_BUDGET_VERSION,
            "output_unit_policy": "rank_prefix",
            "span_unit": evaluator.SPAN_UNIT,
        },
        "routes": ["lexical"],
        "file_universe": universe,
        "file_universe_digest": evaluator.universe_digest(universe),
        "diagnostic_policy": evaluator.OBSERVED_PREFIX_DIAGNOSTIC_POLICY,
        "tasks": [task],
    }
    return repo, suite


def test_literal_suite_replays_independent_file_gold_and_blinds_labels(tmp_path):
    repo, suite = _suite(tmp_path)
    schema = json.loads(
        (Path(__file__).parents[2] / "benchmark/retrieval/suite.schema.json").read_text()
    )
    jsonschema.validate(suite, schema)
    checked, pack, _ = evaluator.validate_suite(repo, suite)
    assert checked == suite
    assert pack["tasks"] == [
        {
            "task_id": "exact.001",
            "query": "type Param",
            "query_sha256": evaluator.digest(b"type Param"),
        }
    ]


@pytest.mark.parametrize("mutation", ["missing_file", "wrong_span", "wrong_intent", "extra_route"])
def test_literal_suite_rejects_tampered_gold_or_semantics(tmp_path, mutation):
    repo, suite = _suite(tmp_path)
    broken = copy.deepcopy(suite)
    task = broken["tasks"][0]
    if mutation == "missing_file":
        task["file_judgments"] = []
    elif mutation == "wrong_span":
        task["gold"][0]["start_byte"] += 1
    elif mutation == "wrong_intent":
        task["query_intent"] = "bare_symbol"
    else:
        broken["routes"].append("semble-lexical-file")
    with pytest.raises(evaluator.EvidenceError):
        evaluator.validate_suite(repo, broken)


def _literal_fixture(tmp_path, monkeypatch):
    release, capsule, checkout = _fixture(tmp_path, monkeypatch)
    raw = (checkout / "main.go").read_bytes()
    query = "Alpha()"
    gold = holdout_c4._read(capsule / "gold.json")
    blind = holdout_c4._read(capsule / "blind.json")
    for payload in (gold, blind):
        payload["tasks"][0].update(intent="literal_utf8_exact", query=query, language=None)
    gold["tasks"][0]["labels"] = [
        {
            "path": "main.go",
            "file_sha256": hashlib.sha256(raw).hexdigest(),
            "start_byte": raw.index(query.encode()),
            "end_byte": raw.index(query.encode()) + len(query),
            "kind": "literal_occurrence",
            "local_name": None,
        }
    ]
    _resign(capsule, "gold.json", gold)
    _resign(capsule, "blind.json", blind)
    return release, capsule, checkout


def test_literal_adapter_derives_source_bound_suite(tmp_path, monkeypatch):
    release, capsule, checkout = _literal_fixture(tmp_path, monkeypatch)
    suite, pack, report = holdout_literal.derive(release, capsule, checkout)
    assert report["selected"] == 1
    assert report["product_capture"] is False
    assert report["execution_policy"] == "code_search_exact_content_file"
    assert report["binding"]["index_universe_attested"] is False
    assert suite["routes"] == ["lexical"]
    assert suite["tasks"][0]["file_judgments"] == [
        {
            "path": "main.go",
            "file_sha256": hashlib.sha256((checkout / "main.go").read_bytes()).hexdigest(),
            "grade": 3,
        }
    ]
    assert "gold" not in pack["tasks"][0]


def test_literal_adapter_excludes_invalid_query_without_backfilling(tmp_path, monkeypatch):
    release, capsule, checkout = _literal_fixture(tmp_path, monkeypatch)
    gold = holdout_c4._read(capsule / "gold.json")
    blind = holdout_c4._read(capsule / "blind.json")
    query = "Alpha() {}\nfunc AlphaBeta"
    raw = (checkout / "main.go").read_bytes()
    start = raw.index(query.encode())
    invalid = copy.deepcopy(gold["tasks"][0])
    invalid.update(task_id="toy.lit.002", query_family_id="toy.lit.002", query=query)
    invalid["labels"] = [
        {
            "path": "main.go",
            "file_sha256": hashlib.sha256(raw).hexdigest(),
            "start_byte": start,
            "end_byte": start + len(query.encode()),
            "kind": "literal_occurrence",
            "local_name": None,
        }
    ]
    gold["tasks"].append(invalid)
    public = copy.deepcopy(blind["tasks"][0])
    public.update(task_id="toy.lit.002", query_family_id="toy.lit.002", query=query)
    blind["tasks"].append(public)
    _resign(capsule, "gold.json", gold)
    _resign(capsule, "blind.json", blind)

    suite, pack, report = holdout_literal.derive(release, capsule, checkout)
    assert report["selected"] == 1
    assert report["excluded"] == [
        {"task_id": "toy.lit.002", "reason": "outside_literal_query_contract"}
    ]
    assert len(suite["tasks"]) == len(pack["tasks"]) == 1


def test_literal_adapter_rejects_producer_span_drift(tmp_path, monkeypatch):
    release, capsule, checkout = _literal_fixture(tmp_path, monkeypatch)
    gold = holdout_c4._read(capsule / "gold.json")
    gold["tasks"][0]["labels"][0]["start_byte"] += 1
    _resign(capsule, "gold.json", gold)
    with pytest.raises(ValueError, match="independent oracle"):
        holdout_literal.derive(release, capsule, checkout)


@pytest.mark.parametrize(
    "mutation", ["source", "split_manifest", "split_releases", "oracle_source"]
)
def test_literal_batch_rechecks_inputs_after_suite_derivation(tmp_path, monkeypatch, mutation):
    release, capsule, checkout = _literal_fixture(tmp_path, monkeypatch)
    capsule_root = tmp_path / "capsules"
    checkout_root = tmp_path / "checkouts"
    shutil.copytree(capsule, capsule_root / "toy")
    shutil.copytree(checkout, checkout_root / "toy")
    (capsule_root / "toy" / "split-manifest.json").write_text("{}")
    (capsule_root / "toy" / "split-releases.json").write_text("{}")
    document = holdout_c4._read(release / "release.json")
    split = {"test": "validated"}
    monkeypatch.setattr(
        holdout_literal.corpus_binding,
        "_split_releases",
        lambda _payload: {document["digest"]: release},
    )
    monkeypatch.setattr(
        holdout_literal.corpus_binding,
        "_validated_split_manifest",
        lambda _raw, _releases: (split, {document["digest"]: document}),
    )
    monkeypatch.setattr(
        holdout_literal.corpus_binding,
        "validate_split_manifest",
        lambda _raw, _releases: split,
    )
    real_prepare = holdout_c4._prepare
    real_derive = holdout_literal._derive_prepared
    monkeypatch.setattr(
        holdout_c4,
        "_prepare",
        lambda release, capsule, checkout, **_kwargs: real_prepare(release, capsule, checkout),
    )

    def derive_then_mutate(prepared):
        result = real_derive(prepared)
        if mutation == "source":
            (checkout_root / "toy" / "main.go").write_bytes(b"changed after suite derivation\n")
        elif mutation == "split_manifest":
            (capsule_root / "toy" / "split-manifest.json").write_text('{"changed":true}')
        elif mutation == "split_releases":
            (capsule_root / "toy" / "split-releases.json").write_text('{"changed":true}')
        else:
            original = Path.read_bytes
            oracle_path = Path(literal_source_oracle.source_oracle.__file__)

            def changed(path):
                if path == oracle_path:
                    return b"changed source oracle"
                return original(path)

            monkeypatch.setattr(Path, "read_bytes", changed)
        return result

    monkeypatch.setattr(holdout_literal, "_derive_prepared", derive_then_mutate)
    failure = (
        "tracked or untracked changes|file hash"
        if mutation == "source"
        else "tool source changed during admission"
        if mutation == "oracle_source"
        else "split.*changed during admission|input changed during admission"
    )
    with pytest.raises((evaluator.EvidenceError, ValueError), match=failure):
        holdout_literal.derive_batch(release, capsule_root, checkout_root, expected_repositories=1)
