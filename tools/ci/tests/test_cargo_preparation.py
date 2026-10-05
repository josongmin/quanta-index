"""One native preparation feeds independent collection and execution admission."""

from __future__ import annotations

import json
import os

import pytest

from tools.benchmark.retrieval import portable_proof
from tools.ci.tests import test_portable_proof as producer_fixtures

fake_execution = producer_fixtures.fake_execution
proof_actor_environment = producer_fixtures.proof_actor_environment


@pytest.mark.parametrize("rail", ["contract", "sdk"])
def test_one_preparation_preserves_original_selection_and_reuses_both_commands(
    fake_execution, rail
):
    out, _, calls = fake_execution
    context = json.loads(portable_proof.produce(rail, out).read_bytes())
    native = [argv for argv, _ in calls if argv[3:5] in (["nextest", "list"], ["nextest", "run"])]
    selector = ["-p", "quanta-index-retrieval-bench"]
    selector += (
        [
            "--lib",
            "--bin",
            "quanta-index-retrieval-bench",
            "--test",
            "chunking_contract",
            "--test",
            "l5_parser_regressions",
        ]
        if rail == "contract"
        else ["--test", "sdk_roundtrip"]
    )
    selector += ["--all-features", "--locked"]
    assert len(native) == 3
    assert native[0][3:] == [
        "nextest",
        "list",
        *selector,
        "--list-type",
        "binaries-only",
        "--message-format",
        "json",
    ]
    for argv, operation, output_flags in (
        (native[1], "list", ["--message-format", "json"]),
        (
            native[2],
            "run",
            ["--message-format", "libtest-json-plus", "--message-format-version", "0.1"],
        ),
    ):
        assert argv[3:] == [
            "nextest",
            operation,
            "--binaries-metadata",
            str(out / "rust-build.stdout"),
            "--cargo-metadata",
            str(out / "metadata.stdout"),
            *output_flags,
        ]
    plan = portable_proof._expected_commands(
        rail,
        out,
        context["tools"],
        context["binaries"],
        inherited_environment=context["commands"][0]["inherited_environment"],
    )
    assert [(row["name"], row["argv"]) for row in context["commands"]] == [
        (name, argv) for name, argv, _ in plan
    ]
    assert [
        row["name"]
        for row in context["commands"]
        if row["name"] in {"rust-build", "metadata", "rust-collection"}
    ] == ["rust-build", "metadata", "rust-collection"]
    if rail == "sdk":
        # Searchd remains a separate build because a joint build changes its
        # dependency feature graph. The selected test build supplies the runner.
        builds = [argv[3:] for argv, _ in calls if argv[3:4] == ["build"]]
        assert builds == [
            [
                "build",
                "-p",
                "quanta-index-searchd-runtime",
                "--bin",
                "quanta-index-searchd",
                "--locked",
            ],
        ]


@pytest.mark.parametrize(
    "mutation", ["missing", "wrong_package", "wrong_path", "wrong_kind", "extra"]
)
def test_sdk_runner_must_be_in_selected_native_build(fake_execution, mutation):
    out, runner, _ = fake_execution
    portable_proof.produce("sdk", out)
    build = json.loads((out / "rust-build.stdout").read_bytes())
    metadata = (out / "metadata.stdout").read_bytes()
    collection = (out / "rust-collection.stdout").read_bytes()
    assert portable_proof.verify_reused_build(
        json.dumps(build).encode(),
        metadata,
        collection,
        workspace_root=portable_proof.ROOT,
        required_non_test_binary=runner,
    )
    entries = build["rust-build-meta"]["non-test-binaries"]
    if mutation == "missing":
        del build["rust-build-meta"]["non-test-binaries"]
    elif mutation == "wrong_package":
        entries["other-package"] = entries.pop("fixture-retrieval-package")
    elif mutation == "wrong_path":
        entries["fixture-retrieval-package"][0]["path"] = "debug/other-runner"
    elif mutation == "wrong_kind":
        entries["fixture-retrieval-package"][0]["kind"] = "test"
    else:
        entries["fixture-retrieval-package"].append(entries["fixture-retrieval-package"][0])
    with pytest.raises(ValueError, match="SDK runner is absent"):
        portable_proof.verify_reused_build(
            json.dumps(build).encode(),
            metadata,
            collection,
            workspace_root=portable_proof.ROOT,
            required_non_test_binary=runner,
        )


@pytest.mark.parametrize("name", ["rust-build.stdout", "metadata.stdout"])
@pytest.mark.parametrize("mutation", ["restore", "replace", "symlink", "content"])
def test_collection_reuse_input_mutation_blocks_publication(
    fake_execution, monkeypatch, name, mutation
):
    out, _, calls = fake_execution
    execute = portable_proof.execute

    def mutate_collection(argv, **kwargs):
        result = execute(argv, **kwargs)
        if argv[3:5] != ["nextest", "list"] or "--binaries-metadata" not in argv:
            return result
        path = out / name
        original, stamp = path.read_bytes(), path.stat()
        if mutation == "restore":
            path.write_bytes(b"foreign metadata")
            path.write_bytes(original)
            os.utime(path, ns=(stamp.st_atime_ns, stamp.st_mtime_ns))
        elif mutation in {"replace", "symlink"}:
            replacement = out / "foreign-metadata"
            replacement.write_bytes(original)
            if mutation == "replace":
                replacement.replace(path)
            else:
                path.unlink()
                path.symlink_to(replacement)
        else:
            path.write_bytes(b"foreign metadata")
        return result

    monkeypatch.setattr(portable_proof, "execute", mutate_collection)
    with pytest.raises(ValueError, match="nextest reuse input"):
        portable_proof.produce("sdk", out)
    assert not any(argv[3:5] == ["nextest", "run"] for argv, _ in calls)
    assert not (out / "execution-context.json").exists()
    assert not (out / "sdk_receipt.json").exists()


@pytest.mark.parametrize("name", ["rust-build.stdout", "metadata.stdout"])
def test_collection_refuses_changed_input_before_launch(tmp_path, monkeypatch, name):
    build, metadata = b"build", b"metadata"
    (tmp_path / "rust-build.stdout").write_bytes(build)
    (tmp_path / "metadata.stdout").write_bytes(metadata)
    build = portable_proof.RawFile.capture(tmp_path / "rust-build.stdout")
    metadata = portable_proof.RawFile.capture(tmp_path / "metadata.stdout")
    (tmp_path / name).write_bytes(b"different identity")
    monkeypatch.setattr(
        portable_proof, "_run", lambda *_a, **_k: pytest.fail("collection launched")
    )
    with pytest.raises(ValueError, match="nextest reuse input"):
        portable_proof._run_reused_nextest(
            "/wrapper", tmp_path, [], build, metadata, env_overrides={}, operation="list"
        )


@pytest.mark.parametrize("value", ["4", "", "-1"])
@pytest.mark.parametrize(
    "key",
    [
        "CARGO_BUILD_JOBS",
        "QUANTA_INDEX_RESOURCE_ADMISSION",
        "QUANTA_INDEX_RESOURCE_WAIT_SECONDS",
        "QUANTA_INDEX_RESOURCE_TIMEOUT_SECONDS",
    ],
)
def test_explicit_resource_control_is_part_of_recorded_environment(
    fake_execution, monkeypatch, key, value
):
    out, _, _ = fake_execution
    monkeypatch.setenv(key, value)
    context = json.loads(portable_proof.produce("sdk", out).read_bytes())
    assert all(row["inherited_environment"][key] == value for row in context["commands"])
    assert portable_proof._environment_digest({key: value}) != portable_proof._environment_digest(
        {}
    )
