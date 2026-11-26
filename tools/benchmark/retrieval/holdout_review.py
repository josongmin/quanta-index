"""Prepare blind file-review forms and reissue reviewed NL file diagnostics.

This is a preparation adapter, not a labeler or a qualification pipeline.
Candidates from retrieval, source alternatives and random controls are pooled
by file. Product identities and pool membership stay in the owner-only custody
document; scores and ranks are rejected. Human decisions must subsequently enter the
existing evaluator judgments and run.py annotation/adjudication receipts.
Diagnostic projection preserves supplied labels and review identities without
attesting human provenance or transferring the original admission receipts.
"""

from __future__ import annotations

import argparse
import copy
import hashlib
import shutil
import sys
from pathlib import Path

from tools.benchmark.evidence import RawFile, _read_control_file, parse_json
from tools.benchmark.retrieval import evaluator, native_file_evidence, query_plan, source_oracle

POOL_KINDS = ("retrieval", "source_alternative", "random_control")
MAX_REVIEW_BYTES = 64 * 1024 * 1024


def bind_supplemental_review_tasks(
    checkout: Path, suite_bytes: bytes, tasks: list[dict]
) -> list[dict]:
    """Bind unjudged supplemental files to the original suite's query and threshold.

    The returned subset cannot establish overall answerability. No judgments or
    original artifacts are changed, and no model is called. An explicit incoming
    threshold must agree with the frozen suite; omission is filled from that suite.
    """
    suite = parse_json(suite_bytes.decode("utf-8"))
    suite, _pack, source = evaluator.validate_suite(checkout, suite)
    original = {task["task_id"]: task for task in suite["tasks"]}
    universe = {row["path"]: row["file_sha256"] for row in suite["file_universe"]}
    require = evaluator.require
    require(isinstance(tasks, list) and bool(tasks), "supplemental tasks missing")
    bound, seen, exported = [], set(), 0
    for task in tasks:
        evaluator.object_keys_optional(
            task,
            ["task_id", "query", "query_sha256", "intent", "provenance", "rubric", "files"],
            ["answerability_min_grade"],
            "supplemental task",
        )
        task_id = evaluator.string(task["task_id"], "supplemental task ID")
        require(task_id not in seen, "duplicate supplemental task")
        seen.add(task_id)
        require(task_id in original, "unknown supplemental task")
        frozen = original[task_id]
        require("file_judgments" in frozen, "supplemental task requires file judgments")
        require(
            all(task[key] == frozen[key] for key in ("query", "query_sha256")),
            "supplemental query differs from frozen suite",
        )
        threshold = evaluator.answerability_min_grade(frozen, task_id)
        if "answerability_min_grade" in task:
            require(
                evaluator.answerability_min_grade(task, task_id) == threshold,
                "supplemental answerability_min_grade differs from frozen suite",
            )
        for key in ("intent", "provenance", "rubric"):
            evaluator.string(task[key], "supplemental " + key)
        files = task["files"]
        require(isinstance(files, list) and bool(files), "supplemental files missing")
        judged = {row["path"] for row in frozen["file_judgments"]}
        paths = set()
        for file in files:
            evaluator.object_keys(
                file,
                ["path", "file_sha256", "source_text", "grade", "unresolved"],
                "supplemental file",
            )
            path = evaluator.string(file["path"], "supplemental path")
            require(path not in paths, "duplicate supplemental pair")
            paths.add(path)
            require(path not in judged, "supplemental pair already judged")
            require(
                path in universe and file["file_sha256"] == universe[path],
                "supplemental source hash/universe mismatch",
            )
            raw = source.file(path)[0]
            exported += len(raw)
            require(exported <= MAX_REVIEW_BYTES, "supplemental source export byte limit exceeded")
            require(
                isinstance(file["source_text"], str) and file["source_text"].encode("utf-8") == raw,
                "supplemental source text differs",
            )
            require(
                file["grade"] is None and file["unresolved"] is None,
                "supplemental input contains decisions",
            )
        row = copy.deepcopy(task)
        row["answerability_min_grade"] = threshold
        bound.append(row)
    return bound


def project_natural_language_file_diagnostic(
    checkout: Path, suite_bytes: bytes, *, suite_id: str
) -> tuple[dict, dict, dict]:
    """Reissue reviewed NL tasks without carrying mixed-suite admission claims.

    The complete input suite must still validate. Every selected query, label,
    family and review identity is preserved; only its request contract changes.
    Review identities remain self-reported and existing receipts still bind the
    original suite, so this projection cannot qualify a comparison.
    """
    original = parse_json(suite_bytes.decode("utf-8"))
    evaluator.validate_suite(checkout, original)
    evaluator.string(suite_id, "projected suite ID")
    evaluator.require(suite_id != original["suite_id"], "projection requires a new suite ID")
    selected = [task for task in original["tasks"] if task.get("query_intent") == "semantic_intent"]
    evaluator.require(bool(selected), "reviewed NL projection has no semantic_intent tasks")
    for task in selected:
        evaluator.require(
            task["split"] == "eval"
            and "source_oracle" not in task
            and "file_judgments" in task
            and task.get("judgment_policy") == evaluator.COMPLETE_JUDGMENT_POLICY
            and task.get("label_review", {}).get("assessment")
            in evaluator.LABEL_REVIEW_ASSESSMENTS[1:],
            "NL projection requires reviewed eval file judgments: " + task["task_id"],
        )
        query_plan.plan_lexical_request("natural_language_file", task["query"])
    projected = copy.deepcopy(original)
    projected["suite_id"] = suite_id
    projected["routes"] = ["lexical", "semble-lexical-file"]
    projected["diagnostic_policy"] = evaluator.OBSERVED_PREFIX_DIAGNOSTIC_POLICY
    projected["tasks"] = copy.deepcopy(selected)
    contract = {
        "request_mode": query_plan.NATURAL_LANGUAGE_FILE_SEARCH,
        "gold_unit": "distinct_file",
        "result_unit": "distinct_file",
    }
    for task in projected["tasks"]:
        task["evaluation_contract"] = dict(contract)
    checked, pack, _source = evaluator.validate_suite(checkout, projected)
    selected_ids = {task["task_id"] for task in selected}
    lineage = {
        "schema_version": 1,
        "status": "diagnostic_unqualified",
        "qualified": False,
        "human_provenance_attested": False,
        "split_admission": "not_carried_forward",
        "review_receipts": "remain_bound_to_original_suite",
        "input_suite_bytes_sha256": evaluator.digest(suite_bytes),
        "input_suite_canonical_sha256": _digest(original),
        "suite_sha256": _digest(checked),
        "blind_pack_sha256": _digest(pack),
        "repository_commit": checked["repository_commit"],
        "file_universe_digest": checked["file_universe_digest"],
        "preserved_task_fields_except": ["evaluation_contract"],
        "selected_task_ids": [task["task_id"] for task in selected],
        "excluded_task_ids": [
            task["task_id"] for task in original["tasks"] if task["task_id"] not in selected_ids
        ],
    }
    return checked, pack, lineage


def write_natural_language_file_diagnostic(
    checkout: Path, suite_path: Path, output: Path, *, suite_id: str
) -> dict:
    """Write the projection to a fresh external root, rechecking its inputs."""
    if not output.is_absolute() or output.exists() or output.is_symlink():
        raise ValueError("NL diagnostic output root must be fresh and absolute")
    target = output.resolve()
    for root in (
        checkout.resolve(),
        suite_path.parent.resolve(),
        Path(__file__).resolve().parents[3],
    ):
        if target.is_relative_to(root) or root.is_relative_to(target):
            raise ValueError("NL diagnostic output root must be external and disjoint")
    sources = [Path(__file__), Path(evaluator.__file__), Path(query_plan.__file__)]
    source_digests = {path.name: evaluator.digest(path.read_bytes()) for path in sources}
    raw = _read_control_file(suite_path)
    suite, pack, lineage = project_natural_language_file_diagnostic(
        checkout, raw, suite_id=suite_id
    )
    evaluator.validate_file_universe(
        evaluator.SourceSnapshot(checkout, suite["repository_commit"]), suite["file_universe"]
    )
    if raw != _read_control_file(suite_path) or source_digests != {
        path.name: evaluator.digest(path.read_bytes()) for path in sources
    }:
        raise ValueError("NL diagnostic input or tool changed during projection")
    lineage["tool_source_sha256"] = source_digests
    output.mkdir(parents=True)
    try:
        for name, payload in (
            ("suite.json", suite),
            ("blind-pack.json", pack),
            ("lineage.json", lineage),
        ):
            (output / name).write_bytes(evaluator.canonical(payload))
    except BaseException:
        shutil.rmtree(output)
        raise
    return lineage


def _digest(value: object) -> str:
    return evaluator.digest(evaluator.canonical(value))


def _same_json_value(left: object, right: object) -> bool:
    """Compare frozen JSON fields without Python's bool/int/float aliases."""
    return evaluator.canonical(left) == evaluator.canonical(right)


def capture_review_pool(
    checkout: Path, suite_path: Path, record_path: Path, *, pool_id: str
) -> tuple[dict, dict, dict]:
    """Harvest a checked file capture for reissuing unjudged review forms.

    One pool represents one recorded route. Ranks, scores and capture identity
    stay out of reviewer input; this never carries existing grades forward or
    attests full external index coverage or independent reviewer provenance.
    """
    evaluator.string(pool_id, "review pool ID")
    before = {p: _read_control_file(p) for p in (suite_path, record_path)}
    suite, pack, record = evaluator.load_evidence(checkout, suite_path, record_path)
    evaluator.require(len(suite["routes"]) == 1, "review pool requires one recorded route")
    tasks = {task["task_id"]: [] for task in pack["tasks"]}
    for row in record["results"]:
        evaluator.require(
            row.get("rank_unit") == "distinct_file",
            "review pool requires native distinct_file results",
        )
        tasks[row["task_id"]] = sorted(
            [{key: item[key] for key in ("path", "file_sha256")} for item in row["candidates"]],
            key=lambda item: item["path"],
        )
    evaluator.require(
        all(before[p] == _read_control_file(p) for p in before),
        "review pool capture input changed during validation",
    )
    pool = {"pool_id": pool_id, "kind": "retrieval", "tasks": tasks}
    custody = {
        "status": "unjudged_preparation",
        "qualified": False,
        "pool_execution_attested": False,
        "human_provenance_attested": False,
        "suite_bytes_sha256": evaluator.digest(before[suite_path]),
        "record_bytes_sha256": evaluator.digest(before[record_path]),
        "query_pack_sha256": _digest(pack),
        "pool_sha256": _digest(pool),
        "route": suite["routes"][0],
    }
    return pack, pool, custody


def capture_native_review_pools(
    checkout: Path,
    suite_path: Path,
    query_pack_path: Path,
    record_path: Path,
    *,
    pool_ids: tuple[str, str, str],
    source_inputs: tuple[Path, Path, Path, Path, str] | None = None,
) -> tuple[dict, list[dict], dict]:
    """Project a checked same-view Native capture into three unjudged file pools.

    The pool IDs are owner-selected opaque names in lexical, Hybrid, ANN order. The
    review forms receive only file identity; route membership remains in custody.
    """
    require = evaluator.require
    require(
        isinstance(pool_ids, (tuple, list)) and len(pool_ids) == 3,
        "Native review requires three pool IDs",
    )
    names = [evaluator.string(value, "Native review pool ID") for value in pool_ids]
    require(len(set(names)) == 3, "Native review pool IDs must differ")
    require(
        all(
            not any(
                route in name.casefold() for route in ("lexical", "hybrid", "dense", "ann", "rrf")
            )
            for name in names
        ),
        "Native review pool IDs must hide route names",
    )
    paths = (suite_path, query_pack_path, record_path)
    require(len(set(paths)) == len(paths), "Native review inputs must be distinct files")
    before = {path: _read_control_file(path) for path in (suite_path, query_pack_path)}
    record = RawFile.capture(record_path)
    suite, pack, capture, source = evaluator._bind_native_file_inputs(
        checkout,
        parse_json(before[suite_path].decode("utf-8")),
        before[query_pack_path],
        parse_json(before[query_pack_path].decode("utf-8")),
        record.consume_seekable(lambda handle: parse_json(handle.read().decode("utf-8"))),
    )
    candidates = native_file_evidence.prepare_review_candidates(
        suite, pack, capture, source, composite_source_inputs=source_inputs
    )
    route_ids = dict(zip(("lexical", "hybrid", "dense"), names, strict=True))
    pools = [
        {"pool_id": pool_id, "kind": "retrieval", "tasks": candidates[route]}
        for route, pool_id in route_ids.items()
    ]
    require(
        all(raw == _read_control_file(path) for path, raw in before.items())
        and RawFile.capture(record_path) == record,
        "Native review capture input changed during validation",
    )
    custody = {
        "status": "unjudged_preparation",
        "qualified": False,
        "pool_execution_attested": False,
        "human_provenance_attested": False,
        "suite_bytes_sha256": evaluator.digest(before[suite_path]),
        "query_pack_bytes_sha256": evaluator.digest(before[query_pack_path]),
        "record_bytes_sha256": record.sha256.removeprefix("sha256:"),
        "query_pack_sha256": _digest(pack),
        "pools_sha256": _digest(pools),
        "route_pool_ids": route_ids,
    }
    return pack, pools, custody


def prepare(
    checkout: Path,
    pack: dict,
    contexts: dict,
    pools: list[dict],
    *,
    seed: int,
    native_capture: dict | None = None,
) -> tuple[list[dict], dict]:
    """Bind the frozen source and retain every pooled file as unjudged.

    The caller supplies authored task provenance/rubrics and actual pool inputs.
    Pool diversity is checked structurally; this function does not attest that
    the declared retrieval methods were run or that controls are irrelevant.
    """
    require = evaluator.require
    evaluator.object_keys(
        pack,
        [
            "schema_version",
            "suite_id",
            "suite_commitment_sha256",
            "repository_commit",
            "tokenizer",
            "tokenizer_budget_version",
            "routes",
            "file_universe",
            "tasks",
            "comparison_contract",
            "file_universe_digest",
        ],
        "review query pack",
    )
    require(type(seed) is int, "review seed must be an integer")
    require(
        type(pack["schema_version"]) is int and pack["schema_version"] == evaluator.SCHEMA_VERSION,
        "review requires the existing current query pack",
    )
    evaluator.sha(pack["suite_commitment_sha256"], "suite commitment")
    evaluator.string(pack["suite_id"], "suite ID")
    require(
        pack["tokenizer"] == evaluator.TOKENIZER
        and pack["tokenizer_budget_version"] == evaluator.TOKENIZER_BUDGET_VERSION,
        "review query pack tokenizer mismatch",
    )
    evaluator.validate_comparison_contract(
        pack["comparison_contract"], "review comparison contract"
    )
    require(
        isinstance(pack["routes"], list)
        and bool(pack["routes"])
        and all(isinstance(route, str) and route.strip() for route in pack["routes"])
        and len(set(pack["routes"])) == len(pack["routes"]),
        "review routes malformed",
    )
    source = evaluator.SourceSnapshot(
        checkout, pack["repository_commit"], max_total_bytes=source_oracle.MAX_SOURCE_BYTES
    )
    universe, ordered = evaluator.validate_file_universe(source, pack["file_universe"])
    require(
        pack["file_universe_digest"] == evaluator.universe_digest(ordered),
        "review file universe digest mismatch",
    )
    tasks = pack["tasks"]
    require(
        isinstance(tasks, list) and 0 < len(tasks) <= source_oracle.MAX_QUERIES,
        "review tasks empty or over limit",
    )
    task_ids = []
    for task in tasks:
        evaluator.object_keys(task, ["task_id", "query", "query_sha256"], "review task")
        task_ids.append(evaluator.string(task["task_id"], "review task ID"))
        query = evaluator.string(task["query"], "review query")
        require(
            task["query_sha256"] == evaluator.digest(query.encode()), "review query hash mismatch"
        )
    require(len(set(task_ids)) == len(task_ids), "duplicate review task")
    require(
        isinstance(contexts, dict) and set(contexts) == set(task_ids),
        "review context task coverage mismatch",
    )
    for context in contexts.values():
        evaluator.object_keys_optional(
            context,
            ["intent", "provenance", "rubric"],
            ["answerability_min_grade"],
            "review context",
        )
        evaluator.answerability_min_grade(context, "review context")
        for field, value in context.items():
            if field != "answerability_min_grade":
                evaluator.string(value, "review context " + field)
    require(isinstance(pools, list) and bool(pools), "review candidate pools missing")
    pool_ids = set()
    candidates = {task_id: {} for task_id in task_ids}
    coverage = {task_id: {kind: set() for kind in POOL_KINDS} for task_id in task_ids}
    for pool in pools:
        evaluator.object_keys(pool, ["pool_id", "kind", "tasks"], "review pool")
        pool_id = evaluator.string(pool["pool_id"], "review pool ID")
        require(pool_id not in pool_ids, "duplicate review pool ID")
        pool_ids.add(pool_id)
        require(pool["kind"] in POOL_KINDS, "unknown review pool kind")
        require(
            isinstance(pool["tasks"], dict) and set(pool["tasks"]) == set(task_ids),
            "review pool task coverage mismatch",
        )
        for task_id, rows in pool["tasks"].items():
            require(isinstance(rows, list), "review pool candidates malformed")
            coverage[task_id][pool["kind"]].add(pool_id)
            for row in rows:
                evaluator.object_keys(row, ["path", "file_sha256"], "review pooled file")
                path = evaluator.string(row["path"], "review pooled path")
                require(
                    path in universe and row["file_sha256"] == universe[path],
                    "review pooled file source hash/universe mismatch",
                )
                candidates[task_id].setdefault(path, set()).add(pool_id)
    for task_id, kinds in coverage.items():
        require(
            len(kinds["retrieval"]) >= 2
            and kinds["source_alternative"]
            and kinds["random_control"],
            "review requires varied retrieval, source and control pools",
        )
        require(candidates[task_id], "review task has no pooled files")
        for kind in ("source_alternative", "random_control"):
            require(
                any(members & kinds[kind] for members in candidates[task_id].values()),
                "review task lacks source alternatives or random controls",
            )
    pool_membership = {
        "pool_kinds": {pool["pool_id"]: pool["kind"] for pool in pools},
        "membership": {
            task_id: {path: sorted(members) for path, members in sorted(rows.items())}
            for task_id, rows in candidates.items()
        },
    }
    inputs = {
        "query_pack_sha256": _digest(pack),
        "contexts_sha256": _digest(contexts),
        "pools_sha256": _digest(pool_membership),
        "seed": seed,
    }
    if native_capture is not None:
        require(
            isinstance(native_capture, dict)
            and native_capture.get("query_pack_sha256") == inputs["query_pack_sha256"]
            and native_capture.get("pools_sha256")
            == _digest([pool for pool in pools if pool["kind"] == "retrieval"]),
            "Native review capture does not bind the issued query pack and retrieval pools",
        )
        evaluator.sha(native_capture.get("record_bytes_sha256"), "Native review capture digest")
        inputs["native_capture"] = native_capture
    review_inputs_sha256 = _digest(inputs)
    # Export complete frozen file text; never silently truncate review evidence.
    paths = sorted({path for rows in candidates.values() for path in rows})
    texts, total_bytes = {}, 0
    for path in paths:
        raw = source.file(path)[0]
        total_bytes += 2 * len(raw) * sum(path in rows for rows in candidates.values())
        require(total_bytes <= MAX_REVIEW_BYTES, "review source export byte limit exceeded")
        try:
            texts[path] = raw.decode("utf-8")
        except UnicodeDecodeError as exc:
            raise evaluator.EvidenceError("review source must be UTF-8: " + path) from exc
    forms = []
    for slot in range(2):
        reviews = []
        for task in tasks:
            task_id = task["task_id"]
            order = sorted(
                candidates[task_id],
                key=lambda path: hashlib.sha256(
                    evaluator.canonical([seed, slot, task_id, path])
                ).digest(),
            )
            reviews.append(
                {
                    **task,
                    **contexts[task_id],
                    "answerable": None,
                    "rationale": "",
                    "files": [
                        {
                            "path": path,
                            "file_sha256": universe[path],
                            "source_text": texts[path],
                            "grade": None,
                            "rationale": "",
                        }
                        for path in order
                    ],
                }
            )
        forms.append(
            {
                "schema_version": 2,
                "status": "unjudged_preparation",
                "form_slot": slot + 1,
                "reviewer_id": None,
                "repository_commit": pack["repository_commit"],
                "file_universe_digest": pack["file_universe_digest"],
                "query_pack_sha256": inputs["query_pack_sha256"],
                "review_inputs_sha256": review_inputs_sha256,
                "reviews": reviews,
            }
        )
    custody = {
        "schema_version": 2,
        "status": "unjudged_preparation",
        "qualified": False,
        "pool_execution_attested": False,
        **inputs,
        "review_inputs_sha256": review_inputs_sha256,
        **pool_membership,
        "form_sha256": [_digest(form) for form in forms],
        "receipt_contract": "run._validate_gold_review_receipt",
    }
    return forms, custody


def write(
    checkout: Path, pack: dict, contexts: dict, pools: list[dict], output: Path, *, seed: int
) -> dict:
    """Write unjudged forms to a fresh external root; custody is owner-only."""
    return _write_review_forms(checkout, pack, contexts, pools, output, seed=seed)


def _write_review_forms(
    checkout: Path,
    pack: dict,
    contexts: dict,
    pools: list[dict],
    output: Path,
    *,
    seed: int,
    native_capture: dict | None = None,
) -> dict:
    if not output.is_absolute() or output.exists() or output.is_symlink():
        raise ValueError("review output root must be fresh and absolute")
    target = output.resolve()
    for root in (checkout.resolve(), Path(__file__).resolve().parents[3]):
        if target.is_relative_to(root) or root.is_relative_to(target):
            raise ValueError("review output root must be external and disjoint")
    forms, custody = prepare(
        checkout,
        pack,
        contexts,
        pools,
        seed=seed,
        native_capture=native_capture,
    )
    output.mkdir(parents=True)
    try:
        for index, form in enumerate(forms, 1):
            (output / f"reviewer-{index}.json").write_bytes(evaluator.canonical(form))
        owner = output / "owner"
        owner.mkdir(mode=0o700)
        (owner / "custody.json").write_bytes(evaluator.canonical(custody))
    except BaseException:
        shutil.rmtree(output)
        raise
    return custody


def write_native_review_forms(
    checkout: Path,
    suite: Path,
    query_pack: Path,
    capture: Path,
    contexts: dict,
    control_pools: list[dict],
    output: Path,
    *,
    seed: int,
    source_inputs: tuple[Path, Path, Path, Path, str] | None = None,
) -> dict:
    """Issue blinded forms with the verified Native capture bound in owner custody."""
    pack, pools, capture_custody = _native_review_inputs(
        checkout,
        suite,
        query_pack,
        capture,
        control_pools,
        source_inputs=source_inputs,
    )
    return _write_review_forms(
        checkout,
        pack,
        contexts,
        pools,
        output,
        seed=seed,
        native_capture=capture_custody,
    )


def _native_review_inputs(
    checkout: Path,
    suite: Path,
    query_pack: Path,
    capture: Path,
    control_pools: list[dict],
    *,
    source_inputs: tuple[Path, Path, Path, Path, str] | None,
) -> tuple[dict, list[dict], dict]:
    """Use the same captured candidates and controls for issue and replay."""
    evaluator.require(
        isinstance(control_pools, list)
        and all(
            isinstance(pool, dict) and pool.get("kind") in ("source_alternative", "random_control")
            for pool in control_pools
        ),
        "Native review controls must be source alternatives or random controls",
    )
    pack, retrieval_pools, capture_custody = capture_native_review_pools(
        checkout,
        suite,
        query_pack,
        capture,
        pool_ids=("route-a", "route-b", "route-c"),
        source_inputs=source_inputs,
    )
    return pack, retrieval_pools + control_pools, capture_custody


def replay_native_review_labels(
    checkout: Path,
    suite_path: Path,
    query_pack_path: Path,
    capture_path: Path,
    *,
    source_inputs: tuple[Path, Path, Path, Path, str],
    contexts: dict,
    control_pools: list[dict],
    completed_forms: list[dict],
    adjudicated_form: dict,
    task_contracts: dict,
    seed: int,
    expected_capture_sha256: str,
) -> dict:
    """Rebuild adjudicated labels from the captured candidate pool and forms."""
    evaluator.require(
        isinstance(source_inputs, tuple) and len(source_inputs) == 5,
        "Native reviewed labels require complete composite source inputs",
    )
    pack, pools, capture_custody = _native_review_inputs(
        checkout,
        suite_path,
        query_pack_path,
        capture_path,
        control_pools,
        source_inputs=source_inputs,
    )
    evaluator.require(
        capture_custody["record_bytes_sha256"] == expected_capture_sha256,
        "Native reviewed labels raw capture digest differs",
    )
    return finalize_file_review_labels(
        checkout,
        pack,
        contexts,
        pools,
        completed_forms,
        adjudicated_form,
        task_contracts,
        seed=seed,
        native_capture=capture_custody,
    )


def validate_completed_forms(
    checkout: Path,
    pack: dict,
    contexts: dict,
    pools: list[dict],
    completed: list[dict],
    *,
    seed: int,
    native_capture: dict | None = None,
) -> dict:
    """Check two completed forms against frozen inputs; expose disagreements.

    Reviewer identities are self-reported. This check cannot establish human
    provenance, complete relevance outside the pool, or adjudicated qrels.
    """
    expected, custody = prepare(
        checkout,
        pack,
        contexts,
        pools,
        seed=seed,
        native_capture=native_capture,
    )
    require = evaluator.require
    require(isinstance(completed, list) and len(completed) == 2, "two review forms required")
    reviewer_ids = []
    for form, template in zip(completed, expected, strict=True):
        require(isinstance(form, dict), "review form malformed")
        require(set(form) == set(template), "review form fields changed")
        reviewer_ids.append(evaluator.string(form["reviewer_id"], "reviewer identity"))
        for key in set(template) - {"reviewer_id", "reviews"}:
            require(
                _same_json_value(form[key], template[key]),
                "review form source binding changed: " + key,
            )
        require(
            isinstance(form["reviews"], list) and len(form["reviews"]) == len(template["reviews"]),
            "review task coverage changed",
        )
        for row, frozen in zip(form["reviews"], template["reviews"], strict=True):
            require(isinstance(row, dict) and set(row) == set(frozen), "review task fields changed")
            for key in set(frozen) - {"answerable", "rationale", "files"}:
                require(
                    _same_json_value(row[key], frozen[key]),
                    "review query/context changed: " + key,
                )
            require(type(row["answerable"]) is bool, "review answerability missing")
            evaluator.string(row["rationale"], "review task rationale")
            require(
                isinstance(row["files"], list) and len(row["files"]) == len(frozen["files"]),
                "review candidate coverage changed",
            )
            has_answer = False
            answer_grade = evaluator.answerability_min_grade(row, "review task")
            for file_row, source_row in zip(row["files"], frozen["files"], strict=True):
                require(
                    isinstance(file_row, dict) and set(file_row) == set(source_row),
                    "review candidate fields changed",
                )
                for key in set(source_row) - {"grade", "rationale"}:
                    require(
                        _same_json_value(file_row[key], source_row[key]),
                        "review source/candidate changed: " + key,
                    )
                grade = file_row["grade"]
                require(type(grade) is int and 0 <= grade <= 3, "review grade must be 0..3")
                evaluator.string(file_row["rationale"], "review file rationale")
                has_answer |= grade >= answer_grade
            require(
                row["answerable"] or not has_answer,
                "review cannot deny answerability with a sufficient-answer grade",
            )
    require(
        evaluator.distinct_review_identities(*reviewer_ids),
        "two distinct reviewer identities required",
    )

    by_slot = [{row["task_id"]: row for row in form["reviews"]} for form in completed]
    disagreements = []
    for task in pack["tasks"]:
        task_id = task["task_id"]
        left, right = (rows[task_id] for rows in by_slot)
        grade_by_slot = [
            {row["path"]: row["grade"] for row in review["files"]} for review in (left, right)
        ]
        file_disagreements = [
            {
                "path": path,
                "file_sha256": file_hash,
                "grades": [grade_by_slot[0][path], grade_by_slot[1][path]],
            }
            for path, file_hash in sorted(
                (row["path"], row["file_sha256"]) for row in left["files"]
            )
            if grade_by_slot[0][path] != grade_by_slot[1][path]
        ]
        if left["answerable"] != right["answerable"] or file_disagreements:
            disagreements.append(
                {
                    "task_id": task_id,
                    "query_sha256": task["query_sha256"],
                    "answerable": [left["answerable"], right["answerable"]],
                    "files": file_disagreements,
                }
            )
    return {
        "schema_version": 1,
        "status": "completed_forms_validated_unqualified",
        "qualified": False,
        "human_provenance_attested": False,
        "pool_execution_attested": False,
        "query_pack_sha256": custody["query_pack_sha256"],
        "review_inputs_sha256": custody["review_inputs_sha256"],
        "template_sha256": custody["form_sha256"],
        "completed_form_sha256": [_digest(form) for form in completed],
        "reviewer_ids": reviewer_ids,
        "task_count": len(pack["tasks"]),
        "disagreements": disagreements,
    }


def finalize_file_review_labels(
    checkout: Path,
    pack: dict,
    contexts: dict,
    pools: list[dict],
    completed: list[dict],
    adjudicated: dict,
    task_contracts: dict,
    *,
    seed: int,
    native_capture: dict | None = None,
) -> dict:
    """Issue NL file-label IR from two reviews and a source-bound decision.

    The adjudicator fills the slot-1 form with a third actual identity. Whole-file
    gold blocks are file witnesses, never declaration/context-span judgments.
    This does not attest human provenance, reviewer independence, pool execution
    or qualification. Native labels carry the original capture digest without
    changing the suite commitment recorded by the SDK.
    """
    checked = validate_completed_forms(
        checkout,
        pack,
        contexts,
        pools,
        completed,
        seed=seed,
        native_capture=native_capture,
    )
    for form in completed:
        for row in form["reviews"]:
            threshold = evaluator.answerability_min_grade(row, "completed review")
            evaluator.require(
                row["answerable"] == any(file["grade"] >= threshold for file in row["files"]),
                "review answerability requires a sufficient pooled file: " + row["task_id"],
            )
    evaluator.require(isinstance(adjudicated, dict), "adjudication form must be an object")
    adjudicator = evaluator.string(adjudicated.get("reviewer_id"), "adjudicator identity")
    evaluator.require(
        evaluator.distinct_review_identities(*checked["reviewer_ids"], adjudicator),
        "adjudicator must differ from both reviewers",
    )
    # Reuse the frozen form/source checks on the actual adjudication form;
    # neither its identity nor any original review is synthesized.
    validate_completed_forms(
        checkout,
        pack,
        contexts,
        pools,
        [adjudicated, completed[1]],
        seed=seed,
        native_capture=native_capture,
    )
    evaluator.require(
        isinstance(task_contracts, dict)
        and set(task_contracts) == {task["task_id"] for task in pack["tasks"]},
        "file review evaluation contract coverage differs",
    )
    source = evaluator.SourceSnapshot(
        checkout, pack["repository_commit"], max_total_bytes=source_oracle.MAX_SOURCE_BYTES
    )
    disputed = {row["task_id"] for row in checked["disagreements"]}
    original_rows = [{row["task_id"]: row for row in form["reviews"]} for form in completed]
    evidence = {
        "query_pack_sha256": checked["query_pack_sha256"],
        "review_inputs_sha256": checked["review_inputs_sha256"],
        "completed_form_sha256": checked["completed_form_sha256"],
        "adjudicated_form_sha256": _digest(adjudicated),
    }
    if native_capture is not None:
        evidence["native_capture_sha256"] = native_capture["record_bytes_sha256"]
    labels = {}
    for row in adjudicated["reviews"]:
        task_id = row["task_id"]
        final_grades = {file["path"]: file["grade"] for file in row["files"]}
        if any(
            original[task_id]["answerable"] != row["answerable"]
            or {file["path"]: file["grade"] for file in original[task_id]["files"]} != final_grades
            for original in original_rows
        ):
            disputed.add(task_id)
        threshold = evaluator.answerability_min_grade(row, "adjudicated task")
        judgments = [
            {key: file[key] for key in ("path", "file_sha256", "grade")}
            for file in sorted(row["files"], key=lambda file: file["path"])
        ]
        contract = evaluator.validate_evaluation_contract(
            {
                "evaluation_contract": task_contracts[task_id],
                "file_judgments": judgments,
                "query_intent": "semantic_intent",
            },
            task_id,
        )
        evaluator.require(
            contract["request_mode"] == query_plan.NATURAL_LANGUAGE_FILE_SEARCH,
            "file review issuer requires the NL file search contract",
        )
        sufficient = [file for file in judgments if file["grade"] >= threshold]
        evaluator.require(
            row["answerable"] == bool(sufficient),
            "adjudicated answerability requires a sufficient pooled file: " + task_id,
        )
        gold = []
        for file in sufficient:
            raw, lines, digest = source.file(file["path"])
            evaluator.require(raw and digest == file["file_sha256"], "file gold source differs")
            gold.append(
                {
                    **file,
                    "block_sha256": digest,
                    "start_byte": 0,
                    "end_byte": len(raw),
                    "start_line": 1,
                    "end_line": len(lines),
                }
            )
        label = {
            "query_intent": "semantic_intent",
            "answerable": row["answerable"],
            "answerability_min_grade": threshold,
            "gold": gold,
            "file_judgments": judgments,
            "judgment_policy": "complete_ranked_pool_v1",
            "evaluation_contract": dict(contract),
        }
        labels[task_id] = {
            **label,
            "label_review": {
                "assessment": "reviewed_ambiguous"
                if task_id in disputed
                else "reviewed_unambiguous",
                "reviewer_id": adjudicator,
                "evidence_sha256": _digest({**evidence, "task_id": task_id, "label": label}),
            },
        }
    return {
        "status": "adjudicated_file_labels_unqualified",
        "qualified": False,
        "human_provenance_attested": False,
        "pool_execution_attested": False,
        "reviewer_ids": checked["reviewer_ids"],
        "adjudicator_id": adjudicator,
        **evidence,
        "task_labels": labels,
    }


def bind_native_reviewed_suite(suite: dict, pack: dict, receipt: dict, capture_sha256: str) -> dict:
    """Overlay adjudicated file labels on the exact unjudged execution inputs."""
    require = evaluator.require
    evidence_fields = (
        "query_pack_sha256",
        "review_inputs_sha256",
        "completed_form_sha256",
        "adjudicated_form_sha256",
        "native_capture_sha256",
    )
    evaluator.object_keys(
        receipt,
        [
            *evidence_fields,
            "status",
            "qualified",
            "human_provenance_attested",
            "pool_execution_attested",
            "reviewer_ids",
            "adjudicator_id",
            "task_labels",
        ],
        "Native reviewed labels receipt",
    )
    evidence = {key: receipt[key] for key in evidence_fields}
    require(
        receipt["status"] == "adjudicated_file_labels_unqualified"
        and receipt["qualified"]
        is receipt["human_provenance_attested"]
        is receipt["pool_execution_attested"]
        is False
        and evidence["query_pack_sha256"] == _digest(pack)
        and evidence["native_capture_sha256"] == capture_sha256,
        "Native reviewed labels differ from the frozen query pack or raw capture",
    )
    reviewers = receipt["reviewer_ids"]
    adjudicator = evaluator.string(receipt["adjudicator_id"], "Native label adjudicator")
    require(
        isinstance(reviewers, list)
        and len(reviewers) == 2
        and all(isinstance(identity, str) and identity.strip() for identity in reviewers)
        and evaluator.distinct_review_identities(*reviewers, adjudicator),
        "Native reviewed labels require two reviewers and a distinct adjudicator",
    )
    forms = evidence["completed_form_sha256"]
    require(isinstance(forms, list) and len(forms) == 2, "Native review form digests are missing")
    form_digests = (*forms, evidence["adjudicated_form_sha256"])
    for digest in (*form_digests, evidence["review_inputs_sha256"]):
        evaluator.sha(digest, "Native review evidence digest")
    require(
        len(set(form_digests)) == 3, "Native labels require distinct review and adjudication forms"
    )
    labels = receipt["task_labels"]
    require(
        isinstance(labels, dict) and set(labels) == {task["task_id"] for task in suite["tasks"]},
        "Native reviewed labels do not cover the frozen tasks",
    )
    tasks = []
    label_fields = native_file_evidence.REVIEW_FILE_LABEL_FIELDS
    for task in suite["tasks"]:
        task_id = task["task_id"]
        require(
            task["split"] == "eval"
            and task["answerable"] is False
            and task["gold"] == []
            and task.keys() & label_fields == {"answerable", "gold"},
            "Native reviewed labels require an unjudged execution suite",
        )
        label = labels[task_id]
        require(
            isinstance(label, dict) and set(label) == label_fields,
            "Native reviewed label fields differ",
        )
        review = label.get("label_review")
        label_data = {key: value for key, value in label.items() if key != "label_review"}
        require(
            isinstance(review, dict)
            and review.get("reviewer_id") == adjudicator
            and review.get("evidence_sha256")
            == _digest(
                {**evidence, "task_id": task_id, "label": label_data},
            ),
            "Native reviewed label evidence differs",
        )
        tasks.append({**task, **label})
    return {
        **suite,
        "diagnostic_policy": evaluator.OBSERVED_PREFIX_DIAGNOSTIC_POLICY,
        "tasks": tasks,
    }


def _prepare_native_command(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(
        prog="holdout_review prepare-native",
        description="Write blinded forms from a same-view Native capture",
    )
    for name in ("repo", "suite", "query-pack", "capture", "contexts", "control-pools", "output"):
        parser.add_argument(f"--{name}", required=True, type=Path)
    parser.add_argument("--seed", required=True, type=int)
    for name in ("source-universe", "source-binding", "base-release", "target-releases"):
        parser.add_argument(f"--{name}", type=Path)
    parser.add_argument("--family-id")
    args = parser.parse_args(argv)
    composite = (
        args.source_universe,
        args.source_binding,
        args.base_release,
        args.target_releases,
        args.family_id,
    )
    if any(value is not None for value in composite) and any(value is None for value in composite):
        parser.error("composite sources require all four source paths and --family-id")
    contexts = parse_json(_read_control_file(args.contexts).decode("utf-8"))
    controls = parse_json(_read_control_file(args.control_pools).decode("utf-8"))
    write_native_review_forms(
        args.repo,
        args.suite,
        args.query_pack,
        args.capture,
        contexts,
        controls,
        args.output,
        seed=args.seed,
        source_inputs=composite if args.family_id is not None else None,
    )
    print(f"wrote unjudged review forms at {args.output}")
    return 0


def main(argv: list[str] | None = None) -> int:
    args_tail = sys.argv[1:] if argv is None else argv
    if args_tail[:1] == ["prepare-native"]:
        return _prepare_native_command(args_tail[1:])
    parser = argparse.ArgumentParser(description="Project reviewed NL tasks to a file diagnostic")
    parser.add_argument("--repo", required=True, type=Path)
    parser.add_argument("--suite", required=True, type=Path)
    parser.add_argument("--suite-id", required=True)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args(args_tail)
    lineage = write_natural_language_file_diagnostic(
        args.repo, args.suite, args.output, suite_id=args.suite_id
    )
    print(f"projected {len(lineage['selected_task_ids'])} diagnostic NL tasks at {args.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
