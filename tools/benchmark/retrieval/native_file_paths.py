"""Path admission shared by Native capture and report producers."""

from collections.abc import Iterable, Iterator
from dataclasses import dataclass, field
from functools import cache
from pathlib import Path

from tools.benchmark.evidence import EvidenceError, RunStore

COHORT_FILES = ("suite", "query_pack", "capture", "source_universe", "source_binding")
RELEASE_ROOTS = ("base_release", "target_releases")


def resolve_path(path: Path) -> Path:
    try:
        try:
            return path.resolve(strict=True)
        except FileNotFoundError:
            return path.resolve()
    except (OSError, RuntimeError) as error:
        # Strict resolution detects cycles on both sides of the Python 3.13 change.
        raise EvidenceError(f"cannot resolve Native path {path}: {error}") from error


def cohort_inputs(
    manifest_path: Path,
    manifest: dict,
    *,
    publishing_family: str | None = None,
) -> Iterator[tuple[str, Path]]:
    for family in manifest["families"]:
        yield "source repository", manifest_path.parent / family["repo"]
        for role in (*RELEASE_ROOTS, *COHORT_FILES, "daemon_config", "selection_path"):
            if role not in family or (
                role == "capture" and family["family_id"] == publishing_family
            ):
                continue
            yield role, manifest_path.parent / family[role]


def custody_paths(root: Path) -> tuple[Path, ...]:
    store = RunStore(resolve_path(root))
    return (
        store.latest_path,
        store.runs_dir,
        store.staging_dir,
        store.baselines_dir,
        *(
            store.root / name
            for name in ("captures", "profiles", "work", "failures", ".custody.lock")
        ),
    )


def require_disjoint(
    writes: Iterable[Path],
    inputs: Iterable[tuple[str, Path]],
    *,
    error: str,
) -> None:
    @cache
    def identity(path: Path) -> tuple[int, int] | None:
        try:
            info = path.stat()
        except (FileNotFoundError, NotADirectoryError):
            return None
        return info.st_dev, info.st_ino

    def ancestry(path: Path) -> set[tuple[int, int]]:
        return {key for parent in (path, *path.parents) if (key := identity(parent)) is not None}

    # A filesystem can resolve differently spelled paths to the same
    # directory. Compare ancestry by identity as well as by canonical name.
    destinations = [
        (target, identity(target), ancestry(target)) for target in map(resolve_path, writes)
    ]
    for role, path in inputs:
        source = resolve_path(path)
        source_key, source_ancestry = identity(source), ancestry(source)
        if any(
            target.is_relative_to(source)
            or source.is_relative_to(target)
            or source_key in target_ancestry
            or target_key in source_ancestry
            for target, target_key, target_ancestry in destinations
        ):
            raise EvidenceError(error.format(role=role))


@dataclass
class Output:
    declared: Path
    path: Path
    _inputs: list[tuple[str, Path]] = field(default_factory=list, init=False, repr=False)

    @classmethod
    def prepare(cls, path: Path, *, exists_error: str) -> "Output":
        path = path.absolute()
        destination = resolve_path(path.parent) / path.name
        if destination.is_symlink() or destination.exists():
            raise EvidenceError(exists_error)
        return cls(path, destination)

    def protect(self, inputs: Iterable[tuple[str, Path]]) -> None:
        paths = tuple(inputs)
        require_disjoint((self.path,), paths, error="Native report output overlaps {role}")
        self._inputs.extend(paths)

    def write(self, rendered: str) -> None:
        if resolve_path(self.declared.parent) != self.path.parent:
            raise EvidenceError("Native report output parent changed before publication")
        require_disjoint((self.path,), self._inputs, error="Native report output overlaps {role}")
        with self.path.open("x", encoding="utf-8") as stream:
            stream.write(rendered)
