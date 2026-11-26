import copy
import hashlib
import json
import subprocess
from pathlib import Path

import jsonschema
import pytest

from tools.benchmark.retrieval import evaluator as ev
from tools.benchmark.retrieval import holdout_review, query_plan
from tools.benchmark.retrieval import native_file_evidence as native


def test_native_source_provenance_suite_schema_matches_runtime_contract():
    schema = json.loads(Path(ev.__file__).with_name("suite.schema.json").read_text())
    jsonschema.Draft202012Validator.check_schema(schema)
    assert "source_provenance" in schema["properties"]
    provenance = schema["properties"]["source_provenance"]
    for valid in (
        {"kind": "single_git_commit_v1"},
        {
            "kind": "composite_git_releases_v1",
            "source_universe_sha256": "a" * 64,
            "source_binding_sha256": "b" * 64,
        },
    ):
        jsonschema.validate(valid, provenance)
    for invalid in (
        {"kind": "single_git_commit_v1", "source_binding_sha256": "b" * 64},
        {"kind": "composite_git_releases_v1", "source_universe_sha256": "a" * 64},
        {
            "kind": "composite_git_releases_v1",
            "source_universe_sha256": "A" * 64,
            "source_binding_sha256": "b" * 64,
        },
        {"kind": "unknown"},
    ):
        with pytest.raises(jsonschema.ValidationError):
            jsonschema.validate(invalid, provenance)


def owner(number):
    return {"producer": [0] * 16, "stable_id": [number] * 16}


def hit(number, score, dense=False):
    value = {
        "owner": owner(number),
        "source": [number] * 32,
        "path": f"src/{number}.rs",
        "score": score,
    }
    if dense:
        value.update(projection_owner=owner(number + 10), vector_id=[number], chunk=None)
    return value


def paired():
    lexical = [hit(1, 3.0), hit(2, 2.0), hit(3, 1.0)]
    dense = [hit(2, 0.9, True), hit(4, 0.8, True), hit(1, 0.7, True)]
    # Independent fixed k60 gold: 2,1,4,3. No production ranker is called.
    fused = [
        {
            "owner": owner(2),
            "lexical_rank": 2,
            "dense_rank": 1,
            "score": float.fromhex("0x1.0a6c92bff7560p-5"),
        },
        {
            "owner": owner(1),
            "lexical_rank": 1,
            "dense_rank": 3,
            "score": float.fromhex("0x1.0853aaffeef26p-5"),
        },
        {"owner": owner(4), "lexical_rank": None, "dense_rank": 2, "score": 1 / 62},
        {"owner": owner(3), "lexical_rank": 3, "dense_rank": None, "score": 1 / 63},
    ]
    row = {
        "lexical": {
            "view": "fixed-view",
            "instance": [6] * 16,
            "coverage": "CoreRequired",
            "artifact": [7] * 32,
            "matched": 3,
            "exhaustive": True,
            "hits": lexical,
        },
        "hybrid": {
            "view": "fixed-view",
            "text_instance": [6] * 16,
            "semantic_instance": [8] * 16,
            "lexical_artifact": [7] * 32,
            "lexical_matched": 3,
            "semantic_coverage": {
                "instance": [8] * 16,
                "desired": {
                    "root": None,
                    "schema": [1] * 16,
                    "policy": [2] * 32,
                    "context": [3] * 32,
                },
                "covered": {
                    "root": None,
                    "schema": [1] * 16,
                    "policy": [2] * 32,
                    "context": [3] * 32,
                },
                "searchable": [10] * 32,
                "status": "Complete",
                "owners": [9] * 32,
            },
            "semantic_traversal": {"Approximate": {"capped": False}},
            "semantic_exhaustive": False,
            "scanned_owners": 4,
            "skipped_stale": 0,
            "file_top_k": 4,
            "lexical_pool": lexical,
            "dense_pool": dense,
            "hits": fused,
        },
    }
    row["pool_hashes"] = {
        "scheme": "qidx.native.file.pool.v1",
        "lexical": native.pool_hash("lexical", lexical),
        "dense": native.pool_hash("dense", dense),
    }
    return row


def test_native_pool_hash_matches_cross_language_fixed_gold():
    row = {"owner": owner(1), "source": [2] * 32, "path": "src/alpha.rs", "score": 1.0}
    assert (
        native.pool_hash("lexical", [row])
        == "34ab76cf9c57709c0a345e91ee50a61652636b8455edddbf0e26b48cacb68fe0"
    )


def test_native_rrf_replay_matches_sdk_binary64_order_at_equal_rational_sum():
    row = paired()
    # Ranks (12, 28) and (6, 39) both sum to 5/198 as rationals. The
    # independently fixed binary64 results differ by one representable value.
    lexical_owners = list(range(10, 22))
    lexical_owners[5] = 2
    lexical_owners[11] = 1
    dense_owners = list(range(30, 69))
    dense_owners[27] = 1
    dense_owners[38] = 2
    lexical = [hit(number, 1 / rank) for rank, number in enumerate(lexical_owners, 1)]
    dense = [hit(number, 1 / rank, True) for rank, number in enumerate(dense_owners, 1)]
    row["lexical"].update(matched=len(lexical), hits=lexical)
    row["hybrid"].update(
        lexical_matched=len(lexical),
        lexical_pool=lexical,
        dense_pool=dense,
        scanned_owners=len(dense),
        file_top_k=2,
        hits=[
            {
                "owner": owner(2),
                "lexical_rank": 6,
                "dense_rank": 39,
                "score": float.fromhex("0x1.9dbcc48676f32p-6"),
            },
            {
                "owner": owner(1),
                "lexical_rank": 12,
                "dense_rank": 28,
                "score": float.fromhex("0x1.9dbcc48676f31p-6"),
            },
        ],
    )
    for lane, pool in (("lexical", lexical), ("dense", dense)):
        row["pool_hashes"][lane] = native.pool_hash(lane, pool)
    universe = {file["path"] for file in (*lexical, *dense)}
    assert native.replay_pools(row, 39, 2, universe)["hybrid"] == [
        {"path": "src/2.rs"},
        {"path": "src/1.rs"},
    ]
    row["hybrid"]["hits"].reverse()
    with pytest.raises(ev.EvidenceError, match="RRF order"):
        native.replay_pools(row, 39, 2, universe)


@pytest.mark.parametrize(
    "case",
    [
        "source",
        "projection",
        "rendered",
        "wrong_id",
        "wrong_source",
        "wrong_policy",
        "bad_span",
        "bad_domain",
    ],
)
def test_native_dense_chunk_replay_checks_sdk_binding_even_with_matching_pool_hash(case):
    row = paired()
    hit = row["hybrid"]["dense_pool"][0]
    hit["vector_id"] = [2] * 32
    chunk = {
        "id": [2] * 32,
        "payload": [3] * 32,
        "input": {
            "source": [2] * 32,
            "projection": None,
            "producer_context": [0] * 32,
            "derivation_policy": [2] * 32,
        },
        "input_digest": [4] * 32,
        "span": {"SourcePayload": {"start_byte": 0, "end_byte": 4}},
    }
    hit["chunk"] = chunk
    if case in ("projection", "rendered"):
        chunk["input"].update(projection=[5] * 32, producer_context=[6] * 32)
        chunk["span"] = {"ProjectionPayload": {"start_byte": 0, "end_byte": 4}}
        if case == "rendered":
            chunk["span"] = {"RenderedMember": {"ordinal": 4095}}
    elif case == "wrong_id":
        chunk["id"][0] ^= 1
    elif case == "wrong_source":
        chunk["input"]["source"][0] ^= 1
    elif case == "wrong_policy":
        chunk["input"]["derivation_policy"][0] ^= 1
    elif case == "bad_span":
        chunk["span"]["SourcePayload"]["end_byte"] = 16_777_217
    elif case == "bad_domain":
        chunk["input"]["projection"] = [5] * 32
    row["pool_hashes"]["dense"] = native.pool_hash("dense", row["hybrid"]["dense_pool"])
    universe = {f"src/{number}.rs" for number in (1, 2, 3, 4)}
    if case in ("source", "projection", "rendered"):
        assert native.replay_pools(row, 4, 4, universe)["hybrid"][0] == {"path": "src/2.rs"}
    else:
        with pytest.raises(ev.EvidenceError, match="Native dense chunk"):
            native.replay_pools(row, 4, 4, universe)


class FrozenFiles:
    def __init__(self):
        self.raw = {"src/a.rs": b"hello\n", "src/b.rs": b"pub fn alpha() {}\n"}

    def file(self, path):
        raw = self.raw[path]
        return raw, raw.splitlines(keepends=True), hashlib.sha256(raw).hexdigest()


def indexed_fixture():
    source = FrozenFiles()
    view = {"epoch": 1, "sequence": 3}
    cut = {"owners": [1] * 32, "paths": [2] * 32, "frontiers": [3] * 32}
    universe = [{"path": path, "file_sha256": source.file(path)[2]} for path in sorted(source.raw)]
    entries = [
        {
            "owner": owner(index),
            "path": path,
            "source": [index + 10] * 32,
            "bytes": list(bytes.fromhex(object_ref)),
            "byte_len": len(source.raw[path]),
        }
        for index, (path, object_ref) in enumerate(
            (
                ("src/a.rs", "dce5b5a93070568e40ccea73b8998233f3b5ba5a853971dd9ee044984461ab73"),
                ("src/b.rs", "dd526b7b8334ae2935d4515031f6ab54600fdc9490188bfbb2b595f3a24168a4"),
            ),
            1,
        )
    ]
    lookups = [
        {
            "view": copy.deepcopy(view),
            "source": copy.deepcopy(cut),
            "rows": [
                {"owner": copy.deepcopy(row["owner"]), "source": row["source"].copy()}
                for row in entries
            ],
        }
    ]
    inventory = {
        "view": copy.deepcopy(view),
        "source_cut": copy.deepcopy(cut),
        "file_instance": [9] * 16,
        "entries": entries,
        "owner_lookups": lookups,
    }
    return source, universe, view, cut, inventory


def test_indexed_inventory_authenticates_fixed_raw_body_refs_and_source_cut():
    source, universe, view, cut, inventory = indexed_fixture()
    assert native.verify_indexed_universe(source, universe, view, cut, inventory) == {
        row["path"]: (native._owner(row["owner"]), bytes(row["source"]))
        for row in inventory["entries"]
    }


@pytest.mark.parametrize(
    "mutation",
    [
        "missing",
        "duplicate_path",
        "duplicate_owner",
        "foreign",
        "body",
        "length",
        "source",
        "owner_lookup",
        "lookup_missing",
        "lookup_duplicate",
        "view",
        "cut",
        "unsorted",
        "file_instance",
        "two_producers",
    ],
)
def test_indexed_inventory_rejects_missing_foreign_or_tampered_rows(mutation):
    source, universe, view, cut, inventory = indexed_fixture()
    rows = inventory["entries"]
    if mutation == "missing":
        rows.pop()
    elif mutation == "duplicate_path":
        rows.append(copy.deepcopy(rows[0]))
    elif mutation == "duplicate_owner":
        rows[1]["owner"] = rows[0]["owner"]
    elif mutation == "foreign":
        rows[1]["path"] = "src/foreign.rs"
    elif mutation == "body":
        rows[0]["bytes"][0] ^= 1
    elif mutation == "length":
        rows[0]["byte_len"] += 1
    elif mutation == "source":
        rows[0]["source"][0] ^= 1
    elif mutation == "owner_lookup":
        inventory["owner_lookups"][0]["rows"][0]["source"][0] ^= 1
    elif mutation == "lookup_missing":
        inventory["owner_lookups"][0]["rows"].pop()
    elif mutation == "lookup_duplicate":
        inventory["owner_lookups"][0]["rows"].append(
            copy.deepcopy(inventory["owner_lookups"][0]["rows"][0])
        )
    elif mutation == "view":
        inventory["view"]["sequence"] += 1
    elif mutation == "cut":
        inventory["source_cut"]["paths"][0] ^= 1
    elif mutation == "unsorted":
        rows.reverse()
    elif mutation == "two_producers":
        rows[1]["owner"]["producer"] = [7] * 16
    else:
        inventory["file_instance"] = []
    with pytest.raises(ev.EvidenceError):
        native.verify_indexed_universe(source, universe, view, cut, inventory)


def test_indexed_inventory_rejects_frozen_source_change_even_with_same_paths():
    source, universe, view, cut, inventory = indexed_fixture()
    source.raw["src/a.rs"] = b"changed\n"
    with pytest.raises(ev.EvidenceError):
        native.verify_indexed_universe(source, universe, view, cut, inventory)


def test_native_independent_rrf_preserves_dense_only_and_file_metric_gold():
    universe = {f"src/{number}.rs" for number in (1, 2, 3, 4)}
    row = paired()
    pools = native.replay_pools(row, 4, 4, universe)
    assert [row["path"] for row in pools["hybrid"]] == [
        "src/2.rs",
        "src/1.rs",
        "src/4.rs",
        "src/3.rs",
    ]
    qrels = [{"path": f"src/{number}.rs", "grade": int(number == 4)} for number in (1, 2, 3, 4)]
    assert ev.file_mrr_at_k_judged(pools["lexical"], qrels, 10) == 0
    assert ev.file_mrr_at_k_judged(pools["hybrid"], qrels, 10) == pytest.approx(1 / 3)
    assert ev.file_recall_at_k_judged(pools["hybrid"], qrels, 10) == 1
    row["hybrid"]["file_top_k"] = 2
    row["hybrid"]["hits"] = row["hybrid"]["hits"][:2]
    pools = native.replay_pools(row, 4, 2, universe)
    assert [hit["path"] for hit in pools["dense"]] == ["src/2.rs", "src/4.rs"]
    assert [hit["path"] for hit in pools["lexical"]] == ["src/1.rs", "src/2.rs"]
    assert [hit["path"] for hit in pools["hybrid"]] == ["src/2.rs", "src/1.rs"]


@pytest.mark.parametrize(
    "mutation",
    ["missing", "partial", "stale_binding", "wrong_instance", "false_exhaustive", "bad_scan"],
)
def test_native_scoring_refuses_unbound_semantic_coverage(mutation):
    row = paired()
    hybrid = row["hybrid"]
    if mutation == "missing":
        del hybrid["semantic_coverage"]
    elif mutation == "partial":
        hybrid["semantic_coverage"]["status"] = {"Partial": {"missing": None}}
    elif mutation == "stale_binding":
        hybrid["semantic_coverage"]["covered"]["context"][0] ^= 1
    elif mutation == "wrong_instance":
        hybrid["semantic_coverage"]["instance"] = "other"
    elif mutation == "false_exhaustive":
        hybrid["semantic_exhaustive"] = True
    else:
        hybrid["scanned_owners"] = 2
    with pytest.raises(ev.EvidenceError, match="semantic"):
        native.replay_pools(row, 4, 4, {f"src/{number}.rs" for number in (1, 2, 3, 4)})


@pytest.mark.parametrize("mutation", ["rank", "source", "order", "hash", "duplicate", "outside"])
def test_native_pool_replay_refuses_fabricated_or_unpaired_evidence(mutation):
    row = copy.deepcopy(paired())
    if mutation == "rank":
        row["hybrid"]["hits"][0]["lexical_rank"] = 1
    elif mutation == "source":
        row["hybrid"]["dense_pool"][0]["source"] = [99] * 32
    elif mutation == "order":
        row["hybrid"]["hits"].reverse()
    elif mutation == "hash":
        row["pool_hashes"]["dense"] = "0" * 64
    elif mutation == "duplicate":
        row["hybrid"]["dense_pool"].append(copy.deepcopy(row["hybrid"]["dense_pool"][0]))
    else:
        row["hybrid"]["dense_pool"][1]["path"] = "foreign.rs"
    with pytest.raises(ev.EvidenceError):
        native.replay_pools(row, 4, 4, {f"src/{number}.rs" for number in (1, 2, 3, 4)})


@pytest.mark.parametrize(
    "metric,before,after", [("file_mrr_at_10", 1.0, 0.5), ("file_recall_at_10", 0.1, 0.9)]
)
def test_native_repository_rows_use_declared_mrr_or_recall_without_ndcg_coercion(
    metric, before, after
):
    suite = {
        "suite_id": "native",
        "repository_commit": "a" * 40,
        "tasks": [
            {
                "task_id": "q",
                "query_family_id": "family",
                "split": "eval",
                "gold": [],
                "answerable": True,
            }
        ],
    }
    report = {
        "suite_id": suite["suite_id"],
        "repository_commit": suite["repository_commit"],
        "suite_commitment_sha256": ev.digest(ev.canonical(suite)),
        "graded": True,
        "rank_metrics": {
            "comparison": {
                "baseline": "lexical",
                "candidate": "hybrid",
                "primary_metric": metric,
                "sample_count": 1,
                "primary_delta": after - before,
            }
        },
        "per_query": [
            {"task_id": "q", "route": route, metric: score, "file_ndcg_at_10": 1 - score}
            for route, score in (("lexical", before), ("hybrid", after))
        ],
    }
    assert ev.paired_query_family_rows(suite, report, "lexical", "hybrid", require_graded=True) == [
        ("q", "family", "uncategorized", after - before)
    ]


def _replayed_native_family(index: int, family: str, deltas: list[float]) -> dict:
    repository = f"repo{index:02d}"
    source_commit = f"{index + 101:040x}"
    target_commit = f"{index + 1:040x}"
    binding = {
        "family_id": family,
        "qualification": "SOURCE_ONLY_NOT_FORMAL_EVAL",
        "source_release_count": 12,
        "source_file_count": 12,
        "source_universe_sha256": f"{index + 1:064x}",
        "source_binding_sha256": f"{index + 101:064x}",
        "source_commit": source_commit,
        "file_universe_digest": "sha256:" + f"{index + 201:064x}",
        "base_release_digest": "sha256:" + "b" * 64,
        "target_repository": repository,
        "target_commit": target_commit,
        "target_release_digest": "sha256:" + f"{index + 301:064x}",
    }
    tasks = [
        {
            "task_id": f"{family}:{number}",
            "query_family_id": family,
            "split": "eval",
            "gold": [],
            "answerable": True,
        }
        for number in range(len(deltas))
    ]
    suite = {
        "suite_id": family,
        "repository_commit": source_commit,
        "file_universe_digest": binding["file_universe_digest"].removeprefix("sha256:"),
        "source_provenance": {
            "kind": "composite_git_releases_v1",
            "source_universe_sha256": binding["source_universe_sha256"],
            "source_binding_sha256": binding["source_binding_sha256"],
        },
        "tasks": tasks,
    }
    capture = {
        "schema_version": 2,
        "profile": "native_file",
        "engine": "native",
        "suite_id": family,
        "repository_commit": source_commit,
        "suite_commitment_sha256": ev.digest(ev.canonical(suite)),
        "file_universe_digest": suite["file_universe_digest"],
        "source_universe_sha256": binding["source_universe_sha256"],
        "source_binding_sha256": binding["source_binding_sha256"],
        "tasks": [{"task_id": task["task_id"]} for task in tasks],
    }
    report = {
        "status": "native_file_evidence_unqualified",
        "product_default_decision": False,
        "runtime_qualified": False,
        "indexed_universe_verified": True,
        "graded": True,
        "suite_id": family,
        "repository_commit": source_commit,
        "suite_commitment_sha256": ev.digest(ev.canonical(suite)),
        "capture_commitment_sha256": ev.digest(ev.canonical(capture)),
        "composite_source_binding": binding,
        "rank_metrics": {
            "comparison": {
                "baseline": "lexical",
                "candidate": "hybrid",
                "primary_metric": "file_mrr_at_10",
                "sample_count": len(deltas),
                "primary_delta": sum(deltas) / len(deltas),
            }
        },
        "per_query": [
            {"task_id": task["task_id"], "route": route, "file_mrr_at_10": score}
            for task, delta in zip(tasks, deltas, strict=True)
            for route, score in (
                ("lexical", 0.0 if delta >= 0 else -delta),
                ("hybrid", delta if delta >= 0 else 0.0),
            )
        ],
    }
    return {"suite": suite, "capture": capture, "report": report}


def _native_family_cohort() -> tuple[list[dict], dict[str, str], dict[str, str]]:
    # Twenty tasks in repo00 oppose one task in each other repository.
    # Its two family means cancel, so repository weighting differs from task weighting.
    families = [_replayed_native_family(0, "repo00-a", [1.0] * 19)]
    families.append(_replayed_native_family(0, "repo00-b", [-1.0]))
    families.extend(
        _replayed_native_family(index, f"repo{index:02d}-a", [-1.0]) for index in range(1, 12)
    )
    repositories = {f"repo{index:02d}": f"{index + 1:040x}" for index in range(12)}
    strata = {repository: "fixed" for repository in repositories}
    return families, repositories, strata


def test_native_composite_family_ci_weights_families_then_repositories() -> None:
    families, repositories, strata = _native_family_cohort()
    result = native.repository_cluster_ci_from_native_families(families, repositories, strata)
    assert result["status"] == "native_repository_cluster_diagnostic_unqualified"
    assert result["product_default_decision"] is False
    assert result["runtime_qualified"] is False
    assert (
        result["repository_cluster_ci"]["aggregation"]
        == "equal_family_within_repository_equal_repository"
    )
    assert result["repository_cluster_ci"]["repository_count"] == 12
    assert result["repository_cluster_ci"]["family_count"] == 13
    assert result["repository_cluster_ci"]["sample_count"] == 31
    assert result["repository_cluster_ci"]["mean"] == pytest.approx(-11 / 12)
    assert result["repository_cluster_ci"]["mean"] != pytest.approx(7 / 31)
    assert {row["target_release_digest"] for row in result["families"]} == {
        "sha256:" + f"{index + 301:064x}" for index in range(12)
    }


def _overlay_native_family_cohort():
    families, repositories, strata = _native_family_cohort()
    for item in families:
        original = copy.deepcopy(item["suite"])
        for task in original["tasks"]:
            task["answerable"] = False
        item["execution_suite"] = original
        original_sha = ev.digest(ev.canonical(original))
        item["capture"]["suite_commitment_sha256"] = original_sha
        item["report"]["execution_suite_commitment_sha256"] = original_sha
        item["report"]["capture_commitment_sha256"] = ev.digest(ev.canonical(item["capture"]))
    return families, repositories, strata


def test_native_composite_family_ci_overlay_binds_both_suite_digests():
    families, repositories, strata = _overlay_native_family_cohort()
    result = native.repository_cluster_ci_from_native_families(families, repositories, strata)
    for row, item in zip(
        result["families"],
        sorted(
            families,
            key=lambda f: (
                f["report"]["composite_source_binding"]["target_repository"],
                f["report"]["composite_source_binding"]["family_id"],
            ),
        ),
        strict=True,
    ):
        assert row["suite_commitment_sha256"] == ev.digest(ev.canonical(item["suite"]))
        assert row["execution_suite_commitment_sha256"] == ev.digest(
            ev.canonical(item["execution_suite"])
        )
        assert row["suite_commitment_sha256"] != row["execution_suite_commitment_sha256"]


@pytest.mark.parametrize("mutation", ["original_query", "capture_digest", "report_digest"])
def test_native_composite_family_ci_overlay_rejects_rebinding(mutation):
    families, repositories, strata = _overlay_native_family_cohort()
    if mutation == "original_query":
        families[0]["execution_suite"]["tasks"][0]["query"] = "forged"
    elif mutation == "capture_digest":
        families[0]["capture"]["suite_commitment_sha256"] = "0" * 64
        families[0]["report"]["capture_commitment_sha256"] = ev.digest(
            ev.canonical(families[0]["capture"])
        )
    else:
        families[0]["report"]["execution_suite_commitment_sha256"] = "0" * 64
    with pytest.raises(ev.EvidenceError):
        native.repository_cluster_ci_from_native_families(families, repositories, strata)


@pytest.mark.parametrize(
    "mutation",
    [
        "missing_family",
        "duplicate_family",
        "foreign_target",
        "wrong_target_release",
        "changed_pool",
        "wrong_source_release",
        "duplicate_task",
        "missing_pair",
        "ungraded",
        "wrong_metric",
        "forged_qualified",
        "source_only_promoted",
        "wrong_snapshot_digest",
        "prefixed_query_digest",
    ],
)
def test_native_composite_family_ci_rejects_unbound_or_promoted_rows(mutation: str) -> None:
    families, repositories, strata = _native_family_cohort()
    if mutation == "missing_family":
        families.pop()
    elif mutation == "duplicate_family":
        families.append(copy.deepcopy(families[0]))
    elif mutation == "foreign_target":
        families[0]["report"]["composite_source_binding"]["target_repository"] = "foreign"
    elif mutation == "wrong_target_release":
        families[1]["report"]["composite_source_binding"]["target_release_digest"] = (
            "sha256:" + "d" * 64
        )
    elif mutation == "changed_pool":
        families[0]["capture"]["tasks"][0]["pool_hashes"] = {"lexical": "f" * 64}
    elif mutation == "wrong_source_release":
        families[0]["report"]["composite_source_binding"]["source_commit"] = "d" * 40
    elif mutation in ("wrong_snapshot_digest", "prefixed_query_digest"):
        capture = families[0]["capture"]
        binding = families[0]["report"]["composite_source_binding"]
        capture["file_universe_digest"] = (
            "0" * 64 if mutation == "wrong_snapshot_digest" else binding["file_universe_digest"]
        )
        families[0]["report"]["capture_commitment_sha256"] = ev.digest(ev.canonical(capture))
    elif mutation == "duplicate_task":
        families[0]["report"]["per_query"].append(
            copy.deepcopy(families[0]["report"]["per_query"][0])
        )
    elif mutation == "missing_pair":
        families[0]["report"]["per_query"].pop()
    elif mutation == "ungraded":
        families[0]["report"]["graded"] = False
    elif mutation == "wrong_metric":
        families[0]["report"]["rank_metrics"]["comparison"]["primary_metric"] = "file_recall_at_10"
    elif mutation == "forged_qualified":
        families[0]["report"]["status"] = "qualified"
    else:
        families[0]["report"]["composite_source_binding"]["qualification"] = "QUALIFIED"
    with pytest.raises(ev.EvidenceError):
        native.repository_cluster_ci_from_native_families(families, repositories, strata)


def complete_capture():
    universe = [
        {"path": f"src/{number}.rs", "file_sha256": str(number) * 64} for number in (1, 2, 3, 4)
    ]
    universe_digest = ev.digest(ev.canonical(universe))
    tasks = [
        {
            "task_id": task_id,
            "query": query,
            "query_family_id": task_id,
            "split": "eval",
            "gold": [],
            "answerable": answerable,
            "judgment_policy": ev.COMPLETE_JUDGMENT_POLICY,
            "file_judgments": [
                {
                    "path": row["path"],
                    "file_sha256": row["file_sha256"],
                    "grade": 2 if answerable and row["path"] == "src/4.rs" else 0,
                }
                for row in universe
            ],
        }
        for task_id, query, answerable in (
            ("positive", "alpha", True),
            ("negative", "omega", False),
        )
    ]
    suite = {
        "suite_id": "native",
        "repository_commit": "a" * 40,
        "source_provenance": {"kind": "single_git_commit_v1"},
        "comparison_contract": {"top_k": 10},
        "file_universe": universe,
        "file_universe_digest": universe_digest,
        "tasks": tasks,
    }
    pack = {
        "suite_id": "native",
        "suite_commitment_sha256": ev.digest(ev.canonical(suite)),
        "repository_commit": "a" * 40,
        "comparison_contract": {"top_k": 10},
        "file_universe": universe,
        "file_universe_digest": universe_digest,
        "tasks": [
            {
                "task_id": task["task_id"],
                "query": task["query"],
                "query_sha256": ev.digest(task["query"].encode()),
            }
            for task in tasks
        ],
    }
    view = {
        "workspace": {
            "tenant": [1] * 16,
            "collection": [2] * 16,
            "worktree": [3] * 16,
            "slot": [4] * 16,
        },
        "epoch": 1,
        "sequence": 2,
        "digest": list(
            bytes.fromhex("3d20fae5cca46179450ef8f25ecd1c395bd07dcdbe7a56b4ebc4f762f80d125e")
        ),
    }
    session = [5] * 32
    rows = []
    for task in tasks:
        row = paired()
        row["task_id"] = task["task_id"]
        row["lexical_micros"] = 1
        row["hybrid_micros"] = 2
        row["lexical"].update(view=view, instance=[6] * 16, artifact=[7] * 32)
        row["hybrid"].update(
            view=view,
            session=session,
            text_instance=[6] * 16,
            semantic_instance=[8] * 16,
            lexical_artifact=[7] * 32,
            file_top_k=10,
        )
        row["hybrid"]["semantic_coverage"]["instance"] = [8] * 16
        if not task["answerable"]:
            row["lexical"].update(matched=0, hits=[])
            row["hybrid"].update(lexical_matched=0, lexical_pool=[])
            row["hybrid"]["hits"] = [
                {
                    "owner": owner(number),
                    "lexical_rank": None,
                    "dense_rank": rank,
                    "score": 1 / (60 + rank),
                }
                for rank, number in enumerate((2, 4, 1), 1)
            ]
        planned = f"select:file case:no {task['query']}"
        row["query_identity"] = {
            "original_query_sha256": ev.digest(task["query"].encode()),
            "effective_lexical_request_sha256": ev.digest(planned.encode()),
            "semantic_text_sha256": ev.digest(task["query"].encode()),
            "policy_config_sha256": ev.digest(
                query_plan.policy_config_canonical("natural_language_file").encode()
            ),
        }
        row["text_request"] = {
            "syntax": "native",
            "query_text": planned,
            "constraints": {"language_any_of": []},
            "generation": None,
            "generation_selector": None,
            "cursor": None,
            "top_k": 10,
        }
        row["semantic_request"] = {
            "query_text": task["query"],
            "constraints": {"language_any_of": []},
            "generation": None,
            "generation_selector": None,
            "lexical_scope": None,
            "top_k": 10,
        }
        row["pool_hashes"] = {
            "scheme": "qidx.native.file.pool.v1",
            "lexical": native.pool_hash("lexical", row["hybrid"]["lexical_pool"]),
            "dense": native.pool_hash("dense", row["hybrid"]["dense_pool"]),
        }
        rows.append(row)
    capture = {
        "schema_version": 1,
        "profile": "native_file",
        "engine": "native",
        "suite_id": "native",
        "repository_commit": "a" * 40,
        "file_universe_digest": universe_digest,
        "query_pack_sha256": ev.digest(ev.canonical(pack)),
        "suite_commitment_sha256": ev.digest(ev.canonical(suite)),
        "view": view,
        "selected_manifest": {
            "identity": view,
            "profile": [9] * 32,
            "source": {"owners": [10] * 32, "paths": [11] * 32, "frontiers": [12] * 32},
            "projections": None,
            "members": [13] * 32,
            "coverage": None,
        },
        "text_instance": [6] * 16,
        "semantic_instance": [8] * 16,
        "lane_top_k": 10,
        "file_top_k": 10,
        "query_deadline_seconds": 60,
        "released_session": {
            "session": session,
            "view": view,
            "revision": 1,
            "expires_at_ms": 1000,
            "state": "Released",
        },
        "settlement": {
            "session": session,
            "view": view,
            "head": view,
            "exact_floor": 0,
            "state": "Pending",
            "has_remaining_target_pins": False,
        },
        "tasks": rows,
    }
    return suite, pack, capture


def source_bound_capture(*, source_local=False):
    suite, pack, capture = complete_capture()
    source = FrozenFiles()
    source.raw = {
        f"src/{number}.rs": f"fn item_{number}() {{}}\n".encode() for number in (1, 2, 3, 4)
    }
    universe = [{"path": path, "file_sha256": source.file(path)[2]} for path in sorted(source.raw)]
    for value in (suite, pack, capture):
        value["file_universe_digest"] = ev.digest(ev.canonical(universe))
    suite["file_universe"] = pack["file_universe"] = universe
    for task in suite["tasks"]:
        for label in task["file_judgments"]:
            label["file_sha256"] = source.file(label["path"])[2]
    pack["suite_commitment_sha256"] = ev.digest(ev.canonical(suite))
    capture["suite_commitment_sha256"] = pack["suite_commitment_sha256"]
    capture["query_pack_sha256"] = ev.digest(ev.canonical(pack))
    view = capture["view"]
    cut = capture["selected_manifest"]["source"]
    entries = [
        {
            "owner": owner(number),
            "path": path,
            "source": [number] * 32,
            "bytes": list(native._body_ref(source.file(path)[0])),
            "byte_len": len(source.file(path)[0]),
        }
        for number, path in enumerate(sorted(source.raw), 1)
    ]
    capture["schema_version"] = 2
    # Fixed QIOBJ001 profile and CatalogView commitments for this fixture.
    capture["selected_manifest"]["profile"] = list(
        bytes.fromhex(
            "dd8bb14a5016359677d23edf175aadaa30998ff65a87326a0b82c02b4080978b"
            if source_local
            else "d6881fc2cb0241ab11f78673a32f1e062222e25a730405b3af5f70247b6403e3"
        )
    )
    view["digest"] = list(
        bytes.fromhex(
            "f7c7a9fb2a16d8eb4e77e1a7a054e5abbd16f68fa43386bc4c4ba34748dbd535"
            if source_local
            else "287a23c6b2680fe6163aa6e13dd05cf298fe0fd0586801a7ddbb3f963f6c940b"
        )
    )
    capture["indexed_universe"] = {
        "view": copy.deepcopy(view),
        "source_cut": copy.deepcopy(cut),
        "file_instance": [9] * 16,
        "entries": entries,
        "owner_lookups": [
            {
                "view": copy.deepcopy(view),
                "source": copy.deepcopy(cut),
                "rows": [
                    {"owner": copy.deepcopy(row["owner"]), "source": row["source"].copy()}
                    for row in entries
                ],
            }
        ],
    }
    capture["selected_profile"] = {
        "session": capture["released_session"]["session"].copy(),
        "view": copy.deepcopy(capture["selected_manifest"]),
        "profile": {
            "instances": [
                {
                    "instance": [number] * 16,
                    "kind": [20 + number] * 16,
                    "group": [30 + number] * 16,
                    "activation": "AsyncCoverage" if number == 8 else "CoreRequired",
                    "input_domain": (
                        {"Projection": {"projection": [35] * 16, "producer": [36] * 16}}
                        if number == 8 and not source_local
                        else "SourceLocal"
                    ),
                    "schema": [40 + number] * 16,
                    "policy": [50 + number] * 32,
                    "format": [60 + number] * 16,
                }
                for number in (6, 8, 9)
            ]
        },
    }
    for row in capture["tasks"]:
        coverage = row["hybrid"]["semantic_coverage"]
        for field in ("desired", "covered"):
            coverage[field]["schema"] = [48] * 16
            coverage[field]["policy"] = [58] * 32
            if source_local:
                coverage[field]["root"] = [10] * 32
                coverage[field]["context"] = list(
                    bytes.fromhex(
                        "2afbe302400b2cdbc8a065a0bc06b470f5eb971c95c1f0bec7f5faf14e4a99b4"
                    )
                )
    return suite, pack, capture, source


def unlabelled_capture():
    suite, pack, capture, source = source_bound_capture(source_local=True)
    for task in suite["tasks"]:
        task["answerable"] = False
        del task["judgment_policy"], task["file_judgments"]
    pack["suite_commitment_sha256"] = ev.digest(ev.canonical(suite))
    capture["suite_commitment_sha256"] = pack["suite_commitment_sha256"]
    capture["query_pack_sha256"] = ev.digest(ev.canonical(pack))
    return suite, pack, capture, source


def test_native_review_overlay_scores_judged_suite_without_rebinding_capture():
    reviewed, _, _, _ = source_bound_capture(source_local=True)
    original, pack, capture, source = unlabelled_capture()
    reviewed["diagnostic_policy"] = ev.OBSERVED_PREFIX_DIAGNOSTIC_POLICY
    report = native.evaluate(original, pack, capture, source, reviewed_suite=reviewed)
    assert report["graded"] is True
    assert report["suite_commitment_sha256"] == ev.digest(ev.canonical(reviewed))
    assert report["execution_suite_commitment_sha256"] == ev.digest(ev.canonical(original))
    assert report["capture_commitment_sha256"] == ev.digest(ev.canonical(capture))
    assert report["rank_metrics"]["comparison"]["sample_count"] == 1
    assert report["metrics"]["hybrid"]["file_mrr_at_10"] == pytest.approx(1 / 3)


@pytest.mark.parametrize(
    "mutation", ["query", "source", "unexpected_task_field", "diagnostic_policy"]
)
def test_native_review_overlay_rejects_changed_execution_inputs(mutation):
    reviewed, _, _, _ = source_bound_capture(source_local=True)
    original, pack, capture, source = unlabelled_capture()
    reviewed["diagnostic_policy"] = ev.OBSERVED_PREFIX_DIAGNOSTIC_POLICY
    if mutation == "query":
        reviewed["tasks"][0]["query"] = "changed"
    elif mutation == "source":
        reviewed["file_universe"][0]["file_sha256"] = "0" * 64
    elif mutation == "unexpected_task_field":
        reviewed["tasks"][0]["semantic_input"] = "changed"
    else:
        reviewed["diagnostic_policy"] = "unreviewed"
    with pytest.raises(ev.EvidenceError, match="reviewed suite.*execution"):
        native.evaluate(original, pack, capture, source, reviewed_suite=reviewed)


def test_native_review_preparation_retains_pool_membership_without_grading():
    suite, pack, capture, source = unlabelled_capture()
    pools = native.prepare_review_candidates(suite, pack, capture, source)
    files = pack["file_universe"]
    assert pools == {
        "lexical": {"positive": files[:3], "negative": []},
        "hybrid": {"positive": files, "negative": [files[i] for i in (0, 1, 3)]},
        "dense": {
            "positive": [files[i] for i in (0, 1, 3)],
            "negative": [files[i] for i in (0, 1, 3)],
        },
    }
    with pytest.raises(ev.EvidenceError, match="Native file qrels are incomplete"):
        native.evaluate(suite, pack, capture, source)


def test_native_review_preparation_preserves_same_view_and_requires_inventory():
    suite, pack, capture, source = unlabelled_capture()
    capture["schema_version"] = 1
    with pytest.raises(ev.EvidenceError, match="indexed inventory"):
        native.prepare_review_candidates(suite, pack, capture, source)
    capture["schema_version"] = 2
    capture["tasks"][0]["hybrid"]["view"] = {**capture["view"], "sequence": 4}
    with pytest.raises(ev.EvidenceError, match="view/session"):
        native.prepare_review_candidates(suite, pack, capture, source)


def test_native_review_preparation_refuses_preexisting_judgments():
    suite, pack, capture, source = source_bound_capture()
    with pytest.raises(ev.EvidenceError, match="unjudged eval tasks"):
        native.prepare_review_candidates(suite, pack, capture, source)


@pytest.mark.parametrize("large_capture", [False, True])
def test_native_review_cli_replays_frozen_unlabelled_inputs(tmp_path, large_capture):
    from tools.benchmark.evidence import CONTROL_DOCUMENT_BYTES

    suite, _pack, capture, source = unlabelled_capture()
    checkout = tmp_path / "source"
    checkout.mkdir()
    for relative, body in source.raw.items():
        path = checkout / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(body)
    subprocess.run(["git", "init", "-q", str(checkout)], check=True)
    git = ["git", "-C", str(checkout)]
    subprocess.run([*git, "add", "."], check=True)
    subprocess.run(
        [
            *git,
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.com",
            "commit",
            "-qm",
            "review fixture",
        ],
        check=True,
    )
    commit = subprocess.check_output([*git, "rev-parse", "HEAD"], text=True).strip()
    suite.update(
        schema_version=ev.SCHEMA_VERSION,
        repository_commit=commit,
        routes=["lexical", "hybrid"],
        comparison_contract={
            "top_k": 10,
            "tokenizer": ev.TOKENIZER,
            "tokenizer_budget_version": ev.TOKENIZER_BUDGET_VERSION,
            "output_unit_policy": "rank_prefix",
            "span_unit": ev.SPAN_UNIT,
        },
    )
    for task in suite["tasks"]:
        task["query_sha256"] = ev.digest(task["query"].encode())
    suite, pack, _source = ev.validate_suite(checkout, suite)
    capture.update(
        repository_commit=commit,
        suite_commitment_sha256=pack["suite_commitment_sha256"],
        query_pack_sha256=ev.digest(ev.canonical(pack)),
        query_pack_file_sha256=ev.digest(ev.canonical(pack)),
    )
    task_ids = [task["task_id"] for task in pack["tasks"]]
    contexts = {
        task_id: {
            "intent": "file relevance",
            "provenance": "fixture",
            "rubric": "Grade the source file.",
        }
        for task_id in task_ids
    }
    controls = [
        {
            "pool_id": kind,
            "kind": kind,
            "tasks": {task_id: [pack["file_universe"][index]] for task_id in task_ids},
        }
        for kind, index in (("source_alternative", 2), ("random_control", 3))
    ]
    args = ["prepare-native", "--repo", str(checkout), "--seed", "42"]
    capture_bytes = ev.canonical(capture)
    if large_capture:
        capture_bytes += b" " * CONTROL_DOCUMENT_BYTES
    for name, value in (
        ("suite", suite),
        ("query-pack", pack),
        ("capture", capture),
        ("contexts", contexts),
        ("control-pools", controls),
    ):
        path = tmp_path / (name + ".json")
        path.write_bytes(capture_bytes if name == "capture" else ev.canonical(value))
        args.extend(["--" + name, str(path)])
    output = tmp_path / "review"
    assert holdout_review.main([*args, "--output", str(output)]) == 0
    raw = (output / "reviewer-1.json").read_bytes()
    form = json.loads(raw)
    assert all(review["answerable"] is None for review in form["reviews"])
    assert b"lexical" not in raw and b"hybrid" not in raw and b"score" not in raw
    custody = json.loads((output / "owner" / "custody.json").read_bytes())
    assert custody["native_capture"]["record_bytes_sha256"] == ev.digest(capture_bytes)
    assert custody["qualified"] is False
    if large_capture:
        return

    # The frozen Git source and actual captured pools become labels only after
    # review; the original suite, query pack and capture stay byte-identical.
    source_paths = tuple(
        (tmp_path / (name + ".json")).read_bytes() for name in ("suite", "query-pack", "capture")
    )
    replay_pack, retrieval_pools, capture_custody = holdout_review.capture_native_review_pools(
        checkout,
        tmp_path / "suite.json",
        tmp_path / "query-pack.json",
        tmp_path / "capture.json",
        pool_ids=("route-a", "route-b", "route-c"),
    )
    all_pools = retrieval_pools + controls
    completed = []
    for slot in (1, 2):
        filled = json.loads((output / f"reviewer-{slot}.json").read_bytes())
        filled["reviewer_id"] = f"fixture-reviewer-{slot}"
        for review in filled["reviews"]:
            review["answerable"] = review["task_id"] == "positive"
            review["rationale"] = "Fixture source relevance judgment."
            for file in review["files"]:
                file["grade"] = (
                    2 if review["task_id"] == "positive" and file["path"] == "src/4.rs" else 0
                )
                file["rationale"] = "Fixture source file grade."
        completed.append(filled)
    adjudicated = copy.deepcopy(completed[0])
    adjudicated["reviewer_id"] = "fixture-adjudicator"
    contracts = {
        task_id: {
            "request_mode": query_plan.NATURAL_LANGUAGE_FILE_SEARCH,
            "gold_unit": "distinct_file",
            "result_unit": "distinct_file",
        }
        for task_id in task_ids
    }
    receipt = holdout_review.finalize_file_review_labels(
        checkout,
        replay_pack,
        contexts,
        all_pools,
        completed,
        adjudicated,
        contracts,
        seed=42,
        native_capture=capture_custody,
    )
    receipt_path = tmp_path / "native-reviewed-labels.json"
    receipt_path.write_bytes(ev.canonical(receipt))
    replay = ev.load_native_file_replay(
        checkout,
        tmp_path / "suite.json",
        tmp_path / "query-pack.json",
        tmp_path / "capture.json",
        reviewed_labels=receipt_path,
    )
    report = replay["report"]
    assert report["graded"] is True
    assert report["suite_commitment_sha256"] == ev.digest(ev.canonical(replay["suite"]))
    assert report["execution_suite_commitment_sha256"] == ev.digest(
        ev.canonical(replay["execution_suite"])
    )
    assert report["rank_metrics"]["comparison"]["sample_count"] == 1
    assert report["metrics"]["hybrid"]["file_mrr_at_10"] == pytest.approx(1 / 3)
    assert source_paths == tuple(
        (tmp_path / (name + ".json")).read_bytes() for name in ("suite", "query-pack", "capture")
    )
    (tmp_path / "capture.json").write_bytes(capture_bytes + b" ")
    with pytest.raises(ev.EvidenceError, match="frozen query pack or raw capture"):
        ev.load_native_file_replay(
            checkout,
            tmp_path / "suite.json",
            tmp_path / "query-pack.json",
            tmp_path / "capture.json",
            reviewed_labels=receipt_path,
        )


@pytest.mark.parametrize("source_local", [False, True])
def test_source_bound_native_evaluation_marks_inventory_only_verified(source_local):
    suite, pack, capture, source = source_bound_capture(source_local=source_local)
    report = native.evaluate(suite, pack, capture, source)
    assert report["indexed_universe_verified"] is True
    assert (
        report["indexed_universe_verification_scope"]
        == "frozen_body_refs_exact_view_source_owner_lookup_profile"
    )
    assert report["runtime_qualified"] is False
    assert report["product_default_decision"] is False
    assert report["metrics"]["hybrid"]["file_mrr_at_10"] == pytest.approx(1 / 3)


@pytest.mark.parametrize("field", ["root", "context"])
def test_native_source_local_coverage_cannot_name_a_different_source_cut(field):
    suite, pack, capture, source = source_bound_capture(source_local=True)
    for row in capture["tasks"]:
        coverage = row["hybrid"]["semantic_coverage"]
        coverage["desired"][field][0] ^= 1
        coverage["covered"][field][0] ^= 1
    with pytest.raises(
        ev.EvidenceError, match="Native semantic coverage differs from selected SourceCut"
    ):
        native.evaluate(suite, pack, capture, source)


@pytest.mark.parametrize(
    "change",
    ["foreign_head", "older_head", "future_floor", "settled_pinned", "settled_target_alive"],
)
def test_native_replay_rejects_settlement_the_sdk_would_refuse(change):
    suite, pack, capture, source = source_bound_capture()
    settlement = capture["settlement"]
    settlement["head"] = copy.deepcopy(settlement["head"])
    if change == "foreign_head":
        settlement["head"]["workspace"]["tenant"][0] ^= 1
    elif change == "older_head":
        settlement["head"]["sequence"] -= 1
    elif change == "future_floor":
        settlement["exact_floor"] = settlement["head"]["sequence"] + 1
    else:
        settlement["state"] = "SettledAtSnapshot"
        if change == "settled_pinned":
            settlement["head"]["sequence"] += 1
            settlement["exact_floor"] = settlement["head"]["sequence"]
            settlement["has_remaining_target_pins"] = True
        else:
            settlement["exact_floor"] = capture["view"]["sequence"]
    with pytest.raises(ev.EvidenceError, match="Native settlement binding differs"):
        native.evaluate(suite, pack, capture, source)


@pytest.mark.parametrize("foreign_family", [False, True])
def test_native_composite_replay_requires_eval_tasks_from_bound_source_family(
    monkeypatch, tmp_path, foreign_family
):
    from tools.benchmark.retrieval import composite_source_binding

    suite, pack, capture, source = source_bound_capture()
    family = "bound-family"
    for task in suite["tasks"]:
        task["query_family_id"] = family
    if foreign_family:
        suite["tasks"][-1]["query_family_id"] = "other-family"
    provenance = {"source_universe_sha256": "0" * 64, "source_binding_sha256": "1" * 64}
    suite["source_provenance"] = {"kind": "composite_git_releases_v1", **provenance}
    capture.update(provenance)
    pack["suite_commitment_sha256"] = capture["suite_commitment_sha256"] = ev.digest(
        ev.canonical(suite)
    )
    capture["query_pack_sha256"] = ev.digest(ev.canonical(pack))
    monkeypatch.setattr(
        composite_source_binding, "verify", lambda *_, **_kwargs: {"family_id": family}
    )
    source_inputs = (tmp_path, tmp_path, tmp_path, tmp_path, family)
    if foreign_family:
        with pytest.raises(
            ev.EvidenceError, match="Native composite eval tasks differ from source family"
        ):
            native.evaluate(suite, pack, capture, source, source_inputs)
    else:
        assert native.evaluate(suite, pack, capture, source, source_inputs)["graded"] is True


@pytest.mark.parametrize(
    "mode",
    [query_plan.DEFAULT_FILE_SEARCH, query_plan.NATURAL_LANGUAGE_FILE_SEARCH],
)
def test_native_natural_language_capture_respects_declared_suite_mode(mode):
    suite, pack, capture, source = source_bound_capture()
    for task in suite["tasks"]:
        task["query_intent"] = "semantic_intent"
        task["evaluation_contract"] = {
            "request_mode": mode,
            "gold_unit": "distinct_file",
            "result_unit": "distinct_file",
        }
    pack["suite_commitment_sha256"] = capture["suite_commitment_sha256"] = ev.digest(
        ev.canonical(suite)
    )
    capture["query_pack_sha256"] = ev.digest(ev.canonical(pack))
    if mode == query_plan.DEFAULT_FILE_SEARCH:
        with pytest.raises(ev.EvidenceError, match="Native file capture request mode differs"):
            native.evaluate(suite, pack, capture, source)
    else:
        assert native.evaluate(suite, pack, capture, source)["graded"] is True


def test_native_selected_profile_matches_after_json_key_reordering():
    _, _, capture, _ = source_bound_capture()
    native.verify_selected_profile(json.loads(json.dumps(capture, sort_keys=True)))


@pytest.mark.parametrize("field", ["schema", "policy"])
def test_native_semantic_coverage_requires_authenticated_profile(field):
    suite, pack, capture, source = source_bound_capture()
    for row in capture["tasks"]:
        coverage = row["hybrid"]["semantic_coverage"]
        coverage["desired"][field][0] ^= 1
        coverage["covered"][field][0] ^= 1
    with pytest.raises(
        ev.EvidenceError, match="Native semantic coverage differs from retained profile"
    ):
        native.evaluate(suite, pack, capture, source)


def test_native_view_digest_matches_rust_catalog_with_reordered_capture_keys():
    # Fixed native_view_v1 row emitted by Rust for the local benchmark workspace.
    identity = {
        "workspace": dict(
            zip(("tenant", "collection", "worktree", "slot"), ([n] * 16 for n in range(1, 5)))
        ),
        "epoch": 1,
        "sequence": 1,
        "digest": list(
            bytes.fromhex("a6c711aa458e83884ae3ce9814e48a425a1935d5c8dbe3007951beb5c503269c")
        ),
    }
    manifest = {
        "identity": identity,
        "profile": list(
            bytes.fromhex("6f1d82914a40548602620af8ba736699afb5fc3e45e1f23d235ced8ca7d93561")
        ),
        "source": {"owners": None, "paths": None, "frontiers": None},
        "projections": None,
        "members": list(
            bytes.fromhex("c2fc75ab0a6234e46ca59d7271ced08348ab02af304bcc5e110710e2c2f54af1")
        ),
        "coverage": list(
            bytes.fromhex("6ad09e7be9f7e5a6d28af4a2ecd3810629ea1a941a9d832b99d0f5eca07f2a9e")
        ),
    }
    native.verify_selected_view(json.loads(json.dumps(manifest, sort_keys=True)), identity)


@pytest.mark.parametrize("field", ["profile", "members", "coverage", "sources"])
def test_native_selected_view_rejects_manifest_changed_under_original_identity(field):
    suite, pack, capture, source = source_bound_capture()
    manifest = capture["selected_manifest"]
    if field == "sources":
        # Keep every copied SourceCut in agreement.
        manifest["source"]["paths"][0] ^= 1
        inventory = capture["indexed_universe"]
        inventory["source_cut"] = manifest["source"]
        for lookup in inventory["owner_lookups"]:
            lookup["source"] = manifest["source"]
    else:
        manifest[field] = [17] * 32
    capture["selected_profile"]["view"] = copy.deepcopy(manifest)
    with pytest.raises(ev.EvidenceError, match="Native selected view digest differs"):
        native.evaluate(suite, pack, capture, source)


@pytest.mark.parametrize("field", ["lexical_artifact", "semantic_coverage"])
def test_native_same_view_rejects_cross_query_resource_drift(field):
    suite, pack, capture = complete_capture()
    row = capture["tasks"][1]
    if field == "lexical_artifact":
        row["lexical"]["artifact"] = row["hybrid"][field] = [17] * 32
    else:
        row["hybrid"][field]["searchable"] = [17] * 32
    with pytest.raises(ev.EvidenceError, match="Native same-view query resources differ"):
        native.evaluate(suite, pack, capture)


@pytest.mark.parametrize("coverage", [None, "AsyncCoverage"])
def test_source_bound_replay_requires_core_text_coverage(coverage):
    suite, pack, capture, source = source_bound_capture()
    lexical = capture["tasks"][0]["lexical"]
    if coverage is None:
        del lexical["coverage"]
    else:
        lexical["coverage"] = coverage
    with pytest.raises(ev.EvidenceError, match="lexical coverage"):
        native.evaluate(suite, pack, capture, source)


@pytest.mark.parametrize("artifact", [None, "wrong-wire-type", [1] * 31, [True] + [1] * 31])
def test_source_bound_replay_requires_typed_lexical_artifact_even_if_rows_agree(artifact):
    suite, pack, capture, source = source_bound_capture()
    for row in capture["tasks"]:
        row["lexical"]["artifact"] = artifact
        row["hybrid"]["lexical_artifact"] = artifact
    with pytest.raises(ev.EvidenceError, match="lexical artifact"):
        native.evaluate(suite, pack, capture, source)


def test_source_bound_replay_refuses_boolean_hybrid_artifact_byte():
    suite, pack, capture, source = source_bound_capture()
    for row in capture["tasks"]:
        row["lexical"]["artifact"] = [1] * 32
        row["hybrid"]["lexical_artifact"] = [True] + [1] * 31
    with pytest.raises(ev.EvidenceError, match="Hybrid lexical artifact"):
        native.evaluate(suite, pack, capture, source)


@pytest.mark.parametrize("target", ["instance", "view", "hits", "matched", "file_top_k"])
def test_source_bound_replay_refuses_loose_lexical_pool_equality(target):
    suite, pack, capture, source = source_bound_capture()
    row = capture["tasks"][0]
    if target == "instance":
        row["lexical"]["instance"] = [6.0] + [6] * 15
    elif target == "view":
        row["lexical"]["view"] = dict(row["lexical"]["view"], epoch=True)
    elif target == "hits":
        row["lexical"]["hits"] = copy.deepcopy(row["lexical"]["hits"])
        row["lexical"]["hits"][0]["source"][0] = True
    elif target == "matched":
        row["hybrid"]["lexical_matched"] = float(row["lexical"]["matched"])
    else:
        row["hybrid"]["file_top_k"] = float(capture["file_top_k"])
    with pytest.raises(ev.EvidenceError, match="lexical (?:instance|pool|match)"):
        native.evaluate(suite, pack, capture, source)


@pytest.mark.parametrize("target", ["shared_view", "session", "covered", "semantic_instance"])
def test_source_bound_replay_checks_task_resources_against_pinned_authority(target):
    suite, pack, capture, source = source_bound_capture()
    row = capture["tasks"][0]
    hybrid = row["hybrid"]
    if target == "shared_view":
        row["lexical"]["view"] = hybrid["view"] = dict(hybrid["view"], epoch=True)
    elif target == "session":
        hybrid["session"] = [5.0] + [5] * 31
    elif target == "covered":
        coverage = hybrid["semantic_coverage"]
        for task in capture["tasks"]:
            task["hybrid"]["semantic_coverage"]["covered"] = dict(
                coverage["desired"], schema=[48.0] + [48] * 15
            )
    else:
        for task in capture["tasks"]:
            task["hybrid"]["semantic_instance"] = [8.0] + [8] * 15
            task["hybrid"]["semantic_coverage"]["instance"] = [8.0] + [8] * 15
    with pytest.raises(
        ev.EvidenceError, match="task view/session|semantic (?:source coverage|coverage instance)"
    ):
        native.evaluate(suite, pack, capture, source)


def test_native_selected_profile_rejects_alias_for_two_query_lanes():
    suite, pack, capture, source = source_bound_capture()
    capture["semantic_instance"] = capture["text_instance"]
    for row in capture["tasks"]:
        row["hybrid"]["semantic_instance"] = capture["semantic_instance"]
        row["hybrid"]["semantic_coverage"]["instance"] = capture["semantic_instance"]
    with pytest.raises(
        ev.EvidenceError, match="Native profile lane and file instances must be distinct"
    ):
        native.evaluate(suite, pack, capture, source)


def test_source_bound_native_evaluation_rejects_pool_identity_outside_sdk_inventory():
    suite, pack, capture, source = source_bound_capture()
    for row in capture["tasks"]:
        for lane in ("lexical_pool", "dense_pool"):
            for hit in row["hybrid"][lane]:
                if hit["path"] == "src/1.rs":
                    hit["source"] = [99] * 32
        row["pool_hashes"] = {
            "scheme": "qidx.native.file.pool.v1",
            "lexical": native.pool_hash("lexical", row["hybrid"]["lexical_pool"]),
            "dense": native.pool_hash("dense", row["hybrid"]["dense_pool"]),
        }
    with pytest.raises(ev.EvidenceError, match="authenticated indexed file"):
        native.evaluate(suite, pack, capture, source)


@pytest.mark.parametrize("mutation", ["missing", "session", "view", "instance", "duplicate"])
def test_source_bound_native_evaluation_requires_same_session_profile_instance(mutation):
    suite, pack, capture, source = source_bound_capture()
    profile = capture["selected_profile"]
    if mutation == "missing":
        del capture["selected_profile"]
    elif mutation == "session":
        profile["session"][0] ^= 1
    elif mutation == "view":
        profile["view"]["identity"]["sequence"] += 1
    elif mutation == "instance":
        profile["profile"]["instances"][2]["instance"] = [0] * 16
    else:
        profile["profile"]["instances"].append(copy.deepcopy(profile["profile"]["instances"][0]))
    with pytest.raises(ev.EvidenceError):
        native.evaluate(suite, pack, capture, source)


def test_source_bound_native_evaluation_rejects_profile_body_changed_at_same_view():
    suite, pack, capture, source = source_bound_capture()
    capture["selected_profile"]["profile"]["instances"][0]["policy"] = [17] * 32
    with pytest.raises(ev.EvidenceError, match="Native selected profile reference differs"):
        native.evaluate(suite, pack, capture, source)


@pytest.mark.parametrize("timing", [None, True, -1, 60_000_001])
def test_source_bound_native_evaluation_rejects_invalid_sdk_query_timing(timing):
    suite, pack, capture, source = source_bound_capture()
    row = capture["tasks"][0]
    if timing is None:
        del row["hybrid_micros"]
    else:
        row["hybrid_micros"] = timing
    with pytest.raises(ev.EvidenceError, match="Native task SDK timing exceeds its deadline"):
        native.evaluate(suite, pack, capture, source)


def test_complete_native_replay_scores_all_qrels_and_does_not_credit_nonempty_negative():
    suite, pack, capture = complete_capture()
    report = native.evaluate(suite, pack, capture)
    assert report["metrics"]["lexical"]["file_mrr_at_10"] == 0
    assert report["metrics"]["hybrid"]["file_mrr_at_10"] == pytest.approx(1 / 3)
    assert report["metrics"]["hybrid"]["file_recall_at_10"] == 1
    assert report["negative_count"] == 1
    assert report["false_positive_at_10"] == {"lexical": 0, "hybrid": 1}
    observed = {
        row["route"]: row["status"] for row in report["per_query"] if row["task_id"] == "negative"
    }
    assert observed == {"lexical": "abstained", "hybrid": "success"}
    comparison = report["rank_metrics"]["comparison"]
    assert comparison["no_answer_abstention_delta"]["sample_count"] == 1
    assert comparison["no_answer_abstention_delta"]["mean_delta"] == -1
    assert report["product_default_decision"] is False
    assert report["indexed_universe_verified"] is False


@pytest.mark.parametrize("duplicate_in", ["suite", "pack"])
def test_native_replay_rejects_duplicate_eval_task_identity(duplicate_in):
    suite, pack, capture = complete_capture()
    owner = suite if duplicate_in == "suite" else pack
    owner["tasks"].append(copy.deepcopy(owner["tasks"][0]))
    pack["suite_commitment_sha256"] = ev.digest(ev.canonical(suite))
    capture["suite_commitment_sha256"] = pack["suite_commitment_sha256"]
    capture["query_pack_sha256"] = ev.digest(ev.canonical(pack))

    with pytest.raises(ev.EvidenceError, match="Native .*task identit.*duplicate"):
        native.evaluate(suite, pack, capture)


@pytest.mark.parametrize(
    "traversal,stale",
    [({"Approximate": {"capped": False}}, 0), ("ExactCapped", 0), ("ExactExhausted", 1)],
)
def test_empty_negative_hybrid_requires_exhaustive_semantic_observation(traversal, stale):
    suite, pack, capture = complete_capture()
    negative = capture["tasks"][1]
    hybrid = negative["hybrid"]
    hybrid["dense_pool"] = []
    hybrid["hits"] = []
    hybrid["semantic_traversal"] = traversal
    hybrid["skipped_stale"] = stale
    hybrid["semantic_exhaustive"] = False
    negative["pool_hashes"]["dense"] = native.pool_hash("dense", [])
    with pytest.raises(ev.EvidenceError, match="empty negative|exhaustive"):
        native.evaluate(suite, pack, capture)


def test_empty_negative_hybrid_exact_exhaustion_can_be_scored():
    suite, pack, capture = complete_capture()
    negative = capture["tasks"][1]
    hybrid = negative["hybrid"]
    hybrid["dense_pool"] = []
    hybrid["hits"] = []
    hybrid["semantic_traversal"] = "ExactExhausted"
    hybrid["semantic_exhaustive"] = True
    negative["pool_hashes"]["dense"] = native.pool_hash("dense", [])
    report = native.evaluate(suite, pack, capture)
    assert report["false_positive_at_10"] == {"lexical": 0, "hybrid": 0}
    comparison = report["rank_metrics"]["comparison"]
    assert comparison["no_answer_abstention_delta"]["mean_delta"] == 0


def test_empty_positive_approximate_hybrid_is_scored_as_miss():
    suite, pack, capture = complete_capture()
    positive = capture["tasks"][0]
    positive["lexical"].update(matched=0, hits=[])
    positive["hybrid"].update(lexical_matched=0, lexical_pool=[], dense_pool=[], hits=[])
    positive["pool_hashes"] = {
        "scheme": "qidx.native.file.pool.v1",
        "lexical": native.pool_hash("lexical", []),
        "dense": native.pool_hash("dense", []),
    }
    report = native.evaluate(suite, pack, capture)
    assert report["metrics"]["hybrid"]["file_mrr_at_10"] == 0


def test_direct_native_scorer_refuses_pack_from_different_suite_with_fresh_capture_digest():
    suite, pack, capture = complete_capture()
    pack["suite_commitment_sha256"] = "0" * 64
    capture["query_pack_sha256"] = ev.digest(ev.canonical(pack))
    with pytest.raises(ev.EvidenceError, match="suite digest"):
        native.evaluate(suite, pack, capture)


@pytest.mark.parametrize("mutation", ["untyped", "composite_without_release_proof"])
def test_direct_native_scorer_requires_frozen_source_authority(mutation):
    suite, pack, capture = complete_capture()
    if mutation == "untyped":
        del suite["source_provenance"]
    else:
        suite["source_provenance"] = {
            "kind": "composite_git_releases_v1",
            "source_universe_sha256": "0" * 64,
            "source_binding_sha256": "1" * 64,
        }
        capture["source_universe_sha256"] = "0" * 64
        capture["source_binding_sha256"] = "1" * 64
    pack["suite_commitment_sha256"] = ev.digest(ev.canonical(suite))
    capture["suite_commitment_sha256"] = pack["suite_commitment_sha256"]
    capture["query_pack_sha256"] = ev.digest(ev.canonical(pack))
    with pytest.raises(ev.EvidenceError, match="source"):
        native.evaluate(suite, pack, capture)


@pytest.mark.parametrize(
    "constraint",
    [
        {"language_any_of": ["rust"]},
        {"language_any_of": [], "repo_relative_path_exact": "src/4.rs"},
    ],
)
def test_native_replay_refuses_equally_filtered_lanes_outside_frozen_query_policy(constraint):
    suite, pack, capture = complete_capture()
    for row in capture["tasks"]:
        row["text_request"]["constraints"] = copy.deepcopy(constraint)
        row["semantic_request"]["constraints"] = copy.deepcopy(constraint)
    with pytest.raises(ev.EvidenceError, match="eligibility"):
        native.evaluate(suite, pack, capture)


def test_native_replay_refuses_candidate_depth_different_from_frozen_pack():
    suite, pack, capture = complete_capture()
    capture["lane_top_k"] = 11
    for row in capture["tasks"]:
        row["text_request"]["top_k"] = row["semantic_request"]["top_k"] = 11
    with pytest.raises(ev.EvidenceError, match="depth"):
        native.evaluate(suite, pack, capture)


def test_native_replay_refuses_short_output_as_top10_evidence():
    suite, pack, capture = complete_capture()
    capture["file_top_k"] = 1
    for row in capture["tasks"]:
        row["hybrid"]["file_top_k"] = 1
        row["hybrid"]["hits"] = row["hybrid"]["hits"][:1]
    with pytest.raises(ev.EvidenceError, match="top10"):
        native.evaluate(suite, pack, capture)


@pytest.mark.parametrize(
    "mutation", ["denominator", "query", "eligibility", "view", "release", "unjudged"]
)
def test_complete_native_replay_refuses_changed_inputs_and_missing_judgments(mutation):
    suite, pack, capture = complete_capture()
    if mutation == "denominator":
        capture["tasks"].pop()
    elif mutation == "query":
        capture["tasks"][0]["text_request"]["query_text"] = "wrong"
    elif mutation == "eligibility":
        capture["tasks"][0]["semantic_request"]["constraints"] = {"path": "foreign"}
    elif mutation == "view":
        capture["tasks"][0]["hybrid"]["view"] = "foreign"
    elif mutation == "release":
        capture["released_session"]["state"] = "Active"
    else:
        suite["tasks"][0]["file_judgments"].pop(0)
        pack["suite_commitment_sha256"] = ev.digest(ev.canonical(suite))
        capture["suite_commitment_sha256"] = pack["suite_commitment_sha256"]
        capture["query_pack_sha256"] = ev.digest(ev.canonical(pack))
    with pytest.raises(ev.EvidenceError):
        native.evaluate(suite, pack, capture)


def _git_backed_native_replay_inputs(tmp_path: Path) -> tuple[Path, Path, Path, Path]:
    """Materialize the fixed SDK packet against actual committed source bytes."""
    import subprocess

    suite, _unused_pack, capture, fixture_source = source_bound_capture(source_local=True)
    repo = tmp_path / "source"
    repo.mkdir()
    for name, body in fixture_source.raw.items():
        target = repo / name
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(body)
    subprocess.run(["git", "init", "-q", str(repo)], check=True)
    subprocess.run(["git", "-C", str(repo), "add", "--all"], check=True)
    subprocess.run(
        [
            "git",
            "-C",
            str(repo),
            "-c",
            "user.name=Native fixture",
            "-c",
            "user.email=native@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "source",
        ],
        check=True,
    )
    commit = subprocess.check_output(
        ["git", "-C", str(repo), "rev-parse", "HEAD"], text=True
    ).strip()
    suite.update(
        schema_version=3,
        repository_commit=commit,
        routes=["lexical", "hybrid"],
        diagnostic_policy="observed_prefix_v1",
        comparison_contract={
            "top_k": 10,
            "tokenizer": "qi-regex-v1",
            "tokenizer_budget_version": "qb-v1",
            "output_unit_policy": "rank_prefix",
            "span_unit": "byte_span_with_line_projection_v1",
        },
    )
    gold_body = fixture_source.raw["src/4.rs"]
    suite["tasks"][0]["gold"] = [
        {
            "path": "src/4.rs",
            "start_line": 1,
            "end_line": 1,
            "start_byte": 0,
            "end_byte": len(gold_body),
            "file_sha256": ev.digest(gold_body),
            "block_sha256": ev.digest(gold_body),
            "grade": 2,
        }
    ]
    for task in suite["tasks"]:
        task["query_sha256"] = ev.digest(task["query"].encode())
        task["label_review"] = {
            "assessment": "reviewed_unambiguous",
            "reviewer_id": "synthetic-native-fixture",
            "evidence_sha256": ev.digest(b"synthetic complete four-file gold"),
        }
    _validated, pack, _actual_source = ev.validate_suite(repo, suite)
    capture["repository_commit"] = commit
    capture["suite_commitment_sha256"] = ev.digest(ev.canonical(suite))
    capture["query_pack_sha256"] = ev.digest(ev.canonical(pack))
    suite_path, pack_path, capture_path = (
        tmp_path / "suite.json",
        tmp_path / "pack.json",
        tmp_path / "capture.json",
    )
    suite_path.write_text(json.dumps(suite), encoding="utf-8")
    pack_bytes = json.dumps(pack, sort_keys=True).encode()
    pack_path.write_bytes(pack_bytes)
    capture["query_pack_file_sha256"] = ev.digest(pack_bytes)
    capture_path.write_text(json.dumps(capture), encoding="utf-8")
    return repo, suite_path, pack_path, capture_path


def test_native_file_loader_replays_committed_git_sdk_packet_and_fixed_gold(tmp_path: Path):
    repo, suite, pack, capture = _git_backed_native_replay_inputs(tmp_path)
    captured = capture.read_bytes()
    commitment = ("sha256:" + ev.digest(captured), len(captured))
    report = ev.load_native_file_replay(repo, suite, pack, capture, expected_capture=commitment)[
        "report"
    ]
    assert report["indexed_universe_verified"] is True
    assert report["metrics"]["lexical"]["file_mrr_at_10"] == 0
    assert report["metrics"]["hybrid"]["file_mrr_at_10"] == pytest.approx(1 / 3)
    assert report["metrics"]["hybrid"]["file_recall_at_10"] == 1
    assert report["negative_count"] == 1
    assert report["false_positive_at_10"] == {"lexical": 0, "hybrid": 1}
    assert report["runtime_qualified"] is False
    assert report["product_default_decision"] is False

    capture.write_bytes(captured + b" ")
    with pytest.raises(ev.EvidenceError, match="controlled execution commitment"):
        ev.load_native_file_replay(repo, suite, pack, capture, expected_capture=commitment)


def test_native_file_loader_refuses_numeric_alias_in_frozen_blind_pack(tmp_path: Path):
    repo, suite, pack_path, capture_path = _git_backed_native_replay_inputs(tmp_path)
    observed_pack = json.loads(pack_path.read_bytes())
    assert observed_pack["comparison_contract"]["top_k"] == 10
    observed_pack["comparison_contract"]["top_k"] = 10.0
    pack_bytes = json.dumps(observed_pack, sort_keys=True).encode()
    pack_path.write_bytes(pack_bytes)

    capture = json.loads(capture_path.read_bytes())
    capture["query_pack_file_sha256"] = ev.digest(pack_bytes)
    capture_bytes = json.dumps(capture).encode()
    capture_path.write_bytes(capture_bytes)

    with pytest.raises(ev.EvidenceError, match="Native frozen blind pack differs"):
        ev.load_native_file_replay(
            repo,
            suite,
            pack_path,
            capture_path,
            expected_capture=("sha256:" + ev.digest(capture_bytes), len(capture_bytes)),
        )


@pytest.mark.parametrize("mutation", ["source_bytes", "head_commit"])
def test_native_file_loader_rejects_changed_git_source_without_core_mocks(
    tmp_path: Path, mutation: str
):
    import subprocess

    repo, suite, pack, capture = _git_backed_native_replay_inputs(tmp_path)
    (repo / "src/4.rs").write_bytes(b"fn changed_after_freeze() {}\n")
    if mutation == "head_commit":
        subprocess.run(["git", "-C", str(repo), "add", "--all"], check=True)
        subprocess.run(
            [
                "git",
                "-C",
                str(repo),
                "-c",
                "user.name=Native fixture",
                "-c",
                "user.email=native@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-qm",
                "changed",
            ],
            check=True,
        )
    expected = (
        "checkout has tracked or untracked changes"
        if mutation == "source_bytes"
        else "checkout HEAD differs from frozen commit"
    )
    with pytest.raises(ev.EvidenceError, match=expected):
        ev.load_native_file_replay(repo, suite, pack, capture)


@pytest.mark.parametrize(
    ("change", "error"),
    [
        ("head", "checkout HEAD differs from frozen commit"),
        ("directory", "checkout directory changed during verification"),
    ],
)
def test_native_frozen_repo_rejects_changes_during_clean_status(
    tmp_path, monkeypatch, change, error
):
    repo, _, _, _ = _git_backed_native_replay_inputs(tmp_path)
    real_git = ev.retrieval_contract._git
    frozen_head = real_git(repo, "rev-parse", "HEAD")
    replacement = tmp_path / "replacement"
    if change == "directory":
        real_git(repo, "clone", "-q", "--no-hardlinks", str(repo), str(replacement))
        (replacement / "src/4.rs").write_text("fn changed_after_freeze() {}\n")

    def git_then_change_checkout(checkout, *args):
        result = real_git(checkout, *args)
        if args[0] == "status":
            assert result == ""
            if change == "head":
                real_git(
                    checkout,
                    "-c",
                    "user.name=Native fixture",
                    "-c",
                    "user.email=native@example.invalid",
                    "-c",
                    "commit.gpgsign=false",
                    "commit",
                    "--allow-empty",
                    "-qm",
                    "replacement",
                )
            else:
                repo.rename(tmp_path / "original")
                replacement.rename(repo)
        return result

    monkeypatch.setattr(ev.retrieval_contract, "_git", git_then_change_checkout)
    with pytest.raises(ev.EvidenceError, match=error):
        ev.verify_repo(repo, frozen_head)

    head = real_git(repo, "rev-parse", "HEAD")
    status = real_git(repo, "status", "--porcelain", "--untracked-files=all")
    if change == "head":
        assert head != frozen_head and not status
    else:
        assert head == frozen_head and status == "M src/4.rs"
