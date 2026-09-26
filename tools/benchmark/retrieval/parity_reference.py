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
from pathlib import Path

SCHEMA_VERSION = 1
REFERENCE_PROFILE = "model2vec-static-potion-code-16M-v2"
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
    norm = math.sqrt(sum(value * value for value in vector))
    if norm == 0.0:
        return [0.0 for _ in vector]
    return [value / norm for value in vector]


def cosine(left: list[float], right: list[float]) -> float:
    return sum(a * b for a, b in zip(left, right))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model-dir", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    args = parser.parse_args()

    asset_digests = verify_reference_inputs(args.model_dir)
    from model2vec import StaticModel

    model = StaticModel.from_pretrained(str(args.model_dir))
    model_id = getattr(model, "model_name", None) or str(args.model_dir)

    # The reference contract: no truncation (max_length=None), matching
    # the Rust decoder's unbounded pooling. model2vec 0.9.0 encode applies
    # internal L2 normalization, so the pinned reference output IS the
    # unit layer; the Rust rail compares its L2-normalized output here.
    vectors = model.encode(INPUTS, max_length=None).tolist()
    permuted = model.encode(list(reversed(INPUTS)), max_length=None).tolist()
    for index, vector in enumerate(reversed(permuted)):
        assert vector == vectors[index], f"batch permutation changed vector {index}"

    norms = [math.sqrt(sum(value * value for value in vector)) for vector in vectors]
    # The model output is approximately-unit (fp16 rounding leaves norms
    # like 1.0068), so pairwise cosine compares DIRECTIONS: both sides
    # are explicitly L2-normalized before the dot product.
    unit = [l2_normalize(vector) for vector in vectors]
    pairwise = [
        [cosine(unit[i], unit[j]) for j in range(i + 1, len(INPUTS))]
        for i in range(len(INPUTS))
    ]

    payload = {
        "schema_version": SCHEMA_VERSION,
        "profile": REFERENCE_PROFILE,
        "library": {"model2vec": MODEL2VEC_VERSION},
        "model": {
            "id": model_id,
            "dir_name": args.model_dir.name,
            "safetensors_sha256": asset_digests["model.safetensors"],
            "tokenizer_sha256": asset_digests["tokenizer.json"],
            "config_sha256": asset_digests["config.json"],
        },
        "policy": {"max_length": None, "normalization": "approx-unit-fp16 (rail L2-normalizes both sides)"},
        "inputs": INPUTS,
        "vectors": vectors,
        "norms": norms,
        "pairwise_cosine_upper": pairwise,
        "dimension": len(vectors[0]) if vectors else 0,
    }
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(payload, sort_keys=True, indent=2) + "\n")
    print(
        f"parity reference written: {args.out} "
        f"({len(INPUTS)} inputs x {payload['dimension']} dims)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
