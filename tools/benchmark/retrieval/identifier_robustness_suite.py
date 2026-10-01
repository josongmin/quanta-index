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
    from tools.benchmark.retrieval import evaluator, source_oracle, source_oracle_suite
except ModuleNotFoundError:  # direct script invocation
    sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
    from tools.benchmark.evidence import read_control
    from tools.benchmark.retrieval import evaluator, source_oracle, source_oracle_suite

LANES = {
    "prefix": ("PFX", source_oracle.GO_NAME_PREFIX),
    "infix": ("IFX", source_oracle.GO_NAME_INFIX),
    "components": ("CMP", source_oracle.GO_NAME_COMPONENTS),
    "typo": ("TYP", source_oracle.GO_NAME_OSA1),
    "no-answer": ("NOA", source_oracle.GO_EXACT_LOCAL_NAME),
}
# Derived from the declaration-intent no-answer lane: only probes absent from
# every universe file even under case folding. The evaluator independently
# rechecks that content-absence oracle against the frozen source on replay.
CONTENT_NO_ANSWER = ("no-answer-content", "NOC")
TYPO_OPERATIONS = ("insertion", "deletion", "substitution", "transposition")
LETTERS = "abcdefghijklmnopqrstuvwxyz"
MAX_ATTEMPTS = 8
SHORT_NAME_MAX = 6
TOOL_FILES = source_oracle_suite.TOOL_FILES + (
    "tools/benchmark/retrieval/identifier_robustness_suite.py",
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


def _strata(task: dict, file_gold: list[dict], generated: set[str]) -> dict[str, Any]:
    name = task["query"]
    paths = [row["path"] for row in file_gold]
    return {
        "test_only_gold": bool(paths) and all(path.endswith("_test.go") for path in paths),
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
    oracle: source_oracle.SourceOracleIndex, names: list[str], seed: int, count: int
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
        if oracle.expected_rows(source_oracle.GO_EXACT_LOCAL_NAME, probe, "distinct_file"):
            continue
        pool.add(probe, probe)
        probes.append((probe, {"components": [first, second], "attempt": attempt - 1}))
    return probes


def derive(
    repo: Path, baseline: dict[str, Any], seed: int, sample_size: int, no_answer: int
) -> tuple[dict[str, tuple[dict, dict]], dict]:
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
    declared = sorted({token.decode("utf-8") for token in oracle._index_go_declarations()})
    declared_set = set(declared)
    generated = {path for path, (raw, _digest) in files.items() if GENERATED_HEADER.search(raw)}
    base_gold = {
        t["task_id"]: oracle.expected_rows(
            source_oracle.GO_EXACT_LOCAL_NAME, t["query"], "distinct_file"
        )
        for t in tasks
    }
    strata = {t["task_id"]: _strata(t, base_gold[t["task_id"]], generated) for t in tasks}
    random_sample = set(sample_families(tasks, seed, sample_size))
    multi_file = {t["query_family_id"] for t in tasks if strata[t["task_id"]]["multi_file_gold"]}
    selected = [t for t in tasks if t["query_family_id"] in random_sample | multi_file]
    census: dict[str, Any] = {
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
    for lane, (code, contract) in LANES.items():
        suite = copy.deepcopy(baseline)
        suite["suite_id"] = f"{baseline['suite_id']}-robustness-{lane}-seed{seed}"
        suite["routes"] = ["lexical"]
        suite["diagnostic_policy"] = evaluator.OBSERVED_PREFIX_DIAGNOSTIC_POLICY
        pool, rows, records = _Pool(), [], []
        if lane == "no-answer":
            probes = _no_answer_probes(oracle, declared, seed, no_answer)
            # The word index only covers constructor query names; give probes their own.
            probe_words = source_oracle.SourceOracleIndex(files, {probe for probe, _ in probes})
            items = [
                (f"{code}-{index:03d}", f"gin-no-answer-{probe}", probe, meta, None)
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
                    strata=strata[base["task_id"]],
                )
            if query is None:
                record["status"] = "ineligible"
                records.append(record)
                continue
            # A digit-leading fragment cannot be a declaration name, so it cannot collide.
            if (
                lane == "typo"
                and source_oracle.IDENTIFIER.fullmatch(query)
                and oracle.expected_rows(source_oracle.GO_EXACT_LOCAL_NAME, query, "distinct_file")
            ):
                record["status"] = "excluded_exact_name_collision"
                records.append(record)
                continue
            judgments = oracle.expected_rows(contract, query, "distinct_file")
            names = oracle.matched_names(contract, query)
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
                query_is_declaration_name=query in declared_set,
                base_name_in_gold=None if base is None else base["query"] in names,
            )
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
                    oracle.matched_names(source_oracle.GO_NAME_INFIX, query)
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
    lane, code = CONTENT_NO_ANSWER
    outputs[lane], census["lanes"][lane] = _content_no_answer(
        repo, outputs["no-answer"][0], census["lanes"]["no-answer"]["records"], code, seed
    )
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
    repo: Path, baseline_path: Path, output_root: Path, seed: int, sample_size: int, no_answer: int
) -> dict:
    tool_root = Path(__file__).resolve().parents[3]
    tool_files = [
        {"path": n, "sha256": evaluator.digest((tool_root / n).read_bytes())} for n in TOOL_FILES
    ]
    baseline_bytes = read_control(baseline_path)
    baseline = source_oracle_suite._baseline_from_bytes(baseline_bytes)
    suites, census = derive(repo, baseline, seed, sample_size, no_answer)
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
            "seed": seed,
            "sample_size": sample_size,
            "no_answer": no_answer,
            "max_attempts": MAX_ATTEMPTS,
            "case_policy": "case_sensitive_except_components_lowercased",
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
    args = parser.parse_args()
    try:
        manifest = write(
            args.repo.resolve(),
            args.baseline_suite,
            args.output_root,
            args.seed,
            args.sample_size,
            args.no_answer,
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
