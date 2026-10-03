"""Admit frozen declaration capsules to a lexical file diagnostic.

This is an adapter over the existing suite, blind pack and source-oracle
contracts. Parser-refused files are excluded only per query when the complete
capsule proves that its raw bytes cannot contain a matching declaration name.
Checker disagreements remain unsupported. Declaration target relevance differs
from default CodeSearch semantics; this is not a qualified default-search score.
"""

from __future__ import annotations

import argparse
import copy
import hashlib
import json
import shutil
import sys
from dataclasses import dataclass
from pathlib import Path

# Support direct execution from an external working directory. corpus_binding
# also imports corpus_release as a top-level benchmark module.
for path in (Path(__file__).resolve().parents[3], Path(__file__).resolve().parents[1]):
    if str(path) not in sys.path:
        sys.path.insert(0, str(path))

from tools.benchmark import corpus_binding  # noqa: E402
from tools.benchmark.evidence import _read_control_file, digest_bytes, parse_json  # noqa: E402
from tools.benchmark.retrieval import (  # noqa: E402
    evaluator,
    gold_oracle,
    query_plan,
    source_oracle,
)

MATRIX_INTENTS = tuple(sorted(gold_oracle.DECLARATION_INTENTS))


def _execution_policy(intent: str) -> str:
    """Only intended-name typo gold is eligible for the explicit typo mode."""
    return (
        "code_search_typo_file"
        if intent == "declaration_name_osa1_casefold"
        else "code_search_components_file"
        if intent == "declaration_name_components"
        else "code_search_file"
    )


@dataclass(frozen=True)
class _Prepared:
    release: Path
    capsule: Path
    checkout: Path
    identity_sha256: str
    identity: dict
    document: dict
    selection: dict
    gold: dict
    blind: dict
    row: dict
    manifest: dict
    manifest_raw: bytes
    source: evaluator.SourceSnapshot
    files: dict[str, bytes]


@dataclass(frozen=True)
class _Batch:
    release: Path
    capsule_root: Path
    checkout_root: Path
    document_raw: bytes
    document: dict
    names: tuple[str, ...]
    split_raw: bytes
    split_releases_raw: bytes
    releases: dict[str, Path]
    split_manifest: dict
    documents: dict[str, dict]

    @property
    def verified_split(self) -> tuple[bytes, dict[str, Path], dict, dict[str, dict]]:
        return self.split_raw, self.releases, self.split_manifest, self.documents


def _read(path: Path) -> dict:
    raw = (
        corpus_binding._read_gold_capsule_file(path)
        if path.name == "gold.json"
        else _read_control_file(path)
    )
    value = parse_json(raw.decode("utf-8"))
    if not isinstance(value, dict):
        raise ValueError(f"C4 input is not an object: {path}")
    return value


def _raw(value: dict) -> bytes:
    return (json.dumps(value, indent=2, sort_keys=True, ensure_ascii=False) + "\n").encode()


def _prepare(
    release: Path,
    capsule: Path,
    checkout: Path,
    *,
    verified_split: tuple[bytes, dict[str, Path], dict, dict[str, dict]] | None = None,
) -> _Prepared:
    """Validate one capsule and source for this invocation only."""
    identity = _read(capsule / "identity.json")
    identity_raw = _read_control_file(capsule / "identity.json")
    release_document_raw = _read_control_file(release / "release.json")
    if verified_split is None:
        validated_identity = corpus_binding.validate_gold(capsule)
    else:
        split_raw, releases, manifest, documents = verified_split
        names = corpus_binding.corpus.regular_tree(capsule)
        if names != {
            "selection.json",
            "recipe.json",
            "gold.json",
            "blind.json",
            "identity.json",
            "split-manifest.json",
            "split-releases.json",
        }:
            raise ValueError("C4 matrix capsule inventory differs")
        if _read_control_file(capsule / "split-manifest.json") != split_raw:
            raise ValueError("C4 matrix split manifest differs")
        selection = _read(capsule / "selection.json")
        digest = selection.get("release_digest")
        if digest not in documents or releases[digest] != release:
            raise ValueError("C4 matrix capsule selects another release")
        material = corpus_binding._gold_material(
            release,
            selection,
            _read_control_file(capsule / "recipe.json"),
            (split_raw, releases),
            validated_manifest=manifest,
            validated_document=documents[digest],
        )
        validated_identity = corpus_binding._verify_gold_material(capsule, material, names)
    if validated_identity != identity:
        raise ValueError("C4 source-derived gold identity differs")
    if _read_control_file(release / "release.json") != release_document_raw:
        raise ValueError("C4 release document changed during validation")
    for name in ("selection.json", "recipe.json", "gold.json", "blind.json"):
        if identity.get("files", {}).get(name) != digest_bytes(
            corpus_binding._read_gold_capsule_file(capsule / name)
        ):
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
    manifest_raw = _read_control_file(release / row["views"]["code_only"]["manifest"])
    manifest = parse_json(manifest_raw.decode("utf-8"))
    if not isinstance(manifest, dict):
        raise ValueError("C4 manifest is malformed")
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
    return _Prepared(
        release,
        capsule,
        checkout,
        hashlib.sha256(identity_raw).hexdigest(),
        identity,
        document,
        selection,
        gold,
        blind,
        row,
        manifest,
        manifest_raw,
        source,
        files,
    )


def _derive_prepared(
    prepared: _Prepared,
    intent: str,
    *,
    allow_empty: bool = False,
    filter_query_duplicates: bool = False,
) -> tuple[dict | None, dict | None, dict]:
    if intent not in gold_oracle.DECLARATION_INTENTS:
        raise ValueError("C4 requires one supported declaration-name intent")
    release, checkout = prepared.release, prepared.checkout
    document, selection, gold, blind = (
        prepared.document,
        prepared.selection,
        prepared.gold,
        prepared.blind,
    )
    row, manifest, source, files = prepared.row, prepared.manifest, prepared.source, prepared.files
    universe = manifest["files"]
    name = selection["repository"]
    language = row["recipe"]["language"]
    execution_policy = _execution_policy(intent)
    intended_typo = intent == "declaration_name_osa1_casefold"
    try:
        contract = gold_oracle._name_contract(
            source_oracle.ALL_DECLARATION_LANGUAGES,
            "declaration_name_exact" if intended_typo else intent,
        )
    except source_oracle.SourceOracleError as exc:
        raise ValueError("C4 has no declaration oracle for selected language") from exc
    audits = gold.get("census_audits", {})
    languages = {
        source_language
        for path in files
        if (source_language := source_oracle.declaration_language(path)) is not None
    }
    if set(audits) != languages:
        raise ValueError("C4 independent census language inventory differs from source")
    refused_paths, disagreement_paths = [], []
    for source_language, audit in audits.items():
        refused = audit.get("refused_paths")
        disagreements = audit.get("disagreement_paths")
        if (
            audit.get("status") not in ("admitted", "unsupported")
            or not isinstance(refused, list)
            or not isinstance(disagreements, list)
            or refused != sorted(set(refused))
            or disagreements != sorted(set(disagreements))
            or set(refused) & set(disagreements)
            or any(
                source_oracle.declaration_language(path) != source_language
                for path in (*refused, *disagreements)
            )
            or (audit["status"] == "admitted" and (refused or disagreements))
            or (audit["status"] == "unsupported" and not (refused or disagreements))
        ):
            raise ValueError("C4 independent census is not admitted or query-provable")
        refused_paths.extend(refused)
        disagreement_paths.extend(disagreements)
    required_rows = sorted(
        [(path, "census_refused") for path in refused_paths]
        + [(path, "census_disagreement") for path in disagreement_paths]
    )
    # The independent checker can refuse a file that the primary parser
    # censuses completely. Only primary parser refusals need an oracle
    # exclusion; checker-only refusals remain visible in required_rows.
    parser_refused_paths = set()
    for path in refused_paths:
        if path not in files:
            raise ValueError("C4 refused declaration file is outside the universe")
        try:
            source_oracle.declaration_census(
                source_oracle.declaration_language(path), path, files[path]
            )
        except source_oracle.SourceOracleError as exc:
            if not source_oracle._excludable_census_refusal(exc):
                raise ValueError("C4 primary declaration census is unavailable") from exc
            parser_refused_paths.add(path)
    gold_tasks, blind_tasks = gold["tasks"], blind["tasks"]
    selected, excluded = [], []
    declaration_exclusions: dict[tuple[str, str], set[str]] = {}
    proved_exclusion_tasks = 0
    negative_queries = {
        task["query"]
        for task in gold_tasks
        if isinstance(task, dict)
        and task.get("intent") == intent
        and task.get("answerable") is False
        and isinstance(task.get("query"), str)
        and source_oracle.IDENTIFIER.fullmatch(task["query"])
    }
    absence_oracle = (
        source_oracle.SourceOracleIndex(
            {path: (raw, source.file(path)[2]) for path, raw in files.items()},
            negative_queries,
        )
        if negative_queries
        else None
    )
    typo_names = {
        name
        for task in gold_tasks
        if intended_typo and isinstance(task, dict) and task.get("intent") == intent
        for name in (task.get("query"), task.get("intended_name"))
        if isinstance(name, str)
    }
    typo_exclusions: dict[tuple[str, str], set[str]] = {}
    if intended_typo:
        near_contract = gold_oracle._name_contract(source_oracle.ALL_DECLARATION_LANGUAGES, intent)
        for task in gold_tasks:
            if (
                not isinstance(task, dict)
                or task.get("intent") != intent
                or task.get("label_state") != "mechanical_unreviewed"
                or task.get("near_declaration_state") != "complete"
            ):
                continue
            near_rows = task.get("near_census_text_excluded", [])
            source_rows = task.get("census_text_excluded", [])
            if not isinstance(near_rows, list) or not isinstance(source_rows, list):
                raise ValueError("C4 typo census exclusion rows are malformed")
            expected = {
                (row["path"], row["reason"])
                for row in source_rows
                if isinstance(row, dict) and set(row) == {"path", "reason"}
            }
            observed = {
                (row["path"], row["reason"])
                for row in near_rows
                if isinstance(row, dict) and set(row) == {"path", "reason"}
            }
            if (
                len(expected) != len(source_rows)
                or len(observed) != len(near_rows)
                or expected != observed
                or not observed <= set(required_rows)
                or any(path not in files for path, _reason in observed)
            ):
                raise ValueError("C4 typo near-census exclusion differs")
            for path, reason in observed:
                if not source_oracle.declaration_query_textually_excluded(
                    files[path], task["query"], "osa1_casefold"
                ):
                    raise ValueError("C4 typo near-census exclusion is not query-proven")
                if path in parser_refused_paths and reason == "census_refused":
                    typo_exclusions.setdefault((near_contract, task["query"]), set()).add(path)
            for path, reason in expected:
                if path in parser_refused_paths and reason == "census_refused":
                    typo_exclusions.setdefault((contract, task["intended_name"]), set()).add(path)
    typo_oracle = (
        source_oracle.SourceOracleIndex(
            {path: (raw, source.file(path)[2]) for path, raw in files.items()},
            typo_names,
            declaration_exclusions=typo_exclusions,
        )
        if typo_names
        else None
    )
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
            or task.get("case_semantics")
            != ("casefold" if intent in gold_oracle.CASEFOLD_INTENTS else "sensitive")
            or task.get("normalization") != "none_raw_utf8"
            or task.get("scope_prefix") != ""
        ):
            excluded.append({"task_id": task["task_id"], "reason": "unjudged_or_unsupported"})
            continue
        if intended_typo:
            if not task["answerable"]:
                excluded.append({"task_id": task["task_id"], "reason": "typo_no_answer_unjudged"})
                continue
            if task.get("near_declaration_state") != "complete":
                excluded.append(
                    {"task_id": task["task_id"], "reason": "incomplete_near_declaration_census"}
                )
                continue
            try:
                partition = typo_oracle.typo_gold_partition(
                    source_oracle.ALL_DECLARATION_LANGUAGES, task["query"], task["intended_name"]
                )
            except source_oracle.SourceOracleError as exc:
                raise ValueError("C4 typo target differs from source oracle") from exc
            if (
                task.get("near_declaration_names") != partition["near_declaration_names"]
                or task.get("near_declaration_files") != partition["near_declaration_files"]
            ):
                raise ValueError("C4 typo ambiguity metadata differs from source oracle")
            if (
                task.get("near_declaration_state") != "complete"
                or partition["other_near_declaration_names"]
                or partition["exact_content_collision_paths"]
                or partition["query_is_declaration_name"]
                or task.get("exact_collision_names") != []
            ):
                excluded.append({"task_id": task["task_id"], "reason": "ambiguous_typo_target"})
                continue
        census_rows = task.get("census_text_excluded")
        if not isinstance(census_rows, list) or any(
            not isinstance(row, dict)
            or set(row) != {"path", "reason"}
            or row["reason"] not in ("census_refused", "census_disagreement")
            for row in census_rows
        ):
            excluded.append({"task_id": task["task_id"], "reason": "unproved_census_exclusion"})
            continue
        excluded_rows = sorted((row["path"], row["reason"]) for row in census_rows)
        if excluded_rows != required_rows:
            excluded.append({"task_id": task["task_id"], "reason": "incomplete_census_exclusion"})
            continue
        refused_selected_paths = [
            path
            for path, reason in excluded_rows
            if reason == "census_refused" and path in parser_refused_paths
        ]
        if not task["answerable"]:
            try:
                if absence_oracle is None:
                    raise source_oracle.SourceOracleError("negative query is not an identifier")
                absence_oracle.expected_rows(
                    source_oracle.ASCII_CODE_SEARCH_DEFAULT_ABSENT_CASEFOLD,
                    task["query"],
                    "distinct_file",
                )
            except source_oracle.SourceOracleError:
                excluded.append(
                    {"task_id": task["task_id"], "reason": "negative_not_default_search_absent"}
                )
                continue
        try:
            query_plan.plan_lexical_request(execution_policy, task["query"])
        except query_plan.QueryPlanError:
            excluded.append({"task_id": task["task_id"], "reason": "query_not_admitted"})
            continue
        if filter_query_duplicates:
            try:
                # Selected queries have already passed pairwise admission.
                # Rechecking every old pair for each new task is cubic.
                for prior in selected:
                    evaluator.check_query_near_duplicates(
                        [
                            (prior["task_id"], prior["query"]),
                            (task["task_id"], task["query"]),
                        ]
                    )
            except evaluator.EvidenceError:
                excluded.append({"task_id": task["task_id"], "reason": "query_near_duplicate"})
                continue
        selected.append(task)
        if excluded_rows:
            proved_exclusion_tasks += 1
        if refused_selected_paths:
            declaration_exclusions[
                (contract, task["intended_name"] if intended_typo else task["query"])
            ] = set(refused_selected_paths)
    if not selected and not allow_empty:
        raise ValueError("C4 has no admitted declaration task")
    if not selected:
        return (
            None,
            None,
            {
                "status": "no_admission_diagnostic",
                "repository": name,
                "intent": intent,
                "selected": 0,
                "excluded": excluded,
                "reason": "no_tasks_for_intent"
                if not any(task["intent"] == intent for task in gold_tasks)
                else "all_tasks_excluded",
                "gold_capsule_identity_sha256": prepared.identity_sha256,
                "release_digest": document["digest"],
                "repository_commit": manifest["repository_commit"],
            },
        )
    oracle = source_oracle.SourceOracleIndex(
        {path: (raw, source.file(path)[2]) for path, raw in files.items()},
        {
            name
            for task in selected
            for name in (
                (task["query"], task["intended_name"]) if intended_typo else (task["query"],)
            )
        },
        declaration_exclusions=declaration_exclusions,
    )
    rows = []
    for task in selected:
        oracle_query = task["intended_name"] if intended_typo else task["query"]
        observed = {
            (label["path"], label["start_byte"], label["end_byte"]) for label in task["labels"]
        }
        expected = set(oracle.declaration_name_spans(contract, oracle_query))
        if observed != expected or task["answerable"] != bool(expected):
            raise ValueError(f"C4 declaration labels differ from source oracle: {task['task_id']}")
        for label in task["labels"]:
            if label["file_sha256"] != source.file(label["path"])[2]:
                raise ValueError(f"C4 label file hash differs: {task['task_id']}")
        score_contract = (
            contract
            if task["answerable"]
            else source_oracle.ASCII_CODE_SEARCH_DEFAULT_ABSENT_CASEFOLD
        )
        judgments = oracle.expected_rows(score_contract, oracle_query, "distinct_file")
        source_contract = {"contract": score_contract, "unit": "distinct_file"}
        paths = sorted(declaration_exclusions.get((contract, oracle_query), set()))
        if paths and task["answerable"]:
            source_contract["declaration_exclusions"] = paths
        if intended_typo:
            near_paths = sorted(
                row["path"]
                for row in task.get("near_census_text_excluded", [])
                if row["reason"] == "census_refused" and row["path"] in parser_refused_paths
            )
            if near_paths:
                source_contract["near_declaration_exclusions"] = near_paths
        row = {
            "task_id": task["task_id"],
            "query": task["query"],
            "query_sha256": evaluator.digest(task["query"].encode()),
            "query_family_id": task["query_family_id"],
            "split": "eval",
            "category": task["intent"],
            "query_intent": (
                "symbol_components" if intent == "declaration_name_components" else "bare_symbol"
            ),
            "evaluation_contract": {
                "request_mode": (
                    query_plan.EXPLICIT_OSA1_TYPO
                    if intended_typo
                    else query_plan.EXPLICIT_SYMBOL_COMPONENTS
                    if intent == "declaration_name_components"
                    else query_plan.DEFAULT_FILE_SEARCH
                ),
                "gold_unit": "distinct_file",
                "result_unit": "distinct_file",
            },
            "source_oracle": source_contract,
            "judgment_policy": evaluator.SOURCE_ORACLE_JUDGMENT_POLICY,
            "file_judgments": judgments,
            "gold": evaluator.source_oracle_gold(source, oracle, score_contract, oracle_query),
            "answerable": bool(judgments),
        }
        if intended_typo and task["answerable"]:
            row["intended_name"] = oracle_query
        rows.append(row)
    suite = {
        "schema_version": evaluator.SCHEMA_VERSION,
        "suite_id": f"{name}-{manifest['repository_commit'][:8]}-{intent}-{execution_policy.replace('_', '-')}",
        "repository_commit": manifest["repository_commit"],
        "comparison_contract": {
            "top_k": 10,
            "tokenizer": evaluator.TOKENIZER,
            "tokenizer_budget_version": evaluator.TOKENIZER_BUDGET_VERSION,
            "output_unit_policy": "rank_prefix",
            "span_unit": evaluator.SPAN_UNIT,
        },
        "routes": (
            ["lexical"]
            if intent in ("declaration_name_components", "declaration_name_osa1_casefold")
            else ["lexical", "semble-lexical-file"]
        ),
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
        prepared.manifest_raw,
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
            "census_excluded_source_paths": [
                {"path": path, "reason": reason} for path, reason in required_rows
            ],
            "selected_tasks_with_proved_exclusions": proved_exclusion_tasks,
            "relevance_contract": contract,
            "execution_policy": execution_policy,
            "semantic_relation": "declaration_target_file_diagnostic_only",
            "case_semantics_equivalent": False,
            "negative_control": (
                "excluded_unjudged"
                if intended_typo
                else "single_term_absent_from_folded_content_and_path"
            ),
            "qualified_default_search_conformance": False,
            "gold_capsule_identity_sha256": prepared.identity_sha256,
            "binding": binding,
        },
    )


def derive(release: Path, capsule: Path, checkout: Path, intent: str) -> tuple[dict, dict, dict]:
    """Validate capsule/source and derive one diagnostic suite without writing."""
    corpus_binding.require_gold_runtime()
    if intent not in gold_oracle.DECLARATION_INTENTS:
        raise ValueError("C4 requires one supported declaration-name intent")
    suite, pack, report = _derive_prepared(_prepare(release, capsule, checkout), intent)
    assert suite is not None and pack is not None
    return suite, pack, report


def project_fixed_cohort(
    checkout: Path,
    fresh_suite: dict,
    legacy_suite_raw: bytes,
    legacy_suite_sha256: str,
    *,
    suite_id: str,
    default_file_typo: bool = False,
) -> tuple[dict, dict, dict]:
    """Project a current C4 suite onto an SHA-pinned earlier task cohort.

    The legacy suite is a selector only: its old source-oracle annotations are
    never copied into the result. Every selected query and relevance field must
    agree with the newly validated source-derived suite. This deliberately
    creates a new suite and blind pack; old product records cannot be rebound.
    """
    if (
        not isinstance(legacy_suite_raw, bytes)
        or not isinstance(legacy_suite_sha256, str)
        or len(legacy_suite_sha256) != 64
        or hashlib.sha256(legacy_suite_raw).hexdigest() != legacy_suite_sha256
    ):
        raise ValueError("C4 legacy suite byte commitment differs")
    if type(default_file_typo) is not bool:
        raise ValueError("C4 default_file_typo must be boolean")
    legacy = parse_json(legacy_suite_raw.decode("utf-8"))
    if not isinstance(legacy, dict) or not isinstance(legacy.get("tasks"), list):
        raise ValueError("C4 legacy suite selector is malformed")
    checked, _full_pack, _ = evaluator.validate_suite(checkout, fresh_suite)
    if checked != fresh_suite:
        raise ValueError("C4 fresh suite differs after validation")
    for field in (
        "schema_version",
        "repository_commit",
        "comparison_contract",
        "file_universe",
        "file_universe_digest",
        "diagnostic_policy",
        "leakage_allowlist",
    ):
        if legacy.get(field) != checked.get(field):
            raise ValueError(f"C4 fixed-cohort source or contract differs: {field}")
    if (
        not isinstance(suite_id, str)
        or not suite_id
        or suite_id
        in (
            legacy.get("suite_id"),
            checked["suite_id"],
        )
    ):
        raise ValueError("C4 fixed-cohort requires a new suite_id")
    routes = legacy.get("routes")
    if (
        not isinstance(routes, list)
        or not routes
        or any(not isinstance(route, str) for route in routes)
        or len(set(routes)) != len(routes)
    ):
        raise ValueError("C4 legacy routes are malformed")
    fresh_rows = checked["tasks"]
    fresh_by_id = {row["task_id"]: row for row in fresh_rows}
    legacy_rows = legacy["tasks"]
    legacy_ids = [row.get("task_id") for row in legacy_rows if isinstance(row, dict)]
    valid_ids = len(legacy_ids) == len(legacy_rows) and all(
        isinstance(task_id, str) and task_id for task_id in legacy_ids
    )
    selected_ids = set(legacy_ids) if valid_ids else set()
    if (
        not legacy_ids
        or not valid_ids
        or len(selected_ids) != len(legacy_ids)
        or legacy_ids != [row["task_id"] for row in fresh_rows if row["task_id"] in selected_ids]
    ):
        raise ValueError("C4 legacy task cohort is unknown, duplicated, or reordered")
    for old in legacy_rows:
        fresh = fresh_by_id[old["task_id"]]
        old_truth = copy.deepcopy(
            {key: value for key, value in old.items() if key != "source_oracle"}
        )
        fresh_truth = {key: value for key, value in fresh.items() if key != "source_oracle"}
        if default_file_typo:
            if (
                old.get("category") != "declaration_name_osa1_casefold"
                or old.get("evaluation_contract", {}).get("request_mode")
                != query_plan.DEFAULT_FILE_SEARCH
                or fresh.get("evaluation_contract", {}).get("request_mode")
                != query_plan.EXPLICIT_OSA1_TYPO
            ):
                raise ValueError("C4 legacy ordinary-file typo mode differs")
            old_truth["evaluation_contract"]["request_mode"] = query_plan.EXPLICIT_OSA1_TYPO
        if old_truth != fresh_truth:
            raise ValueError(f"C4 selected query or truth changed: {old['task_id']}")
        old_oracle, fresh_oracle = old.get("source_oracle"), fresh.get("source_oracle")
        if (
            not isinstance(old_oracle, dict)
            or not isinstance(fresh_oracle, dict)
            or {key: old_oracle.get(key) for key in ("contract", "unit")}
            != {key: fresh_oracle.get(key) for key in ("contract", "unit")}
            or set(old_oracle)
            - {"contract", "unit", "declaration_exclusions", "near_declaration_exclusions"}
        ):
            raise ValueError(f"C4 selected source-oracle contract changed: {old['task_id']}")
    selected = [copy.deepcopy(fresh_by_id[task_id]) for task_id in legacy_ids]
    if default_file_typo:
        if any(
            row.get("category") != "declaration_name_osa1_casefold"
            or row.get("evaluation_contract", {}).get("request_mode")
            != query_plan.EXPLICIT_OSA1_TYPO
            for row in selected
        ):
            raise ValueError("C4 ordinary-file projection requires explicit OSA1 tasks")
        for row in selected:
            row["evaluation_contract"]["request_mode"] = query_plan.DEFAULT_FILE_SEARCH
    projected = copy.deepcopy(checked)
    projected["suite_id"] = suite_id
    projected["routes"] = routes
    projected["tasks"] = selected
    validated, pack, _ = evaluator.validate_suite(checkout, projected)
    if validated != projected or [row["task_id"] for row in pack["tasks"]] != legacy_ids:
        raise ValueError("C4 fixed-cohort projection did not validate")
    lineage = {
        "status": "diagnostic_unqualified",
        "legacy_suite_sha256": legacy_suite_sha256,
        "fresh_suite_sha256": hashlib.sha256(evaluator.canonical(checked)).hexdigest(),
        "projected_suite_sha256": hashlib.sha256(evaluator.canonical(validated)).hexdigest(),
        "blind_pack_sha256": hashlib.sha256(evaluator.canonical(pack)).hexdigest(),
        "fresh_selected": len(fresh_rows),
        "fixed_selected": len(selected),
        "selected_task_ids": legacy_ids,
        "excluded_fresh_task_ids": [
            row["task_id"] for row in fresh_rows if row["task_id"] not in selected_ids
        ],
        "default_file_typo": default_file_typo,
    }
    return validated, pack, lineage


def project_ordinary_file_osa1(
    checkout: Path, fresh_suite: dict, *, suite_id: str
) -> tuple[dict, dict, dict]:
    """Submit every fresh OSA1 task to ordinary file search without changing gold."""
    checked, _original_pack, _ = evaluator.validate_suite(checkout, fresh_suite)
    if checked != fresh_suite or checked["routes"] != ["lexical"]:
        raise ValueError("C4 ordinary-file input must be a validated lexical OSA1 suite")
    if not isinstance(suite_id, str) or not suite_id or suite_id == checked["suite_id"]:
        raise ValueError("C4 ordinary-file projection requires a new suite_id")
    if any(
        task.get("category") != "declaration_name_osa1_casefold"
        or task.get("evaluation_contract", {}).get("request_mode") != query_plan.EXPLICIT_OSA1_TYPO
        for task in checked["tasks"]
    ):
        raise ValueError("C4 ordinary-file projection requires explicit OSA1 tasks")
    projected = copy.deepcopy(checked)
    projected["suite_id"] = suite_id
    projected["routes"] = ["lexical", "semble-lexical-file"]
    for task in projected["tasks"]:
        task["evaluation_contract"]["request_mode"] = query_plan.DEFAULT_FILE_SEARCH
    validated, pack, _ = evaluator.validate_suite(checkout, projected)
    if validated != projected or [row["task_id"] for row in pack["tasks"]] != [
        row["task_id"] for row in checked["tasks"]
    ]:
        raise ValueError("C4 ordinary-file projection did not validate")
    lineage = {
        "status": "diagnostic_unqualified",
        "fresh_suite_sha256": hashlib.sha256(evaluator.canonical(checked)).hexdigest(),
        "projected_suite_sha256": hashlib.sha256(evaluator.canonical(validated)).hexdigest(),
        "blind_pack_sha256": hashlib.sha256(evaluator.canonical(pack)).hexdigest(),
        "selected_task_ids": [row["task_id"] for row in checked["tasks"]],
        "request_mode_from": query_plan.EXPLICIT_OSA1_TYPO,
        "request_mode_to": query_plan.DEFAULT_FILE_SEARCH,
        "relevance_unchanged": True,
    }
    return validated, pack, lineage


def _batch_preflight(
    release: Path, capsule_root: Path, checkout_root: Path, expected_repositories: int
) -> _Batch:
    """Validate the shared release roster and global split once per batch."""
    corpus_binding.require_gold_runtime()
    document_raw = _read_control_file(release / "release.json")
    document = parse_json(document_raw.decode("utf-8"))
    if not isinstance(document, dict) or not isinstance(document.get("repositories"), list):
        raise ValueError("C4 matrix release is malformed")
    names = [row["recipe"]["name"] for row in document["repositories"]]
    if (
        type(expected_repositories) is not int
        or expected_repositories < 1
        or len(names) != expected_repositories
        or len(set(names)) != len(names)
    ):
        raise ValueError("C4 matrix repository roster differs from expected count")
    capsule_names = {path.name for path in capsule_root.iterdir() if path.is_dir()}
    if capsule_names != set(names) or any(path.is_symlink() for path in capsule_root.iterdir()):
        raise ValueError("C4 matrix capsule roster differs from release")
    # A source mismatch makes every regenerated gold byte ineligible. Check all
    # capsule identities before the expensive corpus-wide split replay; the
    # full source-derived capsule validation below remains the authority.
    producer_sources = corpus_binding._gold_producer_source_digests()
    for name in names:
        identity = _read(capsule_root / name / "identity.json")
        if identity.get("producer_source_digests") != producer_sources:
            raise ValueError(f"C4 gold producer source differs: {name}")
    first = capsule_root / min(names)
    split_raw = _read_control_file(first / "split-manifest.json")
    split_releases_raw = _read_control_file(first / "split-releases.json")
    parsed_releases = parse_json(split_releases_raw.decode("utf-8"))
    if not isinstance(parsed_releases, dict):
        raise ValueError("C4 matrix split releases are malformed")
    releases = corpus_binding._split_releases(parsed_releases)
    split_manifest, documents = corpus_binding._validated_split_manifest(split_raw, releases)
    if releases.get(document["digest"]) != release or documents[document["digest"]] != document:
        raise ValueError("C4 matrix validated release differs")
    return _Batch(
        release,
        capsule_root,
        checkout_root,
        document_raw,
        document,
        tuple(sorted(names)),
        split_raw,
        split_releases_raw,
        releases,
        split_manifest,
        documents,
    )


def _batch_prepare(batch: _Batch, name: str) -> _Prepared:
    if name not in batch.names:
        raise ValueError("C4 matrix capsule is outside release roster")
    capsule = batch.capsule_root / name
    if (
        _read_control_file(capsule / "split-manifest.json") != batch.split_raw
        or _read_control_file(capsule / "split-releases.json") != batch.split_releases_raw
    ):
        raise ValueError("C4 matrix capsule split differs")
    prepared = _prepare(
        batch.release,
        capsule,
        batch.checkout_root / name,
        verified_split=batch.verified_split,
    )
    if prepared.selection["repository"] != name:
        raise ValueError("C4 matrix capsule/repository mismatch")
    return prepared


def _batch_recheck(
    batch: _Batch,
    prepared_rows: list[_Prepared],
    tool_sources: dict[str, Path],
    source_digests: dict[str, str],
) -> None:
    """Reopen cached source and every input before publishing batch artifacts."""
    if len(prepared_rows) != len(batch.names) or {
        row.selection["repository"] for row in prepared_rows
    } != set(batch.names):
        raise ValueError("C4 matrix prepared repository roster differs")
    for prepared in prepared_rows:
        fresh = evaluator.SourceSnapshot(
            batch.checkout_root / prepared.selection["repository"],
            prepared.manifest["repository_commit"],
        )
        evaluator.validate_file_universe(fresh, prepared.manifest["files"])
        if (
            hashlib.sha256(_read_control_file(prepared.capsule / "identity.json")).hexdigest()
            != prepared.identity_sha256
            or _read_control_file(prepared.capsule / "split-manifest.json") != batch.split_raw
            or _read_control_file(prepared.capsule / "split-releases.json")
            != batch.split_releases_raw
            or _read_control_file(batch.release / prepared.row["views"]["code_only"]["manifest"])
            != prepared.manifest_raw
        ):
            raise ValueError("C4 matrix input changed during admission")
        for file_name in ("selection.json", "recipe.json", "gold.json", "blind.json"):
            if prepared.identity["files"][file_name] != digest_bytes(
                corpus_binding._read_gold_capsule_file(prepared.capsule / file_name)
            ):
                raise ValueError("C4 matrix capsule changed during admission")
    if (
        _read_control_file(batch.release / "release.json") != batch.document_raw
        or corpus_binding.validate_split_manifest(batch.split_raw, batch.releases)
        != batch.split_manifest
        or any(
            digest_bytes(path.read_bytes()) != source_digests[name]
            for name, path in tool_sources.items()
        )
    ):
        raise ValueError("C4 matrix split or tool source changed during admission")


def _batch_tool_sources() -> dict[str, Path]:
    """Bind the batch adapter, source oracles, and release validator together."""
    return {
        "holdout_c4": Path(__file__),
        "gold_oracle": Path(gold_oracle.__file__),
        "source_oracle": Path(source_oracle.__file__),
        "declaration_census_audit": Path(gold_oracle.declaration_census_audit.__file__),
        "query_plan": Path(query_plan.__file__),
        "evaluator": Path(evaluator.__file__),
        "corpus_binding": Path(corpus_binding.__file__),
        "corpus_release": Path(corpus_binding.corpus.__file__),
    }


def derive_matrix(
    release: Path,
    capsule_root: Path,
    checkout_root: Path,
    *,
    expected_repositories: int = 12,
    _payloads: dict[tuple[str, str], tuple[dict, dict, dict]] | None = None,
) -> dict:
    """Admit all declaration intents without publishing suites or product scores."""
    tool_sources = _batch_tool_sources()
    source_digests = {name: digest_bytes(path.read_bytes()) for name, path in tool_sources.items()}
    batch = _batch_preflight(release, capsule_root, checkout_root, expected_repositories)
    cells = []
    prepared_rows = []
    for name in batch.names:
        prepared = _batch_prepare(batch, name)
        prepared_rows.append(prepared)
        language = prepared.row["recipe"]["language"]
        for intent in MATRIX_INTENTS:
            candidate_ids = [
                task["task_id"] for task in prepared.gold["tasks"] if task["intent"] == intent
            ]
            try:
                gold_oracle._name_contract(language, intent)
                supported = True
            except source_oracle.SourceOracleError:
                supported = False
            if not supported:
                status = "no_admission_diagnostic"
                selected_ids = []
                reason = "unsupported_language_intent"
                excluded = [{"task_id": task_id, "reason": reason} for task_id in candidate_ids]
                suite_sha = pack_sha = admission_sha = None
            else:
                suite, pack, report = _derive_prepared(
                    prepared, intent, allow_empty=True, filter_query_duplicates=True
                )
                selected_ids = [task["task_id"] for task in suite["tasks"]] if suite else []
                excluded = [
                    row for row in report["excluded"] if row["reason"] != "outside_selected_intent"
                ]
                status = report["status"]
                reason = report.get("reason")
                suite_sha = (
                    hashlib.sha256(evaluator.canonical(suite)).hexdigest() if suite else None
                )
                pack_sha = hashlib.sha256(evaluator.canonical(pack)).hexdigest() if pack else None
                admission_sha = (
                    hashlib.sha256(evaluator.canonical(report)).hexdigest() if suite else None
                )
                if suite is not None and pack is not None and _payloads is not None:
                    _payloads[(name, intent)] = (suite, pack, report)
            if sorted(candidate_ids) != sorted(selected_ids + [row["task_id"] for row in excluded]):
                raise ValueError("C4 matrix cell task accounting differs")
            cells.append(
                {
                    "repository": name,
                    "intent": intent,
                    "status": status,
                    "reason": reason,
                    "candidate_task_ids": candidate_ids,
                    "selected_task_ids": selected_ids,
                    "excluded": excluded,
                    "release_digest": prepared.document["digest"],
                    "manifest_sha256": digest_bytes(prepared.manifest_raw),
                    "repository_commit": prepared.manifest["repository_commit"],
                    "gold_capsule_identity_sha256": prepared.identity_sha256,
                    "suite_sha256": suite_sha,
                    "blind_pack_sha256": pack_sha,
                    "admission_sha256": admission_sha,
                }
            )
    _batch_recheck(batch, prepared_rows, tool_sources, source_digests)
    return {
        "schema_version": 1,
        "status": "diagnostic_unqualified",
        "product_capture": False,
        "qualified_default_search_conformance": False,
        "release_digest": batch.document["digest"],
        "release_document_sha256": digest_bytes(batch.document_raw),
        "split_manifest_sha256": hashlib.sha256(batch.split_raw).hexdigest(),
        "tool_source_sha256": source_digests,
        "repository_count": len(batch.names),
        "intent_count": len(MATRIX_INTENTS),
        "cells": cells,
    }


def write_matrix(
    release: Path,
    capsule_root: Path,
    checkout_root: Path,
    output: Path,
    *,
    expected_repositories: int = 12,
    emit_suites: bool = False,
) -> dict:
    """Write a matrix and optionally its admitted suites from one validation pass."""
    if not output.is_absolute() or output.exists() or output.is_symlink():
        raise ValueError("C4 matrix output root must be fresh and absolute")
    target = output.resolve()
    protected = (Path(__file__).resolve().parents[3], release, capsule_root, checkout_root)
    if any(
        target.is_relative_to(root.resolve()) or root.resolve().is_relative_to(target)
        for root in protected
    ):
        raise ValueError("C4 matrix output root must be external and disjoint")
    payloads: dict[tuple[str, str], tuple[dict, dict, dict]] | None = {} if emit_suites else None
    matrix = derive_matrix(
        release,
        capsule_root,
        checkout_root,
        expected_repositories=expected_repositories,
        _payloads=payloads,
    )
    output.mkdir(parents=True)
    try:
        (output / "admission-matrix.json").write_bytes(_raw(matrix))
        if payloads is not None:
            for (name, intent), (suite, pack, report) in sorted(payloads.items()):
                cell_root = output / name / intent
                cell_root.mkdir(parents=True)
                for file_name, value in (
                    ("suite.json", suite),
                    ("blind-pack.json", pack),
                    ("admission.json", report),
                ):
                    (cell_root / file_name).write_bytes(evaluator.canonical(value))
    except BaseException:
        shutil.rmtree(output)
        raise
    return matrix


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


def main() -> int:
    parser = argparse.ArgumentParser(description="Build a source-bound C4 admission matrix")
    parser.add_argument("--release", required=True, type=Path)
    parser.add_argument("--capsules", required=True, type=Path)
    parser.add_argument("--checkouts", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--expected-repositories", type=int, default=12)
    parser.add_argument("--emit-suites", action="store_true")
    args = parser.parse_args()
    matrix = write_matrix(
        args.release,
        args.capsules,
        args.checkouts,
        args.output,
        expected_repositories=args.expected_repositories,
        emit_suites=args.emit_suites,
    )
    print(f"admitted {len(matrix['cells'])} diagnostic C4 cells at {args.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
