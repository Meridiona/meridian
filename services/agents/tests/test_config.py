# meridian — normalises screenpipe activity into structured app sessions
"""Unit tests for stage-flag resolution in agents.config."""
from __future__ import annotations

import itertools
import json

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


# ─────────────────────── default_stages ───────────────────────────────────────
@pytest.mark.parametrize(
    "s1,s2,s3,expected",
    [
        (False, False, False, set()),
        (True,  False, False, {1}),
        (False, True,  False, {2}),
        (False, False, True,  {3}),
        (True,  True,  False, {1, 2}),
        (True,  False, True,  {1, 3}),
        (False, True,  True,  {2, 3}),
        (True,  True,  True,  {1, 2, 3}),
    ],
)
def test_default_stages_for_all_combinations(monkeypatch, s1, s2, s3, expected):
    """Each of the 8 STAGE{1,2,3}_ENABLED combinations resolves correctly."""
    monkeypatch.setattr(cfg, "STAGE1_ENABLED", s1)
    monkeypatch.setattr(cfg, "STAGE2_ENABLED", s2)
    monkeypatch.setattr(cfg, "STAGE3_ENABLED", s3)
    assert cfg.default_stages() == expected


# ─────────────────────── stages_from_file / write / clear ────────────────────
def test_stages_from_file_returns_none_when_absent(monkeypatch, tmp_path):
    """No override file → stages_from_file returns None (env defaults rule)."""
    monkeypatch.setattr(cfg, "TAGGER_CONFIG_FILE", tmp_path / "absent.json")
    assert cfg.stages_from_file() is None


def test_stages_from_file_reads_present_file(monkeypatch, tmp_path):
    """A valid override file is parsed into a stage set."""
    path = tmp_path / "tagger.config.json"
    path.write_text(json.dumps({"stage1": True, "stage2": False, "stage3": "yes"}))
    monkeypatch.setattr(cfg, "TAGGER_CONFIG_FILE", path)
    assert cfg.stages_from_file() == {1, 3}


def test_stages_from_file_handles_malformed(monkeypatch, tmp_path):
    """Malformed JSON falls back to None so default_stages() takes over."""
    path = tmp_path / "tagger.config.json"
    path.write_text("not json at all")
    monkeypatch.setattr(cfg, "TAGGER_CONFIG_FILE", path)
    assert cfg.stages_from_file() is None


def test_write_stages_override_writes_correct_json(monkeypatch, tmp_path):
    """write_stages_override emits the documented {stage1, stage2, stage3} dict."""
    path = tmp_path / "nested" / "tagger.config.json"
    monkeypatch.setattr(cfg, "TAGGER_CONFIG_FILE", path)
    out_path = cfg.write_stages_override(stage1=True, stage2=False, stage3=True)
    assert out_path == path
    data = json.loads(path.read_text())
    assert data == {"stage1": True, "stage2": False, "stage3": True}


def test_clear_stages_override_removes_file(monkeypatch, tmp_path):
    """clear_stages_override deletes the file and returns its path."""
    path = tmp_path / "tagger.config.json"
    path.write_text("{}")
    monkeypatch.setattr(cfg, "TAGGER_CONFIG_FILE", path)
    out = cfg.clear_stages_override()
    assert out == path
    assert not path.exists()


def test_clear_stages_override_noop_when_absent(monkeypatch, tmp_path):
    """clear_stages_override returns None when there's nothing to delete."""
    path = tmp_path / "tagger.config.json"
    monkeypatch.setattr(cfg, "TAGGER_CONFIG_FILE", path)
    assert cfg.clear_stages_override() is None


# ─────────────────────── current_stages ───────────────────────────────────────
def test_current_stages_prefers_file_over_env(monkeypatch, tmp_path):
    """File override wins over env-driven defaults."""
    # env defaults: all on
    monkeypatch.setattr(cfg, "STAGE1_ENABLED", True)
    monkeypatch.setattr(cfg, "STAGE2_ENABLED", True)
    monkeypatch.setattr(cfg, "STAGE3_ENABLED", True)
    # file override: only stage 2
    path = tmp_path / "tagger.config.json"
    path.write_text(json.dumps({"stage1": False, "stage2": True, "stage3": False}))
    monkeypatch.setattr(cfg, "TAGGER_CONFIG_FILE", path)
    assert cfg.current_stages() == {2}


def test_current_stages_falls_back_to_env_when_no_file(monkeypatch, tmp_path):
    """Without an override file, current_stages mirrors default_stages()."""
    monkeypatch.setattr(cfg, "STAGE1_ENABLED", True)
    monkeypatch.setattr(cfg, "STAGE2_ENABLED", False)
    monkeypatch.setattr(cfg, "STAGE3_ENABLED", True)
    monkeypatch.setattr(cfg, "TAGGER_CONFIG_FILE", tmp_path / "absent.json")
    assert cfg.current_stages() == {1, 3}
