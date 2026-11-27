"""Explicit candidate symbol capture stays separate from vendor qualification."""

import hashlib
import json
import platform
from contextlib import contextmanager
from pathlib import Path

import pytest

from tools.benchmark.retrieval import query_plan, run, source_oracle, symbol_coverage, ts_projection


def _candidate(tmp_path: Path) -> tuple[Path, str]:
    path = tmp_path / "candidate.json"
    path.write_text(
        json.dumps(
            {
                "schema": "quanta-symbol-candidate-artifacts-v1",
                "grammar_identity": "registry-go@0.25.0;registry-checksum=test",
            }
        ),
        encoding="utf-8",
    )
    return path, hashlib.sha256(path.read_bytes()).hexdigest()


def _spec(tmp_path: Path) -> dict:
    return {
        "spec_version": 2,
        "repo": "repo",
        "manifest": "manifest",
        "suite": "suite",
        "query_pack": "pack",
        "execution_profiles": {"quanta": query_plan.execution_profile("native")},
        "top_k": 1,
        "output_root": "out",
        "runner_binary": "runner",
        "strategies": [{"name": "whole_file"}],
        "searchd_binary": "searchd",
        "searchd_expected_sha256": "a" * 64,
        "scope": "exploratory",
    }


def test_candidate_spec_is_pinned_and_pair_requires_independent_census(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    path, sha = _candidate(tmp_path)
    spec = _spec(tmp_path)
    spec.update(symbol_candidate_manifest=str(path), symbol_candidate_manifest_sha256=sha)
    spec_path = tmp_path / "spec.json"

    def load(*, standalone: bool = True) -> dict:
        spec_path.write_text(json.dumps(spec), encoding="utf-8")
        return run.load_spec(spec_path, standalone_quanta=standalone)

    assert load() == spec
    identity = symbol_coverage.candidate_grammar_identity(path, sha)
    assert identity.startswith("registry-go@0.25.0;")
    assert identity.endswith(";artifact-manifest-sha256=" + sha)
    with pytest.raises(run.RunError, match="candidate symbol manifest is invalid"):
        load(standalone=False)
    seen = []
    monkeypatch.setattr(
        run.source_oracle,
        "validate_candidate_manifest",
        lambda manifest, digest: seen.append((manifest, digest)),
    )
    assert load(standalone=False) == spec
    assert seen == [(path, sha)]
    spec["claims"] = {"quality": True}
    with pytest.raises(run.RunError, match="standalone exploratory only"):
        load()
    assert load(standalone=False) == spec
    spec.pop("claims")
    spec["symbol_candidate_manifest_sha256"] = "b" * 64
    with pytest.raises(run.RunError, match="differs from pinned SHA-256"):
        load()
    spec.pop("symbol_candidate_manifest_sha256")
    with pytest.raises(run.RunError, match="requires its pinned SHA-256"):
        load()


@pytest.mark.parametrize("name", ["a.go", "a.js", "a.jsx", "a.ts", "a.tsx", "a.mjs", "a.cts"])
def test_affected_source_requires_candidate_manifest(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, name: str
) -> None:
    spec = _spec(tmp_path)
    monkeypatch.setattr(
        run, "read_json", lambda path: {"files": [{"path": name}]} if path.name == "manifest" else {}
    )
    with pytest.raises(run.RunError, match="require a pinned candidate manifest"):
        run._require_candidate_for_admitted_sources(spec)
    path, sha = _candidate(tmp_path)
    spec.update(symbol_candidate_manifest=str(path), symbol_candidate_manifest_sha256=sha)
    run._require_candidate_for_admitted_sources(spec)


def test_rust_python_only_source_does_not_require_candidate_manifest(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(
        run,
        "read_json",
        lambda path: {"files": [{"path": "a.rs"}, {"path": "b.py"}]}
        if path.name == "manifest" else {},
    )
    run._require_candidate_for_admitted_sources(_spec(tmp_path))


def test_qualified_census_covers_every_affected_file_with_reused_sessions(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    path, sha = _candidate(tmp_path)
    inputs = {
        "a.go": b"package x\nfunc A() {}\n",
        "b.js": b"function b() {}\n",
        "c.tsx": b"function C() { return <x />; }\n",
    }
    admitted = [
        {"path": name, "file_sha256": hashlib.sha256(raw).hexdigest()}
        for name, raw in inputs.items()
    ] + [{"path": "d.rs", "file_sha256": "0" * 64}]
    manifest_path = tmp_path / "corpus.json"
    manifest_path.write_text(json.dumps({"repository_commit": "a" * 40, "files": admitted}))
    suite = {"repository_commit": "a" * 40, "file_universe": admitted[:1]}
    sessions: list[str] = []
    visited: list[str] = []

    @contextmanager
    def session(language, *, candidate_manifest_path, candidate_manifest_sha256):
        assert candidate_manifest_path == path
        assert candidate_manifest_sha256 == sha
        sessions.append(language)

        def census(name, raw):
            assert raw == inputs[name]
            visited.append(name)
            return [(0, 1, 0, len(raw), "function_declaration")]

        yield census

    class Source:
        def file(self, name):
            raw = inputs[name]
            return raw, [], hashlib.sha256(raw).hexdigest()

    monkeypatch.setattr(run.source_oracle, "census_parser_identity", lambda **_kwargs: "p" * 64)
    monkeypatch.setattr(run.source_oracle, "declaration_census_session", session)
    result = run._independent_candidate_census(
        {"manifest": str(manifest_path), "symbol_candidate_manifest": str(path),
         "symbol_candidate_manifest_sha256": sha},
        suite, Source(),
    )
    assert sessions == ["go", "javascript", "typescript"]
    assert visited == ["a.go", "b.js", "c.tsx"]
    assert [row["path"] for row in result["files"]] == visited
    assert all(row["declaration_count"] == 1 for row in result["files"])
    assert result["parser_identity"] == "p" * 64
    assert result["candidate_manifest_sha256"] == sha


def test_qualified_census_refuses_missing_pin_source_drift_and_parse_failure(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    raw = b"package x\nfunc A() {}\n"
    entry = {"path": "a.go", "file_sha256": hashlib.sha256(raw).hexdigest()}
    manifest_path = tmp_path / "corpus.json"
    manifest_path.write_text(json.dumps({"repository_commit": "a" * 40, "files": [entry]}))
    suite = {"repository_commit": "a" * 40, "file_universe": [entry]}
    with pytest.raises(run.RunError, match="require a pinned candidate manifest"):
        run._independent_candidate_census({"manifest": str(manifest_path)}, suite, object())

    path, sha = _candidate(tmp_path)
    spec = {"manifest": str(manifest_path), "symbol_candidate_manifest": str(path),
            "symbol_candidate_manifest_sha256": sha}
    monkeypatch.setattr(run.source_oracle, "census_parser_identity", lambda **_kwargs: "p" * 64)

    @contextmanager
    def session(_language, **_kwargs):
        def failed(_name, _raw):
            raise source_oracle.SourceOracleError("source oracle parse error")
        yield failed

    monkeypatch.setattr(run.source_oracle, "declaration_census_session", session)

    class Source:
        def __init__(self, observed_sha):
            self.observed_sha = observed_sha

        def file(self, _name):
            return raw, [], self.observed_sha

    with pytest.raises(run.RunError, match="source bytes differ"):
        run._independent_candidate_census(spec, suite, Source("0" * 64))
    with pytest.raises(source_oracle.SourceOracleError, match="parse error"):
        run._independent_candidate_census(spec, suite, Source(hashlib.sha256(raw).hexdigest()))
    manifest_path.write_text(json.dumps({"repository_commit": "a" * 40, "files": [
        {"path": "d.rs", "file_sha256": "0" * 64}
    ]}))
    assert run._independent_candidate_census(spec, suite, object()) is None


def test_pair_freezes_pinned_candidate_manifest(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    path, sha = _candidate(tmp_path)
    spec = _spec(tmp_path)
    for key in ("suite", "query_pack", "manifest", "semble_lockfile", "host_profile"):
        source = tmp_path / f"{key}.json"
        source.write_text("{}", encoding="utf-8")
        spec[key] = str(source)
    spec.update(symbol_candidate_manifest=str(path), symbol_candidate_manifest_sha256=sha)
    monkeypatch.setattr(run, "validate_host_profile", lambda _value: None)
    monkeypatch.setattr(run.source_oracle, "validate_candidate_manifest", lambda _path, _sha: None)
    stage = tmp_path / "stage"
    stage.mkdir()
    frozen = run.freeze_inputs(spec, stage)
    frozen_candidate = Path(frozen["symbol_candidate_manifest"])
    assert frozen_candidate == stage / "symbol-candidate-manifest.json"
    assert hashlib.sha256(frozen_candidate.read_bytes()).hexdigest() == sha
    path.write_text("{}", encoding="utf-8")
    assert hashlib.sha256(frozen_candidate.read_bytes()).hexdigest() == sha
    with pytest.raises(run.RunError, match="changed before freeze"):
        run.freeze_inputs(spec, stage)


def test_archived_manifest_replays_against_original_pinned_artifacts(tmp_path: Path) -> None:
    installed = (tmp_path / "installed").resolve()
    archived = (tmp_path / "archive").resolve()
    installed.mkdir()
    archived.mkdir()
    artifacts = {}
    for name in ("official_binary", "syntax_batch_binary", "go_helper", "node", "typescript_js"):
        path = installed / name
        path.write_bytes(name.encode())
        artifacts[name] = {
            "path": str(path),
            "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
        }
    manifest = {
        "schema": "quanta-symbol-candidate-artifacts-v1",
        "platform": {"Darwin": "macos", "Linux": "linux"}[platform.system()],
        "architecture": {"arm64": "aarch64"}.get(platform.machine(), platform.machine()),
        "grammar_identity": "registry-syntax@0.25.0",
        "go_version": "go1.25.4",
        "node_version": "v1.0.0",
        "typescript_version": "5.9.3",
        "script_sha256": hashlib.sha256(ts_projection.SOURCE.read_bytes()).hexdigest(),
        **artifacts,
        "node_libraries": [],
    }
    raw = json.dumps(manifest, sort_keys=True).encode()
    (installed / "candidate.json").write_bytes(raw)
    archived_path = archived / "symbol-candidate-manifest.json"
    archived_path.write_bytes(raw)
    pinned_sha = hashlib.sha256(raw).hexdigest()
    assert source_oracle.validate_candidate_manifest(archived_path, pinned_sha) == manifest
    (installed / "go_helper").write_bytes(b"changed")
    with pytest.raises(source_oracle.SourceOracleError, match="artifact bytes differ"):
        source_oracle.validate_candidate_manifest(archived_path, pinned_sha)


def test_quanta_strategy_forwards_candidate_manifest_before_process(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    path, sha = _candidate(tmp_path)
    pack = tmp_path / "pack.json"
    pack.write_text('{"tasks": []}', encoding="utf-8")
    spec = _spec(tmp_path)
    spec.update(symbol_candidate_manifest=str(path), symbol_candidate_manifest_sha256=sha)
    seen = []

    class CapturedCommand(Exception):
        pass

    def capture(command, **_kwargs):
        seen.extend(command)
        raise CapturedCommand

    monkeypatch.setattr(run, "run_monitored_process", capture)
    with pytest.raises(CapturedCommand):
        run.run_quanta_strategy(
            spec, spec["strategies"][0], 0, tmp_path / "out", ["lexical"], pack, "c" * 64
        )
    assert seen[seen.index("--symbol-candidate-manifest") + 1] == str(path)
    assert seen[seen.index("--symbol-candidate-manifest-sha256") + 1] == sha


def test_candidate_policy_digest_differs_and_rejects_unpinned_identity(tmp_path: Path) -> None:
    path, sha = _candidate(tmp_path)
    identity = symbol_coverage.candidate_grammar_identity(path, sha)
    policy = {
        "max_file_bytes": 1,
        "max_symbols_per_file": 1,
        "max_symbols_total": 1,
        "max_diagnostics_per_file": 1,
        "max_diagnostics_total": 1,
        "timeout_per_file_ns": "1",
        "timeout_total_ns": "2",
    }
    vendor = symbol_coverage.policy_digest(policy)
    candidate = symbol_coverage.policy_digest(
        policy, candidate_identity=identity, candidate_manifest_sha256=sha
    )
    assert candidate != vendor
    with pytest.raises(ValueError, match="differs from pinned manifest"):
        symbol_coverage.policy_digest(
            policy, candidate_identity=identity, candidate_manifest_sha256="b" * 64
        )


def test_preflight_artifact_accepts_only_selected_candidate_policy(tmp_path: Path) -> None:
    path, sha = _candidate(tmp_path)
    identity = symbol_coverage.candidate_grammar_identity(path, sha)
    policy = {
        "max_file_bytes": 1024,
        "max_symbols_per_file": 10,
        "max_symbols_total": 10,
        "max_diagnostics_per_file": 1,
        "max_diagnostics_total": 1,
        "timeout_per_file_ns": "1000000",
        "timeout_total_ns": "120000000000",
    }
    source_sha = "a" * 64
    row = {
        "path": "a.go",
        "source_sha256": source_sha,
        "language": "go",
        "coverage": {"state": "complete", "symbol_count": 0},
        "failure": None,
        "failure_detail": None,
        "failure_detail_truncated": False,
        "diagnostics": [],
        "diagnostics_total": 0,
        "diagnostics_truncated": False,
        "diagnostics_complete": True,
    }
    report = {
        "schema": "symbol-preflight-v1",
        "producer_identity": symbol_coverage.PRODUCER,
        "grammar_identity": identity,
        "lockfile_sha256": hashlib.sha256(
            (symbol_coverage.ROOT / "Cargo.lock").read_bytes()
        ).hexdigest(),
        "producer_policy_sha256": symbol_coverage.policy_digest(
            policy, candidate_identity=identity, candidate_manifest_sha256=sha
        ),
        "policy": policy,
        "files": [row],
        "admitted_files": 1,
        "incomplete_files": 0,
    }
    corpus = {"repository_commit": "b" * 40, "files": [{"path": "a.go", "file_sha256": source_sha}]}
    envelope = {
        "symbol_coverage_policy": "require-complete",
        "repository_commit": corpus["repository_commit"],
        "file_universe_sha256": hashlib.sha256(b"a.go\0" + source_sha.encode() + b"\0").hexdigest(),
        "preflight": report,
    }
    artifact = tmp_path / "symbol-preflight.json"
    artifact.write_text(json.dumps(envelope), encoding="utf-8")
    metrics = {
        "symbol_producer_identity": symbol_coverage.PRODUCER,
        "symbol_grammars": identity,
        "symbol_count": 0,
        "file_count": 1,
        "symbol_coverage_policy": "require-complete",
        "symbol_preflight_sha256": hashlib.sha256(artifact.read_bytes()).hexdigest(),
        "symbol_producer_policy_sha256": report["producer_policy_sha256"],
        "symbol_preflight_out": artifact.name,
        "symbol_coverage": [
            {**{key: row[key] for key in symbol_coverage.ROW_KEYS}, "definition_count": 0}
        ],
        "symbol_incomplete_files": 0,
        "symbol_unsupported_files": 0,
        "symbol_only_scopes": 0,
        "empty_scopes": 0,
        "symbol_unsupported_details": [],
    }
    assert (
        symbol_coverage.verify_artifact(
            metrics,
            tmp_path / "phase.json",
            corpus,
            candidate_identity=identity,
            candidate_manifest_sha256=sha,
            expected_timeout_total_ms=120_000,
        )
        == artifact
    )
    with pytest.raises(ValueError, match="invalid symbol producer evidence"):
        symbol_coverage.verify_artifact(metrics, tmp_path / "phase.json", corpus)
    with pytest.raises(ValueError, match="candidate policy identity differs"):
        symbol_coverage.verify_artifact(
            metrics,
            tmp_path / "phase.json",
            corpus,
            candidate_identity=identity,
            candidate_manifest_sha256="c" * 64,
        )
