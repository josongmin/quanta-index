"""Fail-closed clean/one-overlay scanner source identity.

This is a source snapshot primitive, not a build or runtime attestation.
No repository file is modified by this module.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import stat
import subprocess
import tempfile
from difflib import unified_diff
from os import stat_result
from pathlib import Path


class CustodyError(ValueError):
    pass


SCANNER_PATH = "crates/quanta-index-lexical/src/searcher/code_search.rs"
CONTROL_POLICY = "code-search-typo-unicode-control-v1"

# Fixed reviewed Rust fragments. A rewrite of these anchors requires a new
# policy review; unrelated changes elsewhere in code_search.rs do not.
_HELPER = b"""fn typo_text_is_ascii(text: &str, budget: &RequestBudgetV1) -> Result<bool, CoreError> {
    for chunk in text.as_bytes().chunks(16_384) {
        budget.checkpoint("lexical:code-search-typo-ascii-detect")?;
        if !chunk.is_ascii() {
            return Ok(false);
        }
    }
    Ok(true)
}
"""
_START = b'    budget.checkpoint("lexical:code-search-typo-file-start")?;\n'
_END = b'    budget.checkpoint("lexical:code-search-typo-file-end")?;\n'
_ASCII_BRANCH = b"""    if typo_text_is_ascii(text, budget)? {
        // ASCII byte offsets are scalar ordinals, and the token predicate is
        // exactly ASCII alphanumeric or underscore. A non-ASCII scalar may
        // join an ASCII run, so mixed text retains the Unicode tokenizer.
        scan_typo_token_spans(
            text.bytes()
                .enumerate()
                .map(|(index, byte)| (index, byte.is_ascii_alphanumeric() || byte == b'_')),
            text.len(),
            budget,
            &mut check_token,
        )?;
    } else {
        scan_typo_token_spans(
            text.char_indices()
                .map(|(index, ch)| (index, normalize::is_token_char(ch))),
            text.len(),
            budget,
            &mut check_token,
        )?;
    }
"""
_UNICODE_BRANCH = b"""    scan_typo_token_spans(
        text.char_indices()
            .map(|(index, ch)| (index, normalize::is_token_char(ch))),
        text.len(),
        budget,
        &mut check_token,
    )?;
"""


def canonical_control_bytes(base_data: bytes) -> bytes:
    """Apply exactly the reviewed ASCII-to-Unicode scanner control policy."""
    if not isinstance(base_data, bytes):
        raise CustodyError("scanner policy input must be bytes")
    witness = b"fn typo_witness(\n"
    next_item = b"/// Conservatively shortlist files with shared trigrams.\n"
    if (
        base_data.count(_HELPER) != 1
        or base_data.count(witness) != 1
        or base_data.count(next_item) != 1
    ):
        raise CustodyError("scanner control policy helper or witness anchor changed")
    helper_at = base_data.index(_HELPER)
    if base_data[max(0, helper_at - len(b"#[cfg(test)]\n")) : helper_at] == b"#[cfg(test)]\n":
        raise CustodyError("scanner control helper is already test-only")
    witness_at = base_data.index(witness)
    next_at = base_data.index(next_item, witness_at)
    body = base_data[witness_at:next_at]
    if (
        helper_at >= witness_at
        or body.count(_START) != 1
        or body.count(_END) != 1
        or body.count(_ASCII_BRANCH) != 1
        or body.count(_START + _ASCII_BRANCH + _END) != 1
        or base_data.count(_ASCII_BRANCH) != 1
    ):
        raise CustodyError("scanner control policy branch or span changed")
    return base_data.replace(_HELPER, b"#[cfg(test)]\n" + _HELPER, 1).replace(
        _START + _ASCII_BRANCH + _END,
        _START + _UNICODE_BRANCH + _END,
        1,
    )


def _canonical(value: object) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()


def _sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _hex(value: object, length: int) -> bool:
    return (
        isinstance(value, str)
        and len(value) == length
        and all(c in "0123456789abcdef" for c in value)
    )


def _git(repo: Path, *argv: str, env: dict[str, str] | None = None) -> bytes:
    process = subprocess.run(["git", *argv], cwd=repo, env=env, check=False, capture_output=True)
    if process.returncode:
        raise CustodyError(f"git {argv[0]} failed: {process.stderr.decode(errors='replace')[:300]}")
    return process.stdout


def _tree(repo: Path, base: str, env: dict[str, str] | None) -> dict[str, tuple[str, str]]:
    entries: dict[str, tuple[str, str]] = {}
    for item in _git(repo, "ls-tree", "-rz", "--full-tree", base, env=env).split(b"\0"):
        if not item:
            continue
        try:
            header, raw = item.split(b"\t", 1)
            mode, kind, oid = header.decode("ascii").split()
            path = raw.decode("utf-8")
        except (ValueError, UnicodeDecodeError) as error:
            raise CustodyError("invalid Git tree inventory") from error
        if kind != "blob" or mode not in ("100644", "100755"):
            raise CustodyError(f"unsupported tracked entry: {path}")
        if path in entries or Path(path).is_absolute() or ".." in Path(path).parts:
            raise CustodyError("duplicate or unsafe Git tree path")
        entries[path] = (mode, oid)
    if not entries:
        raise CustodyError("empty source tree")
    return entries


def _check_overlay(
    repo: Path, base: str, tree: dict, overlay: dict | None, env: dict[str, str] | None
) -> None:
    if overlay is None:
        return
    if not isinstance(overlay, dict) or set(overlay) != {
        "policy_version",
        "path",
        "base_sha256",
        "result_sha256",
        "patch_path",
        "patch_sha256",
    }:
        raise CustodyError("overlay schema differs")
    path = overlay["path"]
    if overlay["policy_version"] != CONTROL_POLICY or path != SCANNER_PATH or path not in tree:
        raise CustodyError("overlay must change only the canonical CodeSearch scanner")
    for key in ("base_sha256", "result_sha256", "patch_sha256"):
        if not _hex(overlay[key], 64):
            raise CustodyError(f"invalid overlay {key}")
    if not isinstance(overlay["patch_path"], str):
        raise CustodyError("overlay patch path must be a string")
    patch_path = Path(overlay["patch_path"])
    if (
        not patch_path.is_absolute()
        or not patch_path.is_file()
        or patch_path.resolve(strict=True) != patch_path
        or not stat.S_ISREG(patch_path.stat().st_mode)
    ):
        raise CustodyError("overlay patch must be an absolute regular file")
    patch = patch_path.read_bytes()
    if _sha(patch) != overlay["patch_sha256"]:
        raise CustodyError("overlay patch digest differs")
    base_data = _git(repo, "show", f"{base}:{path}", env=env)
    if _sha(base_data) != overlay["base_sha256"]:
        raise CustodyError("overlay base digest differs")
    canonical = canonical_control_bytes(base_data)
    if _sha(canonical) != overlay["result_sha256"]:
        raise CustodyError("overlay result differs from canonical scanner control policy")
    with tempfile.TemporaryDirectory(prefix="qi-scanner-patch-") as scratch:
        root = Path(scratch)
        target = root / path
        target.parent.mkdir(parents=True)
        target.write_bytes(base_data)
        process = subprocess.run(
            ["git", "apply", "--whitespace=nowarn", "-"],
            cwd=root,
            env=env,
            input=patch,
            check=False,
            capture_output=True,
        )
        if process.returncode:
            raise CustodyError("overlay patch does not apply to base bytes")
        observed = {p.relative_to(root).as_posix() for p in root.rglob("*") if p.is_file()}
        if observed != {path} or target.read_bytes() != canonical:
            raise CustodyError("overlay patch changes unexpected source bytes")


def prepare_control(repo: Path, base: str, patch_path: Path, overlay_path: Path) -> dict:
    """Write a reviewed-policy control patch and dynamically bound overlay spec."""
    repo = repo.resolve(strict=True)
    if not _hex(base, 40) or _git(repo, "rev-parse", "HEAD").decode().strip() != base:
        raise CustodyError("base revision differs from checkout HEAD")
    if patch_path == overlay_path:
        raise CustodyError("control patch and overlay paths must differ")
    for destination in (patch_path, overlay_path):
        if (
            not destination.is_absolute()
            or destination.exists()
            or destination.resolve() != destination
            or repo == destination.parent.resolve()
            or repo in destination.parent.resolve().parents
        ):
            raise CustodyError("control output must be a new absolute file outside checkout")
    source = _git(repo, "show", f"{base}:{SCANNER_PATH}")
    control = canonical_control_bytes(source)
    try:
        before = source.decode("utf-8").splitlines(keepends=True)
        after = control.decode("utf-8").splitlines(keepends=True)
    except UnicodeDecodeError as error:
        raise CustodyError("scanner source is not UTF-8") from error
    patch = "".join(
        unified_diff(
            before,
            after,
            fromfile=f"a/{SCANNER_PATH}",
            tofile=f"b/{SCANNER_PATH}",
        )
    ).encode("utf-8")
    overlay = {
        "policy_version": CONTROL_POLICY,
        "path": SCANNER_PATH,
        "base_sha256": _sha(source),
        "result_sha256": _sha(control),
        "patch_path": str(patch_path),
        "patch_sha256": _sha(patch),
    }
    patch_path.parent.mkdir(parents=True, exist_ok=True)
    overlay_path.parent.mkdir(parents=True, exist_ok=True)
    with patch_path.open("xb") as stream:
        stream.write(patch)
    with overlay_path.open("xb") as stream:
        stream.write(_canonical(overlay) + b"\n")
    return overlay


def capture(
    repo: Path, base: str, overlay: dict | None = None, *, env: dict[str, str] | None = None
) -> dict:
    """Bind full tracked tree and at most one verified overlay at a stable HEAD."""
    repo = repo.resolve(strict=True)
    if not _hex(base, 40) or _git(repo, "rev-parse", "HEAD", env=env).decode().strip() != base:
        raise CustodyError("base revision differs from checkout HEAD")
    if _git(repo, "rev-parse", "--show-object-format", env=env).decode().strip() != "sha1":
        raise CustodyError("unsupported Git object format")
    if _git(repo, "diff", "--cached", "--name-only", "-z", env=env):
        raise CustodyError("staged index drift")
    if _git(repo, "ls-files", "--others", "-z", env=env):
        raise CustodyError("untracked or ignored source present")
    tree = _tree(repo, base, env)
    tracked = {
        p.decode("utf-8")
        for p in _git(repo, "ls-files", "--cached", "-z", env=env).split(b"\0")
        if p
    }
    if tracked != set(tree):
        raise CustodyError("index path set differs from base tree")
    _check_overlay(repo, base, tree, overlay, env)
    files = []
    observed_states = {}
    for relative, (mode, oid) in sorted(tree.items()):
        path = repo / relative
        if path.is_symlink() or not path.is_file():
            raise CustodyError(f"tracked source is missing or not a regular file: {relative}")
        before = path.stat()
        data = path.read_bytes()
        after = path.stat()

        def state(s: stat_result) -> tuple[int, ...]:
            return (s.st_ino, s.st_size, s.st_mtime_ns, s.st_ctime_ns)

        if state(before) != state(after):
            raise CustodyError(f"source changed while reading: {relative}")
        if bool(after.st_mode & 0o111) != (mode == "100755"):
            raise CustodyError(f"source mode differs: {relative}")
        observed_states[relative] = state(after)
        digest = _sha(data)
        if overlay is not None and relative == overlay["path"]:
            if digest != overlay["result_sha256"]:
                raise CustodyError("overlay result bytes differ")
        elif hashlib.sha1(b"blob " + str(len(data)).encode() + b"\0" + data).hexdigest() != oid:
            raise CustodyError(f"unlisted dirty source: {relative}")
        files.append({"path": relative, "mode": mode, "sha256": digest})
    for relative, observed in observed_states.items():
        path = repo / relative
        if path.is_symlink() or state(path.stat()) != observed:
            raise CustodyError(f"source changed during snapshot: {relative}")
    if _git(repo, "rev-parse", "HEAD", env=env).decode().strip() != base:
        raise CustodyError("HEAD changed during source snapshot")
    if _git(repo, "ls-files", "--others", "-z", env=env):
        raise CustodyError("new untracked source appeared")
    if _git(repo, "diff", "--cached", "--name-only", "-z", env=env):
        raise CustodyError("index changed during source snapshot")
    normalized_overlay = (
        None
        if overlay is None
        else {
            key: overlay[key]
            for key in ("policy_version", "path", "base_sha256", "result_sha256", "patch_sha256")
        }
    )
    core = {
        "schema_version": 1,
        "base_git_revision": base,
        "source_inventory": files,
        "source_inventory_sha256": _sha(_canonical(files)),
        "overlay": normalized_overlay,
    }
    return {**core, "identity_sha256": _sha(_canonical(core))}


def verify(
    repo: Path, expected: dict, *, patch_path: Path | None = None, env: dict[str, str] | None = None
) -> None:
    """Re-read actual source and refuse a malformed or stale claimed identity."""
    if (
        not isinstance(expected, dict)
        or set(expected)
        != {
            "schema_version",
            "base_git_revision",
            "source_inventory",
            "source_inventory_sha256",
            "overlay",
            "identity_sha256",
        }
        or type(expected["schema_version"]) is not int
        or expected["schema_version"] != 1
    ):
        raise CustodyError("invalid scanner source identity schema")
    overlay = expected["overlay"]
    if overlay is not None:
        if not isinstance(overlay, dict) or patch_path is None:
            raise CustodyError("overlay patch path required")
        overlay = {**overlay, "patch_path": str(patch_path)}
    actual = capture(repo, expected["base_git_revision"], overlay, env=env)
    if _canonical(actual) != _canonical(expected):
        raise CustodyError("scanner source identity differs from actual checkout")


def verify_pair(control: dict, candidate: dict) -> None:
    """Require one approved scanner overlay against an otherwise identical Git tree."""
    if not isinstance(control, dict) or not isinstance(candidate, dict):
        raise CustodyError("scanner A/B identities must be objects")
    overlay = control.get("overlay")
    if (
        not isinstance(overlay, dict)
        or set(overlay)
        != {"policy_version", "path", "base_sha256", "result_sha256", "patch_sha256"}
        or overlay.get("policy_version") != CONTROL_POLICY
        or overlay.get("path") != SCANNER_PATH
        or candidate.get("overlay") is not None
        or control.get("base_git_revision") != candidate.get("base_git_revision")
    ):
        raise CustodyError("scanner A/B must share one base with only the approved control overlay")
    rows = (control.get("source_inventory"), candidate.get("source_inventory"))
    if any(not isinstance(part, list) for part in rows):
        raise CustodyError("scanner A/B source inventories are malformed")
    by_path = []
    for part in rows:
        if any(not isinstance(row, dict) or set(row) != {"path", "mode", "sha256"} for row in part):
            raise CustodyError("scanner A/B source row is malformed")
        mapped = {row["path"]: row for row in part}
        if len(mapped) != len(part):
            raise CustodyError("scanner A/B source path is duplicated")
        by_path.append(mapped)
    before, after = by_path
    if set(before) != set(after) or SCANNER_PATH not in before:
        raise CustodyError("scanner A/B tracked source paths differ")
    if any(before[path] != after[path] for path in before if path != SCANNER_PATH):
        raise CustodyError("scanner A/B changed source outside CodeSearch scanner")
    if (
        before[SCANNER_PATH]["mode"] != after[SCANNER_PATH]["mode"]
        or before[SCANNER_PATH]["sha256"] != overlay["result_sha256"]
        or after[SCANNER_PATH]["sha256"] != overlay["base_sha256"]
        or control.get("identity_sha256") == candidate.get("identity_sha256")
    ):
        raise CustodyError("scanner A/B scanner byte or mode transition differs")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("prepare", choices=("prepare",))
    parser.add_argument("--repo", type=Path, required=True)
    parser.add_argument("--base", required=True)
    parser.add_argument("--patch", type=Path, required=True)
    parser.add_argument("--overlay", type=Path, required=True)
    args = parser.parse_args()
    prepare_control(args.repo, args.base, args.patch, args.overlay)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
