"""Strict OpenGrok query-bound reader witness parser.

The header is emitted from SearchEngine's acquired searcher before destroy.
This attests only the projects selected by that particular search request.
"""

from __future__ import annotations

import hashlib
import re

from tools.benchmark.evidence import canonical_json

HEADER = "X-QI-Reader-Witness"
REQUEST_HEADER = "X-QI-Request-Nonce"
CONTRACT = "qi-opengrok-1.14.18-searchcontroller-query-reader-v1"
ORIGINAL_SOURCE_SHA256 = "e7880011219a11e8b2e133c8c8b6cc678b9d8b39cc4fbb7e98836c4631c55654"
PATCHED_SOURCE_SHA256 = "21b0cb48cd58553a2d4657b1434ee5fb5228dc630f30de98ccaddd813b560f27"
SOURCE_PATCH_SHA256 = "8e6b4cdb7d11fad1fe864eb69f4a5647fa04482f77059b703fba1be2e013ca98"
COMPILER_IMAGE_SHA256 = "1b79b7700154fec76b32816c560b1d67f30e115868fc8caf5c123207ae6074e7"
CLASS_FILES = frozenset(
    "WEB-INF/classes/org/opengrok/web/api/v1/controller/" + name
    for name in (
        "SearchController.class",
        "SearchController$SearchEngineWrapper.class",
        "SearchController$SearchResult.class",
        "SearchController$SearchHit.class",
    )
)
# Compiled from PATCHED_SOURCE_SHA256 with the pinned image javac and image WAR classpath.
PINNED_CLASS_SHA256: dict[str, str] = {
    "WEB-INF/classes/org/opengrok/web/api/v1/controller/SearchController$SearchEngineWrapper.class": "e477af140507338e72c22eb0753720a1f63c89aa7811a7fbea2fdb67259c1335",
    "WEB-INF/classes/org/opengrok/web/api/v1/controller/SearchController$SearchHit.class": "f8945465238c3ed984f51edb987339ab85ed615ee0e7f375ac600c4780f16c65",
    "WEB-INF/classes/org/opengrok/web/api/v1/controller/SearchController$SearchResult.class": "8db685df4821fba6f3fb0cc218b78ca7fcc638fb5c72fc4845f1f42aee80ac82",
    "WEB-INF/classes/org/opengrok/web/api/v1/controller/SearchController.class": "c492587703599e7203d17872326fe8d0af694d2547b801dbe7195a4036b9c372"
}
MAX_HEADER_BYTES = 8192
PROJECT = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]*\Z")
SEGMENTS = re.compile(r"segments_[0-9a-z]+\Z")
DIGEST = re.compile(r"[0-9a-f]{64}\Z")
NUMBER = re.compile(r"(?:0|[1-9][0-9]*)\Z")
COMMIT_KEYS = {
    "segmentsFile", "generation", "readerVersion", "numDocs", "maxDoc", "fileNamesSha256"
}


def _strict_commit(value: object) -> dict:
    """Validate the native commit independently of Python numeric equality aliases."""
    if type(value) is not dict or set(value) != COMMIT_KEYS:
        raise ValueError("OpenGrok native reader commit shape differs")
    segments = value["segmentsFile"]
    digest = value["fileNamesSha256"]
    if (
        type(segments) is not str
        or SEGMENTS.fullmatch(segments) is None
        or len(segments) > len("segments_") + 13
        or type(digest) is not str
        or DIGEST.fullmatch(digest) is None
        or any(type(value[key]) is not int or not 0 <= value[key] < 2**63
               for key in ("generation", "readerVersion"))
        or any(type(value[key]) is not int or not 0 <= value[key] <= 4 * 1024 * 1024
               for key in ("numDocs", "maxDoc"))
        or value["numDocs"] > value["maxDoc"]
        or int(segments[len("segments_"):], 36) != value["generation"]
    ):
        raise ValueError("OpenGrok native reader commit field differs")
    return value


def strictly_equal(actual: object, expected: object) -> bool:
    """Compare JSON-like evidence without bool/int or integral-float aliases."""
    if type(actual) is not type(expected):
        return False
    if type(expected) is dict:
        return set(actual) == set(expected) and all(
            strictly_equal(actual[key], value) for key, value in expected.items()
        )
    if type(expected) is list:
        return len(actual) == len(expected) and all(
            strictly_equal(left, right) for left, right in zip(actual, expected, strict=True)
        )
    return actual == expected


def canonical_typed_equal(actual: object, expected: object) -> bool:
    return strictly_equal(actual, expected) and canonical_json(actual) == canonical_json(expected)


def request_nonce(output_root: str, config: dict, task: dict) -> str:
    """Stable per-capture/task nonce; replay derives it from frozen inputs."""
    binding = {
        "contract": "qi-opengrok-query-reader-v1",
        "output_root": output_root,
        "container_id": config["backend_snapshot"]["container_id"],
        "project": config["project"],
        "task_id": task["task_id"],
        "query": task["query"],
    }
    return hashlib.sha256(canonical_json(binding).encode("utf-8")).hexdigest()[:32]


def verify_header(
    values: list[str] | None, *, nonce: str, project: str, native_commits: dict
) -> dict:
    """Require the selected project's exact native commit on this response."""
    if (
        type(values) is not list
        or len(values) != 1
        or type(values[0]) is not str
        or not 0 < len(values[0]) <= MAX_HEADER_BYTES
        or not values[0].isascii()
        or type(project) is not str
        or PROJECT.fullmatch(project) is None
        or type(nonce) is not str
        or re.fullmatch(r"[0-9a-f]{32}", nonce) is None
    ):
        raise ValueError("OpenGrok query reader header is missing, repeated or malformed")
    raw = values[0]
    pieces = raw.split(":")
    if len(pieces) != 3 or pieces[:2] != ["v1", nonce]:
        raise ValueError("OpenGrok query reader nonce or project count differs")
    fields = pieces[2].split(",")
    if len(fields) != 7 or fields[0] != project:
        raise ValueError("OpenGrok query reader project differs")
    _, segments, generation, version, live, maximum, file_digest = fields
    if (
        SEGMENTS.fullmatch(segments) is None
        or any(NUMBER.fullmatch(value) is None or len(value) > 19
               for value in (generation, version, live, maximum))
        or DIGEST.fullmatch(file_digest) is None
    ):
        raise ValueError("OpenGrok query reader commit field is malformed")
    commit = {
        "segmentsFile": segments,
        "generation": int(generation),
        "readerVersion": int(version),
        "numDocs": int(live),
        "maxDoc": int(maximum),
        "fileNamesSha256": file_digest,
    }
    if type(native_commits) is not dict or set(native_commits) != {project}:
        raise ValueError("OpenGrok native reader project scope differs")
    native_commit = _strict_commit(native_commits[project])
    _strict_commit(commit)
    if not strictly_equal(commit, native_commit):
        raise ValueError("OpenGrok query reader differs from native disk commit")
    return {
        "scope": "selected_query_project_loaded_reader",
        "project": project,
        "commit": commit,
        "nonce": nonce,
        "header_sha256": hashlib.sha256(raw.encode("ascii")).hexdigest(),
    }
