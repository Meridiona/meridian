# ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
"""Unit tests for agents.worklog_pipeline.classifier._post_classify.

The critical invariant: a TRANSPORT/HTTP failure must RAISE (so the retry tier fires),
NOT return [] — because an empty list is indistinguishable from a genuine "no match"
and would silently drop the hour's real tickets (then wrongly propose a new one). Also
covers the hallucinated-key and low-confidence drops. Model-free: urlopen is mocked.
"""
from __future__ import annotations

import io
import json
import urllib.error
from contextlib import contextmanager

import pytest

from agents.worklog_pipeline import classifier as clf
from agents.worklog_pipeline.classifier import Candidate, _post_classify


def _cands(*keys):
    return [Candidate(task_key=k, title=k, doc=k) for k in keys]


@contextmanager
def _fake_response(payload: dict):
    """Mimic urlopen's context-manager + .read() interface."""
    yield io.BytesIO(json.dumps(payload).encode())


def _patch_urlopen(monkeypatch, *, payload=None, exc=None):
    def fake_urlopen(req, timeout=None):
        if exc is not None:
            raise exc
        return _fake_response(payload)
    monkeypatch.setattr(clf.urllib.request, "urlopen", fake_urlopen)


# ─────────────────── the cardinal-sin invariant ───────────────────────────────
def test_transport_error_raises_not_empty(monkeypatch):
    """A connection/timeout error must propagate, never collapse to []."""
    _patch_urlopen(monkeypatch, exc=urllib.error.URLError("connection refused"))
    with pytest.raises(urllib.error.URLError):
        _post_classify("http://x", "report", _cands("KAN-1"), 1, "note")


def test_server_500_raises(monkeypatch):
    """The route's HTTPError(500) on an inference fault must propagate too."""
    err = urllib.error.HTTPError("http://x", 500, "boom", {}, io.BytesIO(b"err"))
    _patch_urlopen(monkeypatch, exc=err)
    with pytest.raises(urllib.error.HTTPError):
        _post_classify("http://x", "report", _cands("KAN-1"), 1, "note")


# ─────────────────── normal parsing + filtering ───────────────────────────────
def test_wellformed_match_returns_binding(monkeypatch):
    _patch_urlopen(monkeypatch, payload={
        "reasoning": "r",
        "matches": [{"task_key": "KAN-1", "confidence": 0.9, "why": "did it"}],
    })
    out = _post_classify("http://x", "report", _cands("KAN-1", "KAN-2"), 1, "note")
    assert [(b.task_key, b.confidence) for b in out] == [("KAN-1", pytest.approx(0.9))]


def test_empty_matches_returns_empty(monkeypatch):
    """A well-formed 'nothing matched' response is a legitimate empty result."""
    _patch_urlopen(monkeypatch, payload={"reasoning": "no overlap", "matches": []})
    assert _post_classify("http://x", "report", _cands("KAN-1"), 1, "note") == []


def test_hallucinated_key_dropped(monkeypatch):
    """A key that was never a candidate is dropped (can't bind to a non-candidate)."""
    _patch_urlopen(monkeypatch, payload={
        "reasoning": "r",
        "matches": [{"task_key": "KAN-999", "confidence": 0.9, "why": "ghost"}],
    })
    assert _post_classify("http://x", "report", _cands("KAN-1"), 1, "note") == []


def test_low_confidence_dropped(monkeypatch):
    """Below _MIN_CONFIDENCE matches are dropped."""
    _patch_urlopen(monkeypatch, payload={
        "reasoning": "r",
        "matches": [{"task_key": "KAN-1", "confidence": 0.2, "why": "weak"}],
    })
    assert _post_classify("http://x", "report", _cands("KAN-1"), 1, "note") == []
