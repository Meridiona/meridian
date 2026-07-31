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

  **Root is a stopgap, not the sanctioned end state.** The SSH tunnel bypasses
  Caddy entirely, so "read-only" here is enforced only by the convention of
  this doc — root can do anything the API allows, not just `_search`. Prefer
  authenticating as a dedicated **Viewer**-role OpenObserve user (Settings ->
  Users in the UI, or the `/api/{org}/users` endpoint) once one exists for
  this org; ask whoever administers the instance to provision it. Provisioning
  and rotating that account is tracked as follow-up work, not done as part of
  this doc.

**Run every command below in a real terminal on your own machine — not Google
Cloud Shell.** Cloud Shell is a separate, ephemeral environment; the SSH
tunnel's local port-forward has to bind on *your* machine, and running it in
Cloud Shell fails with `bind [::1]:5080: Cannot assign requested address`
because there's nothing local there to serve it to.

## Steps

**1. Find OpenObserve's internal container IP** (SSH into the VM first):

```bash
gcloud compute ssh --zone "asia-south1-a" "meridian-telemetry" --project "meridiona-observability"

# once on the VM, in the directory with docker-compose.yml:
cd central-observability
container_id="$(docker compose ps -q openobserve)"
test -n "$container_id" || { echo "openobserve service not running"; exit 1; }
address="$(docker inspect -f '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' "$container_id")"
test -n "$address" || { echo "container has no network address"; exit 1; }
echo "$address"
# e.g. 172.18.0.2
exit
```

Resolving through the Compose service name (rather than a hardcoded container
name) means this keeps working across a redeploy. Still re-run it after any
redeploy though — the bridge IP is only stable for the container currently
running; don't cache it.

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
curl -fsS -o /dev/null -w 'HTTP %{http_code}\n' http://127.0.0.1:5080/healthz
```

`-f` makes curl exit non-zero on a 4xx/5xx instead of silently reporting
success on a transfer that completed but returned an error status.

**4. Query `_search` directly:**

```bash
export OO_ROOT_USER_EMAIL='<from the VM .env>'
# Give curl the user only, not "user:password" — it then prompts for the
# password interactively, which keeps it out of shell history and `ps` output
# (a literal '<user>:<password>' on the command line ends up in both).

end_time=$(( $(date +%s) * 1000000 ))
start_time=$(( end_time - 30 * 86400 * 1000000 ))   # last 30 days; adjust as needed

curl --user "$OO_ROOT_USER_EMAIL" \
  -X POST 'http://127.0.0.1:5080/api/default/_search' \
  -H 'Content-Type: application/json' \
  -d '{
    "query": {
      "sql": "SELECT * FROM \"default\" WHERE host_name = '\''mac_970e5ebf236cb0ce'\'' ORDER BY _timestamp DESC LIMIT 50",
      "start_time": '"$start_time"',
      "end_time": '"$end_time"'
    }
  }'
```

Notes:
- Org is `default` (matches `OO_ORG` in `.env.example`) unless the deploy was
  changed.
- `start_time`/`end_time` are **microsecond epoch timestamps** — `0` is
  rejected with `invalid time range`; the snippet above computes a real
  window at run time so it never goes stale. `start_time`/`end_time` in the
  request body only bound the search window — they're independent of any
  `_timestamp` condition inside `sql`.
- A Support ID is the pseudonymized `host_name` **resource attribute**, not
  text inside the log message — filter on `host_name = '<support id>'`
  (exact match), not a `LIKE` search over `body`. The log message column
  itself is called `body`, not `message`.
- If you do need a substring search over `body`, remember `_` and `%` are
  SQL `LIKE` wildcards — a literal underscore (as in `mac_970e...`) matches
  any single character unless escaped, e.g. `LIKE '%foo\_bar%' ESCAPE '\'`.
  `host_name` needs no such escaping since it's an exact-match column, not a
  pattern.
- `SELECT * FROM "default"` (or `/api/default/streams`) shows the full
  schema when unsure what's queryable — the stream/table name and available
  columns otherwise depend on what the OTel Collector is configured to write
  (see `otel-collector-config.yaml`).

## When you're done

Kill the tunnel (`Ctrl-C` in the terminal running the `gcloud compute ssh -N -L …`
command). Nothing on the VM needs cleanup — the tunnel is purely local
port-forwarding, it doesn't change anything server-side.
