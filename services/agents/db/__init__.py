"""DB layer for the meridian-agents service.

Reads sessions/active_session/etc. from meridian.db and writes the agent-side
tables (agent_runs, agent_cursor, session_summaries, dispatch_queue,
context_graph_nodes, activity_context). Schema is owned by the Rust ETL
(see src/migrations/005_agents.sql) — Python only does SELECT/INSERT/UPDATE.
"""
from .connections import connection, _json_or_none, _utc_now
from .agent_runs import (
    start_agent_run,
    complete_agent_run,
    get_cursor,
    advance_cursor,
    advance_cursor_to_id,
)
from .sessions import (
    fetch_session,
    fetch_recent_sessions,
    fetch_unprocessed_sessions,
    fetch_active_session,
    fetch_ticket_link,
    clear_ticket_link,
    write_ticket_link,
    fetch_pm_tasks,
    upsert_pm_task,
    upsert_session_dimension,
    fetch_session_dimensions,
    clear_session_dimensions,
    session_id_max,
)
from .context import (
    fetch_context_graph_nodes,
    upsert_context_node,
    fetch_activity_context,
    write_activity_context,
    mark_activity_synced,
)
from .dispatch import (
    write_session_summary,
    enqueue_dispatch,
    fetch_pending_dispatches,
    mark_dispatch_sent,
    mark_dispatch_failed,
    mark_dispatch_skipped,
)
from .jira_updates import (
    log_jira_update,
    get_last_update,
    mark_update_sent,
    mark_update_failed,
)

__all__ = [
    "connection",
    "start_agent_run",
    "complete_agent_run",
    "get_cursor",
    "advance_cursor",
    "advance_cursor_to_id",
    "fetch_session",
    "fetch_recent_sessions",
    "fetch_unprocessed_sessions",
    "fetch_active_session",
    "fetch_ticket_link",
    "clear_ticket_link",
    "write_ticket_link",
    "fetch_pm_tasks",
    "upsert_pm_task",
    "upsert_session_dimension",
    "fetch_session_dimensions",
    "clear_session_dimensions",
    "session_id_max",
    "fetch_context_graph_nodes",
    "upsert_context_node",
    "fetch_activity_context",
    "write_activity_context",
    "mark_activity_synced",
    "write_session_summary",
    "enqueue_dispatch",
    "fetch_pending_dispatches",
    "mark_dispatch_sent",
    "mark_dispatch_failed",
    "mark_dispatch_skipped",
    "log_jira_update",
    "get_last_update",
    "mark_update_sent",
    "mark_update_failed",
]
