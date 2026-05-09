# meridian — normalises screenpipe activity into structured app sessions

"""Environment-driven configuration for meridian-agents.

Mirrors the conventions of the Rust daemon's `src/config.rs`:
- env vars first, with documented defaults
- `~/` expansion for paths
- `~/.meridian/.env` loaded automatically if present
- validation at the boundary; required vars raise `ConfigError`
"""

from __future__ import annotations

import os
from dataclasses import dataclass
from pathlib import Path

from dotenv import load_dotenv

DEFAULT_MERIDIAN_DB = "~/.meridian/meridian.db"
DEFAULT_DOTENV_PATH = "~/.meridian/.env"
DEFAULT_POLL_INTERVAL_SECS = 300
DEFAULT_AUTO_THRESHOLD = 0.85
DEFAULT_QUEUE_THRESHOLD = 0.60
DEFAULT_LOG_FILTER = "meridian_agents=info"
DEFAULT_OLLAMA_BASE_URL = "https://ollama.com"


class ConfigError(ValueError):
    """Raised when required environment variables are missing or invalid."""


@dataclass(frozen=True)
class JiraConfig:
    base_url: str
    email: str
    api_token: str


@dataclass(frozen=True)
class Config:
    meridian_db: str
    poll_interval_secs: int
    auto_threshold: float
    queue_threshold: float
    log_filter: str
    ollama_base_url: str
    ollama_api_key: str
    ollama_model: str
    jira: JiraConfig | None

    @property
    def jira_enabled(self) -> bool:
        return self.jira is not None

    def meridian_db_uri_ro(self) -> str:
        """SQLite URI for read-only access (mode=ro)."""
        return f"file:{self.meridian_db}?mode=ro"

    def meridian_db_uri_rw(self) -> str:
        """SQLite URI for read-write access; busy_timeout is set on the
        connection rather than via URI (aiosqlite doesn't parse it)."""
        return f"file:{self.meridian_db}?mode=rwc"


def _expand(path: str) -> str:
    return str(Path(os.path.expanduser(path)))


def _read_str(name: str, default: str) -> str:
    value = os.environ.get(name)
    return value if value is not None and value != "" else default


def _require_str(name: str) -> str:
    value = os.environ.get(name)
    if value is None or value == "":
        raise ConfigError(f"{name} is required")
    return value


def _read_int(name: str, default: int) -> int:
    raw = os.environ.get(name)
    if raw is None or raw == "":
        return default
    try:
        return int(raw)
    except ValueError as e:
        raise ConfigError(f"{name} must be an integer, got {raw!r}") from e


def _read_float(name: str, default: float) -> float:
    raw = os.environ.get(name)
    if raw is None or raw == "":
        return default
    try:
        return float(raw)
    except ValueError as e:
        raise ConfigError(f"{name} must be a number, got {raw!r}") from e


def _read_jira() -> JiraConfig | None:
    base_url = os.environ.get("JIRA_BASE_URL")
    email = os.environ.get("JIRA_EMAIL")
    token = os.environ.get("JIRA_API_TOKEN")
    present = [v for v in (base_url, email, token) if v]
    if not present:
        return None
    if len(present) < 3:
        raise ConfigError(
            "JIRA_BASE_URL, JIRA_EMAIL, and JIRA_API_TOKEN must all be set "
            "to enable the Jira sink (got partial config)"
        )
    return JiraConfig(base_url=base_url, email=email, api_token=token)


def load(*, dotenv_path: str | None = None, dotenv_override: bool = False) -> Config:
    """Load configuration from environment, optionally seeded by a .env file.

    By default, attempts to load `~/.meridian/.env` (mirrors the Rust daemon).
    Pass `dotenv_path=""` to skip dotenv loading entirely (useful in tests).
    """
    if dotenv_path is None:
        dotenv_path = DEFAULT_DOTENV_PATH
    if dotenv_path:
        expanded = _expand(dotenv_path)
        if Path(expanded).is_file():
            load_dotenv(expanded, override=dotenv_override)

    meridian_db = _expand(_read_str("MERIDIAN_DB", DEFAULT_MERIDIAN_DB))

    poll_interval_secs = _read_int(
        "MERIDIAN_AGENTS_POLL_INTERVAL_SECS", DEFAULT_POLL_INTERVAL_SECS
    )
    if poll_interval_secs <= 0:
        raise ConfigError(
            f"MERIDIAN_AGENTS_POLL_INTERVAL_SECS must be > 0, got {poll_interval_secs}"
        )

    auto = _read_float("MERIDIAN_AGENTS_AUTO_THRESHOLD", DEFAULT_AUTO_THRESHOLD)
    queue = _read_float("MERIDIAN_AGENTS_QUEUE_THRESHOLD", DEFAULT_QUEUE_THRESHOLD)
    for name, value in (("AUTO", auto), ("QUEUE", queue)):
        if not 0.0 <= value <= 1.0:
            raise ConfigError(
                f"MERIDIAN_AGENTS_{name}_THRESHOLD must be in [0, 1], got {value}"
            )
    if not auto > queue:
        raise ConfigError(
            f"MERIDIAN_AGENTS_AUTO_THRESHOLD ({auto}) must be strictly greater than "
            f"MERIDIAN_AGENTS_QUEUE_THRESHOLD ({queue})"
        )

    return Config(
        meridian_db=meridian_db,
        poll_interval_secs=poll_interval_secs,
        auto_threshold=auto,
        queue_threshold=queue,
        log_filter=_read_str("MERIDIAN_AGENTS_LOG", DEFAULT_LOG_FILTER),
        ollama_base_url=_read_str("OLLAMA_BASE_URL", DEFAULT_OLLAMA_BASE_URL),
        ollama_api_key=_require_str("OLLAMA_API_KEY"),
        ollama_model=_require_str("OLLAMA_MODEL"),
        jira=_read_jira(),
    )
