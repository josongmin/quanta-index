"""Pinned, local Jina code embeddings for retrieval benchmarks.

The model and its custom code are external artifacts. This server never fetches
files: it verifies a manifest and every artifact before importing model code.
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.metadata
import json
import math
import os
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path

MODEL_ID = "jinaai/jina-embeddings-v2-base-code"
MODEL_REVISION = "516f4baf13dec4ddddda8631e019b5737c8bc250"
CODE_REVISION = "3baf9e3ac750e76e8edd3019170176884695fb94"
DIMENSIONS = 768
CONTEXT_TOKENS = 8192
MAX_BODY_BYTES = 1_048_576
MAX_ITEMS = 64
MAX_SUBBATCH = 2
MAX_SUBBATCH_TOKENS = 2048
EXPECTED_SHA256 = {
    "model/config.json": "e426aa684c7f9a95c5f020aa855faf93a24f065f5fad0c9e17b124670cabdea6",
    "model/tokenizer.json": "b01c78a902aa4facb2f47f95449f48e2f7bbfea5d2472ee2f6ce92323c6f86e5",
    "model/tokenizer_config.json": "f477aeb15ff9f78d3c1ddf2361d2b0b8b20cf55220f839f29a37f3a18efddd89",
    "model/special_tokens_map.json": "06e405a36dfe4b9604f484f6a1e619af1a7f7d09e34a8555eb0b77b66318067f",
    "model/model.safetensors": "8b53bfd4ae2cd586004a6ca4a16551b630a2a1b1d655ff1ee9be1286a1781c5b",
    "code/configuration_bert.py": "d018de11bcb62cedf436c8876459e5cd119d1faf72bc107338576a6d1b8eeea5",
    "code/modeling_bert.py": "8f96ef0576401f6b996f9b72ad34e13190139eec1195767516485876434bb612",
}


class RequestError(Exception):
    def __init__(self, status: int, message: str):
        super().__init__(message)
        self.status = status


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def verify_artifacts(root: Path, manifest_path: Path) -> dict:
    manifest_bytes = manifest_path.read_bytes()
    manifest = json.loads(manifest_bytes.decode("utf-8"))
    expected = {
        "model": MODEL_ID,
        "revision": MODEL_REVISION,
        "code_revision": CODE_REVISION,
        "dimensions": DIMENSIONS,
        "context_tokens": CONTEXT_TOKENS,
        "pooling": "attention_mask_mean",
        "normalization": "l2",
        "sha256": EXPECTED_SHA256,
    }
    for key, value in expected.items():
        if manifest.get(key) != value:
            raise ValueError(f"manifest {key} differs from pinned contract")
    for relative, expected_hash in EXPECTED_SHA256.items():
        path = root / relative
        if not path.is_file() or path.is_symlink() or sha256_file(path) != expected_hash:
            raise ValueError(f"pinned artifact failed SHA256 verification: {relative}")
    manifest["_manifest_sha256"] = hashlib.sha256(manifest_bytes).hexdigest()
    return manifest


def load_model(root: Path, manifest: dict):
    # Imports occur only after the custom model code and model files pass custody.
    os.environ["HF_HOME"] = str(root / "cache")
    os.environ["HF_MODULES_CACHE"] = str(root / "cache/modules")
    os.environ["HF_HUB_OFFLINE"] = "1"
    os.environ["TRANSFORMERS_OFFLINE"] = "1"
    import torch
    import transformers
    from transformers import AutoModel, AutoTokenizer

    versions = {
        "python": sys.version.split()[0],
        "torch": torch.__version__,
        "transformers": transformers.__version__,
        "tokenizers": importlib.metadata.version("tokenizers"),
        "safetensors": importlib.metadata.version("safetensors"),
        "huggingface_hub": importlib.metadata.version("huggingface-hub"),
        "numpy": importlib.metadata.version("numpy"),
    }
    if manifest.get("runtime_versions") != versions:
        raise ValueError("runtime versions differ from manifest")
    model_dir = root / "runtime"
    source = json.loads((root / "model/config.json").read_text(encoding="utf-8"))
    source["auto_map"] = {key: value.split("--")[-1] for key, value in source["auto_map"].items()}
    if json.loads((model_dir / "config.json").read_text(encoding="utf-8")) != source:
        raise ValueError("local runtime config differs from pinned source")
    for name in ("tokenizer.json", "tokenizer_config.json", "special_tokens_map.json"):
        if sha256_file(model_dir / name) != EXPECTED_SHA256[f"model/{name}"]:
            raise ValueError(f"runtime tokenizer differs: {name}")
    for name in ("configuration_bert.py", "modeling_bert.py"):
        if sha256_file(model_dir / name) != EXPECTED_SHA256[f"code/{name}"]:
            raise ValueError(f"runtime model code differs: {name}")
    weight = model_dir / "model.safetensors"
    if not weight.is_symlink() or weight.resolve() != (root / "model/model.safetensors").resolve():
        raise ValueError("runtime weight link differs from pinned artifact")
    device = "mps" if torch.backends.mps.is_available() else "cpu"
    model = (
        AutoModel.from_pretrained(
            str(model_dir),
            trust_remote_code=True,
            local_files_only=True,
            use_safetensors=True,
            torch_dtype=torch.float32,
        )
        .to(device)
        .eval()
    )
    tokenizer = AutoTokenizer.from_pretrained(str(model_dir), local_files_only=True, use_fast=True)
    if model.config.hidden_size != DIMENSIONS or tokenizer.model_max_length != CONTEXT_TOKENS:
        raise ValueError("loaded model dimension or tokenizer context differs")
    return torch, tokenizer, model, device, versions


def embed(torch, tokenizer, model, device: str, inputs: list[str], max_input_tokens: int):
    tokenized = [
        tokenizer(text, add_special_tokens=True, truncation=False, max_length=CONTEXT_TOKENS)[
            "input_ids"
        ]
        for text in inputs
    ]
    lengths = [len(tokens) for tokens in tokenized]
    if any(length > max_input_tokens for length in lengths):
        raise RequestError(413, f"input exceeds {max_input_tokens} tokens")
    vectors = []
    start = 0
    while start < len(inputs):
        end = start + 1
        while (
            end < len(inputs)
            and end - start < MAX_SUBBATCH
            and max(lengths[start : end + 1]) * (end + 1 - start) <= MAX_SUBBATCH_TOKENS
        ):
            end += 1
        group = inputs[start:end]
        encoded = tokenizer(group, padding=True, truncation=False, return_tensors="pt").to(device)
        with torch.inference_mode():
            hidden = model(**encoded).last_hidden_state
            mask = encoded["attention_mask"].unsqueeze(-1)
            pooled = (hidden * mask).sum(dim=1) / mask.sum(dim=1).clamp(min=1)
            normalized = torch.nn.functional.normalize(pooled.float(), p=2, dim=1)
        for row in normalized.cpu().tolist():
            norm = math.sqrt(sum(value * value for value in row))
            if (
                len(row) != DIMENSIONS
                or not all(math.isfinite(value) for value in row)
                or not 0.999 <= norm <= 1.001
            ):
                raise RuntimeError("model returned invalid embedding")
            vectors.append(row)
        start = end
    return vectors, sum(lengths)


def make_handler(state):
    class Handler(BaseHTTPRequestHandler):
        server_version = "LocalCodeEmbedding/1"
        sys_version = ""

        def log_message(self, format, *args):
            # Never log request bodies, model inputs, or URL parameters.
            pass

        def _reply(self, status: int, payload: dict):
            body = json.dumps(payload, allow_nan=False, separators=(",", ":")).encode("utf-8")
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Cache-Control", "no-store")
            self.end_headers()
            self.wfile.write(body)

        def _check_host(self):
            expected = f"127.0.0.1:{self.server.server_port}"
            if self.headers.get("Host") != expected or self.headers.get("Origin") is not None:
                raise RequestError(403, "loopback host required")

        def do_GET(self):
            try:
                self._check_host()
                if self.path != "/healthz":
                    raise RequestError(404, "unknown path")
                self._reply(200, state["health"])
            except RequestError as error:
                self._reply(
                    error.status,
                    {"error": {"message": str(error), "type": "invalid_request_error"}},
                )

        def do_POST(self):
            try:
                self._check_host()
                if self.path != "/v1/embeddings":
                    raise RequestError(404, "unknown path")
                if (
                    self.headers.get("Content-Type", "").split(";", 1)[0].lower()
                    != "application/json"
                ):
                    raise RequestError(415, "application/json required")
                if self.headers.get("Transfer-Encoding") is not None:
                    raise RequestError(400, "chunked bodies unsupported")
                content_length = self.headers.get("Content-Length", "")
                if not content_length.isdecimal() or not 0 < int(content_length) <= MAX_BODY_BYTES:
                    raise RequestError(413, "invalid or excessive body length")
                try:
                    request = json.loads(self.rfile.read(int(content_length)).decode("utf-8"))
                except (UnicodeDecodeError, json.JSONDecodeError):
                    raise RequestError(400, "invalid UTF-8 JSON") from None
                if not isinstance(request, dict) or request.get("model") != MODEL_ID:
                    raise RequestError(400, "pinned model name required")
                if request.get("dimensions", DIMENSIONS) != DIMENSIONS or isinstance(
                    request.get("dimensions", DIMENSIONS), bool
                ):
                    raise RequestError(400, "dimensions must be 768")
                inputs = request.get("input")
                if isinstance(inputs, str):
                    inputs = [inputs]
                if (
                    not isinstance(inputs, list)
                    or not 1 <= len(inputs) <= MAX_ITEMS
                    or not all(isinstance(item, str) and item for item in inputs)
                ):
                    raise RequestError(400, "input must contain 1 to 64 nonempty strings")
                vectors, token_count = embed(*state["engine"], inputs, state["max_input_tokens"])
                self._reply(
                    200,
                    {
                        "object": "list",
                        "model": MODEL_ID,
                        "data": [
                            {"object": "embedding", "index": i, "embedding": row}
                            for i, row in enumerate(vectors)
                        ],
                        "usage": {"prompt_tokens": token_count, "total_tokens": token_count},
                    },
                )
            except RequestError as error:
                self._reply(
                    error.status,
                    {"error": {"message": str(error), "type": "invalid_request_error"}},
                )

    return Handler


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--artifact-root", required=True, type=Path)
    parser.add_argument("--manifest", required=True, type=Path)
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--max-input-tokens", type=int, default=CONTEXT_TOKENS)
    args = parser.parse_args(argv)
    if not 1 <= args.port <= 65535 or not 1 <= args.max_input_tokens <= CONTEXT_TOKENS:
        parser.error("invalid port or token limit")
    manifest = verify_artifacts(args.artifact_root, args.manifest)
    engine = load_model(args.artifact_root, manifest)
    torch, tokenizer, model, device, versions = engine
    health = {
        "model": MODEL_ID,
        "revision": MODEL_REVISION,
        "code_revision": CODE_REVISION,
        "artifact_sha256": EXPECTED_SHA256["model/model.safetensors"],
        "manifest_sha256": manifest["_manifest_sha256"],
        "sha256": EXPECTED_SHA256,
        "dimensions": DIMENSIONS,
        "context_tokens": CONTEXT_TOKENS,
        "pooling": "attention_mask_mean",
        "normalization": "l2",
        "device": device,
        "dtype": "float32",
        "runtime_versions": versions,
        "max_input_tokens": args.max_input_tokens,
    }
    state = {
        "health": health,
        "engine": (torch, tokenizer, model, device),
        "max_input_tokens": args.max_input_tokens,
    }
    server = HTTPServer(("127.0.0.1", args.port), make_handler(state))
    print(
        f"local code embedding ready on 127.0.0.1:{server.server_port} device={device}", flush=True
    )
    server.serve_forever(poll_interval=0.5)


if __name__ == "__main__":
    raise SystemExit(main())
