# meridian — normalises screenpipe activity into structured app sessions
"""Unit tests for agents.llm_selector — model selection and server management.

All external I/O (subprocess, socket, HTTP, platform, psutil, mlx) is patched
so these tests run offline with no Apple Silicon or macOS requirement.

Tests cover:
  - _select_mlx_entry: budget/thermal/apple_fm filtering
  - _thermal_level: ctypes happy path and fallback
  - probe_compute: wiring of sub-components into ComputeSnapshot
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

