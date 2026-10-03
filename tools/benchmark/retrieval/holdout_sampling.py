"""Freeze a seeded, label-free sampling ledger and v2 gold recipes for a holdout release.

Inputs are a holdout corpus release, a development corpus release and a seed.
Every choice is a pure function of (seed, repository, lane, key) and frozen
release bytes; no product output or score is read. The output is the per-lane
population, quota, admitted count, underfill reasons and a probability only
where the sampling design supports one. It also writes one schema v2
single-split `gold_oracle` recipe per holdout repository and the corpus-wide
repository-disjoint split manifest that binds them. Labels are derived later
by `corpus_binding.capture_gold`; subjective
natural-language lanes stay underfilled until reviewed qrels exist.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from collections import defaultdict
from pathlib import Path
from typing import Any

try:
    from tools.benchmark.retrieval import (
        declaration_census_audit,
        gold_oracle,
        identifier_robustness_suite,
        literal_source_oracle,
        source_oracle,
    )
except ModuleNotFoundError:  # direct script invocation
    sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
    from tools.benchmark.retrieval import (
        declaration_census_audit,
        gold_oracle,
        identifier_robustness_suite,
        literal_source_oracle,
        source_oracle,
    )

SAMPLING_VERSION = 3
# Provisional per-repository engineering design (S30-B08); not a population rule.
QUOTAS = {
    "exact_content": 20,
    "exact_definition": 100,
    "variant_prefix": 8,
    "variant_infix": 8,
    "variant_components": 7,
    "variant_osa1": 400,
    "no_answer_synthetic": 5,
    "no_answer_wrong_repository": 5,
    "natural_language_workflow": 20,
}
SCALE_DIAGNOSTIC_QUOTAS = {
    "exact_content": 100,
    "exact_definition": 100,
    "variant_prefix": 130,
    "variant_infix": 130,
    "variant_components": 130,
    "variant_osa1": 400,
    "no_answer_synthetic": 100,
    "no_answer_wrong_repository": 100,
    "natural_language_workflow": 0,
}
PROFILES = {"baseline_v3": QUOTAS, "scale_diagnostic_v1": SCALE_DIAGNOSTIC_QUOTAS}
SCALE_MINIMUM_LANES = tuple(
    lane for lane in SCALE_DIAGNOSTIC_QUOTAS if lane != "natural_language_workflow"
)
VARIANT_LANES = {
    "variant_prefix": ("prefix", "declaration_name_prefix"),
    "variant_infix": ("infix", "declaration_name_infix"),
    "variant_components": ("components", "declaration_name_components"),
    "variant_osa1": ("typo", "declaration_name_osa1_casefold"),
}
# Only exact-definition draws take the first K of a fixed eligible-name set.
# Other lanes filter or retry after ranking, or do not enumerate a finite
# query population. Their admitted/population ratio is not an inclusion
# probability and must not be used as a design weight.
INCLUSION_PROBABILITY_BASIS = {
    "exact_content": "not_derived_post_rank_occurrence_cap",
    "exact_definition": "nominal_uniform_seeded_rank_over_eligible_names",
    **{lane: "not_derived_round_robin_proposal_and_rejection" for lane in VARIANT_LANES},
    "no_answer_synthetic": "not_derived_bounded_rejection_sampler",
    "no_answer_wrong_repository": "not_derived_post_rank_content_filter",
    "natural_language_workflow": "not_derived_unfilled_pending_review",
}
MIN_BASE_NAME = 4
MIN_WRONG_REPOSITORY_NAME = 6
MAX_TASK_LABELS = 2000
LITERAL_BYTES = (12, 80)
LITERAL_MIN_ALNUM = 4
MAX_ATTEMPTS = identifier_robustness_suite.MAX_ATTEMPTS
DUNDER = re.compile(r"__\w+__\Z")


def _draw(seed: int, *parts: object) -> int:
    text = ":".join(str(part) for part in (seed, *parts))
    return int.from_bytes(hashlib.sha256(text.encode("utf-8")).digest()[:8], "big")


def _ranked(values: set[str] | list[str], seed: int, *parts: object) -> list[str]:
    return sorted(set(values), key=lambda value: (_draw(seed, *parts, value), value))


def _canonical(value: object) -> bytes:
    return (json.dumps(value, indent=2, sort_keys=True, ensure_ascii=False) + "\n").encode("utf-8")


class Repository:
    """One release repository's code_only bytes, census and content index."""

    def __init__(self, release: Path, document: dict, row: dict) -> None:
        self.name = row["recipe"]["name"]
        self.language = row["recipe"]["language"]
        self.commit = row["recipe"]["revision"]
        self.release_digest = document["digest"]
        metadata = row["views"]["code_only"]
        self.universe_digest = metadata["file_universe_digest"]
        manifest = json.loads((release / metadata["manifest"]).read_bytes())
        self.view = release / "views" / self.name / "code_only"
        self.files: dict[str, bytes] = {}
        for entry in manifest["files"]:
            raw = (self.view / entry["path"]).read_bytes()
            if hashlib.sha256(raw).hexdigest() != entry["file_sha256"]:
                raise ValueError(f"release view differs from manifest: {self.name}:{entry['path']}")
            self.files[entry["path"]] = raw
        self.folded = [raw.decode("utf-8").casefold() for raw in self.files.values()]
        self.folded_words = {word for text in self.folded for word in re.findall(r"[^\W]+", text)}
        self.folded_paths = [path.casefold() for path in self.files]
        self.ascii_tokens_by_length: dict[int, set[str]] = defaultdict(set)
        for raw in self.files.values():
            for match in source_oracle.ASCII_TOKEN_SUPERSET.finditer(raw):
                token = match.group().decode("ascii").lower()
                self.ascii_tokens_by_length[len(token)].add(token)
        self.audit: dict[str, Any] | None = None
        self.all_audits: dict[str, dict[str, Any]] = {}
        self.names: dict[str, int] = {}

    def run_census(self) -> None:
        if self.language not in source_oracle.DECLARATION_CENSUS:
            return
        languages = sorted(
            {
                language
                for path in self.files
                if (language := source_oracle.declaration_language(path))
                in source_oracle.DECLARATION_CENSUS
            }
        )
        self.all_audits = {
            language: declaration_census_audit.audit_files(language, self.view, sorted(self.files))
            for language in languages
        }
        self.audit = self.all_audits[self.language]
        excluded = {row["path"] for row in self.audit["refused"]} | {
            row["path"] for row in self.audit["disagreements"]
        }
        counts: dict[str, int] = defaultdict(int)
        for path, raw in self.files.items():
            if source_oracle.declaration_language(path) != self.language or path in excluded:
                continue
            for start, end, *_rest in source_oracle.declaration_census(self.language, path, raw):
                counts[raw[start:end].decode("utf-8")] += 1
        self.names = dict(counts)

    def content_absent(self, query: str) -> bool:
        folded = query.casefold()
        return not any(folded in text for text in self.folded)

    def default_file_search_absent(self, query: str) -> bool:
        """Prove that neither literal search nor its empty-result OSA1 fallback can match."""
        folded = query.casefold()
        if not self.content_absent(query) or any(folded in path for path in self.folded_paths):
            return False
        if not (
            query.isascii() and source_oracle.IDENTIFIER.fullmatch(query) and 3 <= len(query) <= 64
        ):
            return True
        for length in range(max(1, len(folded) - 1), len(folded) + 2):
            if any(
                source_oracle.osa_distance_at_most_one(folded, token)
                for token in self.ascii_tokens_by_length.get(length, ())
            ):
                return False
        return True

    def variant_rows(self, variant: str, query: str) -> int:
        if variant == "exact":
            return self.names.get(query, 0)
        return sum(
            count
            for name, count in self.names.items()
            if source_oracle._variant_matches(variant, query, name)
        )


def _task(
    repository: Repository,
    lane: str,
    index: int,
    family: str,
    intent: str,
    query: str,
    *,
    intended_name: str | None = None,
):
    task = {
        "task_id": f"{repository.name}.{lane}.{index:03d}",
        "query_family_id": family,
        "intent": intent,
        "query": query,
        "scope_prefix": "",
        "language": None if intent == "literal_utf8_exact" else repository.language,
        "case_semantics": ("casefold" if intent in gold_oracle.CASEFOLD_INTENTS else "sensitive"),
        "normalization": "none_raw_utf8",
    }
    if intended_name is not None:
        if intent != "declaration_name_osa1_casefold":
            raise ValueError("intended name is valid only for casefold OSA1 tasks")
        task["intended_name"] = intended_name
    return task


def _literals(
    repository: Repository, seed: int, ledger: dict, quotas: dict[str, int]
) -> list[dict]:
    candidates: dict[str, tuple[str, int]] = {}
    for path, raw in repository.files.items():
        for number, line in enumerate(raw.split(b"\n"), 1):
            text = line.strip()
            if (
                LITERAL_BYTES[0] <= len(text) <= LITERAL_BYTES[1]
                and sum(chr(byte).isalnum() for byte in text if byte < 128) >= LITERAL_MIN_ALNUM
                and b"\r" not in text
            ):
                candidates.setdefault(text.decode("utf-8"), (path, number))
    population = len(candidates)
    tasks, skipped = [], defaultdict(int)
    for query in _ranked(list(candidates), seed, repository.name, "exact_content"):
        if len(tasks) == quotas["exact_content"]:
            break
        try:
            literal_source_oracle.require_literal(query)
        except literal_source_oracle.LiteralOracleError:
            skipped["outside_literal_query_contract"] += 1
            continue
        encoded = query.encode("utf-8")
        occurrences = sum(raw.count(encoded) for raw in repository.files.values())
        if occurrences > MAX_TASK_LABELS:
            skipped["too_many_occurrences"] += 1
            continue
        index = len(tasks) + 1
        tasks.append(
            _task(
                repository,
                "lit",
                index,
                f"{repository.name}.lit.{index:03d}",
                "literal_utf8_exact",
                query,
            )
        )
    ledger["exact_content"] = {
        "population": population,
        "population_rule": f"distinct stripped code_only lines of {LITERAL_BYTES[0]}-"
        f"{LITERAL_BYTES[1]} bytes with at least {LITERAL_MIN_ALNUM} ASCII alphanumerics",
        "skipped": dict(skipped),
    }
    return tasks


def _declarations(
    repository: Repository, seed: int, ledger: dict, quotas: dict[str, int]
) -> list[dict]:
    if repository.audit is None:
        for lane in ("exact_definition", *VARIANT_LANES):
            ledger[lane] = {"population": 0, "underfill": "language_without_declaration_census"}
        return []
    eligible = sorted(
        name
        for name, count in repository.names.items()
        if name.isascii()
        and source_oracle.IDENTIFIER.fullmatch(name)
        and len(name) >= MIN_BASE_NAME
        and not DUNDER.fullmatch(name)
        and count <= MAX_TASK_LABELS
    )
    ranked = _ranked(eligible, seed, repository.name, "base_name")
    tasks: list[dict] = []
    ledger["exact_definition"] = {"population": len(eligible)}
    paired_bases = ranked[: quotas["exact_definition"]]
    for index, name in enumerate(paired_bases, 1):
        tasks.append(
            _task(
                repository,
                "def",
                index,
                f"{repository.name}.name.{name}",
                "declaration_name_exact",
                name,
            )
        )
    used = {("declaration_name_exact", task["query"]) for task in tasks}
    filled = {lane: [] for lane in VARIANT_LANES}
    records: list[dict] = []
    cursor = 0
    lanes = [lane for lane in VARIANT_LANES if lane != "variant_osa1"]
    for name in ranked:
        open_lanes = [lane for lane in lanes if len(filled[lane]) < quotas[lane]]
        if not open_lanes:
            break
        lane = open_lanes[cursor % len(open_lanes)]
        cursor += 1
        proposer, intent = VARIANT_LANES[lane]
        variant = gold_oracle.DECLARATION_INTENTS[intent]
        family = f"{repository.name}.name.{name}"
        record: dict[str, Any] = {"lane": lane, "base_name": name}
        query = None
        for attempt in range(MAX_ATTEMPTS):
            query, meta = identifier_robustness_suite.propose(proposer, name, seed, family, attempt)
            if query is None and "retry" in meta:
                continue
            if query is None:
                record["ineligible"] = meta.get("ineligible", "no_variant")
                break
            try:
                source_oracle._require_query(
                    gold_oracle._name_contract(repository.language, intent), query
                )
            except source_oracle.SourceOracleError:
                record["ineligible"] = "outside_query_contract"
                query = None
                break
            if (intent, query) in used:
                record["ineligible"] = "duplicate_query"
                query = None
                break
            if variant in ("osa1", "osa1_casefold") and query.casefold() in {
                declared.casefold() for declared in repository.names
            }:
                record["ineligible"] = "typo_collides_with_declared_name"
                query = None
                break
            rows = repository.variant_rows(variant, query)
            if rows > MAX_TASK_LABELS:
                record["ineligible"] = "too_many_labels"
                query = None
                break
            record.update(query=query, attempt=attempt, matched_rows=rows)
            used.add((intent, query))
            break
        records.append(record)
        if query is not None:
            filled[lane].append(
                _task(
                    repository, lane.split("_")[1][:3], len(filled[lane]) + 1, family, intent, query
                )
            )
    # All typo draws come from an exact-definition family, so clean and noisy
    # queries can be compared without inferring a missing base task. One draw
    # per operation keeps the edit classes balanced before source admissions.
    typo_intent = VARIANT_LANES["variant_osa1"][1]
    typo_variant = gold_oracle.DECLARATION_INTENTS[typo_intent]
    folded_declared = {name.casefold() for name in repository.names}
    for name in paired_bases:
        family = f"{repository.name}.name.{name}"
        for operation in identifier_robustness_suite.TYPO_OPERATIONS:
            if len(filled["variant_osa1"]) >= quotas["variant_osa1"]:
                break
            record = {"lane": "variant_osa1", "base_name": name, "operation": operation}
            query = None
            for attempt in range(MAX_ATTEMPTS):
                candidate, meta = identifier_robustness_suite.propose_typo_operation(
                    operation, name, seed, family, attempt
                )
                if candidate is None:
                    record["ineligible"] = meta.get("ineligible", "no_variant")
                    break
                try:
                    source_oracle._require_query(
                        gold_oracle._name_contract(repository.language, typo_intent), candidate
                    )
                except source_oracle.SourceOracleError:
                    record["ineligible"] = "outside_query_contract"
                    continue
                if candidate.casefold() in folded_declared:
                    record["ineligible"] = "typo_collides_with_declared_name"
                    continue
                if candidate.casefold() in repository.folded_words:
                    record["ineligible"] = "typo_collides_with_content_token"
                    continue
                if (typo_intent, candidate) in used:
                    record["ineligible"] = "duplicate_query"
                    continue
                rows = repository.variant_rows(typo_variant, candidate)
                if not 0 < rows <= MAX_TASK_LABELS:
                    record["ineligible"] = "base_not_gold_or_too_many_labels"
                    continue
                query = candidate
                near_names = sorted(
                    declared
                    for declared in repository.names
                    if source_oracle._variant_matches("osa1_casefold", query, declared)
                )
                record.update(
                    query=query,
                    intended_name=name,
                    attempt=attempt,
                    matched_rows=rows,
                    near_declaration_names=near_names,
                    other_near_declaration_names=[
                        declared for declared in near_names if declared != name
                    ],
                    user_intent_state="unjudged",
                )
                used.add((typo_intent, query))
                break
            records.append(record)
            if query is not None:
                filled["variant_osa1"].append(
                    _task(
                        repository,
                        "osa",
                        len(filled["variant_osa1"]) + 1,
                        family,
                        typo_intent,
                        query,
                        intended_name=name,
                    )
                )
    for lane in VARIANT_LANES:
        ledger[lane] = {
            "population": len(eligible),
            "attempted_base_names": sum(1 for row in records if row["lane"] == lane),
            "ineligible": dict(
                sorted(
                    _tally(
                        row["ineligible"]
                        for row in records
                        if row["lane"] == lane and "ineligible" in row
                    ).items()
                )
            ),
        }
        tasks.extend(filled[lane])
    ledger["variant_records"] = records
    return tasks


def _tally(values) -> dict[str, int]:
    counts: dict[str, int] = defaultdict(int)
    for value in values:
        counts[value] += 1
    return dict(counts)


def _no_answer(
    repository: Repository,
    others: list[Repository],
    seed: int,
    ledger: dict,
    quotas: dict[str, int],
    require_default_absence: bool,
) -> list[dict]:
    tasks: list[dict] = []
    if repository.audit is None:
        for lane in ("no_answer_synthetic", "no_answer_wrong_repository"):
            ledger[lane] = {"population": 0, "underfill": "language_without_declaration_census"}
        return tasks
    declared_folded = {name.casefold() for name in repository.names}
    vocabulary = sorted(
        {
            part
            for name in repository.names
            for part in source_oracle.name_components(name)
            if part.isalpha() and len(part) >= 3
        }
    )
    attempts = 0
    while len(tasks) < quotas["no_answer_synthetic"] and vocabulary and attempts < 2000:
        first = vocabulary[_draw(seed, repository.name, "noa", "first", attempts) % len(vocabulary)]
        second = vocabulary[
            _draw(seed, repository.name, "noa", "second", attempts) % len(vocabulary)
        ]
        attempts += 1
        probe = first.capitalize() + second.capitalize()
        if (
            first == second
            or probe.casefold() in declared_folded
            or any(task["query"] == probe for task in tasks)
            or not (
                repository.default_file_search_absent(probe)
                if require_default_absence
                else repository.content_absent(probe)
            )
        ):
            continue
        tasks.append(
            _task(
                repository,
                "noa",
                len(tasks) + 1,
                f"{repository.name}.noa.{probe}",
                "declaration_name_exact",
                probe,
            )
        )
    ledger["no_answer_synthetic"] = {
        "population": "unbounded_component_recombinations",
        "vocabulary": len(vocabulary),
        "attempts": attempts,
        "rule": (
            "two declared components, absent from folded content/path and OSA1 content tokens"
            if require_default_absence
            else "two declared components, absent as a declaration and as case-folded content"
        ),
    }
    sources = {
        name: other.name
        for other in sorted(others, key=lambda row: row.name)
        for name in other.names
        if source_oracle.IDENTIFIER.fullmatch(name) and len(name) >= MIN_WRONG_REPOSITORY_NAME
    }
    # Population: names absent here as a declaration and as a case-folded word;
    # each admitted name is additionally proven absent as a case-folded substring.
    candidates = [
        name
        for name in _ranked(list(sources), seed, repository.name, "wrong_repository")
        if name.casefold() not in declared_folded and name.casefold() not in repository.folded_words
    ]
    wrong, substring_present = [], 0
    for name in candidates:
        if len(wrong) == quotas["no_answer_wrong_repository"]:
            break
        if (
            repository.default_file_search_absent(name)
            if require_default_absence
            else repository.content_absent(name)
        ):
            wrong.append(name)
        else:
            substring_present += 1
    population = len(candidates)
    for index, name in enumerate(wrong, 1):
        tasks.append(
            _task(
                repository,
                "wrr",
                index,
                f"{repository.name}.wrong.{name}",
                "declaration_name_exact",
                name,
            )
        )
    ledger["no_answer_wrong_repository"] = {
        "population": population,
        "negative_corpus": "other repositories of the same holdout release",
        "rule": (
            "declared elsewhere; absent here as a declaration, folded content/path substring "
            "and OSA1 content token"
            if require_default_absence
            else "declared elsewhere in the release, absent here as a declaration and as a "
            "case-folded word; admitted names are also absent as a case-folded substring"
        ),
        (
            "skipped_default_search_present"
            if require_default_absence
            else "skipped_substring_present"
        ): substring_present,
        "sources": {name: sources[name] for name in wrong},
    }
    return tasks


def build(
    holdout_release: Path,
    development_release: Path,
    seed: int,
    profile: str = "baseline_v3",
) -> tuple[dict, dict[str, dict], bytes, dict]:
    if profile not in PROFILES:
        raise ValueError(f"unknown sampling profile: {profile}")
    quotas = PROFILES[profile]
    holdout = json.loads((holdout_release / "release.json").read_bytes())
    development = json.loads((development_release / "release.json").read_bytes())
    repositories = [Repository(holdout_release, holdout, row) for row in holdout["repositories"]]
    for repository in repositories:
        repository.run_census()
    task_sets: dict[str, list[dict]] = {}
    ledgers: dict[str, dict] = {}
    for repository in repositories:
        ledger: dict[str, Any] = {
            "language": repository.language,
            "repository_commit": repository.commit,
            "code_only_files": len(repository.files),
            "census_audit": None
            if repository.audit is None
            else {
                key: repository.audit[key]
                for key in ("checker", "files", "agreeing_files", "agreeing_declarations", "status")
            }
            | {
                "refused": len(repository.audit["refused"]),
                "disagreements": len(repository.audit["disagreements"]),
            },
        }
        tasks = (
            _literals(repository, seed, ledger, quotas)
            + _declarations(repository, seed, ledger, quotas)
            + _no_answer(
                repository,
                [r for r in repositories if r is not repository],
                seed,
                ledger,
                quotas,
                profile == "scale_diagnostic_v1",
            )
        )
        ledger["natural_language_workflow"] = {
            "population": None,
            "underfill": "requires_two_reviewer_adjudicated_qrels_(C3_not_run)",
        }
        by_lane = _tally(task["task_id"].split(".")[1] for task in tasks)
        lane_codes = {
            "exact_content": "lit",
            "exact_definition": "def",
            "variant_prefix": "pre",
            "variant_infix": "inf",
            "variant_components": "com",
            "variant_osa1": "osa",
            "no_answer_synthetic": "noa",
            "no_answer_wrong_repository": "wrr",
            "natural_language_workflow": None,
        }
        for lane, quota in quotas.items():
            admitted = by_lane.get(lane_codes[lane], 0) if lane_codes[lane] else 0
            population = ledger[lane].get("population")
            probability = (
                admitted / population
                if lane == "exact_definition" and isinstance(population, int) and population
                else None
            )
            ledger[lane].update(
                quota=quota,
                admitted=admitted,
                underfilled=quota - admitted,
                inclusion_probability=probability,
                inclusion_probability_basis=INCLUSION_PROBABILITY_BASIS[lane],
            )
        task_sets[repository.name] = tasks
        ledgers[repository.name] = ledger
    split_rows = [
        {
            "release_digest": holdout["digest"],
            "repository": repository.name,
            "repository_commit": repository.commit,
            "code_only_universe_digest": repository.universe_digest,
            "split": "holdout",
            "query_family_ids": sorted(
                {task["query_family_id"] for task in task_sets[repository.name]}
            ),
        }
        for repository in repositories
    ] + [
        {
            "release_digest": development["digest"],
            "repository": row["recipe"]["name"],
            "repository_commit": row["recipe"]["revision"],
            "code_only_universe_digest": row["views"]["code_only"]["file_universe_digest"],
            "split": "development",
            "query_family_ids": [],
        }
        for row in development["repositories"]
    ]
    sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
    import corpus_binding

    manifest = {
        "schema_version": 1,
        "kind": "repository_disjoint_split_manifest",
        "leakage_policy": corpus_binding.SPLIT_LEAKAGE_POLICY,
        "repositories": sorted(
            split_rows, key=lambda row: (row["release_digest"], row["repository"])
        ),
    }
    manifest_raw = corpus_binding.canonical_json(manifest).encode() + b"\n"
    manifest_sha = hashlib.sha256(manifest_raw).hexdigest()
    recipes = {}
    repositories_by_name = {repository.name: repository for repository in repositories}
    for name, tasks in task_sets.items():
        recipe = {
            "schema_version": 2,
            "split": "holdout",
            "split_manifest_sha256": manifest_sha,
            "tasks": tasks,
        }
        repository = repositories_by_name[name]
        if repository.audit is not None:
            recipe["checker_identity"] = {
                language: audit["checker"] for language, audit in repository.all_audits.items()
            }
        gold_oracle.validate_recipe(recipe)
        recipes[name] = recipe
    summary = {
        "schema_version": 1,
        "kind": "holdout_sampling_ledger",
        "sampling_version": SAMPLING_VERSION if profile == "baseline_v3" else 4,
        "seed": seed,
        "holdout_release_digest": holdout["digest"],
        "development_release_digest": development["digest"],
        "quotas": quotas,
        "split_manifest_sha256": manifest_sha,
        "repositories": ledgers,
        "totals": {
            lane: {
                "quota": quotas[lane] * len(repositories),
                "admitted": sum(ledgers[name][lane]["admitted"] for name in ledgers),
            }
            for lane in quotas
        },
    }
    if profile == "scale_diagnostic_v1":
        summary["profile"] = profile
        underfilled = {
            lane: summary["totals"][lane]["admitted"]
            for lane in SCALE_MINIMUM_LANES
            if summary["totals"][lane]["admitted"] < 1000
        }
        if underfilled:
            raise ValueError(f"scale diagnostic has fewer than 1000 admitted tasks: {underfilled}")
    return summary, recipes, manifest_raw, manifest


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--holdout-release", required=True, type=Path)
    parser.add_argument("--development-release", required=True, type=Path)
    parser.add_argument("--seed", required=True, type=int)
    parser.add_argument("--profile", choices=tuple(PROFILES), default="baseline_v3")
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    output = args.output.resolve()
    root = Path(__file__).resolve().parents[3]
    if output.exists() or output == root or root in output.parents:
        parser.exit(2, "ERROR: output must be new and outside the checkout\n")
    summary, recipes, manifest_raw, _manifest = build(
        args.holdout_release.resolve(),
        args.development_release.resolve(),
        args.seed,
        args.profile,
    )
    output.mkdir(parents=True)
    (output / "recipes").mkdir()
    (output / "split-manifest.json").write_bytes(manifest_raw)
    for name, recipe in recipes.items():
        (output / "recipes" / f"{name}.json").write_bytes(_canonical(recipe))
    tool_files = [
        "tools/benchmark/retrieval/holdout_sampling.py",
        "tools/benchmark/retrieval/gold_oracle.py",
        "tools/benchmark/retrieval/source_oracle.py",
        "tools/benchmark/retrieval/declaration_census_audit.py",
        "tools/benchmark/retrieval/identifier_robustness_suite.py",
        "tools/benchmark/corpus_binding.py",
    ]
    summary["tool_files"] = {
        name: hashlib.sha256((root / name).read_bytes()).hexdigest() for name in tool_files
    }
    (output / "ledger.json").write_bytes(_canonical(summary))
    print(json.dumps(summary["totals"], sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
