"""pm_worklog_update — synth-helper layer for the Rust pm-worklog stage.

The full PM-worklog pipeline (collect → synth → ground → post) now lives in
the Rust daemon (`src/pm_worklog/`). This package is reduced to the synth
helper the MLX server's `/synthesise_worklog` endpoint calls to turn a
collected `SessionBundle` into a Jira-ready `JiraUpdate`.

See `services/agents/pm_worklog_update/workflow.py` for the two live entry
points (`_render_workflow_input`, `_coerce_jira`).
"""
