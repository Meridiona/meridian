#!/usr/bin/env bash
# Smoke-test a deployed hf-proxy: pull a small PUBLIC model file through it,
# confirm it proxies Hugging Face, and report the edge cache status.
#
#   bash smoke.sh https://hf.meridiona.com
#
# Run it twice: the first pull is a MISS (fills Cache Reserve), the second should
# report `cf-cache-status: HIT` once Cache Reserve is enabled + warm. A HIT on the
# large weight file is the signal the proxy is actually accelerating downloads.
set -euo pipefail

BASE="${1:?usage: bash smoke.sh <proxy-base-url>   e.g. https://hf.meridiona.com}"
# A tiny public file from the default llm repo — cheap to fetch, always present.
FILE="/mlx-community/Qwen3.5-2B-OptiQ-4bit/resolve/main/config.json"
OUT="$(mktemp)"

echo "→ GET ${BASE}${FILE}"
curl -fsS -D - -o "${OUT}" "${BASE}${FILE}" \
  | grep -iE '^(HTTP/|cf-cache-status:|x-hf-proxy:|content-length:|content-type:)' || true

echo "→ body: $(wc -c < "${OUT}") bytes"
python3 -c "import json; json.load(open('${OUT}')); print('✓ valid model config served through the proxy')"
rm -f "${OUT}"
echo "(run again — the second fetch should show 'cf-cache-status: HIT' once Cache Reserve is on)"
