# meridian — normalises screenpipe activity into structured app sessions
"""Tests for run_task_linker_mlx — outlines-based MLX in-process classification.

Run from services/:
    python -m pytest agents/tests/test_run_task_linker_mlx.py -v
"""
from __future__ import annotations

import json
import sqlite3
import sys
from io import StringIO
from pathlib import Path
from unittest.mock import MagicMock, patch

import pytest

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

_GOOD_JSON = json.dumps({
    "task_key":     "KAN-42",
    "confidence":   0.85,
    "session_type": "task",
    "reasoning":    "Editing run_watcher.py with KAN-42 ticket open.",
    "dimensions":   {"activity": ["coding"], "tool": ["vscode"]},
})

_OVERHEAD_JSON = json.dumps({
    "task_key":     None,
    "confidence":   0.0,
    "session_type": "overhead",
    "reasoning":    "Idle desktop session, no work signal.",
    "dimensions":   {},
})


@pytest.fixture()
def db(tmp_path: Path) -> Path:
    """Minimal SQLite DB with one session and one PM task."""
    p = tmp_path / "meridian.db"
    con = sqlite3.connect(str(p))
    con.executescript("""
        CREATE TABLE app_sessions (
            id INTEGER PRIMARY KEY,
            app_name TEXT, started_at TEXT, ended_at TEXT,
            duration_s REAL, session_text TEXT, session_text_source TEXT,
            window_titles TEXT DEFAULT '[]',
            category TEXT, confidence REAL DEFAULT 0.0,
            task_key TEXT, task_routing TEXT
        );
        CREATE TABLE pm_tasks (
            task_key TEXT PRIMARY KEY, title TEXT, description_text TEXT,
            status_category TEXT DEFAULT 'in_progress',
            issue_type TEXT, parent_key TEXT, epic_title TEXT, sprint_name TEXT,
            assignee_name TEXT
        );
    """)
    con.execute("""
        INSERT INTO app_sessions
            (id, app_name, started_at, ended_at, duration_s,
             session_text, session_text_source, window_titles, category, confidence)
        VALUES (1, 'Code', '2026-01-01T10:00:00', '2026-01-01T10:05:00', 300,
                'editing run_watcher.py implementation', 'ocr',
                '["run_watcher.py"]', 'coding', 0.9)
    """)
    con.execute("""
        INSERT INTO pm_tasks (task_key, title, description_text, status_category)
        VALUES ('KAN-42', 'Fix gap detection',
                'Fix gap detection across ETL run boundaries', 'in_progress')
    """)
    con.commit()
    con.close()
    return p


@pytest.fixture(autouse=True)
def clear_model_cache():
    """Isolate the in-process model cache between tests."""
    import agents.run_task_linker_mlx as m
    m._model_cache.clear()
    yield
    m._model_cache.clear()


def _make_outlines_model(return_json: str = _GOOD_JSON) -> MagicMock:
    """Return a mock outlines model that returns return_json when called."""
    mock = MagicMock(name="outlines_model")
    mock.return_value = return_json
    return mock


def _patch_modules(outlines_model: MagicMock) -> dict:
    """Build the sys.modules patch dict for outlines + mlx_lm."""
    mock_mlx_lm = MagicMock(name="mlx_lm")
    mock_mlx_lm.load.return_value = (MagicMock(name="mlx_model"), MagicMock(name="tokenizer"))

    mock_sample_utils = MagicMock(name="mlx_lm.sample_utils")
    mock_sample_utils.make_sampler.return_value = MagicMock(name="sampler")

    mock_outlines = MagicMock(name="outlines")
    mock_outlines.from_mlxlm.return_value = outlines_model

    mock_outlines_inputs = MagicMock(name="outlines.inputs")
    mock_outlines_inputs.Chat = MagicMock(name="Chat")

    return {
        "outlines":            mock_outlines,
        "outlines.inputs":     mock_outlines_inputs,
        "mlx_lm":              mock_mlx_lm,
        "mlx_lm.sample_utils": mock_sample_utils,
    }


# ---------------------------------------------------------------------------
# _classify_one
# ---------------------------------------------------------------------------

class TestClassifyOne:
    def test_successful_task_classification(self, db: Path):
        import agents.run_task_linker_mlx as m

        model = _make_outlines_model(_GOOD_JSON)
        with patch.dict(sys.modules, _patch_modules(model)), \
             patch.object(m, "_get_model", return_value=model):
            con = sqlite3.connect(str(db))
            con.row_factory = sqlite3.Row
            result = m._classify_one(1, con)
            con.close()

        assert result["session_id"] == 1
        assert result["task_key"] == "KAN-42"
        assert result["confidence"] == pytest.approx(0.85)
        assert result["session_type"] == "task"
        assert result["method"] == "mlx_direct"
        assert "coding" in result["dimensions"].get("activity", [])
        assert result["elapsed_s"] >= 0.0

    def test_session_not_found_returns_error_dict(self, db: Path):
        import agents.run_task_linker_mlx as m

        con = sqlite3.connect(str(db))
        con.row_factory = sqlite3.Row
        result = m._classify_one(9999, con)
        con.close()

        assert result["session_id"] == 9999
        assert result["task_key"] is None
        assert result["method"] == "mlx_error"
        assert result["elapsed_s"] == 0.0

    def test_invalid_task_key_triggers_parse_error(self, db: Path):
        import agents.run_task_linker_mlx as m

        bad_json = json.dumps({
            "task_key":     "NONEXISTENT-999",
            "confidence":   0.9,
            "session_type": "task",
            "reasoning":    "test",
            "dimensions":   {},
        })
        model = _make_outlines_model(bad_json)
        with patch.dict(sys.modules, _patch_modules(model)), \
             patch.object(m, "_get_model", return_value=model):
            con = sqlite3.connect(str(db))
            con.row_factory = sqlite3.Row
            result = m._classify_one(1, con)
            con.close()

        assert result["task_key"] is None
        assert result["method"] == "mlx_parse_error"

    def test_inference_exception_returns_error_dict(self, db: Path):
        import agents.run_task_linker_mlx as m

        model = MagicMock(name="outlines_model")
        model.side_effect = RuntimeError("OOM")
        with patch.dict(sys.modules, _patch_modules(model)), \
             patch.object(m, "_get_model", return_value=model):
            con = sqlite3.connect(str(db))
            con.row_factory = sqlite3.Row
            result = m._classify_one(1, con)
            con.close()

        assert result["task_key"] is None
        assert result["method"] == "mlx_error"
        assert "OOM" in result["reasoning"]

    def test_overhead_classification(self, db: Path):
        import agents.run_task_linker_mlx as m

        model = _make_outlines_model(_OVERHEAD_JSON)
        with patch.dict(sys.modules, _patch_modules(model)), \
             patch.object(m, "_get_model", return_value=model):
            con = sqlite3.connect(str(db))
            con.row_factory = sqlite3.Row
            result = m._classify_one(1, con)
            con.close()

        assert result["task_key"] is None
        assert result["session_type"] == "overhead"
        assert result["method"] == "mlx_direct"

    def test_confidence_clamped_above_1(self, db: Path):
        """Confidence values beyond [0,1] are clamped after parse."""
        import agents.run_task_linker_mlx as m

        model = _make_outlines_model(_GOOD_JSON)
        original_validate = m.SessionClassification.model_validate_json

        def patched_validate(raw):
            obj = original_validate(raw)
            obj.confidence = 1.5
            return obj

        with patch.dict(sys.modules, _patch_modules(model)), \
             patch.object(m, "_get_model", return_value=model), \
             patch.object(m.SessionClassification, "model_validate_json",
                          staticmethod(patched_validate)):
            con = sqlite3.connect(str(db))
            con.row_factory = sqlite3.Row
            result = m._classify_one(1, con)
            con.close()

        assert result["confidence"] <= 1.0


# ---------------------------------------------------------------------------
# main() — stdin/stdout contract
# ---------------------------------------------------------------------------

class TestMain:
    def _run_main(self, stdin_payload: dict, db: Path) -> dict:
        import agents.run_task_linker_mlx as m

        stdin_data = json.dumps({**stdin_payload, "meridian_db": str(db)})
        captured_out = StringIO()

        with patch("sys.stdin", StringIO(stdin_data)), \
             patch("sys.stdout", captured_out):
            m.main()

        return json.loads(captured_out.getvalue())

    def test_empty_session_ids_returns_empty_results(self, db: Path):
        result = self._run_main({"session_ids": []}, db)
        assert result == {"results": []}

    def test_missing_db_path_exits_1(self):
        import agents.run_task_linker_mlx as m

        with patch("sys.stdin", StringIO(json.dumps({"session_ids": [1], "meridian_db": ""}))):
            with pytest.raises(SystemExit) as exc:
                m.main()
        assert exc.value.code == 1

    def test_nonexistent_db_exits_1(self, tmp_path: Path):
        import agents.run_task_linker_mlx as m

        payload = json.dumps({
            "session_ids": [1],
            "meridian_db": str(tmp_path / "no_such.db"),
        })
        with patch("sys.stdin", StringIO(payload)):
            with pytest.raises(SystemExit) as exc:
                m.main()
        assert exc.value.code == 1

    def test_malformed_stdin_exits_1(self):
        import agents.run_task_linker_mlx as m

        with patch("sys.stdin", StringIO("not json {")):
            with pytest.raises(SystemExit) as exc:
                m.main()
        assert exc.value.code == 1

    def test_successful_run_returns_correct_shape(self, db: Path):
        import agents.run_task_linker_mlx as m

        model = _make_outlines_model(_GOOD_JSON)
        with patch.dict(sys.modules, _patch_modules(model)), \
             patch.object(m, "_get_model", return_value=model):
            output = self._run_main({"session_ids": [1]}, db)

        assert "results" in output
        assert len(output["results"]) == 1
        r = output["results"][0]

        for field in ("session_id", "task_key", "confidence", "session_type",
                      "reasoning", "method", "dimensions", "elapsed_s"):
            assert field in r, f"missing field: {field}"

        assert r["session_id"] == 1
        assert r["task_key"] == "KAN-42"
        assert r["method"] == "mlx_direct"

    def test_cursor_advances_for_each_session(self, db: Path):
        """Multiple sessions produce one result each in order."""
        import agents.run_task_linker_mlx as m

        con = sqlite3.connect(str(db))
        con.execute("""
            INSERT INTO app_sessions
                (id, app_name, started_at, ended_at, duration_s,
                 session_text, session_text_source, window_titles, category, confidence)
            VALUES (2, 'Terminal', '2026-01-01T10:06:00', '2026-01-01T10:07:00', 60,
                    'running cargo test', 'ocr', '["terminal"]', 'coding', 0.8)
        """)
        con.commit()
        con.close()

        model = _make_outlines_model(_OVERHEAD_JSON)
        with patch.dict(sys.modules, _patch_modules(model)), \
             patch.object(m, "_get_model", return_value=model):
            output = self._run_main({"session_ids": [1, 2]}, db)

        assert len(output["results"]) == 2
        assert {r["session_id"] for r in output["results"]} == {1, 2}


# ---------------------------------------------------------------------------
# _get_model caching
# ---------------------------------------------------------------------------

class TestModelCache:
    def test_model_loaded_only_once(self):
        import agents.run_task_linker_mlx as m

        mock_mlx_lm = MagicMock(name="mlx_lm")
        mock_mlx_lm.load.return_value = (MagicMock(), MagicMock())
        mock_sample_utils = MagicMock(name="mlx_lm.sample_utils")

        outlines_model = _make_outlines_model()
        mock_outlines = MagicMock(name="outlines")
        mock_outlines.from_mlxlm.return_value = outlines_model

        with patch.dict(sys.modules, {
            "outlines":            mock_outlines,
            "mlx_lm":              mock_mlx_lm,
            "mlx_lm.sample_utils": mock_sample_utils,
        }):
            m._model_cache.clear()
            m._get_model()
            m._get_model()

        mock_mlx_lm.load.assert_called_once()
        mock_outlines.from_mlxlm.assert_called_once()

    def test_cache_cleared_between_tests(self):
        import agents.run_task_linker_mlx as m
        assert m._model_cache == {}

    def test_missing_outlines_raises_import_error(self):
        import agents.run_task_linker_mlx as m

        m._model_cache.clear()
        with patch.dict(sys.modules, {"outlines": None, "mlx_lm": None}):
            with pytest.raises((ImportError, TypeError)):
                m._get_model()


# ---------------------------------------------------------------------------
# SessionClassification schema
# ---------------------------------------------------------------------------

class TestSessionClassificationSchema:
    def test_valid_task_classification(self):
        from agents.run_task_linker_mlx import SessionClassification

        obj = SessionClassification.model_validate_json(_GOOD_JSON)
        assert obj.task_key == "KAN-42"
        assert obj.confidence == pytest.approx(0.85)
        assert obj.session_type == "task"
        assert "coding" in obj.dimensions.get("activity", [])

    def test_valid_overhead_classification(self):
        from agents.run_task_linker_mlx import SessionClassification

        obj = SessionClassification.model_validate_json(_OVERHEAD_JSON)
        assert obj.task_key is None
        assert obj.session_type == "overhead"
        assert obj.dimensions == {}

    def test_invalid_session_type_raises(self):
        from agents.run_task_linker_mlx import SessionClassification
        from pydantic import ValidationError

        bad = json.dumps({
            "task_key": None,
            "confidence": 0.5,
            "session_type": "bogus",
            "reasoning": "test",
            "dimensions": {},
        })
        with pytest.raises(ValidationError):
            SessionClassification.model_validate_json(bad)
