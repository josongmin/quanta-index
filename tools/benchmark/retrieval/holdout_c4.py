"""Admit frozen declaration capsules to a lexical file diagnostic.

This is an adapter over the existing suite, blind pack and source-oracle
contracts. Parser-refused files are excluded only per query when the complete
capsule proves that its raw bytes cannot contain a matching declaration name.
Checker disagreements remain unsupported. Declaration target relevance differs
from default CodeSearch semantics; this is not a qualified default-search score.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import shutil
import sys
from dataclasses import dataclass
from pathlib import Path

# corpus_binding still imports corpus_release as a top-level benchmark module.
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from tools.benchmark import corpus_binding
from tools.benchmark.evidence import _read_control_file, digest_bytes, parse_json
from tools.benchmark.retrieval import evaluator, gold_oracle, query_plan, source_oracle

MATRIX_INTENTS = tuple(sorted(gold_oracle.DECLARATION_INTENTS))


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
    value = parse_json(_read_control_file(path).decode("utf-8"))
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
    try:
        contract = gold_oracle._name_contract(language, intent)
    except source_oracle.SourceOracleError as exc:
        raise ValueError("C4 has no declaration oracle for selected language") from exc
    audit = gold.get("census_audits", {}).get(language, {})
    refused_paths = audit.get("refused_paths")
    disagreement_paths = audit.get("disagreement_paths")
    if (
        audit.get("status") not in ("admitted", "unsupported")
        or not isinstance(refused_paths, list)
        or not isinstance(disagreement_paths, list)
        or refused_paths != sorted(set(refused_paths))
        or disagreement_paths != sorted(set(disagreement_paths))
        or set(refused_paths) & set(disagreement_paths)
        or (audit["status"] == "admitted" and (refused_paths or disagreement_paths))
        or (audit["status"] == "unsupported" and not (refused_paths or disagreement_paths))
    ):
        raise ValueError("C4 independent census is not admitted or query-provable")
    required_rows = sorted(
        [(path, "census_refused") for path in refused_paths]
        + [(path, "census_disagreement") for path in disagreement_paths]
    )
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
            path for path, reason in excluded_rows if reason == "census_refused"
        ]
        if not task["answerable"]:
            try:
                if absence_oracle is None:
                    raise source_oracle.SourceOracleError("negative query is not an identifier")
                absence_oracle.expected_rows(
                    source_oracle.ASCII_CODE_SEARCH_ABSENT_CASEFOLD,
                    task["query"],
                    "distinct_file",
                )
            except source_oracle.SourceOracleError:
                excluded.append(
                    {"task_id": task["task_id"], "reason": "negative_not_default_search_absent"}
                )
                continue
        try:
            query_plan.plan_lexical_request("code_search_file", task["query"])
        except query_plan.QueryPlanError:
            excluded.append({"task_id": task["task_id"], "reason": "query_not_admitted"})
            continue
        if filter_query_duplicates:
            try:
                evaluator.check_query_near_duplicates(
                    [(prior["task_id"], prior["query"]) for prior in selected]
                    + [(task["task_id"], task["query"])]
                )
            except evaluator.EvidenceError:
                excluded.append({"task_id": task["task_id"], "reason": "query_near_duplicate"})
                continue
        selected.append(task)
        if excluded_rows:
            proved_exclusion_tasks += 1
        if refused_selected_paths:
            declaration_exclusions[(contract, task["query"])] = set(refused_selected_paths)
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
        score_contract = (
            contract if task["answerable"] else source_oracle.ASCII_CODE_SEARCH_ABSENT_CASEFOLD
        )
        judgments = oracle.expected_rows(score_contract, task["query"], "distinct_file")
        source_contract = {"contract": score_contract, "unit": "distinct_file"}
        paths = sorted(declaration_exclusions.get((contract, task["query"]), set()))
        if paths and task["answerable"]:
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
                "gold": evaluator.source_oracle_gold(source, oracle, score_contract, task["query"]),
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
            "execution_policy": "code_search_file",
            "semantic_relation": "declaration_target_file_diagnostic_only",
            "case_semantics_equivalent": False,
            "negative_control": "single_term_absent_from_folded_content_and_path",
            "qualified_default_search_conformance": False,
            "gold_capsule_identity_sha256": prepared.identity_sha256,
            "binding": binding,
        },
    )


def derive(release: Path, capsule: Path, checkout: Path, intent: str) -> tuple[dict, dict, dict]:
    """Validate capsule/source and derive one diagnostic suite without writing."""
    if intent not in gold_oracle.DECLARATION_INTENTS:
        raise ValueError("C4 requires one supported declaration-name intent")
    suite, pack, report = _derive_prepared(_prepare(release, capsule, checkout), intent)
    assert suite is not None and pack is not None
    return suite, pack, report


def _batch_preflight(
    release: Path, capsule_root: Path, checkout_root: Path, expected_repositories: int
) -> _Batch:
    """Validate the shared release roster and global split once per batch."""
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
                _read_control_file(prepared.capsule / file_name)
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


def derive_matrix(
    release: Path, capsule_root: Path, checkout_root: Path, *, expected_repositories: int = 12
) -> dict:
    """Admit all declaration intents without publishing suites or product scores."""
    tool_sources = {
        "holdout_c4": Path(__file__),
        "gold_oracle": Path(gold_oracle.__file__),
        "source_oracle": Path(source_oracle.__file__),
        "query_plan": Path(query_plan.__file__),
        "evaluator": Path(evaluator.__file__),
        "corpus_binding": Path(corpus_binding.__file__),
    }
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
                suite_sha = pack_sha = None
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
) -> dict:
    """Write one admission-only matrix to a fresh external directory."""
    if not output.is_absolute() or output.exists() or output.is_symlink():
        raise ValueError("C4 matrix output root must be fresh and absolute")
    target = output.resolve()
    protected = (Path(__file__).resolve().parents[3], release, capsule_root, checkout_root)
    if any(
        target.is_relative_to(root.resolve()) or root.resolve().is_relative_to(target)
        for root in protected
    ):
        raise ValueError("C4 matrix output root must be external and disjoint")
    matrix = derive_matrix(
        release, capsule_root, checkout_root, expected_repositories=expected_repositories
    )
    output.mkdir(parents=True)
    try:
        (output / "admission-matrix.json").write_bytes(_raw(matrix))
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
    args = parser.parse_args()
    matrix = write_matrix(
        args.release,
        args.capsules,
        args.checkouts,
        args.output,
        expected_repositories=args.expected_repositories,
    )
    print(f"admitted {len(matrix['cells'])} diagnostic C4 cells at {args.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
