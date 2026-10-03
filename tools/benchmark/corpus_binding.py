"""Bind frozen query inputs to one Git-derived external corpus view.

A retained release capsule replays real Git objects without mutable source
paths. Binding input universes is not independent product index attestation,
gold adjudication, rank equivalence, or performance qualification.
"""

from __future__ import annotations

import hashlib
import re
import shutil
import sys
import tempfile
import unicodedata
from collections import defaultdict
from importlib.metadata import PackageNotFoundError, version
from pathlib import Path

try:
    import tomllib
except ModuleNotFoundError:  # Python 3.10 support
    import tomli as tomllib

import corpus_release as corpus
from evidence import (
    CONTROL_DOCUMENT_BYTES,
    EvidenceError,
    RawFile,
    _read_control_file,
    _read_regular_file,
    canonical_json,
    digest_bytes,
    parse_json,
)
from raw_archive import ArchiveLimits
from raw_archive import pack as pack_archive
from raw_archive import unpack as unpack_archive

from tools.benchmark.retrieval.retrieval_contract import canonical

MAX_CAPSULE_BYTES = 256 * 1024 * 1024
GOLD_DOCUMENT_BYTES = 32 * 1024 * 1024
MAX_SPLIT_REPOSITORIES = 64
MAX_SPLIT_FAMILIES = 100_000
GOLD_RUNTIME_SOURCE_ROOT = Path(__file__).resolve().parents[2]
GOLD_RUNTIME_PACKAGES = (
    "regex",
    "tree-sitter",
    "tree-sitter-language-pack",
    "unicodedata2",
)
# Cross-split source leakage is checked over code_only bytes. Exact copies are
# refused from a byte floor that skips empty package stubs; near duplicates use
# winnowed token k-gram fingerprints (Schleimer et al., SIGMOD 2003) and refuse
# when either file's informative fingerprints are mostly contained in the other.
# Fingerprints present in more files than the frequency bound are treated as
# shared boilerplate (license headers, imports) and dropped on both sides.
SPLIT_LEAKAGE_POLICY = {
    "id": "repository-disjoint-code-only-v1",
    "view": "code_only",
    "split_sides": ["development", "holdout"],
    "repository_identity": "url_casefold_without_dot_git_and_revision_unique",
    "query_families": "globally_unique_one_repository",
    "exact_file_min_bytes": 256,
    "near_duplicate": {
        "tokenizer": "ascii_word_or_single_symbol_v1",
        "kgram_tokens": 8,
        "winnow_window": 4,
        "hash": "sha256_first_8_bytes_big_endian",
        "min_informative_fingerprints": 16,
        "max_document_frequency": 64,
        "containment_threshold": 0.8,
    },
}
_LEAK_TOKENS = re.compile(rb"[A-Za-z0-9_]+|[^\sA-Za-z0-9_]")


def _json(raw: bytes) -> object:
    if not isinstance(raw, bytes):
        raise EvidenceError("corpus binding JSON requires captured bytes")
    try:
        return parse_json(raw.decode("utf-8"))
    except (UnicodeError, ValueError) as error:
        raise EvidenceError(f"invalid corpus binding JSON: {error}") from error


def _selection(selection: object) -> dict:
    if (
        not isinstance(selection, dict)
        or set(selection) != {"release_path", "release_digest", "repository", "view"}
        or selection["view"] not in corpus.VIEWS
        or not isinstance(selection["repository"], str)
        or not selection["repository"]
        or not isinstance(selection["release_digest"], str)
        or not selection["release_digest"].startswith("sha256:")
        or len(selection["release_digest"]) != 71
        or any(c not in "0123456789abcdef" for c in selection["release_digest"][7:])
    ):
        raise EvidenceError("corpus selection is malformed")
    path = selection["release_path"]
    if (
        not isinstance(path, str)
        or not path
        or "\x00" in path
        or "\\" in path
        or not Path(path).is_absolute()
        or ".." in Path(path).parts
        or Path(path).as_posix() != path
    ):
        raise EvidenceError("corpus release path must be canonical absolute")
    return selection


def read_spec(raw: bytes, roles: tuple[str, ...]) -> tuple[dict, dict[str, Path]]:
    value = _json(raw)
    if (
        not isinstance(value, dict)
        or set(value) != {"schema_version", "corpus", "inputs"}
        or type(value["schema_version"]) is not int
        or value["schema_version"] != 2
    ):
        raise EvidenceError("common lexical spec requires schema_version 2 and corpus binding")
    selection = _selection(value["corpus"])
    inputs = value["inputs"]
    if not isinstance(inputs, dict) or set(inputs) != set(roles):
        raise EvidenceError("common lexical input role inventory differs")
    for value in (selection["release_path"], *inputs.values()):
        if (
            not isinstance(value, str)
            or not value
            or "\x00" in value
            or "\\" in value
            or not Path(value).is_absolute()
            or ".." in Path(value).parts
            or Path(value).as_posix() != value
        ):
            raise EvidenceError("corpus/input paths must be canonical absolute paths")
    return selection, {role: Path(inputs[role]) for role in roles}


def _bind(
    document: dict, manifest_raw: bytes, selection: dict, suite_raw: bytes, pack_raw: bytes
) -> dict:
    """Use a validated/reconstructed release, not caller-asserted index state."""
    if document["digest"] != selection["release_digest"]:
        raise EvidenceError("selected release digest differs")
    repositories = [
        row for row in document["repositories"] if row["recipe"]["name"] == selection["repository"]
    ]
    if len(repositories) != 1 or selection["view"] not in corpus.VIEWS:
        raise EvidenceError("selected repository/view is absent or ambiguous")
    repository = repositories[0]
    view = repository["views"][selection["view"]]
    if digest_bytes(manifest_raw) != view["manifest_digest"]:
        raise EvidenceError("selected corpus view manifest differs")
    manifest = _json(manifest_raw)
    suite = _json(suite_raw)
    pack = _json(pack_raw)
    if not isinstance(suite, dict) or not isinstance(pack, dict):
        raise EvidenceError("suite/query pack must be objects")
    commit = repository["recipe"]["revision"]
    universe = manifest["files"]
    universe_digest = hashlib.sha256(canonical(universe)).hexdigest()
    if "sha256:" + universe_digest != view["file_universe_digest"]:
        raise EvidenceError("selected view universe digest differs from native contract")
    for payload in (suite, pack):
        if (
            payload.get("repository_commit") != commit
            or payload.get("file_universe") != universe
            or payload.get("file_universe_digest") != universe_digest
        ):
            raise EvidenceError("suite/query universe differs from selected release view")
    if pack.get("suite_commitment_sha256") != hashlib.sha256(canonical(suite)).hexdigest():
        raise EvidenceError("query pack does not bind its suite")
    paths = {entry["path"] for entry in universe}
    if not isinstance(suite.get("tasks"), list) or not suite["tasks"]:
        raise EvidenceError("bound suite has no task inventory")
    for task in suite["tasks"]:
        if not isinstance(task, dict) or not isinstance(task.get("gold"), list):
            raise EvidenceError("bound suite task/gold inventory is malformed")
        if any(
            not isinstance(label, dict) or label.get("path") not in paths for label in task["gold"]
        ):
            raise EvidenceError("gold path is outside the selected corpus view")
    return {
        "schema_version": 1,
        "kind": "corpus_view_query_binding",
        "release_digest": document["digest"],
        "repository": selection["repository"],
        "view": selection["view"],
        "repository_commit": commit,
        "manifest_digest": view["manifest_digest"],
        "file_universe_digest": view["file_universe_digest"],
        "suite_digest": digest_bytes(suite_raw),
        "query_pack_digest": digest_bytes(pack_raw),
        "index_universe_attested": False,
        "binding_owner_digest": digest_bytes(_read_regular_file(Path(__file__))),
    }


def capture(
    root: Path, selection: dict, suite: bytes, pack: bytes, target: Path
) -> tuple[dict, RawFile]:
    _selection(selection)
    if root.absolute() != Path(selection["release_path"]):
        raise EvidenceError("corpus capture path differs from original selection")
    document = corpus.validate(root)
    repository = next(
        (
            row
            for row in document["repositories"]
            if row["recipe"]["name"] == selection["repository"]
        ),
        None,
    )
    if repository is None or selection["view"] not in corpus.VIEWS:
        raise EvidenceError("selected repository/view is absent")
    manifest = _read_regular_file(root / repository["views"][selection["view"]]["manifest"])
    binding = _bind(document, manifest, selection, suite, pack)
    names = [
        "release.json",
        "recipe.json",
        *(row["bundle"]["path"] for row in document["repositories"]),
    ]
    if len(names) != len(set(names)):
        raise EvidenceError("release capsule bundle inventory is duplicate")
    if sum((root / name).stat().st_size for name in names) > MAX_CAPSULE_BYTES:
        raise EvidenceError("release capsule exceeds explicit 256 MiB custody limit")
    capsule = pack_archive(
        {name: RawFile.capture(root / name) for name in names},
        target,
        limits=ArchiveLimits(MAX_CAPSULE_BYTES),
    )
    if replay(capsule, selection, suite, pack) != binding:
        raise EvidenceError("corpus release changed during capsule freeze")
    return binding, capsule


def replay(capsule: RawFile, selection: dict, suite: bytes, pack: bytes) -> dict:
    """Reconstruct the release from retained bundles, never original paths."""
    if not isinstance(capsule, RawFile) or not 0 < capsule.size <= MAX_CAPSULE_BYTES:
        raise EvidenceError("release capsule is empty or exceeds custody limit")
    _selection(selection)
    try:
        return _replay(capsule, selection, suite, pack)
    except (OSError, ValueError, RuntimeError) as error:
        raise EvidenceError(f"invalid corpus release capsule: {error}") from error


def _replay(capsule: RawFile, selection: dict, suite: bytes, pack: bytes) -> dict:
    with tempfile.TemporaryDirectory(prefix="quanta-corpus-binding-") as temporary:
        scratch = Path(temporary).resolve()
        held = scratch / "held"
        held.mkdir()
        names = []

        def admit(observed):
            if len(observed) < 3:
                raise EvidenceError("release capsule inventory is partial")
            for name in observed:
                corpus.canonical_path(name)
                parts = Path(name).parts
                if name not in {"release.json", "recipe.json"} and (
                    len(parts) != 2 or parts[0] != "bundles" or not parts[1].endswith(".bundle")
                ):
                    raise EvidenceError("release capsule contains an unexpected artifact")
            names.extend(observed)

        unpack_archive(capsule, held, limits=ArchiveLimits(MAX_CAPSULE_BYTES), admit_names=admit)
        document = _json(_read_control_file(held / "release.json"))
        spec = _json(_read_control_file(held / "recipe.json"))
        entries = corpus.validate_spec(spec)
        wanted = {
            "release.json",
            "recipe.json",
            *(f"bundles/{entry['name']}.bundle" for entry in entries),
        }
        if set(names) != wanted:
            raise EvidenceError("release capsule repository bundle inventory differs")
        checkouts = scratch / "checkouts"
        checkouts.mkdir()
        for entry in entries:
            corpus.git(
                checkouts,
                "clone",
                "--quiet",
                "--",
                str(held / f"bundles/{entry['name']}.bundle"),
                str(checkouts / entry["name"]),
            )
        restored = scratch / "release"
        expected = corpus.build(spec, checkouts, restored, bundles=held / "bundles")
        if expected != document:
            raise EvidenceError("retained release metadata differs from real Git objects")
        repository = next(
            (
                row
                for row in expected["repositories"]
                if row["recipe"]["name"] == selection["repository"]
            ),
            None,
        )
        if repository is None or selection["view"] not in corpus.VIEWS:
            raise EvidenceError("selected repository/view is absent")
        manifest = _read_regular_file(restored / repository["views"][selection["view"]]["manifest"])
        return _bind(expected, manifest, selection, suite, pack)


def _fingerprints(raw: bytes) -> set[int]:
    """Winnowed token k-gram hashes; whitespace and layout do not affect identity."""
    policy = SPLIT_LEAKAGE_POLICY["near_duplicate"]
    tokens = _LEAK_TOKENS.findall(raw)
    k, window = policy["kgram_tokens"], policy["winnow_window"]
    hashes = [
        int.from_bytes(hashlib.sha256(b"\0".join(tokens[i : i + k])).digest()[:8], "big")
        for i in range(len(tokens) - k + 1)
    ]
    if len(hashes) <= window:
        return set(hashes)
    return {min(hashes[i : i + window]) for i in range(len(hashes) - window + 1)}


def _canonical_url(url: str) -> str:
    url = url.casefold().rstrip("/")
    return url[:-4] if url.endswith(".git") else url


def _split_entry(entry: object) -> dict:
    keys = {
        "release_digest",
        "repository",
        "repository_commit",
        "code_only_universe_digest",
        "split",
        "query_family_ids",
    }
    if (
        not isinstance(entry, dict)
        or set(entry) != keys
        or entry["split"] not in SPLIT_LEAKAGE_POLICY["split_sides"]
        or any(
            not isinstance(entry[key], str) or not entry[key]
            for key in ("release_digest", "repository", "repository_commit")
        )
        or not isinstance(entry["code_only_universe_digest"], str)
        or not isinstance(entry["query_family_ids"], list)
        or entry["query_family_ids"] != sorted(set(entry["query_family_ids"]))
        or any(
            not isinstance(family, str) or re.fullmatch(r"[A-Za-z][A-Za-z0-9_.-]*", family) is None
            for family in entry["query_family_ids"]
        )
    ):
        raise EvidenceError("split manifest repository entry is malformed")
    return entry


def validate_split_manifest(raw: bytes, releases: dict[str, Path]) -> dict:
    return _validated_split_manifest(raw, releases)[0]


def _validated_split_manifest(
    raw: bytes, releases: dict[str, Path]
) -> tuple[dict, dict[str, dict]]:
    """Prove a corpus-wide repository-disjoint split against validated releases.

    Every repository of every bound release is assigned exactly once. Query
    families are globally unique, a repository (URL or revision) never appears
    twice, and no development/holdout code_only file pair is an exact or near
    copy. This is not evaluator experiment custody and admits no labels.
    """
    manifest = _json(raw)
    if (
        not isinstance(manifest, dict)
        or set(manifest) != {"schema_version", "kind", "leakage_policy", "repositories"}
        or type(manifest["schema_version"]) is not int
        or manifest["schema_version"] != 1
        or manifest["kind"] != "repository_disjoint_split_manifest"
        or manifest["leakage_policy"] != SPLIT_LEAKAGE_POLICY
        or not isinstance(manifest["repositories"], list)
        or not 2 <= len(manifest["repositories"]) <= MAX_SPLIT_REPOSITORIES
    ):
        raise EvidenceError("split manifest identity, policy or repository inventory is invalid")
    entries = [_split_entry(entry) for entry in manifest["repositories"]]
    if [(e["release_digest"], e["repository"]) for e in entries] != sorted(
        {(e["release_digest"], e["repository"]) for e in entries}
    ):
        raise EvidenceError("split manifest repositories are duplicate or unsorted")
    if {entry["split"] for entry in entries} != set(SPLIT_LEAKAGE_POLICY["split_sides"]):
        raise EvidenceError("split manifest requires development and holdout repositories")
    if (
        not isinstance(releases, dict)
        or set(releases) != {entry["release_digest"] for entry in entries}
        or any(not isinstance(path, Path) or not path.is_absolute() for path in releases.values())
    ):
        raise EvidenceError("split manifest release paths differ from its release inventory")
    families: set[str] = set()
    urls: set[str] = set()
    commits: set[str] = set()
    sources: dict[str, list[tuple[str, str, bytes | None, set[int]]]] = {
        side: [] for side in SPLIT_LEAKAGE_POLICY["split_sides"]
    }
    documents: dict[str, dict] = {}
    for digest, root in sorted(releases.items()):
        document = corpus.validate(root)
        if document["digest"] != digest:
            raise EvidenceError("split manifest binds a stale or different release digest")
        documents[digest] = document
        assigned = {e["repository"]: e for e in entries if e["release_digest"] == digest}
        if set(assigned) != {row["recipe"]["name"] for row in document["repositories"]}:
            raise EvidenceError("split manifest omits or invents a release repository")
        for repository in document["repositories"]:
            entry = assigned[repository["recipe"]["name"]]
            view = repository["views"][SPLIT_LEAKAGE_POLICY["view"]]
            url = _canonical_url(repository["recipe"]["url"])
            if (
                entry["repository_commit"] != repository["recipe"]["revision"]
                or entry["code_only_universe_digest"] != view["file_universe_digest"]
            ):
                raise EvidenceError("split manifest repository commit or source differs")
            if url in urls or entry["repository_commit"] in commits:
                raise EvidenceError("split manifest repeats one upstream repository")
            urls.add(url)
            commits.add(entry["repository_commit"])
            repeated = families & set(entry["query_family_ids"])
            families.update(entry["query_family_ids"])
            if repeated or len(families) > MAX_SPLIT_FAMILIES:
                raise EvidenceError("split manifest repeats a query family across repositories")
            files = _json(_read_control_file(root / view["manifest"]))["files"]
            base = root / "views" / repository["recipe"]["name"] / SPLIT_LEAKAGE_POLICY["view"]
            for row in files:
                raw = _read_regular_file(base / row["path"])
                if digest_bytes(raw) != "sha256:" + row["file_sha256"]:
                    raise EvidenceError("split manifest source differs from release view")
                exact = (
                    hashlib.sha256(raw).digest()
                    if len(raw) >= SPLIT_LEAKAGE_POLICY["exact_file_min_bytes"]
                    else None
                )
                sources[entry["split"]].append(
                    (entry["repository"], row["path"], exact, _fingerprints(raw))
                )
    _refuse_cross_split_source(sources)
    return manifest, documents


def _refuse_cross_split_source(
    sources: dict[str, list[tuple[str, str, bytes | None, set[int]]]],
) -> None:
    development, holdout = (sources[side] for side in SPLIT_LEAKAGE_POLICY["split_sides"])
    exact = {digest: (name, path) for name, path, digest, _ in development if digest}
    for name, path, digest, _values in holdout:
        if digest and (match := exact.get(digest)):
            raise EvidenceError(
                f"split leakage: identical source {match[0]}:{match[1]} / {name}:{path}"
            )
    near = SPLIT_LEAKAGE_POLICY["near_duplicate"]
    frequency: dict[int, int] = defaultdict(int)
    for rows in sources.values():
        for *_identity, values in rows:
            for value in values:
                frequency[value] += 1
    common = {value for value, count in frequency.items() if count > near["max_document_frequency"]}
    informative = {
        side: [(name, path, values - common) for name, path, _digest, values in rows]
        for side, rows in sources.items()
    }
    posting: dict[int, list[int]] = defaultdict(list)
    left = [
        row
        for row in informative["development"]
        if len(row[2]) >= near["min_informative_fingerprints"]
    ]
    for index, (_name, _path, values) in enumerate(left):
        for value in values:
            posting[value].append(index)
    for name, path, values in informative["holdout"]:
        if len(values) < near["min_informative_fingerprints"]:
            continue
        shared: dict[int, int] = defaultdict(int)
        for value in values:
            for index in posting.get(value, ()):
                shared[index] += 1
        for index, count in shared.items():
            other = left[index]
            if count / min(len(values), len(other[2])) >= near["containment_threshold"]:
                raise EvidenceError(
                    f"split leakage: near-duplicate source {other[0]}:{other[1]} / {name}:{path}"
                )


def _split_releases(value: object) -> dict[str, Path]:
    if not isinstance(value, dict) or not value:
        raise EvidenceError("split release paths are malformed")
    releases = {}
    for digest, path in value.items():
        _selection(
            {"release_path": path, "release_digest": digest, "repository": "x", "view": "code_only"}
        )
        releases[digest] = Path(path)
    return releases


def _split_binding(
    recipe: dict,
    selection: dict,
    split: tuple[bytes, dict[str, Path]] | None,
    validated_manifest: dict | None = None,
) -> tuple[dict | None, dict[str, bytes]]:
    """v2 recipes are admitted only through a verified corpus-wide split manifest."""
    if recipe["schema_version"] == 1:
        if split is not None:
            raise EvidenceError("gold recipe v1 carries its own splits; no split manifest allowed")
        return None, {}
    if split is None:
        raise EvidenceError("gold recipe v2 requires its corpus-wide split manifest")
    manifest_raw, releases = split
    if hashlib.sha256(manifest_raw).hexdigest() != recipe["split_manifest_sha256"]:
        raise EvidenceError("gold recipe split-manifest SHA-256 differs")
    if releases.get(selection["release_digest"]) != Path(selection["release_path"]):
        raise EvidenceError("split manifest does not bind the selected release path")
    manifest = (
        validated_manifest
        if validated_manifest is not None
        else validate_split_manifest(manifest_raw, releases)
    )
    entry = next(
        (
            row
            for row in manifest["repositories"]
            if (row["release_digest"], row["repository"])
            == (selection["release_digest"], selection["repository"])
        ),
        None,
    )
    families = sorted({task["query_family_id"] for task in recipe["tasks"]})
    if entry is None or entry["split"] != recipe["split"]:
        raise EvidenceError("split manifest assigns the selected repository to another split")
    if entry["query_family_ids"] != families:
        raise EvidenceError("gold recipe query families differ from the split manifest")
    paths = {digest: str(path) for digest, path in sorted(releases.items())}
    return (
        {
            "split": recipe["split"],
            "split_manifest_sha256": recipe["split_manifest_sha256"],
            "leakage_policy_id": SPLIT_LEAKAGE_POLICY["id"],
            "release_digests": sorted(releases),
        },
        {
            "split-manifest.json": manifest_raw,
            "split-releases.json": canonical_json(paths).encode() + b"\n",
        },
    )


def _gold_producer_source_digests() -> dict[str, str]:
    """Bind the direct Python owners of mechanical gold and census admission."""
    from tools.benchmark.retrieval import (
        declaration_census_audit,
        declaration_parsers,
        gold_oracle,
        source_oracle,
    )

    owners = {
        "corpus_binding": Path(__file__),
        "declaration_census_audit": Path(declaration_census_audit.__file__),
        "declaration_parsers": Path(declaration_parsers.__file__),
        "gold_oracle": Path(gold_oracle.__file__),
        "source_oracle": Path(source_oracle.__file__),
    }
    return {
        **{name: digest_bytes(_read_regular_file(path)) for name, path in sorted(owners.items())},
        "pyproject_toml": digest_bytes(
            _read_regular_file(GOLD_RUNTIME_SOURCE_ROOT / "pyproject.toml")
        ),
        "uv_lock": digest_bytes(_read_regular_file(GOLD_RUNTIME_SOURCE_ROOT / "uv.lock")),
        **{
            name: "sha256:" + digest
            for name, digest in declaration_parsers.component_source_digests().items()
        },
    }


def require_gold_runtime() -> dict[str, str]:
    """Fail before source replay if the active parser differs from source pins."""
    try:
        project = tomllib.loads(
            _read_regular_file(GOLD_RUNTIME_SOURCE_ROOT / "pyproject.toml").decode("utf-8")
        )
        locked = tomllib.loads(
            _read_regular_file(GOLD_RUNTIME_SOURCE_ROOT / "uv.lock").decode("utf-8")
        )
    except (OSError, UnicodeError, ValueError) as error:
        raise EvidenceError("gold runtime source pins are unreadable or malformed") from error
    dependencies = project.get("project", {}).get("dependencies")
    optional = project.get("project", {}).get("optional-dependencies", {}).get("dev")
    packages = locked.get("package")
    if (
        not isinstance(dependencies, list)
        or not isinstance(optional, list)
        or not isinstance(packages, list)
    ):
        raise EvidenceError("gold runtime source pins are malformed")
    requirements = dependencies + optional
    locked_versions: dict[str, list[str]] = {name: [] for name in GOLD_RUNTIME_PACKAGES}
    for package in packages:
        if isinstance(package, dict) and package.get("name") in locked_versions:
            locked_versions[package["name"]].append(package.get("version"))
    expected: dict[str, str] = {}
    for name in GOLD_RUNTIME_PACKAGES:
        versions = locked_versions[name]
        if len(versions) != 1 or not isinstance(versions[0], str):
            raise EvidenceError(f"gold runtime lock has no unique exact pin: {name}")
        pinned = versions[0]
        if name != "tree-sitter":
            direct = [
                match.group(1)
                for requirement in requirements
                if isinstance(requirement, str)
                and (
                    match := re.fullmatch(
                        rf"{re.escape(name)}==([A-Za-z0-9][A-Za-z0-9._+-]*)", requirement
                    )
                )
            ]
            if direct != [pinned]:
                raise EvidenceError(f"gold runtime project/lock pin differs: {name}")
        try:
            active = version(name)
        except PackageNotFoundError as error:
            raise EvidenceError(f"gold runtime dependency missing: {name}") from error
        if active != pinned:
            raise EvidenceError(f"gold runtime dependency differs from source pin: {name}")
        expected[name] = pinned
    return expected


def _read_gold_capsule_file(path: Path) -> bytes:
    """Allow the source-derived label payload a larger explicit bound."""
    limit = GOLD_DOCUMENT_BYTES if path.name == "gold.json" else CONTROL_DOCUMENT_BYTES
    return _read_control_file(path, max_bytes=limit)


def _require_gold_material_bounds(material: dict[str, bytes]) -> None:
    for name, raw in material.items():
        limit = GOLD_DOCUMENT_BYTES if name == "gold.json" else CONTROL_DOCUMENT_BYTES
        if len(raw) > limit:
            raise EvidenceError(f"gold capsule {name} exceeds {limit}-byte limit")


def _gold_material(
    root: Path,
    selection: dict,
    recipe_raw: bytes,
    split: tuple[bytes, dict[str, Path]] | None = None,
    *,
    validated_manifest: dict | None = None,
    validated_document: dict | None = None,
) -> dict[str, bytes]:
    """Re-derive labels from a validated release, never captured product rows."""
    from tools.benchmark.retrieval import gold_oracle

    runtime = require_gold_runtime()
    producer_sources = _gold_producer_source_digests()
    _selection(selection)
    if root.absolute() != Path(selection["release_path"]):
        raise EvidenceError("gold release path differs from selected corpus")
    recipe = _json(recipe_raw)
    gold_oracle.validate_recipe(recipe)
    split_binding, split_material = _split_binding(recipe, selection, split, validated_manifest)
    document = validated_document if validated_document is not None else corpus.validate(root)
    if document["digest"] != selection["release_digest"]:
        raise EvidenceError("gold recipe selected a different corpus release")
    repositories = [
        row for row in document["repositories"] if row["recipe"]["name"] == selection["repository"]
    ]
    if len(repositories) != 1:
        raise EvidenceError("gold repository is absent or ambiguous")
    metadata = repositories[0]["views"][selection["view"]]
    manifest_raw = _read_control_file(root / metadata["manifest"])
    if digest_bytes(manifest_raw) != metadata["manifest_digest"]:
        raise EvidenceError("gold manifest differs from selected release")
    view = root / "views" / selection["repository"] / selection["view"]
    gold, blind = gold_oracle.derive(recipe, _json(manifest_raw), view)
    if _gold_producer_source_digests() != producer_sources:
        raise EvidenceError("gold producer source changed during derivation")
    context = {
        "release_digest": document["digest"],
        "manifest_digest": metadata["manifest_digest"],
        "repository": selection["repository"],
        "view": selection["view"],
        "repository_commit": repositories[0]["recipe"]["revision"],
    }
    gold.update(context)
    blind.update(context)
    material = {
        "selection.json": canonical_json(selection).encode() + b"\n",
        "recipe.json": recipe_raw,
        "gold.json": canonical_json(gold).encode() + b"\n",
        "blind.json": canonical_json(blind).encode() + b"\n",
        **split_material,
    }
    _require_gold_material_bounds(material)
    identity = {
        "schema_version": 2,
        "kind": "source_derived_gold_capsule",
        "qualification": "mechanical_unreviewed_diagnostic",
        "holdout_custody": "unsealed_external_custody_required",
        "release_digest": document["digest"],
        "manifest_digest": metadata["manifest_digest"],
        "producer_source_digests": producer_sources,
        "split_binding": split_binding,
        "parser_runtime": {
            "python": sys.version.split()[0],
            "unicode": unicodedata.unidata_version,
            "tree_sitter": runtime["tree-sitter"],
            "tree_sitter_language_pack": runtime["tree-sitter-language-pack"],
        },
        "files": {name: digest_bytes(raw) for name, raw in sorted(material.items())},
    }
    material["identity.json"] = canonical_json(identity).encode() + b"\n"
    return material


def capture_gold(
    root: Path,
    selection: dict,
    recipe_raw: bytes,
    target: Path,
    split: tuple[bytes, dict[str, Path]] | None = None,
) -> dict:
    """Publish a fresh, external gold capsule and label-free runner pack.

    `split` is (split-manifest bytes, release digest -> release path) and is
    required exactly for a schema v2 single-split recipe.
    """
    if (
        not target.is_absolute()
        or ".." in target.parts
        or target.exists()
        or target.is_symlink()
        or target.resolve().is_relative_to(corpus.ROOT)
        or target.resolve().is_relative_to(root.resolve())
        or root.resolve().is_relative_to(target.resolve())
    ):
        raise EvidenceError("gold target must be fresh, external and disjoint from corpus")
    stage = target.with_name(target.name + ".staging")
    if stage.exists() or stage.is_symlink():
        raise EvidenceError("gold staging target already exists")
    material = _gold_material(root, selection, recipe_raw, split)
    stage.mkdir(parents=True)
    for name, raw in material.items():
        with (stage / name).open("xb") as output:
            output.write(raw)
    identity = _verify_gold_material(stage, material)
    if _gold_producer_source_digests() != identity["producer_source_digests"]:
        raise EvidenceError("gold capture source changed before publication")
    if target.exists() or target.is_symlink():
        raise EvidenceError("gold output appeared before publication")
    stage.rename(target)
    return identity


def capture_gold_batch(
    root: Path,
    recipes: dict[str, bytes],
    target: Path,
    split: tuple[bytes, dict[str, Path]],
) -> dict[str, dict]:
    """Publish one capsule per repository with one corpus-wide split replay.

    The target is a fresh external directory. All release repositories must
    have one recipe; publication is one directory rename after a second split
    replay, so source changes during derivation cannot escape validation.
    """
    if not isinstance(split, tuple) or len(split) != 2:
        raise EvidenceError("gold batch requires its corpus-wide split manifest")
    manifest_raw, releases = split
    if (
        not isinstance(root, Path)
        or not root.is_absolute()
        or not isinstance(recipes, dict)
        or not recipes
        or not isinstance(target, Path)
        or not target.is_absolute()
        or ".." in target.parts
        or target.exists()
        or target.is_symlink()
        or target.resolve().is_relative_to(corpus.ROOT)
        or not isinstance(releases, dict)
        or any(
            not isinstance(path, Path)
            or target.resolve().is_relative_to(path.resolve())
            or path.resolve().is_relative_to(target.resolve())
            for path in releases.values()
        )
    ):
        raise EvidenceError("gold batch target must be fresh, external and disjoint from corpus")
    stage = target.with_name(target.name + ".staging")
    if stage.exists() or stage.is_symlink():
        raise EvidenceError("gold staging target already exists")
    require_gold_runtime()
    # Refuse every recipe before restoring Git bundles and comparing the
    # corpus-wide source fingerprints. Invalid later recipes must not make
    # an otherwise valid batch pay for a full split replay.
    from tools.benchmark.retrieval import gold_oracle

    for name, raw in recipes.items():
        if (
            not isinstance(name, str)
            or re.fullmatch(r"[A-Za-z][A-Za-z0-9_.-]*", name) is None
            or not isinstance(raw, bytes)
        ):
            raise EvidenceError("gold batch recipe inventory differs from release repositories")
        if not 0 < len(raw) <= CONTROL_DOCUMENT_BYTES:
            raise EvidenceError("gold batch recipe exceeds control-document size limit")
        gold_oracle.validate_recipe(_json(raw))
    manifest, documents = _validated_split_manifest(manifest_raw, releases)
    matches = [digest for digest, path in releases.items() if path == root]
    if len(matches) != 1:
        raise EvidenceError("gold batch release differs from split inventory")
    digest = matches[0]
    document = documents[digest]
    names = {row["recipe"]["name"] for row in document["repositories"]}
    if set(recipes) != names or any(
        not isinstance(name, str)
        or re.fullmatch(r"[A-Za-z][A-Za-z0-9_.-]*", name) is None
        or not isinstance(raw, bytes)
        for name, raw in recipes.items()
    ):
        raise EvidenceError("gold batch recipe inventory differs from release repositories")
    identities: dict[str, dict] = {}
    producer_sources = _gold_producer_source_digests()
    stage.mkdir(parents=True)
    try:
        for name in sorted(names):
            selection = {
                "release_path": str(root),
                "release_digest": digest,
                "repository": name,
                "view": SPLIT_LEAKAGE_POLICY["view"],
            }
            material = _gold_material(
                root,
                selection,
                recipes[name],
                split,
                validated_manifest=manifest,
                validated_document=document,
            )
            capsule = stage / name
            capsule.mkdir()
            for filename, raw in material.items():
                with (capsule / filename).open("xb") as output:
                    output.write(raw)
            identities[name] = _verify_gold_material(capsule, material)
        # The first replay establishes leakage and release identity; this
        # replay rejects changes to either split before atomic publication.
        if validate_split_manifest(manifest_raw, releases) != manifest:
            raise EvidenceError("gold batch split manifest changed before publication")
        if _gold_producer_source_digests() != producer_sources or any(
            identity["producer_source_digests"] != producer_sources
            for identity in identities.values()
        ):
            raise EvidenceError("gold capture source changed before publication")
        if target.exists() or target.is_symlink():
            raise EvidenceError("gold output appeared before publication")
        stage.rename(target)
    except BaseException:
        shutil.rmtree(stage)
        raise
    return identities


def _verify_gold_material(
    target: Path, material: dict[str, bytes], names: set[str] | None = None
) -> dict:
    observed = corpus.regular_tree(target) if names is None else names
    if observed != set(material):
        raise EvidenceError("gold capsule inventory differs from its recipe schema")
    for name, raw in material.items():
        if _read_gold_capsule_file(target / name) != raw:
            raise EvidenceError("gold capsule differs from source-derived oracle: " + name)
    return _json(material["identity.json"])


def validate_gold(target: Path) -> dict:
    """Reconstruct the exact oracle output from source and reject forged digests."""
    names = corpus.regular_tree(target)
    expected = {"selection.json", "recipe.json", "gold.json", "blind.json", "identity.json"}
    split_names = {"split-manifest.json", "split-releases.json"}
    if names == expected | split_names:
        expected = names
    elif names != expected:
        raise EvidenceError("gold capsule inventory differs")
    selection = _json(_read_control_file(target / "selection.json"))
    _selection(selection)
    recipe_raw = _read_control_file(target / "recipe.json")
    split = None
    if split_names <= names:
        split = (
            _read_control_file(target / "split-manifest.json"),
            _split_releases(_json(_read_control_file(target / "split-releases.json"))),
        )
    material = _gold_material(Path(selection["release_path"]), selection, recipe_raw, split)
    return _verify_gold_material(target, material, names)
