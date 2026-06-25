"""Worklog pipeline — hour activity report → task match → worklog draft.

The new agno-based pipeline that replaces both the per-session task classifier
and the old per-ticket pm_worklog synth. It operates at the HOUR level:

    distil_hour → activity report → reranker hint → tiered task match →
    worklog generation → draft for UI approval

Matching is abstention-first (an hour can map to one task, several, or none),
and every structured step is captured by schema enforcement — never regex.
"""
