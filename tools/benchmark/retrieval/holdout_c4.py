"""Admit frozen declaration capsules to a lexical file diagnostic.

This is an adapter over the existing suite, blind pack and source-oracle
contracts. Parser-refused files are excluded only per query when the complete
capsule proves that its raw bytes cannot contain a matching declaration name.
Checker disagreements remain unsupported. Declaration target relevance differs
from default CodeSearch semantics; this is not a qualified default-search score.
"""

from __future__ import annotations

import hashlib
import json
import shutil
import sys
import unicodedata
from pathlib import Path

# corpus_binding still imports corpus_release as a top-level benchmark module.
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from tools.benchmark import corpus_binding
from tools.benchmark.evidence import _read_control_file, digest_bytes, parse_json
from tools.benchmark.retrieval import evaluator, gold_oracle, query_plan, source_oracle


def _read(path: Path) -> dict:
    value = parse_json(_read_control_file(path).decode("utf-8"))
    if not isinstance(value, dict):
        raise ValueError(f"C4 input is not an object: {path}")
    return value


def _raw(value: dict) -> bytes:
    return (json.dumps(value, indent=2, sort_keys=True, ensure_ascii=False) + "\n").encode()


def _default_search_absent(query: str, files: dict[str, bytes]) -> bool:
    """Prove a single term is absent from both default-search surfaces.

    The default route folds Unicode and searches content plus path. A phrase or
    component query has different execution semantics and is not a hard negative.
    """
    if not query.isascii() or any(char.isspace() for char in query):
        return False
    needle = unicodedata.normalize("NFC", query).casefold()
    return all(
        needle not in unicodedata.normalize("NFC", path).casefold()
        and needle not in unicodedata.normalize("NFC", raw.decode("utf-8", "replace")).casefold()
        for path, raw in files.items()
    )


def derive(release: Path, capsule: Path, checkout: Path, intent: str) -> tuple[dict, dict, dict]:
    """Validate capsule/source and derive one diagnostic suite without writing."""
    if intent not in gold_oracle.DECLARATION_INTENTS:
        raise ValueError("C4 requires one supported declaration-name intent")
    identity = _read(capsule / "identity.json")
    release_document_raw = _read_control_file(release / "release.json")
    if corpus_binding.validate_gold(capsule) != identity:
        raise ValueError("C4 source-derived gold identity differs")
    if _read_control_file(release / "release.json") != release_document_raw:
        raise ValueError("C4 release document changed during validation")
    for name in ("selection.json", "recipe.json", "gold.json", "blind.json"):
        if identity.get("files", {}).get(name) != digest_bytes(_read_control_file(capsule / name)):
            raise ValueError(f"C4 capsule identity differs: {name}")
    selection = _read(capsule / "selection.json")
    gold, blind = _read(capsule / "gold.json"), _read(capsule / "blind.json")
    document = parse_json(release_document_raw.decode("utf-8"))
    if not isinstance(document, dict):
        raise ValueError("C4 validated release document is malformed")
    if (
        selection.get("release_path") != str(release.resolve())
        or selection.get("release_digest") != document["digest"]
        or selection.get("view") != "code_only"
        or gold.get("release_digest") != document["digest"]
        or blind.get("release_digest") != document["digest"]
    ):
        raise ValueError("C4 release/capsule identity differs")
    name = selection["repository"]
    if gold.get("repository") != name or blind.get("repository") != name:
        raise ValueError("C4 repository identity differs")
    row = next((r for r in document["repositories"] if r["recipe"]["name"] == name), None)
    if row is None:
        raise ValueError("C4 repository is absent")
    language = row["recipe"]["language"]
    try:
        contract = gold_oracle._name_contract(language, intent)
    except source_oracle.SourceOracleError as exc:
        raise ValueError("C4 has no declaration oracle for selected language") from exc
    audit = gold.get("census_audits", {}).get(language, {})
    refused_paths = audit.get("refused_paths")
    disagreement_paths = audit.get("disagreement_paths")
    if disagreement_paths:
        raise ValueError("C4 independent census has checker disagreement")
    if (
        audit.get("status") not in ("admitted", "unsupported")
        or not isinstance(refused_paths, list)
        or not isinstance(disagreement_paths, list)
        or refused_paths != sorted(set(refused_paths))
        or (audit["status"] == "admitted" and refused_paths)
        or (audit["status"] == "unsupported" and not refused_paths)
    ):
        raise ValueError("C4 independent census is not admitted or query-provable")
    manifest = _read(release / row["views"]["code_only"]["manifest"])
    source = evaluator.SourceSnapshot(checkout, row["recipe"]["revision"])
    universe = manifest["files"]
    evaluator.validate_file_universe(source, universe)
    files = {entry["path"]: source.file(entry["path"])[0] for entry in universe}
    if (
        manifest["repository_commit"] != gold.get("repository_commit")
        or manifest["repository_commit"] != blind.get("repository_commit")
        or gold.get("manifest_digest") != row["views"]["code_only"]["manifest_digest"]
        or blind.get("manifest_digest") != gold.get("manifest_digest")
    ):
        raise ValueError("C4 manifest/commit identity differs")
    gold_tasks, blind_tasks = gold.get("tasks"), blind.get("tasks")
    if not isinstance(gold_tasks, list) or not gold_tasks or not isinstance(blind_tasks, list):
        raise ValueError("C4 task inventory is empty or malformed")
    if len(gold_tasks) != len(blind_tasks):
        raise ValueError("C4 gold/blind task count differs")
    selected, excluded = [], []
    declaration_exclusions: dict[tuple[str, str], set[str]] = {}
    for task, public in zip(gold_tasks, blind_tasks, strict=True):
        if not isinstance(task, dict) or not isinstance(public, dict):
            raise ValueError("C4 task row is malformed")
        blind_fields = gold_oracle.TASK_FIELDS - {"split"}
        if set(public) != blind_fields or any(task.get(k) != public[k] for k in blind_fields):
            raise ValueError("C4 blind query differs from gold intent")
        if task.get("intent") != intent:
            excluded.append({"task_id": task["task_id"], "reason": "outside_selected_intent"})
            continue
        if (
            task.get("label_state") != "mechanical_unreviewed"
            or task.get("unsupported") != []
            or type(task.get("answerable")) is not bool
            or task.get("language") != language
            or task.get("case_semantics") != "sensitive"
            or task.get("normalization") != "none_raw_utf8"
            or task.get("scope_prefix") != ""
        ):
            excluded.append({"task_id": task["task_id"], "reason": "unjudged_or_unsupported"})
            continue
        census_rows = task.get("census_text_excluded")
        if not isinstance(census_rows, list) or any(
            not isinstance(row, dict)
            or set(row) != {"path", "reason"}
            or row["reason"] != "census_refused"
            for row in census_rows
        ):
            excluded.append({"task_id": task["task_id"], "reason": "unproved_census_exclusion"})
            continue
        excluded_paths = sorted(row["path"] for row in census_rows)
        if excluded_paths != refused_paths:
            excluded.append({"task_id": task["task_id"], "reason": "incomplete_census_exclusion"})
            continue
        if not task["answerable"] and not _default_search_absent(task["query"], files):
            excluded.append(
                {"task_id": task["task_id"], "reason": "negative_not_default_search_absent"}
            )
            continue
        try:
            query_plan.plan_lexical_request("code_search_file", task["query"])
        except query_plan.QueryPlanError:
            excluded.append({"task_id": task["task_id"], "reason": "query_not_admitted"})
            continue
        selected.append(task)
        if excluded_paths:
            declaration_exclusions[(contract, task["query"])] = set(excluded_paths)
    if not selected:
        raise ValueError("C4 has no admitted declaration task")
    oracle = source_oracle.SourceOracleIndex(
        {path: (raw, source.file(path)[2]) for path, raw in files.items()},
        {task["query"] for task in selected},
        declaration_exclusions=declaration_exclusions,
    )
    rows = []
    for task in selected:
        observed = {
            (label["path"], label["start_byte"], label["end_byte"]) for label in task["labels"]
        }
        expected = set(oracle.declaration_name_spans(contract, task["query"]))
        if observed != expected or task["answerable"] != bool(expected):
            raise ValueError(f"C4 declaration labels differ from source oracle: {task['task_id']}")
        for label in task["labels"]:
            if label["file_sha256"] != source.file(label["path"])[2]:
                raise ValueError(f"C4 label file hash differs: {task['task_id']}")
        judgments = oracle.expected_rows(contract, task["query"], "distinct_file")
        source_contract = {"contract": contract, "unit": "distinct_file"}
        paths = sorted(declaration_exclusions.get((contract, task["query"]), set()))
        if paths:
            source_contract["declaration_exclusions"] = paths
        rows.append(
            {
                "task_id": task["task_id"],
                "query": task["query"],
                "query_sha256": evaluator.digest(task["query"].encode()),
                "query_family_id": task["query_family_id"],
                "split": "eval",
                "category": task["intent"],
                "query_intent": "bare_symbol",
                "source_oracle": source_contract,
                "judgment_policy": evaluator.SOURCE_ORACLE_JUDGMENT_POLICY,
                "file_judgments": judgments,
                "gold": evaluator.source_oracle_gold(source, oracle, contract, task["query"]),
                "answerable": bool(judgments),
            }
        )
    suite = {
        "schema_version": evaluator.SCHEMA_VERSION,
        "suite_id": f"{name}-{manifest['repository_commit'][:8]}-{intent}-code-search-file",
        "repository_commit": manifest["repository_commit"],
        "comparison_contract": {
            "top_k": 10,
            "tokenizer": evaluator.TOKENIZER,
            "tokenizer_budget_version": evaluator.TOKENIZER_BUDGET_VERSION,
            "output_unit_policy": "rank_prefix",
            "span_unit": evaluator.SPAN_UNIT,
        },
        "routes": ["lexical", "semble-lexical-file"],
        "file_universe": universe,
        "file_universe_digest": evaluator.universe_digest(universe),
        "diagnostic_policy": evaluator.OBSERVED_PREFIX_DIAGNOSTIC_POLICY,
        "tasks": rows,
    }
    checked, pack, _ = evaluator.validate_suite(checkout, suite)
    selection_binding = {
        "release_path": str(release.resolve()),
        "release_digest": document["digest"],
        "repository": name,
        "intent": intent,
        "view": "code_only",
    }
    binding = corpus_binding._bind(
        document,
        _read_control_file(release / row["views"]["code_only"]["manifest"]),
        selection_binding,
        evaluator.canonical(checked),
        evaluator.canonical(pack),
    )
    return (
        checked,
        pack,
        {
            "status": "diagnostic_unqualified",
            "repository": name,
            "selected": len(selected),
            "excluded": excluded,
            "parser_refused_source_paths": refused_paths,
            "selected_tasks_with_proved_exclusions": len(declaration_exclusions),
            "relevance_contract": contract,
            "execution_policy": "code_search_file",
            "semantic_relation": "declaration_target_file_diagnostic_only",
            "case_semantics_equivalent": False,
            "negative_control": "single_term_absent_from_folded_content_and_path",
            "qualified_default_search_conformance": False,
            "gold_capsule_identity_sha256": hashlib.sha256(
                _read_control_file(capsule / "identity.json")
            ).hexdigest(),
            "binding": binding,
        },
    )


def write(release: Path, capsule: Path, checkout: Path, intent: str, output: Path) -> dict:
    """Write to a fresh root; remove partial files on failure.

    Publication is not atomic: concurrent readers must wait for a complete
    admission.json and verify its suite and blind-pack commitments.
    """
    if not output.is_absolute() or output.exists() or output.is_symlink():
        raise ValueError("C4 output root must be fresh and absolute")
    target = output.resolve()
    protected = (Path(__file__).resolve().parents[3], release, capsule, checkout)
    if any(
        target.is_relative_to(root.resolve()) or root.resolve().is_relative_to(target)
        for root in protected
    ):
        raise ValueError("C4 output root must be external and disjoint")
    suite, pack, report = derive(release, capsule, checkout, intent)
    # mkdir refuses an existing root, including one created during derive().
    # Only the root created by this call is removed after a write failure.
    output.mkdir(parents=True)
    try:
        for name, payload in (
            ("suite.json", suite),
            ("blind-pack.json", pack),
            ("admission.json", report),
        ):
            (output / name).write_bytes(_raw(payload))
    except BaseException:
        shutil.rmtree(output)
        raise
    return report
