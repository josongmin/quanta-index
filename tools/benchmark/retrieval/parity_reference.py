#!/usr/bin/env python3
"""Generate the full-vector encoder parity reference fixture (RBR-07).

Runs the pinned model2vec reference inside the frozen Semble venv and
emits a JSON fixture binding every correctness-relevant identity: model
id and file digests, tokenizer digest, the truncation policy, the
adversarial input set (identifiers, qualified names, punctuation/code,
Unicode, empty and whitespace-only tokenless inputs, a long input, a
duplicate, and a batch permutation), the full 256-dimension
L2-normalized vectors, per-input norms, and the pairwise cosine upper
triangle.

The Rust parity rail (crates/quanta-index-embed model2vec tests) verifies
against this fixture. The one-sentence/8-component comparison of the past
is not full parity; this fixture is. Regenerate only with this script;
hand edits break the digest chain.
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.metadata
import json
import math
import sys
from pathlib import Path

SCHEMA_VERSION = 3
REFERENCE_PROFILE = "model2vec-static-potion-code-16M-v2"
MODEL_ID = "minishlab/potion-code-16M-v2"
MODEL_REVISION = "e9d2a44ca6a05ac6685f3b23709ea57eb7352d5b"
MODEL2VEC_VERSION = "0.9.0"
PINNED_ASSET_SHA256 = {
    "model.safetensors": "75cf7a6c2171b230ad19b1e7d8e0b1aee86da5a02af8e7cacedd9921d227623c",
    "tokenizer.json": "107bbdcbad4bff1d299b7a4c3a2fb17c52890688b7dd0e4c9deab79d3c4f3d45",
    "config.json": "148e5691a6fcc553437156859701fba017a1ba5d340b170f17e0f3668fb861a7",
}

INPUTS = [
    "refresh access token",
    "parse_and_expression",
    "quanta_index_retrieval_bench::sdk::query_route",
    'fn main() { println!("{}", x); }',
    "한글 검색 αβγ 🚀",
    "",
    "   ",
    "a" * 5000,
    "route " * 600 + "render content type",
    "route " * 600 + "binding form values",
    "refresh access token",
]


def sha_file(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def verify_reference_inputs(model_dir: Path) -> dict[str, str]:
    """Refuse a reference from unpinned assets or a different library build."""
    version = importlib.metadata.version("model2vec")
    if version != MODEL2VEC_VERSION:
        raise ValueError(f"model2vec version mismatch: {version} != {MODEL2VEC_VERSION}")
    observed = {name: sha_file(model_dir / name) for name in PINNED_ASSET_SHA256}
    for name, expected in PINNED_ASSET_SHA256.items():
        if observed[name] != expected:
            raise ValueError(f"{name} SHA-256 mismatch: {observed[name]} != {expected}")
    return observed


def l2_normalize(vector: list[float]) -> list[float]:
    if len(vector) != 256 or any(not math.isfinite(value) for value in vector):
        raise ValueError("reference vector must contain 256 finite components")
    norm = math.sqrt(sum(value * value for value in vector))
    if not math.isfinite(norm) or norm <= 0.0:
        raise ValueError("reference vector norm must be finite and positive")
    return [value / norm for value in vector]


def cosine(left: list[float], right: list[float]) -> float:
    return sum(a * b for a, b in zip(left, right))


def main() -> int:
    if sys.version_info[:2] != (3, 13):
        raise ValueError("reference execution requires Python 3.13")
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model-dir", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--inputs-json", type=Path)
    parser.add_argument("--model-id", choices=[MODEL_ID], default=MODEL_ID)
    args = parser.parse_args()
    inputs = INPUTS if args.inputs_json is None else json.loads(args.inputs_json.read_bytes())
    if (
        not isinstance(inputs, list)
        or not 0 < len(inputs) <= 4096
        or any(not isinstance(text, str) for text in inputs)
    ):
        raise ValueError("reference inputs must be 1..4096 strings")
    long_suffix_pair = INPUTS[-3:-1]
    if any(text not in inputs for text in long_suffix_pair):
        raise ValueError("reference inputs must include both long suffix probes")

    asset_digests = verify_reference_inputs(args.model_dir)
    from model2vec import StaticModel

    model = StaticModel.from_pretrained(str(args.model_dir))
    # The pinned tokenizer.json itself carries a 512-token truncation. The
    # encode(max_length=None) argument alone does not override that stored
    # tokenizer policy. Disable it on this in-memory model before reference
    # generation, leaving the digest-pinned asset untouched.
    if model.tokenizer.truncation is None or model.tokenizer.truncation["max_length"] != 512:
        raise ValueError("pinned tokenizer must carry a 512-token truncation")
    model.tokenizer.no_truncation()
    if model.tokenizer.truncation is not None:
        raise ValueError("tokenizer truncation remains enabled")
    if any(len(model.tokenizer.encode(text).ids) <= 512 for text in long_suffix_pair):
        raise ValueError("adversarial suffix input does not exceed the old cap")

    # The reference contract: no truncation (max_length=None), matching
    # the Rust decoder's unbounded pooling. model2vec 0.9.0 encode applies
    # internal L2 normalization, so the pinned reference output IS the
    # unit layer; the Rust rail compares its L2-normalized output here.
    vectors = model.encode(inputs, max_length=None).tolist()
    if vectors[inputs.index(long_suffix_pair[0])] == vectors[inputs.index(long_suffix_pair[1])]:
        raise ValueError("long suffix probes embedded identically")
    permuted = model.encode(list(reversed(inputs)), max_length=None).tolist()
    if len(vectors) != len(inputs) or len(permuted) != len(inputs):
        raise ValueError("reference encoder returned an incomplete batch")
    for index, vector in enumerate(reversed(permuted)):
        if vector != vectors[index]:
            raise ValueError(f"batch permutation changed vector {index}")

    norms = [math.sqrt(sum(value * value for value in vector)) for vector in vectors]
    # The model output is approximately-unit (fp16 rounding leaves norms
    # like 1.0068), so pairwise cosine compares DIRECTIONS: both sides
    # are explicitly L2-normalized before the dot product.
    unit = [l2_normalize(vector) for vector in vectors]
    pairwise = [
        [cosine(unit[i], unit[j]) for j in range(i + 1, len(inputs))] for i in range(len(inputs))
    ]

    payload = {
        "schema_version": SCHEMA_VERSION,
        "profile": REFERENCE_PROFILE,
        "library": {"model2vec": MODEL2VEC_VERSION},
        "model": {
            "id": MODEL_ID,
            "revision": MODEL_REVISION,
            "dir_name": args.model_dir.name,
            "safetensors_sha256": asset_digests["model.safetensors"],
            "tokenizer_sha256": asset_digests["tokenizer.json"],
            "config_sha256": asset_digests["config.json"],
        },
        "policy": {
            "max_length": None,
            "tokenizer_embedded_truncation_disabled": True,
            "normalization": "approx-unit-fp16 (rail L2-normalizes both sides)",
        },
        "inputs": inputs,
        "vectors": vectors,
        "norms": norms,
        "pairwise_cosine_upper": pairwise,
        "dimension": len(vectors[0]) if vectors else 0,
    }
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(payload, sort_keys=True, indent=2) + "\n")
    print(
        f"parity reference written: {args.out} ({len(inputs)} inputs x {payload['dimension']} dims)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
