#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT_DIR}"

if rg -n '^\s*#!?\[allow\(' crates --glob '*.rs'; then
  echo "Rust #[allow] attributes are banned. Use #[expect(..., reason = ...)] only through an approved exception." >&2
  exit 1
fi
