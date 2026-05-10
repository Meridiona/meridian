# meridian — normalises screenpipe activity into structured app sessions
"""Unit tests for the deterministic text composition used by Stage 2."""
from __future__ import annotations

import pytest

from agents import text_for_embedding as tfe


def test_session_text_is_deterministic():
    """Same session dict produces the same output (and hash) on every call."""
    sess = {
        "app_name": "Cursor",
        "category": "coding",
        "window_titles": [
            {"window_name": "main.rs — meridian", "count": 4},
        ],
        "ocr_samples": [{"text": "fn run_etl()"}],
        "audio_snippets": [],
    }
    a = tfe.session_text(sess)
    b = tfe.session_text(sess)
    assert a == b
    assert tfe.text_hash(a) == tfe.text_hash(b)


def test_session_text_samples_returns_multiple_labels():
    """Multiple OCR fragments produce one (label, text) tuple each."""
    sess = {
        "app_name": "Code",
        "category": "coding",
        "window_titles": [{"window_name": "main.py", "count": 2}],
        "ocr_samples": [
            {"text": "def alpha(): return 1   # this is the first OCR sample"},
            {"text": "def beta(): return 2    # this is the second OCR sample"},
            {"text": "def gamma(): return 3   # this is the third OCR sample"},
        ],
        "audio_snippets": [],
    }
    samples = tfe.session_text_samples(sess)
    labels = [label for label, _ in samples]
    assert "titles" in labels
    # Three OCR fragments → three ocr_* labels.
    ocr_labels = [l for l in labels if l.startswith("ocr_")]
    assert len(ocr_labels) == 3


def test_task_text_composes_known_sections():
    """task_text always carries title / type / project / description in that order."""
    task = {
        "title": "Migrate KAN-86 to new ETL",
        "description_text": "Move the legacy session writer to the new sqlx pool.",
        "issue_type": "task",
        "project_key": "KAN",
    }
    out = tfe.task_text(task)
    # Each labelled section should appear as its own line.
    lines = out.split("\n")
    assert any(l.startswith("title:") for l in lines)
    assert any(l.startswith("type:") for l in lines)
    assert any(l.startswith("project:") for l in lines)
    assert any(l.startswith("description:") for l in lines)


def test_text_hash_stable_and_distinguishing():
    """Hash is identical for identical input and different for different input."""
    a = tfe.text_hash("hello world")
    b = tfe.text_hash("hello world")
    c = tfe.text_hash("hello, world")
    assert a == b
    assert a != c
    # 16-char sha1 prefix.
    assert len(a) == 16


def test_empty_session_falls_back_to_empty_label():
    """A session with no titles/ocr/audio still produces one ('empty', …) tuple."""
    sess = {
        "app_name": "",
        "category": "",
        "window_titles": [],
        "ocr_samples": [],
        "audio_snippets": [],
    }
    samples = tfe.session_text_samples(sess)
    assert len(samples) == 1
    assert samples[0][0] == "empty"


def test_clean_title_strips_vscode_extension_banner():
    """_clean_title removes the long "extensions want to relaunch" tail."""
    raw = (
        "main.py — The following extensions want to relaunch the terminal because "
        "they have updated: Python, GitHub Copilot, Pylance"
    )
    cleaned = tfe._clean_title(raw)
    assert cleaned == "main.py"
