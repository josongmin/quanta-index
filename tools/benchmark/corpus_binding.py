"""Bind frozen query inputs to one Git-derived external corpus view.

A retained release capsule replays real Git objects without mutable source
paths. Binding input universes is not independent product index attestation,
gold adjudication, rank equivalence, or performance qualification.
"""

from __future__ import annotations

import hashlib
import tempfile
from importlib.metadata import version
from pathlib import Path

import corpus_release as corpus
from evidence import (
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


def _gold_material(root: Path, selection: dict, recipe_raw: bytes) -> dict[str, bytes]:
    """Re-derive labels from a validated release, never captured product rows."""
    from tools.benchmark.retrieval import gold_oracle

    _selection(selection)
    if root.absolute() != Path(selection["release_path"]):
        raise EvidenceError("gold release path differs from selected corpus")
    recipe = _json(recipe_raw)
    gold_oracle.validate_recipe(recipe)
    document = corpus.validate(root)
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
    }
    if any(len(raw) > 16 * 1024 * 1024 for raw in material.values()):
        raise EvidenceError("gold control document exceeds 16 MiB")
    identity = {
        "schema_version": 1,
        "kind": "source_derived_gold_capsule",
        "qualification": "mechanical_unreviewed_diagnostic",
        "holdout_custody": "unsealed_external_custody_required",
        "release_digest": document["digest"],
        "manifest_digest": metadata["manifest_digest"],
        "oracle_source_digest": digest_bytes(_read_regular_file(Path(gold_oracle.__file__))),
        "binding_source_digest": digest_bytes(_read_regular_file(Path(__file__))),
        "parser_runtime": {
            "tree_sitter": version("tree-sitter"),
            "tree_sitter_language_pack": version("tree-sitter-language-pack"),
        },
        "files": {name: digest_bytes(raw) for name, raw in sorted(material.items())},
    }
    material["identity.json"] = canonical_json(identity).encode() + b"\n"
    return material


def capture_gold(root: Path, selection: dict, recipe_raw: bytes, target: Path) -> dict:
    """Publish a fresh, external gold capsule and label-free runner pack."""
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
    material = _gold_material(root, selection, recipe_raw)
    stage.mkdir(parents=True)
    for name, raw in material.items():
        with (stage / name).open("xb") as output:
            output.write(raw)
    identity = validate_gold(stage)
    if target.exists() or target.is_symlink():
        raise EvidenceError("gold output appeared before publication")
    stage.rename(target)
    return identity


def validate_gold(target: Path) -> dict:
    """Reconstruct the exact oracle output from source and reject forged digests."""
    names = corpus.regular_tree(target)
    expected = {"selection.json", "recipe.json", "gold.json", "blind.json", "identity.json"}
    if names != expected:
        raise EvidenceError("gold capsule inventory differs")
    selection = _json(_read_control_file(target / "selection.json"))
    _selection(selection)
    recipe_raw = _read_control_file(target / "recipe.json")
    material = _gold_material(Path(selection["release_path"]), selection, recipe_raw)
    for name in expected:
        if _read_control_file(target / name) != material[name]:
            raise EvidenceError("gold capsule differs from source-derived oracle: " + name)
    return _json(material["identity.json"])
