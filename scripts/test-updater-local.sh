#!/usr/bin/env bash
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
#
# Step-2 gate for the DMG auto-update plan: prove the ad-hoc-signed bundle
# updates + relaunches cleanly, locally, with NO public artifacts.
#
#   bash scripts/test-updater-local.sh
#
# What it does (self-contained, ~2 slow builds):
#   1. builds v0.1.0 (the "installed" app that will check for updates), with the
#      updater endpoint temporarily pointed at http://localhost:8000 and
#      `dangerousInsecureTransportProtocol` on, so no HTTPS host is needed;
#   2. builds v0.1.1 (the update payload) — same minisign key, real `.app.tar.gz`
#      + `.sig`;
#   3. writes a `latest.json` from the REAL signature and serves it + the tarball
#      over localhost;
#   4. stages v0.1.0 to a throwaway dir and tells you exactly what to click.
#
# Faithful to the real unknown: the minisign signature is verified for real, and
# the bundle that gets swapped in + relaunched is the genuine ad-hoc/dev-signed
# `.app`. Only the transport (http/localhost vs https/GitHub) is faked — which
# has no bearing on whether macOS relaunches the updated bundle. The repo's
# tauri.conf.json is restored on exit (trap), so the working tree stays clean.
#
# Prereqs: `bash scripts/dev-signing.sh setup` done once; the updater key at
# ~/.tauri/meridian-updater.key; python3; the screenpipe-fork git creds (the
# default `capture` build pulls the private fork).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT}"

KEY="${HOME}/.tauri/meridian-updater.key"
CONF="tray/src-tauri/tauri.conf.json"
BUNDLE="target/release/bundle/macos"
PORT=8000
SERVE_DIR="${HOME}/meridian-updater-test"
OLD_VER="0.1.0"
NEW_VER="0.1.1"

# ── preconditions ────────────────────────────────────────────────────────────
[[ -f "${KEY}" ]] || { echo "✗ updater key not found at ${KEY} — run: cd tray && npm run tauri signer generate -- -w ${KEY}" >&2; exit 1; }
command -v python3 >/dev/null || { echo "✗ python3 required" >&2; exit 1; }

export TAURI_SIGNING_PRIVATE_KEY="$(cat "${KEY}")"
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD="${TAURI_SIGNING_PRIVATE_KEY_PASSWORD:-}"

SERVER_PID=""
# Restore the committed config and stop the server no matter how we exit.
cleanup() {
  [[ -n "${SERVER_PID}" ]] && kill "${SERVER_PID}" 2>/dev/null || true
  if [[ -f "${CONF}.testbak" ]]; then
    mv -f "${CONF}.testbak" "${CONF}"
    echo "↩︎  restored ${CONF}"
  fi
}
trap cleanup EXIT INT TERM

cp "${CONF}" "${CONF}.testbak"

# Patch tauri.conf.json: <version>, localhost endpoint, allow insecure transport.
patch_conf() {
  local ver="$1"
  python3 - "${CONF}" "${ver}" "${PORT}" <<'PY'
import json, sys
conf, ver, port = sys.argv[1], sys.argv[2], sys.argv[3]
with open(conf) as fh:
    d = json.load(fh)
d["version"] = ver
up = d.setdefault("plugins", {}).setdefault("updater", {})
up["endpoints"] = [f"http://localhost:{port}/latest.json"]
up["dangerousInsecureTransportProtocol"] = True
with open(conf, "w") as fh:
    json.dump(d, fh, indent=2)
    fh.write("\n")
PY
}

build() {
  echo ""
  echo "▶ building v$1 (this is slow — full release build + bundle + sign)…"
  ( cd tray && npm run build ) >/dev/null
  [[ -f "${BUNDLE}/Meridian.app.tar.gz" ]] || { echo "✗ updater tarball not produced — is createUpdaterArtifacts on + TAURI_SIGNING_PRIVATE_KEY set?" >&2; exit 1; }
}

mkdir -p "${SERVE_DIR}"
rm -rf "${SERVE_DIR:?}/Meridian.app" "${SERVE_DIR}/Meridian.app.tar.gz" "${SERVE_DIR}/latest.json"

# ── 1. build v0.1.0 (the checker) and stage it ───────────────────────────────
patch_conf "${OLD_VER}"
build "${OLD_VER}"
cp -R "${BUNDLE}/Meridian.app" "${SERVE_DIR}/Meridian.app"
echo "✓ staged v${OLD_VER} → ${SERVE_DIR}/Meridian.app"

# ── 2. build v0.1.1 (the payload) ────────────────────────────────────────────
patch_conf "${NEW_VER}"
build "${NEW_VER}"
cp "${BUNDLE}/Meridian.app.tar.gz" "${SERVE_DIR}/Meridian.app.tar.gz"
SIG="$(cat "${BUNDLE}/Meridian.app.tar.gz.sig")"

# ── 3. write latest.json from the REAL signature ─────────────────────────────
python3 - "${SERVE_DIR}/latest.json" "${NEW_VER}" "${PORT}" "${SIG}" <<'PY'
import json, sys
out, ver, port, sig = sys.argv[1:5]
manifest = {
    "version": ver,
    "notes": "Local updater test build.",
    "pub_date": "2026-01-01T00:00:00Z",
    "platforms": {
        "darwin-aarch64": {
            "signature": sig,
            "url": f"http://localhost:{port}/Meridian.app.tar.gz",
        }
    },
}
with open(out, "w") as fh:
    json.dump(manifest, fh, indent=2)
PY
echo "✓ wrote ${SERVE_DIR}/latest.json (v${NEW_VER})"

# Restore the committed config now; the running build is self-contained.
mv -f "${CONF}.testbak" "${CONF}"
echo "↩︎  restored ${CONF}"

# ── 4. serve + instruct ──────────────────────────────────────────────────────
( cd "${SERVE_DIR}" && python3 -m http.server "${PORT}" >/dev/null 2>&1 ) &
SERVER_PID=$!
sleep 1

cat <<EOF

────────────────────────────────────────────────────────────────────────────
  Local updater test is ready. Serving v${NEW_VER} on http://localhost:${PORT}
────────────────────────────────────────────────────────────────────────────
  1. Launch the v${OLD_VER} test app:

       open ${SERVE_DIR}/Meridian.app

  2. From the menu-bar tray icon → right-click → "Check for Updates…"
       expect:  "Updating Meridian — Downloading v${NEW_VER}…"
         then:  "Update installed — Restarting into v${NEW_VER}…"
       the app relaunches.  ← this is the thing we're proving (ad-hoc relaunch)

  3. Click "Check for Updates…" ONE more time.
       expect:  "Meridian is up to date"
       ↳ that confirms the running app is now v${NEW_VER}. SUCCESS.

  (A silent check also fires ~5 s after launch; it installs in place but does
   NOT restart — the relaunch only happens on the menu-driven check in step 2.)

  Press Ctrl+C here when done — it stops the server and restores config.
  Cleanup:  rm -rf ${SERVE_DIR}
────────────────────────────────────────────────────────────────────────────
EOF

wait "${SERVER_PID}"
