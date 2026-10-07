"""Read-only ARB file/BCY scorer for frozen Quanta capture arms.

Usage: python -m tools.benchmark.retrieval.arb_official_score --help
Record index: {"records":{"sample-id":{"path":"/capture/record.json",
"sha256":"..."}}}; an optional kind="refusal" denotes a pinned query-plan
refusal artifact. Omitted cases remain NOT_RUN. The output must be outside both
the source checkout and retained b06 archive. BCY remains BLOCKED until the
official archive and extracted corpus bytes have independent custody proof.
The suite's span labels are custody evidence, never ARB relevance gold.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
import sys
import tarfile
from collections import Counter
from pathlib import Path
from typing import Any

from tools.benchmark import evidence as benchmark_evidence
from tools.benchmark.retrieval import arb_corpus_custody, evaluator

ROUTES = ("lexical", "semantic", "hybrid")
BUDGETS = (4000, 8000, 16000, 32000)
RELEASES = ("v2_trace2code", "v2_code2test", "v2_edit2ripple")


class ScoringRefusal(ValueError):
    """An input cannot be bound to the frozen ARB/Quanta contract."""


def require(condition: bool, reason: str) -> None:
    if not condition:
        raise ScoringRefusal(reason)


def sha256(raw: bytes) -> str:
    return hashlib.sha256(raw).hexdigest()


def read_strict(path: Path, expected: str | None = None) -> tuple[Any, bytes]:
    """Read one stable regular control file and reject ambiguous JSON."""
    try:
        raw = benchmark_evidence._read_control_file(path)
        if expected is not None:
            require(
                type(expected) is str and len(expected) == 64, f"invalid expected digest: {path}"
            )
            require(sha256(raw) == expected, f"digest mismatch: {path}")
        value = benchmark_evidence.parse_json(raw.decode("utf-8"))
        benchmark_evidence.canonical_json(value)  # also rejects 1e999 -> infinity
        return value, raw
    except (benchmark_evidence.EvidenceError, UnicodeError) as exc:
        raise ScoringRefusal(f"invalid pinned JSON {path}: {exc}") from exc


def read_pinned(path: Path, expected: str) -> Any:
    return read_strict(path, expected)[0]


def read_regular(path: Path, expected: str | None = None) -> bytes:
    try:
        raw = benchmark_evidence._read_control_file(path)
    except benchmark_evidence.EvidenceError as exc:
        raise ScoringRefusal(f"invalid pinned file {path}: {exc}") from exc
    if expected is not None:
        require(type(expected) is str and len(expected) == 64, f"invalid expected digest: {path}")
        require(sha256(raw) == expected, f"digest mismatch: {path}")
    return raw


def assert_git_source_clean(arb: Path, expected_commit: str) -> None:
    """Reject any source tree other than the pinned, fully clean ARB checkout."""
    require(
        type(expected_commit) is str
        and len(expected_commit) == 40
        and all(char in "0123456789abcdef" for char in expected_commit),
        "invalid pinned ARB commit",
    )

    def git(*args: str) -> str:
        try:
            result = subprocess.run(
                ["git", "--no-optional-locks", "-C", str(arb), *args],
                check=True,
                capture_output=True,
                text=True,
                timeout=30,
            )
        except (OSError, subprocess.CalledProcessError, subprocess.TimeoutExpired) as exc:
            raise ScoringRefusal(f"ARB source git guard failed: {exc}") from exc
        return result.stdout.strip()

    require(
        Path(git("rev-parse", "--show-toplevel")).resolve() == arb.resolve(),
        "ARB source is not its own Git checkout",
    )
    require(git("rev-parse", "HEAD") == expected_commit, "ARB source commit mismatch")
    require(
        not git("status", "--porcelain=v1", "--untracked-files=all"),
        "ARB source has tracked or untracked changes",
    )
    ignored_python = [
        path
        for path in git(
            "ls-files",
            "-z",
            "--others",
            "--ignored",
            "--exclude-standard",
            "--",
            "src/agent_retrieval_bench",
        ).split("\x00")
        if path.endswith(".py")
    ]
    require(not ignored_python, f"ARB source has ignored Python modules: {ignored_python}")


def assert_cached_arb_origins(source: Path) -> None:
    """A cached foreign module must not bypass the pinned source import root."""
    canonical = source.resolve()
    for name, module in tuple(sys.modules.items()):
        if name != "agent_retrieval_bench" and not name.startswith("agent_retrieval_bench."):
            continue
        origin = getattr(getattr(module, "__spec__", None), "origin", None)
        module_file = getattr(module, "__file__", None)
        require(
            type(origin) is str and type(module_file) is str,
            f"ARB module has no file origin: {name}",
        )
        origin_path = Path(origin).resolve()
        require(
            origin_path == Path(module_file).resolve()
            and origin_path.suffix == ".py"
            and origin_path.is_relative_to(canonical),
            f"foreign ARB module origin: {name}: {origin_path}",
        )


def assert_official_source_custody(b06: Path, policy: dict[str, Any]) -> None:
    arb = b06 / "arb"
    source = arb / "src/agent_retrieval_bench"
    assert_git_source_clean(arb, policy["arb"]["commit"])
    assert_cached_arb_origins(source)


def pinned_arb(b06: Path):
    """Import only the ARB modules whose bytes the retained policy pins."""
    policy = read_strict(b06 / "policy.json")[0]
    source = b06 / "arb/src/agent_retrieval_bench"
    assert_official_source_custody(b06, policy)
    for name, expected in policy["arb"]["module_sha256"].items():
        read_regular(source / name, expected)
    # ARB imports sibling modules; this is an explicit, separately pinned source root.
    sys.path.insert(0, str(b06 / "arb/src"))
    from agent_retrieval_bench import baseline, bcy_curve  # type: ignore[import-not-found]

    require(
        Path(baseline.__file__).resolve() == (source / "baseline.py").resolve(),
        "wrong ARB baseline module",
    )
    require(
        Path(bcy_curve.__file__).resolve() == (source / "bcy_curve.py").resolve(),
        "wrong ARB BCY module",
    )
    for name, expected in policy["arb"]["module_sha256"].items():
        read_regular(source / name, expected)
    assert_official_source_custody(b06, policy)
    return policy, baseline, bcy_curve


def project_files(candidates: list[dict[str, Any]], universe: set[str], top_k: int) -> list[str]:
    """ARB b06 collection policy: rank-prefix chunks to first 20 distinct files."""
    require(top_k == 100, "ARB collection policy requires top_k=100")
    require(len(candidates) <= top_k, "candidate count exceeds top_k")
    ranks = [c.get("rank") for c in candidates]
    require(all(type(rank) is int for rank in ranks), "non-integer candidate rank")
    require(sorted(ranks) == list(range(1, len(ranks) + 1)), "candidate ranks are not contiguous")
    paths = []
    seen = set()
    for candidate in sorted(candidates, key=lambda c: c["rank"]):
        path = candidate.get("path")
        require(
            type(path) is str and path in universe, f"candidate outside ARB all_files: {path!r}"
        )
        if path not in seen:
            seen.add(path)
            paths.append(path)
    return paths[:20]


def file_metrics(gold_files: list[str], top_files: list[str], baseline) -> dict[str, float]:
    """Official ARB file primitives on the b06 top-20 projection."""
    gold = set(gold_files)
    require(bool(gold), "no official gold files")
    return {
        "Recall@20": baseline.recall_at(gold, top_files, 20),
        "MRR@20": baseline.reciprocal_rank(gold, top_files),
    }


def score_route(
    result: dict[str, Any], gold: list[str], universe: set[str], top_k: int, baseline
) -> dict[str, Any]:
    """Score a valid retrieval outcome; abstention is observed empty retrieval."""
    status = result["status"]
    require(
        status in ("success", "capped", "abstained", "error", "timeout", "unavailable"),
        "unknown result status",
    )
    candidates = result["candidates"]
    if status in ("error", "timeout", "unavailable"):
        require(not candidates, "failed result carries candidates")
        return {"status": "FAILED", "reason": result.get("error")}
    if status == "abstained":
        require(not candidates, "abstained result carries candidates")
    else:
        require(bool(candidates), "successful result has no candidates")
    top = project_files(candidates, universe, top_k)
    return {
        "status": "COMPLETED",
        "top_files": top,
        "gold_files": gold,
        "metrics": file_metrics(gold, top, baseline),
        "distinct_files_in_k100": len({candidate["path"] for candidate in candidates}),
        "bcy": {
            "status": "BLOCKED",
            "reason": "official archive and extracted corpus bytes have no independent custody proof",
        },
    }


def validate_refusal(payload: dict[str, Any], sample_id: str, adapted_sha: str) -> str:
    """A pinned planning refusal is an attempted case, never an empty result."""
    require(isinstance(payload, dict), "refusal must be an object")
    require(
        type(payload.get("schema_version")) is int and payload["schema_version"] == 1,
        "unsupported refusal schema",
    )
    require(
        payload.get("kind") == "quanta_retrieval_query_plan_refusal", "unsupported refusal kind"
    )
    require(payload.get("phase") == "query_plan", "unsupported refusal phase")
    require(payload.get("task_id") == "ARB-" + sample_id, "refusal task ID mismatch")
    require(payload.get("original_query_sha256") == adapted_sha, "refusal adapted query mismatch")
    error = payload.get("error")
    require(isinstance(error, dict), "refusal error must be an object")
    code = error.get("code")
    require(type(code) is str and bool(code), "refusal has no code")
    return code


def bind_record_identity(record: dict[str, Any], spec: dict[str, Any], arm: dict[str, Any]) -> None:
    """Reject a valid old record replayed into a newer capture arm."""
    runner = record["runner"]
    require(runner["name"] == spec["runner_name"], "record runner name mismatch")
    strategy = spec["strategies"][0]["name"]
    require(runner["run_id"] == f"{spec['run_id']}-{strategy}", "record run ID mismatch")
    require(
        runner["revision"] == "sha256:" + arm["runner_binary_sha256"],
        "record runner binary digest mismatch",
    )
    for field in ("blinding", "isolation_method", "access_block_log"):
        require(runner[field] == spec[field], f"record {field} mismatch")
    require(spec["runner_binary"] == arm["runner_binary"], "arm runner binary path mismatch")
    require(spec["searchd_binary"] == arm["searchd_binary"], "arm searchd binary path mismatch")
    require(
        spec["searchd_expected_sha256"] == arm["searchd_binary_sha256"],
        "arm searchd digest mismatch",
    )
    profile = spec["execution_profiles"]["quanta"]
    for capture in record["captures"].values():
        require(capture["system"] == "quanta", "record contains non-Quanta capture")
        require(capture["execution_profile"] == profile, "record execution profile mismatch")


def bind_identity(
    case: dict[str, Any],
    sample: dict[str, Any],
    suite: dict[str, Any],
    pack: dict[str, Any],
    spec: dict[str, Any],
    source_manifest: dict[str, Any],
    index: dict[str, Any],
    baseline,
) -> list[str]:
    """Bind independent raw sample, official gold, source universe and adapted query."""
    sid = case["sample_id"]
    original_sha = case.get("original_query_sha256", case.get("query_sha256"))
    adapted_sha = case.get("adapted_query_sha256", original_sha)
    raw_text = baseline.query_text_for_eval(sample)
    require(sample.get("id") == sid, "raw sample ID mismatch")
    require(sample.get("repo") == "gin-gonic/gin", "raw sample repository mismatch")
    require(sample.get("base_commit") == case["base_commit"], "raw sample base commit mismatch")
    require(sha256(raw_text.encode()) == original_sha, "raw query digest mismatch")
    gold = baseline.target_gold_files(sample)
    require(bool(gold), "sample has no official gold")
    require(not baseline.query_has_leakage(sample, raw_text), "official query leakage skip")
    expected_index = index[sid]
    require(
        expected_index["base_commit"] == case["base_commit"], "sample index base commit mismatch"
    )
    require(expected_index["query_sha256"] == original_sha, "sample index query digest mismatch")
    require(expected_index["arb_gold_files"] == gold, "sample index official gold mismatch")
    if "official_gold_files_sha256" in case:
        require(
            sha256(json.dumps(gold, ensure_ascii=False, separators=(",", ":")).encode())
            == case["official_gold_files_sha256"],
            "official gold digest mismatch",
        )
    require(suite["repository_commit"] == case["base_commit"], "suite base commit mismatch")
    require(pack["repository_commit"] == case["base_commit"], "pack base commit mismatch")
    require(
        source_manifest["repository_commit"] == case["base_commit"],
        "source manifest base commit mismatch",
    )
    require(
        suite["file_universe"] == source_manifest["files"],
        "suite differs from ARB all_files universe",
    )
    require(
        pack["file_universe"] == source_manifest["files"],
        "pack differs from ARB all_files universe",
    )
    require(len(suite["tasks"]) == len(pack["tasks"]) == 1, "expected one task per ARB case")
    task_id = "ARB-" + sid
    require(
        suite["tasks"][0]["task_id"] == pack["tasks"][0]["task_id"] == task_id, "task ID mismatch"
    )
    require(
        suite["tasks"][0]["query_sha256"] == pack["tasks"][0]["query_sha256"] == adapted_sha,
        "adapted query digest mismatch",
    )
    require(
        sha256(suite["tasks"][0]["query"].encode()) == adapted_sha,
        "suite adapted query bytes mismatch",
    )
    require(
        pack["tasks"][0]["query"] == suite["tasks"][0]["query"], "pack query differs from suite"
    )
    require(
        spec["top_k"] == 100 and spec["routes"] == list(ROUTES), "spec collection policy mismatch"
    )
    require(spec["repo"] and spec["manifest"] == case["manifest"], "spec source binding mismatch")
    require(
        spec["suite"] == case["suite"] and spec["query_pack"] == case["pack"],
        "spec suite/pack binding mismatch",
    )
    require(
        spec["strategies"]
        == [{"name": "fixed_window_strict", "overlap_bytes": 256, "window_bytes": 4096}],
        "spec strategy mismatch",
    )
    return gold


def load_samples(
    b06: Path, policy: dict[str, Any]
) -> tuple[dict[str, dict[str, Any]], dict[str, str]]:
    samples: dict[str, dict[str, Any]] = {}
    release_by_id: dict[str, str] = {}
    for release in RELEASES:
        path = b06 / "data/benchmark" / release / "samples.jsonl"
        expected = policy["arb"]["releases"][release]["samples_jsonl_sha256"]
        raw = read_regular(path, expected)
        selected_count = 0
        for line in raw.splitlines():
            try:
                sample = benchmark_evidence.parse_json(line.decode("utf-8"))
                benchmark_evidence.canonical_json(sample)
            except (benchmark_evidence.EvidenceError, UnicodeError) as exc:
                raise ScoringRefusal(f"invalid official sample JSONL: {release}: {exc}") from exc
            if sample.get("repo") != "gin-gonic/gin":
                continue
            require(
                sample.get("task_type") == release.removeprefix("v2_"),
                f"official sample task type differs from release: {release}",
            )
            sid = sample["id"]
            require(sid not in samples, f"duplicate official sample ID: {sid}")
            samples[sid] = sample
            release_by_id[sid] = release
            selected_count += 1
        require(
            selected_count
            == policy["sample_selection"]["expected_counts"][release.removeprefix("v2_")],
            f"official sample count differs from release: {release}",
        )
    require(
        len(samples) == policy["sample_selection"]["expected_counts"]["total"],
        "official denominator mismatch",
    )
    return samples, release_by_id


def case_context(case: dict[str, Any], sample: dict[str, Any], index: dict[str, Any], baseline):
    spec = read_pinned(Path(case["spec"]), case["spec_sha256"])
    suite = read_pinned(Path(case["suite"]), case["suite_sha256"])
    pack = read_pinned(Path(case["pack"]), case["pack_sha256"])
    source_manifest = read_pinned(Path(case["manifest"]), case["manifest_sha256"])
    gold = bind_identity(case, sample, suite, pack, spec, source_manifest, index, baseline)
    repo = Path(spec["repo"])
    require(repo.is_dir(), f"source checkout missing: {repo}")
    validated_suite, validated_pack, source = evaluator.validate_suite(repo, suite)
    require(validated_pack == pack, "frozen pack differs from validated suite")
    return gold, spec, validated_suite, validated_pack, source, repo


def capture_specs(
    preparation_path: Path | None,
    arm_path: Path,
    arm_sha: str,
    cases: list[dict[str, Any]],
    revision: str,
) -> tuple[dict[str, dict[str, Any]], str | None]:
    """Bind copied capture specs to their frozen prep inputs, allowing four declared edits."""
    if preparation_path is None:
        return {}, None
    preparation, raw = read_strict(preparation_path)
    require(
        preparation["source_revision"] == revision, "capture preparation source revision mismatch"
    )
    matches = [
        item
        for item in preparation["arms"]
        if Path(item["input_manifest"]).resolve() == arm_path.resolve()
        and item["input_manifest_sha256"] == arm_sha
    ]
    require(len(matches) == 1, "capture preparation does not bind exactly one arm")
    prepared = matches[0]
    frozen = {case["sample_id"]: case for case in cases}
    supplied = {case["sample_id"]: case for case in prepared["cases"]}
    require(len(supplied) == len(prepared["cases"]), "duplicate capture preparation case ID")
    require(set(frozen) == set(supplied), "capture preparation case set differs from frozen arm")
    allowed_edits = {"access_block_log", "isolation_method", "output_root", "run_id"}
    out = {}
    for sid, case in frozen.items():
        capture = supplied[sid]
        for field in (
            "manifest",
            "manifest_sha256",
            "suite",
            "suite_sha256",
            "pack",
            "pack_sha256",
        ):
            require(capture[field] == case[field], f"capture preparation {field} mismatch: {sid}")
        require(
            capture["prepared_spec"] == case["spec"]
            and capture["prepared_spec_sha256"] == case["spec_sha256"],
            f"capture preparation frozen spec mismatch: {sid}",
        )
        original = read_pinned(Path(case["spec"]), case["spec_sha256"])
        current = read_pinned(Path(capture["spec"]), capture["spec_sha256"])
        require(
            capture["output_root"] == current["output_root"],
            f"capture preparation output root mismatch: {sid}",
        )
        require(set(original) == set(current), f"capture spec fields differ: {sid}")
        require(
            all(original[key] == current[key] for key in original if key not in allowed_edits),
            f"capture spec has undeclared semantic edits: {sid}",
        )
        require(
            all(type(current[key]) is str and current[key] for key in allowed_edits),
            f"capture spec has invalid execution metadata: {sid}",
        )
        out[sid] = current
    return out, sha256(raw)


def arm_denominators(arm: dict[str, Any], accepted_cases: int) -> tuple[int, int, int]:
    requested = arm.get("requested_denominator", arm.get("denominator_original"))
    accepted = arm.get(
        "current_v3_plan_accepted", arm.get("current_v3_plan_accepted_from_original88")
    )
    refused = arm.get("current_v3_plan_refused", arm.get("current_v3_plan_refused_from_original88"))
    require(
        all(type(value) is int for value in (requested, accepted, refused)),
        "arm denominators must be integers",
    )
    require(
        (requested, accepted, refused) == (88, accepted_cases, 88 - accepted_cases),
        "arm denominator/refusal mismatch",
    )
    return requested, accepted, refused


def score_arm(
    arm_path: Path,
    record_index_path: Path,
    b06: Path,
    preparation_path: Path | None = None,
    corpus_work_root: Path | None = None,
) -> dict[str, Any]:
    arm, arm_raw = read_strict(arm_path)
    record_index, record_index_raw = read_strict(record_index_path)
    require(
        isinstance(record_index, dict) and set(record_index) == {"records"},
        "record index must have exactly the records field",
    )
    records = record_index["records"]
    require(isinstance(records, dict), "record index must map sample IDs to pinned records")
    policy, baseline, bcy_curve = pinned_arb(b06)
    declared_policy_sha = arm.get("original_policy_sha256", arm.get("policy_sha256"))
    policy_raw = read_regular(b06 / "policy.json")
    require(declared_policy_sha == sha256(policy_raw), "arm policy digest mismatch")
    amendment_sha = arm.get("original_policy_amendments_sha256", arm.get("policy_amendment_sha256"))
    require(
        amendment_sha == sha256(read_regular(b06 / "policy_amendments.json")),
        "arm policy amendment digest mismatch",
    )
    samples, release_by_id = load_samples(b06, policy)
    official_sources = arm.get("official_source_files", arm.get("original_samples"))
    require(isinstance(official_sources, dict), "arm has no official sample source map")
    for release in RELEASES:
        declared = official_sources[release]
        expected = policy["arb"]["releases"][release]["samples_jsonl_sha256"]
        require(
            declared.get("samples_sha256", declared.get("sha256")) == expected,
            f"arm sample digest mismatch: {release}",
        )
        declared_path = declared.get("samples_path", declared.get("path"))
        require(
            Path(declared_path).resolve()
            == (b06 / "data/benchmark" / release / "samples.jsonl").resolve(),
            f"arm sample path mismatch: {release}",
        )
    index_rows = read_strict(b06 / "samples_index.json")[0]
    index = {row["sample_id"]: row for row in index_rows}
    require(len(index_rows) == len(index), "duplicate sample index ID")
    require(set(index) == set(samples), "sample index differs from official gin sample IDs")
    cases = arm["cases"]
    case_ids = [case["sample_id"] for case in cases]
    require(
        len(case_ids) == len(set(case_ids)) and set(case_ids) <= set(samples),
        "arm case IDs invalid",
    )
    require(set(records) <= set(case_ids), "record index contains unknown arm sample")
    requested, accepted, refused = arm_denominators(arm, len(cases))
    prepared_specs, preparation_sha = capture_specs(
        preparation_path, arm_path, sha256(arm_raw), cases, arm["source_pair_revision"]
    )
    rows = []
    for case in cases:
        sid = case["sample_id"]
        sample = samples[sid]
        if "source_samples_sha256" in case:
            matched_source = [
                release
                for release in RELEASES
                if Path(case["source_samples_path"]).resolve()
                == (b06 / "data/benchmark" / release / "samples.jsonl").resolve()
                and case["source_samples_sha256"]
                == policy["arb"]["releases"][release]["samples_jsonl_sha256"]
            ]
            require(len(matched_source) == 1, f"case official sample source mismatch: {sid}")
        raw_query = baseline.query_text_for_eval(sample)
        skip_reason = (
            "no_gold"
            if not baseline.target_gold_files(sample)
            else "query_leakage"
            if baseline.query_has_leakage(sample, raw_query)
            else None
        )
        if skip_reason is not None:
            rows.extend(
                {
                    "sample_id": sid,
                    "task_type": sample["task_type"],
                    "route": route,
                    "status": "SKIPPED",
                    "reason": skip_reason,
                }
                for route in ROUTES
            )
            continue
        context = None
        try:
            context = case_context(case, sample, index, baseline)
        except (ScoringRefusal, evaluator.EvidenceError, KeyError, ValueError) as exc:
            rows.extend(
                {
                    "sample_id": sid,
                    "task_type": sample["task_type"],
                    "route": route,
                    "status": "BLOCKED",
                    "reason": str(exc),
                }
                for route in ROUTES
            )
            continue
        gold, spec, suite, pack, source, repo = context
        capture_spec = prepared_specs.get(sid, spec)
        entry = records.get(sid)
        if entry is None:
            rows.extend(
                {
                    "sample_id": sid,
                    "task_type": sample["task_type"],
                    "route": route,
                    "status": "NOT_RUN",
                }
                for route in ROUTES
            )
            continue
        try:
            require(isinstance(entry, dict), "record index entry must be an object")
            kind = entry.get("kind", "record")
            require(kind in ("record", "refusal"), "record index entry has unknown kind")
            required_keys = {"path", "sha256"} | ({"kind"} if "kind" in entry else set())
            require(
                set(entry) == required_keys, "record index entry must have only kind/path/sha256"
            )
            payload = read_pinned(Path(entry["path"]), entry["sha256"])
            if kind == "refusal":
                code = validate_refusal(payload, sid, suite["tasks"][0]["query_sha256"])
                rows.extend(
                    {
                        "sample_id": sid,
                        "task_type": sample["task_type"],
                        "route": route,
                        "status": "REFUSED",
                        "reason": code,
                        "refusal_sha256": entry["sha256"],
                    }
                    for route in ROUTES
                )
                continue
            record = payload
            record = evaluator.validate_evidence_against_suite(repo, suite, pack, source, record)
            bind_record_identity(record, capture_spec, arm)
            results = {result["route"]: result for result in record["results"]}
            require(set(results) == set(ROUTES), "record route set mismatch")
        except (ScoringRefusal, evaluator.EvidenceError, KeyError, ValueError) as exc:
            rows.extend(
                {
                    "sample_id": sid,
                    "task_type": sample["task_type"],
                    "route": route,
                    "status": "BLOCKED",
                    "reason": str(exc),
                }
                for route in ROUTES
            )
            continue
        universe = {item["path"] for item in suite["file_universe"]}
        for route in ROUTES:
            result = results[route]
            row: dict[str, Any] = {
                "sample_id": sid,
                "task_type": sample["task_type"],
                "route": route,
                "result_status": result["status"],
                "record_sha256": entry["sha256"],
            }
            try:
                row.update(score_route(result, gold, universe, spec["top_k"], baseline))
            except (ScoringRefusal, KeyError, ValueError) as exc:
                row.update(status="BLOCKED", reason=str(exc))
            rows.append(row)
    require(len(rows) == len(cases) * len(ROUTES), "incomplete route accounting")
    corpus = None
    corpus_reason = "official archive and extracted corpus bytes have no independent custody proof"
    if corpus_work_root is not None:
        selected_by_release = {release: set() for release in RELEASES}
        for case in cases:
            sid = case["sample_id"]
            selected_by_release[release_by_id[sid]].add(samples[sid]["base_commit"])
        try:
            corpus = arb_corpus_custody.restore(b06, policy, selected_by_release, corpus_work_root)
        except (arb_corpus_custody.CorpusBlocked, OSError, ValueError, tarfile.TarError) as exc:
            corpus_reason = str(exc)
    if corpus is not None:
        cache = arb_corpus_custody.verified_file_cache(corpus, bcy_curve)
        for row in rows:
            if row["status"] != "COMPLETED":
                continue
            sample = samples[row["sample_id"]]
            detail = {
                "sample_id": row["sample_id"],
                "task_type": row["task_type"],
                "repo": sample["repo"],
                "base_commit": sample["base_commit"],
                "gold_files": row["gold_files"],
                "top_files": row["top_files"],
            }
            row["bcy_detail"] = detail
            try:
                evaluated = bcy_curve.evaluate_sample(
                    detail, row["gold_files"], cache, BUDGETS, (1,)
                )
                row["bcy"] = {
                    "status": "VERIFIED",
                    "budgets": {
                        str(budget): evaluated["packed"][budget]["bcy"] for budget in BUDGETS
                    },
                }
            except (OSError, ValueError, KeyError, UnicodeError) as exc:
                row["bcy"] = {"status": "BLOCKED", "reason": str(exc)}
    summaries = {}
    for route in ROUTES:
        selected = [row for row in rows if row["route"] == route]
        counts = Counter(row["status"] for row in selected)
        complete = [row for row in selected if row["status"] == "COMPLETED"]
        metrics = {
            name: (
                sum(row["metrics"][name] for row in complete) / len(complete) if complete else None
            )
            for name in ("Recall@20", "MRR@20")
        }
        bcy_complete = [row for row in complete if row["bcy"]["status"] == "VERIFIED"]
        bcy_summary: dict[str, Any] = {
            "denominator": 0,
            "status": "BLOCKED",
            "reason": corpus_reason if corpus is None else "no complete BCY cohort",
            "budgets": {str(b): None for b in BUDGETS},
        }
        if corpus is not None and complete and len(bcy_complete) == len(complete):
            details = [row["bcy_detail"] for row in complete]
            try:
                official = bcy_curve.evaluate_run(
                    f"quanta-{route}", "quanta", record_index_path, details, cache, BUDGETS, (1,)
                )
                require(official["samples"] == len(complete), "official BCY denominator mismatch")
                bcy_summary = {
                    "denominator": len(complete),
                    "status": "VERIFIED",
                    "budgets": {str(b): official["overall"][f"BCY@{b}"] for b in BUDGETS},
                    "official_overall": official["overall"],
                    "official_by_task": official["by_task"],
                }
            except (OSError, ValueError, KeyError, UnicodeError) as exc:
                bcy_summary["reason"] = f"official BCY aggregate failed: {exc}"
        elif corpus is not None and complete:
            bcy_summary["reason"] = "one or more completed rows failed official BCY evaluation"
        summaries[route] = {
            "counts": {
                "requested": requested,
                "preflight_accepted": accepted,
                "preflight_refused": refused,
                "attempted": len(records),
                "unsupported": refused,
                "unjudged": counts["BLOCKED"],
                **{
                    status: counts[status]
                    for status in (
                        "COMPLETED",
                        "REFUSED",
                        "FAILED",
                        "SKIPPED",
                        "BLOCKED",
                        "NOT_RUN",
                    )
                },
            },
            "completed_only": {"denominator": len(complete), **metrics},
            "bcy_completed_only": bcy_summary,
        }
    if corpus is not None:
        try:
            arb_corpus_custody.verify_extracted(corpus)
        except (arb_corpus_custody.CorpusBlocked, OSError, ValueError) as exc:
            corpus_reason = f"extracted corpus changed during BCY evaluation: {exc}"
            corpus = None
            for row in rows:
                if row["status"] == "COMPLETED":
                    row["bcy"] = {"status": "BLOCKED", "reason": corpus_reason}
            for summary in summaries.values():
                summary["bcy_completed_only"] = {
                    "denominator": 0,
                    "status": "BLOCKED",
                    "reason": corpus_reason,
                    "budgets": {str(b): None for b in BUDGETS},
                }
    assert_official_source_custody(b06, policy)
    return {
        "schema_version": 1,
        "arm": arm.get("arm", "original_text_current_v3"),
        "arm_manifest_sha256": sha256(arm_raw),
        "record_index_sha256": sha256(record_index_raw),
        "capture_preparation_sha256": preparation_sha,
        "arb_policy_sha256": sha256(policy_raw),
        "qualification": "DIAGNOSTIC_UNQUALIFIED",
        "binary_capture_custody": "NOT_ATTESTED_BY_SCORER",
        "corpus_custody": corpus.custody
        if corpus is not None
        else {"status": "BLOCKED", "reason": corpus_reason},
        "candidate_tokenizer": evaluator.TOKENIZER,
        "bcy_tokenizer": bcy_curve.TOKENIZER_NAME,
        "metric_contract": {
            "projection": "first 20 distinct files by first occurrence in top 100 ranked chunks",
            "gold": "pinned ARB baseline.target_gold_files(original sample)",
            "Recall@20": "fraction of official gold files in projected top 20",
            "MRR@20": "reciprocal rank of first official gold file within projected top 20",
            "scored_statuses": ["success", "capped", "abstained"],
            "abstained": "completed empty ranking with observed zero file metrics",
            "failed_statuses": ["error", "timeout", "unavailable"],
        },
        "routes": summaries,
        "samples": rows,
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--arm-manifest", type=Path, required=True)
    parser.add_argument(
        "--record-index",
        type=Path,
        required=True,
        help='JSON: {"records":{"sample-id":{"path":"/absolute/record.json","sha256":"..."}}}',
    )
    parser.add_argument("--b06-root", type=Path, required=True)
    parser.add_argument(
        "--preparation-manifest",
        type=Path,
        help="optional pinned copy-spec manifest for a fresh capture run",
    )
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument(
        "--corpus-work-root",
        type=Path,
        help="fresh direct child of the OS temp directory for verified official corpus restore",
    )
    args = parser.parse_args(argv)
    try:
        root = args.b06_root.resolve()
        out = args.out.resolve()
        require(not out.is_relative_to(root), "output must be outside retained b06 root")
        require(
            not out.is_relative_to(Path(__file__).resolve().parents[3]),
            "output must be outside source checkout",
        )
        require(not args.out.exists(), "output already exists")
        result = score_arm(
            args.arm_manifest,
            args.record_index,
            root,
            args.preparation_manifest,
            args.corpus_work_root,
        )
        args.out.parent.mkdir(parents=True, exist_ok=True)
        with args.out.open("x", encoding="utf-8") as file:
            json.dump(result, file, ensure_ascii=False, sort_keys=True, indent=2, allow_nan=False)
            file.write("\n")
    except (
        ScoringRefusal,
        evaluator.EvidenceError,
        OSError,
        KeyError,
        TypeError,
        IndexError,
        ValueError,
        ModuleNotFoundError,
    ) as exc:
        parser.exit(2, f"BLOCKED: {exc}\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
