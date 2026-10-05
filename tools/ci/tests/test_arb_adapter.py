"""arb-nl-adapter-v2: determinism, gold blindness, and effective term budgets."""

from __future__ import annotations

import copy
import hashlib
import json
import os
from pathlib import Path

import pytest

from tools.benchmark.retrieval import arb_adapter
from tools.benchmark.retrieval.arb_adapter import ArbAdapterRefusal, adapt
from tools.benchmark.retrieval.query_plan import (
    QueryPlanError,
    natural_language_terms,
    plan_lexical_request,
    tokenize_nl,
)

ARB_GIN_DATA = Path(
    os.environ.get(
        "ARB_GIN_BENCHMARK_DIR",
        "/Users/songmin/Documents/code-new/qi-s30-bench-trust-20260930-0d21914e/b06/data/benchmark",
    )
)
ARB_RELEASES = ("v2_trace2code", "v2_code2test", "v2_edit2ripple")

FORBIDDEN_GOLD_KEYS = (
    "root_cause_files",
    "related_tests",
    "supporting_files",
    "files",
    "given_files",
    "fix_commit",
    "negative_distractors",
    "root_cause_symbols",
)
FORBIDDEN_TOP_KEYS = (
    "gold",
    "gold_blocks",
    "gold_spans",
    "hard_negative_files",
    "metadata",
    "audit",
    "candidate_corpus",
    "query_provenance",
    "version",
)


def _trace_sample(excerpt: str = "", command: str = "go test ./.") -> dict:
    return {
        "id": "s-trace",
        "task_type": "trace2code",
        "repo": "gin-gonic/gin",
        "base_commit": "0" * 40,
        "query": {
            "command": command,
            "failure_excerpt": excerpt,
            "run_strategy": "go_test_package",
            "source_type": "local_test_reproduction",
        },
        "gold": {
            "root_cause_files": ["logger.go"],
            "related_tests": ["logger_test.go"],
            "supporting_files": ["context.go"],
            "fix_commit": "a" * 40,
        },
        "gold_spans": [{"path": "logger.go", "start_line": 1, "end_line": 2}],
        "gold_blocks": [{"path": "logger.go", "start_line": 1, "end_line": 2}],
        "hard_negative_files": ["gin.go"],
        "metadata": {"note": "secret"},
    }


def _assert_plan_accepts(result: dict) -> None:
    text = result["adapted_text"]
    tokens = tokenize_nl(text)
    assert tokens == result["tokens"]
    assert 1 <= len(set(tokens)) <= 32
    assert len(tokens) == len(set(tokens))
    assert all(len(token) <= 96 for token in tokens)
    assert 1 <= len(natural_language_terms(text)) <= 32
    plan_lexical_request("natural_language", text)


def _gin_samples() -> list[dict]:
    rows = []
    for release in ARB_RELEASES:
        path = ARB_GIN_DATA / release / "samples.jsonl"
        if not path.is_file():
            return []
        for line in path.read_text(encoding="utf-8").splitlines():
            sample = json.loads(line)
            if sample.get("repo") == "gin-gonic/gin":
                rows.append(sample)
    return rows


def test_rule_is_frozen_and_documented():
    assert arb_adapter.ADAPTER_VERSION == "arb-nl-adapter-v2"
    assert arb_adapter.RULE_TEXT in (arb_adapter.__doc__ or "")
    assert arb_adapter.RULE_SHA256 == (
        "88c17874014115b84e8b27a57194b09da5d3ba627fc1d0aee78b3b0709d652f5"
    )
    assert (
        arb_adapter.RULE_SHA256 == hashlib.sha256(arb_adapter.RULE_TEXT.encode("utf-8")).hexdigest()
    )


def test_adapt_is_deterministic_and_does_not_mutate_input():
    sample = _trace_sample("./logger_test.go:325:12: p.LatencyColor undefined (type X)")
    before = copy.deepcopy(sample)
    first = adapt(sample)
    second = adapt(copy.deepcopy(sample))
    assert first == second
    assert sample == before
    assert first["adapter"] == "arb-nl-adapter-v2"
    expected_query = json.dumps(sample["query"], ensure_ascii=False, sort_keys=True)
    assert first["original_query_sha256"] == hashlib.sha256(expected_query.encode()).hexdigest()


@pytest.mark.parametrize("key", FORBIDDEN_TOP_KEYS)
def test_mutating_forbidden_top_level_field_never_changes_output(key):
    sample = _trace_sample("panic in logger.go ServeHTTP")
    baseline = adapt(sample)
    for replacement in (None, "logger.go ServeHTTP injected", {"x": ["y"]}, ["gin.go"] * 50):
        mutated = copy.deepcopy(sample)
        mutated[key] = replacement
        assert adapt(mutated) == baseline
    removed = copy.deepcopy(sample)
    removed.pop(key, None)
    assert adapt(removed) == baseline


@pytest.mark.parametrize("key", FORBIDDEN_GOLD_KEYS)
def test_mutating_forbidden_gold_field_never_changes_output(key):
    sample = _trace_sample("panic in logger.go ServeHTTP")
    baseline = adapt(sample)
    mutated = copy.deepcopy(sample)
    mutated["gold"][key] = ["injected_gold_path.go", "RootCauseSymbol"]
    assert adapt(mutated) == baseline


def test_projection_exposes_only_allowed_keys():
    projected = arb_adapter.project_input(_trace_sample("x y"))
    assert tuple(projected) == arb_adapter.ALLOWED_SAMPLE_KEYS


def test_forbidden_mutation_on_real_samples_never_changes_output():
    samples = _gin_samples()
    if not samples:
        pytest.skip("ARB gin data not present")
    for sample in samples[::7]:
        baseline = adapt(sample)
        mutated = copy.deepcopy(sample)
        mutated["gold"] = {key: ["poison.go"] for key in FORBIDDEN_GOLD_KEYS}
        for key in FORBIDDEN_TOP_KEYS[1:]:
            mutated[key] = ["poison.go"]
        assert adapt(mutated) == baseline


def test_huge_tokens_are_split_on_joiners_and_oversized_pieces_dropped():
    long_path = "/".join(["segment_name"] * 20)  # 259 chars, joined
    unsplittable = "a" * 200
    result = adapt(_trace_sample(f"{long_path} {unsplittable} ok_word"))
    _assert_plan_accepts(result)
    assert result["dropped"]["split_over_96_chars"] == 2
    assert result["dropped"]["too_long_piece"] == 1
    assert "segment" in result["tokens"]
    assert "name" in result["tokens"]
    assert unsplittable not in result["tokens"]


def test_ten_thousand_tokens_capped_at_32():
    words = " ".join(f"word{i} Snake_{i}_case" for i in range(5000))
    result = adapt(_trace_sample(words))
    _assert_plan_accepts(result)
    assert result["token_count"] == 32
    assert result["dropped"]["over_limit"] > 0


def test_unicode_tokens_respect_char_and_byte_limits():
    cjk = "漢" * 95  # 95 chars, 285 bytes: over the 256-byte index term cap
    combining = "é" * 40
    text = f"{cjk} 日本語テキスト {combining} Ünïcödé_ident ﬁle naïve"
    result = adapt(_trace_sample(text, command=""))
    _assert_plan_accepts(result)
    assert cjk not in result["tokens"]
    assert result["dropped"]["index_term_too_long"] == 1
    assert "日本語テキスト" in result["tokens"]


@pytest.mark.parametrize("text", ["", "   ", "!!! ??? ... --- ///", "1 2 3 42 1.19", "a b c"])
def test_empty_or_punctuation_only_input_is_typed_refusal(text):
    sample = _trace_sample(text, command="")
    sample["query"] = {"failure_excerpt": text, "command": ""}
    with pytest.raises(ValueError) as caught:
        adapt(sample)
    assert isinstance(caught.value, ArbAdapterRefusal)
    assert caught.value.code == "ARB_ADAPTER_EMPTY"


def test_non_dict_query_is_typed_refusal():
    sample = _trace_sample("x")
    sample["query"] = "not a dict"
    with pytest.raises(ArbAdapterRefusal):
        adapt(sample)


def test_numbers_hashes_short_tokens_and_duplicates_dropped():
    text = "325 1.19 2021-01-02 deadbeef1 0123abc x ok ok Handler 0d21914e53b8 defaced"
    result = adapt(_trace_sample(text, command=""))
    assert result["tokens"] == [
        "ok",
        "Handler",
        "defaced",
        "go_test_package",
        "local_test_reproduction",
    ]
    dropped = result["dropped"]
    assert dropped["pure_number"] == 3
    assert dropped["hex_hash"] == 3
    assert dropped["too_short"] == 1
    assert dropped["duplicate"] == 1


def test_edge_joiners_stripped_but_underscore_kept():
    result = adapt(_trace_sample("./logger_test.go deprecated. __init__ -flag", command=""))
    assert result["tokens"][:4] == ["logger_test.go", "deprecated", "__init__", "flag"]


@pytest.mark.parametrize(
    "task_type,query,expected_fields",
    [
        (
            "trace2code",
            {"source_type": "ss", "run_strategy": "rr", "command": "cc", "failure_excerpt": "ff"},
            ["failure_excerpt", "command", "run_strategy", "source_type"],
        ),
        (
            "code2test",
            {
                "changed_file_summary": "a",
                "implementation_file_count": 1,
                "implementation_files": ["x.go"],
                "pr_body": "b",
                "pr_title": "t",
            },
            [
                "pr_title",
                "pr_body",
                "implementation_files",
                "changed_file_summary",
                "implementation_file_count",
            ],
        ),
        (
            "edit2ripple",
            {"intent": "i", "anchor_file": "context.go", "anchor_diff": "d"},
            ["anchor_file", "anchor_diff", "intent"],
        ),
        ("unknown_task", {"b": "xx", "a": "yy"}, ["a", "b"]),
    ],
)
def test_per_task_field_order(task_type, query, expected_fields):
    sample = {"id": "s", "task_type": task_type, "repo": "r", "base_commit": "c", "query": query}
    result = adapt(sample)
    assert result["fields_used"] == [f"query.{field}" for field in expected_fields]


def test_field_order_determines_token_order():
    sample = {
        "id": "s",
        "task_type": "edit2ripple",
        "repo": "r",
        "base_commit": "c",
        "query": {"intent": "intentword", "anchor_diff": "diffword", "anchor_file": "context.go"},
    }
    assert adapt(sample)["tokens"] == ["context.go", "diffword", "intentword"]


def test_identifier_preference_when_over_limit():
    plain = " ".join(f"plain{i}" for i in range(40))
    idents = " ".join(["snake_case_a", "pkg.Func", "path/to/file", "camelCase", "Plain"])
    result = adapt(_trace_sample(f"{plain} {idents}", command=""))
    _assert_plan_accepts(result)
    tokens = result["tokens"]
    for ident in ("snake_case_a", "pkg.Func", "path/to/file", "camelCase"):
        assert ident in tokens
    assert "go_test_package" in tokens and "local_test_reproduction" in tokens
    assert "Plain" not in tokens
    assert tokens[:23] == [f"plain{i}" for i in range(23)]
    assert tokens[23:] == [
        "snake_case_a",
        "pkg.Func",
        "path/to/file",
        "camelCase",
        "go_test_package",
        "local_test_reproduction",
    ]
    assert result["dropped"]["over_limit"] == 47 - 29
    assert len(tokens) == 29
    assert result["effective_term_count"] == 32
    assert len(natural_language_terms(result["adapted_text"])) == 32


def test_joined_identifiers_use_exact_prebudget_planner_terms():
    assert natural_language_terms("pkg.Func path/to/file") == ["pkg", "func", "path", "to", "file"]
    assert plan_lexical_request("natural_language", "pkg.Func path/to/file") == (
        "case:no pkg OR func OR path OR to OR file"
    )
    sample = _trace_sample("pkg.Func path/to/file", command="")
    sample["query"] = {"failure_excerpt": "pkg.Func path/to/file"}
    result = adapt(sample)
    assert result["tokens"] == ["pkg.Func", "path/to/file"]
    assert result["effective_term_count"] == 5
    _assert_plan_accepts(result)


def test_under_cap_tokens_keep_original_text_and_order():
    sample = _trace_sample("", command="")
    sample["query"] = {"failure_excerpt": "alpha beta_gamma DELTA"}
    result = adapt(sample)
    assert result["tokens"] == ["alpha", "beta_gamma", "DELTA"]
    assert result["adapted_text"] == "alpha beta_gamma DELTA"
    assert result["effective_term_count"] == 3
    assert result["dropped"]["over_limit"] == 0
    assert plan_lexical_request("natural_language", result["adapted_text"]) == (
        "case:no alpha OR beta_gamma OR delta"
    )


def test_normalized_overlap_counts_once_and_preserves_safe_under_cap_text():
    assert natural_language_terms("e\u0301 É pkg.Func pkg/func") == ["é", "pkg", "func"]
    sample = _trace_sample("", command="")
    sample["query"] = {"failure_excerpt": "e\u0301e\u0301 ÉÉ pkg.Func pkg/func"}
    result = adapt(sample)
    assert result["tokens"] == ["éé", "ÉÉ", "pkg.Func", "pkg/func"]
    assert result["adapted_text"] == "éé ÉÉ pkg.Func pkg/func"
    assert result["effective_term_count"] == 3
    _assert_plan_accepts(result)


def test_unicode_lowercase_expansion_cannot_emit_an_oversized_index_term():
    expanded = "İ" * 96  # 192 raw UTF-8 bytes; lowercase adds combining dots.
    with pytest.raises(QueryPlanError, match="lexical term max"):
        natural_language_terms(expanded)
    sample = _trace_sample("", command="")
    sample["query"] = {"failure_excerpt": expanded + " safe_word"}
    result = adapt(sample)
    assert result["tokens"] == ["safe_word"]
    assert result["dropped"]["index_term_too_long"] == 1
    _assert_plan_accepts(result)


def test_effective_budget_rejects_overfull_joiner_but_retains_later_terms():
    raw = " ".join(f"plain{i}" for i in range(31)) + " path/to/file"
    assert len(natural_language_terms(raw)) == 34
    with pytest.raises(QueryPlanError, match="34 tokens"):
        plan_lexical_request("natural_language", raw)
    sample = _trace_sample("", command="")
    sample["query"] = {"failure_excerpt": raw}
    result = adapt(sample)
    assert result["tokens"] == [f"plain{i}" for i in range(29)] + ["path/to/file"]
    assert result["dropped"]["over_limit"] == 2
    assert result["effective_term_count"] == 32
    assert len(natural_language_terms(result["adapted_text"])) == 32
    _assert_plan_accepts(result)


def test_all_real_gin_samples_are_plan_accepted():
    samples = _gin_samples()
    if not samples:
        pytest.skip("ARB gin data not present")
    assert len(samples) == 88
    for sample in samples:
        result = adapt(sample)
        _assert_plan_accepts(result)


def test_cli_freeze_writes_outputs_and_refuses_existing_dir(tmp_path):
    samples_path = tmp_path / "samples.jsonl"
    rows = [_trace_sample("alpha beta_gamma"), {**_trace_sample("x"), "repo": "other/repo"}]
    samples_path.write_text("".join(json.dumps(row) + "\n" for row in rows), encoding="utf-8")
    out = tmp_path / "freeze"
    code = arb_adapter.main(
        ["freeze", "--samples", str(samples_path), "--repo", "gin-gonic/gin", "--out", str(out)]
    )
    assert code == 0
    adapted = [json.loads(line) for line in (out / "adapted.jsonl").read_text().splitlines()]
    assert len(adapted) == 1 and adapted[0]["status"] == "adapted"
    manifest = json.loads((out / "manifest.json").read_text())
    assert manifest["rule_sha256"] == arb_adapter.RULE_SHA256
    assert (
        manifest["source_samples"][0]["sha256"]
        == hashlib.sha256(samples_path.read_bytes()).hexdigest()
    )
    assert manifest["counts"]["adapted"] == 1
    before = (out / "adapted.jsonl").read_bytes()
    assert arb_adapter.main(["freeze", "--samples", str(samples_path), "--out", str(out)]) == 2
    assert (out / "adapted.jsonl").read_bytes() == before
    empty = tmp_path / "empty"
    empty.mkdir()
    assert arb_adapter.main(["freeze", "--samples", str(samples_path), "--out", str(empty)]) == 2
    assert not any(empty.iterdir())


def test_cli_freeze_records_refusal_and_exits_nonzero(tmp_path):
    samples_path = tmp_path / "samples.jsonl"
    bad = _trace_sample("")
    bad["query"] = {"failure_excerpt": "!!!", "command": ""}
    samples_path.write_text(json.dumps(bad) + "\n", encoding="utf-8")
    out = tmp_path / "freeze"
    assert arb_adapter.main(["freeze", "--samples", str(samples_path), "--out", str(out)]) == 3
    row = json.loads((out / "adapted.jsonl").read_text())
    assert row["status"] == "refused" and row["refusal_code"] == "ARB_ADAPTER_EMPTY"


def test_freeze_refuses_malformed_rows_and_publishes_atomically(tmp_path):
    from tools.benchmark.retrieval import arb_adapter

    def write(name, rows):
        path = tmp_path / name
        path.write_text("\n".join(rows) + "\n", encoding="utf-8")
        return path

    good = json.dumps(
        {
            "id": "s1",
            "repo": "gin-gonic/gin",
            "base_commit": "a" * 40,
            "task_type": "trace2code",
            "query": {"failure_excerpt": "FAIL TestContextBind  context_test.go"},
        }
    )
    manifest = arb_adapter.freeze([write("ok.jsonl", [good])], "gin-gonic/gin", tmp_path / "ok")
    assert manifest["counts"]["total"] == 1
    assert not (tmp_path / "ok.staging").exists()
    for name, rows, match in [
        ("dup.jsonl", [good, good], "duplicate sample id"),
        ("list.jsonl", ["[1, 2]"], "not a JSON object"),
        ("noid.jsonl", [good.replace('"id": "s1"', '"id": null')], "id must be"),
    ]:
        with pytest.raises(ValueError, match=match):
            arb_adapter.freeze([write(name, rows)], "gin-gonic/gin", tmp_path / (name + ".out"))
        assert not (tmp_path / (name + ".out")).exists()
        assert not (tmp_path / (name + ".out.staging")).exists()
