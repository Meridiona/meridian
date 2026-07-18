# Meridian Scripts

Operational and release tooling for Meridian. Each script documents its own
usage in a header comment - run `head -30 scripts/<name>` (or pass `--help`
where supported) to see it.

The worklog pipeline (distil -> activity report -> workstream fold -> draft) now
runs entirely inside the Rust daemon (`src/worklog_pipeline/`), so there are no
standalone distill / activity-report helper scripts - inspect a run with
`meridian logs` instead.

## Environment Variables

| Variable | Default | Description |
|---|---|---|
| `MERIDIAN_DB` | `~/.meridian/meridian.db` | Path to the Meridian SQLite database. |
