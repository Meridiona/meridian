"""pm_update — agno-powered PM update synthesis for Meridian.

Generates Jira-ready comments + status proposals + worklog entries from
classified `app_sessions` rows. Designed as a standalone module: it can be
run from the CLI (`python -m agents.pm_worklog_update.cli`) before being stitched
into `jira_updater_daemon` for the 1-hour cadence (see PM_WORKLOG_INTERVAL_HOURS).

Architecture: see `services/agents/pm_worklog_update/workflow.py`.
"""
