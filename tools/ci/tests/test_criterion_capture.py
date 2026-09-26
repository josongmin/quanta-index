"""Independent raw Criterion samples govern typed evidence, not PASS flags."""

import copy
import json
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[3] / "tools/benchmark"))
import criterion_capture as capture


def raw():
    estimate = {
        "point_estimate": 100.0,
        "standard_error": 0.0,
        "confidence_interval": {
            "confidence_level": 0.95,
            "lower_bound": 100.0,
            "upper_bound": 100.0,
        },
    }
    return {
        "listing.txt": b"parse/1024: benchmark\n",
        "benchmark.json": json.dumps(
            {
                "full_id": "parse/1024",
                "group_id": "parse",
                "function_id": None,
                "value_str": "1024",
                "throughput": None,
                "directory_name": "parse/1024",
                "title": "parse/1024",
            }
        ).encode(),
        "sample.json": json.dumps(
            {
                "sampling_mode": "Linear",
                "iters": list(range(1, 11)),
                "times": [100 * n for n in range(1, 11)],
            }
        ).encode(),
        "estimates.json": json.dumps(
            {name: estimate for name in ("mean", "median", "median_abs_dev", "slope", "std_dev")}
        ).encode(),
    }


def test_raw_mean_and_iterations_are_preserved():
    result = capture.payload(raw(), "parse/1024")
    assert result == {
        "kind": "micro",
        "bench_id": "parse/1024",
        "metric": "mean",
        "unit": "ns",
        "instrumentation": "wall",
        "statistic": "mean",
        "value": 100.0,
        "iterations": 55,
        "samples": 10,
    }


@pytest.mark.parametrize(
    "filename,field,value",
    [
        ("benchmark.json", "full_id", "wrong"),
        ("benchmark.json", "group_id", []),
        ("benchmark.json", "throughput", {"Bytes": True}),
        ("sample.json", "iters", [1] * 9),
        ("sample.json", "iters", [True] * 10),
        ("sample.json", "iters", [1.5] * 10),
        ("sample.json", "times", [None] * 10),
        ("sample.json", "times", [float("nan")] * 10),
        ("sample.json", "sampling_mode", "unknown"),
        ("estimates.json", "mean", {"point_estimate": 99.0}),
        ("estimates.json", "mean", {}),
        ("estimates.json", "slope", None),
    ],
)
def test_bad_native_facts_are_refused(filename, field, value):
    data = raw()
    document = json.loads(data[filename])
    document[field] = value
    data[filename] = json.dumps(document).encode()
    with pytest.raises(ValueError):
        capture.payload(data, "parse/1024")


@pytest.mark.parametrize("listing", [b"", b"a: benchmark\na: benchmark\n", b"a\n"])
def test_listing_is_exact_and_nonempty(listing):
    with pytest.raises(ValueError):
        capture.listed_cases(listing)


def test_duplicate_json_keys_are_not_last_writer_wins():
    data = raw()
    data["benchmark.json"] = b'{"full_id":"wrong","full_id":"parse/1024"}'
    with pytest.raises(ValueError, match="duplicate"):
        capture.payload(data, "parse/1024")


def test_missing_native_sample_refused():
    data = raw()
    del data["sample.json"]
    with pytest.raises(ValueError, match="incomplete"):
        capture.payload(data, "parse/1024")


def test_replay_refuses_forged_typed_mean(tmp_path):
    data = raw()
    digest = capture.digest_bytes(b"binary")
    build_argv = [
        "/fixture/scripts/cargow",
        "--lane",
        "bench-lane",
        "bench",
        "-p",
        "owner",
        "--bench",
        "pipeline",
        "--all-features",
        "--locked",
        "--no-run",
        "--message-format=json",
    ]
    command = {"argv": ["/fixture/pipeline"]}
    data.update(
        {
            "execution.json": json.dumps(
                {
                    "measure": command,
                    "capture_id": "r1",
                    "binary_digest": digest,
                    "features": [],
                    "build": {"argv": build_argv, "status": "completed", "exit_code": 0},
                }
            ).encode(),
            "rustc.txt": b"rustc pinned",
            "build.jsonl": json.dumps(
                {
                    "reason": "compiler-artifact",
                    "target": {"name": "pipeline", "kind": ["bench"]},
                    "features": [],
                    "executable": "/fixture/pipeline",
                }
            ).encode()
            + b'\n{"reason":"build-finished","success":true}\n',
        }
    )
    run = tmp_path / "runs/r1-0/raw"
    run.mkdir(parents=True)
    for name, content in data.items():
        (run / name).write_bytes(content)
    evidence = {
        "run_id": "r1-0",
        "case_id": "parse/1024",
        "payload": copy.deepcopy(capture.payload(data, "parse/1024")),
        "verdict": {"scope": "diagnostic"},
        "raw": [{"path": f"raw/{name}"} for name in data],
        "command": command,
        "build": {
            "binaries": [{"name": "pipeline", "sha256": digest}],
            "toolchain": "rustc pinned",
            "flags": build_argv[3:],
        },
    }
    capture.replay_run(capture.RunStore(tmp_path), evidence)
    evidence["payload"]["value"] = 42.0
    with pytest.raises(ValueError, match="differs"):
        capture.replay_run(capture.RunStore(tmp_path), evidence)
    evidence["payload"]["value"] = 100.0
    execution = json.loads(data["execution.json"])
    execution["capture_id"] = "another-capture"
    (run / "execution.json").write_text(json.dumps(execution))
    with pytest.raises(ValueError, match="does not belong"):
        capture.replay_run(capture.RunStore(tmp_path), evidence)


@pytest.mark.parametrize(
    "messages",
    [
        b'{"reason":"build-finished","success":false}\n',
        b'{"reason":"compiler-artifact","target":[]}',
        b"[1,2,3]",
        b'{"reason":"compiler-artifact","target":{"name":"pipeline","kind":["bench"]},"executable":"/fixture/pipeline","features":[]}',
    ],
)
def test_malformed_or_partial_cargo_inventory_is_refused(messages):
    with pytest.raises(ValueError):
        capture._binary(messages, "pipeline")


def test_sigterm_kills_owned_producer(tmp_path):
    import os
    import signal
    import subprocess
    import time

    pid_file = tmp_path / "child.pid"
    child_script = (
        f"import os,time; from pathlib import Path; p=Path({str(pid_file)!r}); "
        "staged=p.with_suffix('.tmp'); staged.write_text(str(os.getpid())); staged.replace(p); time.sleep(60)"
    )
    runner = (
        "import os,sys; from pathlib import Path; "
        f"sys.path.insert(0,{str(Path(capture.__file__).parent)!r}); "
        "from criterion_capture import execute; "
        f"execute([sys.executable,'-c',{child_script!r}],cwd=Path({str(tmp_path)!r}),env=dict(os.environ),timeout=60)"
    )
    process = subprocess.Popen(
        [sys.executable, "-c", runner], stdout=subprocess.PIPE, stderr=subprocess.PIPE
    )
    try:
        deadline = time.monotonic() + 10
        while not pid_file.exists() and time.monotonic() < deadline:
            time.sleep(0.01)
        assert pid_file.exists(), "owned child did not start"
        child_pid = int(pid_file.read_text())
        process.send_signal(signal.SIGTERM)
        _, stderr = process.communicate(timeout=10)
        assert process.returncode != 0
        assert b"interrupted by SIGTERM" in stderr
        with pytest.raises(ProcessLookupError):
            os.kill(child_pid, 0)
    finally:
        if process.poll() is None:
            process.terminate()
            process.communicate(timeout=10)


def test_timeout_kills_owned_process_group(tmp_path):
    import os
    import signal

    previous = signal.getsignal(signal.SIGTERM)
    with pytest.raises(ValueError, match="timed out"):
        capture.execute(
            [sys.executable, "-c", "import time; time.sleep(60)"],
            cwd=tmp_path,
            env=dict(os.environ),
            timeout=1,
        )
    assert signal.getsignal(signal.SIGTERM) == previous


@pytest.mark.parametrize("missing", [False, True])
def test_complete_profile_transaction_with_synthetic_native_owner(tmp_path, monkeypatch, missing):
    """Exercise orchestration; synthetic timing is never product evidence."""
    import benchctl

    repo = tmp_path / "repo"
    repo.mkdir()
    (repo / "Cargo.lock").write_bytes(b"locked")
    binary = tmp_path / "pipeline"
    binary.write_bytes(b"synthetic executable identity")
    cases = [
        f"{stage}/{size}"
        for stage in ("tokenize", "parse", "normalize", "hash")
        for size in (1024, 4096, 16384)
    ]
    listing = "".join(f"{case}: benchmark\n" for case in cases).encode()
    source = {
        "revision": "a" * 40,
        "dirty": False,
        "dirty_paths_digest": None,
        "closure_profile": "benchmark-micro",
        "closure_digest": capture.digest_bytes(b"source"),
    }
    monkeypatch.setattr(benchctl, "require_clean_worktree", lambda _repo: None)
    monkeypatch.setattr(benchctl, "require_frozen_source", lambda _repo, _head: None)
    monkeypatch.setattr(benchctl, "resolve_checkout_head", lambda _repo: "a" * 40)
    monkeypatch.setattr(capture, "source_identity", lambda *_args: source)
    registry = {
        "schema_version": 1,
        "closures": {},
        "external_inputs": {},
        "validators": {},
        "scorers": {},
        "profiles": {"micro": {"families": ["micro-lq-norm-pipeline"]}},
        "families": {"micro-lq-norm-pipeline": {"payload": "micro", "producer": "pipeline"}},
        "producers": {
            "pipeline": {"kind": "cargo-bench", "package": "owner", "target": "pipeline"}
        },
    }

    def producer(argv, *, cwd, env, timeout):
        command = {
            "argv": argv,
            "cwd": str(cwd),
            "status": "completed",
            "exit_code": 0,
            "timeout_seconds": timeout,
            "wall_ms": 1,
        }
        if "--message-format=json" in argv:
            output = (
                json.dumps(
                    {
                        "reason": "compiler-artifact",
                        "target": {"name": "pipeline", "kind": ["bench"]},
                        "executable": str(binary),
                        "features": [],
                    }
                ).encode()
                + b'\n{"reason":"build-finished","success":true}\n'
            )
        elif argv[0] == "rustc":
            output = b"rustc pinned\nhost: aarch64-apple-darwin\n"
        elif "--list" in argv:
            output = listing
        elif "--test" in argv:
            output = b"Success\n"
        else:
            for index, case in enumerate(cases[:-1] if missing else cases):
                directory = Path(env["CRITERION_HOME"]) / str(index) / "new"
                directory.mkdir(parents=True)
                data = raw()
                meta = json.loads(data["benchmark.json"])
                meta["full_id"] = case
                data["benchmark.json"] = json.dumps(meta).encode()
                for name in ("benchmark.json", "sample.json", "estimates.json"):
                    (directory / name).write_bytes(data[name])
            output = b"measured\n"
        return output, b"", command

    monkeypatch.setattr(capture, "execute", producer)
    root = tmp_path / "evidence"
    kwargs = dict(samples=10, warmup=0.01, measurement=0.01, resamples=1000, timeout=1)
    if missing:
        with pytest.raises(ValueError, match="missing cases"):
            capture.capture(repo, root, "micro", registry, **kwargs)
        assert not (root / "profiles/micro.json").exists()
        assert not (root / "runs").exists()
    else:
        document = capture.capture(repo, root, "micro", registry, **kwargs)
        assert len(document["runs"]) == 12
        assert capture.validate(repo, root, "micro", registry) == document
