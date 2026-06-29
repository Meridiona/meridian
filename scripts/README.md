# Meridian Scripts

## Activity Report Scripts

### `run-distill.sh` — distill sessions for an hour

Fetches and compresses app sessions for a given hour from the DB. Useful to inspect what the LLM will see before running the activity report.

**Parameters**

| Parameter | Required | Description |
|---|---|---|
| `HOUR` | No | Local hour label `YYYY-MM-DDTHH`. Defaults to current hour. |
| `--exclude-coding` | No | Omit coding-agent sessions (Claude Code, Codex, etc.) from the body. |

**Examples**

```bash
# Current hour
bash scripts/run-distill.sh

# Specific hour (local time)
bash scripts/run-distill.sh 2026-06-28T13

# Specific hour, skip coding-agent sessions
bash scripts/run-distill.sh 2026-06-28T13 --exclude-coding
```

**Output**

```
→ hour:           2026-06-28T13
→ db:             /Users/you/.meridian/meridian.db
→ exclude-coding: false

sessions:    4
raw chars:   12840
out chars:   4387
reduction:   65.8%
elapsed:     1.2s

════════════════════════════════════════
  Distilled body — 2026-06-28T13
════════════════════════════════════════
HOUR 13:00
...
```

---

### `run-activity-report.sh` — distill + run LLM activity summary

Runs the full pipeline: distill the hour's sessions, then send the distilled body to the MLX model to produce a human-readable activity report.

**Parameters**

| Parameter | Required | Description |
|---|---|---|
| `HOUR` | No | Local hour label `YYYY-MM-DDTHH`. Defaults to current hour. |

**Examples**

```bash
# Current hour
bash scripts/run-activity-report.sh

# Specific hour (local time)
bash scripts/run-activity-report.sh 2026-06-28T13
```

**Output**

```
→ hour: 2026-06-28T13
→ db:   /Users/you/.meridian/meridian.db

→ distilling sessions for 2026-06-28T13 …
   sessions: 4  body: 4387 chars
→ running activity_report (this takes ~1–4 min) …
in_tok=1981 out_tok=8192 think_tok=6200 elapsed=63.2s

════════════════════════════════════════
  Activity Report — 2026-06-28T13
════════════════════════════════════════
### TLDR
...
```

**Notes**
- Requires the MLX server to be running (`dev-start.sh` starts it on port 7823).
- Traces and logs appear automatically in OpenObserve — the server runs with `MERIDIAN_OO_EXPORT=1`.
- The hour label is your **system's local timezone** — no IST hardcoding.

---

## Environment Variables

| Variable | Default | Description |
|---|---|---|
| `MERIDIAN_DB` | `~/.meridian/meridian.db` | Path to the Meridian SQLite database. |
