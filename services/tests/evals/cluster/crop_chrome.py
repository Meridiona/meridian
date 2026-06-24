"""Offline prototype: crop the Chrome tab-strip / address-bar from OCR using per-word
bounding boxes (screenpipe ocr_text.text_json), keeping page body. Shows DROPPED vs KEPT
so we can confirm no real content is lost. Usage: python crop_chrome.py [n_frames] [top_thresh]
"""
import sqlite3, json, sys, os, re
SP = os.path.expanduser("~/.screenpipe/db.sqlite")
N = int(sys.argv[1]) if len(sys.argv) > 1 else 6
THRESH = float(sys.argv[2]) if len(sys.argv) > 2 else 0.10
BOT = 0.97  # drop status bar below this
TICKET = re.compile(r"\b[A-Z]{2,4}-\d{2,6}\b")  # avoid UTF-8 false positive


def clean_url(url):
    """Keep host + path + a few meaningful query keys; drop tracking params."""
    if not url:
        return ""
    m = re.match(r"https?://([^/]+)(/[^?#]*)?(?:\?([^#]*))?", url)
    if not m:
        return url[:120]
    host, path, query = m.group(1), m.group(2) or "", m.group(3) or ""
    keep = []
    for kv in query.split("&"):
        k = kv.split("=")[0].lower()
        if k in ("q", "query", "search", "id", "v", "p"):  # meaningful, not tracking
            keep.append(kv)
    q = ("?" + "&".join(keep)) if keep else ""
    return f"{host}{path}{q}"[:160]


def reconstruct(words, lo, hi):
    """Join words whose vertical center is in [lo,hi), in reading order (top then left)."""
    kept = []
    for w in words:
        try:
            top = float(w["top"]); h = float(w.get("height", 0) or 0)
            cy = top + h / 2
        except (KeyError, ValueError):
            continue
        if lo <= cy < hi:
            kept.append((round(top, 2), float(w.get("left", 0) or 0), w.get("text", "")))
    kept.sort(key=lambda x: (x[0], x[1]))
    return " ".join(t for _, _, t in kept if t)


con = sqlite3.connect(SP)
rows = con.execute(
    """SELECT o.frame_id, f.browser_url, o.text_json
       FROM ocr_text o JOIN frames f ON f.id=o.frame_id
       WHERE o.app_name='Google Chrome' AND o.text_json IS NOT NULL
         AND LENGTH(o.text_json) > 800
       ORDER BY o.frame_id DESC LIMIT ?""", (N,)).fetchall()
con.close()

print(f"crop threshold: drop top < {THRESH} (tabs/addr) and top > {BOT} (status)\n")
tot_before = tot_after = 0
for fid, url, tj in rows:
    try:
        words = json.loads(tj)
    except Exception:
        continue
    dropped_top = reconstruct(words, -1, THRESH)
    kept_body = reconstruct(words, THRESH, BOT)
    dropped_bot = reconstruct(words, BOT, 2)
    full = reconstruct(words, -1, 2)
    # re-add the CLEANED browser_url so we don't lose "what page" signal
    curl = clean_url(url)
    final = (f"[url] {curl} . " if curl else "") + kept_body
    tot_before += len(full); tot_after += len(final)
    # any ticket keys hiding in the dropped band?
    drop_tickets = set(TICKET.findall(dropped_top + " " + dropped_bot))
    body_tickets = set(TICKET.findall(kept_body))
    lost = drop_tickets - body_tickets  # tickets ONLY in dropped band = potential loss

    print("=" * 90)
    print(f"frame {fid}   clean_url={clean_url(url) or '—'}")
    print(f"  body {len(kept_body)} chars (was {len(full)} full)  dropped_top={len(dropped_top)} dropped_bot={len(dropped_bot)}")
    print(f"\n  ── DROPPED top band (top<{THRESH}) — should be tabs/addressbar only:")
    print("     " + (dropped_top[:240] or "(none)"))
    if dropped_bot:
        print(f"  ── DROPPED bottom band (top>{BOT}):")
        print("     " + dropped_bot[:160])
    print(f"\n  ── KEPT page body (raw, no url):")
    print("     " + (re.sub(r"\s+", " ", kept_body)[:340] or "(EMPTY!)"))
    print(f"  ── final session_text head (url + body):")
    print("     " + re.sub(r"\s+", " ", final)[:160])
    if lost:
        print(f"\n  ⚠ TICKET KEYS ONLY IN DROPPED BAND (potential loss): {lost}")
    else:
        print(f"\n  ✓ no ticket keys lost (dropped tickets: {drop_tickets or 'none'}; all also in body or none)")
    print()

print("=" * 90)
print(f"OVERALL: {tot_before} -> {tot_after} chars  ({100*tot_after/max(tot_before,1):.0f}% kept, "
      f"{100-100*tot_after/max(tot_before,1):.0f}% removed as chrome)")
