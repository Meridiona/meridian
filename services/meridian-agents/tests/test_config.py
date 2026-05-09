# meridian — normalises screenpipe activity into structured app sessions

"""Unit tests for meridian_agents.config."""

from __future__ import annotations

from dataclasses import FrozenInstanceError

import pytest

from meridian_agents.config import (
    DEFAULT_AUTO_THRESHOLD,
    DEFAULT_OLLAMA_BASE_URL,
    DEFAULT_POLL_INTERVAL_SECS,
    DEFAULT_QUEUE_THRESHOLD,
    ConfigError,
    JiraConfig,
    load,
)


@pytest.fixture(autouse=True)
def _clean_env(monkeypatch):
    """Strip every env var the loader inspects so tests run in isolation.

    monkeypatch.delenv with raising=False is a no-op when the var is absent,
    so this also handles the case where the developer has these set locally.
    """
    for var in (
        "MERIDIAN_DB",
        "MERIDIAN_AGENTS_POLL_INTERVAL_SECS",
        "MERIDIAN_AGENTS_AUTO_THRESHOLD",
        "MERIDIAN_AGENTS_QUEUE_THRESHOLD",
        "MERIDIAN_AGENTS_LOG",
        "OLLAMA_BASE_URL",
        "OLLAMA_API_KEY",
        "OLLAMA_MODEL",
        "JIRA_BASE_URL",
        "JIRA_EMAIL",
        "JIRA_API_TOKEN",
    ):
        monkeypatch.delenv(var, raising=False)


def _set_required(monkeypatch):
    monkeypatch.setenv("OLLAMA_API_KEY", "test-key")
    monkeypatch.setenv("OLLAMA_MODEL", "test-model")


# ---------------------------------------------------------------------------
# Defaults
# ---------------------------------------------------------------------------


def test_load_with_only_required_uses_defaults(monkeypatch):
    _set_required(monkeypatch)
    cfg = load(dotenv_path="")  # skip ~/.meridian/.env in tests
    assert cfg.poll_interval_secs == DEFAULT_POLL_INTERVAL_SECS
    assert cfg.auto_threshold == DEFAULT_AUTO_THRESHOLD
    assert cfg.queue_threshold == DEFAULT_QUEUE_THRESHOLD
    assert cfg.ollama_base_url == DEFAULT_OLLAMA_BASE_URL
    assert cfg.jira is None
    assert cfg.jira_enabled is False


def test_default_meridian_db_path_expands_tilde(monkeypatch):
    _set_required(monkeypatch)
    cfg = load(dotenv_path="")
    assert "~" not in cfg.meridian_db, "~ must be expanded"
    assert cfg.meridian_db.endswith(".meridian/meridian.db")


# ---------------------------------------------------------------------------
# Required vars
# ---------------------------------------------------------------------------


def test_missing_ollama_api_key_raises(monkeypatch):
    monkeypatch.setenv("OLLAMA_MODEL", "m")
    with pytest.raises(ConfigError, match="OLLAMA_API_KEY"):
        load(dotenv_path="")


def test_missing_ollama_model_raises(monkeypatch):
    monkeypatch.setenv("OLLAMA_API_KEY", "k")
    with pytest.raises(ConfigError, match="OLLAMA_MODEL"):
        load(dotenv_path="")


def test_empty_string_treated_as_missing(monkeypatch):
    _set_required(monkeypatch)
    monkeypatch.setenv("OLLAMA_API_KEY", "")
    with pytest.raises(ConfigError, match="OLLAMA_API_KEY"):
        load(dotenv_path="")


# ---------------------------------------------------------------------------
# Overrides + parsing
# ---------------------------------------------------------------------------


def test_meridian_db_override(monkeypatch, tmp_path):
    _set_required(monkeypatch)
    target = tmp_path / "custom.db"
    monkeypatch.setenv("MERIDIAN_DB", str(target))
    cfg = load(dotenv_path="")
    assert cfg.meridian_db == str(target)


def test_poll_interval_override(monkeypatch):
    _set_required(monkeypatch)
    monkeypatch.setenv("MERIDIAN_AGENTS_POLL_INTERVAL_SECS", "120")
    cfg = load(dotenv_path="")
    assert cfg.poll_interval_secs == 120


def test_poll_interval_invalid_int_raises(monkeypatch):
    _set_required(monkeypatch)
    monkeypatch.setenv("MERIDIAN_AGENTS_POLL_INTERVAL_SECS", "not-a-number")
    with pytest.raises(ConfigError, match="must be an integer"):
        load(dotenv_path="")


def test_poll_interval_zero_raises(monkeypatch):
    _set_required(monkeypatch)
    monkeypatch.setenv("MERIDIAN_AGENTS_POLL_INTERVAL_SECS", "0")
    with pytest.raises(ConfigError, match="must be > 0"):
        load(dotenv_path="")


def test_threshold_overrides(monkeypatch):
    _set_required(monkeypatch)
    monkeypatch.setenv("MERIDIAN_AGENTS_AUTO_THRESHOLD", "0.9")
    monkeypatch.setenv("MERIDIAN_AGENTS_QUEUE_THRESHOLD", "0.5")
    cfg = load(dotenv_path="")
    assert cfg.auto_threshold == 0.9
    assert cfg.queue_threshold == 0.5


# ---------------------------------------------------------------------------
# Threshold invariants
# ---------------------------------------------------------------------------


def test_auto_threshold_above_one_raises(monkeypatch):
    _set_required(monkeypatch)
    monkeypatch.setenv("MERIDIAN_AGENTS_AUTO_THRESHOLD", "1.5")
    with pytest.raises(ConfigError, match="AUTO_THRESHOLD must be in"):
        load(dotenv_path="")


def test_queue_threshold_negative_raises(monkeypatch):
    _set_required(monkeypatch)
    monkeypatch.setenv("MERIDIAN_AGENTS_QUEUE_THRESHOLD", "-0.1")
    with pytest.raises(ConfigError, match="QUEUE_THRESHOLD must be in"):
        load(dotenv_path="")


def test_auto_must_exceed_queue(monkeypatch):
    _set_required(monkeypatch)
    monkeypatch.setenv("MERIDIAN_AGENTS_AUTO_THRESHOLD", "0.5")
    monkeypatch.setenv("MERIDIAN_AGENTS_QUEUE_THRESHOLD", "0.5")
    with pytest.raises(ConfigError, match="strictly greater"):
        load(dotenv_path="")


def test_auto_below_queue_raises(monkeypatch):
    _set_required(monkeypatch)
    monkeypatch.setenv("MERIDIAN_AGENTS_AUTO_THRESHOLD", "0.4")
    monkeypatch.setenv("MERIDIAN_AGENTS_QUEUE_THRESHOLD", "0.7")
    with pytest.raises(ConfigError, match="strictly greater"):
        load(dotenv_path="")


# ---------------------------------------------------------------------------
# Jira sink toggle
# ---------------------------------------------------------------------------


def test_jira_disabled_when_no_vars_set(monkeypatch):
    _set_required(monkeypatch)
    cfg = load(dotenv_path="")
    assert cfg.jira is None
    assert not cfg.jira_enabled


def test_jira_enabled_when_all_three_set(monkeypatch):
    _set_required(monkeypatch)
    monkeypatch.setenv("JIRA_BASE_URL", "https://example.atlassian.net")
    monkeypatch.setenv("JIRA_EMAIL", "you@example.com")
    monkeypatch.setenv("JIRA_API_TOKEN", "token")
    cfg = load(dotenv_path="")
    assert cfg.jira == JiraConfig(
        base_url="https://example.atlassian.net",
        email="you@example.com",
        api_token="token",
    )
    assert cfg.jira_enabled


def test_jira_partial_config_raises(monkeypatch):
    _set_required(monkeypatch)
    monkeypatch.setenv("JIRA_BASE_URL", "https://example.atlassian.net")
    monkeypatch.setenv("JIRA_EMAIL", "you@example.com")
    # JIRA_API_TOKEN intentionally missing
    with pytest.raises(ConfigError, match="all be set"):
        load(dotenv_path="")


# ---------------------------------------------------------------------------
# URI helpers
# ---------------------------------------------------------------------------


def test_meridian_db_uri_ro_appends_mode(monkeypatch, tmp_path):
    _set_required(monkeypatch)
    target = tmp_path / "x.db"
    monkeypatch.setenv("MERIDIAN_DB", str(target))
    cfg = load(dotenv_path="")
    assert cfg.meridian_db_uri_ro() == f"file:{target}?mode=ro"
    assert cfg.meridian_db_uri_rw() == f"file:{target}?mode=rwc"


# ---------------------------------------------------------------------------
# Dotenv loading
# ---------------------------------------------------------------------------


def test_dotenv_seeds_env_when_var_unset(monkeypatch, tmp_path):
    env_file = tmp_path / ".env"
    env_file.write_text(
        "OLLAMA_API_KEY=from-dotenv\nOLLAMA_MODEL=from-dotenv-model\n"
    )
    cfg = load(dotenv_path=str(env_file))
    assert cfg.ollama_api_key == "from-dotenv"
    assert cfg.ollama_model == "from-dotenv-model"


def test_dotenv_does_not_override_existing_by_default(monkeypatch, tmp_path):
    monkeypatch.setenv("OLLAMA_API_KEY", "from-shell")
    monkeypatch.setenv("OLLAMA_MODEL", "from-shell-model")
    env_file = tmp_path / ".env"
    env_file.write_text("OLLAMA_API_KEY=from-dotenv\n")
    cfg = load(dotenv_path=str(env_file))
    assert cfg.ollama_api_key == "from-shell"


def test_dotenv_override_flag_wins(monkeypatch, tmp_path):
    monkeypatch.setenv("OLLAMA_API_KEY", "from-shell")
    monkeypatch.setenv("OLLAMA_MODEL", "from-shell-model")
    env_file = tmp_path / ".env"
    env_file.write_text("OLLAMA_API_KEY=from-dotenv\n")
    cfg = load(dotenv_path=str(env_file), dotenv_override=True)
    assert cfg.ollama_api_key == "from-dotenv"


def test_missing_dotenv_file_is_silent(monkeypatch, tmp_path):
    _set_required(monkeypatch)
    nonexistent = tmp_path / "nope.env"
    cfg = load(dotenv_path=str(nonexistent))
    assert cfg.ollama_api_key == "test-key"


# ---------------------------------------------------------------------------
# Frozen invariant
# ---------------------------------------------------------------------------


def test_config_is_frozen(monkeypatch):
    _set_required(monkeypatch)
    cfg = load(dotenv_path="")
    with pytest.raises(FrozenInstanceError):
        cfg.poll_interval_secs = 999  # type: ignore[misc]
