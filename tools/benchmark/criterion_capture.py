"""Registered Criterion 0.5 wall-time capture and raw-derived replay.

One fresh output directory per producer; listing, smoke and measurement must
all succeed before any profile is published. No ambient Criterion cache is read.
"""

from __future__ import annotations

import math
import os
import platform
import socket
import statistics
import sys
import uuid
from datetime import datetime, timezone
from pathlib import Path

from custody import custody
from evidence import (
    EvidenceError,
    RunStore,
    _read_regular_file,
    _run_id,
    canonical_json,
    digest_bytes,
    parse_json,
)
from evidence_bridge import (
    host_identity,
    micro_payload_from_criterion,
    promote_native_run,
    source_identity,
)
from producer_execution import execute
from profile_capture import _directories, commit_capture, load_capture
from registry import registry_digest


def listed_cases(raw: bytes) -> list[str]:
    cases = []
    for line in raw.decode("utf-8").splitlines():
        if not line.strip():
            continue
        if not line.endswith(": benchmark"):
            raise EvidenceError("unexpected Criterion listing output")
        cases.append(line.removesuffix(": benchmark"))
    if not cases or any(not case for case in cases) or len(cases) != len(set(cases)):
        raise EvidenceError("Criterion case listing is empty or duplicated")
    return cases


def require_case_inventory(family: str, cases: list[str]) -> None:
    if family == "micro-lq-norm-pipeline" and set(cases) != {
        f"{stage}/{size}"
        for stage in ("tokenize", "parse", "normalize", "hash")
        for size in (1024, 4096, 16384)
    }:
        raise EvidenceError("LQ Criterion inventory must cover all four stages and three sizes")


def payload(raw: dict[str, bytes], case: str) -> dict:
    required = {"benchmark.json", "sample.json", "estimates.json", "listing.txt"}
    if not required <= raw.keys():
        raise EvidenceError("Criterion raw inventory is incomplete")
    if case not in listed_cases(raw["listing.txt"]):
        raise EvidenceError("Criterion case is not in the frozen listing")
    meta, samples, estimates = (
        parse_json(raw[name].decode())
        for name in ("benchmark.json", "sample.json", "estimates.json")
    )
    if (
        not isinstance(meta, dict)
        or set(meta)
        != {
            "group_id",
            "function_id",
            "value_str",
            "throughput",
            "full_id",
            "directory_name",
            "title",
        }
        or meta.get("full_id") != case
    ):
        raise EvidenceError("Criterion benchmark identity mismatch")
    if any(
        not isinstance(meta[key], str) or not meta[key]
        for key in ("group_id", "directory_name", "title")
    ):
        raise EvidenceError("Criterion benchmark metadata has invalid strings")
    if any(
        meta[key] is not None and not isinstance(meta[key], str)
        for key in ("function_id", "value_str")
    ):
        raise EvidenceError("Criterion benchmark optional identity is malformed")
    throughput = meta["throughput"]
    if throughput is not None:
        if not isinstance(throughput, dict) or len(throughput) != 1:
            raise EvidenceError("Criterion throughput metadata is malformed")
        kind, count = next(iter(throughput.items()))
        if (
            kind not in {"Bytes", "BytesDecimal", "Elements"}
            or type(count) is not int
            or not 0 <= count <= 2**64 - 1
        ):
            raise EvidenceError("Criterion throughput metadata is malformed")
    if not isinstance(samples, dict) or set(samples) != {"sampling_mode", "iters", "times"}:
        raise EvidenceError("Criterion sample schema mismatch")
    if not isinstance(samples["sampling_mode"], str) or samples["sampling_mode"] not in {
        "Linear",
        "Flat",
    }:
        raise EvidenceError("unsupported Criterion sampling mode")
    iters, times = samples["iters"], samples["times"]
    if (
        not isinstance(iters, list)
        or not isinstance(times, list)
        or len(iters) < 10
        or len(iters) != len(times)
    ):
        raise EvidenceError("Criterion samples are partial or below the sample floor")

    def number(value: object, *, zero: bool = False) -> float:
        if type(value) not in {int, float}:
            raise EvidenceError("Criterion sample must be a finite number")
        try:
            result = float(value)
        except OverflowError as exc:
            raise EvidenceError("Criterion sample overflow") from exc
        if not math.isfinite(result) or result < 0 or (not zero and result == 0):
            raise EvidenceError("Criterion sample must be finite and positive")
        return result

    iterations = [number(value) for value in iters]
    elapsed = [number(value) for value in times]
    if any(not value.is_integer() or value > 2**53 for value in iterations):
        raise EvidenceError("Criterion iteration counts must be exact integers")
    # Equal lengths were independently required above; keep the direct CLI
    # compatible with the system Python as well as the locked uv runtime.
    try:
        mean = statistics.fmean(t / n for t, n in zip(elapsed, iterations))
    except OverflowError as exc:
        raise EvidenceError("Criterion raw mean overflow") from exc
    if not isinstance(estimates, dict) or set(estimates) != {
        "mean",
        "median",
        "median_abs_dev",
        "slope",
        "std_dev",
    }:
        raise EvidenceError("Criterion estimates are incomplete")
    if (samples["sampling_mode"] == "Linear") != (estimates["slope"] is not None):
        raise EvidenceError("Criterion slope estimate disagrees with sampling mode")
    for name, item in estimates.items():
        if name == "slope" and item is None:
            continue
        if not isinstance(item, dict) or set(item) != {
            "confidence_interval",
            "point_estimate",
            "standard_error",
        }:
            raise EvidenceError("Criterion estimate schema is incomplete")
        number(item["point_estimate"], zero=True)
        number(item["standard_error"], zero=True)
        ci = item["confidence_interval"]
        if not isinstance(ci, dict) or set(ci) != {
            "confidence_level",
            "lower_bound",
            "upper_bound",
        }:
            raise EvidenceError("Criterion confidence interval schema is incomplete")
        if number(ci["confidence_level"]) >= 1 or number(ci["lower_bound"], zero=True) > number(
            ci["upper_bound"], zero=True
        ):
            raise EvidenceError("Criterion confidence interval is invalid")
    estimate = number(estimates["mean"].get("point_estimate"))
    if not math.isclose(mean, estimate, rel_tol=1e-12, abs_tol=1e-12):
        raise EvidenceError("Criterion mean disagrees with the raw samples")
    return micro_payload_from_criterion(
        bench_id=case,
        statistic="mean",
        value_ns=estimate,
        iterations=sum(int(value) for value in iterations),
        samples=len(iterations),
    )


def _binary(build_output: bytes, target: str) -> tuple[Path, list[str]]:
    matches = []
    finished = 0
    for line in build_output.decode().splitlines():
        if not line.startswith("{"):
            continue  # cargow routing diagnostics are not Cargo messages.
        message = parse_json(line)
        if not isinstance(message, dict):
            raise EvidenceError("Cargo compiler output is not an object")
        if message.get("reason") == "build-finished":
            if message.get("success") is not True:
                raise EvidenceError("Cargo native build verdict is not success")
            finished += 1
        if message.get("reason") == "compiler-artifact" and not isinstance(
            message.get("target"), dict
        ):
            raise EvidenceError("Cargo artifact target is malformed")
        if message.get("reason") == "compiler-artifact" and not isinstance(
            message["target"].get("kind"), list
        ):
            raise EvidenceError("Cargo artifact target kind is malformed")
        if (
            message.get("reason") == "compiler-artifact"
            and message.get("target", {}).get("name") == target
            and "bench" in message.get("target", {}).get("kind", [])
            and message.get("executable")
        ):
            features = message.get("features")
            if (
                not isinstance(message["executable"], str)
                or not isinstance(features, list)
                or any(not isinstance(feature, str) for feature in features)
                or len(set(features)) != len(features)
            ):
                raise EvidenceError("Cargo executable/features inventory is malformed")
            matches.append((Path(message["executable"]), features))
    if len(matches) != 1 or finished != 1:
        raise EvidenceError("Cargo did not report exactly one registered bench executable")
    return matches[0]


def replay_run(store: RunStore, evidence: dict, producer: dict | None = None) -> None:
    if evidence["payload"]["kind"] != "micro" or evidence["verdict"]["scope"] != "diagnostic":
        raise EvidenceError("Criterion wall capture is diagnostic only")
    raw = {
        Path(ref["path"]).name: _read_regular_file(store.run_dir(evidence["run_id"]) / ref["path"])
        for ref in evidence["raw"]
    }
    derived = payload(raw, evidence["case_id"])
    if derived != evidence["payload"]:
        raise EvidenceError("Criterion typed payload differs from native samples")
    if "execution.json" not in raw or "rustc.txt" not in raw or "build.jsonl" not in raw:
        raise EvidenceError("Criterion build/execution provenance is missing")
    execution = parse_json(raw["execution.json"].decode())
    if not isinstance(execution, dict) or execution.get("measure") != evidence["command"]:
        raise EvidenceError("Criterion native command differs from the typed envelope")
    capture_id = _run_id(execution.get("capture_id"))
    if not evidence["run_id"].startswith(capture_id + "-"):
        raise EvidenceError("Criterion run does not belong to its native capture")
    binaries = evidence["build"]["binaries"]
    if len(binaries) != 1 or binaries[0]["sha256"] != execution.get("binary_digest"):
        raise EvidenceError("Criterion binary inventory differs from execution provenance")
    if evidence["build"]["toolchain"] != raw["rustc.txt"].decode().strip():
        raise EvidenceError("Criterion toolchain differs from captured rustc identity")
    build_command = execution.get("build")
    if not isinstance(build_command, dict) or not isinstance(build_command.get("argv"), list):
        raise EvidenceError("Criterion build command is missing")
    argv = build_command["argv"]
    if (
        len(argv) != 12
        or argv[1:5] != ["--lane", "bench-lane", "bench", "-p"]
        or argv[6] != "--bench"
        or argv[8:] != ["--all-features", "--locked", "--no-run", "--message-format=json"]
        or build_command.get("status") != "completed"
        or build_command.get("exit_code") != 0
    ):
        raise EvidenceError("Criterion build command is not the registered Cargo bench rail")
    if producer is not None and (
        producer["kind"] != "cargo-bench"
        or argv[5] != producer["package"]
        or argv[7] != producer["target"]
    ):
        raise EvidenceError("Criterion build owner differs from the registered producer")
    binary, features = _binary(raw["build.jsonl"], argv[7])
    if (
        features != execution.get("features")
        or binary.name != binaries[0]["name"]
        or evidence["command"]["argv"][0] != str(binary)
        or argv[3:] != evidence["build"]["flags"]
    ):
        raise EvidenceError(
            "Criterion native executable/feature/flag inventory differs from envelope"
        )


def validate(repo: Path, root: Path, profile: str, registry: dict) -> dict:
    document = load_capture(root, profile=profile, registry_digest=registry_digest(registry))
    if set(document["expected_cases"]) != set(registry["profiles"][profile]["families"]):
        raise EvidenceError("capture does not cover the current registered profile")
    if document["source"] != source_identity(repo, "benchmark-micro"):
        raise EvidenceError("Criterion source is stale or dirty")
    store = RunStore(root)
    for record in document["runs"]:
        if not record["run_id"].startswith(document["capture_id"] + "-"):
            raise EvidenceError("Criterion profile mixes different producer captures")
        evidence = store.load(record["run_id"])
        replay_run(
            store,
            evidence,
            registry["producers"][registry["families"][record["family"]]["producer"]],
        )
        if evidence["build"]["lockfile_digest"] != digest_bytes((repo / "Cargo.lock").read_bytes()):
            raise EvidenceError("Criterion lockfile differs from current source")
        raw_listing = _read_regular_file(store.run_dir(record["run_id"]) / "raw/listing.txt")
        require_case_inventory(record["family"], listed_cases(raw_listing))
        if set(listed_cases(raw_listing)) != set(document["expected_cases"][record["family"]]):
            raise EvidenceError("capture inventory differs from the independent binary listing")
    return document


def capture(
    repo: Path,
    root: Path,
    profile: str,
    registry: dict,
    *,
    samples: int,
    warmup: float,
    measurement: float,
    resamples: int,
    timeout: int,
) -> dict:
    from benchctl import require_clean_worktree, require_frozen_source, resolve_checkout_head

    if sys.platform != "darwin" and not sys.platform.startswith("linux"):
        raise EvidenceError(
            "Criterion capture requires the supported Linux/macOS POSIX custody rail"
        )
    if root.resolve().is_relative_to(repo.resolve()):
        raise EvidenceError("Criterion evidence root must stay outside the checkout")
    _directories(root)
    if (
        type(samples) is not int
        or samples < 10
        or resamples < 1000
        or timeout < 1
        or any(not math.isfinite(value) or value <= 0 for value in (warmup, measurement))
    ):
        raise EvidenceError("invalid Criterion diagnostic sampling parameters")
    require_clean_worktree(repo)
    head = resolve_checkout_head(repo)
    source = source_identity(repo, "benchmark-micro")
    cpu_count = os.cpu_count()
    if type(cpu_count) is not int or cpu_count < 1:
        raise EvidenceError("cannot establish the capture host CPU inventory")
    host = host_identity(
        policy="local-diagnostic",
        os_name="macos" if sys.platform == "darwin" else "linux",
        arch=platform.machine(),
        cpu_count=cpu_count,
        hostname=socket.gethostname(),
        lease_mode="none",
        lease_samples=0,
    )
    capture_id = f"{profile}-{uuid.uuid4().hex}"
    scratch = root / "work" / capture_id
    scratch.mkdir(parents=True, exist_ok=False)
    # Mark custody before any run promotion, including for the Rust collector.
    (root / "captures").mkdir(exist_ok=True)
    prepared, expected = [], {}
    for family in registry["profiles"][profile]["families"]:
        entry = registry["families"][family]
        producer = registry["producers"][entry["producer"]]
        if entry["payload"] != "micro" or producer["kind"] != "cargo-bench":
            raise EvidenceError("profile contains a non-Criterion producer")
        work = scratch / family
        work.mkdir()
        env = {
            **os.environ,
            "CRITERION_HOME": str(work / "criterion"),
            "DSL_BENCH_WARM_OUT": str(work / "dsl-native.json"),
        }
        print(f"Criterion capture: build {family}", file=sys.stderr, flush=True)
        build_argv = [
            str(repo / "scripts/cargow"),
            "--lane",
            "bench-lane",
            "bench",
            "-p",
            producer["package"],
            "--bench",
            producer["target"],
            "--all-features",
            "--locked",
            "--no-run",
            "--message-format=json",
        ]
        output, stderr, build_command = execute(build_argv, cwd=repo, env=env, timeout=timeout)
        binary, features = _binary(output, producer["target"])
        binary_digest = digest_bytes(_read_regular_file(binary))
        rustc, rustc_stderr, _ = execute(["rustc", "-vV"], cwd=repo, env=env, timeout=timeout)
        target = next(
            (
                line.removeprefix("host: ")
                for line in rustc.decode().splitlines()
                if line.startswith("host: ")
            ),
            None,
        )
        if target is None:
            raise EvidenceError("rustc did not report a target triple")
        listing, list_stderr, _ = execute(
            [str(binary), "--list", "--format", "terse"], cwd=repo, env=env, timeout=timeout
        )
        cases = listed_cases(listing)
        require_case_inventory(family, cases)
        print(
            f"Criterion capture: smoke/measure {family}: {len(cases)} cases",
            file=sys.stderr,
            flush=True,
        )
        smoke, smoke_stderr, _ = execute(
            [str(binary), "--test"], cwd=repo, env=env, timeout=timeout
        )
        measure_argv = [
            str(binary),
            "--bench",
            "--noplot",
            "--sample-size",
            str(samples),
            "--warm-up-time",
            str(warmup),
            "--measurement-time",
            str(measurement),
            "--nresamples",
            str(resamples),
        ]
        measured, measure_stderr, command = execute(
            measure_argv, cwd=repo, env=env, timeout=timeout
        )
        if digest_bytes(_read_regular_file(binary)) != binary_digest:
            raise EvidenceError("executed Criterion binary changed during capture")
        require_frozen_source(repo, head)
        native_cases = {}
        for path in (work / "criterion").rglob("new/benchmark.json"):
            if path.is_symlink() or any(parent.is_symlink() for parent in path.parents):
                raise EvidenceError("Criterion output contains a symlink")
            meta = parse_json(_read_regular_file(path).decode())
            case = meta.get("full_id")
            if not isinstance(case, str) or case in native_cases:
                raise EvidenceError("Criterion output contains a duplicate/invalid case")
            native_cases[case] = path.parent
        if set(native_cases) != set(cases):
            raise EvidenceError("Criterion output is missing cases or has undeclared cases")
        expected[family] = cases
        common = {
            "listing.txt": listing,
            "build.jsonl": output,
            "rustc.txt": rustc,
            "execution.json": canonical_json(
                {
                    "capture_id": capture_id,
                    "build": build_command,
                    "measure": command,
                    "features": features,
                    "binary_digest": binary_digest,
                }
            ).encode(),
            "stdout.txt": measured,
            "smoke.txt": smoke,
            "stderr.txt": stderr + rustc_stderr + list_stderr + smoke_stderr + measure_stderr,
        }
        # Preserve non-secret build controls and diagnostic fixture knobs.
        build_environment = {
            key: value
            for key, value in env.items()
            if key
            in {
                "RUSTFLAGS",
                "CARGO_ENCODED_RUSTFLAGS",
                "CARGO_BUILD_TARGET",
                "RUSTUP_TOOLCHAIN",
                "CARGO_TARGET_DIR",
                "CC",
                "CFLAGS",
                "AR",
                "QUANTA_INDEX_BENCH_DISABLE_QUERY_OBS",
                "DSL_BENCH_WARM_SAMPLES",
            }
            or key.startswith("CARGO_PROFILE_BENCH_")
        }
        common["environment.json"] = canonical_json(build_environment).encode()
        if (work / "dsl-native.json").exists():
            common["dsl-native.json"] = _read_regular_file(work / "dsl-native.json")
        build = {
            "toolchain": rustc.decode().strip(),
            "target_triple": target,
            "lockfile_digest": digest_bytes((repo / "Cargo.lock").read_bytes()),
            "profile": "bench",
            "flags": build_argv[3:],
            "binaries": [{"name": binary.name, "sha256": binary_digest}],
        }
        for case in cases:
            raw = {
                **common,
                **{
                    name: _read_regular_file(native_cases[case] / name)
                    for name in ("benchmark.json", "sample.json", "estimates.json")
                },
            }
            prepared.append((family, case, raw, payload(raw, case), build, command))
    require_frozen_source(repo, head)
    with custody(root):
        return _publish(
            repo, root, profile, registry, head, source, host, capture_id, prepared, expected
        )


def _publish(repo, root, profile, registry, head, source, host, capture_id, prepared, expected):
    from benchctl import require_frozen_source

    promoted = []
    for index, (family, case, raw, derived, build, command) in enumerate(prepared):
        items = list(raw.items())
        result = promote_native_run(
            evidence_root=root,
            run_id=f"{capture_id}-{index}",
            family=family,
            profile=profile,
            case_id=case,
            created_utc=datetime.now(timezone.utc).isoformat().replace("+00:00", "Z"),
            native_path=Path(items[0][0]),
            native_bytes=items[0][1],
            additional_native=[(Path(name), data) for name, data in items[1:]],
            payload=derived,
            source=source,
            build=build,
            inputs=[],
            host=host,
            command=command,
            boundary={
                "clock": "monotonic",
                "instrumentation": "wall",
                "start_event": "criterion_sample_start",
                "end_event": "criterion_sample_end",
            },
            verdict={"scope": "diagnostic", "status": "pass", "reason": None, "metrics": []},
        )
        promoted.append(result["run_id"])
    require_frozen_source(repo, head)
    return commit_capture(
        root,
        capture_id=capture_id,
        profile=profile,
        registry_digest=registry_digest(registry),
        expected_cases=expected,
        run_ids=promoted,
    )
