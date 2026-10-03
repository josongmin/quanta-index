"""Independent golden and refusal cases for the pinned CLARC Group 1 adapter."""

from __future__ import annotations

import hashlib
import json

import pytest

from tools.benchmark.retrieval import clarc_adapter as clarc


def _rows() -> tuple[list[dict], list[dict]]:
    original = [
        {
            "query_id": "q_group_1_id_0",
            "query_text": "Return a true value for an infinity string.",
            "code_id": "c_group_1_id_0",
            "code_text": "bool IsInf(const char *s) { return s[0] == 'i'; }",
            "relevance": 2,
        },
        {
            "query_id": "q_group_1_id_1",
            "query_text": " ".join(f"word{i}" for i in range(33)),
            "code_id": "c_group_1_id_1",
            "code_text": "int Add(int a, int b) { return a + b; }",
            "relevance": 2,
        },
    ]
    neutral = [
        {**original[1], "code_text": "int func_0(int var_0, int var_1) { return var_0 + var_1; }"},
        {**original[0], "code_text": "bool func_0(const char *var_0) { return var_0[0] == 'i'; }"},
    ]
    return original, neutral


def _bytes(rows: list[dict]) -> bytes:
    return json.dumps(rows, ensure_ascii=False).encode("utf-8")


def _inputs(monkeypatch):
    original, neutral = _rows()
    raws = {
        "original": _bytes(original),
        "neutral_renamed": _bytes(neutral),
        "dataset_card": b"---\nlicense: cc-by-sa-4.0\n---\n",
        "project_license_info": b"project_name,license_info,url\nexample,MIT,https://example.test\n",
    }
    monkeypatch.setattr(clarc, "EXPECTED_PAIRS", 2)
    monkeypatch.setattr(
        clarc,
        "SOURCES",
        {
            key: {"path": key, "sha256": hashlib.sha256(raw).hexdigest()}
            for key, raw in raws.items()
        },
    )
    return raws


def test_paired_ids_and_unchanged_queries_create_two_synthetic_file_universes(
    tmp_path, monkeypatch
):
    raws = _inputs(monkeypatch)
    rows, metadata = clarc.admit_pinned_pair(*raws.values())
    assert [pair["original"]["query_id"] for pair in rows] == [
        "q_group_1_id_0",
        "q_group_1_id_1",
    ]
    assert metadata["admission"]["requested"] == 2
    assert metadata["admission"]["coverage"] == 0.5
    assert metadata["admission"]["admitted_query_ids"] == ["q_group_1_id_0"]
    assert [item["query_id"] for item in metadata["admission"]["refused"]] == ["q_group_1_id_1"]
    root = tmp_path / "out"
    manifest = clarc.materialize(*raws.values(), root)
    assert manifest["pairs"] == 2
    assert len(manifest["qrels"]) == 2
    assert manifest["contract"]["qrel_policy"] == (
        "one_upstream_positive_per_query_other_candidates_unjudged"
    )
    assert manifest["contract"]["source_oracle"] == "not_applicable"
    assert manifest["qrels"][1]["query_text"] == _rows()[0][1]["query_text"]
    assert [entry["path"] for entry in manifest["file_universes"]["original"]] == [
        "snippets/c_group_1_id_0.cpp",
        "snippets/c_group_1_id_1.cpp",
    ]
    assert [row["task_id"] for row in manifest["tasks"]["original"]] == [
        "CLARC-G1-ORG-0000",
        "CLARC-G1-ORG-0001",
    ]
    assert [row["task_id"] for row in manifest["tasks"]["neutral_renamed"]] == [
        "CLARC-G1-NEU-0000",
        "CLARC-G1-NEU-0001",
    ]
    assert [row["query"] for row in manifest["tasks"]["original"]] == [
        row["query"] for row in manifest["tasks"]["neutral_renamed"]
    ]
    assert manifest["tasks"]["original"][1]["request_status"] == "refused"
    assert len(manifest["admission"]["ledger"]) == 2
    assert (root / "original/snippets/c_group_1_id_0.cpp").read_text() == _rows()[0][0]["code_text"]
    assert (root / "neutral_renamed/snippets/c_group_1_id_0.cpp").read_text() == _rows()[1][1][
        "code_text"
    ]
    assert json.loads((root / "manifest.json").read_text()) == manifest
    with pytest.raises(FileExistsError):
        clarc.materialize(*raws.values(), root)


@pytest.mark.parametrize(
    "change",
    [
        lambda rows: rows[0].update(code_id="c_group_1_id_0"),
        lambda rows: rows[0].update(query_text="wrong paired query"),
        lambda rows: rows[0].update(relevance=0),
        lambda rows: rows[0].update(query_id="q_group_1_id_0"),
    ],
)
def test_pair_rejects_id_gold_and_query_drift(change):
    original, neutral = _rows()
    change(neutral)
    with pytest.raises(clarc.ClarcAdmissionError):
        clarc._validate_pair(_bytes(original), _bytes(neutral), 2)


def test_pair_rejects_missing_extra_and_noncontiguous_ids():
    original, neutral = _rows()
    with pytest.raises(clarc.ClarcAdmissionError, match="2 rows"):
        clarc._validate_pair(_bytes(original[:1]), _bytes(neutral), 2)
    neutral[1]["unexpected"] = True
    with pytest.raises(clarc.ClarcAdmissionError, match="fields"):
        clarc._validate_pair(_bytes(original), _bytes(neutral), 2)
    original, neutral = _rows()
    for rows in (original, neutral):
        rows[1]["query_id"] = "q_group_1_id_2"
        rows[1]["code_id"] = "c_group_1_id_2"
    with pytest.raises(clarc.ClarcAdmissionError, match="contiguous"):
        clarc._validate_pair(_bytes(original), _bytes(neutral), 2)


def test_pinned_digest_and_license_evidence_are_required(tmp_path, monkeypatch):
    raws = _inputs(monkeypatch)
    changed = {**raws, "original": raws["original"] + b"\n"}
    with pytest.raises(clarc.ClarcAdmissionError, match="SHA-256"):
        clarc.materialize(*changed.values(), tmp_path / "wrong-source")
    assert not (tmp_path / "wrong-source").exists()
    changed = {**raws, "dataset_card": b"---\nlicense: unknown\n---\n"}
    with pytest.raises(clarc.ClarcAdmissionError, match="SHA-256"):
        clarc.admit_pinned_pair(*changed.values())
    assert (
        "redistribution_unverified"
        in clarc.admit_pinned_pair(*raws.values())[1]["contract"]["license_status"]
    )
