"""Fixed source examples are the oracle's independent expectations."""

from __future__ import annotations

import copy
import hashlib
import json
import shutil
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[3] / "tools/benchmark"))
import corpus_binding as binding
from evidence import EvidenceError, canonical_json

from tools.benchmark.retrieval import gold_oracle, source_oracle
from tools.ci.tests.test_corpus_binding import (  # noqa: F401
    disjoint_assignments,
    split_manifest,
    split_releases,
)
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


def v2_recipe(split, manifest_raw, *tasks):
    return {
        "schema_version": 2,
        "split": split,
        "split_manifest_sha256": hashlib.sha256(manifest_raw).hexdigest(),
        "tasks": [{key: value for key, value in row.items() if key != "split"} for row in tasks],
    }


@pytest.mark.parametrize(
    "mutate,error",
    [
        (lambda r: r.update(split="both"), "one split"),
        (lambda r: r.update(split_manifest_sha256="A" * 64), "one split"),
        (lambda r: r["tasks"][0].update(split="holdout"), "missing or unknown fields"),
        (lambda r: r.pop("split_manifest_sha256"), "schema v1 or v2"),
        (lambda r: r.update(tasks=[]), "schema v1 or v2"),
    ],
)
def test_v2_recipe_names_exactly_one_split_and_manifest(mutate, error):
    value = v2_recipe("holdout", b"{}", task("hold-a", "holdout", "alpha", ""))
    assert gold_oracle.validate_recipe(copy.deepcopy(value)) == value
    mutate(value)
    with pytest.raises(EvidenceError, match=error):
        gold_oracle.validate_recipe(value)


def holdout_capsule_inputs(split_releases):  # noqa: F811
    manifest, releases = split_manifest(disjoint_assignments(split_releases))
    manifest_raw = canonical_json(manifest).encode()
    root, document = split_releases["disjoint"]
    selection = {
        "release_path": str(root),
        "release_digest": document["digest"],
        "repository": "beta",
        "view": "code_only",
    }
    value = v2_recipe(
        "holdout",
        manifest_raw,
        task("hold-beta-1", "holdout", "worker_3", "", "named_function_declaration", "python"),
    )
    return root, selection, value, manifest_raw, releases


def test_v2_single_split_gold_capsule_binds_split_manifest(split_releases, tmp_path):  # noqa: F811
    root, selection, value, manifest_raw, releases = holdout_capsule_inputs(split_releases)
    target = tmp_path / "gold"
    identity = binding.capture_gold(
        root, selection, canonical_json(value).encode(), target, (manifest_raw, releases)
    )
    assert identity == binding.validate_gold(target)
    assert identity["split_binding"] == {
        "split": "holdout",
        "split_manifest_sha256": value["split_manifest_sha256"],
        "leakage_policy_id": binding.SPLIT_LEAKAGE_POLICY["id"],
        "release_digests": [selection["release_digest"]],
    }
    gold = json.loads((target / "gold.json").read_bytes())
    assert gold["split"] == "holdout" and gold["tasks"][0]["split"] == "holdout"
    label = gold["tasks"][0]["labels"]
    raw = (root / "views/beta/code_only/app/core.py").read_bytes()
    assert [(row["path"], raw[row["start_byte"] : row["end_byte"]]) for row in label] == [
        ("app/core.py", b"worker_3")
    ]
    blind = json.loads((target / "blind.json").read_bytes())
    assert "split" not in blind and "split" not in blind["tasks"][0]

    (target / "split-manifest.json").chmod(0o644)
    forged = json.loads((target / "split-manifest.json").read_bytes())
    forged["repositories"][0]["split"] = "holdout"
    (target / "split-manifest.json").write_text(canonical_json(forged))
    with pytest.raises(EvidenceError):
        binding.validate_gold(target)


@pytest.mark.parametrize(
    "mutation,error",
    [
        ("no_manifest", "requires its corpus-wide split manifest"),
        ("wrong_sha", "SHA-256 differs"),
        ("wrong_split", "another split"),
        ("extra_family", "query families differ"),
        ("missing_family", "query families differ"),
        ("v1_with_manifest", "no split manifest allowed"),
        ("other_release_path", "does not bind the selected release"),
    ],
)
def test_v2_gold_capsule_refuses_unbound_split(split_releases, tmp_path, mutation, error):  # noqa: F811
    root, selection, value, manifest_raw, releases = holdout_capsule_inputs(split_releases)
    split = (manifest_raw, releases)
    if mutation == "no_manifest":
        split = None
    elif mutation == "wrong_sha":
        value["split_manifest_sha256"] = "f" * 64
    elif mutation == "wrong_split":
        value["split"] = "development"
    elif mutation == "extra_family":
        value["tasks"].append(
            {
                **value["tasks"][0],
                "task_id": "hold-beta-2",
                "query_family_id": "hold-beta-2",
                "query": "worker_4",
            }
        )
    elif mutation == "missing_family":
        selection["repository"] = "epsilon"
    elif mutation == "v1_with_manifest":
        value = recipe(
            task("development-a", "development", "worker_1", ""),
            task("holdout-b", "holdout", "zzz_absent", ""),
        )
    else:
        releases = {selection["release_digest"]: Path("/elsewhere/release")}
        split = (manifest_raw, releases)
    with pytest.raises(EvidenceError, match=error):
        binding.capture_gold(
            root, selection, canonical_json(value).encode(), tmp_path / "gold", split
        )


def declaration_task(task_id, intent, query, language, split="holdout"):
    return {
        **task(task_id, split, query, "", intent, language),
        "query_family_id": task_id,
    }


def test_declaration_intents_label_audited_census_and_unjudge_refused_files(tmp_path):
    view = tmp_path / "view"
    (view / "pkg").mkdir(parents=True)
    files = {
        "pkg/core.py": b"class Loader:\n    def load_json(self):\n        pass\n"
        b"def load_yaml():\n    pass\n# def load_comment(): pass\n",
        "pkg/legacy.py": b"def load_json_legacy(\n",
        "pkg/lib.rs": b"pub fn load_json() {}\nconst fn loader() -> u8 { 0 }\n",
    }
    for name, raw in files.items():
        (view / name).write_bytes(raw)
    manifest = {
        "repository_commit": "a" * 40,
        "files": [
            {"path": name, "file_sha256": gold_oracle._sha(raw)}
            for name, raw in sorted(files.items())
        ],
    }
    # A v2 single-split recipe keeps one repository's tasks on one side.
    tasks = v2_recipe(
        "holdout",
        b"{}",
        declaration_task("dev-exact", "declaration_name_exact", "loader", "rust"),
        declaration_task("hold-exact", "declaration_name_exact", "load_json", "python"),
        declaration_task("hold-prefix", "declaration_name_prefix", "Loa", "python"),
        declaration_task("hold-components", "declaration_name_components", "load yaml", "python"),
        declaration_task("hold-typo", "declaration_name_osa1", "lod_json", "rust"),
    )
    gold, blind = gold_oracle.derive(tasks, manifest, view)
    rows = {row["task_id"]: row for row in gold["tasks"]}
    rust = rows["dev-exact"]
    assert [(r["path"], r["local_name"], r["kind"]) for r in rust["labels"]] == [
        ("pkg/lib.rs", "loader", "function_item")
    ]
    assert rust["label_state"] == "mechanical_unreviewed" and rust["answerable"] is True
    assert [r["local_name"] for r in rows["hold-typo"]["labels"]] == ["load_json"]
    # Both parsers refuse the broken file. A task whose query text occurs there is
    # unjudged, never negative; the others are proven absent from its bytes.
    exact = rows["hold-exact"]
    assert exact["label_state"] == "unjudged" and exact["answerable"] is None
    assert exact["unsupported"] == [{"path": "pkg/legacy.py", "reason": "census_refused"}]
    for task_id in ("hold-prefix", "hold-components"):
        row = rows[task_id]
        assert row["label_state"] == "mechanical_unreviewed" and row["answerable"] is True
        assert row["census_text_excluded"] == [
            {"path": "pkg/legacy.py", "reason": "census_refused"}
        ]
    assert [r["local_name"] for r in rows["hold-prefix"]["labels"]] == ["Loader"]
    assert [r["local_name"] for r in rows["hold-components"]["labels"]] == ["load_yaml"]
    assert gold["census_audits"]["python"]["status"] == "unsupported"
    assert gold["census_audits"]["python"]["refused_paths"] == ["pkg/legacy.py"]
    assert gold["census_audits"]["rust"]["status"] == "admitted"
    assert gold["census_audits"]["rust"]["checker"]["id"] == "rust_syn"
    assert all("labels" not in row for row in blind["tasks"])


@pytest.mark.parametrize(
    "intent,query,language",
    [
        ("declaration_name_prefix", "lo", "python"),
        ("declaration_name_components", "LoadJson", "python"),
        ("declaration_name_exact", "load json", "rust"),
        ("declaration_name_exact", "load_json", None),
        ("declaration_name_exact", "load_json", "cobol"),
    ],
)
def test_declaration_intents_refuse_queries_outside_their_contract(intent, query, language):
    value = recipe(
        declaration_task("dev-a", intent, query, language, "development"),
        task("hold-b", "holdout", "other", ""),
    )
    with pytest.raises(EvidenceError, match="unsupported or ambiguous"):
        gold_oracle.validate_recipe(value)


@pytest.mark.parametrize(
    "variant,query,raw,excluded",
    [
        ("exact", "load_json", b"def other(): pass", True),
        ("exact", "load_json", b"x = load_json", False),
        ("prefix", "load", b"loader = 1", False),
        ("infix", "oad", b"road = 1", False),
        ("components", "load yaml", b"LOAD = 1", True),
        ("components", "load yaml", b"LoadYaml", False),
        ("osa1", "abcdef", b"abXdef", False),
        ("osa1", "abcdef", b"abdcef", False),
        ("osa1", "abcdef", b"zzzzzz", True),
        ("osa1", "ab", b"zz", True),
    ],
)
def test_textual_exclusion_is_sound_for_each_variant(variant, query, raw, excluded):
    assert gold_oracle._textually_excluded(raw, query, variant) is excluded


def test_osa1_textual_exclusion_keeps_every_one_edit_name_unjudged():
    """Exhaust short names against the independent canonical OSA-1 predicate.

    The source wrapper has invalid UTF-8 bytes to exercise replacement decoding;
    any possible declared name written in those bytes must prevent exclusion.
    """
    import itertools

    names = [
        "".join(chars) for size in range(1, 6) for chars in itertools.product("abé$", repeat=size)
    ]
    for size in range(2, 5):
        for chars in itertools.product("ab", repeat=size):
            query = "".join(chars)
            for name in names:
                if not source_oracle.osa_distance_at_most_one(query, name):
                    continue
                raw = b"\xff" + name.encode("utf-8") + b"\xfe"
                assert not gold_oracle._textually_excluded(raw, query, "osa1"), (query, name)


def test_osa1_textual_exclusion_has_bounded_ambiguous_fallback():
    assert not gold_oracle._textually_excluded(b"other", "a" * 65, "osa1")
    assert gold_oracle._textually_excluded(b"invalid source", "parseEror", "osa1")


def test_refused_file_without_query_text_keeps_the_task_judged(tmp_path):
    view = tmp_path / "view"
    view.mkdir()
    files = {
        "core.py": b"def load_json():\n    pass\n",
        "legacy.py": b"def legacy(\n",
    }
    for name, raw in files.items():
        (view / name).write_bytes(raw)
    manifest = {
        "repository_commit": "a" * 40,
        "files": [
            {"path": name, "file_sha256": gold_oracle._sha(raw)}
            for name, raw in sorted(files.items())
        ],
    }
    value = v2_recipe(
        "holdout",
        b"{}",
        declaration_task("judged", "declaration_name_exact", "load_json", "python"),
        declaration_task("unjudged", "declaration_name_exact", "legacy", "python"),
        declaration_task("typo", "declaration_name_osa1", "load_jsom", "python"),
    )
    gold, _blind = gold_oracle.derive(value, manifest, view)
    judged, unjudged, typo = gold["tasks"]
    assert judged["label_state"] == "mechanical_unreviewed" and judged["answerable"] is True
    assert judged["census_text_excluded"] == [{"path": "legacy.py", "reason": "census_refused"}]
    assert unjudged["label_state"] == "unjudged" and unjudged["answerable"] is None
    assert typo["label_state"] == "mechanical_unreviewed" and typo["answerable"] is True
    assert typo["census_text_excluded"] == [{"path": "legacy.py", "reason": "census_refused"}]


def test_census_disagreement_cannot_be_text_excluded(tmp_path, monkeypatch):
    view = tmp_path / "view"
    view.mkdir()
    files = {
        "core.py": b"def target():\n    pass\n",
        "disputed.py": b"def other():\n    pass\n",
    }
    for path, raw in files.items():
        (view / path).write_bytes(raw)
    manifest = {
        "repository_commit": "a" * 40,
        "files": [
            {"path": path, "file_sha256": gold_oracle._sha(raw)}
            for path, raw in sorted(files.items())
        ],
    }
    monkeypatch.setattr(
        gold_oracle,
        "_census_audits",
        lambda _recipe, _sources, _view: {
            "python": {"refused_paths": [], "disagreement_paths": ["disputed.py"]}
        },
    )
    value = v2_recipe(
        "holdout",
        b"{}",
        declaration_task("target", "declaration_name_exact", "target", "python"),
    )
    gold, _blind = gold_oracle.derive(value, manifest, view)
    row = gold["tasks"][0]
    assert row["labels"][0]["path"] == "core.py"
    assert row["unsupported"] == [{"path": "disputed.py", "reason": "census_disagreement"}]
    assert row["census_text_excluded"] == []
    assert row["answerable"] is None
    monkeypatch.setattr(
        gold_oracle,
        "_census_audits",
        lambda _recipe, _sources, _view: {
            "python": {"refused_paths": ["disputed.py"], "disagreement_paths": []}
        },
    )
    gold, _blind = gold_oracle.derive(value, manifest, view)
    row = gold["tasks"][0]
    assert row["unsupported"] == [{"path": "disputed.py", "reason": "census_refused"}]
    assert row["census_text_excluded"] == []
    assert row["answerable"] is None


def test_holdout_sampling_freezes_seeded_ledger_recipes_and_split(split_releases, tmp_path):  # noqa: F811
    from tools.benchmark.retrieval import holdout_sampling

    hold_root, hold_doc = split_releases["hold_only"]
    dev_root, dev_doc = split_releases["dev_only"]
    first = holdout_sampling.build(hold_root, dev_root, 11)
    again = holdout_sampling.build(hold_root, dev_root, 11)
    other = holdout_sampling.build(hold_root, dev_root, 12)
    assert canonical_json(first[0]) == canonical_json(again[0]) and first[1] == again[1]
    assert first[1] != other[1]
    ledger, recipes, manifest_raw, manifest = first
    recipe = recipes["beta"]
    assert recipe["split"] == "holdout"
    assert recipe["split_manifest_sha256"] == hashlib.sha256(manifest_raw).hexdigest()
    beta = ledger["repositories"]["beta"]
    assert ledger["sampling_version"] == 2
    assert beta["census_audit"]["status"] == "admitted"
    lanes = {task["task_id"].split(".")[1] for task in recipe["tasks"]}
    assert lanes == {"lit", "def", "pre", "inf", "com", "osa"}
    assert beta["exact_definition"]["admitted"] == 12  # worker_0 .. worker_11
    assert beta["natural_language_workflow"]["underfilled"] == 20
    assert beta["no_answer_wrong_repository"]["admitted"] == 0
    assert beta["exact_definition"]["inclusion_probability"] == (
        beta["exact_definition"]["admitted"] / beta["exact_definition"]["population"]
    )
    assert beta["exact_definition"]["inclusion_probability_basis"] == (
        "nominal_uniform_seeded_rank_over_eligible_names"
    )
    for lane in (
        "exact_content",
        "variant_prefix",
        "variant_infix",
        "variant_components",
        "variant_osa1",
        "no_answer_synthetic",
        "no_answer_wrong_repository",
        "natural_language_workflow",
    ):
        assert beta[lane]["inclusion_probability"] is None
        assert beta[lane]["inclusion_probability_basis"].startswith("not_derived_")
    for row in beta.values():
        if isinstance(row, dict) and "quota" in row:
            assert row["admitted"] + row["underfilled"] == row["quota"]
    rows = {row["repository"]: row for row in manifest["repositories"]}
    assert rows["alpha"]["split"] == "development" and rows["alpha"]["query_family_ids"] == []
    assert rows["beta"]["query_family_ids"] == sorted(
        {task["query_family_id"] for task in recipe["tasks"]}
    )
    releases = {hold_doc["digest"]: hold_root, dev_doc["digest"]: dev_root}
    assert binding.validate_split_manifest(manifest_raw, releases) == manifest
    selection = {
        "release_path": str(hold_root),
        "release_digest": hold_doc["digest"],
        "repository": "beta",
        "view": "code_only",
    }
    target = tmp_path / "gold"
    identity = binding.capture_gold(
        hold_root, selection, canonical_json(recipe).encode(), target, (manifest_raw, releases)
    )
    gold = json.loads((target / "gold.json").read_bytes())
    assert identity["split_binding"]["split"] == "holdout"
    assert all(row["label_state"] == "mechanical_unreviewed" for row in gold["tasks"])
    assert all(row["answerable"] for row in gold["tasks"])
