# `src/coding_agent_session_ingest/summariser`

Turns each **sealed** coding-agent segment (`task_method = 'pending_summariser'`)
into a factual prose summary for the PM work-log, then flips it to the
classifier's queue (`task_method → 'pending_classifier'`).

Why summarise before classifying? A raw transcript is huge and noisy; the
classifier reasons over the **summary** instead, which is cheaper and sharper.
The summary is also what the Jira updater quotes as evidence.

---

## Engine routing

One transcript in flight at a time (sequential → flat memory, no rate-limit
bursts). Per segment:

```
Codex session  ──▶  codex exec   ─┐
                                  ├─ try primary up to `primary_attempts` (2) times
else           ──▶  claude -p    ─┘        │
                                            ├─ rate-limited?  ──▶  short-circuit to MLX
                                            └─ all attempts failed?  ──▶  MLX
                                                                          │
                                                            still down ──▶ leave row pending, retry next sweep
```

- **Claude sessions → `claude -p`** (`claude.rs`) — loads the `session-summary`
  skill, structured output. Runs on the user's Claude subscription;
  `ANTHROPIC_API_KEY` is dropped from the child env so a stray key can't switch
  to metered billing. `MERIDIAN_SUMMARISER=1` makes the indexer hook ignore the
  throwaway session, and `--no-session-persistence` means no JSONL is written.
- **Codex sessions → `codex exec`** (`codex.rs`) — symmetry with claude.rs.
  Side-effect-free: `-s read-only`, `--skip-git-repo-check`, `--ephemeral`,
  `--output-schema` for the structured final message.
- **Fallback → MLX `/summarise`** (`mlx.rs`) — the local model server endpoint.
  The local model is a reasoner, so it gets only the **tail** of the transcript
  (most recent activity / outcome) plus a cheap reasoning-leak filter on top of
  the endpoint's outlines FSM.

All three target the same `SUMMARY_SCHEMA` and share `SUMMARY_RULES`
(`prompts.rs`): Claude via the skill's `SKILL.md`, Codex via the prompt, MLX via
its system message — kept in one place so they can't drift.

---

## Files

| File | Role |
|---|---|
| `mod.rs` | `run_loop`, `drain`, `summarise_one`, `run_capture` (subprocess w/ `kill_on_drop` + concurrent stdin), `build_prompt` / `cap_transcript`, `cli_summarise`, `SummariserError` |
| `config.rs` | `SummariserConfig::from_env()` — all tunables |
| `db.rs` | `fetch_pending`, `fetch_transcript`, `fetch_prior_summary`, `write_summary` (idempotent) |
| `claude.rs` | `claude -p` engine |
| `codex.rs` | `codex exec` engine |
| `mlx.rs` | MLX `/summarise` fallback |
| `prompts.rs` | shared rules, schema, rate-limit detection |

---

## Cadence

`run_loop` drains, then waits on **whichever fires first**: the indexer's
in-process `Notify` (it pings on its own seals → near-instant) or a short
catch-up sweep (`SUMMARISER_SWEEP_S`, 30 s) that covers hook-sealed rows the
daemon itself didn't seal. If the primary engine is rate-limited **and** MLX is
also down, `drain` returns a back-off signal and the loop sleeps
`SUMMARISER_BACKOFF_S` (30 min) instead.

> **The daemon drain is scoped to *today*.** `drain()` passes
> `Local::now()` as the day filter, so historical sealed rows are *not* picked
> up automatically. Use `meridian coding-agent-summarise --day <YYYY-MM-DD>` to
> backfill an older day.

### Eligibility (`fetch_pending`)

A sealed row is summarised only if it has `claude_session_uuid`, a non-empty
`session_summary IS NULL` slot, `frame_count >= min_turns` (2), and
`length(session_text) >= min_text_bytes` (800). Sub-second / trivially-thin
sessions fall below the floor and are silently skipped — that's intended, not a
stall. `session_text` is read from the **DB** (not the on-disk JSONL), so old
rows stay summarisable even after their JSONL is gone.

---

## The log to watch

On every successful write:

```
INFO summarised coding-agent segment {row_id, uuid, source, written, chars}
```

`source` = `claude` / `codex` / `mlx`; `written: true` confirms it persisted.
Then the batch roll-up `INFO summariser drain {summarised: N}`. Transient
failures are logged (`summarise failed — leaving pending for retry`), never
silent. Tail it:

```bash
tail -f ~/.meridian/logs/meridian-rust.jsonl.$(date +%F) \
  | grep --line-buffered "summarised coding-agent segment"
```

---

## Config (env)

Cadence is adapted to the in-daemon model (notify + short sweep, not a 5-min
standalone poll).

| Env | Default | Purpose |
|---|---|---|
| `SUMMARISER_SWEEP_S` | `30` | catch-up sweep cadence |
| `SUMMARISER_BATCH_PER_TICK` | `8` | rows per drain pass |
| `SUMMARISER_MODEL` | `claude-haiku-4-5-20251001` | Claude model |
| `SUMMARISER_SKILL` | `session-summary` | skill name |
| `SUMMARISER_CLAUDE_TIMEOUT_S` | `240` | `claude -p` timeout |
| `SUMMARISER_CODEX_MODEL` | (empty → codex default) | Codex model |
| `SUMMARISER_CODEX_TIMEOUT_S` | `240` | `codex exec` timeout |
| `SUMMARISER_PRIMARY_ATTEMPTS` | `2` | primary tries before MLX |
| `SUMMARISER_TRANSCRIPT_CAP` | `500000` | primary-engine transcript char cap |
| `SUMMARISER_MIN_TURNS` | `2` | min `frame_count` to summarise |
| `SUMMARISER_MIN_TEXT_BYTES` | `800` | min `session_text` length |
| `SUMMARISER_BACKOFF_S` | `1800` | sleep when rate-limited + MLX down |
| `MLX_SERVER_HOST` / `MLX_SERVER_PORT` | `127.0.0.1` / `7823` | MLX `/summarise` |
| `SUMMARISER_MLX_TIMEOUT_S` | `180` | MLX request timeout |
| `SUMMARISER_MLX_MAX_TOKENS` | `2048` | MLX output cap |
| `SUMMARISER_MLX_INPUT_TOKENS` | `5000` | MLX input tail cap (× chars/token) |
| `SUMMARISER_MLX_CHARS_PER_TOKEN` | `4` | tail-cap char estimate |
| `MERIDIAN_HOME` | `~/.meridian` | neutral cwd for subprocesses |

---

## Idempotency

`write_summary` is `UPDATE … WHERE session_summary IS NULL` — a retry or a
concurrent run can never double-write, and a row another worker already
summarised is left untouched (the write reports `written: false`).
