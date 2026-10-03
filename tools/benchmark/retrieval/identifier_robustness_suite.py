"""Build preregistered identifier-robustness diagnostic suites from a frozen exact-name suite.

Every choice is a pure function of (seed, family ID, lane, attempt) and frozen source
bytes. The builder never consumes search results; labels are exhaustive source-oracle
rows for each lane's declared contract. Output is diagnostic, not a fresh holdout.
"""

from __future__ import annotations

import argparse
import copy
import hashlib
import json
import re
import sys
from pathlib import Path
from typing import Any

try:
    from tools.benchmark.evidence import read_control
    from tools.benchmark.retrieval import (
        declaration_census_audit,
        evaluator,
        source_oracle,
        source_oracle_suite,
    )
except ModuleNotFoundError:  # direct script invocation
    sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
    from tools.benchmark.evidence import read_control
    from tools.benchmark.retrieval import (
        declaration_census_audit,
        evaluator,
        source_oracle,
        source_oracle_suite,
    )

LANE_VARIANTS = {
    "prefix": ("PFX", "prefix"),
    "infix": ("IFX", "infix"),
    "components": ("CMP", "components"),
    "typo": ("TYP", "osa1_casefold"),
    "no-answer": ("NOA", "exact"),
}
LANGUAGES = tuple(source_oracle.DECLARATION_CENSUS)


def contract_for(language: str, variant: str) -> str:
    matches = [
        contract
        for contract, key in source_oracle.NAME_CONTRACTS.items()
        if key == (language, variant)
    ]
    evaluator.require(len(matches) == 1, f"no unique {language} {variant} name contract")
    return matches[0]


def lanes(language: str) -> dict[str, tuple[str, str]]:
    return {
        lane: (code, contract_for(language, variant))
        for lane, (code, variant) in LANE_VARIANTS.items()
    }


LANES = lanes("go")
# Derived from the declaration-intent no-answer lane: only probes absent from
# every universe file even under case folding. The evaluator independently
# rechecks that content-absence oracle against the frozen source on replay.
CONTENT_NO_ANSWER = ("no-answer-content", "NOC")
TYPO_CONTENT_ABSENCE = ("typo-content-absence", "TNA")
TYPO_OPERATIONS = ("insertion", "deletion", "substitution", "transposition")
STRESS_TYPO_LANES = ("keyboard", "boundary")
LETTERS = "abcdefghijklmnopqrstuvwxyz"
MAX_ATTEMPTS = 8
SHORT_NAME_MAX = 6
SURVIVING_COMPONENT_MIN_LENGTH = 3
TYPO_SOURCE_STRATA_POLICY = {
    "component_tokenizer": source_oracle.COMPONENT_TOKENIZER,
    "min_component_length": SURVIVING_COMPONENT_MIN_LENGTH,
    "component_survival": "casefolded_component_token_identity",
    "tokenizer_scope": "source_oracle_diagnostic_with_inferred_acronym_boundaries",
    "literal_relation": "casefolded_proper_substring",
}
TOOL_FILES = source_oracle_suite.TOOL_FILES + (
    "tools/benchmark/retrieval/identifier_robustness_suite.py",
    "tools/benchmark/retrieval/declaration_census_audit.py",
    *(
        f"tools/benchmark/retrieval/census_checkers/{name}"
        for name in (
            "go_checker.go",
            "go_checker.go.mod",
            "rust_checker.rs",
            "rust_checker.Cargo.toml",
            "rust_checker.Cargo.lock",
            "ts_checker.mjs",
            "ts_checker.package.json",
            "ts_checker.package-lock.json",
        )
    ),
)


def _draw(seed: int, *parts: object) -> int:
    text = ":".join(str(part) for part in (seed, *parts))
    return int.from_bytes(hashlib.sha256(text.encode("utf-8")).digest()[:8], "big")


def sample_families(tasks: list[dict], seed: int, size: int) -> list[str]:
    """Rank every family by a seeded hash; the first `size` form the random sample."""
    ranked = sorted(
        {task["query_family_id"] for task in tasks}, key=lambda f: (_draw(seed, "sample", f), f)
    )
    evaluator.require(len(ranked) >= size, "population is smaller than the requested sample")
    return ranked[:size]


def _name_length_stratum(name: str) -> str:
    if len(name) <= SHORT_NAME_MAX:
        return "short_1_6"
    return "medium_7_16" if len(name) <= 16 else "long_17_plus"


def typo_source_strata(original: str, query: str) -> dict[str, str]:
    """Describe source-text overlap without inferring retrieval cause or user intent."""
    evaluator.require(
        source_oracle.IDENTIFIER.fullmatch(original) is not None
        and source_oracle.IDENTIFIER.fullmatch(query) is not None,
        "typo source strata require ASCII identifiers",
    )
    intended, submitted = original.casefold(), query.casefold()
    evaluator.require(intended != submitted, "noisy query equals intended name after case folding")
    relation = (
        "query_proper_substring"
        if submitted in intended
        else "intended_proper_substring"
        if intended in submitted
        else "neither"
    )
    original_components = {
        part
        for part in source_oracle.name_components(original)
        if len(part) >= SURVIVING_COMPONENT_MIN_LENGTH
    }
    query_components = set(source_oracle.name_components(query))
    survived = len(original_components & query_components)
    survival = (
        "no_eligible_components"
        if not original_components
        else "none"
        if survived == 0
        else "all"
        if survived == len(original_components)
        else "some"
    )
    return {"literal_relation": relation, "surviving_components": survival}


def propose(lane: str, name: str, seed: int, family: str, attempt: int) -> tuple[str | None, dict]:
    """Return one deterministic variant for a base name, or None with an ineligibility reason."""
    draw = _draw(seed, lane, family, attempt)
    if lane == "prefix":
        if len(name) < 4:
            return None, {"ineligible": "name_shorter_than_4"}
        length = 3 + draw % (len(name) - 3)
        return name[:length], {"length": length}
    if lane == "infix":
        if len(name) < 5:
            return None, {"ineligible": "name_shorter_than_5"}
        start = 1 + draw % (len(name) - 3)
        length = 3 + (draw >> 16) % (len(name) - start - 2)
        query = name[start : start + length]
        if name.startswith(query):
            # A repeated head (`StatusStatus` -> `Status`) is textually a prefix.
            return None, {"retry": "infix_equals_prefix", "start": start, "length": length}
        return query, {"start": start, "length": length}
    if lane == "components":
        parts = source_oracle.name_components(name)
        if len(parts) < 2:
            return None, {"ineligible": "single_component"}
        if source_oracle.has_inferred_acronym_boundary(name):
            return None, {"ineligible": "ambiguous_acronym_boundary"}
        return " ".join(parts), {
            "components": len(parts),
            "tokenizer": source_oracle.COMPONENT_TOKENIZER,
        }
    if lane == "typo":
        operation = TYPO_OPERATIONS[draw % len(TYPO_OPERATIONS)]
        letter = LETTERS[(draw >> 8) % len(LETTERS)]
        if operation == "insertion":
            index = (draw >> 16) % (len(name) + 1)
            return name[:index] + letter + name[index:], {"operation": operation, "index": index}
        if operation == "deletion":
            if len(name) < 4:
                return None, {"ineligible": "deletion_below_3_characters", "operation": operation}
            index = (draw >> 16) % len(name)
            return name[:index] + name[index + 1 :], {"operation": operation, "index": index}
        if operation == "substitution":
            index = (draw >> 16) % len(name)
            if name[index].lower() == letter:
                letter = LETTERS[(LETTERS.index(letter) + 1) % len(LETTERS)]
            replacement = letter.upper() if name[index].isupper() else letter
            return name[:index] + replacement + name[index + 1 :], {
                "operation": operation,
                "index": index,
            }
        swaps = [i for i in range(len(name) - 1) if name[i] != name[i + 1]]
        if not swaps:
            return None, {"ineligible": "no_distinct_adjacent_pair", "operation": operation}
        index = swaps[(draw >> 16) % len(swaps)]
        swapped = name[:index] + name[index + 1] + name[index] + name[index + 2 :]
        return swapped, {"operation": operation, "index": index}
    raise evaluator.EvidenceError("unknown robustness lane: " + lane)


def propose_typo_operation(
    operation: str, name: str, seed: int, family: str, attempt: int
) -> tuple[str | None, dict]:
    """Draw a reproducible variant for one specified OSA1 edit operation."""
    evaluator.require(operation in TYPO_OPERATIONS, "unsupported typo operation")
    draw = _draw(seed, "typo", operation, family, attempt)
    letter = LETTERS[(draw >> 8) % len(LETTERS)]
    if operation == "insertion":
        index = (draw >> 16) % (len(name) + 1)
        return name[:index] + letter + name[index:], {"operation": operation, "index": index}
    if operation == "deletion":
        if len(name) < 4:
            return None, {"ineligible": "deletion_below_3_characters", "operation": operation}
        index = (draw >> 16) % len(name)
        return name[:index] + name[index + 1 :], {"operation": operation, "index": index}
    if operation == "substitution":
        index = (draw >> 16) % len(name)
        if name[index].lower() == letter:
            letter = LETTERS[(LETTERS.index(letter) + 1) % len(LETTERS)]
        replacement = letter.upper() if name[index].isupper() else letter
        return name[:index] + replacement + name[index + 1 :], {
            "operation": operation,
            "index": index,
        }
    swaps = [index for index in range(len(name) - 1) if name[index] != name[index + 1]]
    if not swaps:
        return None, {"ineligible": "no_distinct_adjacent_pair", "operation": operation}
    index = swaps[(draw >> 16) % len(swaps)]
    return name[:index] + name[index + 1] + name[index] + name[index + 2 :], {
        "operation": operation,
        "index": index,
    }


def propose_two_substitutions(
    name: str, seed: int, family: str, attempt: int
) -> tuple[str | None, dict]:
    """Draw two distinct casefolded substitutions for an unscored stress probe."""
    if len(name) < 3:
        return None, {"ineligible": "name_shorter_than_3"}
    draw = _draw(seed, "two-substitutions", family, attempt)
    first = draw % len(name)
    second = (first + 1 + (draw >> 16) % (len(name) - 1)) % len(name)
    changed = list(name)
    for offset, index in enumerate((first, second)):
        letter = LETTERS[(draw >> (24 + 8 * offset)) % len(LETTERS)]
        if name[index].casefold() == letter:
            letter = LETTERS[(LETTERS.index(letter) + 1) % len(LETTERS)]
        changed[index] = letter.upper() if name[index].isupper() else letter
    return "".join(changed), {
        "operation": "two_substitutions",
        "indices": sorted((first, second)),
    }


_KEYBOARD_ROWS = (("qwertyuiop", 0.0), ("asdfghjkl", 0.5), ("zxcvbnm", 1.5))
_KEYBOARD_POSITIONS = {
    letter: (float(row), column + offset)
    for row, (letters, offset) in enumerate(_KEYBOARD_ROWS)
    for column, letter in enumerate(letters)
}
_KEYBOARD_NEIGHBORS = {
    letter: tuple(
        sorted(
            other
            for other, (other_row, other_col) in _KEYBOARD_POSITIONS.items()
            if other != letter and (other_row - row) ** 2 + (other_col - col) ** 2 <= 2.25
        )
    )
    for letter, (row, col) in _KEYBOARD_POSITIONS.items()
}


def propose_stress_typo(
    lane: str, name: str, seed: int, family: str, attempt: int
) -> tuple[str | None, dict]:
    """Draw one explicit OSA1 typo at a keyboard or code-token boundary."""
    evaluator.require(lane in STRESS_TYPO_LANES, "unsupported stress typo lane")
    draw = _draw(seed, "stress", lane, family, attempt)
    if lane == "keyboard":
        positions = [
            index for index, char in enumerate(name) if char.lower() in _KEYBOARD_NEIGHBORS
        ]
        if not positions:
            return None, {"ineligible": "no_ascii_keyboard_letter"}
        index = positions[draw % len(positions)]
        char = name[index]
        neighbors = _KEYBOARD_NEIGHBORS[char.lower()]
        replacement = neighbors[(draw >> 16) % len(neighbors)]
        if char.isupper():
            replacement = replacement.upper()
        return name[:index] + replacement + name[index + 1 :], {
            "operation": "keyboard_substitution",
            "index": index,
            "from": char,
            "to": replacement,
        }
    boundaries = [
        index
        for index in range(1, len(name))
        if name[index] == "_" or (name[index].isupper() and name[index - 1].islower())
    ]
    if not boundaries:
        return None, {"ineligible": "no_camel_or_snake_boundary"}
    index = boundaries[draw % len(boundaries)]
    if name[index] == "_":
        return name[:index] + name[index + 1 :], {
            "operation": "snake_boundary_deletion",
            "index": index,
        }
    return name[: index - 1] + name[index] + name[index - 1] + name[index + 1 :], {
        "operation": "camel_boundary_transposition",
        "index": index - 1,
    }


class _Pool:
    """Keep one lane's accepted queries below the evaluator's near-duplicate threshold."""

    def __init__(self) -> None:
        self.normalized: dict[str, str] = {}
        self.shingles: list[tuple[str, set[str]]] = []

    def conflict(self, query: str) -> str | None:
        text = evaluator.normalize_query(query)
        if not text:
            return "empty_after_normalization"
        if text in self.normalized:
            return "pool_duplicate"
        grams = evaluator.query_shingles(text)
        if any(
            evaluator.shingle_jaccard(grams, other) >= evaluator.QUERY_NEAR_DUP_JACCARD
            for _, other in self.shingles
        ):
            return "pool_near_duplicate"
        return None

    def add(self, task_id: str, query: str) -> None:
        text = evaluator.normalize_query(query)
        self.normalized[text] = task_id
        self.shingles.append((task_id, evaluator.query_shingles(text)))


GENERATED_HEADER = re.compile(rb"^// Code generated .* DO NOT EDIT\.$", re.MULTILINE)
# Non-Go generated files are recognized only by an explicit marker comment near
# the top of the file; there is no content heuristic.
GENERATED_MARKER = re.compile(rb"(?m)^\s*(?://|#|/\*|\*).{0,120}(?:@generated|DO NOT EDIT)")
TEST_PATHS = {
    "go": re.compile(r"_test\.go\Z"),
    "rust": re.compile(r"(?:\A|/)(?:tests|benches)/|_tests?\.rs\Z|(?:\A|/)tests?\.rs\Z"),
    "python": re.compile(r"(?:\A|/)(?:tests?/|test_[^/]*\.py\Z|[^/]*_test\.py\Z|conftest\.py\Z)"),
    "typescript": re.compile(r"(?:\A|/)(?:__tests__|tests?)/|\.(?:test|spec)\.tsx?\Z"),
    "javascript": re.compile(r"(?:\A|/)(?:__tests__|tests?)/|\.(?:test|spec)\.[cm]?jsx?\Z"),
}


def generated_files(files: dict[str, tuple[bytes, str]], language: str) -> set[str]:
    if language == "go":
        return {path for path, (raw, _digest) in files.items() if GENERATED_HEADER.search(raw)}
    return {
        path
        for path, (raw, _digest) in files.items()
        if source_oracle.declaration_language(path) == language
        and GENERATED_MARKER.search(raw[:2048])
    }


def _strata(
    task: dict, file_gold: list[dict], generated: set[str], language: str = "go"
) -> dict[str, Any]:
    name = task["query"]
    paths = [row["path"] for row in file_gold]
    test_path = TEST_PATHS[language]
    return {
        "test_only_gold": bool(paths) and all(test_path.search(path) for path in paths),
        "generated_only_gold": bool(paths) and all(path in generated for path in paths),
        "multi_file_gold": len(paths) > 1,
        "length": "short" if len(name) <= SHORT_NAME_MAX else "long",
        "style": "snake"
        if "_" in name.strip("_")
        else "camel"
        if len(source_oracle.name_components(name)) > 1
        else "single",
    }


def _no_answer_probes(
    oracle: source_oracle.SourceOracleIndex,
    names: list[str],
    seed: int,
    count: int,
    language: str = "go",
) -> list[tuple[str, dict]]:
    """Recombine existing components into identifiers that no indexed declaration uses."""
    vocabulary = sorted(
        {
            part
            for name in names
            for part in source_oracle.name_components(name)
            if part.isalpha() and len(part) >= 3
        }
    )
    declared = set(names)
    # A case-only variant of a declaration is not absent for case-insensitive search.
    declared_folded = {name.casefold() for name in names}
    pool, probes, attempt = _Pool(), [], 0
    while len(probes) < count:
        evaluator.require(attempt < count * 200, "cannot derive enough no-answer probes")
        first = vocabulary[_draw(seed, "no-answer", "first", attempt) % len(vocabulary)]
        second = vocabulary[_draw(seed, "no-answer", "second", attempt) % len(vocabulary)]
        attempt += 1
        probe = first.capitalize() + second.capitalize()
        if (
            first == second
            or probe in declared
            or probe.casefold() in declared_folded
            or pool.conflict(probe) is not None
        ):
            continue
        if oracle.expected_rows(contract_for(language, "exact"), probe, "distinct_file"):
            continue
        pool.add(probe, probe)
        probes.append((probe, {"components": [first, second], "attempt": attempt - 1}))
    return probes


def derive(
    repo: Path,
    baseline: dict[str, Any],
    seed: int,
    sample_size: int,
    no_answer: int,
    language: str = "go",
) -> tuple[dict[str, tuple[dict, dict]], dict]:
    evaluator.require(language in LANGUAGES, "unsupported robustness language: " + language)
    language_lanes = lanes(language)
    exact = language_lanes["no-answer"][1]
    _checked, _pack, source = evaluator.validate_suite(repo, baseline, source_oracle_admission=True)
    tasks = baseline["tasks"]
    for task in tasks:
        evaluator.require(
            not (source_oracle_suite.OWNED_TASK_FIELDS & task.keys()),
            f"robustness derivation requires an unannotated task: {task['task_id']}",
        )
    files = {
        e["path"]: (source.file(e["path"])[0], e["file_sha256"]) for e in baseline["file_universe"]
    }
    all_names = {task["query"] for task in tasks}
    oracle = source_oracle.SourceOracleIndex(files, all_names)
    census_audit = None
    if language != "go":
        # The Go v3 census keeps its separately audited contract. Other languages
        # are admitted only when an independent parser agrees on every file.
        census_audit = declaration_census_audit.audit_files(language, repo, sorted(files))
        evaluator.require(
            census_audit["status"] == "admitted"
            and census_audit["file_set_sha256"]
            == declaration_census_audit.file_set_sha256(
                language, {path: raw for path, (raw, _digest) in files.items()}
            ),
            f"{language} declaration census is not independently admitted for this universe",
        )
    declared = oracle.declared_names(language)
    declared_set = set(declared)
    declared_folded = {name.casefold() for name in declared}
    generated = generated_files(files, language)
    base_gold = {
        t["task_id"]: oracle.expected_rows(exact, t["query"], "distinct_file") for t in tasks
    }
    strata = {t["task_id"]: _strata(t, base_gold[t["task_id"]], generated, language) for t in tasks}
    random_sample = set(sample_families(tasks, seed, sample_size))
    multi_file = {t["query_family_id"] for t in tasks if strata[t["task_id"]]["multi_file_gold"]}
    selected = [t for t in tasks if t["query_family_id"] in random_sample | multi_file]
    census: dict[str, Any] = {
        "language": language,
        "declaration_census": source_oracle.DECLARATION_CENSUS[language],
        "census_audit": None
        if census_audit is None
        else {
            key: census_audit[key]
            for key in ("checker", "files", "file_set_sha256", "agreeing_declarations", "status")
        },
        "seed": seed,
        "population_tasks": len(tasks),
        "population_families": len({t["query_family_id"] for t in tasks}),
        "random_sample_families": len(random_sample),
        "multi_file_stratum_families": len(multi_file),
        "overlap_random_and_multi_file": len(random_sample & multi_file),
        "selected_tasks": len(selected),
        "selected_families": len(random_sample | multi_file),
        "generated_files": sorted(generated),
        "declared_names": len(declared),
        "population_strata": _count([strata[t["task_id"]] for t in tasks]),
        "selected_strata": _count([strata[t["task_id"]] for t in selected]),
        "lanes": {},
    }
    outputs: dict[str, tuple[dict, dict]] = {}
    family_prefix = "gin" if language == "go" else language
    for lane, (code, contract) in language_lanes.items():
        suite = copy.deepcopy(baseline)
        suffix = "typo-casefold-v1" if lane == "typo" else lane
        suite["suite_id"] = f"{baseline['suite_id']}-robustness-{suffix}-seed{seed}"
        suite["routes"] = ["lexical"]
        suite["diagnostic_policy"] = evaluator.OBSERVED_PREFIX_DIAGNOSTIC_POLICY
        pool, rows, records = _Pool(), [], []
        if lane == "no-answer":
            probes = _no_answer_probes(oracle, declared, seed, no_answer, language)
            # The word index only covers constructor query names; give probes their own.
            probe_words = source_oracle.SourceOracleIndex(files, {probe for probe, _ in probes})
            items = [
                # Go keeps the historical gin family identity of its frozen suites.
                (f"{code}-{index:03d}", f"{family_prefix}-no-answer-{probe}", probe, meta, None)
                for index, (probe, meta) in enumerate(probes, 1)
            ]
        else:
            items = []
            for task in selected:
                meta, query, rejected = {}, None, []
                for attempt in range(MAX_ATTEMPTS):
                    query, meta = propose(
                        lane, task["query"], seed, task["query_family_id"], attempt
                    )
                    if query is None and "retry" in meta:
                        rejected.append(meta.pop("retry"))
                        continue
                    if query is None:
                        break
                    if lane == "typo" and (
                        source_oracle.IDENTIFIER.fullmatch(query) is None
                        or not 3 <= len(query) <= 64
                        or query.casefold() == task["query"].casefold()
                    ):
                        rejected.append("outside_folded_typo_request")
                        query = None
                        continue
                    meta["attempt"] = attempt
                    reason = pool.conflict(query)
                    if reason is None:
                        pool.add(task["task_id"], query)
                        break
                    rejected.append(reason)
                    query = None
                if query is None and "ineligible" not in meta:
                    meta["ineligible"] = rejected[-1] if rejected else "no_attempt"
                if rejected:
                    meta["rejected_attempts"] = rejected
                items.append(
                    (f"{code}-{task['task_id']}", task["query_family_id"], query, meta, task)
                )
        for task_id, family, query, meta, base in items:
            record = {
                "task_id": task_id,
                "query_family_id": family,
                "query": query,
                "generation": meta,
            }
            if base is not None:
                record.update(
                    base_task_id=base["task_id"],
                    base_query=base["query"],
                    strata=dict(strata[base["task_id"]]),
                )
            if query is None:
                record["status"] = "ineligible"
                records.append(record)
                continue
            # A digit-leading fragment cannot be a declaration name, so it cannot collide.
            if (
                lane == "typo"
                and source_oracle.IDENTIFIER.fullmatch(query)
                and query.casefold() in declared_folded
            ):
                record["status"] = "excluded_exact_name_collision"
                records.append(record)
                continue
            judgments = oracle.expected_rows(contract, query, "distinct_file")
            names = oracle.matched_names(contract, query)
            if lane == "typo" and base is not None and base["query"] not in names:
                record["status"] = "excluded_base_name_not_gold"
                records.append(record)
                continue
            record.update(
                status="admitted",
                answer_class="no_answer"
                if not names
                else "unique"
                if len(names) == 1
                else "ambiguous",
                matched_names=len(names),
                gold_files=len(judgments),
                # A variant that is itself another declaration's exact name.
                query_is_declaration_name=(
                    query.casefold() in declared_folded if lane == "typo" else query in declared_set
                ),
                base_name_in_gold=None if base is None else base["query"] in names,
            )
            if lane == "typo" and base is not None:
                record["strata"].update(typo_source_strata(base["query"], query))
            if lane == "no-answer":
                # The declaration-intent answer is empty; content search may still
                # legitimately return these bytes, scored in a separate content lane.
                record["content_word_files"] = len(
                    probe_words.expected_rows(
                        source_oracle.ASCII_IDENTIFIER_WORD, query, "distinct_file"
                    )
                )
                record["content_substring_files"] = sum(
                    query.encode("ascii") in raw for raw, _digest in files.values()
                )
                record["declaration_infix_names"] = len(
                    oracle.matched_names(language_lanes["infix"][1], query)
                )
                # Case-insensitive or subword systems can legitimately find a
                # case variant (`ReadJson` -> `ReadJSON`), so the content lane
                # also excludes case-folded matches.
                folded = query.casefold().encode("utf-8")
                record["content_substring_files_casefold"] = sum(
                    folded in raw.decode("utf-8", "replace").casefold().encode("utf-8")
                    for raw, _digest in files.values()
                )
                record["declaration_infix_names_casefold"] = sum(
                    query.casefold() in name.casefold() for name in declared
                )
            records.append(record)
            rows.append(
                {
                    "task_id": task_id,
                    "query": query,
                    "query_sha256": evaluator.digest(query.encode("utf-8")),
                    "query_family_id": family,
                    "split": "eval",
                    "query_intent": "bare_symbol",
                    "source_oracle": {"contract": contract, "unit": "distinct_file"},
                    "judgment_policy": evaluator.SOURCE_ORACLE_JUDGMENT_POLICY,
                    "file_judgments": judgments,
                    "gold": evaluator.source_oracle_gold(source, oracle, contract, query),
                    "answerable": bool(judgments),
                }
            )
        suite["tasks"] = rows
        _checked, pack, _source = evaluator.validate_suite(repo, suite)
        outputs[lane] = suite, pack
        census["lanes"][lane] = {
            "contract": contract,
            "admitted": len(rows),
            "status": _tally(records, "status"),
            "answer_class": _tally(
                [r for r in records if r["status"] == "admitted"], "answer_class"
            ),
            "query_is_declaration_name": sum(
                1 for r in records if r.get("query_is_declaration_name")
            ),
            "ineligible": _tally(
                [r["generation"] for r in records if r["status"] == "ineligible"], "ineligible"
            ),
            "records": records,
        }
        if lane == "typo":
            census["lanes"][lane]["source_strata_policy"] = dict(TYPO_SOURCE_STRATA_POLICY)
    lane, code = CONTENT_NO_ANSWER
    outputs[lane], census["lanes"][lane] = _content_no_answer(
        repo, outputs["no-answer"][0], census["lanes"]["no-answer"]["records"], code, seed
    )
    lane, code = TYPO_CONTENT_ABSENCE
    outputs[lane], census["lanes"][lane] = _typo_content_absence(
        repo, outputs["typo"][0], files, code, seed
    )
    return outputs, census


def derive_paired_full(
    repo: Path, baseline: dict[str, Any], seed: int, language: str = "go"
) -> tuple[dict[str, tuple[dict, dict]], dict]:
    """Build OSA1 operation and stress suites paired to exact-name families.

    Each result is a separate, source-bound diagnostic. The clean lane keeps
    the same families for paired deltas. The source-derived near-name labels
    do not claim to know which declaration a human intended.
    """
    evaluator.require(language in LANGUAGES, "unsupported robustness language")
    _checked, _pack, source = evaluator.validate_suite(repo, baseline, source_oracle_admission=True)
    files = {
        entry["path"]: (source.file(entry["path"])[0], entry["file_sha256"])
        for entry in baseline["file_universe"]
    }
    oracle = source_oracle.SourceOracleIndex(files, {task["query"] for task in baseline["tasks"]})
    exact = contract_for(language, "exact")
    near = contract_for(language, "osa1_casefold")
    tasks = baseline["tasks"]
    evaluator.require(
        len({task["query_family_id"] for task in tasks}) == len(tasks),
        "paired robustness requires one exact task per family",
    )
    for task in tasks:
        evaluator.require(
            source_oracle.IDENTIFIER.fullmatch(task["query"]) is not None,
            "paired robustness requires ASCII bare names",
        )
        evaluator.require(
            bool(oracle.expected_rows(exact, task["query"], "distinct_file")),
            "paired robustness base is not a declared name",
        )
    outputs: dict[str, tuple[dict, dict]] = {}
    census: dict[str, Any] = {
        "generation": "paired_full_osa1_casefold_v3",
        "seed": seed,
        "population_families": len(tasks),
        "random_sample_family_ids": sorted(sample_families(tasks, seed, min(300, len(tasks)))),
        "multi_file_family_ids": sorted(
            task["query_family_id"]
            for task in tasks
            if len(oracle.expected_rows(exact, task["query"], "distinct_file")) > 1
        ),
        "lanes": {},
    }
    random_sample = set(census["random_sample_family_ids"])
    multi_file = set(census["multi_file_family_ids"])
    # An exact, valid name that is also near another declared name is a
    # source-verified overcorrection probe in the clean lane. Its user intent
    # still needs review before any navigation relevance claim.
    census["overcorrection_candidates"] = [
        {
            "base_task_id": task["task_id"],
            "query_family_id": task["query_family_id"],
            "query": task["query"],
            "exact_files": [
                row["path"] for row in oracle.expected_rows(exact, task["query"], "distinct_file")
            ],
            "other_near_declaration_names": [
                name for name in oracle.matched_names(near, task["query"]) if name != task["query"]
            ],
            "user_intent_state": "unjudged",
        }
        for task in tasks
        if 3 <= len(task["query"]) <= 64
        and any(name != task["query"] for name in oracle.matched_names(near, task["query"]))
    ]
    overcorrection_families = {
        row["query_family_id"] for row in census["overcorrection_candidates"]
    }
    token_paths = oracle._index_folded_tokens()
    base_strata = {
        task["query_family_id"]: {
            "length": _name_length_stratum(task["query"]),
            "short_common": (
                "yes"
                if len(task["query"]) <= SHORT_NAME_MAX
                and len(token_paths.get(task["query"].casefold(), ())) >= 3
                else "no"
            ),
            "overcorrection_candidate": (
                "yes" if task["query_family_id"] in overcorrection_families else "no"
            ),
        }
        for task in tasks
    }

    # This is a source-bound candidate census, not an OSA1 product suite. A
    # two-edit request needs its own product contract before it can be scored.
    two_edit_records: list[dict[str, Any]] = []
    two_edit_pool = _Pool()
    declared_folded = {name.casefold() for name in oracle.declared_names(language)}
    folded_file_contents = tuple(
        raw.decode("utf-8", "replace").casefold() for raw, _digest in files.values()
    )
    for base in tasks:
        rejected: list[str] = []
        candidate = None
        metadata: dict[str, Any] = {}
        for attempt in range(MAX_ATTEMPTS):
            proposal, metadata = propose_two_substitutions(
                base["query"], seed, base["query_family_id"], attempt
            )
            if proposal is None:
                rejected.append(metadata["ineligible"])
                break
            if source_oracle.IDENTIFIER.fullmatch(proposal) is None or not 3 <= len(proposal) <= 64:
                rejected.append("outside_identifier_request")
                continue
            if source_oracle.osa_distance_at_most_one(
                base["query"].casefold(), proposal.casefold()
            ):
                rejected.append("within_osa1")
                continue
            if proposal.casefold() in declared_folded:
                rejected.append("exact_declaration_collision")
                continue
            if any(proposal.casefold() in text for text in folded_file_contents):
                rejected.append("exact_content_collision")
                continue
            if any(proposal.casefold() in path.casefold() for path in files):
                rejected.append("exact_path_collision")
                continue
            reason = two_edit_pool.conflict(proposal)
            if reason:
                rejected.append(reason)
                continue
            candidate = proposal
            metadata["attempt"] = attempt
            two_edit_pool.add(base["task_id"], proposal)
            break
        two_edit_records.append(
            {
                "base_task_id": base["task_id"],
                "query_family_id": base["query_family_id"],
                "base_query": base["query"],
                "query": candidate,
                "generation": metadata,
                "rejected_attempts": rejected,
                "status": "unjudged_unscored" if candidate is not None else "ineligible",
                "near_declaration_names": (
                    oracle.matched_names(near, candidate) if candidate is not None else []
                ),
                "user_intent_state": "unjudged",
            }
        )
    census["two_substitution_stress"] = {
        "contract": "unscored_two_substitutions_casefold",
        "product_request_mode": "unsupported",
        "admitted_candidates": sum(row["query"] is not None for row in two_edit_records),
        "status": _tally(two_edit_records, "status"),
        "records": two_edit_records,
    }

    clean = copy.deepcopy(baseline)
    clean["suite_id"] = baseline["suite_id"] + f"-robustness-clean-paired-v2-seed{seed}"
    # The clean input is the common default-file-search pair. Explicit typo
    # requests remain single-route because Semble's native lexical-file mode
    # has no matching OSA1 request contract.
    clean["routes"] = ["lexical", "semble-lexical-file"]
    clean["diagnostic_policy"] = evaluator.OBSERVED_PREFIX_DIAGNOSTIC_POLICY
    clean["tasks"] = []
    for base in tasks:
        row = {
            "task_id": "CLN-" + base["task_id"],
            "query": base["query"],
            "query_sha256": evaluator.digest(base["query"].encode("utf-8")),
            "query_family_id": base["query_family_id"],
            "split": "eval",
            "query_intent": "bare_symbol",
            "evaluation_contract": {
                "request_mode": "default_file_search",
                "gold_unit": "distinct_file",
                "result_unit": "distinct_file",
            },
            "source_oracle": {"contract": exact, "unit": "distinct_file"},
            "judgment_policy": evaluator.SOURCE_ORACLE_JUDGMENT_POLICY,
            "file_judgments": oracle.expected_rows(exact, base["query"], "distinct_file"),
            "gold": evaluator.source_oracle_gold(source, oracle, exact, base["query"]),
            "answerable": True,
        }
        clean["tasks"].append(row)
    _checked, pack, _source = evaluator.validate_suite(repo, clean)
    outputs["clean"] = clean, pack
    census["lanes"]["clean"] = {
        "contract": exact,
        "admitted": len(clean["tasks"]),
        "records": [
            {
                "task_id": "CLN-" + base["task_id"],
                "query_family_id": base["query_family_id"],
                "query": base["query"],
                "status": "admitted",
                "answer_class": "unique",
                "matched_names": 1,
                "gold_files": len(oracle.expected_rows(exact, base["query"], "distinct_file")),
                "strata": dict(base_strata[base["query_family_id"]]),
            }
            for base in tasks
        ],
    }

    for operation in (*TYPO_OPERATIONS, *STRESS_TYPO_LANES):
        lane = "typo-" + operation
        code = {
            "insertion": "TYI",
            "deletion": "TYD",
            "substitution": "TYS",
            "transposition": "TYT",
            "keyboard": "TYK",
            "boundary": "TYB",
        }[operation]
        suite = copy.deepcopy(clean)
        suite["suite_id"] = baseline["suite_id"] + f"-robustness-{lane}-casefold-v2-seed{seed}"
        suite["routes"] = ["lexical"]
        rows, records, pool = [], [], _Pool()
        for base in tasks:
            query = None
            rejected: list[str] = []
            rejected_candidates: list[dict[str, Any]] = []
            partition = None
            meta: dict[str, Any] = {}
            for attempt in range(MAX_ATTEMPTS):
                candidate, meta = (
                    propose_typo_operation(
                        operation, base["query"], seed, base["query_family_id"], attempt
                    )
                    if operation in TYPO_OPERATIONS
                    else propose_stress_typo(
                        operation, base["query"], seed, base["query_family_id"], attempt
                    )
                )
                if candidate is None:
                    rejected.append(meta.get("ineligible", "no_variant"))
                    break
                if (
                    source_oracle.IDENTIFIER.fullmatch(candidate) is None
                    or not 3 <= len(candidate) <= 64
                    or candidate.casefold() == base["query"].casefold()
                ):
                    rejected.append("outside_folded_typo_request")
                    rejected_candidates.append({"query": candidate, "reason": rejected[-1]})
                    continue
                partition = oracle.typo_gold_partition(language, candidate, base["query"])
                if partition["query_is_declaration_name"]:
                    rejected.append("exact_declaration_collision")
                    rejected_candidates.append(
                        {
                            "query": candidate,
                            "reason": rejected[-1],
                            "source_partition": partition,
                        }
                    )
                    continue
                if partition["exact_content_collision_paths"]:
                    rejected.append("exact_content_collision")
                    rejected_candidates.append(
                        {
                            "query": candidate,
                            "reason": rejected[-1],
                            "source_partition": partition,
                        }
                    )
                    continue
                reason = pool.conflict(candidate)
                if reason:
                    rejected.append(reason)
                    rejected_candidates.append({"query": candidate, "reason": reason})
                    continue
                query = candidate
                meta["attempt"] = attempt
                pool.add(base["task_id"], query)
                break
            membership = (
                "random_and_multifile"
                if base["query_family_id"] in random_sample & multi_file
                else "random"
                if base["query_family_id"] in random_sample
                else "multifile_extra"
                if base["query_family_id"] in multi_file
                else "population_other"
            )
            record = {
                "task_id": code + "-" + base["task_id"],
                "query_family_id": base["query_family_id"],
                "base_task_id": base["task_id"],
                "base_query": base["query"],
                "query": query,
                "operation": operation,
                "sample_membership": membership,
                "generation": meta,
                "rejected_attempts": rejected,
                "rejected_candidates": rejected_candidates,
                "status": "admitted" if query is not None else "ineligible",
                "strata": dict(base_strata[base["query_family_id"]]),
            }
            if query is None:
                records.append(record)
                continue
            evaluator.require(partition is not None, "admitted typo has no source partition")
            record["source_partition"] = partition
            record["strata"]["near_name_collision"] = (
                "yes" if partition["other_near_declaration_names"] else "no"
            )
            record["strata"].update(typo_source_strata(base["query"], query))
            record["intended_name"] = base["query"]
            record["matched_names"] = 1
            record["gold_files"] = len(partition["intended_base_files"])
            records.append(record)
            judgments = oracle.expected_rows(exact, base["query"], "distinct_file")
            rows.append(
                {
                    "task_id": record["task_id"],
                    "query": query,
                    "query_sha256": evaluator.digest(query.encode("utf-8")),
                    "query_family_id": base["query_family_id"],
                    "split": "eval",
                    "query_intent": "bare_symbol",
                    "intended_name": base["query"],
                    "evaluation_contract": {
                        "request_mode": "explicit_osa1_typo",
                        "gold_unit": "distinct_file",
                        "result_unit": "distinct_file",
                    },
                    "source_oracle": {"contract": exact, "unit": "distinct_file"},
                    "judgment_policy": evaluator.SOURCE_ORACLE_JUDGMENT_POLICY,
                    "file_judgments": judgments,
                    "gold": evaluator.source_oracle_gold(source, oracle, exact, base["query"]),
                    "answerable": bool(judgments),
                }
            )
        suite["tasks"] = rows
        evaluator.require(bool(rows), f"{lane} has no eligible tasks")
        _checked, pack, _source = evaluator.validate_suite(repo, suite)
        outputs[lane] = suite, pack
        census["lanes"][lane] = {
            "scoring_contract": exact,
            "near_declaration_metadata_contract": near,
            "source_strata_policy": dict(TYPO_SOURCE_STRATA_POLICY),
            "gold_kind": "intended_original_name",
            "admitted": len(rows),
            "status": _tally(records, "status"),
            "rejected_attempts": _tally(
                [{"reason": reason} for row in records for reason in row["rejected_attempts"]],
                "reason",
            ),
            "records": records,
        }
        if operation in TYPO_OPERATIONS:
            # Keep the ordinary user-input route distinct from the explicit
            # OSA1 feature. Both use the same frozen queries and intended gold.
            default_lane = "default-" + lane
            default_suite = copy.deepcopy(suite)
            default_suite["suite_id"] = (
                baseline["suite_id"] + f"-robustness-{default_lane}-v1-seed{seed}"
            )
            default_suite["routes"] = ["lexical", "semble-lexical-file"]
            for task in default_suite["tasks"]:
                task["evaluation_contract"]["request_mode"] = "default_file_search"
            _checked, default_pack, _source = evaluator.validate_suite(repo, default_suite)
            outputs[default_lane] = default_suite, default_pack
            census["lanes"][default_lane] = {
                "derived_from": lane,
                "product_request_mode": "default_file_search",
                "admitted": len(rows),
            }
    return outputs, census


def _content_no_answer(
    repo: Path, source_suite: dict, source_records: list[dict], code: str, seed: int
) -> tuple[tuple[dict, dict], dict]:
    """Keep declaration no-answer probes absent from every file, also under case folding."""
    source_code = LANES["no-answer"][0]
    reasons: dict[str, list[str]] = {}
    for record in source_records:
        evaluator.require(record["status"] == "admitted", "no-answer probe was not admitted")
        found = []
        if record["content_substring_files"]:
            found.append("content_bytes_present")
        if record["declaration_infix_names"]:
            found.append("declaration_infix_present")
        if record["content_substring_files_casefold"] and not record["content_substring_files"]:
            found.append("content_bytes_present_casefold")
        if record["declaration_infix_names_casefold"] and not record["declaration_infix_names"]:
            found.append("declaration_infix_present_casefold")
        reasons[record["task_id"]] = found
    suite = copy.deepcopy(source_suite)
    suite["suite_id"] = source_suite["suite_id"].replace(
        f"-robustness-no-answer-seed{seed}", f"-robustness-{CONTENT_NO_ANSWER[0]}-v2-seed{seed}"
    )
    evaluator.require(suite["suite_id"] != source_suite["suite_id"], "suite ID was not derived")
    rows, mapping = [], []
    for task in suite["tasks"]:
        evaluator.require(
            task["task_id"].startswith(source_code + "-") and not task["answerable"],
            "content no-answer lane requires declaration no-answer tasks",
        )
        if reasons[task["task_id"]]:
            continue
        task_id = code + task["task_id"][len(source_code) :]
        mapping.append({"task_id": task_id, "source_task_id": task["task_id"]})
        rows.append(
            {
                **task,
                "task_id": task_id,
                "source_oracle": {
                    "contract": source_oracle.ASCII_CONTENT_ABSENT_CASEFOLD,
                    "unit": "distinct_file",
                },
            }
        )
    evaluator.require(bool(rows), "no content-absent probes were admitted")
    suite["tasks"] = rows
    _checked, pack, _source = evaluator.validate_suite(repo, suite)
    excluded = [
        {"source_task_id": task_id, "query": record["query"], "reasons": reasons[task_id]}
        for record in source_records
        if reasons[(task_id := record["task_id"])]
    ]
    return (suite, pack), {
        "contract": source_oracle.ASCII_CONTENT_ABSENT_CASEFOLD,
        "derived_from": "no-answer",
        "criteria": {
            "content_substring_files": 0,
            "content_substring_files_casefold": 0,
            "declaration_infix_names": 0,
            "declaration_infix_names_casefold": 0,
        },
        "source_admitted": len(source_records),
        "admitted": len(rows),
        "excluded": len(excluded),
        "excluded_reasons": _tally(
            [{"reason": reason} for row in excluded for reason in row["reasons"]], "reason"
        ),
        "excluded_probes": excluded,
        "records": mapping,
    }


def _typo_content_absence(
    repo: Path,
    source_suite: dict,
    files: dict[str, tuple[bytes, str]],
    code: str,
    seed: int,
) -> tuple[tuple[dict, dict], dict]:
    """Reuse typo queries as hard negative default-search cases, with separate labels."""
    source_code = LANES["typo"][0]
    queries = {task["query"] for task in source_suite["tasks"]}
    oracle = source_oracle.SourceOracleIndex(files, queries)
    suite = copy.deepcopy(source_suite)
    suite["suite_id"] = source_suite["suite_id"].replace(
        f"-robustness-typo-casefold-v1-seed{seed}",
        f"-robustness-{TYPO_CONTENT_ABSENCE[0]}-casefold-source-v1-seed{seed}",
    )
    evaluator.require(suite["suite_id"] != source_suite["suite_id"], "suite ID was not derived")
    rows, mapping, excluded = [], [], []
    for task in suite["tasks"]:
        evaluator.require(
            task["task_id"].startswith(source_code + "-") and task["answerable"],
            "typo absence lane requires admitted typo tasks",
        )
        try:
            evaluator.require(
                not oracle.expected_rows(
                    source_oracle.ASCII_CODE_SEARCH_ABSENT_CASEFOLD,
                    task["query"],
                    "distinct_file",
                ),
                "absent oracle returned a file",
            )
        except source_oracle.SourceOracleError as exc:
            excluded.append(
                {"source_task_id": task["task_id"], "query": task["query"], "reason": str(exc)}
            )
            continue
        source_task_id = task["task_id"]
        task_id = code + source_task_id[len(source_code) :]
        mapping.append({"task_id": task_id, "source_task_id": source_task_id})
        rows.append(
            {
                **task,
                "task_id": task_id,
                "source_oracle": {
                    "contract": source_oracle.ASCII_CODE_SEARCH_ABSENT_CASEFOLD,
                    "unit": "distinct_file",
                },
                "file_judgments": [],
                "gold": [],
                "answerable": False,
            }
        )
    evaluator.require(bool(rows), "no typo near-miss absent probes were admitted")
    suite["tasks"] = rows
    _checked, pack, _source = evaluator.validate_suite(repo, suite)
    return (suite, pack), {
        "contract": source_oracle.ASCII_CODE_SEARCH_ABSENT_CASEFOLD,
        "derived_from": "typo",
        "criteria": {"content_substring_files_casefold": 0, "path_substring_files_casefold": 0},
        "source_admitted": len(source_suite["tasks"]),
        "admitted": len(rows),
        "excluded": len(excluded),
        "excluded_reasons": _tally(excluded, "reason"),
        "excluded_probes": excluded,
        "records": mapping,
    }


def _tally(rows: list[dict], key: str) -> dict[str, int]:
    counts: dict[str, int] = {}
    for row in rows:
        counts[str(row.get(key))] = counts.get(str(row.get(key)), 0) + 1
    return dict(sorted(counts.items()))


def _count(rows: list[dict]) -> dict[str, dict[str, int]]:
    return {
        key: _tally(rows, key)
        for key in ("test_only_gold", "generated_only_gold", "multi_file_gold", "length", "style")
    }


def write(
    repo: Path,
    baseline_path: Path,
    output_root: Path,
    seed: int,
    sample_size: int,
    no_answer: int,
    language: str = "go",
    paired_full: bool = False,
) -> dict:
    tool_root = Path(__file__).resolve().parents[3]
    tool_files = [
        {"path": n, "sha256": evaluator.digest((tool_root / n).read_bytes())} for n in TOOL_FILES
    ]
    baseline_bytes = read_control(baseline_path)
    baseline = source_oracle_suite._baseline_from_bytes(baseline_bytes)
    suites, census = (
        derive_paired_full(repo, baseline, seed, language)
        if paired_full
        else derive(repo, baseline, seed, sample_size, no_answer, language)
    )
    contents: dict[str, bytes] = {"census.json": source_oracle_suite._json_bytes(census)}
    for lane, (suite, pack) in suites.items():
        contents[f"{lane}-suite.json"] = source_oracle_suite._json_bytes(suite)
        contents[f"{lane}-blind-pack.json"] = source_oracle_suite._json_bytes(pack)
    for item in tool_files:
        raw = (tool_root / item["path"]).read_bytes()
        evaluator.require(
            evaluator.digest(raw) == item["sha256"], "tool source changed during derivation"
        )
        contents["tool-sources/" + item["path"]] = raw
    manifest = {
        "schema_version": 1,
        "qualification": "diagnostic_unqualified_source_exposed",
        "repository_commit": baseline["repository_commit"],
        "input_suite_sha256": evaluator.digest(baseline_bytes),
        "parameters": {
            "language": language,
            "seed": seed,
            "sample_size": sample_size,
            "no_answer": no_answer,
            "max_attempts": MAX_ATTEMPTS,
            "paired_full": paired_full,
            "case_policy": "typo_casefold_other_name_variants_case_sensitive_except_components",
            "component_tokenizer": source_oracle.COMPONENT_TOKENIZER,
        },
        "tool_files": tool_files,
        "artifacts": sorted(
            ({"path": k, "sha256": evaluator.digest(v)} for k, v in contents.items()),
            key=lambda a: a["path"],
        ),
    }
    evaluator.require(
        output_root.is_absolute() and not output_root.exists(),
        "output root must be absolute and new",
    )
    parent = output_root.parent.resolve(strict=True)
    for checkout in (repo.resolve(), tool_root.resolve()):
        evaluator.require(
            parent != checkout and checkout not in parent.parents,
            "output root must be outside checkouts",
        )
    output_root.mkdir()
    for name, raw in contents.items():
        path = output_root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        with path.open("xb") as stream:
            stream.write(raw)
    with (output_root / "manifest.json").open("xb") as stream:
        stream.write(source_oracle_suite._json_bytes(manifest))
    return manifest


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--repo", required=True, type=Path)
    parser.add_argument("--baseline-suite", required=True, type=Path)
    parser.add_argument("--output-root", required=True, type=Path)
    parser.add_argument("--seed", required=True, type=int)
    parser.add_argument("--sample-size", type=int, default=300)
    parser.add_argument("--no-answer", type=int, default=100)
    parser.add_argument("--language", choices=LANGUAGES, default="go")
    parser.add_argument("--paired-full", action="store_true")
    args = parser.parse_args()
    try:
        manifest = write(
            args.repo.resolve(),
            args.baseline_suite,
            args.output_root,
            args.seed,
            args.sample_size,
            args.no_answer,
            args.language,
            args.paired_full,
        )
    except (OSError, ValueError, source_oracle.SourceOracleError) as exc:
        parser.exit(2, f"ERROR: {exc}\n")
    print(
        json.dumps(
            {"artifacts": len(manifest["artifacts"]), "parameters": manifest["parameters"]},
            sort_keys=True,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
