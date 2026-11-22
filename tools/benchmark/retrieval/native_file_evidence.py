"""Replay Native SDK file pools and authenticate their indexed file universe.

The caller supplies a Git-validated SourceSnapshot. Indexed inventory proof does
not by itself qualify runtime or change the product default.
"""

from __future__ import annotations

import hashlib
import json
import math
import struct
from collections.abc import Callable, Iterator
from pathlib import Path
from typing import Any

from tools.benchmark.retrieval import evaluator as ev
from tools.benchmark.retrieval import query_plan


def _bytes(value: Any, length: int, where: str) -> bytes:
    ev.require(
        isinstance(value, list)
        and len(value) == length
        and all(type(byte) is int and 0 <= byte <= 255 for byte in value),
        f"Native {where} is not a fixed byte identity",
    )
    return bytes(value)


def _owner(value: Any) -> tuple[bytes, bytes]:
    ev.require(
        isinstance(value, dict) and set(value) == {"producer", "stable_id"},
        "Native owner shape differs",
    )
    return _bytes(value["producer"], 16, "producer"), _bytes(value["stable_id"], 16, "owner")


def _score(value: Any, *, dense: bool = False) -> float:
    ev.require(type(value) in (int, float), "Native score is not numeric")
    try:
        number = float(value)
    except OverflowError as error:
        raise ev.EvidenceError("Native score is out of range") from error
    ev.require(
        math.isfinite(number) and (not dense or -1 <= number <= 1), "Native score is invalid"
    )
    ev.require(dense or number >= 0, "Native lexical score is negative")
    return number


def _body_ref(raw: bytes) -> bytes:
    """Independent QIOBJ001 childless source body identity, not a product helper."""
    ev.require(len(raw) <= 16 * 1024 * 1024, "Native source body exceeds object limit")
    header = b"QIOBJ001" + len(raw).to_bytes(4, "big") + (0).to_bytes(4, "big")
    return hashlib.sha256(b"quanta-index-immutable-object-v1\0" + header + raw).digest()


def verify_indexed_universe(
    source: ev.SourceSnapshot,
    universe: list[dict],
    view: dict,
    source_cut: dict,
    inventory: dict,
) -> dict[str, tuple[tuple[bytes, bytes], bytes]]:
    """Bind one producer's full exact-view path scan to frozen Git bytes."""
    ev.require(
        isinstance(inventory, dict)
        and set(inventory) == {"view", "source_cut", "file_instance", "entries", "owner_lookups"}
        and inventory["view"] == view
        and inventory["source_cut"] == source_cut,
        "Native indexed inventory view or SourceCut differs",
    )
    _bytes(inventory["file_instance"], 16, "file instance")
    expected = {row["path"]: row["file_sha256"] for row in universe}
    ev.require(len(expected) == len(universe), "Native frozen file universe has duplicate path")
    entries = inventory["entries"]
    ev.require(isinstance(entries, list) and len(entries) == len(expected), "Native indexed file count differs")
    by_path: dict[str, tuple[tuple[bytes, bytes], bytes]] = {}
    by_owner: dict[tuple[bytes, bytes], bytes] = {}
    prior_path: str | None = None
    producer: bytes | None = None
    for row in entries:
        ev.require(
            isinstance(row, dict)
            and set(row) == {"owner", "path", "source", "bytes", "byte_len"},
            "Native indexed file row shape differs",
        )
        path = row["path"]
        ev.require(
            isinstance(path, str)
            and path in expected
            and (prior_path is None or prior_path < path),
            "Native indexed file path is foreign, duplicate, or unordered",
        )
        prior_path = path
        owner = _owner(row["owner"])
        ev.require(
            producer is None or producer == owner[0],
            "Native frozen benchmark spans multiple source producers",
        )
        producer = owner[0]
        ev.require(owner not in by_owner, "Native indexed owner occurs at multiple paths")
        body, _lines, sha = source.file(path)
        ev.require(sha == expected[path], "Native indexed frozen source digest differs")
        ev.require(
            type(row["byte_len"]) is int
            and row["byte_len"] == len(body)
            and _bytes(row["bytes"], 32, "body ref") == _body_ref(body),
            "Native indexed body identity or length differs from frozen source",
        )
        record = _bytes(row["source"], 32, "source record")
        by_path[path] = (owner, record)
        by_owner[owner] = record
    ev.require(set(by_path) == set(expected), "Native indexed file set differs from frozen source")
    lookups = inventory["owner_lookups"]
    ev.require(isinstance(lookups, list) and bool(lookups), "Native source-owner lookups missing")
    seen: dict[tuple[bytes, bytes], bytes] = {}
    previous: tuple[bytes, bytes] | None = None
    for lookup in lookups:
        ev.require(
            isinstance(lookup, dict)
            and set(lookup) == {"view", "source", "rows"}
            and lookup["view"] == view
            and lookup["source"] == source_cut
            and isinstance(lookup["rows"], list)
            and 1 <= len(lookup["rows"]) <= 128,
            "Native source-owner lookup binding or size differs",
        )
        for row in lookup["rows"]:
            ev.require(
                isinstance(row, dict) and set(row) == {"owner", "source"},
                "Native source-owner row shape differs",
            )
            owner = _owner(row["owner"])
            ev.require(
                owner in by_owner and (previous is None or previous < owner),
                "Native source-owner lookup has foreign, duplicate, or unordered owner",
            )
            previous = owner
            record = _bytes(row["source"], 32, "looked-up source record")
            ev.require(record == by_owner[owner], "Native path and owner SourceCut differ")
            seen[owner] = record
    ev.require(seen == by_owner, "Native source-owner lookup set is incomplete")
    return by_path


_SOURCE_CUT_FIELDS = ("owners", "paths", "frontiers")


def _native_context_digest(label: bytes, payload: dict) -> bytes:
    wire = json.dumps(payload, separators=(",", ":")).encode()
    return hashlib.sha256(
        b"quanta-index-vnext\0" + len(label).to_bytes(8, "big") + label
        + len(wire).to_bytes(8, "big") + wire
    ).digest()


def verify_selected_view(manifest: dict, view: dict) -> None:
    manifest_fields = ("identity", "profile", "source", "projections", "members", "coverage")
    identity_fields = ("workspace", "epoch", "sequence", "digest")
    workspace_fields = ("tenant", "collection", "worktree", "slot")
    ev.object_keys(manifest, manifest_fields, "Native selected manifest")
    identity = ev.object_keys(manifest["identity"], identity_fields, "Native selected view identity")
    workspace = ev.object_keys(identity["workspace"], workspace_fields, "Native selected workspace")
    source = ev.object_keys(manifest["source"], _SOURCE_CUT_FIELDS, "Native selected SourceCut")
    ev.require(identity == view, "Native selected view differs")
    for name in workspace_fields:
        _bytes(workspace[name], 16, name)
    for name in ("epoch", "sequence"):
        ev.require(
            ev.nonnegative_int(identity[name], "Native view " + name) < 2**64,
            "Native view coordinate exceeds u64",
        )
    _bytes(manifest["profile"], 32, "profile")
    roots = source | {name: manifest[name] for name in ("projections", "members", "coverage")}
    for name, value in roots.items():
        if value is not None:
            _bytes(value, 32, name)
    expected = _bytes(identity["digest"], 32, "selected view digest")
    # CatalogView::identity_digest uses Rust DTO field order and a zeroed digest.
    payload = {name: manifest[name] for name in manifest_fields}
    payload["identity"] = {
        **{name: identity[name] for name in identity_fields},
        "workspace": {name: workspace[name] for name in workspace_fields},
        "digest": [0] * 32,
    }
    payload["source"] = {name: source[name] for name in _SOURCE_CUT_FIELDS}
    ev.require(
        _native_context_digest(b"catalog-view", payload) == expected,
        "Native selected view digest differs",
    )


def verify_selected_profile(capture: dict) -> dict:
    """Authenticate the retained profile and return its selected semantic spec."""
    selected = capture.get("selected_profile")
    ev.require(
        isinstance(selected, dict)
        and set(selected) == {"session", "view", "profile"}
        and selected["view"] == capture["selected_manifest"]
        and selected["session"] == capture["released_session"]["session"],
        "Native selected profile session or CatalogView differs",
    )
    _bytes(selected["session"], 32, "profile session")
    profile = selected["profile"]
    ev.require(
        isinstance(profile, dict)
        and set(profile) == {"instances"}
        and isinstance(profile["instances"], list)
        and 1 <= len(profile["instances"]) <= 4096,
        "Native selected profile instance inventory differs",
    )
    # IndexProfile is staged as a childless object. Match the Rust DTO field
    # order and enum shapes independently of the capture's JSON key order.
    fields = ("instance", "kind", "group", "activation", "input_domain", "schema", "policy", "format")
    instances = {}
    for raw in profile["instances"]:
        spec = ev.object_keys(raw, fields, "Native profile spec")
        for name in ("kind", "group", "schema", "format"):
            _bytes(spec[name], 16, "profile " + name)
        _bytes(spec["policy"], 32, "profile policy")
        ev.require(
            spec["activation"] in ("CoreRequired", "AsyncCoverage"),
            "Native profile activation differs",
        )
        domain = spec["input_domain"]
        if domain != "SourceLocal":
            projection_fields = ("projection", "producer")
            projection = ev.object_keys(domain, ("Projection",), "Native profile input domain")
            binding = ev.object_keys(projection["Projection"], projection_fields, "Native profile projection")
            for name in projection_fields:
                _bytes(binding[name], 16, "profile " + name)
            domain = {"Projection": {name: binding[name] for name in projection_fields}}
        instance = _bytes(spec["instance"], 16, "profile instance")
        ev.require(instance not in instances, "Native profile has duplicate instance")
        instances[instance] = {
            name: domain if name == "input_domain" else spec[name] for name in fields
        }
    payload = json.dumps({"instances": list(instances.values())}, separators=(",", ":")).encode()
    ev.require(
        _body_ref(payload) == _bytes(capture["selected_manifest"]["profile"], 32, "profile ref"),
        "Native selected profile reference differs",
    )
    lanes = {
        key: _bytes(capture[key], 16, key) for key in ("text_instance", "semantic_instance")
    }
    file_instance = _bytes(capture["indexed_universe"]["file_instance"], 16, "file instance")
    ev.require(
        len({file_instance, *lanes.values()}) == 3,
        "Native profile lane and file instances must be distinct",
    )
    for key, instance in (*lanes.items(), ("file instance", file_instance)):
        ev.require(instance in instances, f"Native {key} is absent from retained profile")
    return instances[lanes["semantic_instance"]]


def pool_hash(lane: str, rows: list[dict]) -> str:
    ev.require(lane in {"lexical", "dense"}, "Native pool lane differs")
    normalized = []
    for row in rows:
        producer, owner = _owner(row["owner"])
        item = {
            "producer": producer.hex(),
            "owner": owner.hex(),
            "source": _bytes(row["source"], 32, "source").hex(),
            "path": row["path"],
            "score_bits": struct.pack(">d", _score(row["score"], dense=lane == "dense")).hex(),
        }
        if lane == "dense":
            projection_producer, projection_owner = _owner(row["projection_owner"])
            vector = row["vector_id"]
            ev.require(
                isinstance(vector, list) and 0 < len(vector) <= 1024,
                "Native vector ID length differs",
            )
            item.update(
                projection_producer=projection_producer.hex(),
                projection_owner=projection_owner.hex(),
                vector_id=_bytes(vector, len(vector), "vector ID").hex(),
                chunk=row["chunk"],
            )
        normalized.append(item)
    return hashlib.sha256(
        b"qidx.native.file.pool.v1\n" + lane.encode() + b"\n" + ev.canonical(normalized)
    ).hexdigest()


def _verify_dense_chunk(hit: dict, source: bytes, policy: bytes) -> None:
    chunk = hit["chunk"]
    if chunk is None:
        return
    chunk = ev.object_keys(
        chunk, ("id", "payload", "input", "input_digest", "span"), "Native dense chunk"
    )
    binding = ev.object_keys(
        chunk["input"], ("source", "projection", "producer_context", "derivation_policy"),
        "Native dense chunk input",
    )
    ev.require(
        _bytes(chunk["id"], 32, "dense chunk ID")
        == _bytes(hit["vector_id"], 32, "dense chunk vector ID")
        and _bytes(binding["source"], 32, "dense chunk source") == source
        and _bytes(binding["derivation_policy"], 32, "dense chunk policy") == policy,
        "Native dense chunk vector, source or recipe differs",
    )
    _bytes(chunk["payload"], 32, "dense chunk payload")
    _bytes(chunk["input_digest"], 32, "dense chunk input digest")
    context = _bytes(binding["producer_context"], 32, "dense chunk producer context")
    projection = binding["projection"]
    span = chunk["span"]
    ev.require(isinstance(span, dict) and len(span) == 1, "Native dense chunk span differs")
    domain, bounds = next(iter(span.items()))
    if domain == "SourcePayload":
        ev.require(
            projection is None and context == bytes(32), "Native dense chunk source domain differs"
        )
    else:
        ev.require(
            domain in ("ProjectionPayload", "RenderedMember"), "Native dense chunk domain differs"
        )
        _bytes(projection, 32, "dense chunk projection")
    if domain == "RenderedMember":
        bounds = ev.object_keys(bounds, ("ordinal",), "Native dense chunk member")
        ordinal = bounds["ordinal"]
        ev.require(type(ordinal) is int and 0 <= ordinal < 4096, "Native dense chunk ordinal differs")
    else:
        bounds = ev.object_keys(bounds, ("start_byte", "end_byte"), "Native dense chunk byte span")
        start, end = bounds["start_byte"], bounds["end_byte"]
        ev.require(
            type(start) is int and type(end) is int and 0 <= start <= end <= 16_777_216
            and (start != end or start == 0),
            "Native dense chunk byte bounds differ",
        )


def replay_pools(
    row: dict,
    lane_top_k: int,
    file_top_k: int,
    universe: set[str],
    indexed: dict[str, tuple[tuple[bytes, bytes], bytes]] | None = None,
) -> dict[str, list[dict]]:
    """Authenticate captured bindings/order and independently recompute k60 RRF."""
    ev.require(type(lane_top_k) is int and 1 <= lane_top_k <= 1024, "Native lane bound differs")
    ev.require(
        type(file_top_k) is int and 1 <= file_top_k <= lane_top_k, "Native file bound differs"
    )
    lexical, hybrid = row["lexical"], row["hybrid"]
    coverage = hybrid.get("semantic_coverage")
    ev.require(
        isinstance(coverage, dict)
        and set(coverage) == {"instance", "desired", "covered", "searchable", "status", "owners"}
        and coverage["status"] == "Complete"
        and ev.canonical(coverage["covered"]) == ev.canonical(coverage["desired"]),
        "Native semantic source coverage differs",
    )
    ev.require(
        _bytes(coverage["instance"], 16, "semantic coverage instance")
        == _bytes(hybrid.get("semantic_instance"), 16, "semantic instance"),
        "Native semantic instance differs",
    )
    desired = coverage["desired"]
    ev.require(
        isinstance(desired, dict)
        and set(desired) == {"root", "schema", "policy", "context"},
        "Native semantic coverage authority differs",
    )
    if desired["root"] is not None:
        _bytes(desired["root"], 32, "semantic root")
    _bytes(desired["schema"], 16, "semantic schema")
    policy = _bytes(desired["policy"], 32, "semantic policy")
    _bytes(desired["context"], 32, "semantic context")
    if coverage["owners"] is not None:
        _bytes(coverage["owners"], 32, "semantic owners")
        ev.require(coverage["searchable"] is not None, "Native semantic index is absent")
    if coverage["searchable"] is not None:
        _bytes(coverage["searchable"], 32, "semantic searchable")
    traversal = hybrid.get("semantic_traversal")
    ev.require(
        traversal in ("ExactExhausted", "ExactCapped")
        or (
            isinstance(traversal, dict)
            and set(traversal) == {"Approximate"}
            and isinstance(traversal["Approximate"], dict)
            and set(traversal["Approximate"]) == {"capped"}
            and type(traversal["Approximate"]["capped"]) is bool
        ),
        "Native semantic traversal differs",
    )
    scanned, stale = hybrid.get("scanned_owners"), hybrid.get("skipped_stale")
    ev.require(
        type(scanned) is int
        and type(stale) is int
        and 0 <= stale <= scanned <= 100_000
        and len(hybrid["dense_pool"]) <= scanned - stale
        and (coverage["owners"] is not None or (scanned == 0 and not hybrid["dense_pool"]))
        and type(hybrid.get("semantic_exhaustive")) is bool
        and hybrid["semantic_exhaustive"] == (traversal == "ExactExhausted" and stale == 0),
        "Native semantic scan or exhaustive claim differs",
    )
    ev.require(lexical.get("coverage") == "CoreRequired", "Native lexical coverage differs")
    instance = _bytes(lexical.get("instance"), 16, "lexical instance")
    hybrid_instance = _bytes(hybrid.get("text_instance"), 16, "Hybrid lexical instance")
    artifact = _bytes(lexical.get("artifact"), 32, "lexical artifact")
    hybrid_artifact = _bytes(hybrid.get("lexical_artifact"), 32, "Hybrid lexical artifact")
    matched = lexical["matched"]
    ev.require(
        type(matched) is int
        and 0 <= matched < 2**64
        and type(hybrid["lexical_matched"]) is int
        and matched == hybrid["lexical_matched"],
        "Native lexical match total differs",
    )
    # JSON equality must preserve the wire types. Python container equality
    # otherwise treats a boolean byte or coordinate as the matching integer.
    hits = lexical["hits"]
    ev.require(
        ev.canonical(lexical["view"]) == ev.canonical(hybrid["view"])
        and instance == hybrid_instance
        and artifact == hybrid_artifact
        and isinstance(hits, list)
        and len(hits) == min(matched, lane_top_k)
        and ev.canonical(hits) == ev.canonical(hybrid["lexical_pool"])
        and lexical["exhaustive"] is True
        and type(hybrid["file_top_k"]) is int
        and hybrid["file_top_k"] == file_top_k,
        "Native standalone lexical pool binding differs",
    )
    identities = {}
    paths = {}
    ranks = {}
    pools = {"lexical": hybrid["lexical_pool"], "dense": hybrid["dense_pool"]}
    for lane, pool in pools.items():
        ev.require(isinstance(pool, list) and len(pool) <= lane_top_k, "Native pool exceeds bound")
        prior = None
        seen = set()
        for rank, hit in enumerate(pool, 1):
            owner = _owner(hit["owner"])
            ev.require(owner not in seen, "Native file pool has duplicate owner")
            seen.add(owner)
            ev.require(
                isinstance(hit["path"], str) and hit["path"] in universe,
                "Native file is outside frozen universe",
            )
            source = _bytes(hit["source"], 32, "source")
            if lane == "dense":
                _verify_dense_chunk(hit, source, policy)
            if indexed is not None:
                ev.require(
                    indexed[hit["path"]] == (owner, source),
                    "Native pool hit differs from authenticated indexed file",
                )
            identity = hit["path"], source
            ev.require(
                identities.setdefault(owner, identity) == identity,
                "Native cross-lane source identity differs",
            )
            ev.require(
                paths.setdefault(hit["path"], owner) == owner,
                "Native path has multiple source owners",
            )
            order = -_score(hit["score"], dense=lane == "dense"), owner
            ev.require(prior is None or prior < order, "Native pool score/owner order differs")
            prior = order
            ranks.setdefault(owner, {})[lane] = rank
    hashes = row["pool_hashes"]
    ev.require(
        isinstance(hashes, dict)
        and set(hashes) == {"scheme", "lexical", "dense"}
        and hashes["scheme"] == "qidx.native.file.pool.v1"
        and all(hashes[lane] == pool_hash(lane, pool) for lane, pool in pools.items()),
        "Native pool hash differs",
    )
    # The wire contract orders rounded binary64 sums. Rational ties can
    # differ by one representable value after rounding each lane's reciprocal.
    scores = {
        owner: (1.0 / (60.0 + lanes["lexical"]) if "lexical" in lanes else 0.0)
        + (1.0 / (60.0 + lanes["dense"]) if "dense" in lanes else 0.0)
        for owner, lanes in ranks.items()
    }
    expected = sorted(scores, key=lambda owner: (-scores[owner], owner))[:file_top_k]
    fused = hybrid["hits"]
    ev.require(
        isinstance(fused, list) and [_owner(hit["owner"]) for hit in fused] == expected,
        "Native independent RRF order differs",
    )
    # Exact owner-list equality above already checks both lengths.
    for owner, hit in zip(expected, fused):
        ev.require(
            all(
                hit[key] is None or (type(hit[key]) is int and hit[key] > 0)
                for key in ("lexical_rank", "dense_rank")
            ),
            "Native RRF lane rank is not a positive integer",
        )
        ev.require(
            hit["lexical_rank"] == ranks[owner].get("lexical")
            and hit["dense_rank"] == ranks[owner].get("dense")
            and _score(hit["score"]) == scores[owner],
            "Native independent RRF rank or score differs",
        )
    return {
        "lexical": [{"path": hit["path"]} for hit in pools["lexical"][:file_top_k]],
        "hybrid": [{"path": identities[owner][0]} for owner in expected],
        "union": [{"path": identities[owner][0]} for owner in sorted(identities)],
    }


REVIEW_FILE_LABEL_FIELDS = frozenset({
    "query_intent", "answerable", "answerability_min_grade", "gold",
    "file_judgments", "judgment_policy", "evaluation_contract", "label_review",
})


def _require_review_overlay(execution_suite: dict, reviewed_suite: dict) -> None:
    """Keep the captured source/query authority immutable across label review."""
    ev.require(
        isinstance(execution_suite, dict) and isinstance(reviewed_suite, dict),
        "Native reviewed suite differs from execution suite",
    )
    original = {key: value for key, value in execution_suite.items() if key != "tasks"}
    reviewed = {key: value for key, value in reviewed_suite.items() if key != "tasks"}
    if "diagnostic_policy" not in original and reviewed.get("diagnostic_policy") == ev.OBSERVED_PREFIX_DIAGNOSTIC_POLICY:
        reviewed.pop("diagnostic_policy")
    ev.require(
        ev.canonical(original) == ev.canonical(reviewed),
        "Native reviewed suite corpus differs from execution suite",
    )
    original_tasks = execution_suite.get("tasks")
    reviewed_tasks = reviewed_suite.get("tasks")
    ev.require(
        isinstance(original_tasks, list)
        and isinstance(reviewed_tasks, list)
        and len(original_tasks) == len(reviewed_tasks),
        "Native reviewed suite tasks differ from execution suite",
    )
    for before, after in zip(original_tasks, reviewed_tasks, strict=True):
        ev.require(
            isinstance(before, dict)
            and isinstance(after, dict)
            and "judgment_policy" not in before
            and "file_judgments" not in before
            and "label_review" not in before
            and ("answerable" not in before or before["answerable"] is False)
            and ("gold" not in before or before["gold"] == []),
            "Native execution suite is already judged or malformed",
        )
        immutable_before = {key: value for key, value in before.items() if key not in REVIEW_FILE_LABEL_FIELDS}
        immutable_after = {key: value for key, value in after.items() if key not in REVIEW_FILE_LABEL_FIELDS}
        ev.require(
            ev.canonical(immutable_before) == ev.canonical(immutable_after),
            "Native reviewed suite tasks differ from execution suite",
        )


def repository_cluster_ci_from_native_families(
    families: list[dict], repositories: dict[str, str], repository_strata: dict[str, str]
) -> dict:
    """Summarize already replayed composite Native families, without product admission.

    A caller must obtain each report from ``evaluate`` with a verified composite
    source. Digest bindings detect cross-family or changed capture inputs here;
    this aggregation cannot itself authenticate a running daemon or qrels review.
    """
    ev.require(isinstance(families, list) and bool(families), "Native family list is empty")
    ev.require(isinstance(repositories, dict) and bool(repositories), "Native target roster is empty")
    rows: list[tuple[str, str, str, float]] = []
    cohort: list[dict] = []
    seen_families: set[str] = set()
    target_releases: dict[str, str] = {}
    base_release: str | None = None
    for item in families:
        overlay = isinstance(item, dict) and "execution_suite" in item
        ev.require(
            isinstance(item, dict)
            and set(item) == ({"suite", "execution_suite", "capture", "report"} if overlay else {"suite", "capture", "report"}),
            "Native family replay shape differs",
        )
        suite, capture, report = item["suite"], item["capture"], item["report"]
        execution_suite = item["execution_suite"] if overlay else suite
        ev.require(
            isinstance(suite, dict) and isinstance(capture, dict) and isinstance(report, dict),
            "Native family replay inputs are malformed",
        )
        if overlay:
            _require_review_overlay(execution_suite, suite)
        binding = report.get("composite_source_binding")
        ev.require(isinstance(binding, dict), "Native composite source binding is absent")
        family = binding.get("family_id")
        target = binding.get("target_repository")
        target_commit = binding.get("target_commit")
        target_release = binding.get("target_release_digest")
        base = binding.get("base_release_digest")
        ev.require(
            isinstance(family, str) and bool(family) and family not in seen_families,
            "Native composite family is empty or repeated",
        )
        seen_families.add(family)
        ev.require(
            isinstance(target, str)
            and target in repositories
            and target_commit == repositories[target]
            and isinstance(target_release, str)
            and target_release.startswith("sha256:")
            and bool(ev.HEX64_RE.fullmatch(target_release[7:]))
            and isinstance(base, str)
            and base.startswith("sha256:")
            and bool(ev.HEX64_RE.fullmatch(base[7:]))
            and target_release != base
            and (base_release is None or base_release == base)
            and target_releases.setdefault(target, target_release) == target_release,
            "Native composite target release or repository differs",
        )
        base_release = base
        source_commit = binding.get("source_commit")
        source_universe = binding.get("source_universe_sha256")
        source_binding = binding.get("source_binding_sha256")
        universe_digest = binding.get("file_universe_digest")
        provenance = suite.get("source_provenance")
        rank_metrics = report.get("rank_metrics")
        comparison = rank_metrics.get("comparison") if isinstance(rank_metrics, dict) else None
        suite_sha = ev.digest(ev.canonical(suite))
        execution_suite_sha = ev.digest(ev.canonical(execution_suite))
        capture_sha = ev.digest(ev.canonical(capture))
        ev.require(
            binding.get("qualification") == "SOURCE_ONLY_NOT_FORMAL_EVAL"
            and binding.get("source_release_count") == 12
            and type(binding.get("source_file_count")) is int
            and binding["source_file_count"] > 0
            and isinstance(source_universe, str)
            and bool(ev.HEX64_RE.fullmatch(source_universe))
            and isinstance(source_binding, str)
            and bool(ev.HEX64_RE.fullmatch(source_binding))
            and isinstance(source_commit, str)
            and bool(ev.COMMIT_RE.fullmatch(source_commit))
            and isinstance(universe_digest, str)
            and universe_digest.startswith("sha256:")
            and bool(ev.HEX64_RE.fullmatch(universe_digest[7:]))
            and isinstance(provenance, dict)
            and provenance
            == {
                "kind": "composite_git_releases_v1",
                "source_universe_sha256": source_universe,
                "source_binding_sha256": source_binding,
            }
            and suite.get("repository_commit") == source_commit
            and suite.get("file_universe_digest") == universe_digest.removeprefix("sha256:")
            and capture.get("schema_version") == 2
            and capture.get("profile") == "native_file"
            and capture.get("engine") == "native"
            and capture.get("suite_id") == suite.get("suite_id")
            and capture.get("repository_commit") == source_commit
            and capture.get("file_universe_digest") == suite["file_universe_digest"]
            and capture.get("source_universe_sha256") == source_universe
            and capture.get("source_binding_sha256") == source_binding
            and capture.get("suite_commitment_sha256") == execution_suite_sha
            and report.get("suite_commitment_sha256") == suite_sha
            and (not overlay or report.get("execution_suite_commitment_sha256") == execution_suite_sha)
            and report.get("capture_commitment_sha256") == capture_sha
            and report.get("status") == "native_file_evidence_unqualified"
            and report.get("product_default_decision") is False
            and report.get("runtime_qualified") is False
            and report.get("indexed_universe_verified") is True
            and isinstance(comparison, dict)
            and comparison.get("primary_metric") == "file_mrr_at_10",
            "Native family source, capture, replay, or qualification differs",
        )
        tasks = suite.get("tasks")
        captured = capture.get("tasks")
        per_query = report.get("per_query")
        ev.require(
            isinstance(tasks, list)
            and isinstance(captured, list)
            and isinstance(per_query, list),
            "Native family task inventory is malformed",
        )
        eval_tasks = [task for task in tasks if isinstance(task, dict) and task.get("split") == "eval"]
        task_ids = {task.get("task_id") for task in eval_tasks}
        ev.require(
            bool(eval_tasks)
            and len(task_ids) == len(eval_tasks)
            and all(task.get("query_family_id") == family for task in eval_tasks)
            and len(captured) == len(eval_tasks)
            and {row.get("task_id") for row in captured if isinstance(row, dict)} == task_ids
            and len(per_query) == 2 * len(eval_tasks)
            and {
                (row.get("task_id"), row.get("route"))
                for row in per_query if isinstance(row, dict)
            }
            == {(task_id, route) for task_id in task_ids for route in ("lexical", "hybrid")},
            "Native family task rows are missing, foreign, or repeated",
        )
        paired = ev.paired_query_family_rows(
            suite, report, "lexical", "hybrid", require_graded=True
        )
        rows.extend((target, task_id, family_id, delta) for task_id, family_id, _category, delta in paired)
        cohort_row = {
                "family_id": family,
                "target_repository": target,
                "target_commit": target_commit,
                "target_release_digest": target_release,
                "source_commit": source_commit,
                "file_universe_digest": universe_digest,
                "source_universe_sha256": source_universe,
                "source_binding_sha256": source_binding,
                "suite_commitment_sha256": suite_sha,
                "capture_commitment_sha256": capture_sha,
            }
        if overlay:
            cohort_row["execution_suite_commitment_sha256"] = execution_suite_sha
        cohort.append(cohort_row)
    ev.require(set(target_releases) == set(repositories), "Native target repository coverage is incomplete")
    cohort.sort(key=lambda item: (item["target_repository"], item["family_id"]))
    cohort_sha = ev.digest(ev.canonical({"base_release_digest": base_release, "families": cohort}))
    # The cohort hash seeds the existing statistics; it is not a signed release.
    ci = ev.repository_cluster_ci(rows, cohort_sha, repositories, repository_strata)
    return {
        "status": "native_repository_cluster_diagnostic_unqualified",
        "product_default_decision": False,
        "runtime_qualified": False,
        "cohort_sha256": cohort_sha,
        "base_release_digest": base_release,
        "families": cohort,
        "repository_cluster_ci": ci,
    }


def _replay_candidate_pools(
    suite: dict,
    pack: dict,
    capture: dict,
    source: ev.SourceSnapshot | None = None,
    composite_source_inputs: tuple[Path, Path, Path, Path, str] | None = None,
    *,
    release_validator: Callable[[Path], dict] | None = None,
    expected_source_universe: tuple[str, int] | None = None,
    expected_source_binding: tuple[str, int] | None = None,
) -> tuple[bool, dict | None, Iterator[tuple[dict, dict, dict]]]:
    """Authenticate the source, exact read view and each paired candidate pool."""
    ev.require(
        type(capture.get("schema_version")) is int
        and capture["schema_version"] in (1, 2)
        and capture.get("profile") == "native_file"
        and capture.get("engine") == "native",
        "Native file capture profile differs",
    )
    for key in ("suite_id", "repository_commit", "file_universe_digest"):
        ev.require(capture.get(key) == pack[key] == suite[key], f"Native capture {key} differs")
    suite_digest = ev.digest(ev.canonical(suite))
    ev.require(
        capture.get("suite_commitment_sha256")
        == pack.get("suite_commitment_sha256")
        == suite_digest,
        "Native capture/pack suite digest differs",
    )
    ev.require(
        capture.get("query_pack_sha256") == ev.digest(ev.canonical(pack)),
        "Native capture blind pack digest differs",
    )
    contract = ev.declared_evaluation_contract(suite["tasks"])
    ev.require(
        contract is None
        or contract["request_mode"] == query_plan.NATURAL_LANGUAGE_FILE_SEARCH,
        "Native file capture request mode differs from frozen suite",
    )
    provenance = suite.get("source_provenance")
    ev.require(
        isinstance(provenance, dict)
        and provenance.get("kind") in ("single_git_commit_v1", "composite_git_releases_v1"),
        "Native frozen source provenance is absent",
    )
    source_binding = None
    if provenance["kind"] == "composite_git_releases_v1":
        ev.require(
            capture.get("source_universe_sha256") == provenance["source_universe_sha256"]
            and capture.get("source_binding_sha256") == provenance["source_binding_sha256"],
            "Native composite capture source identity differs",
        )
        ev.require(
            source is not None and composite_source_inputs is not None,
            "Native composite source has not been verified",
        )
        from tools.benchmark.retrieval import composite_source_binding

        source_binding = composite_source_binding.verify(
            source, suite, pack, capture, *composite_source_inputs,
            release_validator=release_validator,
            expected_source_universe=expected_source_universe,
            expected_source_binding=expected_source_binding,
        )
    else:
        ev.require(
            composite_source_inputs is None
            and "source_universe_sha256" not in capture
            and "source_binding_sha256" not in capture,
            "Native single source carries composite verification",
        )
    ev.require(
        type(capture["lane_top_k"]) is int
        and 10 <= capture["lane_top_k"] <= 1024
        and capture["lane_top_k"] == pack["comparison_contract"]["top_k"],
        "Native candidate depth differs from frozen pack",
    )
    ev.require(
        type(capture["file_top_k"]) is int and 10 <= capture["file_top_k"] <= capture["lane_top_k"],
        "Native output depth cannot support top10 evaluation",
    )
    view = capture["view"]
    verify_selected_view(capture["selected_manifest"], view)
    indexed = None
    semantic_spec = None
    source_context = None
    source_cut = capture["selected_manifest"]["source"]
    if capture["schema_version"] == 2:
        ev.require(source is not None, "Native indexed inventory needs frozen source snapshot")
        semantic_spec = verify_selected_profile(capture)
        if semantic_spec["input_domain"] == "SourceLocal":
            source_context = _native_context_digest(
                b"source-context", {name: source_cut[name] for name in _SOURCE_CUT_FIELDS}
            )
        indexed = verify_indexed_universe(
            source,
            pack["file_universe"],
            view,
            source_cut,
            capture["indexed_universe"],
        )
    session = capture["released_session"]
    session_id = _bytes(session.get("session"), 32, "released session")
    view_identity = ev.canonical(view)
    ev.require(
        session["state"] == "Released" and ev.canonical(session["view"]) == view_identity,
        "Native capture pin was not released",
    )
    settlement = capture["settlement"]
    head = settlement.get("head")
    floor = settlement.get("exact_floor")
    state = settlement.get("state")
    pins = settlement.get("has_remaining_target_pins")
    ev.require(
        _bytes(settlement.get("session"), 32, "settlement session") == session_id
        and ev.canonical(settlement["view"]) == view_identity
        and isinstance(head, dict)
        and head.get("workspace") == view["workspace"]
        and type(head.get("epoch")) is int
        and head["epoch"] == view["epoch"]
        and type(head.get("sequence")) is int
        and view["sequence"] <= head["sequence"] < 2**64
        and type(floor) is int
        and 0 <= floor <= head["sequence"]
        and state in ("Pending", "Incomplete", "SettledAtSnapshot")
        and type(pins) is bool
        and (state != "SettledAtSnapshot" or (not pins and floor > view["sequence"])),
        "Native settlement binding differs",
    )
    ev.require(
        type(capture["query_deadline_seconds"]) is int
        and 1 <= capture["query_deadline_seconds"] <= 60,
        "Native deadline differs",
    )
    deadline_micros = capture["query_deadline_seconds"] * 1_000_000
    eval_tasks = [task for task in suite["tasks"] if task["split"] == "eval"]
    if source_binding is not None:
        ev.require(
            {task["query_family_id"] for task in eval_tasks} == {source_binding["family_id"]},
            "Native composite eval tasks differ from source family",
        )
    tasks = {task["task_id"]: task for task in eval_tasks}
    ev.require(len(tasks) == len(eval_tasks), "Native suite task identities are duplicate")
    captured = capture["tasks"]
    ev.require(
        isinstance(captured, list) and len(captured) == len(tasks),
        "Native task denominator differs",
    )
    ev.require({row["task_id"] for row in captured} == set(tasks), "Native task inventory differs")
    blind = {task["task_id"]: task for task in pack["tasks"]}
    ev.require(len(blind) == len(pack["tasks"]), "Native blind task identities are duplicate")
    ev.require(set(blind) == set(tasks), "Native blind task inventory differs")
    ev.require(captured, "Native file evidence has no tasks")
    selected_hybrid = captured[0]["hybrid"]
    universe = {row["path"] for row in pack["file_universe"]}

    def validated_rows() -> Iterator[tuple[dict, dict, dict]]:
        for row in captured:
            if capture["schema_version"] == 2:
                for key in ("lexical_micros", "hybrid_micros"):
                    elapsed = row.get(key)
                    ev.require(
                        type(elapsed) is int and 0 <= elapsed <= deadline_micros,
                        f"Native task SDK timing exceeds its deadline or is invalid: {key}",
                    )
            task = tasks[row["task_id"]]
            raw_query = blind[row["task_id"]]["query"]
            planned = query_plan.plan_lexical_request("natural_language_file", raw_query)
            identity = row["query_identity"]
            text, semantic = row["text_request"], row["semantic_request"]
            ev.require(
                identity["original_query_sha256"]
                == blind[row["task_id"]]["query_sha256"]
                == ev.digest(raw_query.encode())
                and identity["effective_lexical_request_sha256"] == ev.digest(planned.encode())
                and identity["semantic_text_sha256"] == ev.digest(raw_query.encode())
                and identity["policy_config_sha256"]
                == ev.digest(query_plan.policy_config_canonical("natural_language_file").encode()),
                "Native independently derived query identity differs",
            )
            ev.require(
                text["syntax"] == "native"
                and text["query_text"] == planned
                and semantic["query_text"] == raw_query
                and text["top_k"] == semantic["top_k"] == capture["lane_top_k"]
                and text["constraints"] == semantic["constraints"] == {"language_any_of": []}
                and text["generation"] is None
                and text["generation_selector"] is None
                and semantic["generation"] is None
                and semantic["generation_selector"] is None
                and text["cursor"] is None
                and semantic["lexical_scope"] is None,
                "Native actual query, eligibility or lane depth differs",
            )
            ev.require(
                ev.canonical(row["hybrid"]["view"]) == view_identity
                and _bytes(row["hybrid"].get("session"), 32, "task view/session") == session_id,
                "Native task view/session differs",
            )
            ev.require(
                row["hybrid"]["text_instance"] == capture["text_instance"]
                and row["hybrid"]["semantic_instance"] == capture["semantic_instance"],
                "Native task instance differs",
            )
            ev.require(
                row["hybrid"]["lexical_artifact"] == selected_hybrid["lexical_artifact"]
                and row["hybrid"].get("semantic_coverage") == selected_hybrid.get("semantic_coverage"),
                "Native same-view query resources differ",
            )
            pools = replay_pools(row, capture["lane_top_k"], capture["file_top_k"], universe, indexed)
            if semantic_spec is not None:
                desired = row["hybrid"]["semantic_coverage"]["desired"]
                ev.require(
                    desired["schema"] == semantic_spec["schema"]
                    and desired["policy"] == semantic_spec["policy"],
                    "Native semantic coverage differs from retained profile",
                )
                if source_context is not None:
                    ev.require(
                        desired["root"] == source_cut["owners"]
                        and bytes(desired["context"]) == source_context,
                        "Native semantic coverage differs from selected SourceCut",
                    )
            yield task, row, pools

    return indexed is not None, source_binding, validated_rows()


def verify_execution(
    suite: dict,
    pack: dict,
    capture: dict,
    source: ev.SourceSnapshot,
    composite_source_inputs: tuple[Path, Path, Path, Path, str] | None = None,
    *,
    release_validator: Callable[[Path], dict] | None = None,
    expected_source_universe: tuple[str, int] | None = None,
    expected_source_binding: tuple[str, int] | None = None,
) -> None:
    """Replay the exact captured SDK execution without relevance judgments."""
    indexed, _, rows = _replay_candidate_pools(
        suite, pack, capture, source, composite_source_inputs,
        release_validator=release_validator,
        expected_source_universe=expected_source_universe,
        expected_source_binding=expected_source_binding,
    )
    ev.require(indexed, "Native controlled execution requires authenticated indexed inventory")
    for _ in rows:
        pass


def prepare_review_candidates(
    suite: dict,
    pack: dict,
    capture: dict,
    source: ev.SourceSnapshot,
    composite_source_inputs: tuple[Path, Path, Path, Path, str] | None = None,
) -> dict[str, dict[str, list[dict]]]:
    """Replay unjudged Native candidates and discard scores, ranks and owner IDs."""
    ev.require(
        capture.get("schema_version") == 2 and source is not None,
        "Native review preparation requires authenticated indexed inventory",
    )
    _, _, rows = _replay_candidate_pools(
        suite, pack, capture, source, composite_source_inputs
    )
    universe = {file["path"]: file["file_sha256"] for file in pack["file_universe"]}
    prepared: dict[str, dict[str, list[dict]]] = {"lexical": {}, "hybrid": {}}
    for task, _row, pools in rows:
        ev.require(
            task["answerable"] is False and task["gold"] == []
            and not set(task).intersection({
                "judgment_policy", "file_judgments", "declaration_judgments",
                "label_review", "source_oracle",
            }),
            "Native review preparation requires unjudged eval tasks",
        )
        for route in prepared:
            prepared[route][task["task_id"]] = [
                {"path": path, "file_sha256": universe[path]}
                for path in sorted(hit["path"] for hit in pools[route])
            ]
    return prepared


def evaluate(
    suite: dict,
    pack: dict,
    capture: dict,
    source: ev.SourceSnapshot | None = None,
    composite_source_inputs: tuple[Path, Path, Path, Path, str] | None = None,
    *,
    release_validator: Callable[[Path], dict] | None = None,
    reviewed_suite: dict | None = None,
    expected_source_universe: tuple[str, int] | None = None,
    expected_source_binding: tuple[str, int] | None = None,
) -> dict:
    """Score every frozen eval task; schema 2 also authenticates SDK inventory."""
    if reviewed_suite is not None:
        _require_review_overlay(suite, reviewed_suite)
    judged_suite = reviewed_suite if reviewed_suite is not None else suite
    judged_tasks = (
        {task["task_id"]: task for task in reviewed_suite["tasks"] if task["split"] == "eval"}
        if reviewed_suite is not None else None
    )
    indexed_verified, source_binding, replayed = _replay_candidate_pools(
        suite, pack, capture, source, composite_source_inputs,
        release_validator=release_validator,
        expected_source_universe=expected_source_universe,
        expected_source_binding=expected_source_binding,
    )
    per_query = []
    failures = []
    negative = {route: [] for route in ("lexical", "hybrid")}
    negative_rows = []
    positive = {
        route: {metric: [] for metric in ("file_mrr_at_10", "file_recall_at_10")}
        for route in negative
    }
    for execution_task, row, pools in replayed:
        task = judged_tasks[execution_task["task_id"]] if judged_tasks is not None else execution_task
        ev.require(
            task.get("judgment_policy")
            in (ev.COMPLETE_JUDGMENT_POLICY, ev.SOURCE_ORACLE_JUDGMENT_POLICY),
            "Native file qrels are incomplete",
        )
        judgments = task["file_judgments"]
        if not task["answerable"] and not pools["hybrid"]:
            ev.require(
                row["hybrid"]["semantic_exhaustive"],
                "Native empty negative Hybrid result lacks exhaustive semantic observation",
            )
        for route in ("lexical", "hybrid"):
            hits = pools[route]
            ev.require(
                ev.ranked_file_judgment_exclusion(
                    task["judgment_policy"], judgments, [hit["path"] for hit in hits]
                )
                is None,
                "Native top10 contains unjudged files",
            )
            metrics = {
                "file_mrr_at_10": ev.file_mrr_at_k_judged(hits, judgments, 10),
                "file_recall_at_10": ev.file_recall_at_k_judged(hits, judgments, 10),
            }
            row_metrics = {"task_id": task["task_id"], "route": route, **metrics}
            if task["answerable"]:
                for metric, score in metrics.items():
                    positive[route][metric].append(score)
            else:
                nonempty = bool(hits)
                negative[route].append(float(nonempty))
                row_metrics["status"] = "success" if nonempty else "abstained"
            per_query.append(row_metrics)
        if not task["answerable"]:
            negative_rows.append(
                (task["task_id"], task, negative["lexical"][-1] - negative["hybrid"][-1])
            )
        gold = {label["path"] for label in judgments if label["grade"] > 0}
        failures.append(
            {
                "task_id": task["task_id"],
                "positive_outside_both_pools": sorted(
                    gold - {hit["path"] for hit in pools["union"]}
                ),
                "positive_in_pools_below_hybrid_top10": sorted(
                    (gold & {hit["path"] for hit in pools["union"]})
                    - {hit["path"] for hit in pools["hybrid"][:10]}
                ),
            }
        )
    means = {
        route: {
            metric: (math.fsum(values) / len(values) if values else None)
            for metric, values in metrics.items()
        }
        for route, metrics in positive.items()
    }
    answerable = len(positive["lexical"]["file_mrr_at_10"])
    ev.require(answerable > 0, "Native file evidence has no positive tasks")
    abstention = ev.no_answer_delta_summary(
        negative_rows,
        judged_suite["repository_commit"],
    )
    report = {
        "status": "native_file_evidence_unqualified",
        "product_default_decision": False,
        "graded": True,
        "indexed_universe_verified": indexed_verified,
        "indexed_universe_verification_scope": (
            "frozen_body_refs_exact_view_source_owner_lookup_profile"
            if indexed_verified
            else "none"
        ),
        "runtime_qualified": False,
        "capture_commitment_sha256": ev.digest(ev.canonical(capture)),
        "suite_id": judged_suite["suite_id"],
        "repository_commit": judged_suite["repository_commit"],
        "suite_commitment_sha256": ev.digest(ev.canonical(judged_suite)),
        "rank_metrics": {
            "comparison": {
                "baseline": "lexical",
                "candidate": "hybrid",
                "primary_metric": "file_mrr_at_10",
                "sample_count": answerable,
                "primary_delta": means["hybrid"]["file_mrr_at_10"]
                - means["lexical"]["file_mrr_at_10"],
                "no_answer_abstention_delta": abstention,
            }
        },
        "metrics": means,
        "negative_count": len(negative["lexical"]),
        "false_positive_at_10": {
            route: (math.fsum(values) / len(values) if values else None)
            for route, values in negative.items()
        },
        "per_query": per_query,
        "candidate_failure_analysis": failures,
    }
    if source_binding is not None:
        report["composite_source_binding"] = source_binding
    if reviewed_suite is not None:
        report["execution_suite_commitment_sha256"] = ev.digest(ev.canonical(suite))
    return report
