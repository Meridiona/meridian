"""Tests for select_mlx_model_id in llm_selector.py.

Covers the three-stage selection logic with focus on the low-RAM stage 3 fix:
previously the function returned the oversized preferred model unconditionally
when nothing cached fit; it now picks the largest catalog model that fits.
"""
import sys
from pathlib import Path
from unittest.mock import MagicMock, patch

sys.path.insert(0, str(Path(__file__).parent.parent))


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _make_snap(headroom_gb: float, thermal_level: int = 0,
               screen_locked: bool = False) -> MagicMock:
    snap = MagicMock()
    snap.metal_headroom_gb = headroom_gb
    snap.thermal_level = thermal_level
    snap.screen_locked = screen_locked
    return snap


# ---------------------------------------------------------------------------
# Stage 1 — preferred fits
# ---------------------------------------------------------------------------

class TestStage1PreferredFits:
    def test_returns_preferred_when_budget_allows(self):
        """Stage 1: preferred model returned unchanged when budget covers it.

        14.0 GB headroom × 0.5 = 7.0 GB budget ≥ 6.5 GB min_ram → stage 1 fires.
        """
        snap = _make_snap(headroom_gb=14.0)

        with (
            patch("agents.llm_selector.platform") as mock_platform,
            patch("agents.llm_selector._sysctl", return_value="Apple M2 Pro"),
            patch("agents.llm_selector._apple_intelligence_available", return_value=False),
            patch("agents.llm_selector.probe_compute", return_value=snap),
        ):
            mock_platform.system.return_value = "Darwin"
            from agents.llm_selector import select_mlx_model_id
            result = select_mlx_model_id(
                preferred_hf_id="mlx-community/Qwen3.5-9B-OptiQ-4bit",
                preferred_min_ram_gb=6.5,
                budget_pct=0.5,
            )

        assert result == "mlx-community/Qwen3.5-9B-OptiQ-4bit"

    def test_returns_preferred_on_large_machine(self):
        """Stage 1: 64 GB machine, preferred (6.5 GB) trivially fits."""
        snap = _make_snap(headroom_gb=50.0)

        with (
            patch("agents.llm_selector.platform") as mock_platform,
            patch("agents.llm_selector._sysctl", return_value="Apple M2 Ultra"),
            patch("agents.llm_selector._apple_intelligence_available", return_value=False),
            patch("agents.llm_selector.probe_compute", return_value=snap),
        ):
            mock_platform.system.return_value = "Darwin"
            from agents.llm_selector import select_mlx_model_id
            result = select_mlx_model_id(
                preferred_hf_id="mlx-community/Qwen3.5-9B-OptiQ-4bit",
                preferred_min_ram_gb=6.5,
                budget_pct=0.5,
            )

        # 50.0 × 0.5 = 25 GB budget; 6.5 ≤ 25 → stage 1 fires
        assert result == "mlx-community/Qwen3.5-9B-OptiQ-4bit"


# ---------------------------------------------------------------------------
# Stage 2 — catalog cached fit
# ---------------------------------------------------------------------------

class TestStage2CachedFit:
    def test_returns_largest_cached_model_when_preferred_too_big(self):
        """Stage 2: preferred doesn't fit but a smaller cached model does."""
        snap = _make_snap(headroom_gb=5.5)  # budget = 5.5 × 0.5 = 2.75 GB

        def cached_side_effect(hf_id):
            # Only qwen3.5-4b (2.5 GB) is cached
            return hf_id == "mlx-community/Qwen3.5-4B-MLX-4bit"

        with (
            patch("agents.llm_selector.platform") as mock_platform,
            patch("agents.llm_selector._sysctl", return_value="Apple M1 Air"),
            patch("agents.llm_selector._apple_intelligence_available", return_value=False),
            patch("agents.llm_selector.probe_compute", return_value=snap),
            patch("agents.llm_selector._hf_model_cached",
                  side_effect=cached_side_effect),
        ):
            mock_platform.system.return_value = "Darwin"
            from agents.llm_selector import select_mlx_model_id
            result = select_mlx_model_id(
                preferred_hf_id="mlx-community/Qwen3.5-9B-OptiQ-4bit",
                preferred_min_ram_gb=6.5,
                budget_pct=0.5,
            )

        assert result == "mlx-community/Qwen3.5-4B-MLX-4bit"

    def test_apple_intelligence_returned_when_available(self):
        """Stage 2 (apple_fm branch): Apple Intelligence chosen over mlx on macOS 26+."""
        snap = _make_snap(headroom_gb=5.5)

        with (
            patch("agents.llm_selector.platform") as mock_platform,
            patch("agents.llm_selector._sysctl", return_value="Apple M1 Air"),
            patch("agents.llm_selector._apple_intelligence_available", return_value=True),
            patch("agents.llm_selector.probe_compute", return_value=snap),
            patch("agents.llm_selector._hf_model_cached", return_value=False),
        ):
            mock_platform.system.return_value = "Darwin"
            from agents.llm_selector import select_mlx_model_id, APPLE_INTELLIGENCE_ID
            result = select_mlx_model_id(
                preferred_hf_id="mlx-community/Qwen3.5-9B-OptiQ-4bit",
                preferred_min_ram_gb=6.5,
                budget_pct=0.5,
            )

        assert result == APPLE_INTELLIGENCE_ID


# ---------------------------------------------------------------------------
# Stage 3 — the fixed low-RAM fallback
# ---------------------------------------------------------------------------

class TestStage3LowRamFallback:
    def test_returns_fitting_catalog_model_not_preferred_on_low_ram(self):
        """Stage 3 fix: M1 Air 8 GB — nothing cached, preferred (6.5 GB) doesn't
        fit the 2.7 GB budget — must return a smaller catalog model, NOT preferred.
        """
        snap = _make_snap(headroom_gb=5.4)  # Metal headroom ≈ 5.4 GB on M1 Air 8 GB
        # budget = 5.4 × 0.5 = 2.7 GB → llama3.2-3b (1.8 GB) fits, qwen3.5-4b (2.5 GB) fits

        with (
            patch("agents.llm_selector.platform") as mock_platform,
            patch("agents.llm_selector._sysctl", return_value="Apple M1"),
            patch("agents.llm_selector._apple_intelligence_available", return_value=False),
            patch("agents.llm_selector.probe_compute", return_value=snap),
            patch("agents.llm_selector._hf_model_cached", return_value=False),
        ):
            mock_platform.system.return_value = "Darwin"
            from agents.llm_selector import select_mlx_model_id
            result = select_mlx_model_id(
                preferred_hf_id="mlx-community/Qwen3.5-9B-OptiQ-4bit",
                preferred_min_ram_gb=6.5,
                budget_pct=0.5,
            )

        # Must NOT return the oversized preferred model
        assert result != "mlx-community/Qwen3.5-9B-OptiQ-4bit", (
            "Stage 3 returned the oversized preferred model on a low-RAM machine"
        )
        # Must return a model that fits the budget (largest fitting = qwen3.5-4b at 2.5 GB)
        assert result == "mlx-community/Qwen3.5-4B-MLX-4bit"

    def test_preferred_returned_only_when_nothing_in_catalog_fits(self):
        """Stage 3 true last resort: budget so tiny no catalog model fits."""
        snap = _make_snap(headroom_gb=0.5)  # budget = 0.25 GB — nothing fits

        with (
            patch("agents.llm_selector.platform") as mock_platform,
            patch("agents.llm_selector._sysctl", return_value="Apple M1"),
            patch("agents.llm_selector._apple_intelligence_available", return_value=False),
            patch("agents.llm_selector.probe_compute", return_value=snap),
            patch("agents.llm_selector._hf_model_cached", return_value=False),
        ):
            mock_platform.system.return_value = "Darwin"
            from agents.llm_selector import select_mlx_model_id
            result = select_mlx_model_id(
                preferred_hf_id="mlx-community/Qwen3.5-9B-OptiQ-4bit",
                preferred_min_ram_gb=6.5,
                budget_pct=0.5,
            )

        # Nothing fits → last-resort returns preferred (preserves old behaviour)
        assert result == "mlx-community/Qwen3.5-9B-OptiQ-4bit"

    def test_returns_none_when_no_preferred_and_nothing_fits(self):
        """Stage 3: no preferred given + nothing fits → None."""
        snap = _make_snap(headroom_gb=0.5)

        with (
            patch("agents.llm_selector.platform") as mock_platform,
            patch("agents.llm_selector._sysctl", return_value="Apple M1"),
            patch("agents.llm_selector._apple_intelligence_available", return_value=False),
            patch("agents.llm_selector.probe_compute", return_value=snap),
            patch("agents.llm_selector._hf_model_cached", return_value=False),
        ):
            mock_platform.system.return_value = "Darwin"
            from agents.llm_selector import select_mlx_model_id
            result = select_mlx_model_id(budget_pct=0.5)

        assert result is None

    def test_picks_largest_fitting_not_smallest(self):
        """Stage 3: when multiple catalog models fit, the largest (highest quality) wins."""
        snap = _make_snap(headroom_gb=7.0)  # budget = 3.5 GB
        # qwen3.5-4b (2.5 GB) and llama3.2-3b (1.8 GB) both fit; qwen3.5-4b should win

        with (
            patch("agents.llm_selector.platform") as mock_platform,
            patch("agents.llm_selector._sysctl", return_value="Apple M1 Pro"),
            patch("agents.llm_selector._apple_intelligence_available", return_value=False),
            patch("agents.llm_selector.probe_compute", return_value=snap),
            patch("agents.llm_selector._hf_model_cached", return_value=False),
        ):
            mock_platform.system.return_value = "Darwin"
            from agents.llm_selector import select_mlx_model_id
            result = select_mlx_model_id(
                preferred_hf_id="mlx-community/Qwen3.5-9B-OptiQ-4bit",
                preferred_min_ram_gb=6.5,
                budget_pct=0.5,
            )

        assert result == "mlx-community/Qwen3.5-4B-MLX-4bit"
