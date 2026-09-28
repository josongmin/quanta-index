"""Produce and replay bounded, source-bound T15/T16 owner execution evidence.

The bundle carries raw command streams and input bytes; summaries are rederived.
Like portable_proof, this is local execution custody, not OS attestation. T15
uses pinned assets and every vector component. T16 compares complete logical
semantic and membership rows, including vectors and payloads, not top-k IDs.
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import math
import os
import struct
import unicodedata
from pathlib import Path

from tools.benchmark.retrieval import parity_reference as reference
from tools.benchmark.retrieval import portable_proof, tool_custody
from tools.ci import source_closure
from tools.ci.lint.handoff_validation import _read_repo_regular_bytes

ROOT = Path(__file__).resolve().parents[3]
MAX_BYTES = 32 * 1024 * 1024
VECTOR_TOLERANCE = 0.002
COSINE_TOLERANCE = 0.005
RECIPES = {
    "model_vectors": ("quanta-index-embed", "quanta-index-vector-proof", []),
    "incremental_rows": (
        "quanta-index-semantic",
        "quanta-index-incremental-proof",
        ["--features", "proof"],
    ),
}
REQUIRED_INCREMENTAL_CASES = {
    "append",
    "replace",
    "tombstone",
    "clear_surface",
    "membership_replace",
}


def canonical(value: object) -> bytes:
    return json.dumps(
        value, sort_keys=True, separators=(",", ":"), ensure_ascii=False, allow_nan=False
    ).encode()


def sha(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def load(value: bytes) -> object:
    def unique(pairs):
        result = {}
        for key, item in pairs:
            if key in result:
                raise ValueError(f"duplicate conditional JSON key: {key}")
            result[key] = item
        return result

    if len(value) > MAX_BYTES:
        raise ValueError("conditional artifact exceeds byte limit")
    return json.loads(
        value,
        object_pairs_hook=unique,
        parse_constant=lambda value: (_ for _ in ()).throw(ValueError(f"nonfinite {value}")),
    )


def exact(value: object, keys: set[str], where: str) -> dict:
    if not isinstance(value, dict) or set(value) != keys:
        raise ValueError(f"{where} has missing or unexpected fields")
    return value


def artifact(payload: bytes) -> dict:
    if len(payload) > MAX_BYTES:
        raise ValueError("conditional artifact exceeds byte limit")
    return {"sha256": sha(payload), "bytes_base64": base64.b64encode(payload).decode()}


def decode(value: object) -> bytes:
    row = exact(value, {"sha256", "bytes_base64"}, "conditional artifact")
    if not isinstance(row["bytes_base64"], str) or len(row["bytes_base64"]) > MAX_BYTES * 2:
        raise ValueError("invalid conditional artifact bytes")
    payload = base64.b64decode(row["bytes_base64"], validate=True)
    if len(payload) > MAX_BYTES or sha(payload) != row["sha256"]:
        raise ValueError("conditional artifact digest mismatch")
    return payload


def finite_number(value: object, bound: float = 1.7976931348623157e308) -> bool:
    # Bound arbitrary JSON integers before math.isfinite converts them to
    # f64; otherwise malformed evidence can raise an uncaught OverflowError.
    return type(value) in (int, float) and abs(value) <= bound and math.isfinite(value)


def vector(value: object, where: str) -> list[float]:
    if (
        not isinstance(value, list)
        or len(value) != 256
        or any(not finite_number(item, 2) for item in value)
    ):
        raise ValueError(f"{where} must contain all 256 finite components")
    return value


def model_rows(observed: object, baseline: object, inputs: object) -> tuple[list[dict], int]:
    observed = exact(
        observed,
        {
            "schema_version",
            "model_id",
            "model_revision",
            "dimension",
            "normalization",
            "max_length",
            "inputs",
            "vectors",
            "reversed_vectors",
        },
        "encoder output",
    )
    baseline = exact(
        baseline,
        {
            "schema_version",
            "profile",
            "library",
            "model",
            "policy",
            "inputs",
            "vectors",
            "norms",
            "pairwise_cosine_upper",
            "dimension",
        },
        "reference output",
    )
    model = exact(
        baseline["model"],
        {"id", "revision", "dir_name", "safetensors_sha256", "tokenizer_sha256", "config_sha256"},
        "reference model",
    )
    if (
        type(observed["schema_version"]) is not int
        or type(baseline["schema_version"]) is not int
        or observed["schema_version"] != 1
        or baseline["schema_version"] != reference.SCHEMA_VERSION
        or type(observed["dimension"]) is not int
        or type(baseline["dimension"]) is not int
        or observed["dimension"] != 256
        or baseline["dimension"] != 256
        or observed["model_id"] != "model2vec:minishlab/potion-code-16M-v2"
        or observed["model_revision"]
        != "e9d2a44ca6a05ac6685f3b23709ea57eb7352d5b:model2vec-rs-0.3.0:fancy-regex:full-length-v2"
        or observed["normalization"] != "l2_unit"
        or observed["max_length"] is not None
        or model["id"] != reference.MODEL_ID
        or model["revision"] != reference.MODEL_REVISION
        or baseline["profile"] != reference.REFERENCE_PROFILE
        or baseline["library"] != {"model2vec": reference.MODEL2VEC_VERSION}
        or baseline["policy"]
        != {
            "max_length": None,
            "tokenizer_embedded_truncation_disabled": True,
            "normalization": "approx-unit-fp16 (rail L2-normalizes both sides)",
        }
    ):
        raise ValueError("encoder policy/model identity drift")
    for name, expected in reference.PINNED_ASSET_SHA256.items():
        key = {
            "model.safetensors": "safetensors_sha256",
            "tokenizer.json": "tokenizer_sha256",
            "config.json": "config_sha256",
        }[name]
        if model[key] != expected:
            raise ValueError("reference asset differs from pin")
    if (
        observed["inputs"] != inputs
        or baseline["inputs"] != inputs
        or not isinstance(inputs, list)
        or not inputs
        or any(not isinstance(item, str) for item in inputs)
    ):
        raise ValueError("raw vectors name different inputs")
    if any(
        not isinstance(output, list) or len(output) != len(inputs)
        for output in (observed["vectors"], observed["reversed_vectors"], baseline["vectors"])
    ):
        raise ValueError("raw vector output is partial")
    rows = []
    passed = 0
    normalized = [
        reference.l2_normalize(vector(value, "reference vector")) for value in baseline["vectors"]
    ]
    norms = [
        math.sqrt(sum(component * component for component in value))
        for value in baseline["vectors"]
    ]
    triangle = baseline["pairwise_cosine_upper"]
    if (
        not isinstance(triangle, list)
        or len(triangle) != len(inputs)
        or any(
            not isinstance(row, list)
            or len(row) != len(inputs) - index - 1
            or any(not finite_number(value) for value in row)
            for index, row in enumerate(triangle)
        )
    ):
        raise ValueError("reference pairwise metadata must be a complete finite numeric triangle")
    if (
        not isinstance(baseline["norms"], list)
        or len(baseline["norms"]) != len(norms)
        or any(
            not finite_number(actual) or abs(actual - expected) > 1e-6
            for actual, expected in zip(baseline["norms"], norms, strict=True)
        )
        or baseline["pairwise_cosine_upper"]
        != [
            [reference.cosine(left, right) for right in normalized[i + 1 :]]
            for i, left in enumerate(normalized)
        ]
    ):
        raise ValueError("reference norm/pairwise metadata differs from complete raw vectors")
    observed_vectors = [vector(value, "observed vector") for value in observed["vectors"]]
    reversed_vectors = list(
        reversed([vector(value, "reversed vector") for value in observed["reversed_vectors"]])
    )
    for index, (actual, expected, permuted) in enumerate(
        zip(observed_vectors, normalized, reversed_vectors, strict=True)
    ):
        norm = math.sqrt(sum(value * value for value in actual))
        pairwise_ok = all(
            abs(reference.cosine(actual, other) - reference.cosine(expected, normalized[j]))
            < COSINE_TOLERANCE
            for j, other in enumerate(observed_vectors)
        )
        ok = (
            actual == permuted
            and abs(norm - 1.0) < VECTOR_TOLERANCE
            and pairwise_ok
            and all(
                abs(left - right) < VECTOR_TOLERANCE
                for left, right in zip(actual, expected, strict=True)
            )
        )
        passed += ok
        rows.append(
            {
                "case_id": f"input-{index:06d}",
                "input_sha256": sha(inputs[index].encode()),
                "reference_vector": expected,
                "observed_vector": actual,
                "permuted_vector": permuted,
            }
        )
    return rows, passed


SEMANTIC_COLUMNS = set(
    "embedding_id record_id repo_relative_path owner_id owner_kind corpus_kind parent_owner_id source_doc_id language package symbol_kind visibility source_role generated capability_status authority_digest render_policy_digest card_schema_version embedding_input_digest vector_digest start_line end_line snippet vector".split()
)
MEMBERSHIP_COLUMNS = set(
    "cluster_record_id authority_digest owner_kind owner_id member_symbol_id ordinal member_count membership_content_digest".split()
)


NATIVE_ENUMS = {
    "owner_kind": {
        "File",
        "Module",
        "Symbol",
        "Chunk",
        "Callsite",
        "GraphEdge",
        "Dataflow",
        "Risk",
        "Test",
        "RepoMap",
        "ServiceMap",
        "OwnerMap",
    },
    "corpus_kind": {
        "SymbolCard",
        "ModuleCard",
        "ClusterCard",
        "RawCodeFallback",
        "DocumentLeaf",
        "DocumentSection",
        "DocumentSummary",
        "TestBehavior",
        "RepositorySummary",
    },
    "source_role": {"CardText", "RawFallbackText", "DocumentText", "SummaryText"},
    "capability_status": {"Full", "Degraded", "Unsupported", "NotComputed"},
    "symbol_kind": {
        "function",
        "method",
        "class",
        "struct",
        "enum",
        "trait",
        "interface",
        "variable",
        "constant",
        "module",
        "macro",
        "type_alias",
    },
}


def native_enum(value: object, field: str) -> None:
    if not isinstance(value, str) or value not in NATIVE_ENUMS[field]:
        raise ValueError(f"logical {field} is outside the native enum contract")


def native_language_code(value: object) -> None:
    # contract-base query/constraints.rs::validate_language_code. This is a
    # syntax contract, not a fixed language allowlist or a new length policy.
    if (
        not isinstance(value, str)
        or not value
        or not "a" <= value[0] <= "z"
        or any(character not in "abcdefghijklmnopqrstuvwxyz0123456789-_+" for character in value)
    ):
        raise ValueError("logical language code is outside the native wire contract")


def native_utf8(value: object) -> None:
    # Rust String can represent every Unicode scalar, including controls, but
    # cannot deserialize the unpaired surrogates accepted by Python json.loads.
    pending = [value]
    while pending:
        cell = pending.pop()
        if isinstance(cell, str):
            try:
                cell.encode("utf-8")
            except UnicodeEncodeError as error:
                raise ValueError("native DTO text is not valid UTF-8") from error
        elif isinstance(cell, dict):
            pending.extend(cell.keys())
            pending.extend(cell.values())
        elif isinstance(cell, list):
            pending.extend(cell)


def native_repository_identity(value: object, field: str) -> None:
    # contract-base ids.rs::validate_identity; reject, never normalize.
    if (
        not isinstance(value, str)
        or not value
        or len(value) > 512
        or any(
            ord(character) <= 0x1F
            or 0x7F <= ord(character) <= 0x9F
            or 0xD800 <= ord(character) <= 0xDFFF
            for character in value
        )
        or len(value.encode("utf-8")) > 512
        or unicodedata.normalize("NFC", value) != value
    ):
        raise ValueError(f"incremental {field} is not a canonical repository identity")


def table(value: object, name: str) -> dict:
    value = exact(value, {"count", "rows"}, name)
    native_utf8(value)
    rows = value["rows"]
    if (
        type(value["count"]) is not int
        or value["count"] < 0
        or not isinstance(rows, list)
        or len(rows) != value["count"]
    ):
        raise ValueError("incremental table is partial")
    if any(not isinstance(row, dict) or not row for row in rows):
        raise ValueError("incremental table contains invalid rows")
    for row in rows:
        exact(
            row, SEMANTIC_COLUMNS if name == "semantic" else MEMBERSHIP_COLUMNS, "full logical row"
        )
        for field in NATIVE_ENUMS:
            if field in row and not (field == "symbol_kind" and row[field] is None):
                native_enum(row[field], field)
        if name == "semantic":
            native_language_code(row["language"])
        integer_fields = (
            {"start_line", "end_line", "card_schema_version"}
            if name == "semantic"
            else {"ordinal", "member_count"}
        )
        optional_fields = (
            {"parent_owner_id", "package", "symbol_kind", "visibility"}
            if name == "semantic"
            else set()
        )
        for key, cell in row.items():
            if key in integer_fields:
                if type(cell) is not int or not 0 <= cell < 2**32:
                    raise ValueError("logical integer cell is not a canonical u32")
            elif key == "generated":
                if type(cell) is not bool:
                    raise ValueError("logical generated cell must be a boolean")
            elif key != "vector" and not (
                isinstance(cell, str) or key in optional_fields and cell is None
            ):
                raise ValueError("logical text cell is not canonical")
        if name == "semantic" and (
            not isinstance(row["vector"], list)
            or not row["vector"]
            or any(not finite_number(value, 3.4028234663852886e38) for value in row["vector"])
        ):
            raise ValueError("raw semantic row lacks finite full vector")
    # The Rust exporter sorts using serde_json's canonical ASCII map order.
    encoded = [
        json.dumps(row, sort_keys=True, separators=(",", ":"), ensure_ascii=False) for row in rows
    ]
    if encoded != sorted(set(encoded)):
        raise ValueError("incremental rows are duplicated or reordered")
    identities = [
        (row.get("record_id"), row.get("embedding_id"))
        if name == "semantic"
        else (row.get("owner_kind"), row.get("owner_id"), row.get("ordinal"))
        for row in rows
    ]
    if len(set(identities)) != len(identities) or any(
        any(item is None for item in identity) for identity in identities
    ):
        raise ValueError("incremental row identities are missing/duplicated")
    return value


def model_contract(value: object) -> dict:
    contract = exact(
        value,
        {
            "model_id",
            "model_version",
            "dimension",
            "normalization",
            "distance_metric",
            "policy_digest",
            "view_policy_digest",
        },
        "incremental model contract",
    )
    if (
        type(contract["dimension"]) is not int
        or not 0 < contract["dimension"] < 2**32
        or any(
            not isinstance(contract[key], str) or not contract[key]
            for key in ("model_id", "policy_digest")
        )
        or any(
            contract[key] is not None and not isinstance(contract[key], str)
            for key in ("model_version", "view_policy_digest")
        )
        or not isinstance(contract["normalization"], str)
        or contract["normalization"] not in {"None", "L2Unit"}
        or contract["distance_metric"] != "Cosine"
    ):
        raise ValueError(
            "incremental model contract differs from the typed semantic owner contract"
        )
    return contract


def contract_vector(vector: object, contract: dict) -> list[float]:
    if (
        not isinstance(vector, list)
        or len(vector) != contract["dimension"]
        or any(not finite_number(value, 3.4028234663852886e38) for value in vector)
    ):
        raise ValueError("incremental vector differs from model dimension or finite f32 encoding")
    converted = [struct.unpack("<f", struct.pack("<f", value))[0] for value in vector]
    norm = math.sqrt(sum(value * value for value in converted))
    if (
        norm == 0
        or not math.isfinite(norm)
        or contract["normalization"] == "L2Unit"
        and abs(norm - 1) > 0.001
    ):
        raise ValueError("incremental vector violates nonzero/unit model normalization")
    return converted


def frozen_model_identities(records: object) -> list[tuple[str, str, str]]:
    if (
        not isinstance(records, list)
        or not records
        or any(
            not isinstance(record, dict)
            or not isinstance(record.get("captures"), dict)
            or not record["captures"]
            or any(
                not isinstance(capture, dict)
                or any(
                    not isinstance(capture.get(key), str) or not capture[key]
                    for key in ("system", "model", "model_revision")
                )
                for capture in record["captures"].values()
            )
            for record in records
        )
    ):
        raise ValueError("conditional frozen records lack typed capture/model identities")
    return sorted(
        set(
            (capture["system"], capture["model"], capture["model_revision"])
            for record in records
            for capture in record["captures"].values()
        )
    )


def input_state(batch: dict) -> dict:
    exact(
        batch,
        {
            "repo_id",
            "revision_id",
            "generation",
            "base_generation",
            "manifest_digest",
            "batch_digest",
            "mode",
            "model_contract",
            "required_corpora",
            "corpus_policy_digest",
            "clear_surfaces",
            "replace_scopes",
            "tombstone_scopes",
            "seal",
        },
        "native incremental batch",
    )
    native_utf8(batch)
    for field in ("repo_id", "revision_id"):
        native_repository_identity(batch[field], field)
    if (
        not isinstance(batch["required_corpora"], list)
        or any(
            not isinstance(batch[key], str)
            for key in ("repo_id", "revision_id", "manifest_digest", "batch_digest")
        )
        or batch["corpus_policy_digest"] is not None
        and not isinstance(batch["corpus_policy_digest"], str)
    ):
        raise ValueError("incremental batch identities/corpora are not native DTO values")
    for kind in batch["required_corpora"]:
        native_enum(kind, "corpus_kind")
    if (
        not isinstance(batch["clear_surfaces"], list)
        or any(
            not isinstance(value, str) or value not in {"File", "Module", "Chunk", "Symbol"}
            for value in batch["clear_surfaces"]
        )
        or not isinstance(batch["tombstone_scopes"], list)
    ):
        raise ValueError("incremental clear/tombstone scopes are not native DTO values")
    # StreamScopeAuthorityV1 uses enum declaration order, not lexical order.
    surface_order = {"File": 0, "Module": 1, "Chunk": 2, "Symbol": 3}
    clears = batch["clear_surfaces"]
    if clears != sorted(set(clears), key=surface_order.__getitem__):
        raise ValueError("incremental clear surfaces must be unique in native canonical order")
    tombstone_owners = set()
    for tombstone in batch["tombstone_scopes"]:
        exact(tombstone, {"semantic_scope"}, "native semantic tombstone")
        key = exact(
            tombstone["semantic_scope"],
            {"corpus_kind", "owner_kind", "owner_id"},
            "native semantic owner scope",
        )
        native_enum(key["corpus_kind"], "corpus_kind")
        native_enum(key["owner_kind"], "owner_kind")
        if not isinstance(key["owner_id"], str) or not key["owner_id"]:
            raise ValueError("incremental tombstone owner identity must be a nonempty string")
        identity = owner(key)
        if surface(key) in clears or identity in tombstone_owners:
            raise ValueError(
                "incremental scope authority forbids clear/tombstone conflicts or duplicate tombstones"
            )
        tombstone_owners.add(identity)
    contract = model_contract(batch["model_contract"])
    if not isinstance(batch["replace_scopes"], list):
        raise ValueError("incremental replace scopes must be an array")
    for scope in batch["replace_scopes"]:
        exact(
            scope,
            {"scope", "scope_digest", "embeddings", "cluster_memberships"},
            "incremental replace scope",
        )
        key = exact(scope["scope"], {"doc_surface", "repo_relative_path"}, "native search scope")
        if (
            not isinstance(scope["scope_digest"], str)
            or not isinstance(key["repo_relative_path"], str)
            or not isinstance(key["doc_surface"], str)
            or key["doc_surface"] not in {"File", "Module", "Chunk", "Symbol"}
        ):
            raise ValueError("incremental scope differs from native SearchScopeKey contract")
        if not isinstance(scope["embeddings"], list) or not isinstance(
            scope["cluster_memberships"], list
        ):
            raise ValueError("incremental embeddings and memberships must be arrays")
    records = [record for scope in batch["replace_scopes"] for record in scope["embeddings"]]
    for record in records:
        exact(
            record,
            SEMANTIC_COLUMNS | {"start_byte", "end_byte", "view_kind"},
            "native embedding record",
        )
        if any(
            type(record[field]) is not int or not 0 <= record[field] < 2**32
            for field in ("start_byte", "end_byte")
        ) or not isinstance(record["view_kind"], str):
            raise ValueError("incremental embedding bytes/view are not native DTO values")
    semantic = [{key: record[key] for key in SEMANTIC_COLUMNS} for record in records]
    for row in semantic:
        row["vector"] = contract_vector(row["vector"], contract)
    membership = []
    for scope in batch["replace_scopes"]:
        for replacement in scope["cluster_memberships"]:
            exact(
                replacement,
                {"cluster_record_id", "authority_digest", "members"},
                "cluster membership",
            )
            members = replacement["members"]
            if (
                any(
                    not isinstance(replacement[key], str) or not replacement[key]
                    for key in ("cluster_record_id", "authority_digest")
                )
                or not isinstance(members, list)
                or not 0 < len(members) <= 4096
                or any(not isinstance(member, str) or not member for member in members)
                or members != sorted(set(members))
            ):
                raise ValueError(
                    "cluster membership requires bounded canonical nonempty string identities"
                )
            record = next(
                (
                    row
                    for row in scope["embeddings"]
                    if row["record_id"] == replacement["cluster_record_id"]
                ),
                None,
            )
            if (
                record is None
                or record["corpus_kind"] != "ClusterCard"
                or replacement["authority_digest"] != record["authority_digest"]
            ):
                raise ValueError("membership lacks its authoritative record")
            digest = hashlib.sha256(b"quanta-index:cluster-membership-content:v1\0")
            for member in replacement["members"]:
                encoded = member.encode()
                digest.update(str(len(encoded)).encode() + b"\0" + encoded)
            for ordinal, member in enumerate(replacement["members"]):
                membership.append(
                    {
                        "cluster_record_id": record["record_id"],
                        "authority_digest": record["authority_digest"],
                        "owner_kind": record["owner_kind"],
                        "owner_id": record["owner_id"],
                        "member_symbol_id": member,
                        "ordinal": ordinal,
                        "member_count": len(replacement["members"]),
                        "membership_content_digest": "sha256:" + digest.hexdigest(),
                    }
                )
        expected = sorted(
            row["record_id"] for row in scope["embeddings"] if row["corpus_kind"] == "ClusterCard"
        )
        actual = [replacement["cluster_record_id"] for replacement in scope["cluster_memberships"]]
        if actual != expected or len(actual) != len(set(actual)):
            raise ValueError(
                "cluster membership must cover each ClusterCard once in canonical record order"
            )
    state = logical_state(semantic, membership)
    # Apply stream-wide identity/conflict admission only after full row types
    # have been validated, so malformed fields remain typed refusals.
    record_ids, replaced_owners = set(), set()
    for scope in batch["replace_scopes"]:
        if scope["scope"]["doc_surface"] in clears:
            raise ValueError("incremental scope authority forbids clear/replace conflicts")
        scope_owners = set()
        for record in scope["embeddings"]:
            if surface(record) in clears:
                raise ValueError("incremental scope authority forbids clear/replace conflicts")
            if record["record_id"] in record_ids:
                raise ValueError("incremental scope authority forbids duplicate record_id")
            record_ids.add(record["record_id"])
            scope_owners.add(owner(record))
        if scope_owners & tombstone_owners or scope_owners & replaced_owners:
            raise ValueError(
                "incremental scope authority forbids tombstone/replace conflicts or repeated replace owners"
            )
        replaced_owners.update(scope_owners)
    return state


def logical_state(semantic: list, membership: list) -> dict:
    result = {}
    for name, rows in (("semantic", semantic), ("membership", membership)):
        rows = sorted(
            rows,
            key=lambda row: json.dumps(
                row, sort_keys=True, separators=(",", ":"), ensure_ascii=False
            ),
        )
        result[name] = table({"count": len(rows), "rows": rows}, name)
    return result


def owner(row: dict, membership: bool = False) -> tuple:
    return ("ClusterCard" if membership else row["corpus_kind"], row["owner_kind"], row["owner_id"])


def surface(row: dict) -> str:
    if row["owner_kind"] in {"File", "Module", "Chunk", "Symbol"}:
        return row["owner_kind"]
    if row["corpus_kind"] in {"SymbolCard", "RawCodeFallback"}:
        return "Symbol"
    if row["corpus_kind"] in {"ModuleCard", "ClusterCard"}:
        return "Module"
    if row["corpus_kind"] in {
        "DocumentLeaf",
        "DocumentSection",
        "DocumentSummary",
        "TestBehavior",
        "RepositorySummary",
    }:
        return "File"
    raise ValueError("unknown semantic surface projection")


def operation_oracle(case: dict) -> tuple[dict, dict, dict]:
    before, fresh, changes = [input_state(case[key]) for key in ("before", "fresh", "delta")]
    delta, kind = case["delta"], case["case_id"]
    replaced = {owner(row) for row in changes["semantic"]["rows"]}
    removed = {owner(scope["semantic_scope"]) for scope in delta["tombstone_scopes"]}
    cleared = set(delta["clear_surfaces"])
    previous = {owner(row) for row in before["semantic"]["rows"]}
    if kind in {"append", "replace", "membership_replace"}:
        valid = bool(replaced) and not removed and not cleared
        valid = valid and (
            not (replaced & previous) if kind == "append" else bool(replaced & previous)
        )
        if kind == "append":
            previous_ids = {row["embedding_id"] for row in before["semantic"]["rows"]}
            valid = valid and not any(
                row["embedding_id"] in previous_ids for row in changes["semantic"]["rows"]
            )
    elif kind == "tombstone":
        valid = bool(removed & previous) and not replaced and not cleared
    elif kind == "clear_surface":
        valid = (
            bool(cleared)
            and not replaced
            and not removed
            and any(surface(row) in cleared for row in before["semantic"]["rows"])
        )
    else:
        valid = False

    def retained(row, membership=False):
        key = owner(row, membership)
        projected = {**row, "corpus_kind": key[0]} if membership else row
        return key not in replaced | removed and surface(projected) not in cleared

    unaffected = [row for row in before["semantic"]["rows"] if retained(row)]
    expected = logical_state(
        unaffected + changes["semantic"]["rows"],
        [row for row in before["membership"]["rows"] if retained(row, True)]
        + changes["membership"]["rows"],
    )
    changed = (
        before["membership"] != expected["membership"]
        if kind == "membership_replace"
        else before["semantic"] != expected["semantic"]
    )
    if not valid or not unaffected or not changed or fresh != expected:
        raise ValueError(
            "incremental mutation is vacuous or violates independent operation/unaffected-owner oracle"
        )
    return before, fresh, expected


def default_delete_predicate_admission(owners: object) -> None:
    """Independent golden for the private backend's 32-MiB SQL refusal.

    This bounds escaped delete text, not owner ID input or total residency.
    The renderer is not imported or invoked to derive this expectation.
    """
    semantic, membership = {}, {}
    for corpus, kind, identity in sorted(set(owners)):
        literal = len(identity.encode("utf-8")) + identity.count("'") + 2
        group = semantic.setdefault((corpus, kind), [])
        group.append(literal)
        if corpus == "ClusterCard":
            membership.setdefault(kind, []).append(literal)

    def quoted(value):
        return len(value.encode("utf-8")) + value.count("'") + 2

    semantic_bytes = sum(
        len("(corpus_kind =  AND owner_kind =  AND owner_id IN ())")
        + quoted(corpus)
        + quoted(kind)
        + sum(ids)
        + 2 * (len(ids) - 1)
        for (corpus, kind), ids in semantic.items()
    ) + 4 * max(0, len(semantic) - 1)
    membership_bytes = sum(
        len("(owner_kind =  AND owner_id IN ())") + quoted(kind) + sum(ids) + 2 * (len(ids) - 1)
        for kind, ids in membership.items()
    ) + 4 * max(0, len(membership) - 1)
    if max(semantic_bytes, membership_bytes) > 32 * 1024 * 1024:
        raise ValueError("incremental delete predicate exceeds private backend byte budget")


def default_window_operation_oracle(batch: dict) -> dict[str, int]:
    """Independent operation oracle for the registered resident proof recipe.

    The recipe uses the public default contract: 1024 indivisible owner groups,
    32 MiB f32 vectors. These are verifier expectations, never runtime policy.
    A policy change must deliberately migrate this oracle and its goldens.
    Counts are derived from inputs, not accepted from the producer's report.
    """
    max_owners, max_bytes = 1024, 32 * 1024 * 1024
    windows: list[list[tuple[int, tuple, list[dict]]]] = []
    current: list[tuple[int, tuple, list[dict]]] = []
    used_bytes = 0
    for scope_index, scope in enumerate(batch["replace_scopes"]):
        groups: dict[tuple, list[dict]] = {}
        for row in scope["embeddings"]:
            groups.setdefault(owner(row), []).append(row)
        for identity, rows in groups.items():
            byte_count = sum(4 * len(row["vector"]) for row in rows)
            if byte_count > max_bytes:
                raise ValueError("incremental input owner exceeds default window contract")
            if current and (len(current) == max_owners or used_bytes + byte_count > max_bytes):
                windows.append(current)
                current, used_bytes = [], 0
            current.append((scope_index, identity, rows))
            used_bytes += byte_count
    if current:
        windows.append(current)
    for window in windows:
        default_delete_predicate_admission(identity for _, identity, _ in window)
    fragments = sum(len({index for index, _, _ in window}) for window in windows)
    cluster_windows = sum(
        any(identity[0] == "ClusterCard" for _, identity, _ in window) for window in windows
    )
    membership_appends = 0
    for window in windows:
        for index in {index for index, _, _ in window}:
            record_ids = {
                row["record_id"]
                for scope_index, _, rows in window
                if scope_index == index
                for row in rows
            }
            membership_appends += any(
                item["members"] and item["cluster_record_id"] in record_ids
                for item in batch["replace_scopes"][index]["cluster_memberships"]
            )
    tombstones = batch["tombstone_scopes"]
    chunks = [
        tombstones[start : start + max_owners] for start in range(0, len(tombstones), max_owners)
    ]
    for chunk in chunks:
        default_delete_predicate_admission(owner(item["semantic_scope"]) for item in chunk)
    cluster_chunks = sum(
        any(item["semantic_scope"]["corpus_kind"] == "ClusterCard" for item in chunk)
        for chunk in chunks
    )
    clears = batch["clear_surfaces"]
    # Every admitted surface has an explicit matching ClusterCard owner kind.
    if any(surface not in {"File", "Module", "Symbol", "Chunk"} for surface in clears):
        raise ValueError("incremental clear surface is not in the public contract")
    return {
        "windows": len(windows),
        "replace_scopes": fragments,
        "semantic_delete_commits": len(windows) + len(clears) + len(chunks),
        "membership_delete_commits": cluster_windows + len(clears) + cluster_chunks,
        "membership_append_calls": membership_appends,
    }


def incremental_rows(observed: object, plan: object) -> tuple[list[dict], int]:
    observed = exact(observed, {"schema_version", "cases"}, "incremental output")
    plan = exact(plan, {"schema_version", "cases"}, "incremental plan")
    if (
        type(observed["schema_version"]) is not int
        or type(plan["schema_version"]) is not int
        or observed["schema_version"] != 1
        or plan["schema_version"] != 1
        or not isinstance(plan["cases"], list)
    ):
        raise ValueError("invalid incremental plan version")
    cases = plan["cases"]
    ids = [case["case_id"] for case in cases]
    if (
        ids != sorted(set(ids))
        or set(ids) != REQUIRED_INCREMENTAL_CASES
        or not isinstance(observed["cases"], list)
        or len(observed["cases"]) != len(cases)
    ):
        raise ValueError("incremental coverage differs from required mutation cases")
    rows = []
    passed = 0
    for case, raw in zip(cases, observed["cases"], strict=True):
        case = exact(case, {"case_id", "fresh", "before", "delta"}, "incremental case")
        raw = exact(
            raw, {"case_id", "fresh", "before", "incremental", "receipts"}, "incremental raw case"
        )
        if raw["case_id"] != case["case_id"]:
            raise ValueError("incremental case substitution")
        delta = case["delta"]
        fresh, before = case["fresh"], case["before"]
        if any(not isinstance(batch, dict) for batch in (fresh, before, delta)):
            raise ValueError("incremental batches must be objects")
        if (
            any(
                type(batch.get("generation")) is not int or not 0 <= batch["generation"] < 2**64
                for batch in (fresh, before, delta)
            )
            or type(delta.get("base_generation")) is not int
            or not 0 <= delta["base_generation"] < 2**64
        ):
            raise ValueError("incremental generation identities must be unsigned 64-bit integers")
        if (
            any(batch.get("seal") is not True for batch in (fresh, before, delta))
            or fresh.get("mode") != "ReplaceGeneration"
            or before.get("mode") != "ReplaceGeneration"
            or fresh.get("base_generation") is not None
            or before.get("base_generation") is not None
            or delta.get("mode") != "Delta"
            or delta.get("base_generation") != before["generation"]
            or delta["generation"] == before["generation"]
            or any(
                fresh.get(key) is None
                or fresh[key] != before.get(key)
                or fresh[key] != delta.get(key)
                for key in ("repo_id", "revision_id", "model_contract")
            )
        ):
            raise ValueError("incremental plan identities/modes are inconsistent")
        expected_before, expected_fresh, _ = operation_oracle(case)
        for key in ("before", "fresh", "incremental"):
            state = exact(raw[key], {"semantic", "membership"}, "logical state")
            for name in state:
                table(state[name], name)
            for row in state["semantic"]["rows"]:
                contract_vector(row["vector"], fresh["model_contract"])
        if raw["before"] != expected_before or raw["fresh"] != expected_fresh:
            raise ValueError(
                "before/fresh full state differs from independent input/operation oracle"
            )
        receipts = exact(raw["receipts"], {"fresh", "before", "delta"}, "build receipts")
        for key, batch in (("fresh", case["fresh"]), ("before", case["before"]), ("delta", delta)):
            expected_operations = default_window_operation_oracle(batch)
            receipt = exact(
                receipts[key],
                {
                    "generation",
                    "batch_digest",
                    "manifest_digest",
                    "windows",
                    "replace_scopes",
                    "rows",
                    "stages",
                },
                "owner execution receipt",
            )
            if (
                type(receipt["generation"]) is not int
                or not 0 <= receipt["generation"] < 2**64
                or any(
                    type(receipt[field]) is not int or not 0 <= receipt[field] < 2**64
                    for field in ("windows", "replace_scopes", "rows")
                )
                or any(
                    receipt[field] != batch[field]
                    for field in ("generation", "batch_digest", "manifest_digest")
                )
                or receipt["replace_scopes"] != expected_operations["replace_scopes"]
                or receipt["rows"]
                != sum(len(scope["embeddings"]) for scope in batch["replace_scopes"])
            ):
                raise ValueError("incremental build receipt does not bind input batch")
            stages = exact(
                receipt["stages"],
                {
                    "owner_scopes",
                    "windows",
                    "semantic_delete_calls",
                    "semantic_delete_commits",
                    "membership_delete_calls",
                    "membership_delete_commits",
                    "semantic_append_calls",
                    "membership_append_calls",
                    "durations",
                },
                "ingest stage report",
            )
            if (
                any(
                    type(value) is not int or not 0 <= value < 2**64
                    for key, value in stages.items()
                    if key != "durations"
                )
                or stages["owner_scopes"] != receipt["replace_scopes"]
                or stages["windows"] != receipt["windows"]
            ):
                raise ValueError("ingest stage report differs from actual batch tally")
            scopes, windows = receipt["replace_scopes"], receipt["windows"]
            if (
                (windows == 0) != (scopes == 0)
                or windows > scopes
                or windows != expected_operations["windows"]
                or stages["semantic_append_calls"] != windows
                or stages["membership_append_calls"]
                != expected_operations["membership_append_calls"]
            ):
                raise ValueError("ingest stage execution counts cannot produce the bound batch")
            mutations = len(batch["clear_surfaces"]) + len(batch["tombstone_scopes"])
            if (
                stages["semantic_delete_calls"] != scopes + mutations
                or stages["membership_delete_calls"] != scopes + mutations
                or stages["semantic_delete_commits"]
                != expected_operations["semantic_delete_commits"]
                or stages["membership_delete_commits"]
                != expected_operations["membership_delete_commits"]
            ):
                raise ValueError("ingest delete counts differ from bound owner/mutation execution")
            durations = exact(
                stages["durations"],
                {
                    "total",
                    "prepare",
                    "promotion",
                    "clear_surfaces",
                    "stream",
                    "semantic_delete",
                    "membership_delete",
                    "semantic_append",
                    "membership_append",
                    "tombstones",
                    "seal",
                    "embedding",
                },
                "ingest stage durations",
            )
            if durations["embedding"] is not None or any(
                type(value) is not int or not 0 <= value < 2**64
                for key, value in durations.items()
                if key != "embedding"
            ):
                raise ValueError("owner stage durations lack sealed/precomputed execution")
            if sum(
                durations[key]
                for key in (
                    "prepare",
                    "promotion",
                    "clear_surfaces",
                    "stream",
                    "tombstones",
                    "seal",
                )
            ) > durations["total"] or sum(
                durations[key]
                for key in (
                    "semantic_delete",
                    "membership_delete",
                    "semantic_append",
                    "membership_append",
                )
            ) > sum(durations[key] for key in ("clear_surfaces", "stream", "tombstones")):
                raise ValueError("ingest nested durations exceed containing stages")
        passed += raw["fresh"] == raw["incremental"]
        rows.append(raw)
    return rows, passed


def command_identity(command: dict, context: dict, source_root: Path) -> None:
    expected = {
        "CARGO_NET_OFFLINE": "true",
        **portable_proof.execution_overrides(
            context["tool_custody"]["tools"], context["environment"]["relevant"]
        ),
    }
    if (
        command["cwd"] != str(source_root)
        or command["inherited_environment"] != context["environment"]["relevant"]
        or command["environment"] != expected
        or command["environment_sha256"]
        != portable_proof._environment_digest(
            {
                **command["inherited_environment"],
                **command["environment"],
            }
        )
    ):
        raise ValueError("conditional command environment/source working directory substitution")


COMMAND_IDENTITY = {"cwd", "inherited_environment", "environment", "environment_sha256"}


def _command_frame(command: dict, out: Path, stdout: bytes) -> dict:
    stderr = _read_repo_regular_bytes(out, command["stderr"], label="conditional command stderr")
    if sha(stdout) != command["stdout_sha256"] or sha(stderr) != command["stderr_sha256"]:
        raise ValueError("conditional command bytes differ from captured execution")
    return {
        "argv": command["argv"],
        "exit_code": command["exit_code"],
        "stdout": artifact(stdout),
        "stderr": artifact(stderr),
        **{key: command[key] for key in COMMAND_IDENTITY},
    }


def rederive(kind: str, context: dict) -> tuple[dict, int]:
    inputs = load(decode(context["inputs"]))
    observed = load(decode(context["observed"]))
    if kind == "model_vectors":
        rows, passed = model_rows(observed, load(decode(context["reference"])), inputs)
    elif kind == "incremental_rows":
        if context["reference"] is not None:
            raise ValueError("incremental proof has unrelated reference")
        rows, passed = incremental_rows(observed, inputs)
    else:
        raise ValueError("unknown conditional raw kind")
    return {"kind": kind, "rows": rows}, passed


def validate_results(value: object, kind: str, *, verify_source: bool = False) -> dict:
    results = exact(
        value,
        {
            "schema_version",
            "command",
            "status",
            "selected",
            "executed",
            "passed",
            "failed",
            "identity",
            "raw_proof",
            "execution_receipt",
            "execution_context",
        },
        "conditional results v2",
    )
    identity = exact(
        results["identity"],
        {"source_revision", "repository_commit", "model_sha256", "dependency_sha256"},
        "conditional identity",
    )
    if (
        type(results["schema_version"]) is not int
        or results["schema_version"] != 2
        or results["command"] != f"retrieval-conditional-proof-v2:{kind}"
    ):
        raise ValueError("conditional protocol/recipe mismatch")
    for key, length in (
        ("source_revision", 40),
        ("repository_commit", 40),
        ("model_sha256", 64),
        ("dependency_sha256", 64),
    ):
        if (
            not isinstance(identity[key], str)
            or len(identity[key]) != length
            or any(char not in "0123456789abcdef" for char in identity[key])
        ):
            raise ValueError("invalid conditional identity digest")
    context = exact(
        results["execution_context"],
        {
            "source_closure",
            "cargo_lock_sha256",
            "uv_lock_sha256",
            "suite",
            "corpus",
            "records",
            "semble_lockfile",
            "inputs",
            "reference",
            "observed",
            "build",
            "run",
            "reference_run",
            "binary_sha256",
            "environment",
            "tool_custody",
            "source_manifest",
            "source_capture",
            "source_verify",
        },
        "conditional execution context",
    )
    tool_custody.validate_record(context["tool_custody"])
    tools = context["tool_custody"]["tools"]
    try:
        closure = source_closure.validate_manifest_shape(context["source_closure"])
    except source_closure.ClosureError as error:
        raise ValueError(str(error)) from error
    if closure["profile"] != "retrieval" or closure["revision"] != identity["source_revision"]:
        raise ValueError("conditional source closure substitution")
    closed_files = {entry["path"]: entry["sha256"] for entry in closure["files"]}
    if load(decode(context["source_manifest"])) != closure:
        raise ValueError("conditional source bytes differ from captured source closure")
    if tools["cargow"]["sha256"] != closed_files.get("scripts/cargow"):
        raise ValueError("conditional wrapper differs from closed source")
    if (
        closed_files.get("Cargo.lock") != context["cargo_lock_sha256"]
        or closed_files.get("uv.lock") != context["uv_lock_sha256"]
    ):
        raise ValueError("conditional dependency lockfile substitution")
    suite = load(decode(context["suite"]))
    corpus = load(decode(context["corpus"]))
    if (
        suite["repository_commit"] != identity["repository_commit"]
        or corpus["repository_commit"] != identity["repository_commit"]
        or sha(decode(context["semble_lockfile"])) != identity["dependency_sha256"]
    ):
        raise ValueError("conditional corpus/dependency identity substitution")
    records = load(decode(context["records"]))
    model_identities = frozen_model_identities(records)
    if not model_identities or sha(canonical(model_identities)) != identity["model_sha256"]:
        raise ValueError("conditional frozen model identity substitution")
    package, binary, features = RECIPES[kind]
    build = exact(
        context["build"],
        {"argv", "exit_code", "stdout", "stderr"} | COMMAND_IDENTITY,
        "conditional build",
    )
    expected_args = [
        tools["cargow"]["path"],
        "--lane",
        "test-daemon-lane",
        "build",
        "-p",
        package,
        "--bin",
        binary,
        *features,
        "--locked",
        "--message-format=json",
    ]
    # An absolute checkout path is permitted to differ on replay; all recipe
    # tokens following the wrapper are fixed and no shell is evaluated.
    if (
        not isinstance(build["argv"], list)
        or build["argv"] != expected_args
        or not Path(build["argv"][0]).is_absolute()
        or Path(build["argv"][0]).name != "cargow"
        or type(build["exit_code"]) is not int
        or build["exit_code"] != 0
    ):
        raise ValueError("conditional build recipe differs")
    build_events = [load(line) for line in decode(build["stdout"]).splitlines() if line.strip()]
    if any(
        not isinstance(event, dict)
        or not isinstance(event.get("reason"), str)
        or event["reason"] == "compiler-artifact"
        and not isinstance(event.get("target"), dict)
        for event in build_events
    ):
        raise ValueError("conditional build events must be typed objects")
    decode(build["stderr"])
    executables = [
        event["executable"]
        for event in build_events
        if event.get("reason") == "compiler-artifact"
        and event.get("target", {}).get("name") == binary
        and event.get("executable")
    ]
    terminals = [
        index for index, event in enumerate(build_events) if event.get("reason") == "build-finished"
    ]
    if (
        len(executables) != 1
        or terminals != [len(build_events) - 1]
        or build_events[-1].get("success") is not True
    ):
        raise ValueError("conditional build lacks one executable and successful terminal event")
    run = exact(
        context["run"],
        {"argv", "exit_code", "stdout", "stderr", "executable_sha256"} | COMMAND_IDENTITY,
        "conditional command",
    )
    if (
        not isinstance(run["argv"], list)
        or len(run["argv"]) != 3
        or any(not isinstance(arg, str) for arg in run["argv"])
        or run["argv"][0] != executables[0]
        or type(run["exit_code"]) is not int
        or run["exit_code"] != 0
        or run["executable_sha256"] != context["binary_sha256"]
        or not portable_proof._is_sha256(context["binary_sha256"])
    ):
        raise ValueError("conditional execution differs from built executable")
    if decode(run["stdout"]) != decode(context["observed"]):
        raise ValueError("conditional raw output differs from execution transcript")
    executable_epoch = context["tool_custody"]["epochs"].get(run["argv"][0])
    if executable_epoch is None or executable_epoch["sha256"] != context["binary_sha256"]:
        raise ValueError("conditional executable lacks selected invocation custody")
    decode(run["stderr"])
    environment = context["environment"]
    if (
        not isinstance(environment, dict)
        or not isinstance(environment.get("python_version"), str)
        or environment["python_version"].split(".")[:2] != ["3", "13"]
        or not isinstance(environment.get("relevant"), dict)
        or any(
            not isinstance(key, str) or not isinstance(value, str)
            for key, value in environment["relevant"].items()
        )
    ):
        raise ValueError("conditional producer requires standard Python 3.13")
    source_root = Path(build["argv"][0]).parent.parent
    if Path(build["argv"][0]) != source_root / "scripts/cargow":
        raise ValueError("conditional wrapper is not the source owner")
    source_capture = exact(
        context["source_capture"],
        {"argv", "exit_code", "stdout", "stderr"} | COMMAND_IDENTITY,
        "source capture command",
    )
    source_verify = exact(
        context["source_verify"],
        {"argv", "exit_code", "stdout", "stderr"} | COMMAND_IDENTITY,
        "source verify command",
    )
    source_args = source_capture["argv"]
    input_path = Path(run["argv"][2 if kind == "model_vectors" else 1])
    manifest_path = input_path.parent / "source-closure.json"
    if source_args != [
        tools["python"]["path"],
        str(source_root / "tools/ci/source_closure.py"),
        "capture",
        "--profile",
        "retrieval",
        "--out",
        str(manifest_path),
    ] or source_verify["argv"] != [
        tools["python"]["path"],
        str(source_root / "tools/ci/source_closure.py"),
        "verify",
        "--manifest",
        str(manifest_path),
    ]:
        raise ValueError("conditional source command differs from selected recipe")
    for phase, command in (("capture", source_capture), ("verify", source_verify)):
        if (
            type(command["exit_code"]) is not int
            or command["exit_code"] != 0
            or decode(command["stdout"])
            != f"source closure {phase} ok: retrieval {len(closure['files'])} files {closure['digest']}\n".encode()
        ):
            raise ValueError("conditional source command lacks successful bound terminal output")
        decode(command["stderr"])
        command_identity(command, context, source_root)
    command_identity(build, context, source_root)
    command_identity(run, context, source_root)
    if kind == "model_vectors":
        expected_models = {
            (
                "quanta",
                "model2vec:minishlab/potion-code-16M-v2",
                "e9d2a44ca6a05ac6685f3b23709ea57eb7352d5b:model2vec-rs-0.3.0:fancy-regex:full-length-v2",
            ),
            ("semble", "minishlab/potion-code-16M-v2", "e9d2a44ca6a05ac6685f3b23709ea57eb7352d5b"),
        }
        witnessed_models = set()
        for record in records:
            provenance = record.get("route_provenance")
            if not isinstance(provenance, dict) or not provenance:
                raise ValueError("conditional model proof lacks frozen route provenance")
            for route, route_capture in provenance.items():
                if route not in {"semantic", "hybrid"}:
                    continue
                route_capture = exact(route_capture, {"capture_id"}, "model route provenance")
                capture_id = route_capture["capture_id"]
                if not isinstance(capture_id, str) or capture_id not in record["captures"]:
                    raise ValueError("model route references a missing capture")
                capture = record["captures"][capture_id]
                witnessed_models.add(
                    (capture["system"], capture["model"], capture["model_revision"])
                )
        if witnessed_models != expected_models:
            raise ValueError("frozen pair does not use the proven model/encoder identities")
        expected_inputs = reference.INPUTS + [task["query"] for task in suite["tasks"]]
        if load(decode(context["inputs"])) != expected_inputs:
            raise ValueError("conditional vector inputs differ from frozen suite/adversarial set")
        ref_run = exact(
            context["reference_run"],
            {
                "argv",
                "exit_code",
                "stdout",
                "stderr",
                "output_sha256",
                "interpreter_sha256",
                "script_sha256",
            }
            | COMMAND_IDENTITY,
            "reference execution",
        )
        if (
            type(ref_run["exit_code"]) is not int
            or ref_run["exit_code"] != 0
            or ref_run["output_sha256"] != context["reference"]["sha256"]
            or ref_run["script_sha256"]
            != closed_files.get("tools/benchmark/retrieval/parity_reference.py")
            or not portable_proof._is_sha256(ref_run["interpreter_sha256"])
            or not isinstance(ref_run["argv"], list)
            or len(ref_run["argv"]) != 10
            or any(not isinstance(arg, str) for arg in ref_run["argv"])
            or Path(ref_run["argv"][1]).name != "parity_reference.py"
            or ref_run["argv"][2::2] != ["--model-dir", "--out", "--inputs-json", "--model-id"]
        ):
            raise ValueError("reference command/output binding differs")
        source_root = Path(build["argv"][0]).parent.parent
        if (
            ref_run["argv"][1] != str(source_root / "tools/benchmark/retrieval/parity_reference.py")
            or ref_run["argv"][3] != run["argv"][1]
            or ref_run["argv"][7] != run["argv"][2]
            or ref_run["argv"][9] != "minishlab/potion-code-16M-v2"
        ):
            raise ValueError("reference source/model/input command substitution")
        command_identity(ref_run, context, source_root)
        reference_epoch = context["tool_custody"]["epochs"].get(ref_run["argv"][0])
        if reference_epoch is None or reference_epoch["sha256"] != ref_run["interpreter_sha256"]:
            raise ValueError("conditional reference interpreter lacks selected invocation custody")
        decode(ref_run["stdout"])
        decode(ref_run["stderr"])
    else:
        if context["reference_run"] is not None:
            raise ValueError("incremental proof carries reference execution")
        if (
            Path(run["argv"][1]).name != "inputs.json"
            or Path(run["argv"][2]).name != "fresh-state"
            or Path(run["argv"][1]).parent != Path(run["argv"][2]).parent
        ):
            raise ValueError("incremental execution did not use frozen plan/fresh state")
        plan = load(decode(context["inputs"]))
        for case in plan["cases"]:
            contract = case["fresh"]["model_contract"]
            if ("quanta", contract["model_id"], contract["model_version"]) not in model_identities:
                raise ValueError("incremental plan model differs from frozen runner model")
    raw, passed = rederive(kind, context)
    count = len(raw["rows"])
    if (
        canonical(results["raw_proof"]) != canonical(raw)
        or any(
            type(results[key]) is not int for key in ("selected", "executed", "passed", "failed")
        )
        or (results["selected"], results["executed"], results["passed"], results["failed"])
        != (count, count, passed, count - passed)
        or results["status"] != ("pass" if passed == count else "fail")
    ):
        raise ValueError("conditional summary differs from independent raw replay")
    receipt = exact(
        results["execution_receipt"],
        {
            "schema_version",
            "command",
            "exit_code",
            "source_revision",
            "repository_commit",
            "model_sha256",
            "dependency_sha256",
            "runner_binary_sha256",
            "raw_sha256",
            "context_sha256",
        },
        "conditional execution receipt",
    )
    expected_receipt = {
        "schema_version": 2,
        "command": results["command"],
        "exit_code": 0,
        **identity,
        "runner_binary_sha256": context["binary_sha256"],
        "raw_sha256": sha(canonical(raw)),
        "context_sha256": sha(canonical(context)),
    }
    if (
        type(receipt["schema_version"]) is not int
        or type(receipt["exit_code"]) is not int
        or receipt != expected_receipt
    ):
        raise ValueError("conditional receipt differs from execution/raw identity")
    if verify_source:
        try:
            source_closure.verify_manifest(ROOT, closure)
        except source_closure.ClosureError as error:
            raise ValueError(str(error)) from error
    return results


def produce(args: argparse.Namespace) -> dict:
    with portable_proof.controlled_execution() as guard:
        result = _produce_controlled(args, guard)
    # Publish only after the shared context's terminal custody check succeeds.
    # Exclusive hard-link publication cannot expose a partially written JSON.
    out = args.out.resolve()
    pending = out / "results.pending.json"
    portable_proof._write_json(pending, result)
    os.link(pending, out / "results.json")
    return result


def _produce_controlled(args: argparse.Namespace, guard: tool_custody.ToolCustody) -> dict:
    kind = args.kind
    package, binary, features = RECIPES[kind]
    suite_bytes, corpus_bytes = args.suite.read_bytes(), args.corpus.read_bytes()
    suite = load(suite_bytes)
    load(corpus_bytes)
    records = sorted(
        [load(path.read_bytes()) for path in args.records],
        key=lambda record: sha(canonical(record)),
    )
    models = frozen_model_identities(records)
    out = args.out.resolve()
    out.mkdir(exist_ok=False)
    tools = guard.tools()
    commands = []
    manifest_path = out / "source-closure.json"
    capture_args = [
        tools["python"]["path"],
        str(ROOT / "tools/ci/source_closure.py"),
        "capture",
        "--profile",
        "retrieval",
        "--out",
        str(manifest_path),
    ]
    source_stdout = portable_proof._run("source-capture", capture_args, out, commands)
    source_capture = _command_frame(commands[-1], out, source_stdout)
    manifest_bytes = _read_repo_regular_bytes(
        out, manifest_path.name, label="conditional source manifest"
    )
    closure = source_closure.validate_manifest_shape(load(manifest_bytes))
    identity = {
        "source_revision": closure["revision"],
        "repository_commit": suite["repository_commit"],
        "model_sha256": sha(canonical(models)),
        "dependency_sha256": sha(args.semble_lockfile.read_bytes()),
    }
    build_args = [
        tools["cargow"]["path"],
        "--lane",
        "test-daemon-lane",
        "build",
        "-p",
        package,
        "--bin",
        binary,
        *features,
        "--locked",
        "--message-format=json",
    ]
    build_stdout = portable_proof._run("build", build_args, out, commands)
    build_frame = _command_frame(commands[-1], out, build_stdout)
    events = [load(line) for line in build_stdout.splitlines() if line.strip()]
    binaries = [
        event["executable"]
        for event in events
        if event.get("reason") == "compiler-artifact"
        and event.get("target", {}).get("name") == binary
        and event.get("executable")
    ]
    if len(binaries) != 1:
        raise ValueError("build did not identify exactly one proof binary")
    executable = Path(binaries[0])
    executable_sha = guard.bind_executable(executable)["sha256"]
    if kind == "model_vectors":
        inputs = reference.INPUTS + [task["query"] for task in suite["tasks"]]
    else:
        if args.plan is None:
            raise ValueError("incremental proof needs --plan")
        inputs = load(args.plan.read_bytes())
    input_bytes = canonical(inputs)
    portable_proof._write(out / "inputs.json", input_bytes)
    reference_bytes = None
    ref_record = None
    if kind == "model_vectors":
        if args.model_dir is None or args.reference_python is None:
            raise ValueError("vector proof needs --model-dir and --reference-python")
        ref_args = [
            str(args.reference_python.absolute()),
            str(ROOT / "tools/benchmark/retrieval/parity_reference.py"),
            "--model-dir",
            str(args.model_dir.resolve()),
            "--out",
            str(out / "reference.json"),
            "--inputs-json",
            str(out / "inputs.json"),
            "--model-id",
            "minishlab/potion-code-16M-v2",
        ]
        interpreter_sha = guard.bind_executable(args.reference_python.absolute())["sha256"]
        ref_stdout = portable_proof._run(
            "reference", ref_args, out, commands, expected_executable_sha256=interpreter_sha
        )
        reference_bytes = _read_repo_regular_bytes(
            out, "reference.json", label="conditional reference output"
        )
        ref_record = {
            **_command_frame(commands[-1], out, ref_stdout),
            "output_sha256": sha(reference_bytes),
            "interpreter_sha256": interpreter_sha,
            "script_sha256": next(
                entry["sha256"]
                for entry in closure["files"]
                if entry["path"] == "tools/benchmark/retrieval/parity_reference.py"
            ),
        }
        argv = [str(executable), str(args.model_dir.resolve()), str(out / "inputs.json")]
    else:
        argv = [str(executable), str(out / "inputs.json"), str(out / "fresh-state")]
    observed = portable_proof._run(
        "run", argv, out, commands, expected_executable_sha256=executable_sha
    )
    run_frame = _command_frame(commands[-1], out, observed)
    if kind == "model_vectors":
        for name, expected in reference.PINNED_ASSET_SHA256.items():
            if sha((args.model_dir / name).read_bytes()) != expected:
                raise ValueError("pinned model asset changed during execution")
    verify_args = [
        tools["python"]["path"],
        str(ROOT / "tools/ci/source_closure.py"),
        "verify",
        "--manifest",
        str(manifest_path),
    ]
    verify_stdout = portable_proof._run("source-verify", verify_args, out, commands)
    source_verify = _command_frame(commands[-1], out, verify_stdout)
    if (
        _read_repo_regular_bytes(out, manifest_path.name, label="conditional source manifest")
        != manifest_bytes
    ):
        raise ValueError("conditional source manifest changed between capture and verification")
    guard.check()
    context = {
        "source_closure": closure,
        "cargo_lock_sha256": sha((ROOT / "Cargo.lock").read_bytes()),
        "uv_lock_sha256": sha((ROOT / "uv.lock").read_bytes()),
        "suite": artifact(suite_bytes),
        "corpus": artifact(corpus_bytes),
        "records": artifact(canonical(records)),
        "semble_lockfile": artifact(args.semble_lockfile.read_bytes()),
        "inputs": artifact(input_bytes),
        "reference": artifact(reference_bytes) if reference_bytes is not None else None,
        "observed": artifact(observed),
        "binary_sha256": executable_sha,
        "build": build_frame,
        "run": {**run_frame, "executable_sha256": executable_sha},
        "reference_run": ref_record,
        "environment": {
            **portable_proof._os_identity(),
            "relevant": portable_proof._relevant_environment(dict(os.environ)),
        },
        "tool_custody": guard.record(),
        "source_manifest": artifact(manifest_bytes),
        "source_capture": source_capture,
        "source_verify": source_verify,
    }
    raw, passed = rederive(kind, context)
    count = len(raw["rows"])
    result = {
        "schema_version": 2,
        "command": f"retrieval-conditional-proof-v2:{kind}",
        "status": "pass" if passed == count else "fail",
        "selected": count,
        "executed": count,
        "passed": passed,
        "failed": count - passed,
        "identity": identity,
        "raw_proof": raw,
        "execution_context": context,
    }
    result["execution_receipt"] = {
        "schema_version": 2,
        "command": result["command"],
        "exit_code": 0,
        **identity,
        "runner_binary_sha256": executable_sha,
        "raw_sha256": sha(canonical(raw)),
        "context_sha256": sha(canonical(context)),
    }
    # The guarded source-verify child already checked current source using the
    # selected Git/Cargo. Do not rerun its subprocesses in this ambient parent.
    validate_results(result, kind)
    return result


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--kind", required=True, choices=RECIPES)
    for name in ("suite", "corpus", "semble-lockfile", "out"):
        parser.add_argument(f"--{name}", required=True, type=Path)
    parser.add_argument("--records", required=True, type=Path, nargs="+")
    parser.add_argument("--plan", type=Path)
    parser.add_argument("--model-dir", type=Path)
    parser.add_argument("--reference-python", type=Path)
    args = parser.parse_args()
    try:
        result = produce(args)
    except (ValueError, OSError, KeyError, TypeError, source_closure.ClosureError) as error:
        parser.exit(2, f"conditional proof refused: {error}\n")
    return 0 if result["status"] == "pass" else 1


if __name__ == "__main__":
    raise SystemExit(main())
