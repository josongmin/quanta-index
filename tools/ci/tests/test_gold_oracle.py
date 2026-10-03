"""Fixed source examples are the oracle's independent expectations."""

from __future__ import annotations

import copy
import hashlib
import json
import shutil
import subprocess
import sys
from pathlib import Path
from types import SimpleNamespace

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[3] / "tools/benchmark"))
import corpus_binding as binding
from evidence import canonical_json

from tools.benchmark.evidence import EvidenceError as OracleEvidenceError
from tools.benchmark.retrieval import gold_oracle, source_oracle
from tools.ci.tests.test_corpus_binding import (  # noqa: F401
    disjoint_assignments,
    split_manifest,
    split_releases,
)
from tools.ci.tests.test_corpus_release import release_seed, source_seed  # noqa: F401

# Direct benchmark scripts import ``evidence``; packaged retrieval code imports
# ``tools.benchmark.evidence``. Assert the public refusal from either entrypoint.
EvidenceError = (OracleEvidenceError, binding.EvidenceError)

FIXTURES = Path(__file__).parent / "fixtures" / "gold_oracle"


def test_casefold_osa1_recipe_requires_explicit_case_semantics():
    row = {
        "task_id": "typo-1",
        "query_family_id": "family-1",
        "intent": "declaration_name_osa1_casefold",
        "query": "pram",
        "intended_name": "Param",
        "scope_prefix": "",
        "language": "go",
        "case_semantics": "casefold",
        "normalization": "none_raw_utf8",
    }
    recipe = {
        "schema_version": 2,
        "split": "holdout",
        "split_manifest_sha256": "a" * 64,
        "tasks": [row],
    }
    assert gold_oracle.validate_recipe(recipe) == recipe
    raw = b"package p\nfunc Param() {}\n"
    assert len(gold_oracle._declaration_spans(raw, "p.go", "pram", "go", row["intent"], {})) == 1
    bad = copy.deepcopy(recipe)
    bad["tasks"][0]["case_semantics"] = "sensitive"
    with pytest.raises(EvidenceError, match="query semantics"):
        gold_oracle.validate_recipe(bad)
    bad = copy.deepcopy(recipe)
    bad["tasks"][0]["intended_name"] = "Unrelated"
    with pytest.raises(EvidenceError, match="one casefolded edit"):
        gold_oracle.validate_recipe(bad)


def test_component_recipe_requires_folded_name_semantics():
    row = {
        "task_id": "component-1",
        "query_family_id": "family-1",
        "intent": "declaration_name_components",
        "query": "clean up",
        "scope_prefix": "",
        "language": "go",
        "case_semantics": "casefold",
        "normalization": "none_raw_utf8",
    }
    recipe = {
        "schema_version": 2,
        "split": "holdout",
        "split_manifest_sha256": "a" * 64,
        "tasks": [row],
    }
    assert gold_oracle.validate_recipe(recipe) == recipe
    bad = copy.deepcopy(recipe)
    bad["tasks"][0]["case_semantics"] = "sensitive"
    with pytest.raises(EvidenceError, match="query semantics"):
        gold_oracle.validate_recipe(bad)


def test_casefold_typo_gold_keeps_intended_and_near_declarations_separate(tmp_path):
    view = tmp_path / "view"
    view.mkdir()
    originals = {
        "intended.go": b"package p\nfunc Param() {}\n",
        "neighbor.go": b"package p\nfunc Pram() {}\n",
    }
    for path, raw in originals.items():
        (view / path).write_bytes(raw)
    manifest = {
        "repository_commit": "a" * 40,
        "files": [
            {"path": path, "file_sha256": gold_oracle._sha(raw)}
            for path, raw in sorted(originals.items())
        ],
    }
    recipe = {
        "schema_version": 2,
        "split": "holdout",
        "split_manifest_sha256": "a" * 64,
        "tasks": [
            {
                "task_id": "typo-1",
                "query_family_id": "family-1",
                "intent": "declaration_name_osa1_casefold",
                "query": "pram",
                "intended_name": "Param",
                "scope_prefix": "",
                "language": "go",
                "case_semantics": "casefold",
                "normalization": "none_raw_utf8",
            }
        ],
    }
    gold, blind = gold_oracle.derive(recipe, manifest, view)
    task = gold["tasks"][0]
    assert [(label["path"], label["local_name"]) for label in task["labels"]] == [
        ("intended.go", "Param")
    ]
    assert task["near_declaration_names"] == ["Param"]
    assert task["near_declaration_files"] == ["intended.go"]
    assert task["exact_collision_names"] == ["Pram"]
    assert task["exact_collision_files"] == ["neighbor.go"]
    assert "intended_name" not in blind["tasks"][0]
    bound = copy.deepcopy(recipe)
    bound["checker_identity"] = {"go": gold["census_audits"]["go"]["checker"]}
    assert gold_oracle.derive(bound, manifest, view)[0]["tasks"] == gold["tasks"]
    bound["checker_identity"]["go"]["version"] = "wrong-toolchain"
    with pytest.raises(EvidenceError, match="checker identity differs"):
        gold_oracle.derive(bound, manifest, view)


@pytest.mark.parametrize("near_text_present", [False, True])
def test_typo_near_census_uses_query_specific_absence_for_refused_file(tmp_path, near_text_present):
    view = tmp_path / "view"
    view.mkdir()
    files = {
        "main.go": b"package p\nfunc Param() {}\n",
        "broken.go": b"package p\n"
        + (b"// Paran\n" if near_text_present else b"")
        + b"func Broken(",
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
    recipe = {
        "schema_version": 2,
        "split": "holdout",
        "split_manifest_sha256": "a" * 64,
        "tasks": [
            {
                "task_id": "typo-refused",
                "query_family_id": "family-Param",
                "intent": "declaration_name_osa1_casefold",
                "query": "Paran",
                "intended_name": "Param",
                "scope_prefix": "",
                "language": "go",
                "case_semantics": "casefold",
                "normalization": "none_raw_utf8",
            }
        ],
    }
    gold, _blind = gold_oracle.derive(recipe, manifest, view)
    row = gold["tasks"][0]
    assert [(label["path"], label["local_name"]) for label in row["labels"]] == [
        ("main.go", "Param")
    ]
    assert row["census_text_excluded"] == [{"path": "broken.go", "reason": "census_refused"}]
    assert row["near_declaration_state"] == ("partial" if near_text_present else "complete")
    assert row["near_census_text_excluded"] == (
        [] if near_text_present else [{"path": "broken.go", "reason": "census_refused"}]
    )


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


def batch_inputs(release_fixtures):
    manifest, releases = split_manifest(disjoint_assignments(release_fixtures))
    manifest_raw = canonical_json(manifest).encode()
    root, _document = release_fixtures["disjoint"]
    recipes = {
        "alpha": v2_recipe(
            "development",
            manifest_raw,
            task("dev-alpha-1", "development", "handler_1", ""),
            task("dev-alpha-2", "development", "handler_2", ""),
        ),
        "beta": v2_recipe(
            "holdout",
            manifest_raw,
            task("hold-beta-1", "holdout", "worker_3", ""),
        ),
        "epsilon": v2_recipe(
            "holdout",
            manifest_raw,
            task("hold-epsilon-1", "holdout", "runner_3", ""),
        ),
    }
    return (
        root,
        {name: canonical_json(value).encode() for name, value in recipes.items()},
        (
            manifest_raw,
            releases,
        ),
    )


def test_gold_batch_replays_global_split_twice_and_publishes_independent_capsules(
    split_releases,  # noqa: F811
    tmp_path,
    monkeypatch,
):
    root, recipes, split = batch_inputs(split_releases)
    calls = 0
    original = binding._validated_split_manifest

    def counted(*args):
        nonlocal calls
        calls += 1
        return original(*args)

    monkeypatch.setattr(binding, "_validated_split_manifest", counted)
    target = tmp_path / "batch"
    identities = binding.capture_gold_batch(root, recipes, target, split)
    assert calls == 2
    assert set(identities) == set(recipes)
    for name, identity in identities.items():
        assert identity["schema_version"] == 2
        assert identity["producer_source_digests"] == binding._gold_producer_source_digests()
        assert {"python", "unicode", "tree_sitter", "tree_sitter_language_pack"} == set(
            identity["parser_runtime"]
        )
        assert binding.validate_gold(target / name) == identity
        selection = json.loads((target / name / "selection.json").read_bytes())
        assert selection["repository"] == name


def test_gold_batch_refuses_source_drift_before_atomic_publication(
    split_releases,  # noqa: F811
    tmp_path,
    monkeypatch,
):
    root, recipes, split = batch_inputs(split_releases)
    copied = tmp_path / "copied-release"
    shutil.copytree(root, copied, symlinks=True)
    split = (split[0], {digest: copied for digest in split[1]})
    root = copied
    original = binding._gold_material
    changed = False

    def mutate_after_material(*args, **kwargs):
        nonlocal changed
        material = original(*args, **kwargs)
        if not changed and args[1]["repository"] == "epsilon":
            changed = True
            source = root / "views/beta/code_only/app/core.py"
            source.chmod(0o644)
            source.write_bytes(b"def forged(): pass\n")
        return material

    monkeypatch.setattr(binding, "_gold_material", mutate_after_material)
    target = tmp_path / "batch"
    with pytest.raises(EvidenceError):
        binding.capture_gold_batch(root, recipes, target, split)
    assert not target.exists() and not target.with_name("batch.staging").exists()


def test_gold_batch_refuses_partial_or_wrong_recipe_inventory(split_releases, tmp_path):  # noqa: F811
    root, recipes, split = batch_inputs(split_releases)
    with pytest.raises(EvidenceError, match="recipe inventory"):
        binding.capture_gold_batch(root, {"beta": recipes["beta"]}, tmp_path / "partial", split)
    mismatched = dict(recipes)
    mismatched["beta"] = recipes["epsilon"]
    with pytest.raises(EvidenceError, match="query families differ"):
        binding.capture_gold_batch(root, mismatched, tmp_path / "mismatched", split)
    assert not (tmp_path / "mismatched").exists()


def test_gold_batch_cli_reads_sampler_inventory(split_releases, tmp_path):  # noqa: F811
    from tools.benchmark.retrieval import gold_capture_batch

    root, recipes, split = batch_inputs(split_releases)
    sampling = tmp_path / "sampling"
    (sampling / "recipes").mkdir(parents=True)
    for name, raw in recipes.items():
        (sampling / "recipes" / f"{name}.json").write_bytes(raw)
    (sampling / "split-manifest.json").write_bytes(split[0])
    target = tmp_path / "captured"
    identities = gold_capture_batch.capture(root, root, sampling, target)
    assert set(identities) == {"alpha", "beta", "epsilon"}
    for name in identities:
        assert binding.validate_gold(target / name) == identities[name]


def test_gold_batch_script_imports_from_direct_entrypoint():
    script = Path(__file__).resolve().parents[2] / "benchmark/retrieval/gold_capture_batch.py"
    result = subprocess.run([sys.executable, str(script), "--help"], capture_output=True, text=True)
    assert result.returncode == 0, result.stderr
    assert "--sampling" in result.stdout


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
        "case_semantics": ("casefold" if intent in gold_oracle.CASEFOLD_INTENTS else "sensitive"),
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
    assert exact["unsupported"] == [
        {"path": "pkg/legacy.py", "reason": "census_refused"},
        {"path": "pkg/lib.rs", "reason": "other_language_matching_declaration"},
    ]
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


def test_unscoped_declaration_gold_audits_other_language_files(tmp_path):
    view = tmp_path / "view"
    view.mkdir()
    files = {
        "target.ts": b"export function hello() {}\nexport function OnlyTs() {}\n",
        "collision.js": b"export function HELLP() {}\n",
        "near.js": b"export function help() {}\n",
        "same.js": b"export function hello() {}\n",
        "unrelated.js": b"// OnlyTs is a use, not a declaration\n"
        b"console.log(hello);\nexport function elsewhere() {}\n",
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
    recipe = v2_recipe(
        "holdout",
        b"{}",
        declaration_task("same", "declaration_name_exact", "hello", "typescript"),
        declaration_task("only", "declaration_name_exact", "OnlyTs", "typescript"),
        {
            **declaration_task("typo", "declaration_name_osa1_casefold", "hellp", "typescript"),
            "intended_name": "hello",
        },
    )
    gold, _blind = gold_oracle.derive(recipe, manifest, view)
    rows = {row["task_id"]: row for row in gold["tasks"]}
    assert [(r["path"], r["local_name"]) for r in rows["same"]["labels"]] == [
        ("target.ts", "hello")
    ]
    assert rows["same"]["label_state"] == "unjudged"
    assert rows["same"]["unsupported"] == [
        {"path": "same.js", "reason": "other_language_matching_declaration"}
    ]
    assert rows["only"]["label_state"] == "mechanical_unreviewed"
    assert rows["only"]["unsupported"] == []
    assert set(gold["census_audits"]) == {"typescript", "javascript"}
    assert rows["typo"]["label_state"] == "unjudged"
    assert rows["typo"]["near_declaration_state"] == "partial"
    assert rows["typo"]["unsupported"] == [
        {"path": "collision.js", "reason": "other_language_matching_declaration"},
        {"path": "near.js", "reason": "other_language_matching_declaration"},
        {"path": "same.js", "reason": "other_language_matching_declaration"},
    ]
    bound = {
        **recipe,
        "checker_identity": {
            language: audit["checker"] for language, audit in gold["census_audits"].items()
        },
    }
    gold_oracle.derive(bound, manifest, view)
    bound["checker_identity"] = {"typescript": gold["census_audits"]["typescript"]["checker"]}
    with pytest.raises(EvidenceError, match="checker identity differs"):
        gold_oracle.derive(bound, manifest, view)


def test_other_language_refusal_remains_unjudged_when_name_may_occur(tmp_path):
    view = tmp_path / "view"
    view.mkdir()
    files = {
        "core.py": b"def target():\n    pass\ndef other():\n    pass\n",
        "broken.rs": b"fn target(\n",
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
    recipe = v2_recipe(
        "holdout",
        b"{}",
        declaration_task("target", "declaration_name_exact", "target", "python"),
        declaration_task("other", "declaration_name_exact", "other", "python"),
    )
    gold, _blind = gold_oracle.derive(recipe, manifest, view)
    target, other = gold["tasks"]
    assert target["label_state"] == "unjudged"
    assert target["unsupported"] == [
        {"path": "broken.rs", "reason": "other_language_possible_declaration"}
    ]
    assert other["label_state"] == "mechanical_unreviewed"
    assert other["unsupported"] == []


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


@pytest.mark.parametrize("query", ["abcde", "abcdef", "abcdefg", "abcdefgh"])
@pytest.mark.parametrize("variant", ["osa1", "osa1_casefold"])
def test_osa1_textual_anchor_keeps_all_one_edit_names(query, variant):
    names = set()
    for index in range(len(query)):
        names.add(query[:index] + query[index + 1 :])
        names.add(query[:index] + "x" + query[index + 1 :])
        if index + 1 < len(query):
            names.add(query[:index] + query[index + 1] + query[index] + query[index + 2 :])
    for index in range(len(query) + 1):
        names.add(query[:index] + "x" + query[index:])
    for name in names:
        raw = b"\xff" + name.encode("ascii") + b"\xfe"
        assert not gold_oracle._textually_excluded(raw, query, variant), (query, name)


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


def test_census_disagreement_requires_query_specific_text_absence(tmp_path, monkeypatch):
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
        declaration_task("other", "declaration_name_exact", "other", "python"),
    )
    gold, _blind = gold_oracle.derive(value, manifest, view)
    target, other = gold["tasks"]
    assert target["labels"][0]["path"] == "core.py"
    assert target["unsupported"] == []
    assert target["census_text_excluded"] == [
        {"path": "disputed.py", "reason": "census_disagreement"}
    ]
    assert target["answerable"] is True
    assert other["unsupported"] == [{"path": "disputed.py", "reason": "census_disagreement"}]
    assert other["census_text_excluded"] == []
    assert other["answerable"] is None
    monkeypatch.setattr(
        gold_oracle,
        "_census_audits",
        lambda _recipe, _sources, _view: {
            "python": {"refused_paths": ["disputed.py"], "disagreement_paths": []}
        },
    )
    gold, _blind = gold_oracle.derive(value, manifest, view)
    target = gold["tasks"][0]
    assert target["unsupported"] == [{"path": "disputed.py", "reason": "census_refused"}]
    assert target["census_text_excluded"] == []
    assert target["answerable"] is None


def test_holdout_sampling_freezes_seeded_ledger_recipes_and_split(split_releases, tmp_path):  # noqa: F811
    from tools.benchmark.retrieval import holdout_sampling

    hold_root, hold_doc = split_releases["hold_only"]
    dev_root, dev_doc = split_releases["dev_only"]
    first = holdout_sampling.build(hold_root, dev_root, 11)
    again = holdout_sampling.build(hold_root, dev_root, 11)
    other = holdout_sampling.build(hold_root, dev_root, 12)
    with pytest.raises(ValueError, match="unknown sampling profile"):
        holdout_sampling.build(hold_root, dev_root, 11, "missing")
    with pytest.raises(ValueError, match="fewer than 1000 admitted tasks"):
        holdout_sampling.build(hold_root, dev_root, 11, "scale_diagnostic_v1")
    assert canonical_json(first[0]) == canonical_json(again[0]) and first[1] == again[1]
    assert first[1] != other[1]
    ledger, recipes, manifest_raw, manifest = first
    recipe = recipes["beta"]
    assert recipe["split"] == "holdout"
    assert recipe["split_manifest_sha256"] == hashlib.sha256(manifest_raw).hexdigest()
    beta = ledger["repositories"]["beta"]
    assert recipe["checker_identity"] == {beta["language"]: beta["census_audit"]["checker"]}
    assert ledger["sampling_version"] == 3
    assert beta["census_audit"]["status"] == "admitted"
    lanes = {task["task_id"].split(".")[1] for task in recipe["tasks"]}
    assert lanes == {"lit", "def", "pre", "inf", "com", "osa"}
    assert beta["exact_definition"]["admitted"] == 12  # worker_0 .. worker_11
    exact_families = {
        row["query_family_id"]
        for row in recipe["tasks"]
        if row["intent"] == "declaration_name_exact"
    }
    typo_tasks = [
        row for row in recipe["tasks"] if row["intent"] == "declaration_name_osa1_casefold"
    ]
    assert typo_tasks and {row["query_family_id"] for row in typo_tasks} <= exact_families
    assert all(row["case_semantics"] == "casefold" for row in typo_tasks)
    assert {
        row["operation"] for row in beta["variant_records"] if row["lane"] == "variant_osa1"
    } == {"insertion", "deletion", "substitution", "transposition"}
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


def test_scale_negative_excludes_literal_path_and_near_content_token(split_releases):  # noqa: F811
    from tools.benchmark.retrieval import holdout_sampling

    release, document = split_releases["hold_only"]
    row = next(row for row in document["repositories"] if row["recipe"]["name"] == "beta")
    repository = holdout_sampling.Repository(release, document, row)
    assert repository.content_absent("worker_O")
    assert not repository.default_file_search_absent("worker_O")  # OSA1 of worker_0
    assert not repository.default_file_search_absent("core.py")  # indexed path
    assert not repository.default_file_search_absent("worker_0")  # indexed content
    assert repository.default_file_search_absent("zzzzzzzzz")


def test_literal_sampling_excludes_queries_outside_product_contract():
    from tools.benchmark.retrieval import holdout_sampling

    repository = SimpleNamespace(
        name="toy",
        files={
            "main.go": (
                b"validIdentifierContent\n"
                b"tab\tinsideIdentifier\n" + "cafe\u0301 IdentifierContent\n".encode("utf-8")
            )
        },
    )
    ledger = {}
    tasks = holdout_sampling._literals(repository, 11, ledger, {"exact_content": 3})
    assert [task["query"] for task in tasks] == ["validIdentifierContent"]
    assert ledger["exact_content"]["skipped"]["outside_literal_query_contract"] == 2
