"""Independent wire goldens for explicit hybrid file-RRF evidence."""

from __future__ import annotations

import hashlib
import json
import math
import struct
from copy import deepcopy

import pytest

from tools.benchmark.retrieval import evaluator, query_plan, run
from tools.benchmark.retrieval.hybrid_file_evidence import (
    HybridFileEvidenceError,
    ordered_pool_sha256_v1,
    validate_result,
    validate_route_configuration,
)

REPO = "fixture-repo"
REVISION = "fixture-revision"
ALPHA = b"alpha token\n"
BETA = b"beta token\n"
ALPHA_SHA256 = "b6306c3452ada0ae273ca63077fd968053012a07a41969e641d6aa1292f63078"
BETA_SHA256 = "61bf3fe21a118aebee2baf16a0a0968e34f4a4ff7af99657d3200d6511d59384"
ALPHA_RRF = 0.03278688524590164  # Two independent file-rank-one witnesses.
BETA_RRF = 0.016129032258064516  # Dense file rank two only.


def _fixed_pool_digest(rows: list[tuple[str, str, float]]) -> list[int]:
    """Test oracle assembled from the public transcript, independent of production code."""
    transcript = bytearray(b"quanta-index/hybrid-file/ordered-pool/v1\0")

    def string(value: str) -> None:
        encoded = value.encode("utf-8")
        transcript.extend(struct.pack("<Q", len(encoded)))
        transcript.extend(encoded)

    string(REPO)
    string(REVISION)
    transcript.extend(struct.pack("<Q", 1))
    transcript.extend(struct.pack("<Q", len(rows)))
    for path, candidate_id, score in rows:
        string(REPO)
        string(path)
        string(candidate_id)
        transcript.extend(struct.pack("<f", score))
    return list(hashlib.sha256(transcript).digest())


@pytest.mark.parametrize("selector", ["potion-code", "potion-code-full-v2"])
def test_hybrid_file_model_revision_cannot_bypass_declared_selector(selector):
    from tools.benchmark.retrieval import run

    validated = {"capture": {"system": "quanta", "run": {
        "route_provenance": {"hybrid_file": {"capture_id": "model"}},
        "captures": {"model": {"model": "other-real-model", "model_revision": "other-revision"}},
    }}}
    assert not run._quanta_semantic_capture_identity_matches(validated, selector, {"hybrid_file"})
    suffix = "full-length-v1" if selector == "potion-code" else "full-length-v2"
    validated["capture"]["run"]["captures"]["model"] = {
        "model": "model2vec:minishlab/potion-code-16M-v2",
        "model_revision": (
            "e9d2a44ca6a05ac6685f3b23709ea57eb7352d5b:model2vec-rs-0.3.0:fancy-regex:" + suffix
        ),
    }
    assert run._quanta_semantic_capture_identity_matches(validated, selector, {"hybrid_file"})
    validated["capture"]["run"]["route_provenance"] = {}
    assert not run._quanta_semantic_capture_identity_matches(validated, selector, {"hybrid_file"})


def test_unsupported_hybrid_file_pair_is_refused_before_capture():
    from tools.benchmark.retrieval import run

    spec = {
        "execution_profiles": {
            "quanta": {"policy": "natural_language"},
            "semble": {"mode": "lexical-file"},
        },
        "routes": ["hybrid_file"],
        "top_k": 10,
        "file_top_k": 5,
    }
    with pytest.raises(run.RunError, match="hybrid_file.*paired"):
        run._validate_file_pair_contract(spec, paired=True)
    assert not run._validate_file_pair_contract(spec, paired=False)


def test_driver_binds_record_file_cap_to_requested_spec_before_diagnostic():
    spec = {"file_top_k": 3}
    run._bind_record_file_cap_to_spec(spec, ["hybrid_file"], {"file_top_k": 3})
    for observed in ({"file_top_k": 2}, {}, {"file_top_k": True}):
        with pytest.raises(run.RunError, match="cap differs from declared spec"):
            run._bind_record_file_cap_to_spec(spec, ["hybrid_file"], observed)
    with pytest.raises(run.RunError, match="lacks the explicit hybrid_file route"):
        run._bind_record_file_cap_to_spec(spec, ["lexical"], {"file_top_k": 3})


def _capture() -> dict:
    return {
        "system": "quanta",
        "execution_profile": {"policy": "natural_language"},
        "generation": 1,
        "source_repo_id": REPO,
        "source_revision_id": REVISION,
    }


def _window(returned: int, lane: str) -> dict:
    window = {
        "returned": returned,
        "candidate_count": {"kind": "exact", "value": returned},
        "outcome": {"kind": "exact_exhausted"},
        "coverage": {
            "examined": {"kind": "exact", "value": returned},
            "exhaustion_proof": {"kind": "probe_exhausted", "fetched": returned},
            "lanes": [{
                "lane": "hybrid_file." + lane,
                "executed": True,
                "contributed": returned > 0,
                "filtered_out": 0,
                "candidates": {"kind": "exact", "value": returned},
            }],
        },
    }
    if returned == 0:
        window["empty_provenance"] = "available_empty"
    return window


def _candidate(path: str, unit_id: str, score: float, snippet: str) -> dict:
    return {
        "source_repo_id": REPO,
        "source": None,
        "preview": None,
        "candidate_id": unit_id,
        "repo_id": REPO,
        "revision_id": REVISION,
        "manifest_generation": 1,
        "repo_relative_path": path,
        "start_line": 1,
        "end_line": 1,
        "score": score,
        "snippet": snippet,
        "snippet_hit_offset": 0,
        "highlights": [{"start": 0, "len": 5}],
    }


def _witness(path: str, unit_id: str, file_rank: int, chunk_rank: int, score: float,
             snippet: str) -> dict:
    return {
        "file_rank": file_rank,
        "chunk_rank": chunk_rank,
        "candidate": _candidate(path, unit_id, score, snippet),
    }


def _proved_row(path: str, unit_id: str, source: bytes, digest: str, score: float,
                rank: int) -> dict:
    # A one-line published chunk spans the complete immutable source fixture.
    return {
        "path": path,
        "start_byte": 0,
        "end_byte": len(source),
        "start_line": 1,
        "end_line": 1,
        "file_sha256": digest,
        "block_sha256": digest,
        "tokens": 2,
        "rank": rank,
        "span_accounting": {
            "unit_kind": "chunk",
            "unit_id": unit_id,
            "producer_identity": "fixture-published-chunk",
            "indexed_start_byte": 0,
            "indexed_end_byte": len(source),
            "sdk_start_line": 1,
            "sdk_end_line": 1,
            "extra_context_bytes": 0,
        },
        "score": score,
    }


def _result() -> dict:
    a = "src/alpha.go"
    b = "src/beta.go"
    return {
        "task_id": "HF-1",
        "route": "hybrid_file",
        "status": "success",
        "rank_unit": "distinct_file",
        "ordering": "file_rrf_desc_lexical_presence_key_v1",
        "score_evidence": "hybrid_file_rrf_v1",
        "candidates": [
            _proved_row(a, "alpha-lexical-chunk", ALPHA, ALPHA_SHA256, ALPHA_RRF, 1),
            _proved_row(b, "beta-dense-chunk", BETA, BETA_SHA256, BETA_RRF, 2),
        ],
        "error": None,
        "hybrid_file_evidence": {
            "schema_version": 1,
            "lane_top_k": 4,
            "file_top_k": 3,
            "response": {
                "generation": {
                    "repo_id": REPO,
                    "revision_id": REVISION,
                    "manifest_generation": 1,
                },
                "selected_active_head": {
                    "generation": {
                        lane.lower(): {"repo_id": REPO, "revision_id": REVISION,
                                       "manifest_generation": 1, "track": lane,
                                       "manifest_digest": "manifest-digest"}
                        for lane in ("Lexical", "Semantic")
                    } | {"semantic_content": {
                        "row_root_digest": "sha256:" + "a" * 64,
                        "membership_root_digest": "sha256:" + "b" * 64,
                    }},
                    "activation_token": {"root_incarnation": [1] * 16,
                                         "activation_sequence": 1},
                },
                "file_top_k": 3,
                "bounded_file_count": 2,
                "lexical_pool_sha256": _fixed_pool_digest([
                    (a, "alpha-lexical-chunk", 0.75),
                ]),
                "dense_pool_sha256": _fixed_pool_digest([
                    (a, "alpha-dense-chunk", 0.25),
                    (b, "beta-dense-chunk", 0.5),
                ]),
                "results": [
                    {
                        "file": {"source_repo_id": REPO, "repo_relative_path": a},
                        "lexical": _witness(a, "alpha-lexical-chunk", 1, 1, 0.75,
                                            "alpha token\n"),
                        "dense": _witness(a, "alpha-dense-chunk", 1, 1, 0.25,
                                          "alpha token\n"),
                        "file_rrf_score": ALPHA_RRF,
                    },
                    {
                        "file": {"source_repo_id": REPO, "repo_relative_path": b},
                        "lexical": None,
                        "dense": _witness(b, "beta-dense-chunk", 2, 2, 0.5,
                                          "beta token\n"),
                        "file_rrf_score": BETA_RRF,
                    },
                ],
                "lexical_window": _window(1, "lexical"),
                "dense_window": _window(2, "dense"),
                "explanation": {
                    "planner_trace": [],
                    "engines_touched": [],
                    "engines_executed": [],
                    "request_id": 0,
                    "stage_timings": None,
                    "early_stop_reason": None,
                    "contributions": [],
                    "ranker_weights_hash": [0] * 32,
                    "strategy": "",
                    "summary": "",
                },
            },
        },
    }


def _response(result: dict) -> dict:
    return result["hybrid_file_evidence"]["response"]


def test_file_witness_accepts_source_bound_utf8_preview_and_refuses_metadata_drift() -> None:
    row = _result()
    candidate = _response(row)["results"][0]["lexical"]["candidate"]
    snippet = "éclair token\n"
    source_digest = hashlib.sha256(snippet.encode()).hexdigest()
    row["candidates"][0].update(
        end_byte=len(snippet.encode()),
        file_sha256=source_digest,
        block_sha256=source_digest,
    )
    source = {
        "file": {"source_repo_id": REPO, "repo_relative_path": "src/alpha.go"},
        "revision_id": REVISION,
        "source_sha256": list(bytes.fromhex(source_digest)),
    }
    candidate["source"] = source
    candidate["preview"] = {
        "kind": "source_chunk",
        "source": deepcopy(source),
        "chunk_start_byte": 0,
        "original_focus": {"start": 0, "end": 2},
        "original_context": {"start": 0, "end": len(snippet.encode())},
        "normalized_focus": {"start": 0, "end": 2},
        "normalization_equivalent": False,
        "unavailable_reason": None,
    }
    candidate["snippet"] = snippet
    candidate["highlights"] = [{"start": 0, "len": 2}]
    validate_result(row, _capture(), 4)

    bad_source = deepcopy(row)
    _response(bad_source)["results"][0]["lexical"]["candidate"]["source"]["revision_id"] = "wrong"
    with pytest.raises(HybridFileEvidenceError):
        validate_result(bad_source, _capture(), 4)

    bad_preview = deepcopy(row)
    _response(bad_preview)["results"][0]["lexical"]["candidate"]["preview"]["original_context"]["end"] -= 1
    with pytest.raises(HybridFileEvidenceError):
        validate_result(bad_preview, _capture(), 4)

    bad_highlight = deepcopy(row)
    _response(bad_highlight)["results"][0]["lexical"]["candidate"]["highlights"][0]["len"] = 1
    with pytest.raises(HybridFileEvidenceError):
        validate_result(bad_highlight, _capture(), 4)


def test_file_witness_accepts_optional_source_and_preview_wire_variants() -> None:
    source_only = _result()
    candidate = _response(source_only)["results"][0]["lexical"]["candidate"]
    source = {
        "file": {"source_repo_id": REPO, "repo_relative_path": "src/alpha.go"},
        "revision_id": "source-revision-independent-of-generation",
        "source_sha256": list(bytes.fromhex(ALPHA_SHA256)),
    }
    candidate["source"] = deepcopy(source)
    validate_result(source_only, _capture(), 4)

    malformed_projection = deepcopy(source_only)
    malformed_projection["candidates"][0] = None
    with pytest.raises(HybridFileEvidenceError, match="representative projection differs"):
        validate_result(malformed_projection, _capture(), 4)

    path_preview = deepcopy(source_only)
    candidate = _response(path_preview)["results"][0]["lexical"]["candidate"]
    candidate["snippet"] = "src/alpha.go"
    candidate["highlights"] = [{"start": 0, "len": 3}]
    candidate["preview"] = {
        "kind": "path", "source": deepcopy(source), "chunk_start_byte": None,
        "original_focus": None, "original_context": None, "normalized_focus": None,
        "normalization_equivalent": False, "unavailable_reason": None,
    }
    validate_result(path_preview, _capture(), 4)

    unavailable = _result()
    candidate = _response(unavailable)["results"][0]["lexical"]["candidate"]
    candidate["snippet"] = ""
    candidate["snippet_hit_offset"] = None
    candidate["highlights"] = []
    candidate["preview"] = {
        "kind": "source_chunk", "source": None, "chunk_start_byte": None,
        "original_focus": None, "original_context": None, "normalized_focus": None,
        "normalization_equivalent": False, "unavailable_reason": "source_not_provided",
    }
    validate_result(unavailable, _capture(), 4)


@pytest.mark.parametrize("lane", ["lexical", "dense"])
def test_scored_file_witness_refuses_source_digest_different_from_proved_file(lane: str) -> None:
    row = _result()
    candidate = _response(row)["results"][0][lane]["candidate"]
    candidate["source"] = {
        "file": {"source_repo_id": REPO, "repo_relative_path": "src/alpha.go"},
        "revision_id": "source-revision-independent-of-generation",
        "source_sha256": [0] * 32,
    }
    with pytest.raises(HybridFileEvidenceError, match="source hash differs from proved file"):
        validate_result(row, _capture(), 4)


def test_file_witness_binds_unanchored_dense_span_to_record_accounting() -> None:
    result = _result()
    for hit in _response(result)["results"]:
        hit["dense"]["candidate"].update(
            start_line=0, end_line=0, snippet_hit_offset=None, highlights=[]
        )
    projected = result["candidates"][1]["span_accounting"]
    projected.update(sdk_start_line=0, sdk_end_line=0)
    validate_result(result, _capture(), 4)

    projected.update(sdk_start_line=1, sdk_end_line=1)
    with pytest.raises(HybridFileEvidenceError, match="representative projection differs"):
        validate_result(result, _capture(), 4)
    projected.update(sdk_start_line=0, sdk_end_line=0)

    candidate = _response(result)["results"][1]["dense"]["candidate"]
    candidate["end_line"] = 1
    with pytest.raises(HybridFileEvidenceError, match="line span"):
        validate_result(result, _capture(), 4)


def test_full_file_page_requires_all_lane_file_ranks() -> None:
    result = _result()
    response = _response(result)
    first = response["results"][0]
    first["dense"] = None
    first["file_rrf_score"] = result["candidates"][0]["score"] = 1 / 61
    with pytest.raises(HybridFileEvidenceError, match="dense witness ranks"):
        validate_result(result, _capture(), 4)

    evidence = result["hybrid_file_evidence"]
    evidence["file_top_k"] = response["file_top_k"] = 1
    response["results"] = response["results"][:1]
    result["candidates"] = result["candidates"][:1]
    result["status"] = "capped"
    validate_result(result, _capture(), 4)


def test_file_witness_refuses_score_outside_finite_f32() -> None:
    result = _result()
    _response(result)["results"][0]["dense"]["candidate"]["score"] = 1e300

    with pytest.raises(HybridFileEvidenceError, match="candidate score exceeds finite f32"):
        validate_result(result, _capture(), 4)


@pytest.mark.parametrize("mutation", [
    lambda head: None,
    lambda head: {**head, "activation_token": {**head["activation_token"],
                                                "root_incarnation": [0] * 16}},
    lambda head: {**head, "generation": {**head["generation"], "semantic": {
        **head["generation"]["semantic"], "manifest_generation": 2,
    }}},
], ids=["missing", "zero-activation-root", "semantic-generation-drift"])
def test_bench_record_requires_valid_sdk_active_head(mutation) -> None:
    result = _result()
    response = _response(result)
    validate_result(result, _capture(), 4)
    response["selected_active_head"] = mutation(response["selected_active_head"])

    with pytest.raises(HybridFileEvidenceError, match="active head|activation root"):
        validate_result(result, _capture(), 4)


def test_file_witness_refuses_score_perturbation_that_reverses_a_tie() -> None:
    result = _result()
    response = _response(result)
    alpha, beta = response["results"]
    alpha["dense"] = None
    beta["dense"].update(file_rank=1, chunk_rank=1)
    response["dense_window"] = _window(1, "dense")
    response["dense_pool_sha256"] = _fixed_pool_digest([
        ("src/beta.go", "beta-dense-chunk", 0.5),
    ])
    tied_score = 1 / 61
    for hit, row in zip(response["results"], result["candidates"]):
        hit["file_rrf_score"] = row["score"] = tied_score
    validate_result(result, _capture(), 4)

    response["results"].reverse()
    result["candidates"].reverse()
    for rank, row in enumerate(result["candidates"], 1):
        row["rank"] = rank
    beta["file_rrf_score"] = result["candidates"][0]["score"] = math.nextafter(
        tied_score, math.inf
    )
    with pytest.raises(HybridFileEvidenceError, match="RRF score differs"):
        validate_result(result, _capture(), 4)


def test_full_witness_pool_refuses_digest_drift_from_nonrepresentative_candidate() -> None:
    result = _result()
    validate_result(result, _capture(), 4)
    _response(result)["results"][0]["dense"]["candidate"]["candidate_id"] = "forged-id"

    with pytest.raises(HybridFileEvidenceError, match="dense pool digest differs"):
        validate_result(result, _capture(), 4)


def test_truncated_file_page_requires_space_for_intervening_files() -> None:
    result = _result()
    evidence = result["hybrid_file_evidence"]
    response = _response(result)
    evidence["file_top_k"] = response["file_top_k"] = 2
    response["bounded_file_count"] = 4
    response["dense_window"] = _window(4, "dense")
    result["status"] = "capped"
    first, second = response["results"]
    first["dense"].update(file_rank=2, chunk_rank=3)
    second["dense"].update(file_rank=4, chunk_rank=4)
    first["file_rrf_score"] = result["candidates"][0]["score"] = 1 / 61 + 1 / 62
    second["file_rrf_score"] = result["candidates"][1]["score"] = 1 / 64
    with pytest.raises(HybridFileEvidenceError, match="dense witness ranks"):
        validate_result(result, _capture(), 4)


def test_ordered_pool_digest_matches_independent_f32_transcript() -> None:
    pin = {"repo_id": REPO, "revision_id": REVISION, "manifest_generation": 1}
    rows = [
        {"source_repo_id": REPO, "path": "src/alpha.go",
         "candidate_id": "alpha-lexical-chunk", "score": 0.75},
        {"source_repo_id": REPO, "path": "src/beta.go",
         "candidate_id": "beta-dense-chunk", "score": 0.5},
    ]
    expected = _fixed_pool_digest([
        ("src/alpha.go", "alpha-lexical-chunk", 0.75),
        ("src/beta.go", "beta-dense-chunk", 0.5),
    ])
    assert bytes(expected).hex() == "e303e7b4ea3cd717736fa324845185caec3f226124aba6c2590c71f3a1d6457e"
    assert ordered_pool_sha256_v1(pin, rows) == expected
    assert ordered_pool_sha256_v1(pin, list(reversed(rows))) != expected
    assert ordered_pool_sha256_v1(pin, rows[:1]) != expected
    other_source = [{**rows[0], "source_repo_id": "other-source"}]
    assert ordered_pool_sha256_v1(pin, other_source) != ordered_pool_sha256_v1(pin, rows[:1])
    assert ordered_pool_sha256_v1(pin, other_source) == ordered_pool_sha256_v1(
        pin, deepcopy(other_source))


def test_ordered_pool_digest_f32_json_roundtrip_and_signed_zero_fixed_golden() -> None:
    pin = {"repo_id": "generic-file", "revision_id": "rev-1", "manifest_generation": 1}
    rows = [
        {"source_repo_id": "other-source", "path": "src/a.go",
         "candidate_id": "positive", "score": 0.1},
        {"source_repo_id": "generic-file", "path": "src/b.go",
         "candidate_id": "negative", "score": -0.0},
    ]
    transcript = bytearray(b"quanta-index/hybrid-file/ordered-pool/v1\0")

    def string(value: str) -> None:
        encoded = value.encode("utf-8")
        transcript.extend(struct.pack("<Q", len(encoded)))
        transcript.extend(encoded)

    string("generic-file")
    string("rev-1")
    transcript.extend(struct.pack("<Q", 1))
    transcript.extend(struct.pack("<Q", 2))
    for row in rows:
        string(row["source_repo_id"])
        string(row["path"])
        string(row["candidate_id"])
        transcript.extend(struct.pack("<f", row["score"]))
    assert struct.pack("<f", 0.1).hex() == "cdcccc3d"
    assert struct.pack("<f", -0.0).hex() == "00000080"
    assert hashlib.sha256(transcript).hexdigest() == (
        "5569c8cd927f2483a724f8b5622fde71b45797ae70dc50cb59dff88657023bdc"
    )
    digest = ordered_pool_sha256_v1(pin, rows)
    assert bytes(digest).hex() == hashlib.sha256(transcript).hexdigest()
    decoded = json.loads(json.dumps(rows))
    assert ordered_pool_sha256_v1(pin, decoded) == digest
    assert ordered_pool_sha256_v1(pin, list(reversed(rows))) != digest
    changed = deepcopy(rows)
    changed[0]["score"] = 0.2
    assert ordered_pool_sha256_v1(pin, changed) != digest
    changed = deepcopy(rows)
    changed[1]["score"] = 0.0
    assert ordered_pool_sha256_v1(pin, changed) != digest


@pytest.mark.parametrize("field", ["lexical_pool_sha256", "dense_pool_sha256"])
def test_file_pool_digest_wire_is_required_and_byte_exact(field: str) -> None:
    row = _result()
    _response(row).pop(field)
    with pytest.raises(HybridFileEvidenceError, match="response fields"):
        validate_result(row, _capture(), 4)
    row = _result()
    _response(row)[field] = [0] * 31
    with pytest.raises(HybridFileEvidenceError, match="SHA-256 byte array"):
        validate_result(row, _capture(), 4)


def test_exhaustion_count_boolean_cannot_substitute_for_one_returned_chunk():
    result = _result()
    _response(result)["lexical_window"]["coverage"]["exhaustion_proof"]["fetched"] = True
    with pytest.raises(HybridFileEvidenceError, match="exhaustion_proof"):
        validate_result(result, _capture(), 4)


def _completed_file_row() -> dict:
    row = json.loads(
        '{"route":"hybrid_file","status":"success","candidates":[{"path":"src/a.rs","score":0.5}],'
        '"timings":{"query_latency_ms":1.25},"hybrid_file_evidence":{"schema_version":1,'
        '"lane_top_k":8,"file_top_k":3,"response":{"generation":{"repo_id":"r"},'
        '"file_top_k":3,"bounded_file_count":1,'
        '"results":[{"file":{"source_repo_id":"r","repo_relative_path":"src/a.rs"},'
        '"file_rrf_score":0.5,"lexical":{"file_rank":1,"candidate":{"score":-0.0}},"dense":null}],'
        '"lexical_window":{"returned":1},"dense_window":{"returned":0},'
        '"explanation":{"request_id":7,"stage_timings":[1.5]}}}}'
    )
    response = row["hybrid_file_evidence"]["response"]
    response["lexical_pool_sha256"] = [1] * 32
    response["dense_pool_sha256"] = [2] * 32
    return row


def test_completed_file_response_keeps_fixed_cross_language_score_bits_without_mutation():
    from tools.benchmark.retrieval.retrieval_contract import completed_output_sha256

    row = _completed_file_row()
    before = deepcopy(row)
    assert completed_output_sha256(row) == (
        "bc83e7d83d8f9fb08d27db4bb71a197f5167d0cbfdc4a23c379516399aa7fd08"
    )
    assert row == before
    row["hybrid_file_evidence"]["response"]["explanation"].update(
        request_id=99, stage_timings=[9.75]
    )
    row["timings"]["query_latency_ms"] = 37.25
    assert completed_output_sha256(row) == completed_output_sha256(before)


@pytest.mark.parametrize("mutation", ["rank", "file_score", "raw_score", "file", "cap", "file_cap", "echo_cap", "bounded_count", "lexical_pool", "dense_pool", "pin"])
def test_completed_file_response_hash_binds_semantic_evidence(mutation):
    from tools.benchmark.retrieval.retrieval_contract import completed_output_sha256

    row = _completed_file_row()
    baseline = completed_output_sha256(row)
    evidence = row["hybrid_file_evidence"]
    response = evidence["response"]
    hit = response["results"][0]
    if mutation == "rank":
        hit["lexical"]["file_rank"] = 2
    elif mutation == "file_score":
        hit["file_rrf_score"] = 0.25
    elif mutation == "raw_score":
        hit["lexical"]["candidate"]["score"] = 0.0
    elif mutation == "file":
        hit["file"]["repo_relative_path"] = "src/b.rs"
    elif mutation == "cap":
        evidence["lane_top_k"] = 9
    elif mutation == "file_cap":
        evidence["file_top_k"] = 4
    elif mutation == "echo_cap":
        response["file_top_k"] = 4
    elif mutation == "bounded_count":
        response["bounded_file_count"] = 2
    elif mutation == "lexical_pool":
        response["lexical_pool_sha256"][0] = 3
    elif mutation == "dense_pool":
        response["dense_pool_sha256"][0] = 4
    else:
        response["generation"]["repo_id"] = "other"
    assert completed_output_sha256(row) != baseline


def test_completed_file_response_rejects_unrelated_floats_and_bad_raw_scores():
    from tools.benchmark.retrieval.retrieval_contract import completed_output_sha256

    for value in (True, "0.5", float("inf"), 10**400):
        row = _completed_file_row()
        row["hybrid_file_evidence"]["response"]["results"][0]["lexical"]["candidate"]["score"] = value
        with pytest.raises(ValueError):
            completed_output_sha256(row)
    row = _completed_file_row()
    row["hybrid_file_evidence"]["response"]["unrelated"] = 1.25
    with pytest.raises(ValueError, match="outside candidate score"):
        completed_output_sha256(row)


def _partial(window: dict, kind: str) -> None:
    count = window["returned"]
    window["candidate_count"] = {"kind": "at_least", "value": count}
    window["coverage"].pop("exhaustion_proof")
    window["coverage"]["examined"] = {"kind": "at_least", "value": count}
    window["outcome"] = (
        {"kind": "lower_bound", "continuation": False} if kind == "lower_bound" else
        {"kind": "capped_unknown", "cap": max(1, count)} if kind == "capped_unknown" else
        {"kind": "interrupted_partial", "reason": "deadline"} if kind == "interrupted_partial"
        else {"kind": "approximate", "method": "ann_search",
              "quality_contract": {"examined_lower_bound": count}}
    )


def test_exhausted_short_page_preserves_full_wire_and_source_bytes() -> None:
    row = _result()
    assert len(ALPHA) == 12 and len(BETA) == 11
    assert hashlib.sha256(ALPHA).hexdigest() == ALPHA_SHA256
    assert hashlib.sha256(BETA).hexdigest() == BETA_SHA256
    assert row["candidates"][0]["file_sha256"] == ALPHA_SHA256
    assert row["candidates"][1]["file_sha256"] == BETA_SHA256
    assert [hit["file_rrf_score"] for hit in _response(row)["results"]] == [
        ALPHA_RRF, BETA_RRF,
    ]
    validate_result(row, _capture(), 4)


def test_empty_exact_exhaustion_abstains_and_full_file_cap_is_capped() -> None:
    empty = _result()
    _response(empty)["results"] = []
    _response(empty)["bounded_file_count"] = 0
    _response(empty)["lexical_window"] = _window(0, "lexical")
    _response(empty)["dense_window"] = _window(0, "dense")
    _response(empty)["lexical_pool_sha256"] = _fixed_pool_digest([])
    _response(empty)["dense_pool_sha256"] = _fixed_pool_digest([])
    empty["candidates"] = []
    empty["status"] = "abstained"
    validate_result(empty, _capture(), 4)

    full = _result()
    full["hybrid_file_evidence"]["file_top_k"] = 2
    _response(full)["file_top_k"] = 2
    full["status"] = "capped"
    validate_result(full, _capture(), 4)


def test_unconstrained_file_lane_refuses_even_valid_logical_empty_window() -> None:
    empty = _result()
    response = _response(empty)
    response["results"] = []
    response["bounded_file_count"] = 0
    response["lexical_pool_sha256"] = _fixed_pool_digest([])
    response["dense_pool_sha256"] = _fixed_pool_digest([])
    for lane in ("lexical", "dense"):
        window = _window(0, lane)
        window["empty_provenance"] = "logical_empty"
        window["coverage"]["exhaustion_proof"] = {"kind": "logical_empty"}
        window["coverage"]["lanes"][0]["executed"] = False
        response[lane + "_window"] = window
    empty["candidates"] = []
    empty["status"] = "abstained"
    with pytest.raises(HybridFileEvidenceError, match="logical empty without constraints"):
        validate_result(empty, _capture(), 4)


def test_result_file_cap_must_match_independently_declared_cap() -> None:
    row = _result()
    row["hybrid_file_evidence"]["file_top_k"] = 2
    _response(row)["file_top_k"] = 2
    row["status"] = "capped"
    # The declared request cap remains three. A self-consistent result row
    # must not redefine that input and change the scoring status.
    with pytest.raises(HybridFileEvidenceError, match="file cap"):
        validate_result(row, _capture(), 4, declared_file_top_k=3)


def test_exact_bounded_file_union_cannot_omit_a_returned_winner() -> None:
    row = _result()
    row["hybrid_file_evidence"]["response"]["results"].pop()
    row["candidates"].pop()
    # The exact lane windows still describe the same two-file bounded union.
    with pytest.raises(HybridFileEvidenceError, match="bounded file"):
        validate_result(row, _capture(), 4)


def test_typed_partial_keeps_evidence_but_cannot_score() -> None:
    for kind in ("lower_bound", "capped_unknown", "interrupted_partial", "approximate"):
        partial = _result()
        _partial(_response(partial)["dense_window"], kind)
        partial["status"] = "error"
        partial["candidates"] = []
        partial["error"] = {"code": "incomplete_hybrid_file_window", "message": "partial"}
        validate_result(partial, _capture(), 4)
        forged = deepcopy(partial)
        forged["status"] = "success"
        forged["candidates"] = _result()["candidates"]
        forged["error"] = None
        with pytest.raises(HybridFileEvidenceError):
            validate_result(forged, _capture(), 4)


def test_full_file_cap_can_be_capped_with_lower_bound_lane() -> None:
    capped = _result()
    capped["hybrid_file_evidence"]["file_top_k"] = 2
    _response(capped)["file_top_k"] = 2
    capped["status"] = "capped"
    capped["hybrid_file_evidence"]["lane_top_k"] = 2
    dense = _response(capped)["dense_window"]
    _partial(dense, "lower_bound")
    dense["outcome"]["continuation"] = True
    dense["candidate_count"]["value"] = 3
    validate_result(capped, _capture(), 2)


def test_short_bounded_file_union_keeps_candidates_without_claiming_exhaustion() -> None:
    capped = _result()
    capped["hybrid_file_evidence"]["lane_top_k"] = 3
    dense = _response(capped)["dense_window"]
    dense["returned"] = 3
    dense["coverage"]["lanes"][0]["candidates"]["value"] = 3
    _partial(dense, "lower_bound")
    dense["outcome"]["continuation"] = True
    dense["candidate_count"]["value"] = 4
    capped["status"] = "capped"
    validate_result(capped, _capture(), 3)
    assert len(capped["candidates"]) == 2 < capped["hybrid_file_evidence"]["file_top_k"]

    forged = deepcopy(capped)
    forged["status"] = "success"
    with pytest.raises(HybridFileEvidenceError, match="status invents"):
        validate_result(forged, _capture(), 3)

    short_chunk_pool = deepcopy(capped)
    short_chunk_pool["hybrid_file_evidence"]["lane_top_k"] = 4
    short_chunk_pool["status"] = "error"
    short_chunk_pool["candidates"] = []
    short_chunk_pool["error"] = {"code": "incomplete_hybrid_file_window", "message": "partial"}
    validate_result(short_chunk_pool, _capture(), 4)


@pytest.mark.parametrize(
    "change",
    [
        lambda x: x["hybrid_file_evidence"].update(schema_version=2),
        lambda x: x["hybrid_file_evidence"].update(lane_top_k=5),
        lambda x: x["hybrid_file_evidence"].update(file_top_k=1),
        lambda x: _response(x)["generation"].update(revision_id="forged"),
        lambda x: _response(x)["results"][0]["file"].update(source_repo_id="foreign"),
        lambda x: _response(x)["results"][0]["file"].update(repo_relative_path="../escape.go"),
        lambda x: _response(x)["results"][0]["lexical"].update(file_rank=2),
        lambda x: _response(x)["results"][0]["dense"].update(chunk_rank=3),
        lambda x: _response(x)["results"][1]["dense"].update(file_rank=1),
        lambda x: _response(x)["results"][0].update(file_rrf_score=0.5),
        lambda x: _response(x)["results"].reverse(),
        lambda x: x["candidates"][0].update(score=0.75),
        lambda x: x["candidates"][0].update(path="src/beta.go"),
        lambda x: x["candidates"][0].update(start_line=2),
        lambda x: x["candidates"][0]["span_accounting"].update(unit_id="alpha-dense-chunk"),
        lambda x: _response(x)["results"][0]["lexical"]["candidate"].update(
            revision_id="forged"),
        lambda x: _response(x)["lexical_window"]["coverage"].update(lanes=[]),
        lambda x: _response(x)["dense_window"]["coverage"]["lanes"][0].update(
            lane="hybrid_file.lexical"),
        lambda x: _response(x)["dense_window"]["coverage"]["lanes"][0].update(
            contributed=False),
        lambda x: _response(x)["dense_window"]["coverage"]["lanes"][0]["candidates"].update(
            value=1),
        lambda x: x.update(status="capped"),
        lambda x: x.update(status="success", candidates=[]),
    ],
)
def test_forged_file_evidence_is_rejected(change) -> None:
    row = _result()
    change(row)
    with pytest.raises(HybridFileEvidenceError):
        validate_result(row, _capture(), 4)


def test_missing_evidence_only_for_empty_sdk_failure() -> None:
    failed = _result()
    failed.pop("hybrid_file_evidence")
    failed["candidates"] = []
    failed["error"] = {"code": "sdk_failure", "message": "failed"}
    for status in ("error", "timeout", "unavailable"):
        failed["status"] = status
        validate_result(failed, _capture(), 4)
    failed["status"] = "success"
    with pytest.raises(HybridFileEvidenceError):
        validate_result(failed, _capture(), 4)
    failed["status"] = "error"
    failed["candidates"] = _result()["candidates"]
    with pytest.raises(HybridFileEvidenceError):
        validate_result(failed, _capture(), 4)


def test_existing_file_projection_cannot_be_relabelled_as_hybrid_file() -> None:
    for original_policy in ("natural_language_file", "select:file"):
        row = _result()
        capture = _capture()
        capture["execution_profile"]["policy"] = original_policy
        with pytest.raises(HybridFileEvidenceError):
            validate_result(row, capture, 4)


def test_explicit_route_configuration_rejects_old_file_policy_and_implicit_cap() -> None:
    assert validate_route_configuration("natural_language", ["lexical"], 4, None) is None
    assert validate_route_configuration("natural_language", ["hybrid_file"], 4, 3) == 3
    for policy, routes, file_cap in (
        ("natural_language_file", ["hybrid_file"], 3),
        ("select:file", ["hybrid_file"], 3),
        ("natural_language", ["hybrid_file"], None),
        ("natural_language", ["lexical"], 3),
        ("natural_language", ["hybrid_file"], 0),
        ("natural_language", ["hybrid_file"], 5),
    ):
        with pytest.raises(HybridFileEvidenceError):
            validate_route_configuration(policy, routes, 4, file_cap)


def _measured_diagnostic(result: dict) -> dict:
    typed = deepcopy(_response(result))
    explanation = typed["explanation"]
    explanation["request_id"] = 1
    explanation["strategy"] = "hybrid_file_rrf"
    explanation["engines_executed"] = ["lexical", "semantic"]
    explanation["engines_touched"] = ["lexical", "semantic"]
    stages = (
        ("prepare", None),
        ("read_view", None),
        ("lexical_search", 1),
        ("embedding", None),
        ("dense_fetch", 2),
        ("dense_admission", 2),
        ("fusion", 2),
    )
    explanation["stage_timings"] = [
        {
            "stage": "hybrid." + stage,
            "elapsed_ns": index + 1,
            "calls": 1,
            "returned_candidates": returned,
        }
        for index, (stage, returned) in enumerate(stages)
    ]
    return {
        "response_kind": "returned_files",
        "candidates": [
            {
                "rank": row["rank"],
                "candidate_id": row["span_accounting"]["unit_id"],
                "path": row["path"],
                "start_line": row["start_line"],
                "end_line": row["end_line"],
                "score": row["score"],
                "contributions": [],
            }
            for row in result["candidates"]
        ],
        "response": {
            "lane_top_k": 4,
            "file_top_k": 3,
            "typed_response": typed,
        },
        "error_code": None,
    }


def test_measured_diagnostic_binds_full_sdk_response_to_record() -> None:
    result = _result()
    diagnostic = _measured_diagnostic(result)
    result["hybrid_file_evidence"]["response"] = deepcopy(
        diagnostic["response"]["typed_response"]
    )
    raw = deepcopy(diagnostic["response"]["typed_response"])
    assert raw["explanation"]["ranker_weights_hash"] == [0] * 32
    assert raw["explanation"]["contributions"] == []
    assert raw["explanation"]["summary"] == ""
    assert run._validate_diagnostic_response_v3(
        diagnostic, ("HF-1", "hybrid_file"), 9, "enabled", result, 4
    ) == {"hybrid_file.lexical": True, "hybrid_file.dense": True}
    assert diagnostic["response"]["typed_response"] == raw

    changed = deepcopy(diagnostic)
    changed["response"]["typed_response"]["explanation"]["summary"] = "substituted"
    with pytest.raises(run.RunError, match="file evidence differs"):
        run._validate_diagnostic_response_v3(
            changed, ("HF-1", "hybrid_file"), 9, "enabled", result, 4
        )
    with pytest.raises(run.RunError, match="explicit route/record contract"):
        run._validate_diagnostic_response_v3(
            diagnostic, ("HF-1", "hybrid"), 9, "enabled", result, 4
        )


@pytest.mark.parametrize("field,value", [
    ("candidate_id", "substituted-chunk"),
    ("score", 123.0),
    ("score", math.nextafter(ALPHA_RRF, math.inf)),
])
def test_measured_file_diagnostic_refuses_candidate_identity_or_score_drift(field, value) -> None:
    result = _result()
    diagnostic = _measured_diagnostic(result)
    result["hybrid_file_evidence"]["response"] = deepcopy(
        diagnostic["response"]["typed_response"]
    )
    diagnostic["candidates"][0][field] = value
    with pytest.raises(run.RunError, match="file candidate differs from record or SDK witness"):
        run._validate_diagnostic_response_v3(
            diagnostic, ("HF-1", "hybrid_file"), 9, "enabled", result, 4
        )


def test_measured_file_diagnostic_requires_a_typed_hit_for_each_scored_file() -> None:
    result = _result()
    diagnostic = _measured_diagnostic(result)
    typed = diagnostic["response"]["typed_response"]
    typed["results"].pop()
    typed["explanation"]["stage_timings"][-1]["returned_candidates"] = 1
    result["hybrid_file_evidence"]["response"] = deepcopy(typed)
    with pytest.raises(run.RunError, match="file candidates differ from record"):
        run._validate_diagnostic_response_v3(
            diagnostic, ("HF-1", "hybrid_file"), 9, "enabled", result, 4
        )


def test_hybrid_file_initial_fetch_uses_its_own_plan_prefix() -> None:
    policy = run.hybrid_fetch_policy_configuration("25")
    trace = [{"stage": "plan", "detail": "hybrid_file.internal_top_k=25"}]
    run._validate_hybrid_initial_fetch(trace, 4, policy, prefix="hybrid_file")
    with pytest.raises(run.RunError, match="initial fetch"):
        run._validate_hybrid_initial_fetch(
            [{"stage": "plan", "detail": "hybrid.internal_top_k=25"}],
            4, policy, prefix="hybrid_file",
        )


def test_evaluator_accepts_bound_natural_language_pin_and_file_rrf_ordering() -> None:
    profile = query_plan.execution_profile("natural_language")
    capture = {
        "system": "quanta",
        "chunk_strategy": "whole_file",
        "chunk_config": {},
        "runner_binary": {"name": "fixture-runner", "digest": "a" * 64},
        "searchd_binary": {"binary_digest": "b" * 64},
        "generation": 1,
        "source_repo_id": REPO,
        "source_revision_id": REVISION,
        "receipt_digest": "c" * 64,
        "activation_digest": "d" * 64,
        "model": "fixture-model",
        "model_revision": "fixture-model-revision",
        "execution_profile": profile,
        "execution_profile_sha256": evaluator.digest(evaluator.canonical(profile)),
    }
    assert evaluator.validate_capture(capture, "capture", version=5) == capture
    run_record = {
        "captures": {"q0": capture},
        "route_provenance": {"hybrid_file": {"capture_id": "q0"}},
    }
    results = {("HF-1", "hybrid_file"): _result()}
    assert evaluator._route_ordering(
        run_record, "hybrid_file", results, ["HF-1"]
    ) == "file_rrf_desc_lexical_presence_key_v1"
    forged = deepcopy(results)
    forged[("HF-1", "hybrid_file")]["ordering"] = "score_desc_path_tiebreak"
    with pytest.raises(evaluator.EvidenceError, match="ordering differs"):
        evaluator._route_ordering(run_record, "hybrid_file", forged, ["HF-1"])

    unbound = deepcopy(capture)
    unbound.pop("source_repo_id")
    unbound.pop("source_revision_id")
    assert evaluator.validate_capture(unbound, "capture", version=5) == unbound
    with pytest.raises(HybridFileEvidenceError, match="pin drift"):
        validate_result(_result(), unbound, 4)
