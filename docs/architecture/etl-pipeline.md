# ETL Pipeline

`run_etl()` in `src/etl/runner.rs` is the single entry point called every poll interval.

## Execution steps

1. **Read cursor** — load `last_frame_id` from `etl_cursor`
2. **Insert ETL run row** — `status = 'running'`
3. **Cross-run gap check** — if `active_session` exists from a previous run and the first new frame is >300 s later, classify and record the gap, then close the stale session
4. **Batch processing** — process frames in batches of 500 (`BATCH_SIZE`), maintaining a block state machine keyed on `app_name`
5. **Intra-batch gap check** — before every frame, if the inter-frame gap exceeds `GAP_THRESHOLD_SECS` (300 s), close the current block at its real `ended_at`, record the gap, then start fresh
6. **App-switch close** (`close_block`) — when `app_name` changes, close the old block into `app_sessions`; apply Option C (ui_event refines `ended_at`) and Option D (single-frame sessions use `next_frame_ts`)
7. **Active session upsert** (`upsert_open_block`) — the still-open block at the end of all batches goes into `active_session` (single-row table, `id = 1`)
8. **Advance cursor** — update `last_frame_id`; mark ETL run `success` (or `failed` with error text on error)

## Session boundary detection

A session boundary is detected whenever `app_name` changes between consecutive frames. The old block is closed into `app_sessions` with final timestamps and frame counts; the new block starts.

### Duration correctness (Options C and D)

- **Option C**: if a `ui_event` timestamp is strictly after the last frame timestamp, use it as `ended_at` — more accurate for sessions that end with a UI interaction
- **Option D**: single-frame sessions use `next_frame_ts` as `ended_at` to avoid zero-duration sessions

## Gap detection

A gap is recorded when the time between two consecutive frames exceeds 300 s (strictly greater than).

### Classification

`count_frames_in_window(screenpipe, from, to)` counts all frames inside the gap window, including frames with `NULL app_name`:

- `idle_count * 2 >= total_count` → `user_idle`
- otherwise → `system_sleep`

A gap of exactly 300 s does **not** trigger — the threshold is strictly greater than.

### Cross-run gaps

If the machine sleeps between two ETL runs, the gap spans the `active_session` from the previous run and the first new frame. The cross-run gap check at step 3 handles this: the stale session is closed and the gap recorded before normal batch processing begins.

## Context extraction

`extract_block_context()` in `src/etl/extractor.rs` gathers everything needed to populate a session:

- **OCR text** — up to 20 deduplicated samples from `ocr_text` frames
- **Accessibility elements** — up to 20 deduplicated entries from `elements`
- **Audio snippets** — all unique transcription chunks from `audio_transcriptions`
- **Window titles** — merged `{title, count}` map, sorted descending
- **Signals** — deduplicated clipboard copies and app-switch events from `ui_events`

All deduplication is text-content based — the earliest timestamp is kept when two entries share the same text.

## Invariants

These are enforced by integration tests in `tests/integration_etl.rs`:

| Invariant | Description |
|---|---|
| No phantom sessions | An open block with no app switch stays in `active_session`, never in `app_sessions` |
| Correct frame counts | `frame_count` equals the actual number of frames in the block |
| Gap exclusion | `duration_s` never includes gap time — the pre-gap block closes at the last real frame timestamp |
| No duplicates | Running ETL twice on the same data produces no duplicate rows |
| Cursor monotonicity | `last_frame_id` only advances; never decreases |
| Option C guard | ui_event `ended_at` only applied when it is strictly after the last frame timestamp |
| Gap threshold | A gap of exactly 299 s must not produce a gap row |

## Adding a new extraction signal

1. Add the screenpipe read query in `src/db/screenpipe.rs`
2. Extend `BlockContext` in `src/etl/extractor.rs` and wire it in `extract_block_context()`
3. Update `build_active_session` and `merge_into_active` in `src/etl/runner.rs`
4. Add a migration if the signal needs its own column; otherwise store as JSON in `signals`
5. Add an integration test in `tests/integration_etl.rs`
