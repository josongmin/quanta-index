"""Freeze current non-plan inputs for one source-stable L4 proof run."""

import datetime
import hashlib
import json
import pathlib
import shutil
import subprocess
import tempfile


folder = pathlib.Path(__file__).resolve().parent
live = pathlib.Path(__file__).resolve().parents[6]
previous = folder.parent / "further-audit-20260927" / "snapshot.json"
paths = list(json.loads(previous.read_text())["manifest"])
external_paths = list(json.loads(previous.read_text())["external_dependency_inputs"])


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def inputs(root):
    return {name: sha(root / name) for name in paths}


def external():
    return {name: sha(pathlib.Path(name)) for name in external_paths}


def git(*args):
    return subprocess.check_output(["git", *args], cwd=live, text=True).strip()


before = inputs(live)
external_before = external()
head_before = git("rev-parse", "HEAD")
dirty_before = git("status", "--porcelain=v1")
frozen = pathlib.Path(tempfile.mkdtemp(prefix="qi-l4-overlap-src-"))
for name in paths:
    target = frozen / name
    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(live / name, target)
copied = inputs(frozen)
after = inputs(live)
external_after = external()
head_after = git("rev-parse", "HEAD")
record = {
    "utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
    "origin": str(live),
    "root": str(frozen),
    "head": head_before,
    "head_after": head_after,
    "dirty": dirty_before,
    "dirty_after": git("status", "--porcelain=v1"),
    "manifest": copied,
    "external_dependency_inputs": external_paths,
    "external_dependency_sha256": external_before,
    "source_sha256": hashlib.sha256(
        json.dumps(copied, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest(),
    "copy_matches_live_before_and_after": before == copied == after,
    "external_unchanged": external_before == external_after,
}
(folder / "snapshot.json").write_text(json.dumps(record, indent=2) + "\n")
print(
    json.dumps(
        {key: record[key] for key in (
            "root", "head", "head_after", "source_sha256",
            "copy_matches_live_before_and_after", "external_unchanged"
        )}
    )
)
if not record["copy_matches_live_before_and_after"] or not record["external_unchanged"]:
    raise SystemExit(2)
