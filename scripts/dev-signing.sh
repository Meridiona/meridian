#!/usr/bin/env bash
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
#
# Stable DEV code-signing identity, so macOS TCC permission grants (Screen
# Recording / Accessibility / Input Monitoring) PERSIST across rebuilds.
#
# Why: every `tauri build` is otherwise ad-hoc signed with a FRESH code hash
# (cdhash). macOS anchors a permission grant to the signing identity, so a new
# cdhash looks like a brand-new app — it re-prompts on every rebuild and leaves
# dead TCC records behind (the repeated "Meridian would like to record" loop).
# A single constant self-signed cert ("Meridian Dev") gives one stable identity
# TCC can hold the grant against. Shipping uses Developer ID + notarization; this
# is the dev-loop stand-in until then.
#
#   bash scripts/dev-signing.sh setup      # create the cert ONCE (idempotent)
#   bash scripts/dev-signing.sh identity   # echo the identity for a build to use
#
# `npm run build` / `build:staging` call `identity` automatically (respecting an
# externally-set APPLE_SIGNING_IDENTITY first, e.g. CI's Developer ID): it returns
# "Meridian Dev" when the cert exists, else ad-hoc "-" so CI / cert-less machines
# still build. After `setup`, rebuild, grant the permissions ONCE, and every
# future rebuild keeps them.
set -euo pipefail
CN="Meridian Dev"

has_cert() { security find-identity -p codesigning 2>/dev/null | grep -q "$CN"; }

case "${1:-identity}" in
  setup)
    if has_cert; then echo "✓ '$CN' code-signing identity already exists"; exit 0; fi
    dir="$(mktemp -d)"; pw="meridiandev"
    # Clean up the temp key material on ANY exit (including an early `set -e`
    # failure before the explicit rm below), so the private key never lingers.
    trap 'rm -rf "$dir"' EXIT
    cat > "$dir/c.conf" <<EOF
[req]
distinguished_name=dn
x509_extensions=v3
prompt=no
[dn]
CN=$CN
[v3]
basicConstraints=critical,CA:false
keyUsage=critical,digitalSignature
extendedKeyUsage=critical,codeSigning
EOF
    # Use the SYSTEM LibreSSL: Homebrew OpenSSL 3 writes a PKCS#12 MAC that macOS
    # `security import` rejects ("MAC verification failed during PKCS12 import").
    /usr/bin/openssl req -x509 -newkey rsa:2048 -nodes -days 3650 \
      -keyout "$dir/k.pem" -out "$dir/c.pem" -config "$dir/c.conf" >/dev/null 2>&1
    /usr/bin/openssl pkcs12 -export -inkey "$dir/k.pem" -in "$dir/c.pem" \
      -name "$CN" -out "$dir/c.p12" -passout "pass:$pw" >/dev/null 2>&1
    # -T /usr/bin/codesign grants ONLY codesign access to the imported key; do not
    # add -A (which would let any local app use this signing key).
    security import "$dir/c.p12" -k "$HOME/Library/Keychains/login.keychain-db" \
      -P "$pw" -T /usr/bin/codesign >/dev/null
    rm -rf "$dir"; trap - EXIT
    echo "✓ created '$CN' code-signing identity (self-signed; 'not trusted' is expected & fine)."
    echo "  Next: 'npm run build:staging' → grant the 3 macOS permissions ONCE → done."
    echo "  Every future rebuild now keeps the grants (same stable identity)."
    ;;
  identity)
    # The dev cert if present, else ad-hoc '-' so CI / cert-less machines build.
    if has_cert; then echo "$CN"; else echo "-"; fi
    ;;
  *)
    echo "usage: $0 [setup|identity]" >&2
    exit 2
    ;;
esac
