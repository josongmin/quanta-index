"""Complete-profile commit records over immutable BenchmarkEvidenceV1 runs.

A run is one measured case. A capture is the exact inventory of runs produced
by one profile execution. Publishing the profile pointer is the commit point;
partial family/case output never replaces the previous complete capture.
"""

from __future__ import annotations

import functools
import inspect
import os
import time
import uuid
from collections.abc import Callable
from contextvars import ContextVar
from copy import deepcopy
from pathlib import Path

from custody import publication
from evidence import (
    EvidenceError,
    RawFile,
    RunStore,
    _check_control_size,
    _read_control_file,
    _run_id,
    _sync_dir,
    _write_atomic,
    canonical_json,
    digest_bytes,
    parse_json,
    write_raw_file,
)
from host_monitor import INPUT_ID as HOST_INPUT_ID
from host_monitor import RAW_NAME as HOST_RAW_NAME
from host_monitor import HostMonitor

_active_capture = ContextVar("benchmark_capture_epoch", default=None)


def current_capture():
    epoch = _active_capture.get()
    if epoch is None:
        raise EvidenceError("capture operation has no owning epoch")
    return epoch


def capture_error(error):
    """Keep a primary error even when the CLI converts it to a nonzero status."""
    if (epoch := _active_capture.get()) is not None:
        epoch.reject(error)


def capture_phase(phase, **observations):
    if (epoch := _active_capture.get()) is not None:
        epoch.step(phase, **observations)


class CaptureEpoch:
    """Diagnostic custody spanning preparation through the sole pointer commit.

    Failure records are not BenchmarkEvidence and never enter capture inventory.
    Unobserved identities/terminal states stay absent, not fabricated successes.
    """

    def __init__(self, repo: Path, root: Path, profile: str, *, capture_id=None, monitor_host=False):
        self.repo = repo.resolve()
        self.root = root.absolute()
        if self.root.resolve().is_relative_to(self.repo):
            raise EvidenceError("capture evidence root must stay outside the checkout")
        _directories(self.root)
        self.profile = _run_id(profile)
        self.capture_id = _run_id(capture_id or f"{profile}-{uuid.uuid4().hex}")
        if type(monitor_host) is not bool:
            raise EvidenceError("capture host-monitor policy must be explicit")
        self.monitor_host = monitor_host
        self.host_monitor = None
        self.host_raw = None
        self.work = self.root / "work" / self.capture_id
        self.failure = self.root / "failures" / f"{self.capture_id}.json"
        self.primary = None
        self.state = {
            "schema_version": 1, "kind": "capture-diagnostic",
            "capture_id": self.capture_id, "profile": self.profile,
            "status": "active", "phase": "admission", "sequence": 0,
            "started_ns": time.time_ns(), "updated_ns": time.time_ns(),
            "work_root": str(self.work), "observations": {},
            "history": [],
            "commit_state": "not_started", "error": None,
        }

    def __enter__(self):
        self.work.mkdir(parents=True, exist_ok=False)
        self.token = _active_capture.set(self)
        try:
            self.step("admission")
        except BaseException as error:
            self.__exit__(type(error), error, error.__traceback__)
            raise
        return self

    def step(self, phase, **observations):
        self.require_healthy()
        _run_id(phase)
        self.state.update(phase=phase, sequence=self.state["sequence"] + 1, updated_ns=time.time_ns())
        self.state["history"].append({
            "sequence": self.state["sequence"], "phase": phase, "observed_ns": self.state["updated_ns"],
        })
        self.state["observations"].update(deepcopy(observations))
        _write_atomic(self.work / "capture.json", canonical_json(self.state).encode())

    def reject(self, error):
        if self.primary is None:
            self.primary = error

    def require_healthy(self):
        if self.primary is not None:
            raise self.primary

    def inputs(self, files):
        self.step("preparation", inputs={
            name: {"path": str(raw.path), "sha256": raw.sha256, "bytes": raw.size}
            for name, raw in files.items()
        })

    def execute(self, owner, *args, **kwargs):
        self.require_healthy()
        log_dir = kwargs["log_dir"].absolute()
        if not log_dir.is_relative_to(self.work):
            raise EvidenceError("producer logs escape their capture epoch")
        execution = {"state": "attempted", "log_dir": str(log_dir)}
        self.state["observations"].setdefault("executions", []).append(execution)
        self.step("execution", execution=execution)
        try:
            if self.monitor_host:
                if self.host_raw is not None or "custody_fds" in kwargs:
                    raise EvidenceError("monitored producer has an invalid reservation state")
                if self.host_monitor is None:
                    self.host_monitor = HostMonitor(
                        self.work / HOST_RAW_NAME, self.capture_id, self.profile,
                    ).start()
                self.host_monitor.phase(log_dir.name)
                kwargs["custody_fds"] = (self.host_monitor.fd,)
            result = owner(*args, **kwargs)
        except BaseException as error:
            self.reject(error)
            # The execution owner retains its own terminal/output record. Pin
            # it if available, but never turn absent terminal evidence into 0.
            try:
                raw = RawFile.capture(log_dir / "execution.json")
                execution["record"] = {
                    "path": str(raw.path), "sha256": raw.sha256, "bytes": raw.size,
                }
            except (OSError, ValueError) as record_error:
                execution["record_unavailable"] = str(record_error)[:4096]
            self.state["observations"]["execution"] = deepcopy(execution)
            raise
        execution.update({
            "state": "returned", "log_dir": str(log_dir), "command": result[2],
            "stdout": {"sha256": result[0].sha256, "bytes": result[0].size},
            "stderr": {"sha256": result[1].sha256, "bytes": result[1].size},
        })
        self.step("preparation", execution=execution)
        return result

    def finish_host(self):
        if not self.monitor_host:
            return None
        if self.host_raw is not None:
            return self.host_raw
        if self.host_monitor is None:
            raise EvidenceError("monitored capture has no observed producer execution")
        monitor = self.host_monitor
        try:
            raw = monitor.finish()
        finally:
            if monitor.fd is None:
                self.host_monitor = None
        self.host_raw = raw
        self.step("host_observed", host_raw={
            "path": str(raw.path), "sha256": raw.sha256, "bytes": raw.size,
        })
        return raw

    def committed(self, document):
        self.state["commit_state"] = "returned"
        self.state["status"] = "committed"
        self.step("committed", capture_digest=document["digest"])

    def __exit__(self, kind, error, traceback):
        try:
            monitor_error = None
            if self.host_monitor is not None:
                monitor = self.host_monitor
                try:
                    raw = monitor.finish(failed=True)
                    self.state["observations"]["host_monitor"] = {
                        "status": "failed", "path": str(raw.path),
                        "sha256": raw.sha256, "bytes": raw.size,
                    }
                except BaseException as observed_error:
                    monitor_error = observed_error
                    self.state["observations"]["host_monitor"] = {
                        "status": "incomplete", "path": str(monitor.path),
                        "error": str(observed_error)[:4096],
                    }
                finally:
                    if monitor.fd is None:
                        self.host_monitor = None
                if self.primary is None and error is None and monitor_error is None:
                    monitor_error = EvidenceError("monitored capture returned without host publication")
            primary = self.primary or error or monitor_error
            missing_commit = primary is None and self.state["commit_state"] != "returned"
            if missing_commit:
                primary = EvidenceError("capture returned without a complete profile commit")
            if primary is not None:
                message = str(primary)
                self.state.update(status="failed", updated_ns=time.time_ns(), error={
                    "type": type(primary).__name__, "message": message[:16384],
                    "message_truncated": len(message) > 16384,
                })
                if error is not None and error is not primary:
                    self.state["error"]["secondary"] = {
                        "type": type(error).__name__, "message": str(error)[:16384],
                    }
                if monitor_error is not None and monitor_error is not primary:
                    self.state["error"]["monitor"] = {
                        "type": type(monitor_error).__name__, "message": str(monitor_error)[:16384],
                    }
                try:
                    encoded = canonical_json(self.state).encode()
                    _check_control_size(len(encoded), self.failure)
                    write_raw_file(self.failure, [encoded])
                except BaseException as persistence:
                    raise EvidenceError(
                        f"capture failed: {primary}; failure record NOT_PERSISTED: {persistence}; "
                        f"retained work: {self.work}"
                    ) from primary
                primary.add_note(f"capture failure record: {self.failure}")
                if missing_commit or (monitor_error is not None and error is None and self.primary is None):
                    raise primary
        finally:
            _active_capture.reset(self.token)
        return False


def capture_entrypoint(profile=None, *, profile_argument="profile", repo_argument="repo",
                       root_argument="root", monitor_host=False):
    """Bind all adapter/CLI entrypoints to one epoch, including nested promotion."""
    def decorate(function):
        signature = inspect.signature(function)

        @functools.wraps(function)
        def wrapped(*args, **kwargs):
            arguments = signature.bind(*args, **kwargs).arguments
            repo, root = arguments[repo_argument], arguments[root_argument]
            selected = profile if profile is not None else arguments[profile_argument]

            def invoke(epoch):
                try:
                    result = function(*args, **kwargs)
                except BaseException as error:
                    epoch.reject(error)
                    raise
                if type(result) is int and result != 0:
                    epoch.reject(EvidenceError(
                        f"{function.__name__} returned exit {result} during {epoch.state['phase']}"
                    ))
                else:
                    epoch.require_healthy()
                return result

            existing = _active_capture.get()
            if existing is not None:
                if (existing.root != root.absolute() or existing.profile != selected
                        or existing.repo != repo.resolve()):
                    error = EvidenceError("nested publication differs from its capture epoch")
                    existing.reject(error)
                    raise error
                if monitor_host and not existing.monitor_host:
                    error = EvidenceError("nested monitored capture lacks host reservation authority")
                    existing.reject(error)
                    raise error
                existing.require_healthy()
                return invoke(existing)
            with CaptureEpoch(repo, root, selected, monitor_host=monitor_host) as epoch:
                return invoke(epoch)

        return wrapped
    return decorate


def _directories(root: Path) -> None:
    # Reject intermediate links as well as linked final documents.
    for path in (root, root / "captures", root / "profiles", root / "work", root / "failures"):
        if path.is_symlink():
            raise EvidenceError(f"capture directory is a symlink: {path}")
        if path.exists() and not path.is_dir():
            raise EvidenceError(f"capture path is not a directory: {path}")
    for path in root.absolute().parents:
        if path.is_symlink():
            raise EvidenceError(f"capture ancestor is a symlink: {path}")


@publication
def publish_capture(
    root: Path,
    *,
    capture_id: str,
    profile: str,
    registry_digest: str,
    expected_cases: dict[str, list[str | None]],
    runs: list[dict],
    replay: Callable[[RunStore, dict], object],
    verify_source: Callable[[], None],
) -> dict:
    """Publish prepared runs and their complete profile under one GC custody.

    Producers must have completed before entry. Domain replay and source checks
    remain caller-owned; no profile pointer changes until every run passes them.
    Failed epochs may leave immutable unreferenced runs for explicit collection.
    """
    from evidence_bridge import host_from_observations, promote_native_run, verify_host_binding

    epoch = current_capture()
    epoch.require_healthy()
    if (epoch.root, epoch.profile, epoch.capture_id) != (root.absolute(), profile, capture_id):
        raise EvidenceError("publication differs from its capture epoch")
    host_raw = epoch.finish_host()
    epoch.step("publication_inventory")
    _run_id(capture_id)
    _run_id(profile)
    if not isinstance(runs, list) or not runs:
        raise EvidenceError("publication needs a nonempty prepared run list")
    prepared, expected = deepcopy(runs), deepcopy(expected_cases)
    if host_raw is not None:
        for run in prepared:
            if (not isinstance(run, dict) or not isinstance(run.get("raw_files"), dict)
                    or HOST_RAW_NAME in run["raw_files"] or not isinstance(run.get("inputs"), list)
                    or any(item.get("id") == HOST_INPUT_ID for item in run["inputs"] if isinstance(item, dict))
                    or not isinstance(run.get("host"), dict)):
                raise EvidenceError("prepared run conflicts with capture host observation owner")
            run["raw_files"][HOST_RAW_NAME] = host_raw
            run["inputs"].append({
                "id": HOST_INPUT_ID, "availability": "present",
                "digest": host_raw.sha256, "reason": None,
            })
            run["host"] = host_from_observations(
                host_raw, policy=run["host"]["policy"],
                capture_id=capture_id, profile=profile,
            )
    actual, ids, source = {}, set(), None
    for run in prepared:
        if not isinstance(run, dict) or "evidence_root" in run or run.get("profile") != profile:
            raise EvidenceError("prepared run has wrong publication root or profile")
        run_id = _run_id(run.get("run_id"))
        family = _run_id(run.get("family"))
        if run_id in ids:
            raise EvidenceError("publication repeats a prepared run ID")
        ids.add(run_id)
        actual.setdefault(family, []).append(run.get("case_id"))
        if not isinstance(run.get("source"), dict):
            raise EvidenceError("prepared run has no source identity")
        if source is None:
            source = run["source"]
        elif source != run["source"]:
            raise EvidenceError("publication mixes prepared source identities")
    _check_cases(actual, expected)
    target = root / "captures" / f"{capture_id}.json"
    if target.exists() or target.is_symlink():
        raise EvidenceError("capture ID already exists")
    # Establish Rust-collector exclusion BEFORE materializing any immutable run.
    (root / "captures").mkdir(exist_ok=True)
    store, promoted = RunStore(root), []
    for run in prepared:
        epoch.step("publication_source", run_id=run["run_id"])
        verify_source()
        epoch.require_healthy()
        epoch.step("promotion", source=run["source"], inputs=run.get("inputs"), prepared_command=run.get("command"))
        result = promote_native_run(evidence_root=root, **run)
        epoch.step("promoted_load", run_id=result["run_id"])
        record = store.load(result["run_id"])
        verify_host_binding(store, record, capture_id=capture_id)
        epoch.step("domain_replay")
        replay(store, record)
        epoch.require_healthy()
        promoted.append(record["run_id"])
    epoch.step("final_source", promoted_run_ids=promoted)
    verify_source()
    epoch.require_healthy()
    epoch.state["commit_state"] = "attempted"
    epoch.step("commit")
    document = commit_capture(
        root,
        capture_id=capture_id,
        profile=profile,
        registry_digest=registry_digest,
        expected_cases=expected,
        run_ids=promoted,
    )
    epoch.committed(document)
    return document


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
    encoded = canonical_json(document).encode()
    # Refuse before leaving an unreadable immutable reference that poisons GC.
    _check_control_size(len(encoded), target)
    target.parent.mkdir(parents=True, exist_ok=True)
    if target.exists() or target.is_symlink():
        raise EvidenceError("capture ID already exists")
    # Exclusive creation: two writers must not overwrite an immutable capture.
    with target.open("xb") as handle:
        handle.write(encoded)
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
    from evidence_bridge import verify_host_binding

    _run_id(profile)
    _directories(root)
    pointer = None
    if capture_id is None:
        pointer = parse_json(_read_control_file(root / "profiles" / f"{profile}.json").decode())
        if not isinstance(pointer, dict) or set(pointer) != {"capture_id", "digest"}:
            raise EvidenceError("profile pointer is malformed")
        capture_id = pointer["capture_id"]
    _run_id(capture_id)
    document = parse_json(_read_control_file(root / "captures" / f"{capture_id}.json").decode())
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
        verify_host_binding(store, evidence, capture_id=capture_id)
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
