#!/usr/bin/env bash
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
#
# Real-GitHub dry-run of the DMG auto-update flow — the faithful end-to-end test
# (real HTTPS, real GitHub asset URLs, real Gatekeeper/quarantine semantics),
# without touching the production release line.
#
#   bash scripts/test-updater-github.sh
#
# What it does:
#   1. builds v0.1.0 (the "installed" app) with the updater endpoint pointed at a
#      throwaway `updater-test` PRE-release tag (NOT /latest/, so the real npm
#      release line is untouched);
#   2. builds v0.1.1 (the payload) — same minisign key, real `.app.tar.gz`+`.sig`;
#   3. writes `latest.json` from the REAL signature and publishes it + the tarball
#      to a `updater-test` prerelease (target: main; assets only — the source the
#      tag points at is irrelevant);
#   4. stages v0.1.0 locally and prints what to click.
#
# Clean up after: gh release delete updater-test --repo Meridiona/meridian --yes --cleanup-tag
#
# Prereqs: ~/.tauri/meridian-updater.key + TAURI_SIGNING_PRIVATE_KEY_PASSWORD in
# env; `gh` authenticated; `bash scripts/dev-signing.sh setup` done once; the
# screenpipe-fork git creds (the default `capture` build pulls the private fork).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT}"

KEY="${HOME}/.tauri/meridian-updater.key"
CONF="tray/src-tauri/tauri.conf.json"
BUNDLE="target/release/bundle/macos"
REPO="Meridiona/meridian"
TAG="updater-test"
SERVE_DIR="${HOME}/meridian-updater-test"
OLD_VER="0.1.0"
NEW_VER="0.1.1"
ENDPOINT="https://github.com/${REPO}/releases/download/${TAG}/latest.json"
TARBALL_URL="https://github.com/${REPO}/releases/download/${TAG}/Meridian.app.tar.gz"

# ── preconditions ────────────────────────────────────────────────────────────
[[ -f "${KEY}" ]] || { echo "✗ updater key not found at ${KEY}" >&2; exit 1; }
command -v gh >/dev/null || { echo "✗ gh CLI required" >&2; exit 1; }
gh auth status >/dev/null 2>&1 || { echo "✗ gh not authenticated — run: gh auth login" >&2; exit 1; }
command -v python3 >/dev/null || { echo "✗ python3 required" >&2; exit 1; }

export TAURI_SIGNING_PRIVATE_KEY="$(cat "${KEY}")"
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD="${TAURI_SIGNING_PRIVATE_KEY_PASSWORD:-}"

# Restore the committed config whatever happens.
cp "${CONF}" "${CONF}.testbak"
trap 'mv -f "${CONF}.testbak" "${CONF}" 2>/dev/null || true' EXIT INT TERM

# Patch <version> + point the endpoint at the throwaway tag (HTTPS — no insecure
# flag needed). Pubkey is preserved, so minisign verification is exercised.
patch_conf() {
  python3 - "${CONF}" "$1" "${ENDPOINT}" <<'PY'
import json, sys
conf, ver, endpoint = sys.argv[1], sys.argv[2], sys.argv[3]
with open(conf) as fh:
    d = json.load(fh)
d["version"] = ver
up = d.setdefault("plugins", {}).setdefault("updater", {})
up["endpoints"] = [endpoint]
up.pop("dangerousInsecureTransportProtocol", None)  # GitHub is HTTPS
with open(conf, "w") as fh:
    json.dump(d, fh, indent=2)
    fh.write("\n")
PY
}

build() {
  echo ""
  echo "▶ building v$1 (slow — release build + bundle + sign)…"
  ( cd tray && npm run build ) >/dev/null
  [[ -f "${BUNDLE}/Meridian.app.tar.gz" ]] || { echo "✗ updater tarball not produced" >&2; exit 1; }
}

mkdir -p "${SERVE_DIR}"
rm -rf "${SERVE_DIR:?}/Meridian.app" "${SERVE_DIR}/Meridian.app.tar.gz" "${SERVE_DIR}/latest.json"

# ── 1. v0.1.0 checker (endpoint → test tag) ──────────────────────────────────
patch_conf "${OLD_VER}"
build "${OLD_VER}"
cp -R "${BUNDLE}/Meridian.app" "${SERVE_DIR}/Meridian.app"
echo "✓ staged v${OLD_VER} → ${SERVE_DIR}/Meridian.app"

# ── 2. v0.1.1 payload ────────────────────────────────────────────────────────
patch_conf "${NEW_VER}"
build "${NEW_VER}"
cp "${BUNDLE}/Meridian.app.tar.gz" "${SERVE_DIR}/Meridian.app.tar.gz"
SIG="$(cat "${BUNDLE}/Meridian.app.tar.gz.sig")"

# ── 3. latest.json from the REAL signature ───────────────────────────────────
python3 - "${SERVE_DIR}/latest.json" "${NEW_VER}" "${TARBALL_URL}" "${SIG}" <<'PY'
import json, sys
out, ver, url, sig = sys.argv[1:5]
manifest = {
    "version": ver,
    "notes": "Throwaway updater dry-run build.",
    "pub_date": "2026-01-01T00:00:00Z",
    "platforms": {"darwin-aarch64": {"signature": sig, "url": url}},
}
with open(out, "w") as fh:
    json.dump(manifest, fh, indent=2)
PY
echo "✓ wrote latest.json (v${NEW_VER} → ${TARBALL_URL})"

mv -f "${CONF}.testbak" "${CONF}"  # restore committed config now

# ── 4. publish the throwaway prerelease ──────────────────────────────────────
gh release delete "${TAG}" --repo "${REPO}" --yes --cleanup-tag 2>/dev/null || true
gh release create "${TAG}" --repo "${REPO}" --target main --prerelease \
  --title "Updater dry-run (safe to delete)" \
  --notes "Throwaway release for testing tauri-plugin-updater. Not a real release. Delete with: gh release delete ${TAG} --cleanup-tag" \
  "${SERVE_DIR}/Meridian.app.tar.gz" "${SERVE_DIR}/latest.json"

cat <<EOF

────────────────────────────────────────────────────────────────────────────
  Published ${TAG} prerelease with v${NEW_VER}. Now test the update:
────────────────────────────────────────────────────────────────────────────
  1. Launch the v${OLD_VER} app (run from terminal to see the update: logs):

       ${SERVE_DIR}/Meridian.app/Contents/MacOS/meridian-tray

  2. Open the popover (click the tray icon) OR the dashboard sidebar.
       → the "Update available  v${OLD_VER} → v${NEW_VER}" banner should appear
       → click it / "Restart & Update" → downloads from GitHub → relaunches

  3. After relaunch, open the banner spot again → it should be GONE
       (a re-check now returns up-to-date). That confirms v${NEW_VER} is running.

  Cleanup when done:
     gh release delete ${TAG} --repo ${REPO} --yes --cleanup-tag
     rm -rf ${SERVE_DIR}
────────────────────────────────────────────────────────────────────────────
EOF
