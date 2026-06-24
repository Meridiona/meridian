"""Driver: sweep embedder x affinity x threshold, score cluster purity vs task_key.

Usage:
  python run_cluster.py --days 10 --models bge-small
  python run_cluster.py --days 10 --models bge-small,qwen3-0.6b,jina-v3 --dump
"""
from __future__ import annotations
import argparse, sys, numpy as np
import clib, embed


def cosine(vecs: np.ndarray) -> np.ndarray:
    return np.clip(vecs @ vecs.T, -1.0, 1.0)


def run():
    ap = argparse.ArgumentParser()
    ap.add_argument("--days", type=int, default=10)
    ap.add_argument("--models", default="bge-small")
    ap.add_argument("--strip", default="both", choices=["yes", "no", "both"])
    ap.add_argument("--dump", action="store_true")
    ap.add_argument("--dump-cfg", default="")
    args = ap.parse_args()

    sessions = clib.load_sessions(days=args.days)
    print(f"loaded {len(sessions)} usable sessions over {args.days} days "
          f"({sum(1 for s in sessions if s.task_key)} labeled)\n")

    strip_opts = {"both": [True, False], "yes": [True], "no": [False]}[args.strip]
    # cache content variants
    prepared = {}
    for st in strip_opts:
        clib.prepare_content(sessions, strip=st)
        prepared[st] = [s.content for s in sessions]
    # restore one for entity matrices (independent of strip)
    clib.prepare_content(sessions, strip=True)

    header = (f"{'model':11} {'st':2} {'time':5} {'tau':4} {'thr':4} "
              f"{'#clu':>5} {'#sing':>5} {'#big':>4} {'grp%':>5} {'avgN':>5} "
              f"{'PREC':>5} {'REC':>5} {'homo':>5} {'vmes':>5}")
    print(header); print("-" * len(header))

    rows = []
    best = None
    for tag in args.models.split(","):
        tag = tag.strip()
        for st in strip_opts:
            try:
                vecs = embed.embed(tag, prepared[st])
            except Exception as e:
                print(f"!! embed {tag} strip={st} failed: {e}", file=sys.stderr); continue
            cos = cosine(vecs)
            for time_mode, tau in [("none", 0), ("gate", 30), ("gate", 60), ("gate", 120)]:
                cfg = {"time_mode": time_mode, "tau_min": tau, "ent_boost": 0.0}
                S = clib.combine_affinity(cos, sessions, cfg)
                for thr in (0.30, 0.40, 0.50):
                    labels = clib.cluster_agglomerative(S, thr)
                    ev = clib.evaluate(sessions, labels)
                    row = dict(model=tag, strip=("Y" if st else "N"), time=time_mode,
                               tau=tau, thr=thr, **ev)
                    rows.append((row, labels, cfg, st))
                    print(f"{tag:11} {row['strip']:2} {time_mode:5} {tau:<4} {thr:<4} "
                          f"{ev['n_clusters']:>5} {ev['singletons']:>5} {ev['largest']:>4} "
                          f"{ev.get('grouped_frac',0):>5} {ev.get('mean_multi',0):>5} "
                          f"{ev.get('pair_prec',0):>5} {ev.get('pair_rec',0):>5} "
                          f"{ev.get('homogeneity',0):>5} {ev.get('v_measure',0):>5}")
                    # objective: maximise recall (consolidation) subject to high precision (safety)
                    prec = ev.get("pair_prec", 0)
                    score = ev.get("pair_rec", 0) if prec >= 0.85 else -1 + prec
                    if best is None or score > best[0]:
                        best = (score, row, labels, cfg, st)

    if best:
        _, row, labels, cfg, st = best
        print(f"\n=== BEST by v_measure+ari: {row} ===")
        if args.dump:
            clib.prepare_content(sessions, strip=st)
            print(clib.cluster_purity_report(sessions, labels, top=15))


if __name__ == "__main__":
    run()
