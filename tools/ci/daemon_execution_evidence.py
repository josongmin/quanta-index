"""Typed executable observations made by the passing daemon test bodies."""

from __future__ import annotations

import hashlib
import ipaddress
import os
import re
import stat
from collections.abc import Mapping
from pathlib import Path
from typing import Any
from urllib.parse import urlsplit

try:
    import tomllib
except ModuleNotFoundError:
    import tomli as tomllib

from tools.ci.proof_json import parse_proof_json

WITNESS_DIR_ENV = "QUANTA_PROOF_DAEMON_WITNESS_DIR"
WITNESS_NONCE_ENV = "QUANTA_PROOF_DAEMON_WITNESS_NONCE"
DAEMON_PATH_ENV = "QUANTA_INDEX_SEARCHD_BIN"
P07_PROVIDER_INPUT_ENV = "QUANTA_PROOF_P07_PROVIDER_CONFIG"
P07_PROVIDER_PIN_ENV = "QUANTA_PROOF_P07_PROVIDER_PIN"
P07_PROVIDER_SHA_ENV = "QUANTA_PROOF_P07_PROVIDER_SHA256"
P07_PROVIDER_PIN_NAME = "p07-provider-config.json"
R5_WITNESS_FILE = "r5-caller-daemon.json"
REGISTRY_PATH = "tools/ci/proof-authority.toml"
P04_PROOF = "p04-read-view-lifetime"
P04_SCOPE = "read-view-lifetime"
P07_PROOF = "p07-provider-boundary"
P07_SCOPE = "provider-boundary-release"
P07_EVENT = (
    "quanta-index-searchd-runtime::native_product_process$"
    "native_release::native_provider_enforces_approved_budget_and_cancellation"
)
P07_WITNESS_FILE = "p07-native-provider.json"
P10_PROOF = "p10-state-migration"
P10_SCOPE = "state-migration-release"
NATIVE_RELEASE_SCOPES = {
    "candidate-activation-release": "p03-candidate-activation",
    P04_SCOPE: P04_PROOF,
    "query-truth-release": "p05-query-truth",
    "sdk-binding-release": "p06-sdk-binding",
    P07_SCOPE: P07_PROOF,
    "runtime-supervisor-release": "p08-runtime-supervisor",
    "control-readiness-release": "p09-control-readiness",
    P10_SCOPE: P10_PROOF,
}
NATIVE_STATE_FORMAT_SOURCE = "crates/quanta-index-contract/src/index.rs"
R5_PROOF = "p11-cross-repo-cutover"

# Profiles used by receipts whose committed registry predates explicit selections.
# These are historical format definitions and must remain immutable.
_P04_PREFIX = "quanta-index-searchd-runtime::process_readiness_owner_v1$"
_P04_V1 = {
    _P04_PREFIX + "legacy_read_view_domains::sealed_track_read_view_scenarios_share_one_fixture": (
        "p04-read-view-domains.json",
        1,
    ),
    _P04_PREFIX
    + "legacy_read_view_selection::retired_selected_generation_refuses_before_open_while_fresh_active_serves_g3": (
        "p04-read-view-selection.json",
        1,
    ),
}
R5Case = tuple[str, str, str, str, str, str]
_R5_V1: tuple[R5Case, ...] = (
    (
        "caller",
        "quanta-runtime",
        "index-sdk-ingress,retrieval-authority-contract-surface",
        "index_sdk_ingress_publish_contract_test",
        "test",
        "index_sdk_ingress_live_repomap_roundtrip_survives_runtime_restart_v1",
    ),
    (
        "kernel",
        "quanta-runtime-retrieval-kernel",
        "index-sdk-ingress-surface",
        "quanta_runtime_retrieval_kernel",
        "lib",
        "index_sdk_ingress::terminal_receipt_v1::tests::repomap_v2_receipts_require_exact_full_bundle_and_transition_v2",
    ),
)
_ATOM = r"[A-Za-z0-9_][A-Za-z0-9_-]*"
_RUST_TEST = r"[A-Za-z_][A-Za-z0-9_]*(?:::[A-Za-z_][A-Za-z0-9_]*)*"


def read_proof_registry(root: Path) -> dict[str, Any]:
    from tools.ci.lint.handoff_validation import _read_repo_regular_bytes

    return tomllib.loads(
        _read_repo_regular_bytes(root, REGISTRY_PATH, label="proof registry").decode("utf-8")
    )


def live_proof(root: Path, proof_id: str) -> dict[str, Any]:
    matches = [row for row in read_proof_registry(root)["proofs"] if row["id"] == proof_id]
    if len(matches) != 1:
        raise ValueError(f"execution proof {proof_id!r} must be registered exactly once")
    return matches[0]


def p04_witness_specs(
    proof: dict[str, Any], *, allow_legacy: bool = False
) -> dict[str, tuple[str, int]]:
    if allow_legacy and "daemon_observation_tests" not in proof:
        return dict(_P04_V1)
    rows = proof.get("daemon_observation_tests")
    if not isinstance(rows, list) or len(rows) != 2:
        raise ValueError("P04 requires exactly two executable observation tests")
    return _witness_rows(rows, "P04", (1, 2) if allow_legacy else (2,))


def _witness_rows(
    rows: list,
    label: str,
    allowed_launches: tuple[int, ...] | range,
) -> dict[str, tuple[str, int]]:
    files = {}
    for row in rows:
        if (
            not isinstance(row, dict)
            or set(row) != {"event", "file", "launches"}
            or not isinstance(row["event"], str)
            or re.fullmatch(rf"{_ATOM}::{_ATOM}\${_RUST_TEST}", row["event"]) is None
            or not isinstance(row["file"], str)
            or re.fullmatch(rf"{_ATOM}\.json", row["file"]) is None
            or type(row["launches"]) is not int
            or row["launches"] not in allowed_launches
        ):
            raise ValueError(f"invalid {label} executable observation test")
        files[row["event"]] = (row["file"], row["launches"])
    if len(files) != len(rows) or len({filename for filename, _ in files.values()}) != len(rows):
        raise ValueError(f"{label} executable observation tests and files must be distinct")
    return files


def daemon_witness_specs(
    proof: dict[str, Any],
    *,
    allow_legacy: bool = False,
) -> dict[str, tuple[str, int]]:
    proof_id = proof["id"]
    if proof_id == P04_PROOF:
        return p04_witness_specs(proof, allow_legacy=allow_legacy)
    if proof_id not in NATIVE_RELEASE_SCOPES.values():
        return {}
    if allow_legacy and "daemon_observation_tests" not in proof:
        return {}
    rows = proof.get("daemon_observation_tests")
    if proof_id != P10_PROOF:
        if not isinstance(rows, list) or not rows:
            raise ValueError(f"{proof_id} requires registered daemon observation tests")
        witnesses = _witness_rows(rows, proof_id, range(1, 256))
        if proof_id == P07_PROOF and witnesses != {P07_EVENT: (P07_WITNESS_FILE, 2)}:
            raise ValueError("P07 requires the exact provider case and two daemon observations")
        return witnesses
    event = (
        "quanta-index-searchd-runtime::native_product_process$"
        "native_boot::native_cross_root_sdk_rebuild_refuses_unverified_switch"
    )
    filename = "native-p10-rebuild-daemon-witness.json"
    expected = {"event": event, "file": filename, "launches": 4}
    if rows != [expected] or type(rows[0]["launches"]) is not int:
        raise ValueError("P10 requires the exact Native rebuild test and four daemon observations")
    return {event: (filename, 4)}


def p07_provider_approval_digest(proof: Mapping[str, Any]) -> str:
    digest = proof.get("provider_approval_sha256")
    if not isinstance(digest, str) or re.fullmatch(r"[0-9a-f]{64}", digest) is None:
        raise ValueError("P07 requires a registered approved provider config SHA-256")
    return digest


def read_p07_provider_approval(proof: Mapping[str, Any], env: Mapping[str, str]) -> bytes:
    from tools.benchmark.evidence import read_control

    approved = p07_provider_approval_digest(proof)
    path = Path(env.get(P07_PROVIDER_INPUT_ENV, ""))
    if not path.is_absolute():
        raise ValueError(f"P07 requires an absolute approved config in {P07_PROVIDER_INPUT_ENV}")
    raw = read_control(path)
    if len(raw) > 16_384 or hashlib.sha256(raw).hexdigest() != approved:
        raise ValueError("P07 provider config bytes differ from their registered approval")
    provider = parse_proof_json(raw)
    if not isinstance(provider, dict) or provider.get("kind") != "open_ai":
        raise ValueError("P07 requires an approved Native OpenAI provider config")
    base_url = provider.get("base_url")
    if not isinstance(base_url, str):
        raise ValueError("P07 approved provider must have a remote HTTPS origin")
    url = urlsplit(base_url)
    host = (url.hostname or "").lower().rstrip(".")
    try:
        address = ipaddress.ip_address(host)
        local_address = address.is_loopback or address.is_unspecified
    except ValueError:
        local_address = False
    if (
        url.scheme != "https"
        or not host
        or host == "localhost"
        or host.endswith(".localhost")
        or local_address
        or url.username is not None
        or url.password is not None
        or url.path
        or url.query
        or url.fragment
    ):
        raise ValueError("P07 approved provider must have a remote HTTPS origin")
    budget = provider.get("work_budget")
    quote = budget.get("openai_max_request_cost_micros") if isinstance(budget, dict) else None
    if (
        type(quote) is not int
        or not 0 < quote <= (2**64 - 1) // 2
        or type(budget.get("total_cost_micros_ceiling")) is not int
        or budget.get("total_cost_micros_ceiling") != 2 * quote
        or type(budget.get("retry_attempts_cap")) is not int
        or budget["retry_attempts_cap"] != 0
        or type(provider.get("max_retries")) is not int
        or provider["max_retries"] != 0
    ):
        raise ValueError("P07 approval must fund exactly two paid attempts with retries disabled")
    key_env = provider.get("api_key_env")
    if (
        not isinstance(key_env, str)
        or re.fullmatch(r"[A-Z_][A-Z0-9_]*", key_env) is None
        or not env.get(key_env)
    ):
        raise ValueError("P07 approved provider credential is unavailable")
    return raw


def require_daemon_test_inventory(
    inventory: Mapping[str, tuple[str, str, str]], names: set[str]
) -> None:
    if not names.issubset(inventory):
        raise ValueError("nextest inventory omitted a registered daemon witness test")
    if any(inventory[name][2] != "test" for name in names):
        raise ValueError("daemon witnesses require Cargo integration test targets")


def native_state_root_format(source: bytes) -> str:
    """Read the version used by the Native marker checked by the P10 body."""
    versions = re.findall(rb"(?m)^native_index_state_format!\(([1-9][0-9]{0,9})\);$", source)
    if len(versions) != 1 or int(versions[0]) > 0xFFFF_FFFF:
        raise ValueError("P10 issuing source lacks one canonical Native state version")
    return f"v{versions[0].decode('ascii')}"


def r5_cases(proof: dict[str, Any], *, allow_legacy: bool = False) -> tuple[R5Case, ...]:
    if allow_legacy and "paired_execution_cases" not in proof:
        return _R5_V1
    rows = proof.get("paired_execution_cases")
    fields = ("role", "package", "features", "binary", "kind", "test")
    if not isinstance(rows, list) or len(rows) != 2:
        raise ValueError("R5 requires exactly one caller and one kernel")
    cases = []
    # `rows` is proven to contain exactly two entries above. Avoid
    # `zip(strict=...)` so the repository's Python 3.9 control-plane runtime
    # can execute the registry lint as well as the pinned Python 3.12 rail.
    for row, role, kind in zip(rows, ("caller", "kernel"), ("test", "lib")):
        if (
            not isinstance(row, dict)
            or set(row) != set(fields)
            or any(not isinstance(row[key], str) for key in fields)
            or (row["role"], row["kind"]) != (role, kind)
            or any(re.fullmatch(_ATOM, row[key]) is None for key in ("package", "binary"))
            or re.fullmatch(rf"{_ATOM}(?:,{_ATOM})*", row["features"]) is None
            or re.fullmatch(_RUST_TEST, row["test"]) is None
        ):
            raise ValueError(f"invalid R5 {role} execution selection")
        cases.append(tuple(row[key] for key in fields))
    return tuple(cases)


def read_witness(directory: Path, filename: str) -> bytes:
    parent = os.open(directory, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        descriptor = os.open(filename, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=parent)
        with os.fdopen(descriptor, "rb") as stream:
            if not stat.S_ISREG(os.fstat(stream.fileno()).st_mode):
                raise ValueError(f"daemon witness is not a regular file: {filename}")
            raw = stream.read(4097)
            if len(raw) > 4096:
                raise ValueError(f"daemon witness is too large: {filename}")
            return raw
    finally:
        os.close(parent)


def validate_nonce(value: object) -> str:
    if not isinstance(value, str) or re.fullmatch(r"[0-9a-f]{32}", value) is None:
        raise ValueError("invalid daemon witness nonce")
    return value


def validate_witness(
    row: object,
    *,
    test: str,
    nonce: str,
    launches: int,
    provider_approval_sha256: str | None = None,
) -> str:
    fields = {"schema_version", "test", "nonce", "executables"}
    if provider_approval_sha256 is not None:
        fields.add("provider_approval_sha256")
    if (
        not isinstance(row, dict)
        or set(row) != fields
        or type(row["schema_version"]) is not int
        or row["schema_version"] != 1
        or row["test"] != test
        or row["nonce"] != validate_nonce(nonce)
        or row.get("provider_approval_sha256") != provider_approval_sha256
    ):
        raise ValueError(f"daemon witness differs from its selected execution: {test}")
    digests = row["executables"]
    if (
        not isinstance(digests, list)
        or len(digests) != launches
        or any(
            not isinstance(item, str) or re.fullmatch(r"[0-9a-f]{64}", item) is None
            for item in digests
        )
        or len(set(digests)) != 1
    ):
        raise ValueError(
            f"daemon witness must bind {launches} launch(es) of one executable: {test}"
        )
    return digests[0]


def verify_r5_witness(raw: bytes, *, test: str, nonce: str, daemon_sha256: str) -> None:
    observed = validate_witness(parse_proof_json(raw), test=test, nonce=nonce, launches=2)
    if observed != daemon_sha256:
        raise ValueError("paired R5 executed daemon differs from the release binary")
