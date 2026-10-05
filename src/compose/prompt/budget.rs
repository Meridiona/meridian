//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Keeping each part of the prompt inside its size budget without cutting the part
//! that matters.
//!
//! # Position, not a blind cut
//! The text around a text box is worth most where it is closest to the box: the last
//! messages above a reply box, the first lines below it. So text above the box keeps its
//! **tail**, text below keeps its **head**, and both cut on a line boundary with a marker
//! saying something was left out, so the model never mistakes a truncated thread for a
//! complete one. (A head-and-tail cut that drops the middle is the wrong shape here: it
//! keeps the page's navigation chrome and loses the conversation.)
//!
//! All limits are in characters, never bytes, and every cut lands on a character
//! boundary.
//!
//! # Who calls this
//! [`super::builder`], once per prompt section.
//!
//! # Related
//! - [`super::builder`] - owns the budget numbers.

/// Marker placed where earlier text was dropped.
pub const OMITTED_BEFORE: &str = "[earlier text omitted]";
/// Marker placed where later text was dropped.
pub const OMITTED_AFTER: &str = "[later text omitted]";

/// Normalise noisy captured text: unify newlines, trim line ends, drop lines that are a
/// single stray glyph, drop immediately repeated lines, and collapse runs of blank lines.
///
/// Accessibility captures repeat labels and sprinkle one-character nav glyphs; removing
/// them spends the budget on content. Nothing is reordered.
pub fn tidy(text: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    let mut last_blank = true;
    let normalised = text.replace("\r\n", "\n").replace('\r', "\n");
    for raw in normalised.lines() {
        let line = raw.trim_end();
        let trimmed = line.trim();
        if trimmed.chars().count() == 1 && !trimmed.chars().all(|c| c.is_alphanumeric()) {
            continue;
        }
        if trimmed.is_empty() {
            if !last_blank {
                out.push("");
            }
            last_blank = true;
            continue;
        }
        if out.last().is_some_and(|prev| prev.trim() == trimmed) {
            continue;
        }
        out.push(line);
        last_blank = false;
    }
    while out.last().is_some_and(|l| l.is_empty()) {
        out.pop();
    }
    out.join("\n")
}

/// How many lines back an exact repeat is still treated as the same on-screen label.
const REPEAT_WINDOW: usize = 6;
/// How many lines either side a longer line may sit and still swallow a shorter one.
const CONTAIN_WINDOW: usize = 3;
/// A line must have at least this many characters and words to be swallowed by a neighbour
/// on containment alone. Short lines ("ok", "thanks a lot") are real messages.
const CONTAIN_MIN_CHARS: usize = 12;
const CONTAIN_MIN_WORDS: usize = 2;

fn canonical(line: &str) -> String {
    line.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Remove the duplication accessibility trees are full of, without knowing any app.
///
/// A tree usually exposes both a container's label and the labels of its children, so one
/// on-screen item arrives as three lines: `View Ann's profile`, `View Ann's profile Ann Lee`,
/// `Ann Lee`. Two structural rules collapse that:
///
/// 1. A line that repeats one of the previous few lines exactly is dropped.
/// 2. A line that sits inside a longer line within a few lines of it is dropped, because the
///    longer line already says it - but only if it is long (12+ characters, two+ words) or the
///    longer line is exactly it plus another neighbouring line (the container-and-children
///    shape).
///
/// Short lines that are not part of that shape are never dropped, so real one- and two-word
/// messages survive. Order
/// is preserved and blank lines are kept as paragraph breaks.
pub fn dedupe_nearby(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let canon: Vec<String> = lines.iter().map(|l| canonical(l)).collect();
    let mut keep = vec![true; lines.len()];

    for i in 0..lines.len() {
        if canon[i].is_empty() {
            continue;
        }
        if (i.saturating_sub(REPEAT_WINDOW)..i).any(|j| keep[j] && canon[j] == canon[i]) {
            keep[i] = false;
        }
    }
    for i in 0..lines.len() {
        if !keep[i] || canon[i].is_empty() {
            continue;
        }
        let lo = i.saturating_sub(CONTAIN_WINDOW);
        let hi = (i + CONTAIN_WINDOW + 1).min(lines.len());
        let long_enough = canon[i].chars().count() >= CONTAIN_MIN_CHARS
            && canon[i].split(' ').count() >= CONTAIN_MIN_WORDS;
        let swallowed = (lo..hi).any(|j| {
            if j == i
                || !keep[j]
                || canon[j].len() <= canon[i].len()
                || !canon[j].contains(canon[i].as_str())
            {
                return false;
            }
            // A long line inside a longer neighbour is a duplicate. A short one is only a
            // duplicate when the neighbour is exactly it plus another neighbouring line: the
            // container-and-children shape, which is nothing a person would type.
            long_enough
                || (lo..hi).any(|k| {
                    // `k` may itself have been dropped already (it is the other child), so
                    // its keep flag is deliberately not consulted.
                    k != i
                        && k != j
                        && (canon[j] == format!("{} {}", canon[i], canon[k])
                            || canon[j] == format!("{} {}", canon[k], canon[i]))
                })
        });
        if swallowed {
            keep[i] = false;
        }
    }

    let kept: Vec<&str> = lines
        .iter()
        .zip(&keep)
        .filter(|(_, k)| **k)
        .map(|(l, _)| *l)
        .collect();
    // Dropping lines can leave blank runs behind.
    tidy(&kept.join("\n"))
}

/// The last `max` characters of `text`, starting on a line boundary where possible.
/// Returns the text unchanged when it already fits.
pub fn keep_tail(text: &str, max: usize) -> String {
    let total = text.chars().count();
    if total <= max {
        return text.to_string();
    }
    let skip = total - max;
    let start = text.char_indices().nth(skip).map_or(text.len(), |(i, _)| i);
    let tail = &text[start..];
    // Prefer to start on a fresh line so a message is not cut mid-sentence; fall back to
    // the raw cut when the tail has no newline to use.
    let tail = match tail.find('\n') {
        Some(i) if start > 0 && !text[..start].ends_with('\n') => {
            tail[i + 1..].trim_start_matches('\n')
        }
        _ => tail,
    };
    format!("{OMITTED_BEFORE}\n{tail}")
}

/// The first `max` characters of `text`, ending on a line boundary where possible.
/// Returns the text unchanged when it already fits.
pub fn keep_head(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let end = text.char_indices().nth(max).map_or(text.len(), |(i, _)| i);
    let head = &text[..end];
    let head = match head.rfind('\n') {
        Some(i) if i > 0 && !text[end..].starts_with('\n') => &head[..i],
        _ => head,
    };
    format!("{}\n{OMITTED_AFTER}", head.trim_end())
}

/// Hard cap on a short value such as a window title, with no marker.
pub fn clip(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tidy_drops_glyphs_repeats_and_blank_runs() {
        let raw = "Priya: hi\r\n\u{203a}\nPriya: hi\n\n\n\nAlex: hello  \n";
        assert_eq!(tidy(raw), "Priya: hi\n\nAlex: hello");
    }

    #[test]
    fn tidy_keeps_single_letters_and_digits() {
        assert_eq!(tidy("a\n7"), "a\n7");
    }

    #[test]
    fn tidy_of_empty_is_empty() {
        assert_eq!(tidy(""), "");
        assert_eq!(tidy("\n\n"), "");
    }

    #[test]
    fn keep_tail_returns_short_text_unchanged() {
        assert_eq!(keep_tail("short", 100), "short");
    }

    #[test]
    fn keep_tail_keeps_the_end_on_a_line_boundary_with_a_marker() {
        let text = "line one is old\nline two is old\nline three is new\nline four is new";
        let out = keep_tail(text, 40);
        assert!(out.starts_with(OMITTED_BEFORE));
        assert!(out.ends_with("line four is new"));
        assert!(!out.contains("line one"));
        // never starts mid-line
        assert!(out.lines().nth(1).is_some_and(|l| l.starts_with("line ")));
    }

    #[test]
    fn keep_head_keeps_the_start_on_a_line_boundary_with_a_marker() {
        let text = "first line here\nsecond line here\nthird line here";
        let out = keep_head(text, 30);
        assert!(out.starts_with("first line here"));
        assert!(out.ends_with(OMITTED_AFTER));
        assert!(!out.contains("third"));
    }

    #[test]
    fn cuts_never_split_a_multibyte_character() {
        let text = "\u{1F600}".repeat(50);
        let t = keep_tail(&text, 10);
        let h = keep_head(&text, 10);
        assert!(t.contains(&"\u{1F600}".repeat(10)));
        assert!(h.starts_with(&"\u{1F600}".repeat(10)));
    }

    #[test]
    fn a_single_very_long_line_still_fits_the_budget() {
        let text = "x".repeat(1000);
        let out = keep_tail(&text, 100);
        assert!(out.chars().count() <= 100 + OMITTED_BEFORE.len() + 1);
    }

    #[test]
    fn container_and_child_labels_collapse_to_the_container() {
        let raw = "View Ann's profile\nView Ann's profile Ann Lee\nAnn Lee\n5:32 PM\nHello there";
        assert_eq!(
            dedupe_nearby(raw),
            "View Ann's profile Ann Lee\n5:32 PM\nHello there"
        );
    }

    #[test]
    fn exact_repeats_within_the_window_are_dropped_but_distant_ones_kept() {
        let near = "Product Hunt\nProduct Hunt\nbody";
        assert_eq!(dedupe_nearby(near), "Product Hunt\nbody");
        let far = format!(
            "same line here\n{}same line here",
            "other\n".repeat(REPEAT_WINDOW + 1)
        );
        assert_eq!(dedupe_nearby(&far).matches("same line here").count(), 2);
    }

    #[test]
    fn short_real_messages_are_never_swallowed() {
        let raw = "thanks\nthanks a lot\nok\nok sounds good";
        assert_eq!(dedupe_nearby(raw), raw);
    }

    #[test]
    fn a_two_word_line_inside_a_far_away_longer_line_survives() {
        let raw = format!(
            "sounds good\n{}sounds good to me thanks",
            "filler line number\n".repeat(6)
        );
        assert!(dedupe_nearby(&raw).starts_with("sounds good\n"));
    }

    #[test]
    fn dedupe_keeps_order_and_paragraph_breaks() {
        let raw = "first paragraph line\n\nsecond paragraph line";
        assert_eq!(dedupe_nearby(raw), raw);
        assert_eq!(dedupe_nearby(""), "");
    }

    #[test]
    fn clip_is_a_hard_character_cap() {
        assert_eq!(clip("abcdef", 3), "abc");
        assert_eq!(clip("ab", 3), "ab");
    }
}
