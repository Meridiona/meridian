# meridian — normalises screenpipe activity into structured app sessions

"""Shared pytest fixtures.

`migrated_db_path` applies the real `src/migrations/00*.sql` files (the same
ones the Rust daemon runs via sqlx::migrate!) against a fresh temp-file
SQLite database. This means our Python tests exercise the actual schema —
they fail loudly if the Rust side changes a column shape that db.py relies
on.
"""

from __future__ import annotations

import asyncio
from collections.abc import AsyncIterator
from pathlib import Path

import aiosqlite
import pytest

REPO_ROOT = Path(__file__).resolve().parents[3]
MIGRATIONS_DIR = REPO_ROOT / "src" / "migrations"


def _migration_files() -> list[Path]:
    """All numbered .sql migrations in src/migrations, sorted by name.

    sqlx::migrate! sorts by filename — we mirror that so our tests apply
    them in the same order Rust does.
    """
    files = sorted(MIGRATIONS_DIR.glob("[0-9][0-9][0-9]_*.sql"))
    if not files:
        raise FileNotFoundError(
            f"no migrations found in {MIGRATIONS_DIR} — is the test running "
            "from inside the meridian repo?"
        )
    return files


async def _apply_migrations(db_path: Path) -> None:
    async with aiosqlite.connect(db_path) as conn:
        await conn.execute("PRAGMA journal_mode = WAL")
        await conn.execute("PRAGMA foreign_keys = ON")
        for migration in _migration_files():
            sql = migration.read_text()
            await conn.executescript(sql)
        await conn.commit()


@pytest.fixture
def migrated_db_path(tmp_path: Path) -> Path:
    """Path to a fresh SQLite file with all migrations applied."""
    db_path = tmp_path / "meridian.db"
    asyncio.get_event_loop_policy().new_event_loop().run_until_complete(
        _apply_migrations(db_path)
    )
    return db_path


@pytest.fixture
async def rw_conn(migrated_db_path: Path) -> AsyncIterator[aiosqlite.Connection]:
    """Open a read-write connection to a freshly migrated DB."""
    from meridian_agents.db import open_rw

    conn = await open_rw(str(migrated_db_path))
    try:
        yield conn
    finally:
        await conn.close()


@pytest.fixture
async def ro_conn(migrated_db_path: Path) -> AsyncIterator[aiosqlite.Connection]:
    from meridian_agents.db import open_ro

    conn = await open_ro(str(migrated_db_path))
    try:
        yield conn
    finally:
        await conn.close()


# ---------------------------------------------------------------------------
# Seed helpers — useful in multiple test modules
# ---------------------------------------------------------------------------


async def seed_etl_run(conn: aiosqlite.Connection) -> int:
    cur = await conn.execute(
        """
        INSERT INTO etl_runs (started_at, from_frame_id, to_frame_id, status)
        VALUES ('2026-05-09T10:00:00Z', 0, 0, 'success')
        """
    )
    run_id = cur.lastrowid
    await cur.close()
    assert run_id is not None
    return run_id


async def seed_app_session(
    conn: aiosqlite.Connection,
    *,
    etl_run_id: int,
    app_name: str = "Cursor",
    duration_s: int = 600,
    started_at: str = "2026-05-09T10:00:00Z",
    ended_at: str = "2026-05-09T10:10:00Z",
    window_titles: str = '[["meridian — config.rs", 5]]',
    ocr_samples: str = '["fn main() {}"]',
    audio_snippets: str = "[]",
    signals: str = "{}",
    activity_kind: str | None = None,
) -> int:
    cur = await conn.execute(
        """
        INSERT INTO app_sessions (
            app_name, started_at, ended_at, duration_s,
            window_titles, ocr_samples, elements_samples,
            audio_snippets, signals,
            min_frame_id, max_frame_id, frame_count,
            idle_frame_count, etl_run_id, activity_kind
        ) VALUES (?, ?, ?, ?, ?, ?, '[]', ?, ?, 1, 1, 1, 0, ?, ?)
        """,
        (
            app_name,
            started_at,
            ended_at,
            duration_s,
            window_titles,
            ocr_samples,
            audio_snippets,
            signals,
            etl_run_id,
            activity_kind,
        ),
    )
    session_id = cur.lastrowid
    await cur.close()
    assert session_id is not None
    return session_id


async def seed_pm_task(
    conn: aiosqlite.Connection,
    *,
    task_key: str = "KAN-86",
    provider: str = "jira",
    title: str = "migrate intelligence code",
    expires_at: str = "2099-01-01T00:00:00Z",
) -> None:
    await conn.execute(
        """
        INSERT INTO pm_tasks (task_key, provider, title, updated_at, expires_at)
        VALUES (?, ?, ?, '2026-05-09T10:00:00Z', ?)
        """,
        (task_key, provider, title, expires_at),
    )
