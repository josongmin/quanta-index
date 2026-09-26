"""Complete-profile commit records over immutable BenchmarkEvidenceV1 runs.

A run is one measured case. A capture is the exact inventory of runs produced
by one profile execution. Publishing the profile pointer is the commit point;
partial family/case output never replaces the previous complete capture.
"""

from __future__ import annotations

import os
from pathlib import Path

from custody import publication
from evidence import (
    EvidenceError,
    RunStore,
    _read_regular_file,
    _run_id,
    _sync_dir,
    _write_atomic,
    canonical_json,
    digest_bytes,
    parse_json,
)


def _directories(root: Path) -> None:
    # Reject intermediate links as well as linked final documents.
    for path in (root, root / "captures", root / "profiles"):
        if path.is_symlink():
            raise EvidenceError(f"capture directory is a symlink: {path}")
        if path.exists() and not path.is_dir():
            raise EvidenceError(f"capture path is not a directory: {path}")
    for path in root.absolute().parents:
        if path.is_symlink():
            raise EvidenceError(f"capture ancestor is a symlink: {path}")


@publication
def commit_capture(
    root: Path,
    *,
    capture_id: str,
    profile: str,
    registry_digest: str,
    expected_cases: dict[str, list[str | None]],
    run_ids: list[str],
) -> dict:
    _run_id(capture_id)
    _run_id(profile)
    _directories(root)
    store = RunStore(root)
    if not isinstance(run_ids, list) or not run_ids:
        raise EvidenceError("capture needs unique, nonempty run IDs")
    for run_id in run_ids:
        _run_id(run_id)
    if len(set(run_ids)) != len(run_ids):
        raise EvidenceError("capture needs unique, nonempty run IDs")
    records, actual, source = [], {}, None
    for run_id in run_ids:
        evidence = store.load(run_id)
        if evidence["profile"] != profile:
            raise EvidenceError("capture run belongs to a different profile")
        if evidence["verdict"]["status"] != "pass":
            raise EvidenceError("capture contains a non-passing run")
        if source is None:
            source = evidence["source"]
        elif source != evidence["source"]:
            raise EvidenceError("capture mixes source identities")
        actual.setdefault(evidence["family"], []).append(evidence["case_id"])
        records.append(
            {
                "run_id": run_id,
                "family": evidence["family"],
                "case_id": evidence["case_id"],
                "digest": evidence["digest"],
            }
        )
    _check_cases(actual, expected_cases)
    body = {
        "schema_version": 1,
        "capture_id": capture_id,
        "profile": profile,
        "registry_digest": registry_digest,
        "source": source,
        "expected_cases": expected_cases,
        "runs": records,
    }
    document = {**body, "digest": digest_bytes(canonical_json(body).encode())}
    target = root / "captures" / f"{capture_id}.json"
    target.parent.mkdir(parents=True, exist_ok=True)
    if target.exists() or target.is_symlink():
        raise EvidenceError("capture ID already exists")
    # Exclusive creation: two writers must not overwrite an immutable capture.
    with target.open("xb") as handle:
        handle.write(canonical_json(document).encode())
        handle.flush()
        os.fsync(handle.fileno())
    _sync_dir(target.parent)
    # Publish only after the complete record can be loaded independently.
    load_capture(root, capture_id=capture_id, profile=profile, registry_digest=registry_digest)
    _write_atomic(
        root / "profiles" / f"{profile}.json",
        canonical_json(
            {
                "capture_id": capture_id,
                "digest": document["digest"],
            }
        ).encode(),
    )
    return document


def _check_cases(actual: dict[str, list], expected: dict[str, list]) -> None:
    if not isinstance(expected, dict) or not expected or set(actual) != set(expected):
        raise EvidenceError("capture family inventory is incomplete or extra")
    for family, cases in expected.items():
        _run_id(family)
        if not isinstance(cases, list) or not cases:
            raise EvidenceError(f"expected cases for {family!r} must be nonempty and unique")
        if any(case is not None and (not isinstance(case, str) or not case) for case in cases):
            raise EvidenceError("capture has an invalid case ID")
        if len(set(cases)) != len(cases):
            raise EvidenceError("capture repeats an expected case ID")
        observed = actual[family]
        if any(case is not None and (not isinstance(case, str) or not case) for case in observed):
            raise EvidenceError("capture has an invalid observed case ID")
        if len(set(observed)) != len(observed) or set(observed) != set(cases):
            raise EvidenceError(f"capture case inventory differs for {family!r}")


def load_capture(
    root: Path, *, profile: str, registry_digest: str, capture_id: str | None = None
) -> dict:
    _run_id(profile)
    _directories(root)
    pointer = None
    if capture_id is None:
        pointer = parse_json(_read_regular_file(root / "profiles" / f"{profile}.json").decode())
        if not isinstance(pointer, dict) or set(pointer) != {"capture_id", "digest"}:
            raise EvidenceError("profile pointer is malformed")
        capture_id = pointer["capture_id"]
    _run_id(capture_id)
    document = parse_json(_read_regular_file(root / "captures" / f"{capture_id}.json").decode())
    keys = {
        "schema_version",
        "capture_id",
        "profile",
        "registry_digest",
        "source",
        "expected_cases",
        "runs",
        "digest",
    }
    if not isinstance(document, dict) or set(document) != keys:
        raise EvidenceError("capture record is malformed")
    body = {key: value for key, value in document.items() if key != "digest"}
    if document["digest"] != digest_bytes(canonical_json(body).encode()):
        raise EvidenceError("capture record digest mismatch")
    if pointer is not None and pointer["digest"] != document["digest"]:
        raise EvidenceError("profile pointer digest mismatch")
    if (
        type(document["schema_version"]) is not int
        or document["schema_version"] != 1
        or document["capture_id"] != capture_id
        or document["profile"] != profile
        or document["registry_digest"] != registry_digest
    ):
        raise EvidenceError("capture has wrong profile, ID, schema or registry identity")
    records = document["runs"]
    if not isinstance(records, list) or not records:
        raise EvidenceError("capture run inventory is empty")
    store, actual, seen = RunStore(root), {}, set()
    for record in records:
        if not isinstance(record, dict) or set(record) != {"run_id", "family", "case_id", "digest"}:
            raise EvidenceError("capture run reference is malformed")
        _run_id(record["run_id"])
        _run_id(record["family"])
        if record["run_id"] in seen:
            raise EvidenceError("capture repeats a run reference")
        seen.add(record["run_id"])
        evidence = store.load(record["run_id"])
        if (
            evidence["digest"] != record["digest"]
            or evidence["family"] != record["family"]
            or evidence["case_id"] != record["case_id"]
            or evidence["profile"] != profile
            or evidence["source"] != document["source"]
            or evidence["verdict"]["status"] != "pass"
        ):
            raise EvidenceError("capture run reference has wrong digest, identity or verdict")
        actual.setdefault(record["family"], []).append(record["case_id"])
    if not isinstance(document["expected_cases"], dict):
        raise EvidenceError("capture expected-case inventory is malformed")
    _check_cases(actual, document["expected_cases"])
    return document
