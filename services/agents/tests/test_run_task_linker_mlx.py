# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
"""Tests for run_task_linker_mlx — outlines-based MLX in-process classification.

Run from services/:
    python -m pytest agents/tests/test_run_task_linker_mlx.py -v
"""
from __future__ import annotations

import json
import sqlite3
import sys
import time
from io import StringIO
from pathlib import Path
from typing import Iterator
from unittest.mock import MagicMock, patch

import pytest
from opentelemetry.sdk.trace import TracerProvider
from opentelemetry.sdk.trace.export import SimpleSpanProcessor
from opentelemetry.sdk.trace.export.in_memory_span_exporter import InMemorySpanExporter

# ---------------------------------------------------------------------------
# Fixtures — DB and OTel
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

_UNTRACKED_JSON = json.dumps({
    "task_key":     None,
    "confidence":   0.6,
    "session_type": "untracked",
    "reasoning":    "Real coding work but no matching ticket.",
    "dimensions":   {"activity": ["coding"]},
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
            status_raw TEXT DEFAULT '',
            is_terminal INTEGER DEFAULT 0,
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
        INSERT INTO pm_tasks (task_key, title, description_text, status_raw, is_terminal)
        VALUES ('KAN-42', 'Fix gap detection',
                'Fix gap detection across ETL run boundaries', 'In Progress', 0)
    """)
    con.commit()
    con.close()
    return p


@pytest.fixture()
def span_exporter() -> Iterator[InMemorySpanExporter]:
    """Swap the module's tracer for one backed by an InMemorySpanExporter.

    All spans created by _classify_one / main() during the test are captured
    and available via exporter.get_finished_spans().
    """
    import agents.run_task_linker_mlx as mod

    exporter = InMemorySpanExporter()
    provider = TracerProvider()
    provider.add_span_processor(SimpleSpanProcessor(exporter))
    test_tracer = provider.get_tracer("test")

    original = mod.tracer
    mod.tracer = test_tracer
    yield exporter
    mod.tracer = original
    exporter.clear()


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


def _span_by_name(spans, name: str):
    """Return first finished span with the given name, or None."""
    return next((s for s in spans if s.name == name), None)


def _events_by_name(span, event_name: str) -> list:
    return [e for e in span.events if e.name == event_name]


# ---------------------------------------------------------------------------
# _classify_one — functional correctness (pre-existing tests preserved)
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

    def test_untracked_classification(self, db: Path):
        import agents.run_task_linker_mlx as m

        model = _make_outlines_model(_UNTRACKED_JSON)
        with patch.dict(sys.modules, _patch_modules(model)), \
             patch.object(m, "_get_model", return_value=model):
            con = sqlite3.connect(str(db))
            con.row_factory = sqlite3.Row
            result = m._classify_one(1, con)
            con.close()

        assert result["task_key"] is None
        assert result["session_type"] == "untracked"
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

    def test_confidence_clamped_below_0(self, db: Path):
        """Negative confidence is clamped to 0.0."""
        import agents.run_task_linker_mlx as m

        model = _make_outlines_model(_GOOD_JSON)
        original_validate = m.SessionClassification.model_validate_json

        def patched_validate(raw):
            obj = original_validate(raw)
            obj.confidence = -0.5
            return obj

        with patch.dict(sys.modules, _patch_modules(model)), \
             patch.object(m, "_get_model", return_value=model), \
             patch.object(m.SessionClassification, "model_validate_json",
                          staticmethod(patched_validate)):
            con = sqlite3.connect(str(db))
            con.row_factory = sqlite3.Row
            result = m._classify_one(1, con)
            con.close()

        assert result["confidence"] >= 0.0


# ---------------------------------------------------------------------------
# OTel — span structure emitted by _classify_one
# ---------------------------------------------------------------------------

class TestObservabilityClassifyOne:
    """Verify that _classify_one emits the correct OTel spans and attributes."""

    def _run(self, db: Path, span_exporter: InMemorySpanExporter,
             return_json: str = _GOOD_JSON):
        import agents.run_task_linker_mlx as m
        model = _make_outlines_model(return_json)
        with patch.dict(sys.modules, _patch_modules(model)), \
             patch.object(m, "_get_model", return_value=model):
            con = sqlite3.connect(str(db))
            con.row_factory = sqlite3.Row
            result = m._classify_one(1, con)
            con.close()
        return result, span_exporter.get_finished_spans()

    # ── span names ────────────────────────────────────────────────────────────

    def test_four_spans_emitted(self, db: Path, span_exporter):
        _, spans = self._run(db, span_exporter)
        names = [s.name for s in spans]
        assert "db_fetch" in names
        assert "build_prompt" in names
        assert "llm_inference" in names
        assert "parse_response" in names

    # ── db_fetch ──────────────────────────────────────────────────────────────

    def test_db_fetch_pm_tasks_count(self, db: Path, span_exporter):
        _, spans = self._run(db, span_exporter)
        s = _span_by_name(spans, "db_fetch")
        assert s is not None
        assert s.attributes["pm_tasks_count"] == 1

    def test_db_fetch_recent_sessions_count(self, db: Path, span_exporter):
        _, spans = self._run(db, span_exporter)
        s = _span_by_name(spans, "db_fetch")
        assert s.attributes["recent_sessions_count"] == 0  # no prior sessions

    def test_db_fetch_session_loaded_event_fields(self, db: Path, span_exporter):
        _, spans = self._run(db, span_exporter)
        s = _span_by_name(spans, "db_fetch")
        events = _events_by_name(s, "session_loaded")
        assert len(events) == 1
        attrs = events[0].attributes
        assert attrs["app_name"] == "Code"
        assert attrs["duration_s"] == pytest.approx(300.0)
        assert attrs["category"] == "coding"
        assert attrs["session_text_chars"] > 0
        assert attrs["text_source"] == "ocr"

    def test_db_fetch_context_loaded_event_fields(self, db: Path, span_exporter):
        _, spans = self._run(db, span_exporter)
        s = _span_by_name(spans, "db_fetch")
        events = _events_by_name(s, "context_loaded")
        assert len(events) == 1
        assert events[0].attributes["pm_tasks_count"] == 1

    def test_db_fetch_error_status_on_missing_session(self, db: Path, span_exporter):
        import agents.run_task_linker_mlx as m
        from opentelemetry.trace import StatusCode as SC

        con = sqlite3.connect(str(db))
        con.row_factory = sqlite3.Row
        m._classify_one(9999, con)
        con.close()

        spans = span_exporter.get_finished_spans()
        s = _span_by_name(spans, "db_fetch")
        assert s is not None
        assert s.status.status_code == SC.ERROR

    def test_db_fetch_session_not_found_event(self, db: Path, span_exporter):
        import agents.run_task_linker_mlx as m

        con = sqlite3.connect(str(db))
        con.row_factory = sqlite3.Row
        m._classify_one(9999, con)
        con.close()

        spans = span_exporter.get_finished_spans()
        s = _span_by_name(spans, "db_fetch")
        events = _events_by_name(s, "session_not_found")
        assert len(events) == 1
        assert events[0].attributes["session_id"] == 9999

    def test_db_fetch_only_span_emitted_when_session_missing(
        self, db: Path, span_exporter
    ):
        """No build_prompt / llm_inference / parse_response when session is missing."""
        import agents.run_task_linker_mlx as m

        con = sqlite3.connect(str(db))
        con.row_factory = sqlite3.Row
        m._classify_one(9999, con)
        con.close()

        names = [s.name for s in span_exporter.get_finished_spans()]
        assert "db_fetch" in names
        assert "build_prompt" not in names
        assert "llm_inference" not in names
        assert "parse_response" not in names

    # ── build_prompt ──────────────────────────────────────────────────────────

    def test_build_prompt_attributes(self, db: Path, span_exporter):
        _, spans = self._run(db, span_exporter)
        s = _span_by_name(spans, "build_prompt")
        assert s is not None
        assert s.attributes["pm_tasks_count"] == 1
        assert s.attributes["recent_sessions_count"] == 0
        assert s.attributes["prompt_chars"] > 0

    def test_build_prompt_assembled_event(self, db: Path, span_exporter):
        _, spans = self._run(db, span_exporter)
        s = _span_by_name(spans, "build_prompt")
        events = _events_by_name(s, "prompt_assembled")
        assert len(events) == 1
        attrs = events[0].attributes
        assert attrs["pm_tasks_included"] == 1
        assert attrs["session_text_chars"] > 0
        assert attrs["prompt_chars"] > 0
        assert "prompt_text" in attrs

    # ── llm_inference ─────────────────────────────────────────────────────────

    def test_llm_inference_model_attribute(self, db: Path, span_exporter):
        import agents.run_task_linker_mlx as m
        _, spans = self._run(db, span_exporter)
        s = _span_by_name(spans, "llm_inference")
        assert s is not None
        assert s.attributes["model"] == m._MLX_MODEL_ID

    def test_llm_inference_outcome_on_success(self, db: Path, span_exporter):
        _, spans = self._run(db, span_exporter)
        s = _span_by_name(spans, "llm_inference")
        assert s.attributes["outcome"] == "mlx_direct"

    def test_llm_inference_elapsed_s_recorded(self, db: Path, span_exporter):
        _, spans = self._run(db, span_exporter)
        s = _span_by_name(spans, "llm_inference")
        assert "elapsed_s" in s.attributes
        assert s.attributes["elapsed_s"] >= 0.0

    def test_llm_inference_response_chars_recorded(self, db: Path, span_exporter):
        _, spans = self._run(db, span_exporter)
        s = _span_by_name(spans, "llm_inference")
        assert s.attributes["response_chars"] == len(_GOOD_JSON)

    def test_llm_inference_started_and_complete_events(self, db: Path, span_exporter):
        _, spans = self._run(db, span_exporter)
        s = _span_by_name(spans, "llm_inference")
        assert _events_by_name(s, "inference_started")
        assert _events_by_name(s, "inference_complete")

    def test_llm_inference_error_status_on_exception(self, db: Path, span_exporter):
        import agents.run_task_linker_mlx as m
        from opentelemetry.trace import StatusCode as SC

        model = MagicMock(side_effect=RuntimeError("GPU OOM"))
        with patch.dict(sys.modules, _patch_modules(model)), \
             patch.object(m, "_get_model", return_value=model):
            con = sqlite3.connect(str(db))
            con.row_factory = sqlite3.Row
            m._classify_one(1, con)
            con.close()

        spans = span_exporter.get_finished_spans()
        s = _span_by_name(spans, "llm_inference")
        assert s is not None
        assert s.status.status_code == SC.ERROR

    def test_llm_inference_error_event_on_exception(self, db: Path, span_exporter):
        import agents.run_task_linker_mlx as m

        model = MagicMock(side_effect=RuntimeError("GPU OOM"))
        with patch.dict(sys.modules, _patch_modules(model)), \
             patch.object(m, "_get_model", return_value=model):
            con = sqlite3.connect(str(db))
            con.row_factory = sqlite3.Row
            m._classify_one(1, con)
            con.close()

        spans = span_exporter.get_finished_spans()
        s = _span_by_name(spans, "llm_inference")
        events = _events_by_name(s, "inference_error")
        assert len(events) == 1
        assert events[0].attributes["error_type"] == "RuntimeError"
        assert "GPU OOM" in events[0].attributes["error_message"]

    def test_no_parse_response_span_on_inference_failure(
        self, db: Path, span_exporter
    ):
        """parse_response must not be emitted when inference raises."""
        import agents.run_task_linker_mlx as m

        model = MagicMock(side_effect=RuntimeError("crash"))
        with patch.dict(sys.modules, _patch_modules(model)), \
             patch.object(m, "_get_model", return_value=model):
            con = sqlite3.connect(str(db))
            con.row_factory = sqlite3.Row
            m._classify_one(1, con)
            con.close()

        names = [s.name for s in span_exporter.get_finished_spans()]
        assert "parse_response" not in names

    # ── parse_response ────────────────────────────────────────────────────────

    def test_parse_response_outcome_ok_on_success(self, db: Path, span_exporter):
        _, spans = self._run(db, span_exporter)
        s = _span_by_name(spans, "parse_response")
        assert s is not None
        assert s.attributes["outcome"] == "ok"

    def test_parse_response_task_key_attribute(self, db: Path, span_exporter):
        _, spans = self._run(db, span_exporter)
        s = _span_by_name(spans, "parse_response")
        assert s.attributes["task_key"] == "KAN-42"

    def test_parse_response_confidence_attribute(self, db: Path, span_exporter):
        _, spans = self._run(db, span_exporter)
        s = _span_by_name(spans, "parse_response")
        assert s.attributes["confidence"] == pytest.approx(0.85)

    def test_parse_response_raw_mlx_output_event(self, db: Path, span_exporter):
        _, spans = self._run(db, span_exporter)
        s = _span_by_name(spans, "parse_response")
        events = _events_by_name(s, "raw_mlx_output")
        assert len(events) == 1
        assert events[0].attributes["chars"] == len(_GOOD_JSON)

    def test_parse_response_parse_success_event(self, db: Path, span_exporter):
        _, spans = self._run(db, span_exporter)
        s = _span_by_name(spans, "parse_response")
        events = _events_by_name(s, "parse_success")
        assert len(events) == 1
        attrs = events[0].attributes
        assert attrs["task_key"] == "KAN-42"
        assert attrs["session_type"] == "task"
        assert attrs["confidence"] == pytest.approx(0.85)
        assert attrs["dimensions_count"] == 2

    def test_parse_response_error_on_invalid_task_key(self, db: Path, span_exporter):
        import agents.run_task_linker_mlx as m
        from opentelemetry.trace import StatusCode as SC

        bad_json = json.dumps({
            "task_key": "NONEXISTENT-999", "confidence": 0.9,
            "session_type": "task", "reasoning": "test", "dimensions": {},
        })
        model = _make_outlines_model(bad_json)
        with patch.dict(sys.modules, _patch_modules(model)), \
             patch.object(m, "_get_model", return_value=model):
            con = sqlite3.connect(str(db))
            con.row_factory = sqlite3.Row
            m._classify_one(1, con)
            con.close()

        spans = span_exporter.get_finished_spans()
        s = _span_by_name(spans, "parse_response")
        assert s is not None
        assert s.status.status_code == SC.ERROR
        assert s.attributes["outcome"] == "invalid_task_key"

    def test_parse_response_parse_failure_event_on_invalid_key(
        self, db: Path, span_exporter
    ):
        import agents.run_task_linker_mlx as m

        bad_json = json.dumps({
            "task_key": "NONEXISTENT-999", "confidence": 0.9,
            "session_type": "task", "reasoning": "test", "dimensions": {},
        })
        model = _make_outlines_model(bad_json)
        with patch.dict(sys.modules, _patch_modules(model)), \
             patch.object(m, "_get_model", return_value=model):
            con = sqlite3.connect(str(db))
            con.row_factory = sqlite3.Row
            m._classify_one(1, con)
            con.close()

        spans = span_exporter.get_finished_spans()
        s = _span_by_name(spans, "parse_response")
        events = _events_by_name(s, "parse_failure")
        assert len(events) == 1
        assert "NONEXISTENT-999" in events[0].attributes["error"]

    def test_parse_response_overhead_task_key_attribute_is_dash(
        self, db: Path, span_exporter
    ):
        """Null task_key is stored as '-' in the span attribute."""
        _, spans = self._run(db, span_exporter, return_json=_OVERHEAD_JSON)
        s = _span_by_name(spans, "parse_response")
        assert s.attributes["task_key"] == "-"

    # ── span ordering ─────────────────────────────────────────────────────────

    def test_spans_finish_in_order(self, db: Path, span_exporter):
        """db_fetch → build_prompt → llm_inference → parse_response (by end time)."""
        _, spans = self._run(db, span_exporter)
        ordered = sorted(spans, key=lambda s: s.end_time)
        names = [s.name for s in ordered]
        assert names.index("db_fetch") < names.index("build_prompt")
        assert names.index("build_prompt") < names.index("llm_inference")
        assert names.index("llm_inference") < names.index("parse_response")


# ---------------------------------------------------------------------------
# OTel — span structure emitted by main()
# ---------------------------------------------------------------------------

class TestObservabilityMain:
    """Verify root span and per-session classify_session spans from main()."""

    def _run_main(self, stdin_payload: dict, db: Path,
                  model: MagicMock | None = None) -> tuple[dict, list]:
        import agents.run_task_linker_mlx as m

        stdin_data = json.dumps({**stdin_payload, "meridian_db": str(db)})
        captured_out = StringIO()
        _model = model or _make_outlines_model()
        patches = _patch_modules(_model)

        with patch.dict(sys.modules, patches), \
             patch.object(m, "_get_model", return_value=_model), \
             patch("sys.stdin", StringIO(stdin_data)), \
             patch("sys.stdout", captured_out), \
             patch.object(m, "_open_run_log",
                          return_value=(Path("/tmp/test.jsonl"), MagicMock())):
            m.main()

        output = json.loads(captured_out.getvalue())
        return output, []  # span_exporter accessed via fixture

    def test_root_span_emitted(self, db: Path, span_exporter):
        import agents.run_task_linker_mlx as m

        model = _make_outlines_model()
        stdin_data = json.dumps({"session_ids": [1], "meridian_db": str(db)})
        with patch.dict(sys.modules, _patch_modules(model)), \
             patch.object(m, "_get_model", return_value=model), \
             patch("sys.stdin", StringIO(stdin_data)), \
             patch("sys.stdout", StringIO()), \
             patch.object(m, "_open_run_log",
                          return_value=(Path("/tmp/test.jsonl"), MagicMock())), \
             patch.object(m.observability, "shutdown"):
            m.main()

        spans = span_exporter.get_finished_spans()
        assert _span_by_name(spans, "run_task_linker_mlx") is not None

    def test_root_span_session_count_attribute(self, db: Path, span_exporter):
        import agents.run_task_linker_mlx as m

        model = _make_outlines_model()
        stdin_data = json.dumps({"session_ids": [1], "meridian_db": str(db)})
        with patch.dict(sys.modules, _patch_modules(model)), \
             patch.object(m, "_get_model", return_value=model), \
             patch("sys.stdin", StringIO(stdin_data)), \
             patch("sys.stdout", StringIO()), \
             patch.object(m, "_open_run_log",
                          return_value=(Path("/tmp/test.jsonl"), MagicMock())), \
             patch.object(m.observability, "shutdown"):
            m.main()

        spans = span_exporter.get_finished_spans()
        root = _span_by_name(spans, "run_task_linker_mlx")
        assert root.attributes["session_count"] == 1

    def test_root_span_db_path_attribute(self, db: Path, span_exporter):
        import agents.run_task_linker_mlx as m

        model = _make_outlines_model()
        stdin_data = json.dumps({"session_ids": [1], "meridian_db": str(db)})
        with patch.dict(sys.modules, _patch_modules(model)), \
             patch.object(m, "_get_model", return_value=model), \
             patch("sys.stdin", StringIO(stdin_data)), \
             patch("sys.stdout", StringIO()), \
             patch.object(m, "_open_run_log",
                          return_value=(Path("/tmp/test.jsonl"), MagicMock())), \
             patch.object(m.observability, "shutdown"):
            m.main()

        spans = span_exporter.get_finished_spans()
        root = _span_by_name(spans, "run_task_linker_mlx")
        assert root.attributes["db_path"] == "meridian.db"

    def test_root_span_results_count_attribute(self, db: Path, span_exporter):
        import agents.run_task_linker_mlx as m

        model = _make_outlines_model()
        stdin_data = json.dumps({"session_ids": [1], "meridian_db": str(db)})
        with patch.dict(sys.modules, _patch_modules(model)), \
             patch.object(m, "_get_model", return_value=model), \
             patch("sys.stdin", StringIO(stdin_data)), \
             patch("sys.stdout", StringIO()), \
             patch.object(m, "_open_run_log",
                          return_value=(Path("/tmp/test.jsonl"), MagicMock())), \
             patch.object(m.observability, "shutdown"):
            m.main()

        spans = span_exporter.get_finished_spans()
        root = _span_by_name(spans, "run_task_linker_mlx")
        assert root.attributes["results_count"] == 1

    def test_classify_session_span_per_session(self, db: Path, span_exporter):
        import agents.run_task_linker_mlx as m

        # Add a second session.
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
        stdin_data = json.dumps({"session_ids": [1, 2], "meridian_db": str(db)})
        with patch.dict(sys.modules, _patch_modules(model)), \
             patch.object(m, "_get_model", return_value=model), \
             patch("sys.stdin", StringIO(stdin_data)), \
             patch("sys.stdout", StringIO()), \
             patch.object(m, "_open_run_log",
                          return_value=(Path("/tmp/test.jsonl"), MagicMock())), \
             patch.object(m.observability, "shutdown"):
            m.main()

        spans = span_exporter.get_finished_spans()
        classify_spans = [s for s in spans if s.name == "classify_session"]
        assert len(classify_spans) == 2

    def test_classify_session_span_attributes(self, db: Path, span_exporter):
        import agents.run_task_linker_mlx as m

        model = _make_outlines_model(_GOOD_JSON)
        stdin_data = json.dumps({"session_ids": [1], "meridian_db": str(db)})
        with patch.dict(sys.modules, _patch_modules(model)), \
             patch.object(m, "_get_model", return_value=model), \
             patch("sys.stdin", StringIO(stdin_data)), \
             patch("sys.stdout", StringIO()), \
             patch.object(m, "_open_run_log",
                          return_value=(Path("/tmp/test.jsonl"), MagicMock())), \
             patch.object(m.observability, "shutdown"):
            m.main()

        spans = span_exporter.get_finished_spans()
        cs = _span_by_name(spans, "classify_session")
        assert cs is not None
        assert cs.attributes["session_id"] == 1
        assert cs.attributes["task_key"] == "KAN-42"
        assert cs.attributes["session_type"] == "task"
        assert cs.attributes["method"] == "mlx_direct"
        assert cs.attributes["elapsed_s"] >= 0.0

    def test_classify_session_is_child_of_root(self, db: Path, span_exporter):
        import agents.run_task_linker_mlx as m

        model = _make_outlines_model()
        stdin_data = json.dumps({"session_ids": [1], "meridian_db": str(db)})
        with patch.dict(sys.modules, _patch_modules(model)), \
             patch.object(m, "_get_model", return_value=model), \
             patch("sys.stdin", StringIO(stdin_data)), \
             patch("sys.stdout", StringIO()), \
             patch.object(m, "_open_run_log",
                          return_value=(Path("/tmp/test.jsonl"), MagicMock())), \
             patch.object(m.observability, "shutdown"):
            m.main()

        spans = span_exporter.get_finished_spans()
        root = _span_by_name(spans, "run_task_linker_mlx")
        cs = _span_by_name(spans, "classify_session")
        assert cs.parent is not None
        assert cs.parent.span_id == root.context.span_id

    def test_db_fetch_is_child_of_classify_session(self, db: Path, span_exporter):
        import agents.run_task_linker_mlx as m

        model = _make_outlines_model()
        stdin_data = json.dumps({"session_ids": [1], "meridian_db": str(db)})
        with patch.dict(sys.modules, _patch_modules(model)), \
             patch.object(m, "_get_model", return_value=model), \
             patch("sys.stdin", StringIO(stdin_data)), \
             patch("sys.stdout", StringIO()), \
             patch.object(m, "_open_run_log",
                          return_value=(Path("/tmp/test.jsonl"), MagicMock())), \
             patch.object(m.observability, "shutdown"):
            m.main()

        spans = span_exporter.get_finished_spans()
        cs = _span_by_name(spans, "classify_session")
        db_span = _span_by_name(spans, "db_fetch")
        assert db_span.parent is not None
        assert db_span.parent.span_id == cs.context.span_id

    def test_traceparent_sets_parent_on_root_span(self, db: Path, span_exporter):
        """A valid W3C traceparent in the payload is reflected as the root span's parent."""
        import agents.run_task_linker_mlx as m

        traceparent = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"
        model = _make_outlines_model()
        stdin_data = json.dumps({
            "session_ids": [1],
            "meridian_db": str(db),
            "traceparent": traceparent,
        })
        with patch.dict(sys.modules, _patch_modules(model)), \
             patch.object(m, "_get_model", return_value=model), \
             patch("sys.stdin", StringIO(stdin_data)), \
             patch("sys.stdout", StringIO()), \
             patch.object(m, "_open_run_log",
                          return_value=(Path("/tmp/test.jsonl"), MagicMock())), \
             patch.object(m.observability, "shutdown"):
            m.main()

        spans = span_exporter.get_finished_spans()
        root = _span_by_name(spans, "run_task_linker_mlx")
        assert root.parent is not None
        # The trace ID from the traceparent header must be carried into the span.
        assert format(root.context.trace_id, "032x") == "4bf92f3577b34da6a3ce929d0e0e4736"

    def test_no_traceparent_creates_fresh_root_span(self, db: Path, span_exporter):
        """Without a traceparent the root span has no parent (fresh trace)."""
        import agents.run_task_linker_mlx as m

        model = _make_outlines_model()
        stdin_data = json.dumps({"session_ids": [1], "meridian_db": str(db)})
        with patch.dict(sys.modules, _patch_modules(model)), \
             patch.object(m, "_get_model", return_value=model), \
             patch("sys.stdin", StringIO(stdin_data)), \
             patch("sys.stdout", StringIO()), \
             patch.object(m, "_open_run_log",
                          return_value=(Path("/tmp/test.jsonl"), MagicMock())), \
             patch.object(m.observability, "shutdown"):
            m.main()

        spans = span_exporter.get_finished_spans()
        root = _span_by_name(spans, "run_task_linker_mlx")
        assert root.parent is None

    def test_shutdown_called_after_root_span(self, db: Path, span_exporter):
        """observability.shutdown() is called to flush spans before process exit."""
        import agents.run_task_linker_mlx as m

        model = _make_outlines_model()
        stdin_data = json.dumps({"session_ids": [1], "meridian_db": str(db)})
        with patch.dict(sys.modules, _patch_modules(model)), \
             patch.object(m, "_get_model", return_value=model), \
             patch("sys.stdin", StringIO(stdin_data)), \
             patch("sys.stdout", StringIO()), \
             patch.object(m, "_open_run_log",
                          return_value=(Path("/tmp/test.jsonl"), MagicMock())), \
             patch.object(m.observability, "shutdown") as mock_shutdown:
            m.main()

        mock_shutdown.assert_called_once()


# ---------------------------------------------------------------------------
# main() — stdin/stdout contract (pre-existing tests preserved)
# ---------------------------------------------------------------------------

class TestMain:
    def _run_main(self, stdin_payload: dict, db: Path) -> dict:
        import agents.run_task_linker_mlx as m

        stdin_data = json.dumps({**stdin_payload, "meridian_db": str(db)})
        captured_out = StringIO()

        with patch("sys.stdin", StringIO(stdin_data)), \
             patch("sys.stdout", captured_out), \
             patch.object(m.observability, "shutdown"):
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
             patch.object(m, "_get_model", return_value=model), \
             patch.object(m, "_open_run_log",
                          return_value=(Path("/tmp/test.jsonl"), MagicMock())):
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
             patch.object(m, "_get_model", return_value=model), \
             patch.object(m, "_open_run_log",
                          return_value=(Path("/tmp/test.jsonl"), MagicMock())):
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
# Idle eviction — model_session() in-flight tracking + maybe_evict_idle()
# (the model holds ~7 GB while resident; the server unloads it when idle)
# ---------------------------------------------------------------------------

class TestModelEviction:
    def test_model_session_loads_and_tracks_in_flight(self):
        import agents.run_task_linker_mlx as m
        sentinel = MagicMock(name="model")
        with patch.object(m, "_get_model", return_value=sentinel):
            m._in_flight = 0
            with m.model_session() as model:
                assert model is sentinel
                assert m._in_flight == 1          # marked in-flight while in use
            assert m._in_flight == 0              # released on exit

    def test_evict_noop_when_not_idle_long_enough(self):
        import agents.run_task_linker_mlx as m
        m._model_cache["x"] = MagicMock()
        m._in_flight = 0
        m._last_used = time.monotonic()           # just used
        assert m.maybe_evict_idle(idle_s=600) is None
        assert m.model_resident() is True

    def test_evict_disabled_when_ttl_zero(self):
        import agents.run_task_linker_mlx as m
        m._model_cache["x"] = MagicMock()
        assert m.maybe_evict_idle(idle_s=0) is None
        assert m.model_resident() is True

    def test_evict_noop_when_in_flight(self):
        import agents.run_task_linker_mlx as m
        m._model_cache["x"] = MagicMock()
        m._in_flight = 1                          # an inference is using the model
        m._last_used = time.monotonic() - 1000
        try:
            assert m.maybe_evict_idle(idle_s=0.001) is None
            assert m.model_resident() is True     # never freed mid-inference
        finally:
            m._in_flight = 0

    def test_evict_clears_cache_when_idle(self):
        import agents.run_task_linker_mlx as m
        m._model_cache["x"] = MagicMock()
        m._in_flight = 0
        m._last_used = time.monotonic() - 1000    # idle long past the window
        freed = m.maybe_evict_idle(idle_s=0.001)
        assert freed is not None                  # eviction happened
        assert m.model_resident() is False
        assert m._model_cache == {}


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

    def test_valid_untracked_classification(self):
        from agents.run_task_linker_mlx import SessionClassification

        obj = SessionClassification.model_validate_json(_UNTRACKED_JSON)
        assert obj.task_key is None
        assert obj.session_type == "untracked"

    def test_invalid_session_type_raises(self):
        from agents.run_task_linker_mlx import SessionClassification
        from pydantic import ValidationError

        bad = json.dumps({
            "task_key": None, "confidence": 0.5,
            "session_type": "bogus", "reasoning": "test", "dimensions": {},
        })
        with pytest.raises(ValidationError):
            SessionClassification.model_validate_json(bad)

    def test_confidence_below_zero_raises(self):
        from agents.run_task_linker_mlx import SessionClassification
        from pydantic import ValidationError

        bad = json.dumps({
            "task_key": None, "confidence": -0.1,
            "session_type": "overhead", "reasoning": "test", "dimensions": {},
        })
        with pytest.raises(ValidationError):
            SessionClassification.model_validate_json(bad)

    def test_confidence_above_one_raises(self):
        from agents.run_task_linker_mlx import SessionClassification
        from pydantic import ValidationError

        bad = json.dumps({
            "task_key": None, "confidence": 1.1,
            "session_type": "overhead", "reasoning": "test", "dimensions": {},
        })
        with pytest.raises(ValidationError):
            SessionClassification.model_validate_json(bad)
