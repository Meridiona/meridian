# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
"""Unit tests for agents.config helpers (_env_bool)."""
from __future__ import annotations

import pytest

from agents import config as cfg


# ─────────────────────── _env_bool ────────────────────────────────────────────
@pytest.mark.parametrize("raw", ["1", "true", "TRUE", "yes", "Yes", "on", "ON"])
def test_env_bool_truthy_strings(monkeypatch, raw):
    """"1"/"true"/"yes"/"on" (any case) resolve to True regardless of default."""
    monkeypatch.setenv("MERIDIAN_TEST_FLAG", raw)
    assert cfg._env_bool("MERIDIAN_TEST_FLAG", False) is True
    assert cfg._env_bool("MERIDIAN_TEST_FLAG", True) is True


@pytest.mark.parametrize("raw", ["0", "false", "FALSE", "no", "off", ""])
def test_env_bool_falsey_strings(monkeypatch, raw):
    """"0"/"false"/"no"/"off"/empty resolve to False regardless of default."""
    monkeypatch.setenv("MERIDIAN_TEST_FLAG", raw)
    assert cfg._env_bool("MERIDIAN_TEST_FLAG", True) is False
    assert cfg._env_bool("MERIDIAN_TEST_FLAG", False) is False


def test_env_bool_missing_uses_default(monkeypatch):
    """When the env var is unset, the supplied default applies."""
    monkeypatch.delenv("MERIDIAN_TEST_FLAG", raising=False)
    assert cfg._env_bool("MERIDIAN_TEST_FLAG", True) is True
    assert cfg._env_bool("MERIDIAN_TEST_FLAG", False) is False


