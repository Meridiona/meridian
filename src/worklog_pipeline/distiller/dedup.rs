//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Stages 4-6: document-frequency cut, lexical dedup, and SemDeDup.
//!
//! Ports the DF loop, the `seen`/`lex` loop, and the `keep_mask` cosine loop from
//! `session_distiller._distil`. DF cut drops boilerplate that recurs across many
//! sessions (unless it carries a named entity); lexical dedup removes exact-prefix
//! repeats; SemDeDup drops near-duplicate meaning via embedding cosine — the one stage
//! that needs the model, and the one that degrades to a no-op when the embedder is
//! unavailable (the caller simply passes empty vectors).

use std::collections::{HashMap, HashSet};

use super::segment::{norm, NAMED_ENTITY_RE};
use super::Span;

/// Stage 4 — document-frequency cut. A span whose normalized text appears in
/// `>= max(3, DF_FRAC * nsess)` distinct sessions is boilerplate and dropped, unless it
/// carries a named entity (which always survives).
pub fn df_cut(spans: Vec<Span>, nsess: usize, df_frac: f64) -> Vec<Span> {
    let mut df: HashMap<String, HashSet<i64>> = HashMap::new();
    for sp in &spans {
        df.entry(norm(&sp.line)).or_default().insert(sp.sid);
    }
    let df_cut = std::cmp::max(3, (df_frac * nsess as f64) as usize);
    spans
        .into_iter()
        .filter(|sp| {
            df.get(&norm(&sp.line)).map(|s| s.len()).unwrap_or(0) < df_cut
                || NAMED_ENTITY_RE.is_match(&sp.line)
        })
        .collect()
}

/// Stage 5 — lexical dedup. Sort by `(time, sid)`, then keep the first span for each
/// normalized 80-char prefix. Returns spans in that sorted order (the SemDeDup + floc
/// stages rely on it).
pub fn lexical_dedup(mut spans: Vec<Span>) -> Vec<Span> {
    spans.sort_by(|a, b| (a.t.as_str(), a.sid).cmp(&(b.t.as_str(), b.sid)));
    let mut seen: HashSet<String> = HashSet::new();
    let mut lex: Vec<Span> = Vec::with_capacity(spans.len());
    for sp in spans {
        let key: String = norm(&sp.line).chars().take(80).collect();
        if seen.insert(key) {
            lex.push(sp);
        }
    }
    lex
}

/// Stage 6 — SemDeDup. Given `lex` spans and their row-aligned embeddings `v`, return a
/// keep-mask: a span is dropped when it is more similar than `thr` to an already-kept
/// span **and** (it shares that span's session **or** carries no named entity). A span
/// bearing a fresh entity from a different session always survives.
///
/// `v` must be row-aligned with `lex` (one L2-normalized vector each). An empty `v`
/// (embedder unavailable) keeps every span — SemDeDup becomes a no-op and the body
/// simply dedups less.
pub fn sem_dedup_mask(lex: &[Span], v: &[Vec<f32>], thr: f64) -> Vec<bool> {
    let mut keep = vec![true; lex.len()];
    if v.len() != lex.len() {
        return keep; // degraded: no vectors → keep all
    }
    let mut kept: Vec<usize> = Vec::new();
    for i in 0..lex.len() {
        if !kept.is_empty() {
            // similarity of i to each already-kept span; find the max.
            let mut best_sim = f32::MIN;
            let mut best_kept = kept[0];
            for &k in &kept {
                let s = super::dot(&v[i], &v[k]);
                if s > best_sim {
                    best_sim = s;
                    best_kept = k;
                }
            }
            if best_sim as f64 > thr {
                let same_session = lex[best_kept].sid == lex[i].sid;
                if same_session || !NAMED_ENTITY_RE.is_match(&lex[i].line) {
                    keep[i] = false;
                    continue;
                }
            }
        }
        kept.push(i);
    }
    keep
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(sid: i64, t: &str, line: &str) -> Span {
        Span {
            sid,
            app: "App".into(),
            win: "W".into(),
            t: t.into(),
            line: line.into(),
        }
    }

    #[test]
    fn df_cut_drops_boilerplate_keeps_entities() {
        // "common boilerplate line here" appears in 5 sessions → cut (df_cut = max(3, 0.25*5=1) = 3).
        let mut spans = Vec::new();
        for sid in 0..5 {
            spans.push(span(sid, "10:00", "common boilerplate line here"));
        }
        // an entity-bearing line repeated across sessions survives regardless
        for sid in 0..5 {
            spans.push(span(sid, "10:00", "touched src/hour.rs again"));
        }
        let out = df_cut(spans, 5, 0.25);
        assert!(out.iter().all(|s| s.line.contains("hour.rs")));
        assert_eq!(out.len(), 5);
    }

    #[test]
    fn lexical_dedup_keeps_first_of_prefix() {
        let spans = vec![
            span(1, "10:00", "Reviewed the resolver changes carefully"),
            span(2, "10:01", "Reviewed the resolver changes carefully"),
            span(3, "10:02", "Something entirely different happened"),
        ];
        let out = lexical_dedup(spans);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn sem_dedup_noop_without_vectors() {
        let lex = vec![span(1, "10:00", "a"), span(2, "10:01", "b")];
        let mask = sem_dedup_mask(&lex, &[], 0.86);
        assert_eq!(mask, vec![true, true]);
    }

    #[test]
    fn sem_dedup_drops_near_duplicate_same_session() {
        let lex = vec![
            span(1, "10:00", "plain text one"),
            span(1, "10:01", "plain text two"),
        ];
        // near-identical unit vectors → second dropped (same session, cosine > thr)
        let v = vec![vec![1.0, 0.0], vec![0.999, 0.0447]];
        let mask = sem_dedup_mask(&lex, &v, 0.86);
        assert_eq!(mask, vec![true, false]);
    }
}
