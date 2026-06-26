"""Minimal AgentOS viewer for browsing worklog pipeline traces.

Run this script to expose the agno_traces.db as an AgentOS runtime, then:
  1. Open https://os.agno.com, sign in (free account)
  2. Add new OS → Environment: Local → URL: http://localhost:8000
  3. Click "CONNECT" → refresh the page

Alternatively, browse directly at http://localhost:8000 without the hosted UI.

The DB path defaults to ~/.meridian/agno_traces.db (set AGNO_TRACE_DB to override).
"""
from __future__ import annotations

import logging
import os
from pathlib import Path

log = logging.getLogger(__name__)

from agno.agent import Agent
from agno.db.sqlite import SqliteDb
from agno.os import AgentOS
from agno.workflow import Workflow

_DEFAULT_DB = Path("~/.meridian/agno_traces.db").expanduser()
_db_file = Path(os.environ.get("AGNO_TRACE_DB", "") or _DEFAULT_DB).expanduser()

db = SqliteDb(db_file=str(_db_file))

_agent = Agent(
    name="Meridian Worklog Pipeline",
    description="Worklog pipeline agent — match sessions to PM tasks and draft worklogs.",
    db=db,
)

# Stub workflow registered so os.agno.com marks the OS as active.
# The real pipeline runs inside server.py; this is traces-only.
_workflow = Workflow(
    name="worklog_hour",
    description="Hour-level worklog pipeline: distil → report → rerank → match → draft/propose.",
    db=db,
)

agent_os = AgentOS(
    id="meridian-worklog",
    name="Meridian Worklog Traces",
    description="Viewer for worklog pipeline traces — worklog.hour, match, propose, draft spans.",
    agents=[_agent],
    workflows=[_workflow],
    db=db,
    tracing=True,
)

app = agent_os.get_app()

if __name__ == "__main__":
    log.info("trace DB: %s", _db_file)
    log.info("dashboard: http://localhost:8000")
    log.info("control:   https://os.agno.com → Add new OS → http://localhost:8000")
    agent_os.serve(app="agno_viewer:app", reload=False, port=8000)
