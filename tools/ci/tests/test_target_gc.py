"""Focused tests for tools/ci/target_gc.py against a fake Cargo target tree."""

from __future__ import annotations

import fcntl
import hashlib
import os
import subprocess
import sys
import time
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import target_gc  # noqa: E402

pytestmark = pytest.mark.unit

ROOT = Path(__file__).resolve().parents[3]
HOUR = 3600.0
NOW = time.time()


def _age(path: Path, hours: float) -> None:
    stamp = NOW - hours * HOUR
    os.utime(path, (stamp, stamp), follow_symlinks=False)


def _make_lane(hash_dir: Path, name: str, *, hours: float) -> Path:
    lane = hash_dir / name
    debug = lane / "debug"
    for child in ("deps", ".fingerprint", "build", "incremental"):
        (debug / child).mkdir(parents=True, exist_ok=True)
    (debug / ".cargo-lock").touch()
    (debug / "deps" / "libfoo-0123456789abcdef.rlib").write_bytes(b"x" * 4096)
    for path in sorted(lane.rglob("*"), key=lambda p: len(p.parts), reverse=True):
        _age(path, hours)
    _age(lane, hours)
    return lane


def _exe(deps: Path, name: str, *, hours: float) -> Path:
    path = deps / name
    path.write_bytes(b"\x7fELF" + b"0" * 4096)
    path.chmod(0o755)
    _age(path, hours)
    dep_info = deps / f"{name}.d"
    dep_info.write_text("deps\n")
    _age(dep_info, hours)
    _age(deps, hours)
    return path


def _hash(path: Path) -> str:
    return hashlib.sha256(str(path).encode()).hexdigest()[:16]


@pytest.fixture
def tree(tmp_path: Path) -> dict[str, Path]:
    checkout = tmp_path / "checkout"
    checkout.mkdir()
    cache = tmp_path / "cache"
    live = cache / "target" / _hash(checkout)
    orphan = cache / "target" / "deadbeefdeadbeef"
    live.mkdir(parents=True)
    orphan.mkdir(parents=True)
    return {"checkout": checkout, "cache": cache, "live": live, "orphan": orphan}


def _policy(**overrides: object) -> target_gc.Policy:
    values: dict[str, object] = {
        "idle_seconds": 24 * HOUR,
        "artifact_seconds": 24 * HOUR,
        "protected_lanes": frozenset(target_gc.DEFAULT_PROTECTED_LANES),
    }
    values.update(overrides)
    return target_gc.Policy(**values)  # type: ignore[arg-type]


def _census(*commands: str):
    return lambda: target_gc.ProcessCensus(list(commands))


def _gc(tree: dict[str, Path], *, dry_run: bool = False, census=None, **policy: object):
    return target_gc.run_gc(
        tree["cache"],
        [str(tree["checkout"])],
        _policy(**policy),
        dry_run=dry_run,
        census=census or _census(),
        now=NOW,
    )


def _kinds(report: target_gc.Report) -> dict[str, list[str]]:
    kinds: dict[str, list[str]] = {}
    for action in report.actions:
        kinds.setdefault(str(action["kind"]), []).append(str(action["path"]))
    return kinds


def test_checkout_id_matches_env_script(tmp_path: Path) -> None:
    scripts = tmp_path / "repo" / "scripts"
    scripts.mkdir(parents=True)
    (scripts / "quanta-index-env.sh").write_text((ROOT / "scripts/quanta-index-env.sh").read_text())
    env = {k: v for k, v in os.environ.items() if k != "CARGO_TARGET_DIR"}
    env.update({"QUANTA_INDEX_CACHE_ROOT": str(tmp_path / "c"), "QUANTA_INDEX_SCCACHE": "0"})
    target = subprocess.run(
        [
            "/bin/bash",
            "-c",
            'source "$1"; printf %s "$CARGO_TARGET_DIR"',
            "bash",
            str(scripts / "quanta-index-env.sh"),
        ],
        env=env,
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    repo = str(tmp_path / "repo")
    assert Path(target).parent.name in {
        target_gc.checkout_id(v) for v in target_gc.checkout_path_variants(repo)
    }


def test_idle_orphan_is_removed_but_live_checkout_dir_is_not(tree: dict[str, Path]) -> None:
    _make_lane(tree["orphan"], "dev-lane", hours=48)
    _age(tree["orphan"], 48)
    _make_lane(tree["live"], "shared", hours=48)
    report = _gc(tree)
    assert _kinds(report)["orphan"] == [str(tree["orphan"])]
    assert not tree["orphan"].exists()
    assert (tree["live"] / "shared").is_dir()


def test_recently_active_orphan_is_kept(tree: dict[str, Path]) -> None:
    _make_lane(tree["orphan"], "dev-lane", hours=48)
    (tree["orphan"] / "dev-lane" / target_gc.LANE_STAMP).touch()
    report = _gc(tree)
    assert "orphan" not in _kinds(report)
    assert tree["orphan"].is_dir()


def test_orphan_referenced_by_a_process_is_kept(tree: dict[str, Path]) -> None:
    _make_lane(tree["orphan"], "dev-lane", hours=48)
    _age(tree["orphan"], 48)
    busy = _census(f"{tree['orphan']}/dev-lane/debug/deps/t-0123456789abcdef --nocapture")
    report = _gc(tree, census=busy)
    assert "orphan" not in _kinds(report)
    assert tree["orphan"].is_dir()


def test_idle_lane_removed_protected_and_recent_lanes_kept(tree: dict[str, Path]) -> None:
    idle = _make_lane(tree["live"], "adhoc-lane", hours=30)
    recent = _make_lane(tree["live"], "fresh-lane", hours=2)
    protected = _make_lane(tree["live"], "test-workspace-lane", hours=500)
    extra = _make_lane(tree["live"], "mine-lane", hours=500)
    report = _gc(tree, protected_lanes=frozenset({*target_gc.DEFAULT_PROTECTED_LANES, "mine-lane"}))
    assert _kinds(report)["idle-lane"] == [str(idle)]
    assert not idle.exists()
    assert recent.is_dir() and protected.is_dir() and extra.is_dir()


def test_age_threshold_is_configurable(tree: dict[str, Path]) -> None:
    lane = _make_lane(tree["live"], "adhoc-lane", hours=30)
    report = _gc(tree, idle_seconds=48 * HOUR)
    assert "idle-lane" not in _kinds(report)
    assert lane.is_dir()


def test_last_used_stamp_counts_as_activity(tree: dict[str, Path]) -> None:
    lane = _make_lane(tree["live"], "adhoc-lane", hours=30)
    (lane / target_gc.LANE_STAMP).touch()
    _age(lane, 30)
    report = _gc(tree)
    assert "idle-lane" not in _kinds(report)


def test_locked_lane_is_kept(tree: dict[str, Path]) -> None:
    lane = _make_lane(tree["live"], "adhoc-lane", hours=30)
    with open(lane / "debug" / ".cargo-lock") as handle:
        fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
        report = _gc(tree)
    assert "idle-lane" not in _kinds(report)
    assert lane.is_dir()
    assert any("cargo lock held" in str(k["reason"]) for k in report.kept)


def test_unavailable_process_census_keeps_lanes(tree: dict[str, Path]) -> None:
    lane = _make_lane(tree["live"], "adhoc-lane", hours=30)
    report = _gc(tree, census=lambda: target_gc.ProcessCensus(None))
    assert "idle-lane" not in _kinds(report)
    assert lane.is_dir()


def test_dry_run_deletes_nothing(tree: dict[str, Path]) -> None:
    _make_lane(tree["orphan"], "dev-lane", hours=48)
    _age(tree["orphan"], 48)
    idle = _make_lane(tree["live"], "adhoc-lane", hours=30)
    shared = _make_lane(tree["live"], "shared", hours=1)
    deps = shared / "debug" / "deps"
    old = _exe(deps, "it-1111111111111111", hours=72)
    _exe(deps, "it-2222222222222222", hours=1)
    before = sorted(str(p) for p in tree["cache"].rglob("*"))
    report = _gc(tree, dry_run=True)
    assert sorted(str(p) for p in tree["cache"].rglob("*")) == before
    kinds = _kinds(report)
    assert str(tree["orphan"]) in kinds["orphan"]
    assert str(idle) in kinds["idle-lane"]
    assert str(old) in kinds["stale-deps"]
    assert report.to_json()["summary"]["reclaim_bytes"] > 0  # type: ignore[index]


def test_stale_deps_pruning_keeps_newest_per_stem(tree: dict[str, Path]) -> None:
    lane = _make_lane(tree["live"], "test-workspace-lane", hours=1)
    deps = lane / "debug" / "deps"
    old_a = _exe(deps, "core_tests-aaaaaaaaaaaaaaaa", hours=72)
    old_b = _exe(deps, "core_tests-bbbbbbbbbbbbbbbb", hours=48)
    newest = _exe(deps, "core_tests-cccccccccccccccc", hours=30)
    young = _exe(deps, "other-dddddddddddddddd", hours=2)
    young_old = _exe(deps, "other-eeeeeeeeeeeeeeee", hours=3)
    sole = _exe(deps, "single-ffffffffffffffff", hours=500)
    report = _gc(tree)
    removed = set(_kinds(report)["stale-deps"])
    assert removed == {str(old_a), f"{old_a}.d", str(old_b), f"{old_b}.d"}
    for path in (newest, young, young_old, sole, deps / "libfoo-0123456789abcdef.rlib"):
        assert path.exists()
    assert not old_a.exists() and not Path(f"{old_b}.d").exists()


def test_recently_executed_binary_is_not_stale(tree: dict[str, Path]) -> None:
    lane = _make_lane(tree["live"], "shared", hours=1)
    deps = lane / "debug" / "deps"
    ran = _exe(deps, "t-aaaaaaaaaaaaaaaa", hours=72)
    os.utime(ran, (NOW - HOUR, NOW - 72 * HOUR))  # atime recent: executed lately
    _exe(deps, "t-bbbbbbbbbbbbbbbb", hours=30)
    report = _gc(tree)
    assert str(ran) not in _kinds(report).get("stale-deps", [])
    assert ran.exists()


def test_stale_deps_skipped_while_lane_locked(tree: dict[str, Path]) -> None:
    lane = _make_lane(tree["live"], "shared", hours=1)
    deps = lane / "debug" / "deps"
    old = _exe(deps, "t-aaaaaaaaaaaaaaaa", hours=72)
    _exe(deps, "t-bbbbbbbbbbbbbbbb", hours=1)
    with open(lane / "debug" / ".cargo-lock") as handle:
        fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
        report = _gc(tree)
    assert "stale-deps" not in _kinds(report)
    assert old.exists()


def test_stale_incremental_dirs_pruned(tree: dict[str, Path]) -> None:
    lane = _make_lane(tree["live"], "shared", hours=1)
    incremental = lane / "debug" / "incremental"
    dirs = {}
    for name, hours in (("core-aaaaaaaaaaaaaaaa", 72), ("core-bbbbbbbbbbbbbbbb", 2)):
        session = incremental / name / "s-session"
        session.mkdir(parents=True)
        (session / "obj.o").write_bytes(b"o" * 4096)
        for path in (session / "obj.o", session, incremental / name):
            _age(path, hours)
        dirs[name] = incremental / name
    report = _gc(tree)
    assert _kinds(report)["stale-incremental"] == [str(dirs["core-aaaaaaaaaaaaaaaa"])]
    assert not dirs["core-aaaaaaaaaaaaaaaa"].exists()
    assert dirs["core-bbbbbbbbbbbbbbbb"].is_dir()
    report = _gc(tree, prune_incremental=False)
    assert "stale-incremental" not in _kinds(report)


def test_size_budget_evicts_oldest_unprotected_lanes(tree: dict[str, Path]) -> None:
    oldest = _make_lane(tree["live"], "a-lane", hours=10)
    middle = _make_lane(tree["live"], "b-lane", hours=5)
    newest = _make_lane(tree["live"], "c-lane", hours=3)
    shared = _make_lane(tree["live"], "shared", hours=20)
    for lane, hours in ((oldest, 10), (middle, 5), (newest, 3), (shared, 20)):
        (lane / "blob").write_bytes(b"b" * 1_000_000)
        _age(lane / "blob", hours)
        _age(lane, hours)
    per_lane = target_gc.tree_usage(oldest)[0]
    budget = int(per_lane * 2.5)
    report = _gc(tree, max_total_bytes=budget, prune_artifacts=False)
    assert _kinds(report)["budget-lane"] == [str(oldest), str(middle)]
    assert newest.is_dir() and shared.is_dir()


def test_trash_leftovers_are_swept(tree: dict[str, Path]) -> None:
    trash = tree["live"] / f"{target_gc.TRASH_PREFIX}old-lane-1-2"
    (trash / "debug").mkdir(parents=True)
    report = _gc(tree)
    assert _kinds(report)["trash"] == [str(trash)]
    assert not trash.exists()


def test_auto_mode_is_rate_limited_and_never_fails(tmp_path: Path) -> None:
    cache = tmp_path / "cache"
    state = cache / "target-gc"
    state.mkdir(parents=True)
    (state / "last-run").touch()
    lane = _make_lane(cache / "target" / "deadbeefdeadbeef", "x-lane", hours=48)
    _age(lane.parent, 48)
    assert target_gc.main(["--auto", "--cache-root", str(cache), "--repo-root", str(ROOT)]) == 0
    assert lane.is_dir()  # stamp is fresh: no run


def _cargow_env(tmp_path: Path, **extra: str) -> dict[str, str]:
    binary = tmp_path / "bin"
    binary.mkdir(exist_ok=True)
    cargo = binary / "cargo"
    cargo.write_text(f"#!{sys.executable}\nraise SystemExit(7)\n")
    cargo.chmod(0o755)
    env = {
        k: v
        for k, v in os.environ.items()
        if k not in {"CI", "CARGO_TARGET_DIR", "QUANTA_INDEX_BUILD_LANE", "QUANTA_INDEX_TARGET_GC"}
    }
    env.update(
        {
            "PATH": f"{binary}:{Path(sys.executable).parent}:{env['PATH']}",
            "QUANTA_INDEX_CACHE_ROOT": str(tmp_path / "cache"),
            "QUANTA_INDEX_BUILD_LOGGING": "0",
            "QUANTA_INDEX_SCCACHE": "0",
            "QUANTA_INDEX_RESOURCE_ADMISSION": "0",
        }
    )
    env.update(extra)
    return env


def _wait_for(path: Path, timeout: float = 15.0) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if path.exists() and path.stat().st_size > 0:
            return True
        time.sleep(0.05)
    return False


def test_cargow_marks_lane_and_starts_detached_gc_without_changing_status(tmp_path: Path) -> None:
    env = _cargow_env(tmp_path)
    result = subprocess.run(
        [str(ROOT / "scripts/cargow"), "--lane", "probe-lane", "check"],
        cwd=ROOT,
        env=env,
        capture_output=True,
        text=True,
        timeout=30,
    )
    assert result.returncode == 7  # the leaf status survives the GC hook
    lane = tmp_path / "cache" / "target" / _hash(ROOT) / "probe-lane"
    assert (lane / target_gc.LANE_STAMP).is_file()
    assert _wait_for(tmp_path / "cache" / "target-gc" / "auto.log")
    assert (tmp_path / "cache" / "target-gc" / "last-run").is_file()


def test_cargow_gc_hook_can_be_disabled(tmp_path: Path) -> None:
    env = _cargow_env(tmp_path, QUANTA_INDEX_TARGET_GC="0")
    subprocess.run(
        [str(ROOT / "scripts/cargow"), "--lane", "probe-lane", "check"],
        cwd=ROOT,
        env=env,
        capture_output=True,
        timeout=30,
    )
    time.sleep(0.5)
    assert not (tmp_path / "cache" / "target-gc").exists()
