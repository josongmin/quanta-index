"""Construct a bound canonical diagnostic profile without legacy timer claims."""

from __future__ import annotations

from copy import deepcopy

from tools.benchmark.retrieval.causal_cost_timeline import phase_owners, summarize

U64_MAX = (1 << 64) - 1
BINDING_WIDTHS = {
    "source_revision": 40,
    "binary_sha256": 64,
    "summary_sha256": 64,
    "tier_manifest_sha256": 64,
    "timeline_sha256": 64,
    "timeline_capture_id": 32,
}
FAMILIES = (
    "sync_aggregates",
    "object_kind_aggregates",
    "adoption_group_aggregates",
    "bm25_census_aggregates",
    "bm25_boundary_aggregates",
    "bm25_live_build_aggregates",
)
BM25_STAGES = {
    "bm25_census_aggregates": "bm25_census_build",
    "bm25_boundary_aggregates": "bm25_boundary",
    "bm25_live_build_aggregates": "bm25_live_build",
}


def _add(total: dict, field: str, value: int) -> None:
    if type(value) is not int or not 0 <= value <= U64_MAX:
        raise ValueError(f"canonical {field} is not a u64")
    result = total.get(field, 0) + value
    if result > U64_MAX:
        raise ValueError(f"canonical {field} aggregate exceeds u64")
    total[field] = result


def canonical_profile(capture: dict, product: dict, *, execution_binding: dict) -> dict:
    """Consume parse()/validate_product_artifacts() results from the same execution.

    Input custody stays with capture(). This verifies typed metric scope/ancestry,
    not independent legacy equivalence or quiet-host performance qualification.
    """
    if product.get("backend") == "index-native":
        return canonical_native_profile(capture, product, execution_binding=execution_binding)
    binding = execution_binding
    for field, width in BINDING_WIDTHS.items():
        value = binding.get(field)
        if (
            not isinstance(value, str)
            or len(value) != width
            or any(c not in "0123456789abcdef" for c in value)
        ):
            raise ValueError(f"canonical profile lacks {field} binding")
    if (
        binding.get("product_status") != "PRODUCT_ARTIFACT_VERIFIED"
        or binding["timeline_sha256"] != capture["sha256"]
        or binding["timeline_capture_id"] != capture["capture_id"]
        or product["provenance"]["git_head"] != binding["source_revision"]
    ):
        raise ValueError("canonical profile input bindings disagree")
    if (
        capture["status"] != "complete"
        or capture["health"]
        or capture["orphan_events"]
        or capture["truncated_tail"]
        or any(
            capture["footer"][field]
            for field in ("events_dropped", "events_unwritten", "open_spans", "producer_leases")
        )
    ):
        raise ValueError("canonical profile requires complete lossless capture")
    resources = product["phase_resources"]
    owners = phase_owners(capture)
    phases = {}
    for span in capture["spans"].values():
        if span["stage"] != "harness_phase":
            continue
        name = span.get("phase")
        if name not in resources or name in phases or span["outcome"] != "ok":
            raise ValueError("canonical phase identity/status is missing, repeated or mismatched")
        phases[name] = {
            "status": "ok",
            "duration_ns": span["end_ns"] - span["start_ns"],
            "resources": deepcopy(resources[name]),
            "owner_spans": [],
            **{family: [] for family in FAMILIES},
        }
    if set(phases) != set(resources):
        raise ValueError("canonical capture does not cover all product phases")
    seen = set()
    for family in FAMILIES:
        for row in capture[family]:
            name = owners[row["span_id"]]
            if name not in phases:
                raise ValueError(f"canonical {family} has no verified phase owner")
            owner = capture["spans"][row["span_id"]]
            if owner["outcome"] != "ok" or (
                family in BM25_STAGES and owner["stage"] != BM25_STAGES[family]
            ):
                raise ValueError("canonical aggregate owner stage/outcome differs")
            dimension = row.get(
                "label",
                row.get(
                    "kind", (row.get("reason"), row.get("refs_bucket"), row.get("bytes_bucket"))
                ),
            )
            key = (family, row["span_id"], dimension)
            if key in seen:
                raise ValueError("canonical aggregate repeats an owner/dimension")
            seen.add(key)
            phases[name][family].append(deepcopy(row))
    # Only ingest phases require these metrics. Zero is explicit in an emitted
    # event; missing families cannot be interpreted as a zero-cost seal.
    seals = {
        "full_ingest_seal",
        "full_seal",
        "delta_ingest_seal",
        "noop_seal",
        "delete_seal",
    } & phases.keys()
    if not seals:
        raise ValueError("canonical profile has no measured ingest seal")
    for name in seals:
        phase = phases[name]
        for family in ("sync_aggregates", "bm25_census_aggregates", "bm25_live_build_aggregates"):
            if not phase[family]:
                raise ValueError(f"canonical {name} lacks {family}")
        if not any(row["label"] == "encode" for row in phase["bm25_boundary_aggregates"]):
            raise ValueError(f"canonical {name} lacks BM25 encode metrics")
    for row in summarize(capture)["spans"]:
        name = owners[row["span_id"]]
        if name in phases:
            # Child timers are already included in parent duration. Keep each
            # observed envelope and exclusive duration, never sum wall shares.
            phases[name]["owner_spans"].append(row)
    for phase in phases.values():
        phase["sync"] = {}
        for row in phase["sync_aggregates"]:
            total = phase["sync"].setdefault(row["label"], {})
            for field in ("calls", "failures", "elapsed_sum_ns"):
                _add(total, field, row[field])
            total["elapsed_max_ns"] = max(total.get("elapsed_max_ns", 0), row["elapsed_max_ns"])
    return {
        "schema_version": 2,
        "status": "diagnostic_unqualified",
        "source": "canonical_timeline",
        **{field: binding[field] for field in BINDING_WIDTHS},
        "build_binding": binding.get("build_binding", "UNPROVEN"),
        "tier": product["tier"]["tier"],
        "seed": product["tier"]["seed"],
        "corpus_digest": product["provenance"]["corpus_digest"],
        "config_digest": product["provenance"]["config_digest"],
        "runtime_config": deepcopy(product["runtime_config"]),
        "phases": phases,
        "scope": {
            "source": "typed canonical metrics and independently validated product resources",
            "owner_phase": "parent ancestry plus explicit request/client flow; computation owners contained in attempt",
            "wall": "phase/owner envelopes; nested and concurrent durations are not additive",
            "resources": "summary observation envelopes; unavailable I/O remains unavailable",
            "legacy_equivalence": "NOT_RUN; different timer envelopes are not compared",
            "performance_qualification": "NOT_RUN; requires matched quiet-host Off/Record",
        },
    }


def canonical_native_profile(capture: dict, product: dict, *, execution_binding: dict) -> dict:
    """Bind Native harness phases and independent daemon captures without clock alignment."""
    binding = execution_binding
    for field, width in (
        *BINDING_WIDTHS.items(),
        ("native_daemon_sha256", 64),
        ("native_daemon_sha256_after", 64),
    ):
        value = binding.get(field)
        if (
            not isinstance(value, str)
            or len(value) != width
            or any(character not in "0123456789abcdef" for character in value)
        ):
            raise ValueError(f"canonical Native profile lacks {field} binding")
    if (
        binding.get("product_status") != "PRODUCT_ARTIFACT_VERIFIED"
        or binding["timeline_sha256"] != capture.get("sha256")
        or binding["timeline_capture_id"] != capture.get("capture_id")
        or binding["source_revision"] != product["provenance"]["git_head"]
        or binding["native_daemon_sha256"] != binding["native_daemon_sha256_after"]
        or binding["native_daemon_sha256"] != product["daemon_sha256"]
    ):
        raise ValueError("canonical Native product/source/daemon bindings disagree")
    footer = capture.get("footer")
    if (
        capture.get("status") != "complete"
        or capture.get("health")
        or capture.get("orphan_events") != 0
        or capture.get("truncated_tail") is not False
        or not isinstance(footer, dict)
        or any(
            footer.get(field) != 0
            for field in ("events_dropped", "events_unwritten", "open_spans", "producer_leases")
        )
    ):
        raise ValueError("canonical Native parent capture is incomplete or unhealthy")
    children = product.get("children")
    if not isinstance(children, dict) or set(children) != {"first", "reopen"}:
        raise ValueError("canonical Native independent child captures absent")
    parent_id = capture.get("capture_id")
    child_ids = [children[role]["capture_id"] for role in ("first", "reopen")]
    if len({parent_id, *child_ids}) != 3:
        raise ValueError("canonical Native parent/child capture identities were reused")
    actual_phases = product.get("phases")
    if not isinstance(actual_phases, dict) or not actual_phases:
        raise ValueError("canonical Native product phases absent")
    spans = capture.get("spans")
    if not isinstance(spans, dict):
        raise ValueError("canonical Native parent spans absent")
    observed = {}
    for span in spans.values():
        if span.get("stage") != "harness_phase":
            continue
        name = span.get("phase")
        if not isinstance(name, str) or not name.startswith("native_"):
            raise ValueError("canonical Native harness phase has no native identity")
        phase = name.removeprefix("native_")
        if phase not in actual_phases or phase in observed or span.get("outcome") != "ok":
            raise ValueError("canonical Native harness phase missing, repeated or failed")
        if span.get("end_ns") is None or span["end_ns"] < span["start_ns"]:
            raise ValueError("canonical Native harness phase interval invalid")
        observed[phase] = {
            "status": "ok",
            "duration_ns": span["end_ns"] - span["start_ns"],
            "product_observation": deepcopy(actual_phases[phase]),
            "owner_spans": [],
        }
    if set(observed) != set(actual_phases):
        raise ValueError("canonical Native parent phase coverage differs from product")
    owners = phase_owners(capture)
    for row in summarize(capture)["spans"]:
        name = owners[row["span_id"]]
        if name is None:
            continue
        if not name.startswith("native_") or name.removeprefix("native_") not in observed:
            raise ValueError("canonical Native owner belongs to an unknown phase")
        observed[name.removeprefix("native_")]["owner_spans"].append(row)
    return {
        "schema_version": 2,
        "status": "diagnostic_unqualified",
        "source": "native_parent_and_independent_children",
        **{field: binding[field] for field in BINDING_WIDTHS},
        "native_daemon_sha256": binding["native_daemon_sha256"],
        "build_binding": binding.get("build_binding", "UNPROVEN"),
        "tier": product["tier"]["tier"],
        "seed": product["tier"]["seed"],
        "corpus_digest": product["corpus_digest"],
        "config_digest": product["config_digest"],
        "foreground_workload": deepcopy(product["foreground_workload"]),
        "native_resource_observation": deepcopy(product.get("native_resource_observation")),
        "phases": observed,
        "children": deepcopy(children),
        "scope": {
            "source_build": "observed local pair only when build_binding proves both harness and daemon",
            "corpus": "recorded digest; no independent regeneration of source bytes",
            "phase_wall": "parent harness work envelopes; observer_ms is separate; no direct timer equality",
            "child": "two separately captured daemon clocks and cumulative install IO; no parent/child alignment or per-phase IO split",
            "resources": "validated sampled observation and explicit unsupported axes; no continuous peak or operating qualification",
            "legacy_equivalence": "NOT_APPLICABLE_NATIVE",
            "performance_qualification": "NOT_RUN",
        },
    }
