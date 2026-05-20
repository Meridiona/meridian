# meridian — normalises screenpipe activity into structured app sessions
"""Unit tests for agents.llm_selector — model selection and server management.

All external I/O (subprocess, socket, HTTP, platform, psutil, mlx) is patched
so these tests run offline with no Apple Silicon or macOS requirement.

Tests cover:
  - _select_mlx_entry: budget/thermal/apple_fm filtering
  - _thermal_level: ctypes happy path and fallback
  - probe_compute: wiring of sub-components into ComputeSnapshot
  - select_model_for_hermes: priority ordering, skips, mlx fallback
  - _ensure_mlx_server: PID-file reuse, model-change restart, timeout
"""
from __future__ import annotations

import os
import sys
import time
from pathlib import Path
from unittest.mock import MagicMock, patch, call

import pytest

# Ensure services/ is on the path (mirrors conftest.py bootstrap).
_SERVICES_DIR = Path(__file__).resolve().parent.parent.parent
if str(_SERVICES_DIR) not in sys.path:
    sys.path.insert(0, str(_SERVICES_DIR))


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _make_compute_snapshot(**kw):
    """Build a ComputeSnapshot from agents.llm_selector with sensible defaults."""
    from agents.llm_selector import ComputeSnapshot
    return ComputeSnapshot(
        metal_headroom_gb=kw.get("metal_headroom_gb", 10.0),
        thermal_level=kw.get("thermal_level", 0),
        cpu_pct=kw.get("cpu_pct", 20.0),
        screen_locked=kw.get("screen_locked", False),
        chip_name=kw.get("chip_name", "Apple M3 Pro"),
        mem_bw_gbs=kw.get("mem_bw_gbs", 150),
    )


# ---------------------------------------------------------------------------
# _select_mlx_entry
# ---------------------------------------------------------------------------

class TestSelectMlxEntry:
    """Tests for _select_mlx_entry — the pure budget/thermal/apple_fm filter."""

    def test_picks_largest_fitting_model(self):
        """headroom=25 GB, budget_pct=1.0, thermal=0 → qwen3.6-35b-moe (21 GB) wins.

        llama3.3-70b and r1-70b each need 40 GB, so they are skipped.
        qwen3.6-35b-moe needs 21 GB — the first one that fits.
        """
        from agents.llm_selector import _select_mlx_entry
        entry = _select_mlx_entry(25.0, 1.0, 0, False)
        assert entry is not None
        model_id = entry[0]
        assert model_id == "qwen3.6-35b-moe"

    def test_skips_apple_fm_when_flag_false(self):
        """headroom=0.5 GB, apple_intelligence=False → None.

        apple-intelligence has min_ram=0 but its backend is apple_fm, which is
        filtered out when apple_intelligence=False.  Nothing else fits 0.5 GB.
        """
        from agents.llm_selector import _select_mlx_entry
        entry = _select_mlx_entry(0.5, 1.0, 0, False)
        assert entry is None

    def test_caps_budget_under_thermal_pressure(self):
        """headroom=20 GB, budget_pct=1.0, thermal=2 → budget capped to 9 GB.

        At thermal level 2 the budget is capped at 9 GB regardless of headroom.
        phi-4 (8.5 GB) and r1-14b (8.5 GB) both fit; the first one encountered
        in the ordered catalog wins.
        """
        from agents.llm_selector import _select_mlx_entry
        entry = _select_mlx_entry(20.0, 1.0, 2, False)
        assert entry is not None
        model_id = entry[0]
        assert model_id in ("phi-4", "r1-14b")

    def test_returns_none_when_nothing_fits(self):
        """headroom=1.0 GB, budget_pct=0.5 → effective budget=0.5 GB → no model fits."""
        from agents.llm_selector import _select_mlx_entry
        entry = _select_mlx_entry(1.0, 0.5, 0, False)
        assert entry is None

    def test_returns_apple_fm_entry_when_flag_true_and_nothing_else_fits(self):
        """headroom=0.5 GB, apple_intelligence=True → apple-intelligence is returned."""
        from agents.llm_selector import _select_mlx_entry
        entry = _select_mlx_entry(0.5, 1.0, 0, True)
        assert entry is not None
        assert entry[0] == "apple-intelligence"
        assert entry[1] == "apple_fm"


# ---------------------------------------------------------------------------
# _thermal_level
# ---------------------------------------------------------------------------

class TestThermalLevel:
    """Tests for _thermal_level — reads macOS libnotify without root."""

    def test_returns_int_from_libnotify(self):
        """When ctypes succeeds, returns the raw integer state value."""
        import ctypes
        from agents.llm_selector import _thermal_level

        mock_lib = MagicMock()
        mock_state = ctypes.c_uint64(2)

        def fake_get_state(tok, byref_st):
            # Simulate writing level=2 into the c_uint64 reference.
            byref_st._obj.value = 2

        mock_lib.notify_get_state.side_effect = fake_get_state

        with patch("ctypes.cdll.LoadLibrary", return_value=mock_lib):
            level = _thermal_level()

        assert isinstance(level, int)

    def test_returns_zero_on_exception(self):
        """When LoadLibrary raises, _thermal_level falls back to 0."""
        from agents.llm_selector import _thermal_level

        with patch("ctypes.cdll.LoadLibrary", side_effect=OSError("no lib")):
            assert _thermal_level() == 0


# ---------------------------------------------------------------------------
# probe_compute
# ---------------------------------------------------------------------------

class TestProbeCompute:
    """Tests for probe_compute — assembles ComputeSnapshot from sub-calls."""

    def test_returns_compute_snapshot(self):
        """probe_compute returns a ComputeSnapshot with correct chip wiring."""
        from agents.llm_selector import probe_compute

        mock_psutil = MagicMock()
        mock_psutil.cpu_percent.return_value = 30.0

        with (
            patch("agents.llm_selector._metal_headroom_gb", return_value=12.5),
            patch("agents.llm_selector._thermal_level", return_value=1),
            patch("agents.llm_selector._screen_locked", return_value=False),
            patch("agents.llm_selector._sysctl", return_value="Apple M3 Pro"),
            patch.dict("sys.modules", {"psutil": mock_psutil}),
        ):
            snap = probe_compute()

        assert snap.metal_headroom_gb == 12.5
        assert snap.thermal_level == 1
        assert snap.cpu_pct == 30.0
        assert snap.screen_locked is False
        assert snap.chip_name == "Apple M3 Pro"
        assert snap.mem_bw_gbs == 150  # from _CHIP_SPECS["apple m3 pro"]

    def test_mem_bw_zero_for_unknown_chip(self):
        """Chips not in _CHIP_SPECS get mem_bw_gbs=0."""
        from agents.llm_selector import probe_compute

        mock_psutil = MagicMock()
        mock_psutil.cpu_percent.return_value = 10.0

        with (
            patch("agents.llm_selector._metal_headroom_gb", return_value=5.0),
            patch("agents.llm_selector._thermal_level", return_value=0),
            patch("agents.llm_selector._screen_locked", return_value=False),
            patch("agents.llm_selector._sysctl", return_value="Intel Core i9"),
            patch.dict("sys.modules", {"psutil": mock_psutil}),
        ):
            snap = probe_compute()

        assert snap.mem_bw_gbs == 0


# ---------------------------------------------------------------------------
# select_model_for_hermes  (new function — tested against spec)
# ---------------------------------------------------------------------------

class TestSelectModelForHermes:
    """Tests for select_model_for_hermes — priority-ordered endpoint selection."""

    def test_returns_none_on_non_apple(self):
        """Non-Darwin platforms immediately return None."""
        from agents.llm_selector import select_model_for_hermes

        with patch("platform.system", return_value="Linux"):
            result = select_model_for_hermes()

        assert result is None

    def test_returns_none_on_non_apple_silicon(self):
        """Darwin but non-Apple-Silicon chip also returns None."""
        from agents.llm_selector import select_model_for_hermes

        with (
            patch("platform.system", return_value="Darwin"),
            patch("agents.llm_selector._sysctl", return_value="Intel Core i9"),
        ):
            result = select_model_for_hermes()

        assert result is None

    def test_uses_running_ollama_server(self):
        """A running Ollama server is returned as LocalModelEndpoint immediately."""
        from agents.llm_selector import select_model_for_hermes, RunningServer

        server = RunningServer(
            runtime="ollama",
            base_url="http://127.0.0.1:11434/v1",
            loaded_models=["llama3:8b"],
            best_model="llama3:8b",
        )

        with (
            patch("platform.system", return_value="Darwin"),
            patch("agents.llm_selector._sysctl", return_value="Apple M3 Pro"),
            patch("agents.llm_selector.discover_running_servers", return_value=[server]),
        ):
            result = select_model_for_hermes()

        assert result is not None
        assert result.model == "llama3:8b"
        assert result.base_url == "http://127.0.0.1:11434/v1"
        assert result.runtime == "ollama"

    def test_skips_apple_fm_runtime(self):
        """apple_fm runtime servers are skipped; falls through to probe_compute."""
        from agents.llm_selector import select_model_for_hermes, RunningServer

        apple_server = RunningServer(
            runtime="apple_fm",
            base_url="",
            loaded_models=["apple-intelligence"],
            best_model="apple-intelligence",
        )

        with (
            patch("platform.system", return_value="Darwin"),
            patch("agents.llm_selector._sysctl", return_value="Apple M3 Pro"),
            patch("agents.llm_selector.discover_running_servers", return_value=[apple_server]),
            patch("agents.llm_selector.probe_compute", side_effect=Exception("forced")),
        ):
            result = select_model_for_hermes()

        assert result is None

    def test_starts_mlx_server_when_no_server_running(self):
        """With no running servers, selects MLX model and calls _ensure_mlx_server."""
        from agents.llm_selector import select_model_for_hermes

        snap = _make_compute_snapshot(metal_headroom_gb=10.0, thermal_level=0)

        with (
            patch("platform.system", return_value="Darwin"),
            patch("agents.llm_selector._sysctl", return_value="Apple M3 Pro"),
            patch("agents.llm_selector.discover_running_servers", return_value=[]),
            patch("agents.llm_selector.probe_compute", return_value=snap),
            patch("agents.llm_selector._ensure_mlx_server", return_value=True),
        ):
            result = select_model_for_hermes(budget_pct=1.0)

        assert result is not None
        assert result.runtime == "mlx_managed"
        assert result.base_url == "http://127.0.0.1:8765/v1"

    def test_returns_none_when_server_fails_to_start(self):
        """When _ensure_mlx_server returns False, select_model_for_hermes returns None."""
        from agents.llm_selector import select_model_for_hermes

        snap = _make_compute_snapshot(metal_headroom_gb=10.0, thermal_level=0)

        with (
            patch("platform.system", return_value="Darwin"),
            patch("agents.llm_selector._sysctl", return_value="Apple M3 Pro"),
            patch("agents.llm_selector.discover_running_servers", return_value=[]),
            patch("agents.llm_selector.probe_compute", return_value=snap),
            patch("agents.llm_selector._ensure_mlx_server", return_value=False),
        ):
            result = select_model_for_hermes(budget_pct=1.0)

        assert result is None

    def test_returns_none_when_no_model_fits_budget(self):
        """Tiny headroom means no model fits; returns None without calling _ensure_mlx_server."""
        from agents.llm_selector import select_model_for_hermes

        snap = _make_compute_snapshot(metal_headroom_gb=0.3, thermal_level=0)

        with (
            patch("platform.system", return_value="Darwin"),
            patch("agents.llm_selector._sysctl", return_value="Apple M3 Pro"),
            patch("agents.llm_selector.discover_running_servers", return_value=[]),
            patch("agents.llm_selector.probe_compute", return_value=snap),
            patch("agents.llm_selector._ensure_mlx_server") as mock_ensure,
        ):
            result = select_model_for_hermes(budget_pct=0.5)

        assert result is None
        mock_ensure.assert_not_called()


# ---------------------------------------------------------------------------
# _ensure_mlx_server
# ---------------------------------------------------------------------------

class TestEnsureMlxServer:
    """Tests for _ensure_mlx_server — PID-file lifecycle and timeout."""

    def test_returns_true_when_already_running(self, tmp_path, monkeypatch):
        """If PID file exists with current PID and same model, skips launch and returns True."""
        from agents.llm_selector import _ensure_mlx_server

        pid_file = tmp_path / "mlx_server.pid"
        model_id = "mlx-community/phi-4-4bit"
        current_pid = os.getpid()
        pid_file.write_text(f"{current_pid}\n{model_id}\n")

        monkeypatch.setattr(
            "agents.llm_selector._MLX_PID_FILE",
            str(pid_file),
            raising=False,
        )

        with patch(
            "agents.llm_selector._get_json",
            return_value=({"data": [{"id": model_id}]}, 200),
        ):
            result = _ensure_mlx_server(model_id, port=8765)

        assert result is True

    def test_returns_false_on_timeout(self, tmp_path, monkeypatch):
        """When /v1/models never responds, _ensure_mlx_server returns False after timeout."""
        from agents.llm_selector import _ensure_mlx_server

        pid_file = tmp_path / "mlx_server.pid"
        monkeypatch.setattr(
            "agents.llm_selector._MLX_PID_FILE",
            str(pid_file),
            raising=False,
        )
        # Cap the timeout constant so the test does not actually wait.
        monkeypatch.setattr(
            "agents.llm_selector._MLX_START_TIMEOUT_S",
            2,
            raising=False,
        )

        mock_proc = MagicMock()
        mock_proc.pid = 99999

        with (
            patch("agents.llm_selector._get_json", return_value=(None, None)),
            patch("subprocess.Popen", return_value=mock_proc),
            patch("time.sleep"),
        ):
            result = _ensure_mlx_server(
                "mlx-community/Llama-3.2-3B-Instruct-4bit", port=8765
            )

        assert result is False
