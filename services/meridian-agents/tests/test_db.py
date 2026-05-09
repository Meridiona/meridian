# meridian — normalises screenpipe activity into structured app sessions

"""Unit + functional tests for meridian_agents.db.

Every test runs against a fresh SQLite file with the actual
`src/migrations/00*.sql` applied — see conftest.py — so we catch schema
drift between Rust and Python the moment it happens.
"""

from __future__ import annotations

import json

import aiosqlite
import pytest

from meridian_agents import db as db_mod
from meridian_agents.db import (
    SchemaError,
    advance_cursor,
    claim_dispatch_pending,
    count_new_sessions,
    enqueue_dispatch,
    fetch_context_nodes,
    fetch_pm_tasks,
    fetch_unanalysed_sessions,
    finish_run,
    link_ticket,
    mark_activity_context_synced,
    mark_dispatched,
    open_ro,
    open_rw,
    read_activity_context,
    read_cursor,
    schema_check,
    start_run,
    upsert_context_node,
    write_activity_context,
    write_session_summary,
)
from tests.conftest import (
    seed_app_session,
    seed_etl_run,
    seed_pm_task,
)

# ---------------------------------------------------------------------------
# Connection helpers
# ---------------------------------------------------------------------------


async def test_open_rw_refuses_to_create_missing_file(tmp_path):
    missing = tmp_path / "no-such.db"
    with pytest.raises(aiosqlite.Error):
        await open_rw(str(missing))


async def test_open_rw_sets_busy_timeout(rw_conn):
    cur = await rw_conn.execute("PRAGMA busy_timeout")
    row = await cur.fetchone()
    await cur.close()
    assert row[0] == db_mod.BUSY_TIMEOUT_MS


async def test_open_rw_enables_foreign_keys(rw_conn):
    cur = await rw_conn.execute("PRAGMA foreign_keys")
    row = await cur.fetchone()
    await cur.close()
    assert row[0] == 1


async def test_open_ro_blocks_writes(migrated_db_path):
    conn = await open_ro(str(migrated_db_path))
    try:
        with pytest.raises(aiosqlite.OperationalError, match="readonly"):
            await conn.execute("INSERT INTO agent_runs DEFAULT VALUES")
    finally:
        await conn.close()


# ---------------------------------------------------------------------------
# schema_check
# ---------------------------------------------------------------------------


async def test_schema_check_passes_on_migrated_db(rw_conn):
    await schema_check(rw_conn)  # must not raise


async def test_schema_check_detects_missing_dispatch_queue(rw_conn):
    await rw_conn.execute("DROP TABLE dispatch_queue")
    with pytest.raises(SchemaError, match="dispatch_queue"):
        await schema_check(rw_conn)


async def test_schema_check_detects_missing_session_summaries(rw_conn):
    await rw_conn.execute("DROP TABLE session_summaries")
    with pytest.raises(SchemaError, match="session_summaries"):
        await schema_check(rw_conn)


async def test_schema_check_detects_missing_context_graph(rw_conn):
    await rw_conn.execute("DROP TABLE context_graph_nodes")
    with pytest.raises(SchemaError, match="context_graph_nodes"):
        await schema_check(rw_conn)


async def test_schema_check_detects_missing_activity_context(rw_conn):
    await rw_conn.execute("DROP TABLE activity_context")
    with pytest.raises(SchemaError, match="activity_context"):
        await schema_check(rw_conn)


async def test_schema_check_error_message_is_actionable(rw_conn):
    await rw_conn.execute("DROP TABLE agent_runs")
    with pytest.raises(SchemaError, match="Run the Rust daemon"):
        await schema_check(rw_conn)


# ---------------------------------------------------------------------------
# Cursor lifecycle
# ---------------------------------------------------------------------------


async def test_read_cursor_initial_value_is_zero(rw_conn):
    assert await read_cursor(rw_conn) == 0


async def test_advance_cursor_round_trip(rw_conn):
    await advance_cursor(rw_conn, 42)
    assert await read_cursor(rw_conn) == 42


async def test_advance_cursor_overwrites_previous(rw_conn):
    await advance_cursor(rw_conn, 10)
    await advance_cursor(rw_conn, 7)  # rewinds — allowed
    assert await read_cursor(rw_conn) == 7


# ---------------------------------------------------------------------------
# Run lifecycle
# ---------------------------------------------------------------------------


async def test_start_run_returns_id_and_creates_running_row(rw_conn):
    run_id = await start_run(rw_conn)
    cur = await rw_conn.execute(
        "SELECT status, finished_at FROM agent_runs WHERE id = ?", (run_id,)
    )
    row = await cur.fetchone()
    await cur.close()
    assert row["status"] == "running"
    assert row["finished_at"] is None


async def test_finish_run_success_updates_counters(rw_conn):
    run_id = await start_run(rw_conn)
    await finish_run(
        rw_conn,
        run_id,
        status="success",
        sessions_processed=3,
        summaries_written=3,
        links_written=2,
        dispatches_queued=2,
        dispatches_sent=2,
    )
    cur = await rw_conn.execute(
        "SELECT status, sessions_processed, dispatches_sent, finished_at "
        "FROM agent_runs WHERE id = ?",
        (run_id,),
    )
    row = await cur.fetchone()
    await cur.close()
    assert row["status"] == "success"
    assert row["sessions_processed"] == 3
    assert row["dispatches_sent"] == 2
    assert row["finished_at"] is not None


async def test_finish_run_failure_records_error(rw_conn):
    run_id = await start_run(rw_conn)
    await finish_run(rw_conn, run_id, status="failed", error="boom")
    cur = await rw_conn.execute(
        "SELECT status, error FROM agent_runs WHERE id = ?", (run_id,)
    )
    row = await cur.fetchone()
    await cur.close()
    assert row["status"] == "failed"
    assert row["error"] == "boom"


async def test_finish_run_rejects_unknown_status(rw_conn):
    run_id = await start_run(rw_conn)
    with pytest.raises(ValueError, match="invalid status"):
        await finish_run(rw_conn, run_id, status="queued")


# ---------------------------------------------------------------------------
# count_new_sessions + fetch_unanalysed_sessions
# ---------------------------------------------------------------------------


async def test_count_new_sessions_zero_initially(rw_conn):
    assert await count_new_sessions(rw_conn, after_id=0) == 0


async def test_count_new_sessions_counts_only_after_cursor(rw_conn):
    etl = await seed_etl_run(rw_conn)
    s1 = await seed_app_session(rw_conn, etl_run_id=etl)
    await seed_app_session(rw_conn, etl_run_id=etl)
    await seed_app_session(rw_conn, etl_run_id=etl)
    assert await count_new_sessions(rw_conn, after_id=0) == 3
    assert await count_new_sessions(rw_conn, after_id=s1) == 2


async def test_fetch_unanalysed_returns_new_sessions(rw_conn):
    etl = await seed_etl_run(rw_conn)
    s1 = await seed_app_session(rw_conn, etl_run_id=etl, app_name="Cursor")
    s2 = await seed_app_session(rw_conn, etl_run_id=etl, app_name="Slack")
    sessions = await fetch_unanalysed_sessions(rw_conn, after_id=0)
    assert [s.id for s in sessions] == [s1, s2]


async def test_fetch_unanalysed_respects_after_id(rw_conn):
    etl = await seed_etl_run(rw_conn)
    s1 = await seed_app_session(rw_conn, etl_run_id=etl)
    s2 = await seed_app_session(rw_conn, etl_run_id=etl)
    sessions = await fetch_unanalysed_sessions(rw_conn, after_id=s1)
    assert [s.id for s in sessions] == [s2]


async def test_fetch_unanalysed_skips_already_linked(rw_conn):
    etl = await seed_etl_run(rw_conn)
    linked = await seed_app_session(rw_conn, etl_run_id=etl)
    fresh = await seed_app_session(rw_conn, etl_run_id=etl)
    await link_ticket(
        rw_conn,
        session_id=linked,
        task_key="KAN-86",
        provider="jira",
        confidence=0.9,
        routing="auto",
    )
    sessions = await fetch_unanalysed_sessions(rw_conn, after_id=0)
    assert [s.id for s in sessions] == [fresh]


async def test_fetch_unanalysed_parses_json_columns(rw_conn):
    etl = await seed_etl_run(rw_conn)
    await seed_app_session(
        rw_conn,
        etl_run_id=etl,
        window_titles='[["meridian — db.py", 7]]',
        ocr_samples='["async def open_rw"]',
    )
    [session] = await fetch_unanalysed_sessions(rw_conn, after_id=0)
    assert session.window_titles == [["meridian — db.py", 7]]
    assert session.ocr_samples == ["async def open_rw"]
    assert session.audio_snippets == []


async def test_fetch_unanalysed_tolerates_invalid_json(rw_conn):
    """Defensive: if the Rust side ever writes garbage, we shouldn't crash."""
    etl = await seed_etl_run(rw_conn)
    sid = await seed_app_session(rw_conn, etl_run_id=etl)
    await rw_conn.execute(
        "UPDATE app_sessions SET window_titles = 'not-json' WHERE id = ?",
        (sid,),
    )
    [session] = await fetch_unanalysed_sessions(rw_conn, after_id=0)
    assert session.window_titles == []


async def test_fetch_unanalysed_respects_limit(rw_conn):
    etl = await seed_etl_run(rw_conn)
    for _ in range(5):
        await seed_app_session(rw_conn, etl_run_id=etl)
    sessions = await fetch_unanalysed_sessions(rw_conn, after_id=0, limit=2)
    assert len(sessions) == 2


async def test_fetch_unanalysed_passes_through_category_and_confidence(rw_conn):
    etl = await seed_etl_run(rw_conn)
    await seed_app_session(
        rw_conn, etl_run_id=etl, category="coding", confidence=0.92
    )
    [session] = await fetch_unanalysed_sessions(rw_conn, after_id=0)
    assert session.category == "coding"
    assert session.confidence == pytest.approx(0.92)


# ---------------------------------------------------------------------------
# fetch_pm_tasks
# ---------------------------------------------------------------------------


async def test_fetch_pm_tasks_returns_fresh_only(rw_conn):
    await seed_pm_task(rw_conn, task_key="FRESH-1", expires_at="2099-01-01T00:00:00Z")
    await seed_pm_task(rw_conn, task_key="STALE-1", expires_at="2000-01-01T00:00:00Z")
    tasks = await fetch_pm_tasks(rw_conn, only_fresh=True)
    assert [t.task_key for t in tasks] == ["FRESH-1"]


async def test_fetch_pm_tasks_includes_stale_when_disabled(rw_conn):
    await seed_pm_task(rw_conn, task_key="A", expires_at="2099-01-01T00:00:00Z")
    await seed_pm_task(rw_conn, task_key="B", expires_at="2000-01-01T00:00:00Z")
    tasks = await fetch_pm_tasks(rw_conn, only_fresh=False)
    assert {t.task_key for t in tasks} == {"A", "B"}


async def test_fetch_pm_tasks_filters_by_provider(rw_conn):
    await seed_pm_task(rw_conn, task_key="J-1", provider="jira")
    await seed_pm_task(rw_conn, task_key="G-1", provider="github")
    await seed_pm_task(rw_conn, task_key="L-1", provider="linear")
    tasks = await fetch_pm_tasks(rw_conn, provider="jira")
    assert [t.task_key for t in tasks] == ["J-1"]


async def test_fetch_pm_tasks_excludes_done_by_default(rw_conn):
    await seed_pm_task(rw_conn, task_key="OPEN-1", status_category="in_progress")
    await seed_pm_task(rw_conn, task_key="DONE-1", status_category="done")
    tasks = await fetch_pm_tasks(rw_conn)
    assert [t.task_key for t in tasks] == ["OPEN-1"]


async def test_fetch_pm_tasks_includes_done_when_disabled(rw_conn):
    await seed_pm_task(rw_conn, task_key="OPEN-1", status_category="in_progress")
    await seed_pm_task(rw_conn, task_key="DONE-1", status_category="done")
    tasks = await fetch_pm_tasks(rw_conn, exclude_done=False)
    assert {t.task_key for t in tasks} == {"OPEN-1", "DONE-1"}


# ---------------------------------------------------------------------------
# session_summaries
# ---------------------------------------------------------------------------


async def test_write_session_summary_round_trip(rw_conn):
    etl = await seed_etl_run(rw_conn)
    sid = await seed_app_session(rw_conn, etl_run_id=etl)
    run_id = await start_run(rw_conn)
    payload = json.dumps({"summary": "edited db.py", "tags": ["meridian-agents"]})
    summary_id = await write_session_summary(
        rw_conn, session_id=sid, agent_run_id=run_id, summary_json=payload
    )
    assert summary_id > 0
    cur = await rw_conn.execute(
        "SELECT summary_json, agent_run_id, generated_at "
        "FROM session_summaries WHERE session_id = ?",
        (sid,),
    )
    row = await cur.fetchone()
    await cur.close()
    assert json.loads(row["summary_json"])["summary"] == "edited db.py"
    assert row["agent_run_id"] == run_id
    assert row["generated_at"]


async def test_write_session_summary_unique_per_session(rw_conn):
    etl = await seed_etl_run(rw_conn)
    sid = await seed_app_session(rw_conn, etl_run_id=etl)
    run_id = await start_run(rw_conn)
    await write_session_summary(
        rw_conn, session_id=sid, agent_run_id=run_id, summary_json='{"v":1}'
    )
    with pytest.raises(aiosqlite.IntegrityError):
        await write_session_summary(
            rw_conn, session_id=sid, agent_run_id=run_id, summary_json='{"v":2}'
        )


async def test_write_session_summary_rejects_unknown_session(rw_conn):
    run_id = await start_run(rw_conn)
    with pytest.raises(aiosqlite.IntegrityError):
        await write_session_summary(
            rw_conn, session_id=9999, agent_run_id=run_id, summary_json="{}"
        )


# ---------------------------------------------------------------------------
# context_graph_nodes
# ---------------------------------------------------------------------------


async def test_upsert_context_node_inserts_new_node(rw_conn):
    await upsert_context_node(
        rw_conn,
        node_id="project_meridian",
        node_type="project",
        label="Meridian",
        last_seen="2026-05-09T10:00:00Z",
    )
    [node] = await fetch_context_nodes(rw_conn)
    assert node.node_id == "project_meridian"
    assert node.node_type == "project"
    assert node.label == "Meridian"
    assert node.last_seen == "2026-05-09T10:00:00Z"
    assert node.frequency == 1
    assert node.confidence_avg == pytest.approx(0.7)


async def test_upsert_context_node_increments_frequency_on_conflict(rw_conn):
    await upsert_context_node(
        rw_conn,
        node_id="ticket_KAN-86",
        node_type="ticket",
        label="KAN-86",
        last_seen="2026-05-09T10:00:00Z",
    )
    await upsert_context_node(
        rw_conn,
        node_id="ticket_KAN-86",
        node_type="ticket",
        label="KAN-86",
        last_seen="2026-05-09T10:30:00Z",
    )
    await upsert_context_node(
        rw_conn,
        node_id="ticket_KAN-86",
        node_type="ticket",
        label="KAN-86 — porting",
        last_seen="2026-05-09T11:00:00Z",
    )
    [node] = await fetch_context_nodes(rw_conn)
    assert node.frequency == 3
    # label is refreshed on each upsert
    assert node.label == "KAN-86 — porting"
    assert node.last_seen == "2026-05-09T11:00:00Z"


async def test_upsert_context_node_default_last_seen_is_now(rw_conn):
    await upsert_context_node(
        rw_conn, node_id="tool_cargo", node_type="tool", label="cargo"
    )
    [node] = await fetch_context_nodes(rw_conn)
    assert node.last_seen  # populated by strftime, non-empty


async def test_upsert_context_node_rejects_unknown_type(rw_conn):
    with pytest.raises(ValueError, match="invalid node_type"):
        await upsert_context_node(
            rw_conn, node_id="weird", node_type="banana", label="?"
        )


async def test_fetch_context_nodes_filters_by_type(rw_conn):
    await upsert_context_node(
        rw_conn, node_id="p", node_type="project", label="P"
    )
    await upsert_context_node(rw_conn, node_id="t", node_type="task", label="T")
    await upsert_context_node(
        rw_conn, node_id="x", node_type="tool", label="X"
    )
    project_nodes = await fetch_context_nodes(rw_conn, node_type="project")
    assert [n.node_id for n in project_nodes] == ["p"]


async def test_fetch_context_nodes_orders_by_last_seen_desc(rw_conn):
    await upsert_context_node(
        rw_conn, node_id="old", node_type="tool", label="old", last_seen="2020-01-01T00:00:00Z"
    )
    await upsert_context_node(
        rw_conn, node_id="new", node_type="tool", label="new", last_seen="2026-05-09T10:00:00Z"
    )
    nodes = await fetch_context_nodes(rw_conn)
    assert [n.node_id for n in nodes] == ["new", "old"]


# ---------------------------------------------------------------------------
# activity_context
# ---------------------------------------------------------------------------


async def test_activity_context_seeded_with_id_one(rw_conn):
    ctx = await read_activity_context(rw_conn)
    assert ctx is not None
    assert ctx.inferred_task == ""
    assert ctx.confidence == 0.0
    assert ctx.trigger_jira_sync is False
    assert ctx.tags == []
    assert ctx.last_synced is None


async def test_write_activity_context_round_trip(rw_conn):
    await write_activity_context(
        rw_conn,
        active_project="meridian",
        jira_key="KAN-86",
        inferred_task="porting hermes synthesizer",
        confidence=0.92,
        trigger_jira_sync=True,
        tags=["agents", "port"],
    )
    ctx = await read_activity_context(rw_conn)
    assert ctx is not None
    assert ctx.active_project == "meridian"
    assert ctx.jira_key == "KAN-86"
    assert ctx.inferred_task == "porting hermes synthesizer"
    assert ctx.confidence == pytest.approx(0.92)
    assert ctx.trigger_jira_sync is True
    assert ctx.tags == ["agents", "port"]


async def test_write_activity_context_overwrites(rw_conn):
    await write_activity_context(
        rw_conn,
        inferred_task="first",
        confidence=0.5,
    )
    await write_activity_context(
        rw_conn,
        inferred_task="second",
        confidence=0.8,
        trigger_jira_sync=True,
    )
    ctx = await read_activity_context(rw_conn)
    assert ctx.inferred_task == "second"
    assert ctx.confidence == pytest.approx(0.8)
    assert ctx.trigger_jira_sync is True


async def test_mark_activity_context_synced_clears_trigger(rw_conn):
    await write_activity_context(
        rw_conn,
        inferred_task="x",
        confidence=0.9,
        trigger_jira_sync=True,
    )
    await mark_activity_context_synced(rw_conn, last_synced="2026-05-09T11:00:00Z")
    ctx = await read_activity_context(rw_conn)
    assert ctx.trigger_jira_sync is False
    assert ctx.last_synced == "2026-05-09T11:00:00Z"


async def test_mark_activity_context_synced_default_timestamp(rw_conn):
    await write_activity_context(rw_conn, inferred_task="x", confidence=0.9, trigger_jira_sync=True)
    await mark_activity_context_synced(rw_conn)
    ctx = await read_activity_context(rw_conn)
    assert ctx.last_synced  # populated by strftime


# ---------------------------------------------------------------------------
# link_ticket
# ---------------------------------------------------------------------------


async def test_link_ticket_inserts_with_method_llm(rw_conn):
    etl = await seed_etl_run(rw_conn)
    sid = await seed_app_session(rw_conn, etl_run_id=etl)
    await link_ticket(
        rw_conn,
        session_id=sid,
        task_key="KAN-86",
        provider="jira",
        confidence=0.91,
        routing="auto",
    )
    cur = await rw_conn.execute(
        "SELECT method, confidence, routing, session_type "
        "FROM ticket_links WHERE session_id = ?",
        (sid,),
    )
    row = await cur.fetchone()
    await cur.close()
    assert row["method"] == "llm"
    assert row["confidence"] == pytest.approx(0.91)
    assert row["routing"] == "auto"
    assert row["session_type"] == "task"


async def test_link_ticket_with_null_task_key(rw_conn):
    """Overhead and unknown sessions get a ticket_links row with no task."""
    etl = await seed_etl_run(rw_conn)
    sid = await seed_app_session(rw_conn, etl_run_id=etl)
    await link_ticket(
        rw_conn,
        session_id=sid,
        task_key=None,
        provider=None,
        confidence=0.0,
        routing="skip",
        session_type="overhead",
    )
    cur = await rw_conn.execute(
        "SELECT task_key, session_type FROM ticket_links WHERE session_id = ?",
        (sid,),
    )
    row = await cur.fetchone()
    await cur.close()
    assert row["task_key"] is None
    assert row["session_type"] == "overhead"


async def test_link_ticket_rejects_unknown_routing(rw_conn):
    etl = await seed_etl_run(rw_conn)
    sid = await seed_app_session(rw_conn, etl_run_id=etl)
    with pytest.raises(ValueError, match="invalid routing"):
        await link_ticket(
            rw_conn,
            session_id=sid,
            task_key="KAN-86",
            provider="jira",
            confidence=0.9,
            routing="ship-it",
        )


async def test_link_ticket_unique_per_session(rw_conn):
    etl = await seed_etl_run(rw_conn)
    sid = await seed_app_session(rw_conn, etl_run_id=etl)
    await link_ticket(
        rw_conn,
        session_id=sid,
        task_key="KAN-86",
        provider="jira",
        confidence=0.9,
        routing="auto",
    )
    with pytest.raises(aiosqlite.IntegrityError):
        await link_ticket(
            rw_conn,
            session_id=sid,
            task_key="KAN-87",
            provider="jira",
            confidence=0.5,
            routing="queue",
        )


# ---------------------------------------------------------------------------
# dispatch_queue lifecycle
# ---------------------------------------------------------------------------


async def test_enqueue_dispatch_round_trip(rw_conn):
    etl = await seed_etl_run(rw_conn)
    sid = await seed_app_session(rw_conn, etl_run_id=etl)
    run_id = await start_run(rw_conn)
    dispatch_id = await enqueue_dispatch(
        rw_conn,
        session_id=sid,
        agent_run_id=run_id,
        task_key="KAN-86",
        provider="jira",
        payload_json='{"comment":"hi"}',
    )
    [item] = await claim_dispatch_pending(rw_conn)
    assert item.id == dispatch_id
    assert item.session_id == sid
    assert item.agent_run_id == run_id
    assert item.task_key == "KAN-86"
    assert item.attempts == 0


async def test_enqueue_dispatch_rejects_unknown_provider(rw_conn):
    etl = await seed_etl_run(rw_conn)
    sid = await seed_app_session(rw_conn, etl_run_id=etl)
    run_id = await start_run(rw_conn)
    with pytest.raises(ValueError, match="invalid provider"):
        await enqueue_dispatch(
            rw_conn,
            session_id=sid,
            agent_run_id=run_id,
            task_key="KAN-86",
            provider="notion",
            payload_json="{}",
        )


async def test_enqueue_dispatch_rejects_missing_agent_run_fk(rw_conn):
    etl = await seed_etl_run(rw_conn)
    sid = await seed_app_session(rw_conn, etl_run_id=etl)
    with pytest.raises(aiosqlite.IntegrityError):
        await enqueue_dispatch(
            rw_conn,
            session_id=sid,
            agent_run_id=9999,
            task_key="KAN-86",
            provider="jira",
            payload_json="{}",
        )


async def test_mark_dispatched_sent_stamps_dispatched_at(rw_conn):
    etl = await seed_etl_run(rw_conn)
    sid = await seed_app_session(rw_conn, etl_run_id=etl)
    run_id = await start_run(rw_conn)
    dispatch_id = await enqueue_dispatch(
        rw_conn,
        session_id=sid,
        agent_run_id=run_id,
        task_key="KAN-86",
        provider="log",
        payload_json="{}",
    )
    await mark_dispatched(rw_conn, dispatch_id=dispatch_id, state="sent")
    cur = await rw_conn.execute(
        "SELECT state, attempts, dispatched_at, last_error "
        "FROM dispatch_queue WHERE id = ?",
        (dispatch_id,),
    )
    row = await cur.fetchone()
    await cur.close()
    assert row["state"] == "sent"
    assert row["attempts"] == 1
    assert row["dispatched_at"] is not None
    assert row["last_error"] is None


async def test_mark_dispatched_failed_keeps_dispatched_at_null(rw_conn):
    etl = await seed_etl_run(rw_conn)
    sid = await seed_app_session(rw_conn, etl_run_id=etl)
    run_id = await start_run(rw_conn)
    dispatch_id = await enqueue_dispatch(
        rw_conn,
        session_id=sid,
        agent_run_id=run_id,
        task_key="KAN-86",
        provider="jira",
        payload_json="{}",
    )
    await mark_dispatched(
        rw_conn, dispatch_id=dispatch_id, state="failed", error="429 rate-limited"
    )
    cur = await rw_conn.execute(
        "SELECT state, attempts, dispatched_at, last_error "
        "FROM dispatch_queue WHERE id = ?",
        (dispatch_id,),
    )
    row = await cur.fetchone()
    await cur.close()
    assert row["state"] == "failed"
    assert row["attempts"] == 1
    assert row["dispatched_at"] is None
    assert row["last_error"] == "429 rate-limited"


async def test_mark_dispatched_increments_attempts(rw_conn):
    etl = await seed_etl_run(rw_conn)
    sid = await seed_app_session(rw_conn, etl_run_id=etl)
    run_id = await start_run(rw_conn)
    dispatch_id = await enqueue_dispatch(
        rw_conn,
        session_id=sid,
        agent_run_id=run_id,
        task_key="KAN-86",
        provider="jira",
        payload_json="{}",
    )
    await mark_dispatched(rw_conn, dispatch_id=dispatch_id, state="failed")
    await mark_dispatched(rw_conn, dispatch_id=dispatch_id, state="failed")
    cur = await rw_conn.execute(
        "SELECT attempts FROM dispatch_queue WHERE id = ?", (dispatch_id,)
    )
    row = await cur.fetchone()
    await cur.close()
    assert row["attempts"] == 2


async def test_claim_dispatch_pending_filters_terminal_states(rw_conn):
    etl = await seed_etl_run(rw_conn)
    sid = await seed_app_session(rw_conn, etl_run_id=etl)
    run_id = await start_run(rw_conn)
    pending = await enqueue_dispatch(
        rw_conn,
        session_id=sid,
        agent_run_id=run_id,
        task_key="A",
        provider="log",
        payload_json="{}",
    )
    sent = await enqueue_dispatch(
        rw_conn,
        session_id=sid,
        agent_run_id=run_id,
        task_key="B",
        provider="log",
        payload_json="{}",
    )
    await mark_dispatched(rw_conn, dispatch_id=sent, state="sent")
    items = await claim_dispatch_pending(rw_conn)
    assert [i.id for i in items] == [pending]


# ---------------------------------------------------------------------------
# Functional smoke — full synthesizer tick lifecycle against the real schema
# ---------------------------------------------------------------------------


async def test_full_tick_lifecycle(rw_conn):
    """Mirrors what the synthesizer will do every poll interval.

    Insert ETL run + session + pm_task → start agent run → for one closed
    session: write summary, link ticket, enqueue dispatch → drain dispatch
    → write activity_context + mark synced → upsert context node →
    advance cursor → finish run.
    """
    etl = await seed_etl_run(rw_conn)
    await seed_app_session(rw_conn, etl_run_id=etl)
    await seed_pm_task(rw_conn, task_key="KAN-86")

    run_id = await start_run(rw_conn)
    [session] = await fetch_unanalysed_sessions(rw_conn, after_id=0)
    [task] = await fetch_pm_tasks(rw_conn)

    await write_session_summary(
        rw_conn,
        session_id=session.id,
        agent_run_id=run_id,
        summary_json=json.dumps({"summary": "vendored hermes"}),
    )
    await link_ticket(
        rw_conn,
        session_id=session.id,
        task_key=task.task_key,
        provider=task.provider,
        confidence=0.92,
        routing="auto",
    )
    await upsert_context_node(
        rw_conn,
        node_id="ticket_KAN-86",
        node_type="ticket",
        label="KAN-86",
    )
    await write_activity_context(
        rw_conn,
        active_project="meridian",
        jira_key="KAN-86",
        inferred_task="porting hermes synthesizer",
        confidence=0.92,
        trigger_jira_sync=True,
        tags=["agents"],
    )
    dispatch_id = await enqueue_dispatch(
        rw_conn,
        session_id=session.id,
        agent_run_id=run_id,
        task_key=task.task_key,
        provider="log",
        payload_json=json.dumps({"comment": "logged"}),
    )
    [item] = await claim_dispatch_pending(rw_conn)
    assert item.id == dispatch_id
    await mark_dispatched(rw_conn, dispatch_id=dispatch_id, state="sent")
    await mark_activity_context_synced(rw_conn)
    await advance_cursor(rw_conn, session.id)
    await finish_run(
        rw_conn,
        run_id,
        status="success",
        sessions_processed=1,
        summaries_written=1,
        links_written=1,
        dispatches_queued=1,
        dispatches_sent=1,
    )

    # Verify final state across all the agent-owned tables.
    assert await read_cursor(rw_conn) == session.id

    cur = await rw_conn.execute(
        "SELECT status, summaries_written FROM agent_runs WHERE id = ?", (run_id,)
    )
    run_row = await cur.fetchone()
    await cur.close()
    assert run_row["status"] == "success"
    assert run_row["summaries_written"] == 1

    ctx = await read_activity_context(rw_conn)
    assert ctx.trigger_jira_sync is False
    assert ctx.last_synced is not None
    assert ctx.jira_key == "KAN-86"

    [node] = await fetch_context_nodes(rw_conn)
    assert node.node_id == "ticket_KAN-86"

    # Re-fetching unanalysed should now return nothing for this session
    assert await fetch_unanalysed_sessions(rw_conn, after_id=0) == []


# ---------------------------------------------------------------------------
# Anti-test: the dropped columns must NOT be on app_sessions
# ---------------------------------------------------------------------------


async def test_app_sessions_does_not_have_summary_json_column(rw_conn):
    cur = await rw_conn.execute("PRAGMA table_info(app_sessions)")
    columns = {row["name"] async for row in cur}
    await cur.close()
    assert "summary_json" not in columns
    # session_summaries.summary_json lives in its own table now.


async def test_app_sessions_does_not_have_activity_kind_column(rw_conn):
    cur = await rw_conn.execute("PRAGMA table_info(app_sessions)")
    columns = {row["name"] async for row in cur}
    await cur.close()
    assert "activity_kind" not in columns
    # Rust 004_categories provides 'category' (and 'confidence') instead.
