"""Rebuild sessions_all.json (85 labeled sessions) and tasks.json (22 open pm_tasks +
KAN-231/239/240 from Jira) directly from meridian.db. Replaces the lost /tmp dataset."""
import json, sqlite3, os, sys

BASE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "data")
sys.path.insert(0, BASE)
from labels_all import L

DB = os.path.expanduser("~/.meridian/meridian.db")
con = sqlite3.connect(DB); con.row_factory = sqlite3.Row

ids = list(L.keys())
q = f"SELECT id, started_at, duration_s, session_summary FROM app_sessions WHERE id IN ({','.join('?'*len(ids))})"
rows = con.execute(q, ids).fetchall()
sess = [{"id": r["id"], "started": r["started_at"], "min": round((r["duration_s"] or 0)/60, 1),
         "session_summary": r["session_summary"]} for r in rows]
sess.sort(key=lambda s: s["started"])
json.dump(sess, open(f"{BASE}/sessions_all.json", "w"))
assert len(sess) == len(ids), f"got {len(sess)} of {len(ids)} sessions"

# 22 open pm_tasks
cols = [c[1] for c in con.execute("PRAGMA table_info(pm_tasks)").fetchall()]
def pick(r, *names):
    for n in names:
        if n in r.keys() and r[n]: return r[n]
    return ""
trows = con.execute("SELECT * FROM pm_tasks").fetchall()
tasks = []
for r in trows:
    tasks.append({
        "task_key": pick(r, "task_key", "key"),
        "issue_type": pick(r, "issue_type", "type") or "Task",
        "title": pick(r, "title", "summary", "name"),
        "epic_title": pick(r, "epic_title", "epic"),
        "description_text": pick(r, "description_text", "description", "desc"),
    })
# 3 now-Done tickets that were the dev pool (re-added with Jira descriptions)
tasks += [
 {"task_key": "KAN-231", "issue_type": "Task", "epic_title": "Accuracy & Automated Deep-Eval",
  "title": "Audit task-classification accuracy and improve it",
  "description_text": "Audit classification accuracy on real data and Golden eval datasets, produce a failure "
   "taxonomy (keyword-mention false positives, decoy resistance, untracked-with-tempting-candidate, recall/silent-drop), "
   "root-cause the top failure clusters (prompt, candidate set, summary quality, confidence calibration), implement fixes "
   "and re-measure against the KAN-199 baseline with no regression."},
 {"task_key": "KAN-239", "issue_type": "Task", "epic_title": "Accuracy & Automated Deep-Eval",
  "title": "Pass confirmed daily plan (today's tasks) to the session-task classifier as Tier-1 candidates",
  "description_text": "Feed the developer's confirmed daily plan into the session->task classifier as the Tier-1 candidate "
   "set so each session is matched against what the dev declared they're working on today. Boost (float plan tickets to top, "
   "mark today's focus) but never hard-filter, so a mid-day pivot stays matchable (recall preserved). Validate with eval Goldens."},
 {"task_key": "KAN-240", "issue_type": "Task", "epic_title": "Observability & Log Pipeline",
  "title": "Emit session-task classifier logs to OpenObserve and set up proper debug tracing",
  "description_text": "Get full debug visibility into the session-task classifier (run_task_linker_mlx.py / MLX classify_sessions) "
   "by streaming its logs into OpenObserve and wiring queryable debug spans per classify call: session_id, candidate count, "
   "today_focus_count, chosen task_key, confidence, category, method, model raw output. Make the per-session decision trail "
   "filterable so a single misclassified session can be traced end-to-end; verify the trace tree renders and document the dashboard."},
]
json.dump(tasks, open(f"{BASE}/tasks.json", "w"))
print(f"sessions: {len(sess)}  | tasks: {len(tasks)}  keys: {', '.join(t['task_key'] for t in tasks)}")
print("pm_tasks columns:", cols)
