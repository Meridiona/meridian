# Database Schema

Meridian writes to `~/.meridian/meridian.db`. It never modifies screenpipe's database.

## Tables

### `app_sessions`

Completed sessions. Append-only — rows are never updated after insert.

| Column | Type | Description |
|---|---|---|
| `id` | INTEGER | Primary key |
| `app_name` | TEXT | App that owned the session |
| `started_at` | TEXT | ISO8601 UTC start timestamp |
| `ended_at` | TEXT | ISO8601 UTC end timestamp |
| `duration_s` | REAL | Wall-clock seconds (excludes gap time) |
| `frame_count` | INTEGER | Number of screenpipe frames in the block |
| `category` | TEXT | AI-assigned category (e.g. `coding`, `meeting`, `research`) |
| `confidence` | REAL | Category confidence score (0.0–1.0) |
| `window_titles` | TEXT | JSON array of `{title, count}` — top windows seen, sorted by count |
| `ocr_samples` | TEXT | JSON array of up to 20 deduplicated OCR text samples |
| `elements_samples` | TEXT | JSON array of up to 20 deduplicated accessibility tree samples |
| `audio_snippets` | TEXT | JSON array of transcribed audio |
| `signals` | TEXT | JSON array of deduplicated clipboard copies and app-switch events |
| `min_frame_id` | INTEGER | Lowest screenpipe frame_id in the block |
| `max_frame_id` | INTEGER | Highest screenpipe frame_id in the block |

### `active_session`

Single-row table (`id = 1`) — the currently open, in-progress session. Upserted every poll tick.

Same columns as `app_sessions`. Moved to `app_sessions` when the app changes.

### `etl_runs`

Audit log — one row per `run_etl()` call.

| Column | Type | Description |
|---|---|---|
| `id` | INTEGER | Primary key |
| `started_at` | TEXT | When this ETL run began |
| `finished_at` | TEXT | When it completed (NULL if still running) |
| `status` | TEXT | `running`, `success`, or `failed` |
| `error` | TEXT | Error message if `status = 'failed'` |
| `frames_processed` | INTEGER | Frames consumed in this run |

### `etl_cursor`

Single-row table — tracks `last_frame_id` (the highest screenpipe `frame_id` processed).

### `gaps`

Sleep and idle periods detected between sessions.

| Column | Type | Description |
|---|---|---|
| `id` | INTEGER | Primary key |
| `started_at` | TEXT | Gap start |
| `ended_at` | TEXT | Gap end |
| `duration_s` | REAL | Gap duration in seconds |
| `kind` | TEXT | `user_idle` or `system_sleep` |
| `idle_frame_count` | INTEGER | Frames with `capture_trigger = 'idle'` in window |
| `total_frame_count` | INTEGER | All frames in window |

## Agent-side tables

Written by the Python classification service:

### `ticket_links`

| Column | Description |
|---|---|
| `session_id` | Foreign key to `app_sessions.id` |
| `task_key` | Jira ticket key (e.g. `KAN-87`) |
| `confidence` | Classifier confidence (0.0–1.0) |
| `method` | Classification method used |

Uses `ON CONFLICT DO UPDATE` — idempotent upserts.

### `session_dimensions`

Multi-label dimension tags per session (e.g. `deep_work`, `context_switch`, `planning`).

## JSON column schemas

### `window_titles`
```json
[{"title": "main.rs - meridian", "count": 14}, ...]
```
Sorted by `count` descending. Merged and re-sorted on each `active_session` upsert.

### `ocr_samples` / `elements_samples`
```json
["cargo build --release", "fn run_etl(", ...]
```
Capped at 20 entries per session via `OCR_SAMPLE_CAP`. Deduplicated by text content.

### `audio_snippets`
```json
[{"text": "let me check the ETL runner", "timestamp": "2025-05-29T14:32:01Z"}, ...]
```
Uncapped. Deduplicated by text content; earliest timestamp kept.

### `signals`
```json
[{"kind": "clipboard", "text": "cargo test", "timestamp": "..."}]
```

## Migrations

Migrations live in `src/migrations/`. They run automatically on daemon startup. Never modify an existing migration — always add a new numbered file. All migrations are covered by integration tests via `make_meridian_db()`.
