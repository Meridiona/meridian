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

# The two public hostnames Caddy serves (see ops/central-observability/Caddyfile).
# Deliberately NOT read from the VM's .env: this check's whole value is that it
# always runs, and an ssh round-trip to discover the domain adds a failure mode
# where a flake means we can't determine what to probe. If a domain ever drifts,
# probing the old name fails loudly — which is the correct outcome, not a skip.
GATEWAY_DOMAIN="${GATEWAY_DOMAIN:-telemetry.meridiona.com}"
OO_UI_DOMAIN="${OO_UI_DOMAIN:-observe.meridiona.com}"

# How long to keep retrying a transient probe result before giving up. `up -d`
# returns as soon as the containers are created, so the collector can still be
# binding :4318 when the first probe lands.
PROBE_TIMEOUT_S="${PROBE_TIMEOUT_S:-90}"
PROBE_INTERVAL_S="${PROBE_INTERVAL_S:-3}"

# Everything git owns. `.env` is intentionally not here — see the header.
FILES=(docker-compose.yml otel-collector-config.yaml Caddyfile .env.example)

ssh_vm() {
	gcloud compute ssh "${VM}" --zone="${ZONE}" --project="${PROJECT}" --command "$1"
}

# ── Post-deploy auth assertion ───────────────────────────────────────────────
#
# CLAUDE.md's hard rule requires that an unauthenticated request to anything we
# expose publicly is rejected outright with a 401.
#
# This used to be an `echo` at the end of the deploy telling a human to go and
# check. It sent no request and failed on nothing, so a gateway that started
# answering 200 unauthenticated would deploy green and stay that way until
# somebody noticed — which is the shape of the `infra/hf-proxy` incident
# (173,088 requests in a day against a service that had no callers at all). See
# the Hard Rules section of CLAUDE.md for the full story.

# Map an observed HTTP status onto one of three verdicts. Pure — no network, no
# globals — so `--self-test` can exercise every branch offline.
#
#   pass   the required 401
#   retry  the stack is plausibly still coming up (Caddy up, collector not yet)
#   reject anything else, INCLUDING 2xx/3xx/4xx that are not 401
#
# 200 must never be retried: a success on an unauthenticated request is the
# catastrophe this check exists to catch, and retrying it only delays the
# failure. A 400 likewise rejects immediately — it means the body reached the
# collector, i.e. the request got PAST auth.
classify_probe() {
	case "$1" in
	401) echo pass ;;
	000 | 429 | 502 | 503 | 504) echo retry ;;
	*) echo reject ;;
	esac
}

# Probe one URL until it returns 401 or the budget runs out.
#
# `000` is curl's code for "no HTTP response at all" and covers both a collector
# that has not finished starting AND a dead DNS record / broken TLS / blocked
# egress. Those are indistinguishable from the status alone, so it is retried
# and then FAILS — an exhausted retry loop must never fall through to success,
# which would reproduce the very bug this replaces.
probe_rejects_unauthenticated() {
	local label="$1" url="$2"
	shift 2
	local deadline=$((SECONDS + PROBE_TIMEOUT_S)) code verdict
	while :; do
		code="$(curl -s -o /dev/null -w '%{http_code}' --max-time 15 "$@" "${url}" || true)"
		verdict="$(classify_probe "${code}")"
		case "${verdict}" in
		pass)
			echo "  ok   ${label} -> 401"
			return 0
			;;
		reject)
			echo "  FAIL ${label} -> ${code} (expected 401)" >&2
			return 1
			;;
		retry)
			if [ "${SECONDS}" -ge "${deadline}" ]; then
				echo "  FAIL ${label} -> ${code} after ${PROBE_TIMEOUT_S}s (expected 401)" >&2
				return 1
			fi
			sleep "${PROBE_INTERVAL_S}"
			;;
		esac
	done
}

# Assert both public hostnames reject callers who have no credentials AND
# callers whose credentials are wrong.
#
# The wrong-credential probes are the ones that earn their keep. An
# unauthenticated 401 still passes if the auth extension were swapped for
# something that merely checks a header is PRESENT; only a bad token proves the
# value is actually validated.
verify_public_endpoints_authenticate() {
	local failed=0
	probe_rejects_unauthenticated \
		"ingest  no credentials     " "https://${GATEWAY_DOMAIN}/v1/logs" \
		-X POST -H 'Content-Type: application/json' --data '{}' || failed=1
	probe_rejects_unauthenticated \
		"ingest  wrong bearer token " "https://${GATEWAY_DOMAIN}/v1/logs" \
		-X POST -H 'Content-Type: application/json' \
		-H 'Authorization: Bearer deploy-gateway-probe-not-a-real-token' --data '{}' || failed=1
	probe_rejects_unauthenticated \
		"oo-ui   no credentials     " "https://${OO_UI_DOMAIN}/" || failed=1
	probe_rejects_unauthenticated \
		"oo-ui   wrong basic auth   " "https://${OO_UI_DOMAIN}/" \
		-u 'deploy-gateway-probe:not-a-real-password' || failed=1
	return "${failed}"
}

# ── Argument dispatch ────────────────────────────────────────────────────────
#
# Every argument is either recognised or an ERROR. This used to be two
# `if [ "$1" = "--flag" ]` blocks with no else, so ANY other argument fell
# straight through to the deploy: `--help`, `--dry-run`, `-n`, or a typo like
# `--selftest` all pushed config to the production gateway and restarted it.
# The two safest-sounding things a person types when they are unsure what a
# script does were the two most dangerous.
#
# A deploy now requires exactly zero arguments. Anything unrecognised prints
# usage and exits 2 without touching the VM.
case "${1:-}" in
--verify-only | --self-test | "") ;;
-h | --help)
	cat <<-USAGE
		usage: deploy-gateway.sh [--verify-only | --self-test]

		  (no arguments)   deploy ops/central-observability/ to the gateway VM,
		                   then assert both public hostnames reject
		                   unauthenticated callers
		  --verify-only    run only that assertion, against the live gateway,
		                   deploying nothing
		  --self-test      offline check of the status classifier

		env: GATEWAY_VM GATEWAY_ZONE GATEWAY_PROJECT GATEWAY_DOMAIN
		     OO_UI_DOMAIN PROBE_TIMEOUT_S PROBE_INTERVAL_S
	USAGE
	exit 0
	;;
*)
	echo "deploy-gateway.sh: unknown argument '${1}'" >&2
	echo "run with --help for usage; a deploy takes NO arguments" >&2
	exit 2
	;;
esac

# Run the auth assertion on its own, without deploying anything. Two uses: an
# operator re-checking a gateway they did not just deploy, and testing a change
# to the probes themselves — the alternative is running a production deploy to
# exercise four read-only curls.
if [ "${1:-}" = "--verify-only" ]; then
	echo "==> verifying ${GATEWAY_DOMAIN} and ${OO_UI_DOMAIN} reject unauthenticated callers"
	if verify_public_endpoints_authenticate; then
		echo "==> ok. Both hostnames require credentials."
		exit 0
	fi
	echo "==> FAILED. See above." >&2
	exit 1
fi

# Offline check that `classify_probe` still discriminates. Run it after touching
# the table above:  bash scripts/deploy-gateway.sh --self-test
if [ "${1:-}" = "--self-test" ]; then
	self_test_failures=0
	while read -r code want; do
		[ -z "${code}" ] && continue
		got="$(classify_probe "${code}")"
		if [ "${got}" != "${want}" ]; then
			echo "FAIL classify_probe ${code}: want ${want}, got ${got}" >&2
			self_test_failures=$((self_test_failures + 1))
		fi
	done <<-'CASES'
		401 pass
		200 reject
		201 reject
		204 reject
		301 reject
		302 reject
		400 reject
		403 reject
		404 reject
		500 reject
		000 retry
		429 retry
		502 retry
		503 retry
		504 retry
	CASES
	if [ "${self_test_failures}" -ne 0 ]; then
		echo "self-test: ${self_test_failures} case(s) failed" >&2
		exit 1
	fi
	echo "self-test: classify_probe ok"
	exit 0
fi

# Resolved here rather than at the top so `--self-test` above stays purely
# offline — it must not depend on being run from inside a checkout.
SRC="$(cd "$(dirname "${BASH_SOURCE[0]}")/../ops/central-observability" && pwd)"

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

echo "==> verifying both public hostnames reject unauthenticated callers"
if ! verify_public_endpoints_authenticate; then
	cat >&2 <<-EOF

		DEPLOY FAILED ITS AUTH CHECK.

		A hostname we expose publicly did not answer 401. Treat this as live:
		Cloudflare publishes every hostname to the Certificate Transparency logs
		the moment it issues the cert, so scanners find it whether or not it is
		advertised.

		  - 2xx/4xx-not-401 on ingest: the collector's bearertokenauth/ingest
		    authenticator is not gating the OTLP receiver. Check
		    otel-collector-config.yaml's receivers.otlp.protocols.http.auth and
		    that INGEST_TOKEN is set in the VM's .env.
		  - 2xx on the OO UI: Caddy's basic_auth block is not applying. Check
		    OO_UI_USER / OO_UI_PASSWORD_HASH in the VM's .env.
		  - persistent 000/5xx: the stack never came up. \`docker compose ps\`
		    and \`docker compose logs\` on the VM.

		The new config IS already applied — this check runs after \`up -d\`. Roll
		back or fix forward, but do not walk away from it.
	EOF
	exit 1
fi

echo "==> done."
