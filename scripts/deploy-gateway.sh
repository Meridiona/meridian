#!/usr/bin/env bash
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
#
# Push ops/central-observability/ to the central telemetry gateway VM and apply
# it. Without this, the running config is whatever someone last `scp`-ed by
# hand, and a stale-but-working gateway is indistinguishable from a current one
# — the drift is invisible precisely because nothing breaks.
#
# What it does NOT touch: `.env` on the VM. That file holds the ingest token,
# the OpenObserve root password, and the UI basic-auth hash; it is generated
# once on the box and never lives in git. Copying over it would destroy the
# gateway's credentials, so `.env` is deliberately absent from FILES below —
# do not add it.
#
# New keys added to `.env.example` do NOT reach the VM's `.env` automatically.
# The script warns about the gap rather than guessing values; give every new
# key a compose-level default (`${FOO:-fallback}`) so an un-updated `.env` still
# renders. See ZO_COMPACT_DATA_RETENTION_DAYS in docker-compose.yml.
#
# Usage:
#   bash scripts/deploy-gateway.sh
#   GATEWAY_VM=… GATEWAY_ZONE=… GATEWAY_PROJECT=… bash scripts/deploy-gateway.sh
#
# Requires: gcloud authenticated with access to the gateway project.

set -euo pipefail

VM="${GATEWAY_VM:-meridian-telemetry}"
ZONE="${GATEWAY_ZONE:-asia-south1-a}"
PROJECT="${GATEWAY_PROJECT:-meridiona-observability}"
REMOTE_DIR="central-observability"

# Everything git owns. `.env` is intentionally not here — see the header.
FILES=(docker-compose.yml otel-collector-config.yaml Caddyfile .env.example)

SRC="$(cd "$(dirname "${BASH_SOURCE[0]}")/../ops/central-observability" && pwd)"

ssh_vm() {
	gcloud compute ssh "${VM}" --zone="${ZONE}" --project="${PROJECT}" --command "$1"
}

echo "==> deploying ${SRC} -> ${VM} (${ZONE}, ${PROJECT})"

for f in "${FILES[@]}"; do
	echo "  - ${f}"
	gcloud compute scp "${SRC}/${f}" "${VM}:${REMOTE_DIR}/${f}" \
		--zone="${ZONE}" --project="${PROJECT}" --quiet
done

# Keys the repo now expects that the VM's .env has never heard of. Not fatal —
# compose defaults cover the well-behaved ones — but a silent blank is how you
# end up debugging OpenObserve instead of reading this line.
echo "==> checking for .env keys the VM is missing"
ssh_vm "cd ${REMOTE_DIR} \
	&& grep -oE '^[A-Z_]+' .env.example | sort -u > /tmp/.keys-expected \
	&& grep -oE '^[A-Z_]+' .env         | sort -u > /tmp/.keys-actual \
	&& missing=\$(comm -23 /tmp/.keys-expected /tmp/.keys-actual) \
	&& rm -f /tmp/.keys-expected /tmp/.keys-actual \
	&& if [ -n \"\$missing\" ]; then \
		echo 'WARNING: present in .env.example, absent from .env:'; echo \"\$missing\"; \
	   else echo '  none'; fi"

# Catches YAML/interpolation breakage before it can take the stack down.
echo "==> validating rendered config"
ssh_vm "cd ${REMOTE_DIR} && docker compose config -q && echo '  ok'"

# `up -d` rather than `restart`: environment variables are baked into a
# container at CREATE time, so `restart` would cheerfully bring the old config
# back up and the deploy would appear to succeed while changing nothing.
echo "==> applying"
ssh_vm "cd ${REMOTE_DIR} && docker compose up -d && docker compose ps"

echo "==> done. Verify ingest still works before walking away:"
echo "    https://telemetry.meridiona.com  should 401 without a Bearer token"
