//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Stages 7-8: per-session facility-location selection + entity rescue.
//!
//! Ports `_floc_pick` and `_entity_rescue` from `session_distiller._distil`. Each
//! session's kept spans are trimmed to a cap by max-min diversity (seed the longest,
//! then greedily add the most-distant span), then any entity-bearing span floc dropped
//! is added back if its ticket key / PR / file path isn't already covered.

use std::collections::{HashMap, HashSet};

use super::segment::NAMED_ENTITY_RE;
use super::{Span, ENTITY_RESCUE_CAP};

/// A candidate span paired with its embedding (`None` when the embedder is unavailable).
pub struct Item {
    pub span: Span,
    pub vec: Option<Vec<f32>>,
}

fn longest_idx(items: &[Item]) -> usize {
    let mut best = 0usize;
    let mut best_len = 0usize;
    for (i, it) in items.iter().enumerate() {
        let l = it.span.line.chars().count();
        if l > best_len {
            best_len = l;
            best = i;
        }
    }
    best
}

/// Max-min diversity pick of up to `k` items from one session. With vectors: seed the
/// longest span, then greedily add the item whose max cosine to the picked set is lowest
/// (most distant). Without vectors (degraded): longest-first up to `k`. Returns
/// `(picked, discarded)`; ordering is irrelevant (the caller re-sorts the full set by
/// time before rendering).
pub fn floc_pick(items: Vec<Item>, k: usize) -> (Vec<Span>, Vec<Span>) {
    if items.len() <= k {
        return (items.into_iter().map(|it| it.span).collect(), Vec::new());
    }
    let has_vecs = items.iter().all(|it| it.vec.is_some());
    let seed = longest_idx(&items);
    let mut picked: Vec<usize> = vec![seed];

    if has_vecs {
        while picked.len() < k {
            let mut best_j = usize::MAX;
            let mut best_dist = f64::MIN;
            for j in 0..items.len() {
                if picked.contains(&j) {
                    continue;
                }
                let vj = items[j].vec.as_ref().unwrap();
                let max_sim = picked
                    .iter()
                    .map(|&p| super::dot(vj, items[p].vec.as_ref().unwrap()))
                    .fold(f32::MIN, f32::max);
                let dist = 1.0 - max_sim as f64;
                if dist > best_dist {
                    best_dist = dist;
                    best_j = j;
                }
            }
            picked.push(best_j);
        }
    } else {
        // Degraded: longest-first fill.
        let mut order: Vec<usize> = (0..items.len()).filter(|&j| j != seed).collect();
        order.sort_by_key(|&j| std::cmp::Reverse(items[j].span.line.chars().count()));
        for j in order {
            if picked.len() >= k {
                break;
            }
            picked.push(j);
        }
    }

    let picked_set: HashSet<usize> = picked.into_iter().collect();
    let mut keep = Vec::new();
    let mut drop = Vec::new();
    for (j, it) in items.into_iter().enumerate() {
        if picked_set.contains(&j) {
            keep.push(it.span);
        } else {
            drop.push(it.span);
        }
    }
    (keep, drop)
}

/// Add back entity-bearing spans that floc discarded, when their named entities aren't
/// already covered by the session's selected spans. `discarded_by_sid` is iterated in
/// session (row) order, capped at [`ENTITY_RESCUE_CAP`] rescues per session. Returns the
/// extended selection and the number rescued.
pub fn entity_rescue(
    mut selected: Vec<Span>,
    discarded_by_sid: &[(i64, Vec<Span>)],
) -> (Vec<Span>, usize) {
    let mut coverage: HashMap<i64, String> = HashMap::new();
    for sp in &selected {
        let entry = coverage.entry(sp.sid).or_default();
        entry.push(' ');
        entry.push_str(&sp.line.to_lowercase());
    }

    let mut rescued: Vec<Span> = Vec::new();
    let mut total = 0usize;
    for (sid, candidates) in discarded_by_sid {
        let mut covered = coverage.get(sid).cloned().unwrap_or_default();
        let mut added = 0usize;
        for sp in candidates {
            if added >= ENTITY_RESCUE_CAP {
                break;
            }
            if !NAMED_ENTITY_RE.is_match(&sp.line) {
                continue;
            }
            let entities: Vec<String> = NAMED_ENTITY_RE
                .find_iter(&sp.line)
                .map(|m| m.as_str().to_lowercase())
                .collect();
            if entities.iter().any(|e| !covered.contains(e.as_str())) {
                rescued.push(sp.clone());
                covered.push(' ');
                covered.push_str(&sp.line.to_lowercase());
                added += 1;
            }
        }
        total += added;
    }
    selected.extend(rescued);
    (selected, total)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(line: &str, vec: Option<Vec<f32>>) -> Item {
        Item {
            span: Span {
                sid: 1,
                app: "A".into(),
                win: "W".into(),
                t: "10:00".into(),
                line: line.into(),
            },
            vec,
        }
    }

    #[test]
    fn floc_returns_all_when_under_cap() {
        let items = vec![item("one", None), item("two", None)];
        let (keep, drop) = floc_pick(items, 5);
        assert_eq!(keep.len(), 2);
        assert!(drop.is_empty());
    }

    #[test]
    fn floc_degraded_takes_longest_first() {
        let items = vec![
            item("short", None),
            item("a much much much longer line here", None),
            item("medium length line", None),
        ];
        let (keep, drop) = floc_pick(items, 2);
        assert_eq!(keep.len(), 2);
        assert_eq!(drop.len(), 1);
        // the longest must be kept
        assert!(keep.iter().any(|s| s.line.starts_with("a much much")));
    }

    #[test]
    fn entity_rescue_adds_uncovered_key() {
        let selected = vec![Span {
            sid: 1,
            app: "A".into(),
            win: "W".into(),
            t: "10:00".into(),
            line: "worked on stuff".into(),
        }];
        let discarded = vec![(
            1i64,
            vec![Span {
                sid: 1,
                app: "A".into(),
                win: "W".into(),
                t: "10:01".into(),
                line: "closed KAN-999".into(),
            }],
        )];
        let (out, n) = entity_rescue(selected, &discarded);
        assert_eq!(n, 1);
        assert!(out.iter().any(|s| s.line.contains("KAN-999")));
    }
}
