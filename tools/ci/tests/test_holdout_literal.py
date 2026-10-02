"""Focused admission checks for the B08 exact-content diagnostic lane."""

from __future__ import annotations

import hashlib
from types import SimpleNamespace

import pytest

from tools.benchmark.retrieval import holdout_literal, query_plan


def _prepared(query: str, files: dict[str, bytes]):
    needle = query.encode()
    labels = []
    for path, content in files.items():
        start = content.find(needle)
        if start >= 0:
            labels.append(
                {
                    "path": path,
                    "start_byte": start,
                    "end_byte": start + len(needle),
                    "kind": "literal_occurrence",
                    "file_sha256": hashlib.sha256(content).hexdigest(),
                }
            )
    task = {
        "task_id": "repo.lit.001",
        "split": "holdout",
        "query_family_id": "repo.lit.001",
        "intent": "literal_utf8_exact",
        "query": query,
        "scope_prefix": "",
        "language": None,
        "case_semantics": "sensitive",
        "normalization": "none_raw_utf8",
        "labels": labels,
        "unsupported": [],
        "label_state": "mechanical_unreviewed",
        "answerable": bool(labels),
    }
    public = {
        key: value
        for key, value in task.items()
        if key not in {"split", "labels", "unsupported", "label_state", "answerable"}
    }

    class Source:
        def file(self, path: str):
            return files[path], None, hashlib.sha256(files[path]).hexdigest()

    return SimpleNamespace(
        gold={"tasks": [task]},
        blind={"tasks": [public]},
        files=files,
        source=Source(),
        selection={"repository": "repo"},
        manifest={"repository_commit": "a" * 40, "files": []},
        document={"digest": "sha256:" + "b" * 64},
        identity_sha256="c" * 64,
    )


def test_exact_content_request_quotes_scope_case_and_binds_syntax():
    raw = 'fn f() { let s = "can\'t\\skip"; }'
    request = query_plan.plan_code_search_exact_content_request(raw)
    assert request == 'content:"fn f() { let s = \\"can\'t\\\\skip\\"; }" case:yes'
    assert (
        query_plan.code_search_effective_request_sha256(request)
        != hashlib.sha256(request.encode()).hexdigest()
    )
    assert query_plan.derive_query_identity("code_search_file", "token")[
        "effective_lexical_request_sha256"
    ] == query_plan.code_search_effective_request_sha256("token")


@pytest.mark.parametrize("raw", ["", "e\u0301", "line\nbreak", "x" * 257, "\ud800"])
def test_exact_content_request_refuses_unrepresentable_gold(raw):
    with pytest.raises(query_plan.QueryPlanError):
        query_plan.plan_code_search_exact_content_request(raw)


def test_literal_admission_is_quanta_only_and_source_bound():
    query = 'say("can\'t")'
    prepared = _prepared(query, {"src/a.rs": query.encode(), "src/b.rs": b"other"})
    report = holdout_literal._derive_prepared(prepared)
    assert report["status"] == "diagnostic_unqualified"
    assert report["product"] == "quanta"
    assert report["product_capture"] is False
    assert report["qualified_default_search_conformance"] is False
    assert report["runner_policy"] is None
    assert report["selected"] == 1
    assert report["admitted"][0]["gold_file_count"] == 1
    assert report["admitted"][0]["effective_request"] == ('content:"say(\\"can\'t\\")" case:yes')


def test_literal_admission_refuses_nfc_product_gold_mismatch():
    prepared = _prepared("é", {"src/a.rs": "é".encode(), "src/b.rs": "e\u0301".encode()})
    report = holdout_literal._derive_prepared(prepared)
    assert report["selected"] == 0
    assert report["excluded"] == [
        {"task_id": "repo.lit.001", "reason": "normalized_content_differs_from_raw_gold"}
    ]


def test_literal_admission_rejects_tampered_label_or_blind_query():
    prepared = _prepared("needle", {"src/a.rs": b"needle"})
    prepared.gold["tasks"][0]["labels"][0]["file_sha256"] = "0" * 64
    with pytest.raises(ValueError, match="labels differ"):
        holdout_literal._derive_prepared(prepared)
    prepared = _prepared("needle", {"src/a.rs": b"needle"})
    prepared.blind["tasks"][0]["query"] = "wrong"
    with pytest.raises(ValueError, match="blind task differs"):
        holdout_literal._derive_prepared(prepared)
