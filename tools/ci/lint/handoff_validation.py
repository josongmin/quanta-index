"""SEP-21 handoff Git/archive validation and fixed product-chain policy.

The leaf owns one handoff interpretation for the CLI and aggregate. It takes
the proof checker as an injected dependency and never imports it back.
"""

from __future__ import annotations

import hashlib
import json
import os
import secrets
import stat
import subprocess
from collections.abc import Callable, Sequence
from pathlib import Path
from types import ModuleType
from typing import Any, BinaryIO, TypeVar

import jsonschema

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - Python 3.10 compatibility
    import tomli as tomllib


ROOT = Path(__file__).resolve().parents[3]
SCHEMA_PATH = (
    ROOT / "docs/plans/sep-21-search-plane-sota-hardening/tickets/handoffs/lane-handoff.schema.json"
)
PROOF_SCHEMA_PATH = ROOT / "tools/ci/proof-manifest.schema.json"
PROOF_REGISTRY_PATH = ROOT / "tools/ci/proof-authority.toml"


# Fixed handoff authority, deliberately independent from the operational proof
# registry. The single-handoff CLI and chain checker share this one table.
HANDOFF_POLICIES: dict[str, dict[str, Any]] = {
    "P00": {"ticket": "S21-00+S21-13A", "required": ["p00-authority-freeze"]},
    "P01": {"ticket": "S21-01", "required": ["p01-canonical-identity"]},
    "P02A": {"ticket": "S21-03", "required": ["p02a-repomap-compiler"]},
    "P02B": {"ticket": "S21-04", "required": ["p02b-operation-journal"]},
    "P02I": {
        "ticket": "S21-03+S21-04",
        "required": ["p02a-repomap-compiler", "p02b-operation-journal"],
    },
    "P03": {
        "ticket": "S21-01+S21-02",
        "required": ["p03-candidate-activation-owner"],
        "release": ["p03-candidate-activation"],
    },
    "P04": {
        "ticket": "S21-05",
        "required": ["p04-read-view-lifetime-owner"],
        "release": ["p04-read-view-lifetime"],
    },
    "P05": {
        "ticket": "S21-06",
        "required": ["p05-query-truth-owner"],
        "release": ["p05-query-truth"],
    },
    "P06": {
        "ticket": "S21-07",
        "required": ["p06-sdk-binding-owner"],
        "release": ["p06-sdk-binding"],
    },
    "P07": {
        "ticket": "S21-08",
        "required": ["p07-provider-boundary-owner"],
        "release": ["p07-provider-boundary"],
    },
    "P08": {
        "ticket": "S21-09",
        "required": ["p08-runtime-supervisor-owner"],
        "release": ["p08-runtime-supervisor"],
    },
    "P09": {
        "ticket": "S21-10",
        "required": ["p09-control-readiness-owner"],
        "release": ["p09-control-readiness"],
    },
    "P10": {
        "ticket": "S21-11",
        "required": ["p10-state-migration-owner"],
        "release": ["p10-state-migration"],
    },
    "P11": {
        "ticket": "S21-12",
        "required": ["p11-cross-repo-cutover"],
        "release": ["p11-deployment", "p11-activation", "p11-rollback"],
    },
    "P12A": {
        "ticket": "S21-13",
        "required": ["p12a-proof-infrastructure"],
        "deferred": ["p12-final-qualification"],
    },
    "P12": {"ticket": "S21-13", "required": ["p12-final-qualification"]},
}

PRODUCT_LANES: tuple[str, ...] = tuple(HANDOFF_POLICIES)[:-2]
RECORDED_OWNER_STATES = frozenset(("OWNER_PROOF_GREEN", "RELEASE_PROOF_PENDING"))


def validate_product_handoff_chain(handoffs: Sequence[dict[str, Any]]) -> list[str]:
    """Reject missing/reordered lanes and broken immediate source-history edges.

    P02A and P02B both fork from P01. P02I may start at the common base or
    either branch tip, but its integration receipt must name both branch tips;
    the single-handoff validator separately proves the applied commits.
    """
    lanes = [item.get("lane") for item in handoffs]
    if lanes != list(PRODUCT_LANES):
        return [f"product handoff lanes differ from fixed order: expected {list(PRODUCT_LANES)}"]

    by_lane = dict(zip(PRODUCT_LANES, handoffs))
    errors: list[str] = []
    for lane, item in by_lane.items():
        if item.get("status") not in RECORDED_OWNER_STATES:
            errors.append(f"{lane} has no recorded owner-proof handoff")

    def edge(predecessor: str, successor: str) -> None:
        if by_lane[successor].get("base_sha") != by_lane[predecessor].get("result_sha"):
            errors.append(f"{successor} base_sha differs from {predecessor} result_sha")

    edge("P00", "P01")
    edge("P01", "P02A")
    edge("P01", "P02B")

    join = by_lane["P02I"]
    accepted_bases = {by_lane[lane].get("result_sha") for lane in ("P01", "P02A", "P02B")}
    if join.get("base_sha") not in accepted_bases:
        errors.append("P02I base_sha is not the common base or either branch tip")
    integration = join.get("integration_commits")
    if (
        not isinstance(integration, list)
        or len(integration) != 2
        or not all(isinstance(item, dict) for item in integration)
        or [item.get("lane") for item in integration] != ["P02A", "P02B"]
    ):
        errors.append("P02I integration_commits must name P02A then P02B")
    else:
        for lane, item in zip(("P02A", "P02B"), integration):
            if item.get("original_sha") != by_lane[lane].get("result_sha"):
                errors.append(f"P02I {lane} original_sha differs from {lane} result_sha")

    edge("P02I", "P03")
    for predecessor, successor in zip(PRODUCT_LANES[5:-1], PRODUCT_LANES[6:]):
        edge(predecessor, successor)
    return errors


_ReadResult = TypeVar("_ReadResult")


def _consume_repo_regular_file(
    root: Path,
    value: str,
    *,
    label: str,
    consume: Callable[[BinaryIO], _ReadResult],
) -> _ReadResult:
    """Consume one complete repo-relative file from a no-follow descriptor walk."""

    path = _repo_path(root, value, label=label)
    parts = Path(value).parts
    directory_fd = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        for part in parts[:-1]:
            next_fd = os.open(
                part,
                os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW,
                dir_fd=directory_fd,
            )
            os.close(directory_fd)
            directory_fd = next_fd
        file_fd = os.open(
            parts[-1],
            os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK,
            dir_fd=directory_fd,
        )
        with os.fdopen(file_fd, "rb") as handle:
            before = os.fstat(handle.fileno())
            if not stat.S_ISREG(before.st_mode):
                raise ValueError(f"{label} is not a regular non-symlink file: {path}")
            result = consume(handle)
            after = os.fstat(handle.fileno())
            if (
                handle.tell() != before.st_size
                or after.st_size != before.st_size
                or after.st_mtime_ns != before.st_mtime_ns
                or after.st_ctime_ns != before.st_ctime_ns
            ):
                raise ValueError(f"{label} changed while being read: {path}")
            return result
    finally:
        os.close(directory_fd)


def _read_repo_regular_bytes(root: Path, value: str, *, label: str) -> bytes:
    return _consume_repo_regular_file(
        root, value, label=label, consume=lambda handle: handle.read()
    )


def _repo_entry_present_no_follow(root: Path, value: str, *, label: str) -> bool:
    """Distinguish an absent repo entry from an unsafe symlink or ancestor."""

    _repo_path(root, value, label=label)
    parts = Path(value).parts
    directory_fd = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        for part in parts[:-1]:
            try:
                next_fd = os.open(
                    part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=directory_fd
                )
            except FileNotFoundError:
                return False
            os.close(directory_fd)
            directory_fd = next_fd
        try:
            os.stat(parts[-1], dir_fd=directory_fd, follow_symlinks=False)
        except FileNotFoundError:
            return False
        return True
    finally:
        os.close(directory_fd)


def _sha256_repo_regular_file(root: Path, value: str, *, label: str) -> str:
    def digest_file(handle: BinaryIO) -> str:
        digest = hashlib.sha256()
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
        return digest.hexdigest()

    return _consume_repo_regular_file(root, value, label=label, consume=digest_file)


def _open_repo_output_parent(root: Path, relative: str, *, create: bool = True) -> int:
    """Create/open a repo output parent with each ancestor pinned and no-follow."""

    _repo_path(root, relative, label="output")
    directory_fd = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        for part in Path(relative).parts[:-1]:
            if create:
                try:
                    os.mkdir(part, mode=0o700, dir_fd=directory_fd)
                except FileExistsError:
                    pass
            next_fd = os.open(
                part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=directory_fd
            )
            os.close(directory_fd)
            directory_fd = next_fd
        return directory_fd
    except BaseException:
        os.close(directory_fd)
        raise


def _write_output_temporary(parent_fd: int, name: str, content: bytes) -> str:
    temporary_name = f".{name}.{secrets.token_hex(16)}.tmp"
    descriptor = os.open(
        temporary_name,
        os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
        0o600,
        dir_fd=parent_fd,
    )
    try:
        with os.fdopen(descriptor, "wb") as handle:
            handle.write(content)
            handle.flush()
            os.fsync(handle.fileno())
    except BaseException:
        os.unlink(temporary_name, dir_fd=parent_fd)
        raise
    return temporary_name


def _read_output_regular_bytes(parent_fd: int, name: str) -> bytes | None:
    try:
        descriptor = os.open(name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=parent_fd)
    except FileNotFoundError:
        return None
    with os.fdopen(descriptor, "rb") as handle:
        before = os.fstat(handle.fileno())
        if not stat.S_ISREG(before.st_mode):
            raise ValueError(f"output is not a regular non-symlink file: {name}")
        content = handle.read()
        after = os.fstat(handle.fileno())
        if (
            len(content) != before.st_size
            or after.st_size != before.st_size
            or after.st_mtime_ns != before.st_mtime_ns
            or after.st_ctime_ns != before.st_ctime_ns
        ):
            raise ValueError(f"output changed while being read: {name}")
        return content


def _restore_prior_output(parent_fd: int, name: str, prior_bytes: bytes | None) -> None:
    if prior_bytes is None:
        try:
            os.unlink(name, dir_fd=parent_fd)
        except FileNotFoundError:
            pass
    else:
        temporary_name = _write_output_temporary(parent_fd, name, prior_bytes)
        try:
            os.replace(temporary_name, name, src_dir_fd=parent_fd, dst_dir_fd=parent_fd)
        except BaseException:
            os.unlink(temporary_name, dir_fd=parent_fd)
            raise
    os.fsync(parent_fd)


def _require_output_parent_identity(root: Path, relative: str, parent_fd: int) -> None:
    """Refuse a parent that was renamed or replaced after its descriptor was opened."""

    observed_fd = _open_repo_output_parent(root, relative, create=False)
    try:
        original = os.fstat(parent_fd)
        observed = os.fstat(observed_fd)
        if (original.st_dev, original.st_ino) != (observed.st_dev, observed.st_ino):
            raise ValueError(f"output parent changed during publication: {relative}")
    finally:
        os.close(observed_fd)


def _repo_path(root: Path, value: str, *, label: str) -> Path:
    candidate = Path(value)
    if (
        not value
        or "\\" in value
        or candidate.is_absolute()
        or ".." in candidate.parts
        or candidate.as_posix() != value
        or candidate == Path(".")
    ):
        raise ValueError(f"{label} must be canonical repo-relative: {value!r}")
    return root / candidate


def _git(root: Path, *args: str, check: bool = True) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["git", "-C", str(root), *args],
        check=check,
        capture_output=True,
        text=True,
    )


def _is_ancestor(root: Path, ancestor: str, descendant: str) -> bool:
    return (
        _git(root, "merge-base", "--is-ancestor", ancestor, descendant, check=False).returncode == 0
    )


def _patch_id(root: Path, commit: str) -> str:
    shown = _git(root, "show", "--pretty=format:", "--binary", commit).stdout
    completed = subprocess.run(
        ["git", "-C", str(root), "patch-id", "--stable"],
        input=shown,
        check=True,
        capture_output=True,
        text=True,
    )
    fields = completed.stdout.split()
    if not fields:
        raise ValueError(f"commit has no stable patch identity: {commit}")
    return fields[0]


def _git_delta_paths(root: Path, base_sha: str, result_sha: str) -> list[str]:
    completed = subprocess.run(
        [
            "git",
            "-C",
            str(root),
            "diff",
            "--name-only",
            "--no-renames",
            "-z",
            base_sha,
            result_sha,
            "--",
        ],
        check=True,
        capture_output=True,
    )
    return sorted(
        part.decode("utf-8", errors="surrogateescape")
        for part in completed.stdout.split(b"\0")
        if part
    )


def _git_blob_sha256(root: Path, result_sha: str, path: str) -> str:
    candidate = Path(path)
    if candidate.is_absolute() or ".." in candidate.parts or candidate.as_posix() != path:
        raise ValueError(f"exported contract path is not canonical: {path!r}")
    completed = subprocess.run(
        ["git", "-C", str(root), "show", f"{result_sha}:{path}"],
        check=False,
        capture_output=True,
    )
    if completed.returncode != 0:
        raise ValueError(f"exported contract is absent from result commit: {path}")
    return hashlib.sha256(completed.stdout).hexdigest()


def _validate_lane_policy(payload: dict[str, Any]) -> list[str]:
    errors: list[str] = []
    lane = payload["lane"]
    policy = HANDOFF_POLICIES.get(lane)
    if policy is None:
        return [f"lane has no canonical handoff policy: {lane}"]
    if payload["ticket"] != policy["ticket"]:
        errors.append(f"ticket differs from {lane} authority: expected {policy['ticket']}")
    required = policy.get("required", [])
    release = policy.get("release", [])
    deferred = policy.get("deferred", [])
    expected_ids = [*required, *release, *deferred]
    proof_by_id = {item["id"]: item for item in payload["proofs"]}
    if [item["id"] for item in payload["proofs"]] != expected_ids:
        errors.append(f"proof IDs differ from {lane} authority: expected {expected_ids}")
        return errors
    if payload["status"] == "BLOCKED":
        return errors
    required_recorded = all(proof_by_id[item]["status"] == "RECORDED" for item in required)
    release_not_run = [item for item in release if proof_by_id[item]["status"] == "NOT_RUN"]
    deferred_not_run = all(proof_by_id[item]["status"] == "NOT_RUN" for item in deferred)
    if payload["status"] == "IMPLEMENTATION_DONE":
        if required_recorded:
            errors.append("IMPLEMENTATION_DONE cannot contain every required owner proof")
        return errors
    if not required_recorded:
        errors.append(f"{payload['status']} requires every {lane} owner proof to be RECORDED")
    if not deferred_not_run:
        errors.append(f"{lane} deferred proof must remain NOT_RUN")
    expected_status = "RELEASE_PROOF_PENDING" if release_not_run else "OWNER_PROOF_GREEN"
    if payload["status"] != expected_status:
        errors.append(f"status is not derived from {lane} proof states: expected {expected_status}")
    return errors


def validate_handoff(
    payload: Any,
    *,
    handoff_path: Path,
    root: Path,
    proof_checker: ModuleType,
    require_result_head: bool = False,
) -> list[str]:
    errors: list[str] = []
    proof_registry_path = root / PROOF_REGISTRY_PATH.relative_to(ROOT)
    schema = json.loads(
        _read_repo_regular_bytes(
            root, SCHEMA_PATH.relative_to(ROOT).as_posix(), label="handoff schema"
        )
    )
    validator = jsonschema.Draft202012Validator(schema)
    for error in sorted(validator.iter_errors(payload), key=lambda item: list(item.path)):
        location = ".".join(str(part) for part in error.path) or "root"
        errors.append(f"schema {location}: {error.message}")
    if not isinstance(payload, dict) or errors:
        return errors

    proof_schema = json.loads(
        _read_repo_regular_bytes(
            root, PROOF_SCHEMA_PATH.relative_to(ROOT).as_posix(), label="proof schema"
        )
    )
    registry = tomllib.loads(
        _read_repo_regular_bytes(
            root, PROOF_REGISTRY_PATH.relative_to(ROOT).as_posix(), label="proof registry"
        ).decode("utf-8")
    )
    registry_findings = proof_checker.check_registry(registry, root=root, path=proof_registry_path)
    if registry_findings:
        return [f"proof registry is invalid: {finding.message}" for finding in registry_findings]
    proof_by_id = {
        proof["id"]: proof
        for proof in registry.get("proofs", [])
        if isinstance(proof, dict) and isinstance(proof.get("id"), str)
    }

    base_sha = payload["base_sha"]
    result_sha = payload["result_sha"]
    for label, sha in (("base_sha", base_sha), ("result_sha", result_sha)):
        if _git(root, "cat-file", "-e", f"{sha}^{{commit}}", check=False).returncode != 0:
            errors.append(f"{label} is not a local commit: {sha}")
    if not errors and not _is_ancestor(root, base_sha, result_sha):
        errors.append("base_sha is not an ancestor of result_sha")
    if require_result_head and _git(root, "rev-parse", "HEAD").stdout.strip() != result_sha:
        errors.append("result_sha is not current HEAD")

    expected_name = f"{payload['lane']}.json"
    if handoff_path.name != expected_name:
        errors.append(f"handoff filename differs from lane authority: expected {expected_name}")
    errors.extend(_validate_lane_policy(payload))
    if not errors:
        actual_write_set = _git_delta_paths(root, base_sha, result_sha)
        if sorted(payload["write_set"]) != actual_write_set:
            errors.append(
                f"write_set differs from exact base..result Git delta: expected {actual_write_set}"
            )
    for contract in payload["exported_contracts"]:
        path, separator, claimed = contract.rpartition("@sha256:")
        if not separator:
            errors.append(f"exported contract has no digest binding: {contract!r}")
            continue
        try:
            actual = _git_blob_sha256(root, result_sha, path)
        except ValueError as error:
            errors.append(str(error))
            continue
        if claimed != actual:
            errors.append(f"exported contract digest differs from result commit: {path}")

    proof_ids = [item["id"] for item in payload["proofs"]]
    if len(proof_ids) != len(set(proof_ids)):
        errors.append("proof IDs are not unique")
    not_run_ids = [item["id"] for item in payload["proofs"] if item["status"] == "NOT_RUN"]
    if len(payload["not_run"]) != len(set(payload["not_run"])):
        errors.append("not_run entries are not unique")
    if payload["not_run"] != not_run_ids:
        errors.append("not_run must exactly equal NOT_RUN proof IDs in proof order")

    repositories = payload.get("paired_repositories")
    paired_checkouts = (
        {item["identity"]: Path(item["root"]).resolve() for item in repositories}
        if isinstance(repositories, list)
        else {}
    )

    exact_pair_manifests: list[dict[str, Any]] = []
    for item in payload["proofs"]:
        if item["status"] != "RECORDED":
            continue
        if not (item["selected"] == item["executed"] == item["passed"]):
            errors.append(f"proof {item['id']} must satisfy selected == executed == passed")
        authority = proof_by_id.get(item["id"])
        if authority is None:
            errors.append(f"proof {item['id']} is not registered")
            continue
        if item["command"] != authority["command"]:
            errors.append(f"proof {item['id']} command differs from registry")
        if item["required_host"] != authority["required_host"]:
            errors.append(f"proof {item['id']} required_host differs from registry")
        try:
            manifest_path = _repo_path(root, item["manifest"], label="proof manifest")
        except ValueError as error:
            errors.append(str(error))
            continue
        try:
            manifest_bytes = _read_repo_regular_bytes(
                root, item["manifest"], label="proof manifest archive"
            )
        except (OSError, ValueError) as error:
            errors.append(f"proof manifest is not a regular non-symlink archive: {error}")
            continue
        manifest_digest = hashlib.sha256(manifest_bytes).hexdigest()
        if manifest_digest != item["manifest_sha256"]:
            errors.append(f"proof {item['id']} manifest digest mismatch")
            continue
        try:
            manifest = json.loads(manifest_bytes)
        except (UnicodeDecodeError, json.JSONDecodeError) as error:
            errors.append(f"proof {item['id']} manifest is unreadable: {error}")
            continue
        if not isinstance(manifest, dict):
            errors.append(f"proof {item['id']} manifest root is not an object")
            continue
        try:
            expected_path = proof_checker.proof_archive_relative_path(
                manifest,
                manifest_digest,
            )
        except (KeyError, TypeError, ValueError) as error:
            errors.append(f"proof {item['id']} archive identity is invalid: {error}")
            continue
        if item["manifest"] != expected_path:
            errors.append(f"proof {item['id']} manifest is not its immutable archive path")
        if manifest.get("proof_id") != item["id"]:
            errors.append(f"proof {item['id']} manifest proof_id mismatch")
            continue
        if isinstance(manifest.get("source_pair"), dict):
            exact_pair_manifests.append(manifest)
        if manifest.get("status") != "passed":
            errors.append(f"proof {item['id']} manifest status is not passed")
        source = manifest.get("source", {})
        if source.get("head") != result_sha:
            errors.append(f"proof {item['id']} source HEAD differs from handoff result_sha")
        if source.get("dirty_digest") != f"sha256:{payload['dirty_digest']}":
            errors.append(f"proof {item['id']} dirty digest differs from handoff")
        if manifest.get("counts") != {
            "selected": item["selected"],
            "executed": item["executed"],
            "passed": item["passed"],
            "failed": item["failed"],
            "ignored": item["ignored"],
        }:
            errors.append(f"proof {item['id']} counts differ from manifest")
        errors.extend(
            finding.message
            for finding in proof_checker.check_manifest(
                manifest,
                manifest_path=manifest_path,
                proof=authority,
                schema=proof_schema,
                root=root,
                bind_source=require_result_head,
                paired_checkouts=paired_checkouts or None,
                proof_by_id=proof_by_id,
            )
        )

    if isinstance(repositories, list):
        quanta = repositories[0]
        if quanta["base_sha"] != base_sha:
            errors.append("top-level base_sha differs from paired quanta entry")
        if quanta["result_sha"] != result_sha:
            errors.append("top-level result_sha differs from paired quanta entry")
        if quanta["dirty_digest"] != payload["dirty_digest"]:
            errors.append("top-level dirty_digest differs from paired quanta entry")
        pair = repositories[1]
        for manifest in exact_pair_manifests:
            source_pair = manifest["source_pair"]
            if pair["identity"] != source_pair["repository"]:
                errors.append("paired repository identity differs from proof source_pair")
            if pair["result_sha"] != source_pair["source"]["head"]:
                errors.append("paired repository result_sha differs from proof source_pair")
            if f"sha256:{pair['dirty_digest']}" != source_pair["source"]["dirty_digest"]:
                errors.append("paired repository dirty_digest differs from proof source_pair")
            if pair["dependency_root_digest"] != source_pair["dependency_lock"]["sha256"]:
                errors.append(
                    "paired repository dependency_root_digest differs from proof source_pair"
                )
        for repository in repositories:
            push = repository["push"]
            if push["result"] == "PUSHED" and push["remote_sha"] != repository["result_sha"]:
                errors.append(
                    f"paired repository {repository['identity']} remote_sha differs from result_sha"
                )
            if require_result_head:
                checkout = Path(repository["root"]).resolve()
                if not checkout.is_dir():
                    errors.append(f"paired repository root is missing: {checkout}")
                    continue
                for label in ("base_sha", "result_sha"):
                    sha = repository[label]
                    if (
                        _git(
                            checkout, "cat-file", "-e", f"{sha}^{{commit}}", check=False
                        ).returncode
                        != 0
                    ):
                        errors.append(
                            f"paired repository {repository['identity']} {label} is not a local commit"
                        )
                if not _is_ancestor(checkout, repository["base_sha"], repository["result_sha"]):
                    errors.append(
                        f"paired repository {repository['identity']} base is not an ancestor of result"
                    )
                if _git(checkout, "rev-parse", "HEAD").stdout.strip() != repository["result_sha"]:
                    errors.append(
                        f"paired repository {repository['identity']} result_sha is not current HEAD"
                    )
                live_source = proof_checker.source_snapshot(checkout)
                if live_source["dirty_digest"] != f"sha256:{repository['dirty_digest']}":
                    errors.append(
                        f"paired repository {repository['identity']} dirty digest is not current"
                    )
                try:
                    lock_digest = _sha256_repo_regular_file(
                        checkout, "Cargo.lock", label="paired dependency lock"
                    )
                except (OSError, ValueError):
                    lock_digest = None
                if lock_digest != repository["dependency_root_digest"]:
                    errors.append(
                        f"paired repository {repository['identity']} dependency root is not current"
                    )

    integration_commits = payload.get("integration_commits")
    if isinstance(integration_commits, list):
        for item in integration_commits:
            if item["integration_mode"] == "MERGE":
                if item["original_sha"] != item["applied_sha"]:
                    errors.append(f"{item['lane']} MERGE must preserve original_sha as applied_sha")
                if not _is_ancestor(root, item["original_sha"], result_sha):
                    errors.append(
                        f"{item['lane']} original commit is not an ancestor of result_sha"
                    )
            else:
                if not _is_ancestor(root, item["applied_sha"], result_sha):
                    errors.append(f"{item['lane']} applied commit is not an ancestor of result_sha")
                try:
                    if _patch_id(root, item["original_sha"]) != _patch_id(
                        root, item["applied_sha"]
                    ):
                        errors.append(f"{item['lane']} cherry-pick patch identity differs")
                except (subprocess.CalledProcessError, ValueError) as error:
                    errors.append(f"{item['lane']} patch identity is unavailable: {error}")
    return errors


HANDOFF_DIRECTORY = "artifacts/sep-21/handoffs"


def inspect_handoff_ledger(
    *, root: Path, proof_checker: ModuleType
) -> tuple[dict[str, Any], list[str]]:
    """Derive diagnostic refs from canonical on-disk handoffs, never from a claim.

    Every readable handoff is validated against its historical Git/archive
    authority. The P12A infrastructure handoff is separate from the product
    history; neither can be replaced by a current-source proof manifest.
    """
    findings: list[str] = []
    payloads: list[dict[str, Any]] = []

    def inspect(lane: str) -> dict[str, Any]:
        relative = f"{HANDOFF_DIRECTORY}/{lane}.json"
        path = root / relative
        reference: dict[str, Any] = {
            "lane": lane,
            "path": relative,
            "sha256": None,
            "status": "NOT_RUN",
        }
        if not path.exists() and not path.is_symlink():
            findings.append(f"{relative}: handoff is missing")
            return reference
        reference["status"] = "FAILED"
        if path.is_symlink():
            findings.append(f"{relative}: handoff is not a regular non-symlink file")
            return reference
        try:
            content = _read_repo_regular_bytes(root, relative, label="handoff")
            reference["sha256"] = hashlib.sha256(content).hexdigest()
            payload = json.loads(content)
            errors = validate_handoff(
                payload,
                handoff_path=path,
                root=root,
                proof_checker=proof_checker,
                require_result_head=False,
            )
        except (
            OSError,
            ValueError,
            KeyError,
            TypeError,
            AttributeError,
            tomllib.TOMLDecodeError,
            subprocess.CalledProcessError,
        ) as error:
            findings.append(f"{relative}: unreadable or invalid handoff: {error}")
            return reference
        findings.extend(f"{relative}: {error}" for error in errors)
        if errors:
            return reference
        if payload.get("status") not in RECORDED_OWNER_STATES:
            findings.append(f"{relative}: no recorded owner-proof handoff")
            return reference
        reference["status"] = "VERIFIED"
        if lane in PRODUCT_LANES:
            payloads.append(payload)
        return reference

    product = [inspect(lane) for lane in PRODUCT_LANES]
    if any(item["status"] == "FAILED" for item in product):
        chain_status = "FAILED"
    elif any(item["status"] == "NOT_RUN" for item in product):
        chain_status = "NOT_RUN"
    else:
        chain_errors = validate_product_handoff_chain(payloads)
        findings.extend(chain_errors)
        chain_status = "FAILED" if chain_errors else "VERIFIED"
    infrastructure = inspect("P12A")
    return {
        "product_handoffs": product,
        "product_chain_status": chain_status,
        "infrastructure_handoff": infrastructure,
    }, findings
