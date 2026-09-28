"""Fixed source examples are the oracle's independent expectations."""

from __future__ import annotations

import json
import shutil
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[3] / "tools/benchmark"))
import corpus_binding as binding
from evidence import EvidenceError, canonical_json

from tools.benchmark.retrieval import gold_oracle
from tools.ci.tests.test_corpus_release import release_seed, source_seed  # noqa: F401

FIXTURES = Path(__file__).parent / "fixtures" / "gold_oracle"


@pytest.mark.parametrize(
    "name,language,expected",
    [
        ("python.py", "python", [31, 75]),
        ("rust.rs", "rust", [27, 48, 92]),
        ("typescript.ts", "typescript", [24, 48, 67, 92]),
    ],
)
def test_named_function_oracle_has_fixed_declaration_spans(name, language, expected):
    raw = (FIXTURES / name).read_bytes()
    spans, error = gold_oracle._definition_spans(raw, b"target", language)
    assert error is None
    assert [start for start, _end, _kind in spans] == expected
    assert all(raw[start:end] == b"target" for start, end, _kind in spans)
    absent, error = gold_oracle._definition_spans(raw, b"absent", language)
    assert absent == [] and error is None


def test_gold_source_read_refuses_oversize_before_materializing(tmp_path):
    source = tmp_path / "source"
    with source.open("wb") as stream:
        stream.truncate(17 * 1024 * 1024)
    with pytest.raises(gold_oracle.EvidenceError, match="byte limit"):
        gold_oracle._bounded_source(source, 16 * 1024 * 1024)
    source.write_bytes(b"fixed")
    assert gold_oracle._bounded_source(source, 5) == b"fixed"


def test_literal_oracle_uses_original_utf8_bytes_and_overlapping_spans():
    assert gold_oracle._literal_spans("éé".encode(), "é".encode()) == [
        (0, 2, "literal_occurrence"),
        (2, 4, "literal_occurrence"),
    ]
    assert gold_oracle._literal_spans(b"aaa", b"aa") == [
        (0, 2, "literal_occurrence"),
        (1, 3, "literal_occurrence"),
    ]
    assert gold_oracle._literal_spans("e\u0301".encode(), "é".encode()) == []
    with pytest.raises(EvidenceError, match="label limit"):
        gold_oracle._literal_spans(b"a" * (gold_oracle.MAX_LABELS_PER_TASK + 1), b"a")


def test_malformed_definition_source_is_unjudged_not_negative(tmp_path):
    view = tmp_path / "view"
    view.mkdir()
    raw = b"def broken(\n"
    (view / "broken.py").write_bytes(raw)
    manifest = {
        "repository_commit": "a" * 40,
        "files": [{"path": "broken.py", "file_sha256": gold_oracle._sha(raw)}],
    }
    tasks = recipe(
        task(
            "development-broken",
            "development",
            "absent",
            "",
            "named_function_declaration",
            "python",
        ),
        task("holdout-other", "holdout", "other", ""),
    )
    gold, _blind = gold_oracle.derive(tasks, manifest, view)
    first = gold["tasks"][0]
    assert first["labels"] == []
    assert first["answerable"] is None
    assert first["label_state"] == "unjudged"
    assert first["unsupported"] == [{"path": "broken.py", "reason": "parser_error"}]


def task(task_id, split, query, scope, intent="literal_utf8_exact", language=None):
    return {
        "task_id": task_id,
        "split": split,
        "query_family_id": task_id,
        "intent": intent,
        "query": query,
        "scope_prefix": scope,
        "language": language,
        "case_semantics": "sensitive",
        "normalization": "none_raw_utf8",
    }


def recipe(*tasks):
    return {"schema_version": 1, "tasks": list(tasks)}


@pytest.fixture
def selected_release(release_seed, tmp_path):  # noqa: F811
    root = tmp_path / "release"
    shutil.copytree(release_seed, root, symlinks=True)
    document = json.loads((root / "release.json").read_bytes())
    selection = {
        "release_path": str(root),
        "release_digest": document["digest"],
        "repository": "fixture",
        "view": "code_only",
    }
    return root, selection


def test_source_bound_gold_capsule_is_unreviewed_and_blinded(selected_release, tmp_path):
    root, selection = selected_release
    tasks = recipe(
        task(
            "development-main",
            "development",
            "main",
            "src/main.rs",
            "named_function_declaration",
            "rust",
        ),
        task("holdout-print", "holdout", "print", "src/other.py"),
        task("holdout-absent", "holdout", "absent_identifier", "src/other.py"),
    )
    target = tmp_path / "gold"
    identity = binding.capture_gold(root, selection, canonical_json(tasks).encode(), target)
    assert identity == binding.validate_gold(target)
    assert identity["qualification"] == "mechanical_unreviewed_diagnostic"
    assert identity["holdout_custody"] == "unsealed_external_custody_required"
    gold = json.loads((target / "gold.json").read_bytes())
    blind = json.loads((target / "blind.json").read_bytes())
    assert [row["answerable"] for row in gold["tasks"]] == [True, True, False]
    assert [row["label_state"] for row in gold["tasks"]] == ["mechanical_unreviewed"] * 3
    assert all(
        "labels" not in row and "answerable" not in row and "split" not in row
        for row in blind["tasks"]
    )
    assert [row["query_family_id"] for row in blind["tasks"]] == [
        "development-main",
        "holdout-print",
        "holdout-absent",
    ]
    assert blind["release_digest"] == selection["release_digest"]
    assert gold["tasks"][0]["labels"][0]["kind"] == "function_item"

    original = (target / "gold.json").read_bytes()
    forged = json.loads(original)
    forged["tasks"][0]["labels"] = []
    (target / "gold.json").write_text(canonical_json(forged))
    with pytest.raises(EvidenceError, match="source-derived oracle"):
        binding.validate_gold(target)
    (target / "gold.json").write_bytes(original)
    assert binding.validate_gold(target) == identity

    view = root / "views/fixture/code_only/src/main.rs"
    view.chmod(0o644)
    view.write_bytes(b"fn main_changed() {}\n")
    with pytest.raises(EvidenceError):
        binding.validate_gold(target)


def test_split_leakage_and_unsupported_scope_fail_closed(selected_release, tmp_path):
    root, selection = selected_release
    leaked = recipe(
        task("development-main", "development", "main", "src/main.rs"),
        task("holdout-fn", "holdout", "fn", "src/main.rs"),
    )
    with pytest.raises(EvidenceError, match="file leakage"):
        binding.capture_gold(root, selection, canonical_json(leaked).encode(), tmp_path / "leaked")

    unsupported = recipe(
        task("development-main", "development", "main", "src/main.rs"),
        task(
            "holdout-missing",
            "holdout",
            "missing",
            "nonexistent",
            "named_function_declaration",
            "python",
        ),
    )
    target = tmp_path / "unjudged"
    binding.capture_gold(root, selection, canonical_json(unsupported).encode(), target)
    task_row = json.loads((target / "gold.json").read_bytes())["tasks"][1]
    assert task_row["label_state"] == "unjudged"
    assert task_row["answerable"] is None
    assert task_row["unsupported"] == [{"path": None, "reason": "empty_declared_scope"}]


@pytest.mark.parametrize(
    "mutate,error",
    [
        (lambda tasks: tasks[1].update(query_family_id=tasks[0]["query_family_id"]), "cross-split"),
        (
            lambda tasks: tasks[1].update(
                query=tasks[0]["query"], scope_prefix=tasks[0]["scope_prefix"]
            ),
            "duplicate gold query",
        ),
        (lambda tasks: tasks[1].update(query="ALPHA"), "normalized query leakage"),
        (
            lambda tasks: (
                tasks[0].update(query="find_long_identifier_variant_one"),
                tasks[1].update(query="find_long_identifier_variant_two"),
            ),
            "near-duplicate query",
        ),
        (lambda tasks: tasks[0].update(normalization="NFC"), "unsupported or ambiguous"),
        (lambda tasks: tasks[0].update(language="python"), "unsupported or ambiguous"),
    ],
)
def test_recipe_refuses_leaks_and_semantic_aliases(mutate, error):
    tasks = [
        task("development-a", "development", "alpha", "src/a.rs"),
        task("holdout-b", "holdout", "beta", "src/b.rs"),
    ]
    mutate(tasks)
    with pytest.raises(EvidenceError, match=error):
        gold_oracle.validate_recipe(recipe(*tasks))
