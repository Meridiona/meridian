#!/usr/bin/env bash
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
#
# Clean-machine smoke test for the MLX runtime tarball.
#
# Extracts the tarball to a FRESH path (deliberately not where it was built),
# exactly as the app does (`tar --strip-components=1` into ~/.meridian/runtime/),
# then proves it actually works: import the native stack, boot the server, and
# hit /health. Green here ⇒ green on a customer's Mac. Catches arch / Python-ABI
# / macOS-deployment-target / relocation mismatches in one shot — the failures
# that otherwise only surface on the user's machine.
#
# Run on a DIFFERENT runner/macOS version than the build for a real portability
# check. Usage: scripts/smoke-test-mlx-runtime.sh <path-to-tarball>
set -euo pipefail

TARBALL="${1:?usage: smoke-test-mlx-runtime.sh <tarball>}"
PORT="${SMOKE_PORT:-7799}"   # not 7823, so we never collide with a real server

[[ "$(uname -m)" == "arm64" ]] || { echo "✗ smoke test must run on arm64 (got $(uname -m))" >&2; exit 1; }
echo "→ macOS $(sw_vers -productVersion 2>/dev/null || echo '?')  arch $(uname -m)"

# Extract to a fresh dir, mirroring the app's extraction exactly.
TMP="$(mktemp -d)"
trap 'rm -rf "${TMP}"' EXIT
RT="${TMP}/runtime"
mkdir -p "${RT}"
tar -xzf "${TARBALL}" -C "${RT}" --strip-components=1

PY="${RT}/bin/python"
[[ -x "${PY}" ]] || { echo "✗ ${PY} missing/not executable after extraction" >&2; exit 1; }
echo "→ interpreter: $("${PY}" --version)"

# 1. Native import check — the relocation/ABI smoke. If mlx loads from a fresh
#    path on a clean machine, the heavy lifting is proven.
echo "→ importing native stack…"
"${PY}" -c "import mlx, mlx_lm, outlines, fastapi, agents; print('  imports ok:', mlx.__name__, mlx_lm.__name__)"

# 2. Boot the server and probe /health (the model lazy-loads on first request,
#    so this does NOT need the 7 GB download).
echo "→ booting server on :${PORT}…"
"${PY}" "${RT}/server.py" --port "${PORT}" >"${TMP}/server.log" 2>&1 &
SRV_PID=$!
trap 'kill "${SRV_PID}" 2>/dev/null || true; rm -rf "${TMP}"' EXIT

ok=""
for i in $(seq 1 40); do
    sleep 2
    if ! kill -0 "${SRV_PID}" 2>/dev/null; then
        echo "✗ server exited during startup; log:" >&2
        tail -30 "${TMP}/server.log" >&2
        exit 1
    fi
    code="$(curl -s -m 3 -o /dev/null -w '%{http_code}' "http://127.0.0.1:${PORT}/health" 2>/dev/null || echo 000)"
    if [[ "${code}" == "200" ]]; then
        echo "✓ /health 200 after ~$(( i * 2 ))s"
        curl -s "http://127.0.0.1:${PORT}/health"; echo
        ok="yes"
        break
    fi
done

if [[ -z "${ok}" ]]; then
    echo "✗ /health never returned 200 within 80s; log:" >&2
    tail -30 "${TMP}/server.log" >&2
    exit 1
fi

echo "✓ runtime smoke test passed"
