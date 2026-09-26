"""Cross-language conformance and refusal tests for `BenchmarkEvidenceV1`.

The Rust crate `benchmarks/bench-protocol` is the normative definition of the
canonical form. These tests prove the Python writer/reader produces the same
bytes and refuses the same malformed documents, using the committed fixtures in
`benchmarks/bench-protocol/fixtures/` as the shared oracle.
"""

from __future__ import annotations

import hashlib
import importlib.util
import json
import os
import subprocess
import sys
import uuid
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[3]
EVIDENCE_PATH = REPO_ROOT / "tools" / "benchmark" / "evidence.py"
FIXTURE_DIR = REPO_ROOT / "benchmarks" / "bench-protocol" / "fixtures"


def _load_evidence_module():
    if str(EVIDENCE_PATH.parent) not in sys.path:
        sys.path.insert(0, str(EVIDENCE_PATH.parent))
    spec = importlib.util.spec_from_file_location("benchmark_evidence", EVIDENCE_PATH)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def _sealed_text(module) -> str:
    return module.to_canonical_json(module.seal(module.sample_evidence()))


def test_regular_evidence_refuses_leaf_replaced_after_stat(tmp_path, monkeypatch):
    module = _load_evidence_module()
    path = tmp_path / "evidence"
    replacement = tmp_path / "replacement"
    path.write_bytes(b"original")
    replacement.write_bytes(b"replaced")
    original = Path.lstat
    swapped = False

    def race(candidate, *args, **kwargs):
        nonlocal swapped
        metadata = original(candidate, *args, **kwargs)
        if candidate == path and not swapped:
            swapped = True
            path.unlink()
            path.symlink_to(replacement)
        return metadata

    monkeypatch.setattr(Path, "lstat", race)
    with pytest.raises(module.EvidenceError):
        module._read_regular_file(path)


def test_regular_evidence_refuses_ancestor_symlink(tmp_path):
    module = _load_evidence_module()
    actual = tmp_path / "actual"
    actual.mkdir()
    (actual / "evidence").write_bytes(b"original")
    alias = tmp_path / "alias"
    alias.symlink_to(actual, target_is_directory=True)
    with pytest.raises(module.EvidenceError, match="symlink"):
        module._read_regular_file(alias / "evidence")


@pytest.mark.parametrize("mutation", ["replace", "grow", "restore"])
def test_regular_evidence_refuses_mutation_during_read(tmp_path, monkeypatch, mutation):
    module = _load_evidence_module()
    path = tmp_path / "evidence"
    path.write_bytes(b"original")
    from tools.ci.lint import handoff_validation

    original = handoff_validation._consume_repo_regular_file

    def race(*args, **kwargs):
        raw = original(*args, **kwargs)
        if mutation == "replace":
            other = tmp_path / "new"
            other.write_bytes(raw)
            other.replace(path)
        elif mutation == "grow":
            path.write_bytes(raw + b"extra")
        else:
            before = path.stat()
            path.write_bytes(b"tampered")
            path.write_bytes(raw)
            os.utime(path, ns=(before.st_atime_ns, before.st_mtime_ns))
        return raw

    monkeypatch.setattr(handoff_validation, "_consume_repo_regular_file", race)
    with pytest.raises(module.EvidenceError, match="changed"):
        module._read_regular_file(path)


# --------------------------------------------------------------------------
# Canonical form
# --------------------------------------------------------------------------


def test_canonical_vectors_match_the_rust_oracle() -> None:
    module = _load_evidence_module()
    fixture = json.loads((FIXTURE_DIR / "canonical-json-vectors.json").read_text(encoding="utf-8"))
    assert fixture["schema_version"] == 1
    assert fixture["vectors"], "vector fixture is empty"
    for vector in fixture["vectors"]:
        assert module.canonical_json(vector["json"]) == vector["canonical"], vector["name"]


def test_sample_evidence_matches_the_rust_golden() -> None:
    module = _load_evidence_module()
    golden = (FIXTURE_DIR / "sample-evidence.json").read_text(encoding="utf-8").strip()
    sealed = module.seal(module.sample_evidence())
    assert module.to_canonical_json(sealed) == golden
    assert module.open_evidence(golden)["digest"] == sealed["digest"]


def test_seal_open_round_trips() -> None:
    module = _load_evidence_module()
    text = _sealed_text(module)
    reopened = module.open_evidence(text)
    assert module.to_canonical_json(reopened) == text


def test_float_canonical_form_never_uses_exponents() -> None:
    module = _load_evidence_module()
    assert module.canonical_json({"v": 1.0}) == '{"v":1.0}'
    assert module.canonical_json({"v": 1e-5}) == '{"v":0.00001}'
    assert module.canonical_json({"v": 1e16}) == '{"v":10000000000000000.0}'
    with pytest.raises(module.EvidenceError):
        module.canonical_json({"v": float("inf")})


@pytest.mark.parametrize("value", [float("nan"), float("inf"), -float("inf"), 10**1000])
def test_payload_validation_refuses_non_finite_numeric_facts(value) -> None:
    module = _load_evidence_module()
    payload = module.sample_evidence()["payload"]
    payload["rows"][0]["p50"] = value
    with pytest.raises(module.EvidenceError, match="finite"):
        module.validate_payload(payload)


def test_python_counters_match_the_rust_u64_boundary() -> None:
    module = _load_evidence_module()
    payload = module.sample_evidence()["payload"]
    payload["errors"] = (1 << 64) - 1
    module.validate_payload(payload)
    payload["errors"] += 1
    with pytest.raises(module.EvidenceError, match="u64 range"):
        module.validate_payload(payload)


# --------------------------------------------------------------------------
# Document-level refusals
# --------------------------------------------------------------------------


def test_duplicate_json_key_is_refused() -> None:
    module = _load_evidence_module()
    text = _sealed_text(module)
    mutated = text.replace("{", '{"family":"duplicated",', 1)
    with pytest.raises(module.EvidenceError, match="duplicate JSON key"):
        module.open_evidence(mutated)


def test_unknown_field_is_refused() -> None:
    module = _load_evidence_module()
    text = _sealed_text(module)
    mutated = text.replace("{", '{"bogus_field":1,', 1)
    with pytest.raises(module.EvidenceError):
        module.open_evidence(mutated)


def test_malformed_and_tampered_digests_are_refused() -> None:
    module = _load_evidence_module()
    sealed = module.seal(module.sample_evidence())
    declared = sealed["digest"]
    text = module.to_canonical_json(sealed)
    with pytest.raises(module.EvidenceError, match="invalid digest"):
        module.open_evidence(text.replace(declared, "sha256:zz"))
    retimed = text.replace('"wall_ms":12345', '"wall_ms":12346')
    assert retimed != text
    with pytest.raises(module.EvidenceError, match="digest mismatch"):
        module.open_evidence(retimed)


def test_unknown_protocol_and_version_are_refused() -> None:
    module = _load_evidence_module()
    evidence = module.sample_evidence()
    evidence["protocol_version"] = 2
    with pytest.raises(module.EvidenceError, match="protocol_version"):
        module.seal(evidence)
    evidence = module.sample_evidence()
    evidence["protocol"] = "SomethingElse"
    with pytest.raises(module.EvidenceError, match="unsupported protocol"):
        module.seal(evidence)


# --------------------------------------------------------------------------
# Identity and verdict refusals
# --------------------------------------------------------------------------


@pytest.mark.parametrize(
    ("mutate", "match"),
    [
        (lambda e: e["source"].update({"dirty": True}), "dirty_paths_digest"),
        (lambda e: e["host"].update({"policy": "canonical-linux"}), "canonical-linux"),
        (lambda e: e["verdict"].update({"scope": "performance"}), "exclusive host lease"),
        (
            lambda e: e["command"].update({"status": "timeout", "exit_code": None}),
            "cannot carry verdict",
        ),
        (lambda e: e["verdict"].update({"status": "not_run"}), "must state a reason"),
        (lambda e: e.update({"run_id": "latest"}), "invalid run id"),
        (lambda e: e["raw"][0].update({"path": "../escape.json"}), "escapes the run root"),
    ],
)
def test_identity_and_verdict_mutations_are_refused(mutate, match: str) -> None:
    module = _load_evidence_module()
    evidence = module.sample_evidence()
    mutate(evidence)
    with pytest.raises(module.EvidenceError, match=match):
        module.seal(evidence)


# --------------------------------------------------------------------------
# Payload confusion refusals
# --------------------------------------------------------------------------


def _retrieval_payload(**overrides):
    payload = {
        "kind": "retrieval",
        "lane": "native_default",
        "metric_space": "file",
        "judgments": "pooled",
        "unjudged": 0,
        "rows": [
            {
                "query_id": "q1",
                "metric": "recall@20",
                "unit": "ratio",
                "value": 0.5,
                "state": "judged",
            }
        ],
        "universe_attested": False,
        "corpus_digest": "sha256:" + "ab" * 32,
        "query_pack_digest": "sha256:" + "cd" * 32,
    }
    payload.update(overrides)
    return payload


@pytest.mark.parametrize(
    ("payload", "match"),
    [
        (
            {
                "kind": "micro",
                "bench_id": "lq-norm/pipeline",
                "metric": "instructions",
                "unit": "ms",
                "instrumentation": "instructions",
                "statistic": "mean",
                "value": 12.0,
                "iterations": 1,
                "samples": 1,
            },
            "contradicts instrumentation",
        ),
        (_retrieval_payload(metric_space="span", judgments="mechanically_labeled"), "span"),
        (
            _retrieval_payload(
                unjudged=1,
                rows=[
                    {
                        "query_id": "q1",
                        "metric": "recall@20",
                        "unit": "ratio",
                        "value": 0.0,
                        "state": "unjudged",
                    }
                ],
            ),
            "must not carry a score",
        ),
        (
            {
                "kind": "load",
                "arrival": "closed_loop",
                "generator_saturated": False,
                "points": [
                    {
                        "label": "clients-8",
                        "offered_rate": 200.0,
                        "completed_rate": 180.0,
                        "dropped": 0,
                        "timeouts": 0,
                    }
                ],
                "errors": 0,
            },
            "claims an offered_rate",
        ),
        (
            {
                "kind": "agent_outcome",
                "task_count": 1,
                "pair_count": 1,
                "arms": ["A", "B"],
                "excluded_pairs": 0,
                "unknown_pairs": 0,
                "metrics": [],
                "capture": "recorded_unauthenticated",
                "input_digest": "sha256:" + "ef" * 32,
            },
            "exactly \\[A, B, C\\]",
        ),
        (
            {
                "kind": "recorded_experiment",
                "experiment_id": "scan-vs-index",
                "diagnostic_only": False,
                "points": [{"label": "2000", "metric": "scan_ms", "unit": "ms", "value": 1.0}],
                "source_digest": "sha256:" + "ef" * 32,
            },
            "diagnostic_only",
        ),
    ],
)
def test_typed_payload_confusion_is_refused(payload, match: str) -> None:
    module = _load_evidence_module()
    evidence = module.sample_evidence()
    evidence["payload"] = payload
    with pytest.raises(module.EvidenceError, match=match):
        module.seal(evidence)


# --------------------------------------------------------------------------
# Immutable run store
# --------------------------------------------------------------------------


def _source(module, staged, data):
    return module.write_raw_file(staged.path.parent.parent / "work" / uuid.uuid4().hex, [data])


def _promote(module, store, evidence) -> dict:
    staged = store.stage(evidence["run_id"])
    staged.write_raw("raw/warm-matrix.json", _source(module, staged, module.SAMPLE_RAW))
    staged.write_evidence(evidence)
    return store.promote(staged)


@pytest.mark.parametrize("operation", ["store", "staging", "prefixed", "hex"])
def test_run_store_verifies_raw_with_bounded_reads(tmp_path, monkeypatch, operation):
    module = _load_evidence_module()
    from tools.ci.lint import handoff_validation

    store = module.RunStore(tmp_path / "store")
    data = b"0123456789abcdef" * 131073
    record = module.sample_evidence()
    staged = store.stage(record["run_id"])
    record["raw"] = [staged.write_raw("raw/payload.bin", _source(module, staged, data))]
    sealed = module.seal(record)
    staged.write_evidence(sealed)
    store.promote(staged)
    consume_file = handoff_validation._consume_repo_regular_file
    reads = []

    class BoundedReader:
        def __init__(self, handle):
            self.handle = handle

        def __getattr__(self, name):
            return getattr(self.handle, name)

        def read(self, size=-1):
            assert 0 < size <= 65536, f"unbounded raw read: {size}"
            reads.append(size)
            return self.handle.read(size)

    def enforce(root, value, *, label, consume):
        return consume_file(
            root,
            value,
            label=label,
            consume=lambda handle: (
                consume(BoundedReader(handle)) if value.endswith("payload.bin") else consume(handle)
            ),
        )

    monkeypatch.setattr(handoff_validation, "_consume_repo_regular_file", enforce)
    expected = hashlib.sha256(data).hexdigest()
    if operation == "staging":
        source = module.RawFile.capture(store.run_dir(record["run_id"]) / "raw/payload.bin")
        copied = store.stage("bounded-copy").write_raw("raw/payload.bin", source)
        assert copied == {
            "path": "raw/payload.bin",
            "bytes": len(data),
            "sha256": "sha256:" + expected,
        }
    elif operation == "store":
        loaded = store.load(record["run_id"])
        assert loaded["raw"] == [
            {
                "path": "raw/payload.bin",
                "bytes": len(data),
                "sha256": "sha256:" + expected,
            }
        ]
    else:
        import evidence_bridge

        path = store.run_dir(record["run_id"]) / "raw/payload.bin"
        actual = (
            evidence_bridge.sha256_file(path)
            if operation == "prefixed"
            else evidence_bridge.sha256_hex_file(path)
        )
        assert actual == ("sha256:" + expected if operation == "prefixed" else expected)
    assert len(reads) > 1


def test_oversize_evidence_is_refused_before_json_decode(tmp_path, monkeypatch):
    module = _load_evidence_module()
    path = tmp_path / "evidence.json"
    with path.open("wb") as handle:
        handle.write(b"{}")
        handle.seek(16 * 1024 * 1024)
        handle.write(b" ")

    def forbidden(_text):
        pytest.fail("oversize control document reached the decoder")

    monkeypatch.setattr(module, "open_evidence", forbidden)
    with pytest.raises(module.EvidenceError, match="control document.*limit"):
        module.read_evidence(path)


@pytest.mark.parametrize("size", [0, 1, 65535, 65536, 65537, 196619])
def test_file_digest_exact_count_and_hash(tmp_path, size):
    module = _load_evidence_module()
    path = tmp_path / "raw"
    data = (bytes(range(256)) * (size // 256 + 1))[:size]
    path.write_bytes(data)
    assert module.file_digest(path) == ("sha256:" + hashlib.sha256(data).hexdigest(), size)


@pytest.mark.parametrize("reader", ["raw", "control", "digest", "seekable", "lines"])
@pytest.mark.parametrize(
    "mutation", ["replace", "grow", "truncate", "restore", "parent", "hardlink"]
)
def test_file_consumers_share_epoch_and_namespace_refusals(tmp_path, monkeypatch, reader, mutation):
    module = _load_evidence_module()
    from tools.ci.lint import handoff_validation

    parent = tmp_path / "parent"
    parent.mkdir()
    path = parent / "payload"
    data = b"original"
    path.write_bytes(data)
    commitment = module.RawFile.capture(path)
    original = handoff_validation._consume_repo_regular_file

    def race(root, value, *, label, consume):
        def mutate(handle):
            result = consume(handle)
            before = path.stat()
            if mutation == "replace":
                other = parent / "replacement"
                other.write_bytes(data)
                other.replace(path)
            elif mutation == "grow":
                path.write_bytes(data + b"extra")
            elif mutation == "truncate":
                path.write_bytes(b"")
            elif mutation == "restore":
                path.write_bytes(b"tampered")
                path.write_bytes(data)
                os.utime(path, ns=(before.st_atime_ns, before.st_mtime_ns))
            elif mutation == "parent":
                parent.rename(tmp_path / "old-parent")
                parent.mkdir()
                path.write_bytes(data)
            else:
                other = tmp_path / "same-bytes"
                other.write_bytes(data)
                path.unlink()
                os.link(other, path)
            return result

        return original(root, value, label=label, consume=mutate)

    monkeypatch.setattr(handoff_validation, "_consume_repo_regular_file", race)
    operation = {
        "raw": module._read_regular_file,
        "control": module._read_control_file,
        "digest": module.file_digest,
        "seekable": lambda _path: commitment.consume_seekable(lambda handle: handle.read(1)),
        "lines": lambda _path: commitment.consume_lines(list),
    }[reader]
    with pytest.raises(module.EvidenceError, match="changed"):
        operation(path)


@pytest.mark.parametrize("raw", [b"", b"one", b"one\n", b"one\r\ntwo\nlast"])
def test_raw_line_reader_preserves_exact_bytes(tmp_path, raw):
    module = _load_evidence_module()
    source = module.write_raw_file(tmp_path / "lines", [raw])
    assert b"".join(source.consume_lines(list, max_line_bytes=5)) == raw


@pytest.mark.parametrize("limit", [0, -1, True, 1.5, 16 * 1024 * 1024 + 1])
def test_raw_line_reader_refuses_invalid_limits(tmp_path, limit):
    module = _load_evidence_module()
    source = module.write_raw_file(tmp_path / "lines", [b"one\n"])
    with pytest.raises(module.EvidenceError, match="line limit"):
        source.consume_lines(list, max_line_bytes=limit)


def test_raw_line_reader_refuses_oversize_before_domain_receives_row(tmp_path):
    module = _load_evidence_module()
    source = module.write_raw_file(tmp_path / "lines", [b"1234\n"])
    observed = []
    with pytest.raises(module.EvidenceError, match="line exceeds"):
        source.consume_lines(lambda lines: observed.extend(lines), max_line_bytes=4)
    assert observed == []


@pytest.mark.parametrize("raw", [b"", b"one", b"one\ntwo\n"])
def test_raw_line_reader_refuses_unconsumed_or_short_circuited_input(tmp_path, raw):
    module = _load_evidence_module()
    source = module.write_raw_file(tmp_path / "lines", [raw])
    with pytest.raises(module.EvidenceError, match="complete file"):
        source.consume_lines(lambda lines: None)
    if raw:
        with pytest.raises(module.EvidenceError, match="complete file"):
            source.consume_lines(next)


@pytest.mark.parametrize("field", ["size", "sha"])
def test_raw_line_reader_refuses_forged_commitment(tmp_path, field):
    module = _load_evidence_module()
    source = module.write_raw_file(tmp_path / "lines", [b"one\ntwo\n"])
    forged = (
        module.RawFile(source.path, "sha256:" + "0" * 64, source.size)
        if field == "sha"
        else (module.RawFile(source.path, source.sha256, source.size + 1))
    )
    with pytest.raises(module.EvidenceError, match="commitment"):
        forged.consume_lines(list)


def test_consumer_binds_the_open_descriptor_to_the_prechecked_file(tmp_path, monkeypatch):
    module = _load_evidence_module()
    from tools.ci.lint import handoff_validation

    path, other = tmp_path / "original", tmp_path / "other"
    path.write_bytes(b"same-bytes")
    other.write_bytes(b"same-bytes")
    original = handoff_validation._consume_repo_regular_file

    def substituted(root, value, *, label, consume):
        # Models opening a replaced inode then restoring the original pathname.
        return original(root, other.relative_to(root).as_posix(), label=label, consume=consume)

    monkeypatch.setattr(handoff_validation, "_consume_repo_regular_file", substituted)
    with pytest.raises(module.EvidenceError, match="before descriptor"):
        module.file_digest(path)


@pytest.mark.parametrize("size", [255, 256, 257])
def test_control_document_limit_is_inclusive(tmp_path, monkeypatch, size):
    module = _load_evidence_module()
    monkeypatch.setattr(module, "CONTROL_DOCUMENT_BYTES", 256)
    path = tmp_path / "control.json"
    path.write_bytes(b" " * size)
    if size > 256:
        with pytest.raises(module.EvidenceError, match="control document.*limit"):
            module._read_control_file(path)
    else:
        assert module._read_control_file(path) == b" " * size


def test_oversize_control_write_does_not_replace_prior_document(tmp_path, monkeypatch):
    module = _load_evidence_module()
    path = tmp_path / "control.json"
    path.write_bytes(b"old")
    monkeypatch.setattr(module, "CONTROL_DOCUMENT_BYTES", 256)
    with pytest.raises(module.EvidenceError, match="control document.*limit"):
        module._write_atomic(path, b" " * 257)
    assert path.read_bytes() == b"old"
    assert list(tmp_path.iterdir()) == [path]


@pytest.mark.parametrize("kind", ["latest", "baseline", "capture-gc", "baseline-gc"])
def test_oversize_custody_document_cannot_authorize_deletion(tmp_path, monkeypatch, kind):
    module = _load_evidence_module()
    store = module.RunStore(tmp_path / "store")
    sealed = module.seal(module.sample_evidence())
    _promote(module, store, sealed)
    monkeypatch.setattr(module, "CONTROL_DOCUMENT_BYTES", 256)
    paths = {
        "latest": store.latest_path,
        "baseline": store.baselines_dir / "family.json",
        "capture-gc": store.root / "captures" / "capture.json",
        "baseline-gc": store.baselines_dir / "family.json",
    }
    path = paths[kind]
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(b" " * 257)
    with pytest.raises(module.EvidenceError, match="control document.*limit"):
        if kind == "latest":
            store.read_latest()
        elif kind == "baseline":
            store.read_baseline("family")
        else:
            store.collect([])
    assert store.run_dir(sealed["run_id"]).is_dir()


@pytest.mark.parametrize("operation", ["digest", "staging"])
def test_streamed_raw_digest_peak_rss_does_not_scale_with_payload(
    tmp_path, record_property, operation
):
    """Fresh process measurements complement the deterministic read-size oracle."""
    script = """
import json, resource, sys
from pathlib import Path
sys.path.insert(0, sys.argv[1])
from evidence import file_digest, RawFile, RunStore
path = Path(sys.argv[2])
if sys.argv[3] == 'staging':
    store = RunStore(path.parent / ('store-' + path.name))
    result = store.stage('copy').write_raw('raw/input.bin', RawFile.capture(path))
    digest, count = result['sha256'], result['bytes']
else:
    digest, count = file_digest(path)
peak = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
if sys.platform != 'darwin':
    peak *= 1024
print(json.dumps(dict(digest=digest, count=count, peak_bytes=peak)))
"""
    measurements = []
    # Sparse zero files avoid reserving a large disk extent. The reader still
    # consumes every logical byte; resource counts are child-process RSS only.
    for size in (8 * 1024 * 1024, 128 * 1024 * 1024):
        path = tmp_path / f"raw-{size}"
        with path.open("wb") as handle:
            handle.truncate(size)
        completed = subprocess.run(
            [sys.executable, "-I", "-c", script, str(EVIDENCE_PATH.parent), str(path), operation],
            capture_output=True,
            text=True,
            check=True,
            timeout=60,
        )
        result = json.loads(completed.stdout)
        expected = hashlib.sha256()
        for _ in range(size // 65536):
            expected.update(bytes(65536))
        assert result["digest"] == "sha256:" + expected.hexdigest()
        assert result["count"] == size
        assert result["peak_bytes"] > 0
        measurements.append(result)
        record_property(f"{operation}_{size}_peak_bytes", result["peak_bytes"])
    # A 120 MiB payload increase must not allocate another payload-sized buffer.
    # Leave 32 MiB for process/allocator variance; this is not host performance
    # admission or a bound for archive packing or the remaining bytes readers.
    assert measurements[1]["peak_bytes"] - measurements[0]["peak_bytes"] < 32 * 1024 * 1024


@pytest.mark.parametrize("mutation", ["sha", "size", "growth", "same-size", "symlink"])
def test_streamed_staging_revalidates_prepared_commitment(tmp_path, mutation):
    module = _load_evidence_module()
    source = module.write_raw_file(tmp_path / "source", [b"original"])
    if mutation == "sha":
        source = module.RawFile(source.path, "sha256:" + "a" * 64, source.size)
    elif mutation == "size":
        source = module.RawFile(source.path, source.sha256, source.size + 1)
    elif mutation == "growth":
        source.path.write_bytes(b"original-extended")
    elif mutation == "same-size":
        source.path.write_bytes(b"modified")
    else:
        other = tmp_path / "replacement"
        source.path.rename(other)
        source.path.symlink_to(other)
    store = module.RunStore(tmp_path / "store")
    staged = store.stage("refused")
    with pytest.raises(module.EvidenceError):
        staged.write_raw("raw/input.bin", source)
    assert not store.run_dir("refused").exists()
    assert store.read_latest() is None


@pytest.mark.parametrize("failure", ["disk-full", "interrupt", "invalid-block"])
def test_raw_spool_retains_partial_output_without_issuing_a_reference(tmp_path, failure):
    module = _load_evidence_module()

    def stream():
        yield b"first block"
        if failure == "disk-full":
            raise OSError(28, "No space left on device")
        if failure == "interrupt":
            raise KeyboardInterrupt("cancel raw spool")
        yield "not bytes"

    expected = KeyboardInterrupt if failure == "interrupt" else module.EvidenceError
    with pytest.raises(expected):
        module.write_raw_file(tmp_path / "partial", stream())
    assert (tmp_path / "partial").read_bytes() == b"first block"


def test_streamed_staging_rejects_short_writes(tmp_path, monkeypatch):
    module = _load_evidence_module()
    source = module.write_raw_file(tmp_path / "source", [b"original"])
    staged = module.RunStore(tmp_path / "store").stage("short")
    original = module.os.fdopen

    class ShortWriter:
        def __init__(self, handle):
            self.handle = handle

        def __getattr__(self, name):
            return getattr(self.handle, name)

        def __enter__(self):
            return self

        def __exit__(self, *args):
            return self.handle.__exit__(*args)

        def write(self, data):
            return self.handle.write(data[: len(data) // 2])

    monkeypatch.setattr(
        module.os,
        "fdopen",
        lambda fd, mode, **kwargs: (
            ShortWriter(original(fd, mode, **kwargs))
            if mode == "wb"
            else original(fd, mode, **kwargs)
        ),
    )
    with pytest.raises(module.EvidenceError, match="short raw output write"):
        staged.write_raw("raw/input.bin", source)
    assert (staged.path / "raw/input.bin").read_bytes() == b"orig"


@pytest.mark.parametrize("operation", ["control", "tail", "concatenate"])
def test_raw_file_consumers_reject_replaced_commitment(tmp_path, operation):
    module = _load_evidence_module()
    reference = module.write_raw_file(tmp_path / "source", [b"original"])
    reference.path.write_bytes(b"tampered")
    with pytest.raises(module.EvidenceError, match="commitment"):
        if operation == "control":
            reference.read_control()
        elif operation == "tail":
            reference.tail(3)
        else:
            with module.RawWriter(tmp_path / "joined") as sink:
                reference.copy_into(sink)


def test_raw_tail_is_bounded_and_validates_all_bytes(tmp_path):
    module = _load_evidence_module()
    reference = module.write_raw_file(tmp_path / "source", [b"prefix", b"known-tail"])
    assert reference.tail(4) == b"tail"
    assert reference.tail(0) == b""
    for limit in (-1, True, 1.0, module.IO_CHUNK_BYTES + 1):
        with pytest.raises(module.EvidenceError, match="bounded"):
            reference.tail(limit)


def test_raw_control_read_refuses_oversize_without_silent_truncation(tmp_path, monkeypatch):
    module = _load_evidence_module()
    reference = module.write_raw_file(tmp_path / "source", [b"123456789"])
    monkeypatch.setattr(module, "CONTROL_DOCUMENT_BYTES", 8)
    with pytest.raises(module.EvidenceError, match="exceeds"):
        reference.read_control()


@pytest.mark.parametrize(
    "relative",
    ["failure.json", "raw/../outside", "raw//alias", "raw/./alias", "raw/a/b", "raw/a\x00b"],
)
def test_staging_does_not_accept_noncanonical_raw_destinations(tmp_path, relative):
    module = _load_evidence_module()
    source = module.write_raw_file(tmp_path / "source", [b"oracle"])
    staged = module.RunStore(tmp_path / "store").stage("bad-path")
    with pytest.raises(module.EvidenceError):
        staged.write_raw(relative, source)
    assert list(staged.path.iterdir()) == []


def test_failed_staging_cannot_be_promoted_even_with_valid_raw_and_envelope(tmp_path):
    module = _load_evidence_module()
    store = module.RunStore(tmp_path / "store")
    sealed = module.seal(module.sample_evidence())
    staged = store.stage(sealed["run_id"])
    staged.write_raw("raw/warm-matrix.json", _source(module, staged, module.SAMPLE_RAW))
    staged.write_evidence(sealed)
    staged.mark_failed(KeyboardInterrupt("canceled"))
    with pytest.raises(module.EvidenceError, match="failed staging epoch"):
        store.promote(staged)
    assert store.read_latest() is None
    assert (staged.path / "raw/warm-matrix.json").read_bytes() == module.SAMPLE_RAW


def test_run_store_refuses_same_bytes_raw_symlink_swap_after_file_check(tmp_path, monkeypatch):
    module = _load_evidence_module()
    store = module.RunStore(tmp_path / "store")
    sealed = module.seal(module.sample_evidence())
    _promote(module, store, sealed)
    path = store.run_dir(sealed["run_id"]) / "raw/warm-matrix.json"
    replacement = tmp_path / "same-bytes"
    replacement.write_bytes(module.SAMPLE_RAW)
    original = Path.is_file
    swapped = False

    def race(candidate):
        nonlocal swapped
        result = original(candidate)
        if candidate == path and not swapped:
            swapped = True
            path.unlink()
            path.symlink_to(replacement)
        return result

    monkeypatch.setattr(Path, "is_file", race)
    with pytest.raises(module.EvidenceError, match="symlink"):
        store.load(sealed["run_id"])
    assert swapped


def test_run_store_refuses_linked_evidence_with_identical_valid_bytes(tmp_path):
    module = _load_evidence_module()
    store = module.RunStore(tmp_path / "store")
    sealed = module.seal(module.sample_evidence())
    _promote(module, store, sealed)
    path = store.run_dir(sealed["run_id"]) / "evidence.json"
    replacement = tmp_path / "same-valid-evidence"
    path.rename(replacement)
    path.symlink_to(replacement)
    with pytest.raises(module.EvidenceError, match="symlink"):
        store.load(sealed["run_id"])


@pytest.mark.parametrize("pointer", ["latest", "baseline"])
def test_run_store_absent_optional_pointer_is_none(tmp_path, pointer):
    module = _load_evidence_module()
    store = module.RunStore(tmp_path / "absent-store")
    value = store.read_latest() if pointer == "latest" else store.read_baseline("family")
    assert value is None


@pytest.mark.parametrize("pointer", ["latest", "baseline"])
def test_run_store_dangling_optional_pointer_is_refused(tmp_path, pointer):
    module = _load_evidence_module()
    store = module.RunStore(tmp_path / "store")
    path = store.latest_path if pointer == "latest" else store.baselines_dir / "family.json"
    path.parent.mkdir(parents=True)
    path.symlink_to(tmp_path / "absent-target")
    with pytest.raises(module.EvidenceError, match="symlink"):
        store.read_latest() if pointer == "latest" else store.read_baseline("family")


@pytest.mark.parametrize("pointer", ["latest", "baseline"])
def test_run_store_absent_pointer_under_linked_ancestor_is_refused(tmp_path, pointer):
    module = _load_evidence_module()
    target = tmp_path / "target"
    target.mkdir()
    linked = tmp_path / "linked-store"
    linked.symlink_to(target, target_is_directory=True)
    store = module.RunStore(linked)
    with pytest.raises(module.EvidenceError, match="unsafe optional benchmark pointer"):
        store.read_latest() if pointer == "latest" else store.read_baseline("family")


def test_run_store_promotes_replays_and_retains_baselines(tmp_path: Path) -> None:
    module = _load_evidence_module()
    store = module.RunStore(tmp_path)
    sealed = module.seal(module.sample_evidence())
    promotion = _promote(module, store, sealed)
    assert promotion["digest"] == sealed["digest"]
    assert store.load(sealed["run_id"]) == sealed
    assert store.read_latest()["run_id"] == sealed["run_id"]
    assert store.admit_baseline("dsl-warm", sealed["run_id"], 10_000, "blocked-paired")["run_id"]
    assert store.collect([]) == []
    assert store.load(sealed["run_id"])["digest"] == sealed["digest"]


def test_run_store_refuses_a_repeated_run_id(tmp_path: Path) -> None:
    module = _load_evidence_module()
    store = module.RunStore(tmp_path)
    sealed = module.seal(module.sample_evidence())
    _promote(module, store, sealed)
    with pytest.raises(module.EvidenceError, match="already exists"):
        store.stage(sealed["run_id"])


def test_run_store_refuses_existing_staging_without_erasing_first_writer(tmp_path: Path) -> None:
    module = _load_evidence_module()
    store = module.RunStore(tmp_path)
    run_id = module.sample_evidence()["run_id"]
    first = store.stage(run_id)
    first.write_raw("raw/warm-matrix.json", _source(module, first, module.SAMPLE_RAW))
    with pytest.raises(module.EvidenceError, match="staging"):
        store.stage(run_id)
    assert (first.path / "raw/warm-matrix.json").read_bytes() == module.SAMPLE_RAW


def test_run_store_refuses_linked_staging_parent_and_dangling_run_id(tmp_path: Path) -> None:
    module = _load_evidence_module()
    root = tmp_path / "store"
    root.mkdir()
    store = module.RunStore(root)
    run_id = module.sample_evidence()["run_id"]
    outside = tmp_path / "outside"
    outside.mkdir()
    store.staging_dir.symlink_to(outside, target_is_directory=True)
    with pytest.raises(module.EvidenceError, match="symlink"):
        store.stage(run_id)
    assert not (outside / run_id).exists()
    store.staging_dir.unlink()
    store.runs_dir.mkdir()
    store.run_dir(run_id).symlink_to(tmp_path / "missing-run", target_is_directory=True)
    with pytest.raises(module.EvidenceError, match="already exists"):
        store.stage(run_id)


def test_collect_refuses_linked_runs_parent_without_deleting_outside(tmp_path: Path) -> None:
    module = _load_evidence_module()
    root = tmp_path / "store"
    root.mkdir()
    outside = tmp_path / "outside"
    victim = outside / "unreferenced"
    victim.mkdir(parents=True)
    marker = victim / "retain-me"
    marker.write_text("user data")
    (root / "runs").symlink_to(outside, target_is_directory=True)
    with pytest.raises(module.EvidenceError, match="symlink"):
        module.RunStore(root).collect([])
    assert marker.read_text() == "user data"


def test_baseline_admission_refuses_linked_parent_without_writing_outside(tmp_path: Path) -> None:
    module = _load_evidence_module()
    root = tmp_path / "store"
    store = module.RunStore(root)
    sealed = module.seal(module.sample_evidence())
    _promote(module, store, sealed)
    outside = tmp_path / "outside"
    outside.mkdir()
    store.baselines_dir.symlink_to(outside, target_is_directory=True)

    with pytest.raises(module.EvidenceError, match="symlink"):
        store.admit_baseline("dsl-warm", sealed["run_id"], 10_000, "blocked-paired")
    assert not (outside / "dsl-warm.json").exists()


def test_staged_raw_write_refuses_linked_parent_without_writing_outside(tmp_path: Path) -> None:
    module = _load_evidence_module()
    staged = module.RunStore(tmp_path / "store").stage(module.sample_evidence()["run_id"])
    outside = tmp_path / "outside"
    outside.mkdir()
    (staged.path / "raw").symlink_to(outside, target_is_directory=True)

    with pytest.raises(module.EvidenceError, match="symlink"):
        staged.write_raw("raw/warm-matrix.json", _source(module, staged, module.SAMPLE_RAW))
    assert not (outside / "warm-matrix.json").exists()


def test_staged_raw_write_refuses_existing_hardlink_without_clobber(tmp_path: Path) -> None:
    module = _load_evidence_module()
    staged = module.RunStore(tmp_path / "store").stage(module.sample_evidence()["run_id"])
    outside = tmp_path / "outside"
    outside.write_bytes(b"retain")
    (staged.path / "raw").mkdir()
    os.link(outside, staged.path / "raw" / "warm-matrix.json")

    with pytest.raises(module.EvidenceError, match="unsafe or incomplete raw output"):
        staged.write_raw("raw/warm-matrix.json", _source(module, staged, module.SAMPLE_RAW))
    assert outside.read_bytes() == b"retain"


def test_crash_before_promotion_leaves_no_admissible_run(tmp_path: Path) -> None:
    module = _load_evidence_module()
    store = module.RunStore(tmp_path)
    sealed = module.seal(module.sample_evidence())
    staged = store.stage(sealed["run_id"])
    staged.write_raw("raw/warm-matrix.json", _source(module, staged, module.SAMPLE_RAW))
    staged.write_evidence(sealed)
    with pytest.raises(module.EvidenceError, match="missing run directory"):
        store.load(sealed["run_id"])
    assert store.read_latest() is None


def test_missing_extra_and_tampered_raw_files_are_refused(tmp_path: Path) -> None:
    module = _load_evidence_module()
    sealed = module.seal(module.sample_evidence())

    store = module.RunStore(tmp_path / "missing")
    staged = store.stage(sealed["run_id"])
    staged.write_evidence(sealed)
    with pytest.raises(module.EvidenceError, match="is missing"):
        store.promote(staged)

    store = module.RunStore(tmp_path / "extra")
    staged = store.stage(sealed["run_id"])
    staged.write_raw("raw/warm-matrix.json", _source(module, staged, module.SAMPLE_RAW))
    staged.write_raw("raw/undeclared.json", _source(module, staged, b"surprise"))
    staged.write_evidence(sealed)
    with pytest.raises(module.EvidenceError, match="undeclared raw file"):
        store.promote(staged)

    store = module.RunStore(tmp_path / "tampered")
    staged = store.stage(sealed["run_id"])
    staged.write_raw(
        "raw/warm-matrix.json", _source(module, staged, module.SAMPLE_RAW.replace(b"0.42", b"0.43"))
    )
    staged.write_evidence(sealed)
    with pytest.raises(module.EvidenceError, match="digest mismatch"):
        store.promote(staged)


def test_symlinked_raw_reference_is_refused(tmp_path: Path) -> None:
    module = _load_evidence_module()
    store = module.RunStore(tmp_path)
    sealed = module.seal(module.sample_evidence())
    staged = store.stage(sealed["run_id"])
    (staged.path / "raw").mkdir(parents=True)
    target = tmp_path / "outside.json"
    target.write_bytes(module.SAMPLE_RAW)
    (staged.path / "raw" / "warm-matrix.json").symlink_to(target)
    staged.write_evidence(sealed)
    with pytest.raises(module.EvidenceError, match="symlink"):
        store.promote(staged)


def test_collect_removes_only_unreferenced_runs(tmp_path: Path) -> None:
    module = _load_evidence_module()
    store = module.RunStore(tmp_path)
    sealed = module.seal(module.sample_evidence())
    _promote(module, store, sealed)
    store.admit_baseline("dsl-warm", sealed["run_id"], 10_000, "blocked-paired")

    other = module.sample_evidence()
    other["run_id"] = "run-20260926T130000Z-deadbeef"
    other = module.seal(other)
    _promote(module, store, other)

    assert store.collect([]) == ["run-20260926T130000Z-deadbeef"]
    assert store.load(sealed["run_id"])["digest"] == sealed["digest"]


def test_published_wire_schema_accepts_the_golden_and_refuses_malformed_documents() -> None:
    """The canonical wire schema must agree with the sealed golden document."""
    import jsonschema

    schema = json.loads(
        (REPO_ROOT / "tools" / "benchmark" / "evidence.schema.json").read_text(encoding="utf-8")
    )
    jsonschema.Draft202012Validator.check_schema(schema)
    validator = jsonschema.Draft202012Validator(schema)
    golden = json.loads((FIXTURE_DIR / "sample-evidence.json").read_text(encoding="utf-8"))
    validator.validate(golden)

    # The schema is a projection: the Rust/Python validators remain authoritative
    # for cross-field rules. Here it must reject structural corruption.
    broken = json.loads(json.dumps(golden))
    broken["protocol_version"] = 2
    assert list(validator.iter_errors(broken)), "schema accepted an unknown protocol version"

    broken = json.loads(json.dumps(golden))
    broken["payload"] = {"kind": "latency", "rows": [], "errors": 0, "timeouts": 0, "drops": 0}
    assert list(validator.iter_errors(broken)), "schema accepted an empty latency payload"

    broken = json.loads(json.dumps(golden))
    broken["payload"] = {
        "kind": "micro",
        "bench_id": "x",
        "metric": "mean",
        "unit": "bogus",
        "instrumentation": "wall",
        "statistic": "mean",
        "value": 1.0,
        "iterations": 1,
        "samples": 1,
    }
    assert list(validator.iter_errors(broken)), "schema accepted an unregistered unit"


def test_proof_counts_are_not_relevance_or_performance():
    module = _load_evidence_module()
    record = module.sample_evidence()
    record["verdict"]["scope"] = "contract"
    record["payload"] = {
        "kind": "proof",
        "rail": "retrieval-contract",
        "selected": 8,
        "executed": 8,
        "passed": 8,
        "failed": 0,
        "source_digest": record["source"]["closure_digest"],
        "execution_context_digest": record["source"]["closure_digest"],
    }
    sealed = module.seal(record)
    assert module.open_evidence(module.to_canonical_json(sealed)) == sealed
    record["verdict"]["scope"] = "quality"
    with pytest.raises(ValueError):
        module.seal(record)
    record["verdict"]["scope"] = "contract"
    record["payload"]["executed"] = 7
    with pytest.raises(ValueError):
        module.seal(record)
    record["payload"].update(executed=8, passed=7, failed=1)
    with pytest.raises(ValueError):
        module.seal(record)
    record["verdict"].update(status="fail", reason="one failed test")
    assert module.seal(record)
    record["payload"].update(passed=2**64 - 1, failed=1)
    with pytest.raises(ValueError):
        module.seal(record)
