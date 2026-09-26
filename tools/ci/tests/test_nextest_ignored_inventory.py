"""Inventory-bound ignored events and fragmented nextest 0.9.104 suites.

Goldens retain the exact installed reporter's premature suite finalization.
Only independently collected required test identities may establish success.
"""

from __future__ import annotations

import copy
import hashlib
import importlib.util
import json
from pathlib import Path
from types import SimpleNamespace

import pytest

from tools.ci.nextest_events import (
    NextestEvidenceError,
    NextestInventory,
    parse_nextest,
    parse_nextest_bytes,
    parse_nextest_inventory_bytes,
)

SUITE = {"crate": "demo", "test_binary": "checks", "kind": "test"}
PREFIX = "demo::checks$"


def test_file_consumer_accepts_large_log_with_bounded_lines(tmp_path, monkeypatch):
    rows = events(False)
    path = tmp_path / "events.jsonl"
    digest = hashlib.sha256()
    with path.open("wb") as stream:
        for row in rows:
            raw = json.dumps({**row, "stdout": "x" * (3 * 1024 * 1024)}).encode() + b"\n"
            stream.write(raw)
            digest.update(raw)
    expected = parse_nextest_inventory_bytes(json.dumps(inventory()).encode())
    monkeypatch.setattr(Path, "read_bytes", lambda *_: pytest.fail("whole-file read"))
    parsed = parse_nextest(path, expected)
    assert (parsed.selected, parsed.executed, parsed.passed, parsed.failed) == (2, 2, 2, 0)
    assert parsed.passed_names == {PREFIX + "a", PREFIX + "b"}
    assert parsed.sha256 == digest.hexdigest()


def test_file_consumer_refuses_oversized_line_before_json_decode(tmp_path, monkeypatch):
    from tools.ci import nextest_events as owner

    path = tmp_path / "events.jsonl"
    path.write_bytes(b" " * (owner.CONTROL_DOCUMENT_BYTES + 1))
    monkeypatch.setattr(owner.json, "loads", lambda *_a, **_k: pytest.fail("decoded oversized line"))
    with pytest.raises(NextestEvidenceError, match="line exceeds"):
        parse_nextest(path)


def test_file_consumer_refuses_mutation_during_parse(tmp_path, monkeypatch):
    from tools.ci import nextest_events as owner

    path = tmp_path / "events.jsonl"
    path.write_bytes(b"\n".join(json.dumps(row).encode() for row in events(False)) + b"\n")
    expected = parse_nextest_inventory_bytes(json.dumps(inventory()).encode())
    decode = owner.json.loads
    changed = False

    def mutate(raw, **kwargs):
        nonlocal changed
        result = decode(raw, **kwargs)
        if not changed:
            changed = True
            with path.open("ab") as stream:
                stream.write(b" ")
        return result

    monkeypatch.setattr(owner.json, "loads", mutate)
    with pytest.raises(NextestEvidenceError):
        parse_nextest(path, expected)
    assert changed


def test_event_identity_retention_has_an_explicit_limit(monkeypatch):
    from tools.ci import nextest_events as owner

    monkeypatch.setattr(owner, "CONTROL_DOCUMENT_BYTES", 256)
    rows = [{"type": "suite", "event": "started", "test_count": 10}]
    for index in range(10):
        name = f"case-{index}-" + "x" * 40
        rows.extend({"type": "test", "event": state, "name": name} for state in ("started", "ok"))
    rows.append({"type": "suite", "event": "ok", "passed": 10, "failed": 0, "ignored": 0})
    raw = b"\n".join(json.dumps(row).encode() for row in rows)
    with pytest.raises(NextestEvidenceError, match="identities exceed"):
        parse_nextest_bytes(raw)


def inventory():
    return {
        "test-count": 3,
        "rust-suites": {
            "demo::checks": {
                "package-name": "demo", "binary-name": "checks", "kind": "test",
                "status": "listed",
                "testcases": {
                    "a": {"ignored": False, "filter-match": {"status": "matches"}},
                    "b": {"ignored": False, "filter-match": {"status": "matches"}},
                    "external": {"ignored": True, "filter-match": {
                        "status": "mismatch", "reason": "ignored"}},
                },
            },
        },
    }


def start():
    return {"type": "suite", "event": "started", "test_count": 3, "nextest": SUITE}


def finish(passed, filtered=0):
    return {"type": "suite", "event": "ok", "passed": passed, "failed": 0,
            "ignored": 1, "filtered_out": filtered, "nextest": SUITE}


def event_row(name, event):
    return {"type": "test", "event": event, "name": PREFIX + name}


def events(fragmented):
    rows = [start(), event_row("external", "started"), event_row("a", "started"),
            event_row("b", "started"), event_row("external", "ignored"),
            event_row("a", "ok")]
    if fragmented:
        rows.extend([finish(1), start(), event_row("b", "ok"), finish(1, 1)])
    else:
        rows.extend([event_row("b", "ok"), finish(2)])
    return rows


def replay(rows, collected=None):
    raw = b"\n".join(json.dumps(row).encode() for row in rows) + b"\n"
    expected = parse_nextest_inventory_bytes(
        json.dumps(inventory() if collected is None else collected).encode()
    )
    return parse_nextest_bytes(raw, expected)


@pytest.mark.parametrize("fragmented", [False, True])
def test_explicit_ignored_inventory_does_not_become_selected_success(fragmented):
    parsed = replay(events(fragmented))
    assert (parsed.selected, parsed.executed, parsed.passed, parsed.failed) == (2, 2, 2, 0)
    assert parsed.passed_names == {PREFIX + "a", PREFIX + "b"}


@pytest.mark.parametrize("mutation", [
    "unknown_ignored", "ignored_success", "required_ignored", "missing_required",
    "missing_ignored", "duplicate_required", "duplicate_ignored", "wrong_binary",
    "wrong_pass_count", "wrong_ignored_count", "wrong_announced_count",
    "wrong_fragment_filter_count", "missing_fragment_filter_count", "failed_required",
    "timed_out_required", "missing_start", "missing_ignored_start", "missing_suite_end", "empty_fragment",
])
def test_inventory_bound_ignore_never_hides_incomplete_or_forged_execution(mutation):
    rows = copy.deepcopy(events(True))
    if mutation == "unknown_ignored":
        rows[1]["name"] = rows[4]["name"] = PREFIX + "not-collected"
    elif mutation == "ignored_success":
        rows[4]["event"] = "ok"
    elif mutation == "required_ignored":
        rows[8]["event"] = "ignored"
    elif mutation == "missing_required":
        del rows[8]
    elif mutation == "missing_ignored":
        del rows[4]
        del rows[1]
    elif mutation == "duplicate_required":
        rows.insert(9, event_row("b", "ok"))
    elif mutation == "duplicate_ignored":
        rows.insert(5, event_row("external", "ignored"))
    elif mutation == "wrong_binary":
        rows[8]["name"] = "demo::other$b"
    elif mutation == "wrong_pass_count":
        rows[6]["passed"] = 2
    elif mutation == "wrong_ignored_count":
        rows[6]["ignored"] = 0
    elif mutation == "wrong_announced_count":
        rows[0]["test_count"] = 2
    elif mutation == "wrong_fragment_filter_count":
        rows[9]["filtered_out"] = 0
    elif mutation == "missing_fragment_filter_count":
        del rows[9]["filtered_out"]
    elif mutation == "failed_required":
        rows[8]["event"] = "failed"
    elif mutation == "timed_out_required":
        rows[8]["event"] = "timeout"
    elif mutation == "missing_start":
        del rows[3]
    elif mutation == "missing_ignored_start":
        del rows[1]
    elif mutation == "missing_suite_end":
        del rows[9]
    elif mutation == "empty_fragment":
        rows.extend([start(), finish(0, 2)])
    with pytest.raises(NextestEvidenceError):
        replay(rows)


def test_nonignored_exclusion_cannot_authorize_an_ignored_outcome():
    collected = inventory()
    case = collected["rust-suites"]["demo::checks"]["testcases"]["external"]
    case.update(ignored=False, **{"filter-match": {"status": "mismatch", "reason": "ignored"}})
    with pytest.raises(NextestEvidenceError):
        replay(events(False), collected)


def test_other_filter_exclusion_cannot_authorize_an_ignored_outcome():
    collected = inventory()
    case = collected["rust-suites"]["demo::checks"]["testcases"]["external"]
    case["filter-match"]["reason"] = "expression"
    with pytest.raises(NextestEvidenceError):
        replay(events(False), collected)


def test_unbound_fragment_counts_are_still_refused():
    raw = b"\n".join(json.dumps(row).encode() for row in events(True)) + b"\n"
    with pytest.raises(NextestEvidenceError):
        parse_nextest_bytes(raw)


def test_inventory_metadata_is_immutable_and_not_a_caller_allowlist():
    collected = parse_nextest_inventory_bytes(json.dumps(inventory()).encode())
    with pytest.raises(TypeError):
        collected.ignored[PREFIX + "forged"] = ("demo", "checks", "test")
    with pytest.raises(TypeError):
        collected.suite_counts[("demo", "checks", "test")] = (0, 0, 0)
    with pytest.raises(NextestEvidenceError, match="must be parsed"):
        NextestInventory({}, {PREFIX + "forged": ("demo", "checks", "test")}, {}, _token=object())


def test_inventory_without_exclusions_does_not_authorize_partial_suites():
    collected = inventory()
    cases = collected["rust-suites"]["demo::checks"]["testcases"]
    del cases["external"]
    collected["test-count"] = 2
    rows = [start(), event_row("a", "started"), event_row("a", "ok"), finish(1, 1)]
    rows[0]["test_count"] = 2
    rows[3]["ignored"] = 0
    with pytest.raises(NextestEvidenceError):
        replay(rows, collected)


@pytest.mark.parametrize("fragmented", [False, True])
def test_archived_proof_consumer_counts_required_tests_only(tmp_path, fragmented):
    from tools.ci.proof_execution_result import derive_test_result

    inputs = {"events.jsonl": b"\n".join(json.dumps(row).encode() for row in events(fragmented)) + b"\n",
              "inventory.json": json.dumps(inventory()).encode()}
    artifacts = []
    for name, raw in inputs.items():
        (tmp_path / name).write_bytes(raw)
        artifacts.append({"source_path": name, "path": name,
                          "sha256": hashlib.sha256(raw).hexdigest()})
    counts, names = derive_test_result(tmp_path, {"schema_version": 1, "runs": [{
        "format": "nextest-jsonl", "events": "events.jsonl", "inventory": "inventory.json"}]}, artifacts)
    assert counts == {"selected": 2, "executed": 2, "passed": 2, "failed": 0, "ignored": 0}
    assert names == {("nextest-jsonl", PREFIX + "a"), ("nextest-jsonl", PREFIX + "b")}


@pytest.mark.parametrize("fragmented", [False, True])
def test_local_scope_frontdoor_admits_inventory_bound_ignored_events(tmp_path, monkeypatch, fragmented):
    script = Path(__file__).resolve().parents[1] / "run-local-test-scope.py"
    spec = importlib.util.spec_from_file_location("ignored_scope_frontdoor", script)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    monkeypatch.setattr(module, "ROOT", tmp_path)
    calls = []

    def execute(argv, *, stdout, **kwargs):
        calls.append(argv)
        phase = argv[argv.index("nextest") + 1]
        stdout.write(json.dumps(inventory()).encode() if phase == "list" else
                     b"\n".join(json.dumps(row).encode() for row in events(fragmented)) + b"\n")
        return SimpleNamespace(returncode=0)

    monkeypatch.setattr(module.subprocess, "run", execute)
    assert module.run_with_proof_evidence(
        ["/selected/cargow", "nextest", "run", "--test-threads", "4"],
        scopes=["daemon"], lane="test-daemon-lane", raw_dir=tmp_path / "raw",
    ) == 0
    assert [argv[argv.index("nextest") + 1] for argv in calls] == ["list", "run"]
