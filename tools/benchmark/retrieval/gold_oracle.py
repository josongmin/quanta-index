"""Independent, source-derived code-search labels for a narrow declared cohort.

This oracle never consumes product hits. `named_function_declaration` is a
narrow named-function cohort. The `declaration_name_*` intents use the
declaration census of `source_oracle` and are labelled only for files where an
independent parser agrees with that census (`declaration_census_audit`). A
parser-refused file can be excluded from one query's labels only when its raw
text proves that the queried name is absent. Disagreements remain unjudged.
All labels remain unreviewed.

Schema v1 recipes carry development and holdout tasks for one repository and
check leakage within the recipe. Schema v2 recipes name one split and the
SHA-256 of a corpus-wide repository-disjoint split manifest; this module only
checks the recipe shape. The caller (`corpus_binding`) must verify that
manifest before any v2 recipe is admitted.
"""

from __future__ import annotations

import hashlib
import os
import re
from collections import defaultdict
from pathlib import Path

from tree_sitter_language_pack import get_parser

try:
    from tools.benchmark.retrieval import declaration_census_audit, source_oracle
except ModuleNotFoundError:  # benchmark script path without the repository root
    import sys

    sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
    from tools.benchmark.retrieval import declaration_census_audit, source_oracle

try:
    from evidence import IO_CHUNK_BYTES, EvidenceError, _consume_regular_file
except ModuleNotFoundError:  # package import outside the benchmark script path
    from tools.benchmark.evidence import IO_CHUNK_BYTES, EvidenceError, _consume_regular_file

ORACLE_VERSION = 2
MAX_TASKS = 2000
MAX_FILES = 4096
MAX_SOURCE_BYTES = 512 * 1024 * 1024
MAX_LABELS_PER_TASK = 20_000
MAX_TOTAL_LABELS = 100_000
LANGUAGE_SUFFIXES = {
    "python": frozenset({".py"}),
    "rust": frozenset({".rs"}),
    "go": frozenset({".go"}),
    "typescript": frozenset({".ts", ".tsx"}),
    "javascript": frozenset({".js", ".jsx", ".mjs", ".cjs"}),
}
FUNCTION_NODES = {
    "python": frozenset({"function_definition"}),
    "rust": frozenset({"function_item"}),
    "go": frozenset({"function_declaration", "method_declaration"}),
    "typescript": frozenset({"function_declaration", "method_definition"}),
    "javascript": frozenset({"function_declaration", "method_definition"}),
}
# Declaration-name intents: exact names and identifier variants per language.
DECLARATION_INTENTS = {
    "declaration_name_exact": "exact",
    "declaration_name_prefix": "prefix",
    "declaration_name_infix": "infix",
    "declaration_name_components": "components",
    "declaration_name_osa1": "osa1",
    "declaration_name_osa1_casefold": "osa1_casefold",
}
INTENTS = frozenset({"literal_utf8_exact", "named_function_declaration", *DECLARATION_INTENTS})
ID = re.compile(r"[A-Za-z][A-Za-z0-9_.-]*\Z")
SHA256_HEX = re.compile(r"[0-9a-f]{64}\Z")
SPLITS = ("development", "holdout")
TASK_FIELDS = frozenset(
    {
        "task_id",
        "split",
        "query_family_id",
        "intent",
        "query",
        "scope_prefix",
        "language",
        "case_semantics",
        "normalization",
    }
)
RECIPE_FIELDS = {
    1: frozenset({"schema_version", "tasks"}),
    2: frozenset({"schema_version", "split", "split_manifest_sha256", "tasks"}),
}
BARE_NAME = re.compile(r"[A-Za-z_][A-Za-z0-9_]*\Z")


def _sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _bounded_source(path: Path, remaining: int) -> bytes:
    def consume(handle) -> bytes:
        if os.fstat(handle.fileno()).st_size > remaining:
            raise EvidenceError("gold source exceeds release byte limit")
        blocks, count = [], 0
        while block := handle.read(min(IO_CHUNK_BYTES, remaining - count + 1)):
            count += len(block)
            if count > remaining:
                raise EvidenceError("gold source exceeds release byte limit")
            blocks.append(block)
        return b"".join(blocks)

    return _consume_regular_file(path, consume)


def _path(path: object, *, allow_empty: bool = False) -> bool:
    if not isinstance(path, str):
        return False
    if not path:
        return allow_empty
    return (
        not path.startswith("/")
        and "\\" not in path
        and "\x00" not in path
        and all(
            part not in ("", ".", "..") and part.casefold() != ".git" for part in path.split("/")
        )
    )


def validate_recipe(recipe: object) -> dict:
    version = recipe.get("schema_version") if isinstance(recipe, dict) else None
    if (
        type(version) is not int
        or version not in RECIPE_FIELDS
        or set(recipe)
        not in (
            (RECIPE_FIELDS[version], RECIPE_FIELDS[version] | {"checker_identity"})
            if version == 2
            else (RECIPE_FIELDS[version],)
        )
        or not isinstance(recipe["tasks"], list)
        or not (2 if version == 1 else 1) <= len(recipe["tasks"]) <= MAX_TASKS
    ):
        raise EvidenceError("gold recipe must have bounded schema v1 or v2 tasks")
    if "checker_identity" in recipe and not isinstance(recipe["checker_identity"], dict):
        raise EvidenceError("gold recipe checker identity must be a language mapping")
    if version == 2 and (
        recipe["split"] not in SPLITS
        or not isinstance(recipe["split_manifest_sha256"], str)
        or SHA256_HEX.fullmatch(recipe["split_manifest_sha256"]) is None
    ):
        raise EvidenceError("gold recipe v2 requires one split and a split-manifest SHA-256")
    task_fields = TASK_FIELDS if version == 1 else TASK_FIELDS - {"split"}
    ids: set[str] = set()
    queries: set[tuple[str, str, str, object]] = set()
    normalized_queries: list[tuple[str, str, str]] = []
    families: dict[str, str] = {}
    splits: set[str] = set()
    for task in recipe["tasks"]:
        intended_typo = (
            isinstance(task, dict) and task.get("intent") == "declaration_name_osa1_casefold"
        )
        expected_fields = task_fields | ({"intended_name"} if intended_typo else set())
        if not isinstance(task, dict) or set(task) != expected_fields:
            raise EvidenceError("gold recipe task has missing or unknown fields")
        task_id, family = task["task_id"], task["query_family_id"]
        split = task["split"] if version == 1 else recipe["split"]
        if (
            not isinstance(task_id, str)
            or not ID.fullmatch(task_id)
            or task_id in ids
            or split not in SPLITS
            or not isinstance(family, str)
            or not ID.fullmatch(family)
            or (family in families and families[family] != split)
        ):
            raise EvidenceError("duplicate task or cross-split query family")
        ids.add(task_id)
        families[family] = split
        splits.add(split)
        intent, query, language = task["intent"], task["query"], task["language"]
        if isinstance(query, str):
            try:
                query_bytes = query.encode("utf-8")
            except UnicodeError as error:
                raise EvidenceError("gold query is not valid UTF-8") from error
        else:
            query_bytes = b""
        if intent in DECLARATION_INTENTS:
            try:
                source_oracle._require_query(_name_contract(language, intent), query)
            except (source_oracle.SourceOracleError, TypeError) as error:
                raise EvidenceError(
                    "gold task has unsupported or ambiguous query semantics"
                ) from error
        if intended_typo:
            intended_name = task["intended_name"]
            try:
                source_oracle._require_query(
                    _name_contract(language, "declaration_name_exact"), intended_name
                )
            except (source_oracle.SourceOracleError, TypeError) as error:
                raise EvidenceError(
                    "typo intended name is not a valid exact declaration name"
                ) from error
            if (
                query.casefold() == intended_name.casefold()
                or not source_oracle.osa_distance_at_most_one(
                    query.casefold(), intended_name.casefold()
                )
            ):
                raise EvidenceError("typo query is not one casefolded edit from intended name")
        if (
            intent not in INTENTS
            or not isinstance(query, str)
            or not query
            or len(query_bytes) > 1024
            or "\x00" in query
            or (intent == "named_function_declaration" and BARE_NAME.fullmatch(query) is None)
            or (
                intent == "named_function_declaration"
                and (not isinstance(language, str) or language not in LANGUAGE_SUFFIXES)
            )
            or (intent == "literal_utf8_exact" and language is not None)
            or (intent in DECLARATION_INTENTS and language not in source_oracle.DECLARATION_CENSUS)
            or not _path(task["scope_prefix"], allow_empty=True)
            or task["case_semantics"]
            != ("casefold" if intent == "declaration_name_osa1_casefold" else "sensitive")
            or task["normalization"] != "none_raw_utf8"
        ):
            raise EvidenceError("gold task has unsupported or ambiguous query semantics")
        query_key = (intent, query, task["scope_prefix"], language)
        if query_key in queries:
            raise EvidenceError("duplicate gold query/scope across tasks")
        queries.add(query_key)
        normalized = " ".join(query.casefold().split())
        normalized_queries.append((task_id, split, normalized))
    if version == 1 and splits != set(SPLITS):
        raise EvidenceError("gold recipe requires development and holdout tasks")
    for index, (task_id, split, query) in enumerate(normalized_queries):
        for other_id, other_split, other_query in normalized_queries[index + 1 :]:
            if split == other_split:
                continue
            if query == other_query:
                raise EvidenceError(f"cross-split normalized query leakage: {task_id} / {other_id}")
            if min(len(query), len(other_query)) < 10:
                continue
            grams = {query[pos : pos + 5] for pos in range(len(query) - 4)}
            other_grams = {other_query[pos : pos + 5] for pos in range(len(other_query) - 4)}
            if len(grams & other_grams) / len(grams | other_grams) >= 0.8:
                raise EvidenceError(f"cross-split near-duplicate query: {task_id} / {other_id}")
    return recipe


def _name_contract(language: object, intent: str) -> str:
    key = (language, DECLARATION_INTENTS[intent])
    contracts = [name for name, value in source_oracle.NAME_CONTRACTS.items() if value == key]
    if len(contracts) != 1:
        raise source_oracle.SourceOracleError("no declaration-name contract for language")
    return contracts[0]


def _census_audits(recipe: dict, sources: dict, view: Path) -> dict[str, dict]:
    """Audit each declaration-intent language once over the complete view."""
    languages = sorted(
        {task["language"] for task in recipe["tasks"] if task["intent"] in DECLARATION_INTENTS}
    )
    audits = {}
    for language in languages:
        try:
            result = declaration_census_audit.audit_files(language, view, sorted(sources))
        except declaration_census_audit.CensusAuditError as error:
            raise EvidenceError(f"declaration census audit failed: {error}") from error
        if result["file_set_sha256"] != declaration_census_audit.file_set_sha256(
            language, {path: raw for path, (raw, _sha) in sources.items()}
        ):
            raise EvidenceError("declaration census audit read different source bytes")
        audits[language] = {
            "census": result["census"],
            "checker": result["checker"],
            "files": result["files"],
            "file_set_sha256": result["file_set_sha256"],
            "agreeing_declarations": result["agreeing_declarations"],
            "status": result["status"],
            "refused_paths": sorted(row["path"] for row in result["refused"]),
            "disagreement_paths": sorted(row["path"] for row in result["disagreements"]),
        }
    return audits


def _declaration_spans(
    raw: bytes, path: str, query: str, language: str, intent: str, census: dict
) -> list[tuple[int, int, str]]:
    if path not in census:
        census[path] = source_oracle.declaration_census(language, path, raw)
    variant = DECLARATION_INTENTS[intent]
    spans = []
    for start, end, _definition_start, _definition_end, kind in census[path]:
        name = source_oracle._name_text(raw[start:end])
        if (
            (name == query)
            if variant == "exact"
            else source_oracle._variant_matches(variant, query, name)
        ):
            spans.append((start, end, kind))
    return spans


def _textually_excluded(raw: bytes, query: str, variant: str) -> bool:
    """Use the source oracle's conservative name-absence predicate."""
    try:
        return source_oracle.declaration_query_textually_excluded(raw, query, variant)
    except source_oracle.SourceOracleError:
        return False


def _source_census_refused(language: str, path: str, raw: bytes) -> bool:
    """An independent checker refusal alone cannot authorize an exclusion."""
    try:
        source_oracle.declaration_census(language, path, raw)
    except source_oracle.SourceOracleError as error:
        message = str(error)
        return "parse error:" in message or "declaration lacks name:" in message
    return False


def _literal_spans(raw: bytes, query: bytes) -> list[tuple[int, int, str]]:
    spans = []
    offset = 0
    while (start := raw.find(query, offset)) >= 0:
        spans.append((start, start + len(query), "literal_occurrence"))
        if len(spans) > MAX_LABELS_PER_TASK:
            raise EvidenceError("gold literal task exceeds label limit")
        offset = start + 1
    return spans


def _definition_spans(raw: bytes, query: bytes, language: str) -> tuple[list, str | None]:
    try:
        root = get_parser(language).parse(raw).root_node
    except (LookupError, ValueError) as error:
        return [], type(error).__name__
    if root.has_error:
        return [], "parser_error"
    spans = []
    nodes = [root]
    while nodes:
        node = nodes.pop()
        if node.type in FUNCTION_NODES[language]:
            name = node.child_by_field_name("name")
            if name is None:
                return [], "missing_declaration_name"
            if raw[name.start_byte : name.end_byte] == query:
                spans.append((name.start_byte, name.end_byte, node.type))
                if len(spans) > MAX_LABELS_PER_TASK:
                    raise EvidenceError("gold definition task exceeds label limit")
        nodes.extend(reversed(node.children))
    return sorted(spans), None


def derive(recipe: dict, manifest: dict, view: Path) -> tuple[dict, dict]:
    validate_recipe(recipe)
    if (
        not isinstance(manifest, dict)
        or set(manifest) != {"repository_commit", "files"}
        or not isinstance(manifest["files"], list)
        or not 0 < len(manifest["files"]) <= MAX_FILES
    ):
        raise EvidenceError("gold oracle requires a bounded release manifest")
    sources = {}
    total = 0
    for row in manifest["files"]:
        if (
            not isinstance(row, dict)
            or set(row) != {"path", "file_sha256"}
            or not _path(row["path"])
            or row["path"] in sources
            or not isinstance(row["file_sha256"], str)
        ):
            raise EvidenceError("gold release manifest has a duplicate or invalid file")
        raw = _bounded_source(view / row["path"], MAX_SOURCE_BYTES - total)
        total += len(raw)
        file_sha = _sha(raw)
        if total > MAX_SOURCE_BYTES or file_sha != row["file_sha256"]:
            raise EvidenceError("gold source exceeds limit or differs from release")
        sources[row["path"]] = (raw, file_sha)
    if list(sources) != sorted(sources):
        raise EvidenceError("gold release manifest files are not sorted")
    audits = _census_audits(recipe, sources, view)
    if "checker_identity" in recipe and recipe["checker_identity"] != {
        language: audit["checker"] for language, audit in audits.items()
    }:
        raise EvidenceError("gold checker identity differs from frozen recipe")
    censuses: dict[str, dict] = {language: {} for language in audits}
    source_refusals: dict[tuple[str, str], bool] = {}
    gold_tasks, blind_tasks = [], []
    label_count = 0
    paths_by_split: dict[str, set[str]] = {"development": set(), "holdout": set()}
    recipe_split = recipe.get("split")
    for task in recipe["tasks"]:
        split = task.get("split", recipe_split)
        intended_typo = task["intent"] == "declaration_name_osa1_casefold"
        scoring_query = task["intended_name"] if intended_typo else task["query"]
        scoring_intent = "declaration_name_exact" if intended_typo else task["intent"]
        query = scoring_query.encode("utf-8")
        prefix = task["scope_prefix"]
        labels, unsupported, text_excluded = [], [], []
        selected = 0
        for path, (raw, file_sha) in sources.items():
            if prefix and not (path == prefix or path.startswith(prefix + "/")):
                continue
            if (
                task["intent"] == "named_function_declaration"
                and Path(path).suffix not in (LANGUAGE_SUFFIXES[task["language"]])
            ) or (
                task["intent"] in DECLARATION_INTENTS
                and source_oracle.declaration_language(path) != task["language"]
            ):
                continue
            selected += 1
            if task["intent"] == "literal_utf8_exact":
                spans = _literal_spans(raw, query)
                reason = None
            elif task["intent"] in DECLARATION_INTENTS:
                audit = audits[task["language"]]
                reason = (
                    "census_refused"
                    if path in audit["refused_paths"]
                    else "census_disagreement"
                    if path in audit["disagreement_paths"]
                    else None
                )
                refusal_key = (task["language"], path)
                if reason == "census_refused" and refusal_key not in source_refusals:
                    source_refusals[refusal_key] = _source_census_refused(
                        task["language"], path, raw
                    )
                if (
                    reason in ("census_refused", "census_disagreement")
                    and (reason == "census_disagreement" or source_refusals[refusal_key])
                    and _textually_excluded(raw, scoring_query, DECLARATION_INTENTS[scoring_intent])
                ):
                    # The query cannot match any name written in this file.
                    text_excluded.append({"path": path, "reason": reason})
                    continue
                spans = (
                    []
                    if reason
                    else _declaration_spans(
                        raw,
                        path,
                        scoring_query,
                        task["language"],
                        scoring_intent,
                        censuses[task["language"]],
                    )
                )
            else:
                spans, reason = _definition_spans(raw, query, task["language"])
            if reason is not None:
                unsupported.append({"path": path, "reason": reason})
                continue
            for start, end, kind in spans:
                if len(labels) >= MAX_LABELS_PER_TASK:
                    raise EvidenceError("gold task exceeds label limit")
                label_count += 1
                if label_count > MAX_TOTAL_LABELS:
                    raise EvidenceError("gold capsule exceeds total label limit")
                labels.append(
                    {
                        "path": path,
                        "file_sha256": file_sha,
                        "start_byte": start,
                        "end_byte": end,
                        "kind": kind,
                        "local_name": None
                        if task["intent"] == "literal_utf8_exact"
                        else raw[start:end].decode("utf-8")
                        if task["intent"] in DECLARATION_INTENTS
                        else scoring_query,
                    }
                )
        if selected == 0:
            unsupported.append({"path": None, "reason": "empty_declared_scope"})
        labels.sort(key=lambda row: (row["path"], row["start_byte"], row["end_byte"]))
        paths_by_split[split].update(row["path"] for row in labels)
        near_metadata = {}
        if intended_typo:
            near: dict[str, set[str]] = defaultdict(set)
            exact_collision: dict[str, set[str]] = defaultdict(set)
            for path, declarations in censuses[task["language"]].items():
                if prefix and not (path == prefix or path.startswith(prefix + "/")):
                    continue
                raw = sources[path][0]
                for start, end, *_rest in declarations:
                    name = source_oracle._name_text(raw[start:end])
                    if name.casefold() == task["query"].casefold():
                        exact_collision[name].add(path)
                    elif source_oracle._variant_matches("osa1_casefold", task["query"], name):
                        near[name].add(path)
            uncertain_rows = {
                row["path"]: row for row in (*unsupported, *text_excluded) if row["path"]
            }
            near_excluded = [
                uncertain_rows[path]
                for path in sorted(uncertain_rows)
                if _textually_excluded(sources[path][0], task["query"], "osa1_casefold")
            ]
            near_metadata = {
                "near_declaration_state": (
                    "partial"
                    if len(near_excluded) != len(uncertain_rows) or any(row["path"] is None for row in unsupported)
                    else "complete"
                ),
                "near_census_text_excluded": near_excluded,
                "near_declaration_names": sorted(near),
                "near_declaration_files": sorted(
                    {path for paths in near.values() for path in paths}
                ),
                "exact_collision_names": sorted(exact_collision),
                "exact_collision_files": sorted(
                    {path for paths in exact_collision.values() for path in paths}
                ),
            }
        gold_tasks.append(
            {
                **task,
                **near_metadata,
                "split": split,
                "labels": labels,
                "unsupported": unsupported,
                "census_text_excluded": text_excluded,
                "label_state": "unjudged" if unsupported else "mechanical_unreviewed",
                "answerable": None if unsupported else bool(labels),
            }
        )
        blind_tasks.append(
            {
                key: task[key]
                for key in (
                    "task_id",
                    "query_family_id",
                    "intent",
                    "query",
                    "scope_prefix",
                    "language",
                    "case_semantics",
                    "normalization",
                )
            }
        )
    if paths_by_split["development"] & paths_by_split["holdout"]:
        raise EvidenceError("gold-positive file leakage across development and holdout")
    gold = {
        "schema_version": recipe["schema_version"],
        "oracle_version": ORACLE_VERSION,
        "census_audits": audits,
        "tasks": gold_tasks,
    }
    if recipe["schema_version"] == 2:
        gold.update(split=recipe_split, split_manifest_sha256=recipe["split_manifest_sha256"])
    return gold, {"schema_version": 1, "tasks": blind_tasks}
