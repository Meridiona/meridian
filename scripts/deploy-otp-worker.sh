#!/usr/bin/env bash
# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
#
# Deploy infra/otp-worker/ (the account-email-capture Cloudflare Worker) and
# assert it actually behaves the way CLAUDE.md's Hard Rules require of every
# publicly reachable service we run: authenticate every request, rate-limit,
# and answer nothing outside its allowlisted routes.
#
# This mirrors scripts/deploy-gateway.sh's shape and its reasoning almost
# exactly — read that file's header first if this is your first time here.
# The short version: infra/hf-proxy shipped unauthenticated with no rate
# limit and took the whole Cloudflare account down at 173,088 requests/day
# against a 100k/day cap. This script is what stands between "the Worker
# deployed" and "the Worker deployed correctly" — a deploy that goes green
# without these checks is exactly how that happened.
#
# Three probes, all against the SAME deployed Worker:
#   1. POST /otp/capture, no Authorization header       -> 401
#   2. POST /otp/capture, wrong bearer token             -> 401
#   3. POST /otp/capture, real auth, hammered past cap   -> 429
# Probe 3 needs real auth to even reach the gated logic being tested, so it
# requires OTP_CLIENT_TOKEN in the environment (the same value as the
# `wrangler secret put OTP_CLIENT_TOKEN` on the target Worker) — probes 1-2
# run without it.
#
# This Worker used to also have /otp/send and /otp/verify (a code-based
# verification flow via SES) plus a staging-only happy-path send->verify
# check gated on CI_TEST_TOKEN. Both routes and that check were removed once
# /otp/capture became the only caller either one ever had — see
# infra/otp-worker/README.md and index.ts's module header for why.
#
# Usage:
#   bash scripts/deploy-otp-worker.sh --env staging
#   bash scripts/deploy-otp-worker.sh --env production
#   bash scripts/deploy-otp-worker.sh --verify-only <url>
#   bash scripts/deploy-otp-worker.sh --self-test
#
# Env (probes 2/3 only):
#   OTP_CLIENT_TOKEN   required for probe 3; without it, it's skipped with a loud warning
#   RL_EMAIL_PER_DAY   per-email capture cap the target Worker is configured with (default 3,
#                      matching wrangler.jsonc's default for both environments) — the rate-limit
#                      probe sends this many requests + 1 to guarantee tripping the cap
#   PROBE_TIMEOUT_S / PROBE_INTERVAL_S   as in deploy-gateway.sh

set -euo pipefail

RL_EMAIL_PER_DAY="${RL_EMAIL_PER_DAY:-3}"
PROBE_TIMEOUT_S="${PROBE_TIMEOUT_S:-60}"
PROBE_INTERVAL_S="${PROBE_INTERVAL_S:-3}"

# ── Status classifier ────────────────────────────────────────────────────────
#
# Pure — no network, no globals — so `--self-test` can exercise every branch
# offline. Takes the code we actually got and the code THIS probe expects
# (unlike deploy-gateway.sh, which only ever expects 401, this script expects
# two different codes across its probes, so the expected code is a parameter
# rather than a constant).
#
# Only "the stack isn't up yet" codes retry: 000 (curl's "no response at
# all"), and 502/503/504 (upstream not ready). ANYTHING else — including any
# 2xx, and including a wrong-but-plausible 4xx — rejects immediately. A 200
# on the no-auth probe is exactly the catastrophe this script exists to
# catch, and retrying it only delays the failure.
classify_probe() {
	local actual="$1" expected="$2"
	if [ "${actual}" = "${expected}" ]; then
		echo pass
		return
	fi
	case "${actual}" in
	000 | 502 | 503 | 504) echo retry ;;
	*) echo reject ;;
	esac
}

# POST to `${url}${path}` and print the HTTP status code. `--noproxy '*'` for
# the same reason as deploy-gateway.sh: the point is that THE WORKER rejects
# the caller, not an intercepting proxy.
http_post_code() {
	local url="$1"
	shift
	curl -s --noproxy '*' -o /dev/null -w '%{http_code}' --max-time 15 -X POST "$@" "${url}" || true
}

# Retry one probe until it matches `expected` or the timeout budget runs out.
probe_expect() {
	local label="$1" url="$2" expected="$3"
	shift 3
	local deadline=$((SECONDS + PROBE_TIMEOUT_S)) code verdict
	while :; do
		code="$(http_post_code "${url}" "$@")"
		verdict="$(classify_probe "${code}" "${expected}")"
		case "${verdict}" in
		pass)
			echo "  ok   ${label} -> ${code}"
			return 0
			;;
		reject)
			echo "  FAIL ${label} -> ${code} (expected ${expected})" >&2
			return 1
			;;
		retry)
			if [ "${SECONDS}" -ge "${deadline}" ]; then
				echo "  FAIL ${label} -> ${code} after ${PROBE_TIMEOUT_S}s (expected ${expected})" >&2
				return 1
			fi
			sleep "${PROBE_INTERVAL_S}"
			;;
		esac
	done
}

# Probe 3: hammer /otp/capture for one fixed probe email until the per-email
# cap trips. Only the FINAL attempt (cap + 1) is required to be 429.
probe_rate_limit_trip() {
	local url="$1" token="$2"
	local probe_email="deploy-otp-worker-ratelimit-probe-$$-$(date +%s)@example.com"
	local attempts=$((RL_EMAIL_PER_DAY + 1))
	local code=""
	for _ in $(seq 1 "${attempts}"); do
		code="$(http_post_code "${url}/otp/capture" \
			-H 'Content-Type: application/json' \
			-H "Authorization: Bearer ${token}" \
			--data "{\"email\":\"${probe_email}\"}")"
	done
	local verdict
	verdict="$(classify_probe "${code}" 429)"
	if [ "${verdict}" = "pass" ]; then
		echo "  ok   capture rate limit trips after ${RL_EMAIL_PER_DAY} captures -> 429"
		return 0
	fi
	echo "  FAIL capture rate limit -> ${code} after ${attempts} attempts (expected 429)" >&2
	return 1
}

# The required probes. Probe 3 needs OTP_CLIENT_TOKEN to reach the gated
# logic under test at all — without it, it can't distinguish "correctly
# returns 429" from "incorrectly returns 401 for an unrelated reason," so
# it's skipped with a loud warning rather than silently passing or failing on
# the wrong thing.
run_required_probes() {
	local url="$1"
	local failed=0

	probe_expect "capture no credentials     " "${url}/otp/capture" 401 \
		-H 'Content-Type: application/json' --data '{"email":"deploy-otp-worker-probe@example.com"}' || failed=1

	probe_expect "capture wrong bearer token " "${url}/otp/capture" 401 \
		-H 'Content-Type: application/json' \
		-H 'Authorization: Bearer deploy-otp-worker-probe-not-a-real-token' \
		--data '{"email":"deploy-otp-worker-probe@example.com"}' || failed=1

	if [ -z "${OTP_CLIENT_TOKEN:-}" ]; then
		cat >&2 <<-EOF
			  SKIP capture rate-limit (429) probe:
			       OTP_CLIENT_TOKEN is not set. This probe needs real auth to
			       reach the logic under test — set OTP_CLIENT_TOKEN to the same
			       value as the target Worker's \`OTP_CLIENT_TOKEN\` secret to run it.
		EOF
	else
		probe_rate_limit_trip "${url}" "${OTP_CLIENT_TOKEN}" || failed=1
	fi

	return "${failed}"
}

verify_deployed_worker() {
	local url="$1"
	echo "==> verifying ${url}"
	run_required_probes "${url}"
}

# ── Argument dispatch ────────────────────────────────────────────────────────
#
# Same discipline as deploy-gateway.sh: every argument is either recognised
# or an error, so a typo or an unfamiliar flag never falls through to a real
# deploy.
usage() {
	cat <<-USAGE
		usage: deploy-otp-worker.sh --env <staging|production>
		       deploy-otp-worker.sh --verify-only <url>
		       deploy-otp-worker.sh --self-test

		  --env <name>     deploy infra/otp-worker/ to that wrangler environment,
		                    then run the required probes against the deployed URL
		  --verify-only    run the probes against an already-deployed URL,
		                    deploying nothing
		  --self-test      offline check of the status classifier

		env: OTP_CLIENT_TOKEN RL_EMAIL_PER_DAY
		     PROBE_TIMEOUT_S PROBE_INTERVAL_S
	USAGE
}

case "${1:-}" in
--self-test)
	if [ "$#" -ne 1 ]; then
		echo "deploy-otp-worker.sh: --self-test takes no further arguments" >&2
		exit 2
	fi
	self_test_failures=0
	while read -r actual expected want; do
		[ -z "${actual}" ] && continue
		got="$(classify_probe "${actual}" "${expected}")"
		if [ "${got}" != "${want}" ]; then
			echo "FAIL classify_probe(${actual}, ${expected}): want ${want}, got ${got}" >&2
			self_test_failures=$((self_test_failures + 1))
		fi
	done <<-'CASES'
		401 401 pass
		429 429 pass
		200 401 reject
		200 429 reject
		201 401 reject
		400 429 reject
		403 401 reject
		404 401 reject
		401 429 reject
		429 401 reject
		000 401 retry
		502 401 retry
		503 429 retry
		504 401 retry
	CASES
	if [ "${self_test_failures}" -ne 0 ]; then
		echo "self-test: ${self_test_failures} case(s) failed" >&2
		exit 1
	fi
	echo "self-test: classify_probe ok"
	exit 0
	;;
--verify-only)
	if [ "$#" -ne 2 ] || [ -z "${2}" ]; then
		echo "deploy-otp-worker.sh: --verify-only requires exactly one URL argument" >&2
		usage >&2
		exit 2
	fi
	target_url="${2%/}"
	if verify_deployed_worker "${target_url}"; then
		echo "==> ok."
		exit 0
	fi
	echo "==> FAILED. See above." >&2
	exit 1
	;;
--env)
	if [ "$#" -ne 2 ] || [ -z "${2}" ]; then
		echo "deploy-otp-worker.sh: --env requires an environment name" >&2
		usage >&2
		exit 2
	fi
	case "${2}" in
	staging | production) ;;
	*)
		echo "deploy-otp-worker.sh: unknown environment '${2}' (expected 'staging' or 'production')" >&2
		exit 2
		;;
	esac
	;;
-h | --help)
	usage
	exit 0
	;;
"")
	echo "deploy-otp-worker.sh: no arguments given — a deploy requires --env <staging|production>" >&2
	usage >&2
	exit 2
	;;
*)
	echo "deploy-otp-worker.sh: unknown argument '${1}'" >&2
	usage >&2
	exit 2
	;;
esac

# Only reached for `--env <name>`, which is validated above.
TARGET_ENV="${2}"

# Resolved here, not at the top, so --self-test and --verify-only stay purely
# offline / independent of running from inside a checkout.
WORKER_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../infra/otp-worker" && pwd)"

echo "==> deploying ${WORKER_DIR} to wrangler environment '${TARGET_ENV}'"
# Captured via `if ! deploy_output=...; then` rather than a bare assignment —
# under `set -e`, a bare `deploy_output="$(cmd)"` takes the command
# substitution's exit status, so a FAILING `wrangler deploy` would abort the
# script right here, before the `echo` below ever ran, leaving the operator
# with no output explaining why.
deploy_failed=0
if [ "${TARGET_ENV}" = "production" ]; then
	deploy_output="$(cd "${WORKER_DIR}" && npx wrangler deploy 2>&1)" || deploy_failed=1
else
	deploy_output="$(cd "${WORKER_DIR}" && npx wrangler deploy --env "${TARGET_ENV}" 2>&1)" || deploy_failed=1
fi
echo "${deploy_output}"
if [ "${deploy_failed}" -ne 0 ]; then
	echo "==> wrangler deploy FAILED. See output above." >&2
	exit 1
fi

deployed_url="$(echo "${deploy_output}" | grep -oE 'https://[a-zA-Z0-9.-]+\.workers\.dev' | tail -1 || true)"
if [ -z "${deployed_url}" ]; then
	cat >&2 <<-EOF

		Could not parse the deployed Worker URL out of \`wrangler deploy\`'s output.
		If this Worker is bound to a custom domain instead of the default
		*.workers.dev route, re-run with:
		  bash scripts/deploy-otp-worker.sh --verify-only <your-custom-domain-url>
	EOF
	exit 1
fi

if verify_deployed_worker "${deployed_url}"; then
	echo "==> done."
	exit 0
fi

cat >&2 <<-EOF

	DEPLOY FAILED ITS VERIFICATION CHECKS.

	The new code IS already live — this check runs after \`wrangler deploy\`.
	Treat this as live: Cloudflare publishes every hostname to the Certificate
	Transparency logs the moment it issues the cert, so scanners find it
	whether or not it is advertised. Roll back (\`npx wrangler rollback\` from
	infra/otp-worker/) or fix forward, but do not walk away from it.
EOF
exit 1
