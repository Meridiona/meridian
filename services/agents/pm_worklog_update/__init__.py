"""[LEGACY] pm_worklog_update — synth-helper layer for the Rust pm-worklog stage.

Superseded by services/agents/worklog_pipeline/ (the new agno-native hour-level
pipeline). This package is retained only because the MLX server's
/synthesise_worklog endpoint still calls workflow._render_workflow_input and
workflow._coerce_jira. All new worklog logic lives in worklog_pipeline/.

The full PM-worklog pipeline (collect → synth → ground → post) now lives in
the Rust daemon (`src/pm_worklog/`). This package is reduced to the synth
helper the MLX server's `/synthesise_worklog` endpoint calls to turn a
collected `SessionBundle` into a Jira-ready `JiraUpdate`.

See `services/agents/pm_worklog_update/workflow.py` for the two live entry
points (`_render_workflow_input`, `_coerce_jira`).
"""
