"""Provider-free custody tests for the archived OpenAI A/B capture adapter."""

from __future__ import annotations

import json
import math
import os
import subprocess
import sys
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(REPO_ROOT / "tools/benchmark"))

import relevance_openai_ab_capture as ab_capture  # noqa: E402
from evidence import CONTROL_DOCUMENT_BYTES, EvidenceError, RawFile  # noqa: E402
from relevance_openai_ab_capture import (  # noqa: E402
    _ARCHIVED_JUDGED_SOURCE,
    _JUDGED_COLUMNS,
    _JUDGED_SOURCE,
    APPROVED_AB_INPUT_DIGEST,
    LEGACY_DAEMON_SHA256,
    OPENAI_COST_CEILING,
    OPENAI_REQUEST_COST,
    _approved_input_digest,
    _config_digest,
    _judgments_digest,
    _source_cases,
    capture_detached_ab,
    validate_capture,
    validate_frozen_ab,
)

CASE_IDS = (
    "sem.refresh_auth_token.exact",
    "sem.evict_cache.paraphrase",
    "sem.retry_backoff.paraphrase",
    "sem.dedup.paraphrase",
    "sem.rate_limit.paraphrase",
    "sem.serialize.paraphrase",
    "sem.pagination.paraphrase",
    "sem.graceful_shutdown.paraphrase",
    "sem.memoize.paraphrase",
    "sem.pool_checkout.paraphrase",
    "sem.sanitize.paraphrase",
    "sem.debounce.paraphrase",
    "sem.backpressure.paraphrase",
)
APPROVED_GRANT = {
    "QUANTA_INDEX_PROVIDER_TENANT": "test-tenant",
    "QUANTA_INDEX_PROVIDER_ENDPOINT": "https://api.openai.com/v1/embeddings",
    "QUANTA_INDEX_PROVIDER_REGION": "test-region",
    "QUANTA_INDEX_PROVIDER_RETENTION": "test-retention",
    "QUANTA_INDEX_PROVIDER_PROFILE": "test-profile",
    "QUANTA_INDEX_PROVIDER_SOURCE_CONTENT_CONSENT": "true",
}
APPROVED_GRANT_DIGEST = "sha256:a2dfe79c27ae6031876d4622d441e40b7d0c6f3a88778ca051fa6ac14c0664ad"


def _git(*args: str, cwd: Path) -> str:
    return subprocess.run(
        ["git", *args], cwd=cwd, check=True, capture_output=True, text=True
    ).stdout.strip()


def _source_checkout(root: Path, *, archived: bool = False) -> tuple[Path, str]:
    checkout = root / "historical-source"
    checkout.mkdir()
    _git("init", "-q", cwd=checkout)
    _git("config", "user.email", "fixture@example.invalid", cwd=checkout)
    _git("config", "user.name", "Fixture", cwd=checkout)
    (checkout / "source.txt").write_text("frozen test source\n")
    fixture_path = _ARCHIVED_JUDGED_SOURCE if archived else _JUDGED_SOURCE
    fixture = checkout / fixture_path
    fixture.parent.mkdir(parents=True)
    cases = _fixture("0" * 40)["cases.json"]["cases"]
    if archived:
        kinds = {"exact_token": "ExactToken", "paraphrase": "Paraphrase"}
        blocks = [
            "    SemanticJudgedQuery {\n"
            f"        id: {json.dumps(case['id'])},\n"
            f"        intent_kind: SemanticIntentKind::{kinds[case['intent_kind']]},\n"
            + "".join(
                f"        {field}: {json.dumps(case[field])},\n" for field in _JUDGED_COLUMNS[2:]
            )
            + "    },\n"
            for case in cases
        ]
        fixture.write_text(
            "pub const SEMANTIC_JUDGED_QUERIES: &[SemanticJudgedQuery] = &[\n"
            + "".join(blocks)
            + "];\n"
        )
    else:
        fixture.write_text(
            "\t".join(_JUDGED_COLUMNS)
            + "\n"
            + "".join("\t".join(case[field] for field in _JUDGED_COLUMNS) + "\n" for case in cases)
        )
    _git("add", "source.txt", fixture_path, cwd=checkout)
    _git("commit", "-qm", "fixture", cwd=checkout)
    return checkout, _git("rev-parse", "HEAD", cwd=checkout)


def _fixture(head: str) -> dict[str, dict]:
    runner_config = {
        "schema_version": 1,
        "hash": {"dimension": 64, "model_revision": "hash-dev@test:d64"},
        "openai": {
            "model": "fixture-model",
            "model_revision": "fixture-revision",
            "dimension": 1536,
            "max_batch": 100,
            "max_estimated_tokens_per_request": 8192,
            "max_retries": 2,
            "timeout_secs": 30,
            "timeout_subsec_nanos": 0,
            "cache_enabled": True,
            "cache_retention": {
                "max_entries": 500_000,
                "max_resident_bytes": 2 * 1024**3,
                "max_namespaces": 4,
                "max_entry_age_secs": 30 * 24 * 60 * 60,
                "max_entry_age_subsec_nanos": 0,
                "max_total_bytes": 4 * 1024**3,
            },
            "concurrency": 1,
        },
        "provider_work_budget": {
            "inflight_requests_cap": 2,
            "inflight_bytes_cap": 100_000,
            "total_cost_micros_ceiling": 10_000,
            "retry_attempts_cap": 3,
        },
        "provider_egress_grant_digest": APPROVED_GRANT_DIGEST,
    }
    stats = {
        "schema_version": 2,
        "total_texts_observed": 26,
        "cache_hits": 0,
        "distinct_miss_texts": 26,
        "http_request_count": 2,
        "retry_count": 0,
        "retryable_status_count": 0,
        "transport_error_count": 0,
        "max_request_texts": 13,
        "max_estimated_tokens": 100,
        "request_samples_dropped": 0,
        "request_samples": [
            {"texts_submitted": 13, "estimated_tokens": 100},
            {"texts_submitted": 13, "estimated_tokens": 80},
        ],
    }
    cases = []
    rows = []
    hash_ndcg = 1 / math.log2(3)
    for index, case_id in enumerate(CASE_IDS):
        on_topic = f"src/on_topic_{index}.rs"
        hard_negative = f"src/hard_negative_{index}.rs"
        hash_metrics = {
            "produced_order": [hard_negative, on_topic],
            "mrr_at_10": 0.5,
            "ndcg_at_10": hash_ndcg,
            "recall_at_20": 1.0,
            "top1_is_on_topic": False,
            "hard_negative_rank": 1,
            "on_topic_rank": 2,
        }
        openai_metrics = {
            "produced_order": [on_topic, hard_negative],
            "mrr_at_10": 1.0,
            "ndcg_at_10": 1.0,
            "recall_at_20": 1.0,
            "top1_is_on_topic": True,
            "hard_negative_rank": 2,
            "on_topic_rank": 1,
        }
        cases.append(
            {
                "id": case_id,
                "intent_kind": "paraphrase" if index else "exact_token",
                "intent": f"fixture intent {index}",
                "query": f"fixture query {index}",
                "on_topic_path": on_topic,
                "hard_negative_path": hard_negative,
                "hash": hash_metrics,
                "openai": openai_metrics,
                "delta": {
                    "mrr_at_10": 0.5,
                    "ndcg_at_10": 1.0 - hash_ndcg,
                    "recall_at_20": 0.0,
                    "on_topic_rank_hash": 2,
                    "on_topic_rank_openai": 1,
                    "openai_ranks_on_topic_higher": True,
                    "top1_win": True,
                },
            }
        )
        rows.append(
            {
                "scenario_id": f"relevance-openai-ab.{case_id}",
                "route_family": "semantic",
                "syntax": "native",
                "result_shape": "candidates",
                "latency": None,
                "qps": None,
                "error_count": 0,
                "timeout_count": 0,
                "result_count": 2,
                "typed_error_code": None,
                "engine_touched": ["semantic"],
                "early_stop_reason": None,
            }
        )
    cases_sidecar = {
        "kind": "quanta-index-relevance-openai-ab-cases",
        "supplement_schema_version": 1,
        "cases": cases,
    }
    summary = {
        "schema_version": 2,
        "dimension": "relevance-openai-ab",
        "mode": "warm",
        "concurrency": 1,
        "provenance": {
            "git_head": head,
            "corpus_digest": "sha256:" + "b" * 64,
            "config_digest": _config_digest(runner_config),
            "model_revision": "fixture-model@fixture-revision:d1536",
        },
        "host": {
            "os": "linux",
            "arch": "x86_64",
            "cpu_count": 1,
            "mem_bytes": 1,
            "hostname_hash": "sha256:" + "c" * 64,
        },
        "resources": {"peak_rss_bytes": 1},
        "phases": {"build_ms": None, "update_ms": None, "gc_ms": None},
        "disk_amplification": None,
        "rows": rows,
        "detail": {
            "status": "captured",
            "case_count": 13,
            "paraphrase_case_count": 12,
            "paraphrase_hash_top1": 0,
            "paraphrase_openai_top1": 12,
            "paraphrase_openai_ranks_on_topic_higher": 12,
            "headline_metric": "on_topic_rank",
            "metrics_note": "fixture note; not a measured result",
            "provider_work_budget": runner_config["provider_work_budget"],
            "provider_stats": stats,
            "case_results": cases_sidecar,
            "runner_config": runner_config,
        },
    }
    return {
        "summary.json": summary,
        "cases.json": cases_sidecar,
        "provider-stats.json": stats,
    }


def _fake_runner(root: Path, fixture_dir: Path, marker: Path, *, partial: bool = False) -> Path:
    runner = root / "relevance_openai_ab"
    script = (
        f"#!{sys.executable}\n"
        "import os, pathlib, shutil, sys\n"
        "if sys.argv[1:] == ['--print-inputs']:\n"
        "    assert 'OPENAI_API_KEY' not in os.environ\n"
        f"    print(pathlib.Path({str(root / 'preview.json')!r}).read_text())\n"
        "    sys.exit(0)\n"
        "assert os.environ['OPENAI_API_KEY'] == 'benign-test-only-key'\n"
        f"pathlib.Path({str(marker)!r}).write_text(sys.argv[0])\n"
        "assert len(sys.argv) == 3 and sys.argv[1] == '--out-dir'\n"
        "output = pathlib.Path(sys.argv[2])\n"
        "output.mkdir()\n"
        f"source = pathlib.Path({str(fixture_dir)!r})\n"
        "for name in ('summary.json', 'cases.json', 'provider-stats.json'):\n"
        + ("    if name == 'provider-stats.json': break\n" if partial else "")
        + "    shutil.copyfile(source / name, output / name)\n"
    )
    if runner.exists():
        runner.chmod(0o700)
    runner.write_text(script)
    runner.chmod(0o500)
    return runner


def _setup(tmp_path: Path, *, archived: bool = False) -> tuple[dict, Path, Path, Path, str]:
    checkout, head = _source_checkout(tmp_path, archived=archived)
    fixture_dir = tmp_path / "fixture"
    fixture_dir.mkdir()
    for name, payload in _fixture(head).items():
        (fixture_dir / name).write_text(json.dumps(payload, allow_nan=False) + "\n")
    marker = tmp_path / "executed.txt"
    runner = _fake_runner(tmp_path, fixture_dir, marker)
    options = {
        "runner_binary": runner,
        "runner_sha256": RawFile.capture(runner).sha256,
        "source_checkout": checkout,
        "source_head": head,
        "out_dir": tmp_path / "captured",
        "env": {
            **{
                key: value
                for key, value in os.environ.items()
                if not key.startswith(
                    (
                        "QUANTA_INDEX_LEGACY_SEARCHD_",
                        "QUANTA_INDEX_OPENAI_AB_",
                        "QUANTA_INDEX_EMBED",
                        "QUANTA_INDEX_PROVIDER_",
                    )
                )
            },
            **APPROVED_GRANT,
            "OPENAI_API_KEY": "benign-test-only-key",
        },
    }
    return options, fixture_dir, marker, runner, head


def test_detached_ab_captures_original_three_sidecars_from_private_copy(
    tmp_path: Path,
    monkeypatch,
) -> None:
    options, fixture_dir, marker, runner, head = _prepared_capture(tmp_path)
    executable_scratch = tmp_path / "executable-scratch"
    executable_scratch.mkdir()
    scratch_alias = tmp_path / "executable-scratch-alias"
    scratch_alias.symlink_to(executable_scratch, target_is_directory=True)
    monkeypatch.setattr("tempfile.tempdir", str(scratch_alias))
    expected_cases, _ = _source_cases(options["source_checkout"], head)
    files = capture_detached_ab(**options)
    assert set(files) == {"summary.json", "cases.json", "provider-stats.json"}
    assert set(path.name for path in options["out_dir"].iterdir()) == set(files)
    assert (
        validate_capture(
            options["out_dir"],
            head,
            expected_cases=expected_cases,
            approved_env=options["env"],
            expected_legacy_daemon_sha256=options["env"].get(LEGACY_DAEMON_SHA256),
        )
        == files
    )
    executed_runner = Path(marker.read_text())
    assert executed_runner != runner
    assert executed_runner.name == "relevance_openai_ab"
    assert executed_runner.parents[1] == executable_scratch
    assert not executed_runner.exists()
    for name in files:
        assert (options["out_dir"] / name).read_bytes() == (fixture_dir / name).read_bytes()
    assert "benign-test-only-key" not in repr(files)


def _validate_preserved(options: dict, fixture_dir: Path) -> dict[str, RawFile]:
    env = dict(options["env"])
    env.pop("OPENAI_API_KEY")
    return validate_frozen_ab(
        directory=fixture_dir,
        source_checkout=options["source_checkout"],
        source_head=options["source_head"],
        env=env,
    )


def test_frozen_ab_validates_historical_sidecars_without_executing_runner(tmp_path: Path) -> None:
    options, fixture, marker, runner, head = _setup(tmp_path, archived=True)
    runner.unlink()
    env = dict(options["env"])
    env.pop("OPENAI_API_KEY")
    result = subprocess.run(
        [
            sys.executable,
            str(REPO_ROOT / "tools/benchmark/benchctl.py"),
            "validate-frozen-openai-ab",
            "--capture",
            str(fixture),
            "--source-checkout",
            str(options["source_checkout"]),
            "--source-head",
            head,
        ],
        env=env,
        capture_output=True,
        text=True,
        check=True,
    )
    output = json.loads(result.stdout)
    assert output["scope"] == "frozen_openai_ab_validation"
    assert output["source_head"] == head
    assert output["outputs"] == {
        name: RawFile.capture(fixture / name).sha256
        for name in ("summary.json", "cases.json", "provider-stats.json")
    }
    assert not marker.exists()


def _write_reviewed_inputs(
    options: dict, config: dict, source_batch_digest: str, root: Path
) -> None:
    config_digest = _config_digest(config)
    cases, _ = _source_cases(options["source_checkout"], options["source_head"])
    judgments_digest = _judgments_digest(cases)
    approval_digest = _approved_input_digest(config_digest, source_batch_digest, judgments_digest)
    options["env"][APPROVED_AB_INPUT_DIGEST] = approval_digest
    (root / "preview.json").write_text(
        json.dumps(
            {
                "runner_config": config,
                "config_digest": config_digest,
                "source_batch_digest": source_batch_digest,
                "judgments_digest": judgments_digest,
                "approval_digest": approval_digest,
            }
        )
    )


def _prepared_capture(tmp_path: Path) -> tuple[dict, Path, Path, Path, str]:
    options, fixture, marker, runner, head = _setup(tmp_path)
    evidence = _process_source_evidence(tmp_path, options, fixture)
    summary_path = fixture / "summary.json"
    summary = json.loads(summary_path.read_text())
    summary["detail"]["source_evidence"] = evidence
    summary_path.write_text(json.dumps(summary))
    return options, fixture, marker, runner, head


def _process_source_evidence(
    tmp_path: Path,
    options: dict,
    fixture_dir: Path,
    *,
    cache_enabled: bool = True,
) -> dict:
    daemon = tmp_path / "quanta-index-searchd"
    daemon.write_bytes(b"pinned daemon fixture")
    daemon.chmod(0o500)
    sha = RawFile.capture(daemon).sha256.removeprefix("sha256:")
    options["env"].update(
        QUANTA_INDEX_LEGACY_SEARCHD_BIN=str(daemon),
        QUANTA_INDEX_LEGACY_SEARCHD_SHA256=sha,
    )
    options["env"][OPENAI_REQUEST_COST] = "10"
    stats_path = fixture_dir / "provider-stats.json"
    summary_path = fixture_dir / "summary.json"
    stats = json.loads(stats_path.read_text())
    summary = json.loads(summary_path.read_text())
    detail = summary["detail"]
    detail["runner_config"]["openai"]["cache_enabled"] = cache_enabled
    options["env"][OPENAI_COST_CEILING] = str(
        detail["provider_work_budget"]["total_cost_micros_ceiling"]
    )
    detail["provider_work_budget"]["openai_max_request_cost_micros"] = 10
    detail["runner_config"]["provider_work_budget"] = detail["provider_work_budget"]
    summary["provenance"]["config_digest"] = _config_digest(detail["runner_config"])
    published = dict(
        stats,
        http_request_count=1,
        total_texts_observed=13 if cache_enabled else 0,
        distinct_miss_texts=13 if cache_enabled else 0,
        request_samples=stats["request_samples"][:1],
    )
    stats = dict(
        published,
        http_request_count=14,
        total_texts_observed=26 if cache_enabled else 0,
        distinct_miss_texts=26 if cache_enabled else 0,
        request_samples=published["request_samples"]
        + [{"texts_submitted": 1, "estimated_tokens": 6} for _ in CASE_IDS],
    )
    stats_path.write_text(json.dumps(stats))
    detail["provider_stats"] = stats
    summary_path.write_text(json.dumps(summary))
    lexical = {
        "repo_id": "repo-e2e",
        "revision_id": "rev-e2e",
        "track": "Lexical",
        "manifest_generation": 1,
        "manifest_digest": "source-manifest:1",
    }
    heads = {}
    for arm, incarnation in (("hash", 1), ("openai", 2)):
        heads[arm] = {
            "generation": {
                "lexical": dict(lexical),
                "semantic": dict(lexical, track="Semantic"),
                "semantic_content": {
                    "row_root_digest": "sha256:" + str(incarnation) * 64,
                    "membership_root_digest": "sha256:" + "c" * 64,
                },
            },
            "activation_token": {"root_incarnation": [incarnation] * 16, "activation_sequence": 1},
        }
    source_batch_digest = "sha256:" + "e" * 64
    _write_reviewed_inputs(options, detail["runner_config"], source_batch_digest, tmp_path)
    return {
        "legacy_daemon_sha256": sha,
        "source_batch_digest": source_batch_digest,
        "openai_publication_http_requests": 1,
        "openai_publication_stats": published,
        **heads,
    }


@pytest.mark.parametrize(
    "mutation",
    [
        None,
        "missing",
        "no_pin",
        "daemon",
        "batch",
        "different_batch",
        "different_default",
        "manifest",
        "track",
        "root",
        "zero",
        "bool",
        "above_total",
    ],
)
def test_detached_ab_binds_external_process_source_evidence(tmp_path: Path, mutation) -> None:
    options, fixture_dir, marker, _, _ = _setup(tmp_path, archived=False)
    evidence = _process_source_evidence(tmp_path, options, fixture_dir)
    path = fixture_dir / "summary.json"
    summary = json.loads(path.read_text())
    if mutation != "missing":
        summary["detail"]["source_evidence"] = evidence
    if mutation == "no_pin":
        del options["env"]["QUANTA_INDEX_LEGACY_SEARCHD_BIN"]
        del options["env"]["QUANTA_INDEX_LEGACY_SEARCHD_SHA256"]
    elif mutation == "daemon":
        evidence["legacy_daemon_sha256"] = "0" * 64
    elif mutation == "batch":
        evidence["source_batch_digest"] = "not-a-batch-digest"
    elif mutation == "different_batch":
        evidence["source_batch_digest"] = "sha256:" + "d" * 64
    elif mutation == "different_default":
        config = summary["detail"]["runner_config"]
        config["openai"]["concurrency"] = 2
        summary["provenance"]["config_digest"] = _config_digest(config)
    elif mutation == "manifest":
        for track in ("lexical", "semantic"):
            evidence["openai"]["generation"][track]["manifest_digest"] = "different-manifest"
    elif mutation == "track":
        evidence["openai"]["generation"]["semantic"]["track"] = "Lexical"
    elif mutation == "root":
        evidence["openai"]["activation_token"] = evidence["hash"]["activation_token"]
    elif mutation in {"zero", "bool", "above_total"}:
        evidence["openai_publication_http_requests"] = {"zero": 0, "bool": True, "above_total": 3}[
            mutation
        ]
    path.write_text(json.dumps(summary) + "\n")
    if mutation is None:
        assert set(capture_detached_ab(**options)) == {
            "summary.json",
            "cases.json",
            "provider-stats.json",
        }
    else:
        reason = (
            "differs from the approved input digest"
            if mutation in {"different_batch", "different_default"}
            else "Source|source|daemon|publication"
        )
        with pytest.raises(EvidenceError, match=reason):
            capture_detached_ab(**options)
        assert marker.exists() == (mutation != "no_pin")
        assert not options["out_dir"].exists()


def test_detached_ab_requires_pins_for_current_roster_before_launch(tmp_path: Path) -> None:
    options, _, marker, _, _ = _setup(tmp_path, archived=False)
    with pytest.raises(EvidenceError, match="requires a pinned legacy daemon"):
        capture_detached_ab(**options)
    assert not marker.exists()
    assert not options["out_dir"].exists()


@pytest.mark.parametrize("external_settings", [False, True])
def test_detached_ab_refuses_archived_runner_before_launch(
    tmp_path: Path, external_settings: bool
) -> None:
    options, fixture, marker, _, _ = _setup(tmp_path, archived=True)
    if external_settings:
        evidence = _process_source_evidence(tmp_path, options, fixture)
        summary_path = fixture / "summary.json"
        summary = json.loads(summary_path.read_text())
        summary["detail"]["source_evidence"] = evidence
        summary_path.write_text(json.dumps(summary))
    with pytest.raises(EvidenceError, match="historical OpenAI A/B captures are validation-only"):
        capture_detached_ab(**options)
    assert not marker.exists()
    assert not options["out_dir"].exists()


def test_detached_ab_refuses_a_runner_without_a_credential_free_preview(tmp_path: Path) -> None:
    options, _, marker, runner, _ = _prepared_capture(tmp_path)
    runner.chmod(0o700)
    # A historical runner may ignore an unknown flag. Its live entry still needs the key.
    runner.write_text(
        f"#!{sys.executable}\n"
        "import os, pathlib\n"
        "assert 'OPENAI_API_KEY' in os.environ\n"
        f"pathlib.Path({str(marker)!r}).touch()\n"
    )
    runner.chmod(0o500)
    options["runner_sha256"] = RawFile.capture(runner).sha256
    with pytest.raises(EvidenceError, match="credential-free runner preview failed"):
        capture_detached_ab(**options)
    assert not marker.exists()
    assert not options["out_dir"].exists()


def test_detached_ab_refuses_a_self_consistent_unapproved_preview(tmp_path: Path) -> None:
    options, _, marker, _, _ = _prepared_capture(tmp_path)
    approved = options["env"][APPROVED_AB_INPUT_DIGEST]
    preview = json.loads((tmp_path / "preview.json").read_text())
    _write_reviewed_inputs(options, preview["runner_config"], "sha256:" + "d" * 64, tmp_path)
    options["env"][APPROVED_AB_INPUT_DIGEST] = approved
    with pytest.raises(EvidenceError, match="preview differs from the approved input digest"):
        capture_detached_ab(**options)
    assert not marker.exists()
    assert not options["out_dir"].exists()


def test_detached_ab_refuses_different_query_before_paid_dispatch(tmp_path: Path) -> None:
    options, _, marker, _, _ = _prepared_capture(tmp_path)
    approved_cases, _ = _source_cases(options["source_checkout"], options["source_head"])
    approved_cases[0]["query"] = "different synthetic query"
    preview_path = tmp_path / "preview.json"
    preview = json.loads(preview_path.read_text())
    preview["judgments_digest"] = _judgments_digest(approved_cases)
    preview["approval_digest"] = _approved_input_digest(
        preview["config_digest"], preview["source_batch_digest"], preview["judgments_digest"]
    )
    options["env"][APPROVED_AB_INPUT_DIGEST] = preview["approval_digest"]
    preview_path.write_text(json.dumps(preview))

    with pytest.raises(EvidenceError, match="preview judgments differ from the approved checkout"):
        capture_detached_ab(**options)
    assert not marker.exists()
    assert not options["out_dir"].exists()


def test_frozen_external_ab_accepts_previous_approval_only_when_reading_results(
    tmp_path: Path,
) -> None:
    options, fixture, marker, _, _ = _prepared_capture(tmp_path)
    preview = json.loads((tmp_path / "preview.json").read_text())
    options["env"][APPROVED_AB_INPUT_DIGEST] = _approved_input_digest(
        preview["config_digest"], preview["source_batch_digest"], None
    )
    assert len(_validate_preserved(options, fixture)) == 3
    with pytest.raises(EvidenceError, match="preview differs from the approved input digest"):
        capture_detached_ab(**options)
    assert not marker.exists()
    assert not options["out_dir"].exists()


@pytest.mark.parametrize("collision", ["empty-directory", "file", "symlink"])
def test_detached_ab_never_replaces_a_concurrently_created_output(
    tmp_path: Path, monkeypatch, collision: str
) -> None:
    options, _, marker, _, _ = _prepared_capture(tmp_path)
    destination = options["out_dir"].resolve()
    concurrent = []
    publish = ab_capture._publish_create_only
    link_target = tmp_path / "link-target"
    link_target.mkdir()

    def publish_after_competitor(source, target, parent):
        if target == destination:
            if collision == "empty-directory":
                target.mkdir()
            elif collision == "file":
                target.write_bytes(b"concurrent capture")
            else:
                target.symlink_to(link_target, target_is_directory=True)
            concurrent.append(target.lstat())
        publish(source, target, parent)

    monkeypatch.setattr(ab_capture, "_publish_create_only", publish_after_competitor)
    with pytest.raises(EvidenceError, match="output appeared during capture"):
        capture_detached_ab(**options)
    assert marker.exists()
    assert len(concurrent) == 1 and os.path.samestat(destination.lstat(), concurrent[0])
    if collision == "empty-directory":
        assert list(destination.iterdir()) == []
    elif collision == "file":
        assert destination.read_bytes() == b"concurrent capture"
    else:
        assert destination.readlink() == link_target
        assert list(link_target.iterdir()) == []
    assert not list(tmp_path.glob(".relevance-openai-ab-*"))


def test_detached_ab_checks_atomic_publication_before_launch(tmp_path: Path, monkeypatch) -> None:
    options, _, marker, _, _ = _prepared_capture(tmp_path)

    def unsupported_filesystem(*_args):
        raise OSError("fixture filesystem lacks exclusive rename")

    def unexpected_launch(*_args, **_kwargs):
        pytest.fail("runner launched before publication support was checked")

    monkeypatch.setattr(ab_capture, "_publish_create_only", unsupported_filesystem)
    monkeypatch.setattr(ab_capture, "execute", unexpected_launch)
    with pytest.raises(OSError, match="lacks exclusive rename"):
        capture_detached_ab(**options)
    assert not marker.exists()
    assert not options["out_dir"].exists()
    assert not list(tmp_path.glob(".relevance-openai-ab-*"))


@pytest.mark.parametrize("approval", [None, "invalid"])
def test_detached_ab_requires_input_approval_before_launch(tmp_path: Path, approval) -> None:
    options, fixture, marker, _, _ = _setup(tmp_path, archived=False)
    _process_source_evidence(tmp_path, options, fixture)
    if approval is None:
        del options["env"][APPROVED_AB_INPUT_DIGEST]
    else:
        options["env"][APPROVED_AB_INPUT_DIGEST] = approval
    with pytest.raises(EvidenceError, match=APPROVED_AB_INPUT_DIGEST):
        capture_detached_ab(**options)
    assert not marker.exists()
    assert not options["out_dir"].exists()


def test_approved_input_digest_matches_rust_contract() -> None:
    assert (
        _approved_input_digest("sha256:" + "1" * 64, "sha256:" + "2" * 64, "sha256:" + "3" * 64)
        == "sha256:c4e075295756e5653886b25e2ef802fd9628cd4c0a878bf08f80776fdcdd1d82"
    )


def test_detached_ab_refuses_wrong_external_daemon_pin_before_execution(tmp_path: Path) -> None:
    options, fixture_dir, marker, _, _ = _setup(tmp_path)
    _process_source_evidence(tmp_path, options, fixture_dir)
    options["env"]["QUANTA_INDEX_LEGACY_SEARCHD_SHA256"] = "0" * 64
    with pytest.raises(EvidenceError, match="daemon SHA-256 differs"):
        capture_detached_ab(**options)
    assert not marker.exists()


@pytest.mark.parametrize("ceiling", [None, "0", "9"])
def test_detached_ab_refuses_unfunded_paid_execution_before_launch(
    tmp_path: Path,
    ceiling: str | None,
) -> None:
    options, fixture, marker, _, _ = _setup(tmp_path)
    _process_source_evidence(tmp_path, options, fixture)
    if ceiling is None:
        del options["env"][OPENAI_COST_CEILING]
    else:
        options["env"][OPENAI_COST_CEILING] = ceiling
    with pytest.raises(EvidenceError, match="explicit positive cost ceiling"):
        capture_detached_ab(**options)
    assert not marker.exists()
    assert not options["out_dir"].exists()


@pytest.mark.parametrize("phase", ["Source", "query"])
@pytest.mark.parametrize("retry_cap", [0, 1])
def test_detached_ab_checks_retry_limits_for_each_phase(
    tmp_path: Path,
    phase: str,
    retry_cap: int,
) -> None:
    options, fixture, marker, _, _ = _setup(tmp_path)
    evidence = _process_source_evidence(tmp_path, options, fixture)
    path = fixture / "summary.json"
    summary = json.loads(path.read_text())
    detail = summary["detail"]
    stats = detail["provider_stats"]
    published = evidence["openai_publication_stats"]
    retryable_sample = {"texts_submitted": 1, "estimated_tokens": 6}
    if phase == "Source":
        retryable_sample = published["request_samples"][0]
        published["request_samples"].insert(0, retryable_sample)
        published.update(http_request_count=2, retry_count=1, retryable_status_count=1)
        evidence["openai_publication_http_requests"] = 2
    stats["request_samples"].insert(0 if phase == "Source" else 1, retryable_sample)
    stats.update(http_request_count=15, retry_count=1, retryable_status_count=1)
    config = detail["runner_config"]
    config["openai"]["max_retries"] = retry_cap
    options["env"]["QUANTA_INDEX_EMBED_MAX_RETRIES"] = str(retry_cap)
    summary["provenance"]["config_digest"] = _config_digest(config)
    _write_reviewed_inputs(options, config, evidence["source_batch_digest"], tmp_path)
    detail["source_evidence"] = evidence
    path.write_text(json.dumps(summary))
    (fixture / "provider-stats.json").write_text(json.dumps(stats))
    if retry_cap == 0:
        with pytest.raises(EvidenceError, match="exceeds the approved retry limit"):
            capture_detached_ab(**options)
        assert not options["out_dir"].exists()
    else:
        assert len(capture_detached_ab(**options)) == 3
    assert marker.exists()


@pytest.mark.parametrize("cache_enabled", [False, True])
def test_detached_ab_refuses_source_only_provider_observations(
    tmp_path: Path,
    cache_enabled: bool,
) -> None:
    options, fixture, marker, _, _ = _setup(tmp_path)
    evidence = _process_source_evidence(tmp_path, options, fixture, cache_enabled=cache_enabled)
    path = fixture / "summary.json"
    summary = json.loads(path.read_text())
    summary["detail"]["source_evidence"] = evidence
    summary["detail"]["provider_stats"] = evidence["openai_publication_stats"]
    path.write_text(json.dumps(summary))
    (fixture / "provider-stats.json").write_text(json.dumps(evidence["openai_publication_stats"]))
    with pytest.raises(EvidenceError, match="observations do not account for every"):
        capture_detached_ab(**options)
    assert marker.exists()
    assert not options["out_dir"].exists()


def test_detached_ab_refuses_more_than_one_input_in_a_query_attempt(tmp_path: Path) -> None:
    options, fixture, marker, _, _ = _setup(tmp_path)
    evidence = _process_source_evidence(tmp_path, options, fixture)
    path = fixture / "summary.json"
    summary = json.loads(path.read_text())
    stats = summary["detail"]["provider_stats"]
    stats["request_samples"][-1]["texts_submitted"] = 2
    summary["detail"]["source_evidence"] = evidence
    path.write_text(json.dumps(summary))
    (fixture / "provider-stats.json").write_text(json.dumps(stats))
    with pytest.raises(EvidenceError, match="query request samples must contain one text"):
        capture_detached_ab(**options)
    assert marker.exists()
    assert not options["out_dir"].exists()


def test_detached_ab_refuses_a_bad_current_roster_before_launch(tmp_path: Path) -> None:
    options, _, marker, _, _ = _setup(tmp_path, archived=True)
    checkout = options["source_checkout"]
    (checkout / _JUDGED_SOURCE).write_text("invalid columns\n")
    _git("add", _JUDGED_SOURCE, cwd=checkout)
    _git("commit", "-qm", "malformed current roster", cwd=checkout)
    options["source_head"] = _git("rev-parse", "HEAD", cwd=checkout)
    with pytest.raises(EvidenceError, match="approved semantic judgment columns differ"):
        capture_detached_ab(**options)
    assert not marker.exists()


def test_detached_ab_accepts_fully_observed_cached_queries_without_http(tmp_path: Path) -> None:
    options, fixture, _, _, _ = _setup(tmp_path)
    evidence = _process_source_evidence(tmp_path, options, fixture)
    path = fixture / "summary.json"
    summary = json.loads(path.read_text())
    stats = dict(evidence["openai_publication_stats"], cache_hits=13, total_texts_observed=26)
    summary["detail"]["source_evidence"] = evidence
    summary["detail"]["provider_stats"] = stats
    path.write_text(json.dumps(summary))
    (fixture / "provider-stats.json").write_text(json.dumps(stats))
    assert len(capture_detached_ab(**options)) == 3


@pytest.mark.parametrize(
    ("name", "value", "reason"),
    [
        ("QUANTA_INDEX_EMBED_BATCH", "1", "QUANTA_INDEX_EMBED_BATCH"),
        (
            "QUANTA_INDEX_PROVIDER_MAX_RETRY_ATTEMPTS",
            "1",
            "QUANTA_INDEX_PROVIDER_MAX_RETRY_ATTEMPTS",
        ),
        ("QUANTA_INDEX_PROVIDER_ENDPOINT", "https://other.example/embeddings", "provider grant"),
        ("QUANTA_INDEX_EMBED_CACHE", "off", "QUANTA_INDEX_EMBED_CACHE"),
        (OPENAI_REQUEST_COST, "20", OPENAI_REQUEST_COST),
    ],
)
def test_detached_ab_refuses_self_consistent_unapproved_settings(
    tmp_path: Path,
    name: str,
    value: str,
    reason: str,
) -> None:
    options, fixture, marker, _, _ = _setup(tmp_path)
    evidence = _process_source_evidence(tmp_path, options, fixture)
    path = fixture / "summary.json"
    summary = json.loads(path.read_text())
    summary["detail"]["source_evidence"] = evidence
    path.write_text(json.dumps(summary))
    options["env"][name] = value
    with pytest.raises(EvidenceError, match=reason):
        capture_detached_ab(**options)
    assert not marker.exists()
    assert not options["out_dir"].exists()


@pytest.mark.parametrize("external_daemon", [False, True])
@pytest.mark.parametrize("field", ["id", "query", "on_topic_path", "hard_negative_path"])
def test_detached_ab_refuses_substituted_approved_case(
    tmp_path: Path,
    external_daemon: bool,
    field: str,
) -> None:
    options, fixture, marker, _, _ = _setup(tmp_path, archived=not external_daemon)
    evidence = _process_source_evidence(tmp_path, options, fixture) if external_daemon else None
    path = fixture / "summary.json"
    summary = json.loads(path.read_text())
    if evidence is not None:
        summary["detail"]["source_evidence"] = evidence
    cases_path = fixture / "cases.json"
    cases = json.loads(cases_path.read_text())
    case = cases["cases"][0]
    old_value = case[field]
    case[field] = {
        "id": "sem.substituted.exact",
        "query": "unrelated query substituted for the approved case",
        "on_topic_path": "src/substituted_topic.rs",
        "hard_negative_path": "src/substituted_negative.rs",
    }[field]
    if field.endswith("_path"):
        for arm in ("hash", "openai"):
            order = case[arm]["produced_order"]
            order[order.index(old_value)] = case[field]
    elif field == "id":
        summary["rows"][0]["scenario_id"] = "relevance-openai-ab." + case["id"]
    summary["detail"]["case_results"] = cases
    cases_path.write_text(json.dumps(cases))
    path.write_text(json.dumps(summary))
    with pytest.raises(EvidenceError, match="approved semantic judgment"):
        if external_daemon:
            capture_detached_ab(**options)
        else:
            _validate_preserved(options, fixture)
    assert marker.exists() == external_daemon
    assert not options["out_dir"].exists()


def test_detached_ab_refuses_external_daemon_change_during_execution(tmp_path: Path) -> None:
    options, fixture_dir, marker, runner, _ = _setup(tmp_path)
    evidence = _process_source_evidence(tmp_path, options, fixture_dir)
    path = fixture_dir / "summary.json"
    summary = json.loads(path.read_text())
    summary["detail"]["source_evidence"] = evidence
    path.write_text(json.dumps(summary) + "\n")
    daemon = options["env"]["QUANTA_INDEX_LEGACY_SEARCHD_BIN"]
    script = runner.read_text() + (
        f"daemon = pathlib.Path({daemon!r})\n"
        "daemon.chmod(0o700)\n"
        "daemon.write_bytes(b'different executable')\n"
    )
    runner.chmod(0o700)
    runner.write_text(script)
    runner.chmod(0o500)
    options["runner_sha256"] = RawFile.capture(runner).sha256
    with pytest.raises(EvidenceError, match="legacy daemon changed during execution"):
        capture_detached_ab(**options)
    assert marker.exists()
    assert not options["out_dir"].exists()


def test_capture_applies_control_size_bound_before_hashing_sidecars(
    tmp_path: Path, monkeypatch
) -> None:
    options, fixture_dir, _, _, head = _setup(tmp_path)
    with (fixture_dir / "summary.json").open("r+b") as oversized:
        oversized.truncate(CONTROL_DOCUMENT_BYTES + 1)

    def unbounded_capture(cls, path):
        pytest.fail(f"hashed sidecar before bounded control read: {path.name}")

    monkeypatch.setattr(RawFile, "capture", classmethod(unbounded_capture))
    expected_cases, _ = _source_cases(options["source_checkout"], head)
    with pytest.raises(EvidenceError, match="control document exceeds"):
        validate_capture(
            fixture_dir,
            head,
            expected_cases=expected_cases,
            approved_env=options["env"],
        )


@pytest.mark.parametrize(
    "mutation", ["missing_sha", "wrong_sha", "dirty_source", "existing_output", "checkout_output"]
)
def test_detached_ab_refuses_bad_admission_before_execution(tmp_path: Path, mutation: str) -> None:
    options, _, marker, _, _ = _setup(tmp_path)
    if mutation == "missing_sha":
        options["runner_sha256"] = ""
    elif mutation == "wrong_sha":
        options["runner_sha256"] = "sha256:" + "0" * 64
    elif mutation == "dirty_source":
        (options["source_checkout"] / "source.txt").write_text("drift\n")
    elif mutation == "existing_output":
        options["out_dir"].mkdir()
    else:
        options["out_dir"] = options["source_checkout"] / "capture"
    with pytest.raises((EvidenceError, RuntimeError)):
        capture_detached_ab(**options)
    assert not marker.exists()


@pytest.mark.parametrize("mutation", ["partial", "cases", "stats", "head", "config"])
def test_detached_ab_refuses_incomplete_or_cross_bound_output(
    tmp_path: Path, mutation: str
) -> None:
    options, fixture_dir, marker, runner, _ = _prepared_capture(tmp_path)
    if mutation == "partial":
        _fake_runner(tmp_path, fixture_dir, marker, partial=True)
        options["runner_sha256"] = RawFile.capture(runner).sha256
    else:
        file_name = {
            "cases": "cases.json",
            "stats": "provider-stats.json",
            "head": "summary.json",
            "config": "summary.json",
        }[mutation]
        path = fixture_dir / file_name
        payload = json.loads(path.read_text())
        if mutation == "cases":
            payload["cases"][0]["openai"]["on_topic_rank"] = 2
        elif mutation == "stats":
            payload["http_request_count"] += 1
        elif mutation == "head":
            payload["provenance"]["git_head"] = "f" * 40
        else:
            payload["provenance"]["config_digest"] = "sha256:" + "0" * 64
        path.write_text(json.dumps(payload, allow_nan=False) + "\n")
    with pytest.raises((EvidenceError, RuntimeError)):
        capture_detached_ab(**options)
    assert marker.exists()
    assert not options["out_dir"].exists()


def test_detached_ab_refuses_source_change_during_execution(tmp_path: Path) -> None:
    options, _, marker, runner, _ = _prepared_capture(tmp_path)
    script = runner.read_text().replace(
        "output = pathlib.Path(sys.argv[2])",
        f"pathlib.Path({str(options['source_checkout'] / 'source.txt')!r}).write_text('drift')\noutput = pathlib.Path(sys.argv[2])",
    )
    runner.chmod(0o700)
    runner.write_text(script)
    runner.chmod(0o500)
    options["runner_sha256"] = RawFile.capture(runner).sha256
    with pytest.raises((EvidenceError, RuntimeError)):
        capture_detached_ab(**options)
    assert marker.exists()
    assert not options["out_dir"].exists()


@pytest.mark.parametrize(
    "mutation",
    [
        "rank",
        "score",
        "integer_score",
        "delta",
        "near_zero_delta",
        "paraphrase_total",
        "duplicate_judgment",
        "duplicate_order",
        "oversized_order",
    ],
)
def test_frozen_ab_refuses_inconsistent_scoring_with_matching_sidecars(
    tmp_path: Path, mutation: str
) -> None:
    options, fixture_dir, _, _, _ = _setup(tmp_path, archived=True)
    cases_path = fixture_dir / "cases.json"
    summary_path = fixture_dir / "summary.json"
    cases = json.loads(cases_path.read_text())
    summary = json.loads(summary_path.read_text())
    case = cases["cases"][0]
    if mutation == "rank":
        case["openai"]["on_topic_rank"] = 2
    elif mutation == "score":
        case["hash"]["mrr_at_10"] = 1.0
    elif mutation == "integer_score":
        case["openai"]["mrr_at_10"] = 1
    elif mutation == "delta":
        case["delta"]["top1_win"] = False
    elif mutation == "near_zero_delta":
        case["delta"]["recall_at_20"] = -5e-13
    elif mutation == "duplicate_judgment":
        case["hard_negative_path"] = case["on_topic_path"]
    elif mutation in ("duplicate_order", "oversized_order"):
        order = case["openai"]["produced_order"]
        order.extend(
            [case["on_topic_path"]]
            if mutation == "duplicate_order"
            else [f"src/extra_{index}.rs" for index in range(19)]
        )
        summary["rows"][0]["result_count"] = len(order)
    else:
        summary["detail"]["paraphrase_openai_ranks_on_topic_higher"] = 0
    summary["detail"]["case_results"] = cases
    cases_path.write_text(json.dumps(cases))
    summary_path.write_text(json.dumps(summary))

    with pytest.raises(EvidenceError, match="rank|score|delta|paraphrase|judgment|ordering"):
        _validate_preserved(options, fixture_dir)


@pytest.mark.parametrize("on_topic_rank", [None, 11])
def test_frozen_ab_scores_a_missing_topic_or_a_topic_beyond_rank_ten(
    tmp_path: Path, on_topic_rank: int | None
) -> None:
    options, fixture_dir, _, _, _ = _setup(tmp_path, archived=True)
    cases_path = fixture_dir / "cases.json"
    summary_path = fixture_dir / "summary.json"
    cases = json.loads(cases_path.read_text())
    summary = json.loads(summary_path.read_text())
    case = cases["cases"][1]
    order = [case["hard_negative_path"]]
    if on_topic_rank is not None:
        order.extend(f"src/extra_{index}.rs" for index in range(9))
        order.append(case["on_topic_path"])
    case["openai"] = {
        "produced_order": order,
        "mrr_at_10": 0.0,
        "ndcg_at_10": 0.0,
        "recall_at_20": float(on_topic_rank is not None),
        "top1_is_on_topic": False,
        "hard_negative_rank": 1,
        "on_topic_rank": on_topic_rank,
    }
    case["delta"].update(
        mrr_at_10=-0.5,
        ndcg_at_10=-1 / math.log2(3),
        recall_at_20=0.0 if on_topic_rank is not None else -1.0,
        on_topic_rank_openai=on_topic_rank,
        openai_ranks_on_topic_higher=False,
        top1_win=False,
    )
    summary["rows"][1]["result_count"] = len(order)
    summary["detail"].update(
        case_results=cases,
        paraphrase_openai_top1=11,
        paraphrase_openai_ranks_on_topic_higher=11,
    )
    cases_path.write_text(json.dumps(cases))
    summary_path.write_text(json.dumps(summary))

    assert set(_validate_preserved(options, fixture_dir)) == {
        "summary.json",
        "cases.json",
        "provider-stats.json",
    }


@pytest.mark.parametrize("field", ["cases", "provider_stats", "provider_budget"])
def test_frozen_ab_refuses_type_aliases_between_summary_and_sidecar(
    tmp_path: Path, field: str
) -> None:
    options, fixture_dir, _, _, _ = _setup(tmp_path, archived=True)
    summary_path = fixture_dir / "summary.json"
    summary = json.loads(summary_path.read_text())
    detail = summary["detail"]
    if field == "cases":
        detail["case_results"]["cases"][0]["openai"]["on_topic_rank"] = True
    elif field == "provider_stats":
        detail["provider_stats"]["cache_hits"] = False
    else:
        detail["provider_work_budget"]["retry_attempts_cap"] = 3.0
    summary_path.write_text(json.dumps(summary))

    with pytest.raises(EvidenceError, match="cases differ|provider stats differ|budget differs"):
        _validate_preserved(options, fixture_dir)


@pytest.mark.parametrize(
    "mutation", ["text_maximum", "token_maximum", "short_window", "overlapping_cache_counts"]
)
def test_frozen_ab_refuses_provider_totals_without_matching_observations(
    tmp_path: Path, mutation: str
) -> None:
    options, fixture_dir, _, _, _ = _setup(tmp_path, archived=True)
    stats_path = fixture_dir / "provider-stats.json"
    summary_path = fixture_dir / "summary.json"
    stats = json.loads(stats_path.read_text())
    summary = json.loads(summary_path.read_text())
    if mutation == "text_maximum":
        stats["max_request_texts"] += 1
    elif mutation == "token_maximum":
        stats["max_estimated_tokens"] += 1
    elif mutation == "overlapping_cache_counts":
        stats["cache_hits"] = 1
    else:
        stats["request_samples_dropped"] = 1
        stats["http_request_count"] += 1
    summary["detail"]["provider_stats"] = stats
    stats_path.write_text(json.dumps(stats))
    summary_path.write_text(json.dumps(summary))

    with pytest.raises(
        EvidenceError, match="provider request sample window|run maxima|cache observations"
    ):
        _validate_preserved(options, fixture_dir)


@pytest.mark.parametrize(
    ("case", "refusal"),
    [
        ("batch_limit", "max_batch"),
        ("token_limit", "estimated-token limit"),
        ("oversized_singleton", None),
        ("cache_off", None),
        ("cache_off_with_totals", "disabled cache"),
        ("cache_off_without_requests", "observed no provider requests"),
        ("failures_exceed_requests", "failures exceed observed requests"),
        ("retries_exceed_failures", "retries exceed retryable failures"),
    ],
)
def test_frozen_ab_cross_checks_provider_observations_with_runner_config(
    tmp_path: Path,
    case: str,
    refusal: str | None,
) -> None:
    options, fixture_dir, _, _, _ = _setup(tmp_path, archived=True)
    stats_path = fixture_dir / "provider-stats.json"
    summary_path = fixture_dir / "summary.json"
    stats = json.loads(stats_path.read_text())
    summary = json.loads(summary_path.read_text())
    detail = summary["detail"]
    config = detail["runner_config"]
    if case == "batch_limit":
        config["openai"]["max_batch"] = 12
    elif case in {"token_limit", "oversized_singleton"}:
        config["openai"]["max_estimated_tokens_per_request"] = 80
        if case == "oversized_singleton":
            stats["request_samples"][0]["texts_submitted"] = 1
    elif case == "failures_exceed_requests":
        stats.update(retryable_status_count=2, transport_error_count=1)
    elif case == "retries_exceed_failures":
        stats["retry_count"] = 1
    else:
        config["openai"]["cache_enabled"] = False
        if case != "cache_off_with_totals":
            stats.update(total_texts_observed=0, cache_hits=0, distinct_miss_texts=0)
        if case == "cache_off_without_requests":
            stats.update(
                http_request_count=0,
                max_request_texts=0,
                max_estimated_tokens=0,
                request_samples=[],
            )
    detail["provider_stats"] = stats
    summary["provenance"]["config_digest"] = _config_digest(config)
    stats_path.write_text(json.dumps(stats))
    summary_path.write_text(json.dumps(summary))

    if refusal is None:
        captured = _validate_preserved(options, fixture_dir)
        assert json.loads(captured["provider-stats.json"].read_control()) == stats
    else:
        with pytest.raises(EvidenceError, match=refusal):
            _validate_preserved(options, fixture_dir)


def test_frozen_ab_preserves_maxima_from_before_a_full_recent_window(tmp_path: Path) -> None:
    options, fixture_dir, _, _, _ = _setup(tmp_path, archived=True)
    stats_path = fixture_dir / "provider-stats.json"
    summary_path = fixture_dir / "summary.json"
    stats = json.loads(stats_path.read_text())
    summary = json.loads(summary_path.read_text())
    stats.update(
        total_texts_observed=300,
        http_request_count=257,
        request_samples_dropped=1,
        request_samples=[{"texts_submitted": 1, "estimated_tokens": 5}] * 256,
    )
    summary["detail"]["provider_stats"] = stats
    stats_path.write_text(json.dumps(stats))
    summary_path.write_text(json.dumps(summary))

    captured = _validate_preserved(options, fixture_dir)
    assert json.loads(captured["provider-stats.json"].read_control()) == stats
