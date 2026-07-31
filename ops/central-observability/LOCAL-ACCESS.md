# Querying the central OpenObserve instance locally (for Claude / ad-hoc debugging)

How to pull data out of the central error-observability stack (`docker-compose.yml`
in this directory) from your own machine, for one-off debugging — e.g. asking
Claude Code to investigate a specific Support ID's errors. This is a
read-only, ad-hoc path, not a persistent integration.

## Why not just use the OpenObserve MCP server or the public HTTPS UI?

- **MCP server**: OpenObserve's `/api/<org>/mcp` endpoint is an **Enterprise**
  feature. Confirmed directly against this instance:
  `{"error":"MCP server is only available in enterprise edition"}`. We run the
  OSS image (`OO_IMAGE` in `.env.example`), so this path is closed — there is
  no code fix, only an upgrade path we haven't taken.
- **Public HTTPS UI/API** (`OO_UI_DOMAIN`, e.g. `observe.meridiona.com`): the
  Caddyfile puts **two independent auth layers** in front of OpenObserve on
  purpose (see the Caddyfile's comment) — Caddy basic auth, then OpenObserve's
  own root login. A single `Authorization` header can't satisfy both at once,
  so scripting `_search` calls straight against the public domain doesn't
  work cleanly. That's deliberate defense-in-depth for a world-reachable
  subdomain fronting customer error telemetry — **don't loosen it** to make
  scripting easier.

The sanctioned way around both of these for ad-hoc debugging is an SSH tunnel
straight to OpenObserve's container port, entirely bypassing Caddy.

## Prerequisites

- `gcloud` CLI installed and authenticated against the `meridiona-observability`
  GCP project (`brew install --cask google-cloud-sdk`, then `gcloud init` /
  `gcloud auth login` if you haven't already).
- SSH access to the `meridian-telemetry` VM (zone `asia-south1-a`) — the same
  account that can `gcloud compute ssh` into it.
- The OpenObserve root credentials (`OO_ROOT_USER_EMAIL` / `OO_ROOT_USER_PASSWORD`
  from that VM's `.env`, not this repo's `.env.example`) to authenticate `_search`
  calls. Ask whoever deployed the stack, or read them off the VM directly —
  never copy them into a file in this repo.

**Run every command below in a real terminal on your own machine — not Google
Cloud Shell.** Cloud Shell is a separate, ephemeral environment; the SSH
tunnel's local port-forward has to bind on *your* machine, and running it in
Cloud Shell fails with `bind [::1]:5080: Cannot assign requested address`
because there's nothing local there to serve it to.

## Steps

**1. Find OpenObserve's internal container IP** (SSH into the VM first):

```bash
gcloud compute ssh --zone "asia-south1-a" "meridian-telemetry" --project "meridiona-observability"

# once on the VM:
cd central-observability   # wherever docker-compose.yml lives on this VM
docker inspect -f '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' \
  central-observability-openobserve-1
# e.g. 172.18.0.2
exit
```

This only needs to be redone if the stack is redeployed and the container gets
recreated (compose's private network typically hands out the same IP, but
don't assume it).

**2. Open the tunnel** (from your own terminal, not Cloud Shell):

```bash
gcloud compute ssh --zone "asia-south1-a" "meridian-telemetry" \
  --project "meridiona-observability" -- -N -L 5080:172.18.0.2:5080
```

Replace `172.18.0.2` with whatever step 1 returned. This blocks (that's `-N`)
— leave it running in its own terminal/tab for the duration of your debugging
session.

**3. Verify:**

```bash
curl http://127.0.0.1:5080/healthz   # expect 200 OK
```

**4. Query `_search` directly:**

```bash
curl -s -u '<OO_ROOT_USER_EMAIL>:<OO_ROOT_USER_PASSWORD>' \
  -X POST 'http://127.0.0.1:5080/api/default/_search' \
  -H 'Content-Type: application/json' \
  -d '{
    "query": {
      "sql": "SELECT * FROM \"default\" WHERE message LIKE '\''%mac_970e5ebf236cb0ce%'\'' ORDER BY _timestamp DESC LIMIT 50",
      "start_time": 1735689600000000,
      "end_time": 1753900800000000
    }
  }'
```

Notes:
- Org is `default` (matches `OO_ORG` in `.env.example`) unless the deploy was
  changed.
- `start_time`/`end_time` are **microsecond epoch timestamps** — `0` is
  rejected with `invalid time range`. Compute a real window, e.g. in Python:
  `int(time.time() * 1_000_000)` for "now", and subtract
  `N * 86_400 * 1_000_000` for N days back.
- The stream/table name and available columns depend on what the OTel
  Collector is configured to write (see `otel-collector-config.yaml`) — when
  unsure what's queryable, `SHOW STREAMS` or hit `/api/default/streams` first.

## When you're done

Kill the tunnel (`Ctrl-C` in the terminal running the `gcloud compute ssh -N -L …`
command). Nothing on the VM needs cleanup — the tunnel is purely local
port-forwarding, it doesn't change anything server-side.
