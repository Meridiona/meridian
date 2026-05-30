// meridian — normalises screenpipe activity into structured app sessions
//
// Parity harness: prints the segmentation fingerprint of one coding-agent JSONL
// so it can be diffed against the Python implementation. Critical fields
// (segment count, boundaries, turn counts, active-time) must match exactly;
// `tlen` (transcript char-length) is reported for an approximate transcript
// comparison. Run: cargo run --example ca_parse -- <path.jsonl>

use std::path::Path;

use meridian::coding_agent::{parse_session_segments, SegmentParams};

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: ca_parse <path.jsonl>");
    let (meta, segs) = parse_session_segments(Path::new(&path), &SegmentParams::default());
    println!("AGENT {}", meta.agent);
    println!("NSEG {}", segs.len());
    for (i, s) in segs.iter().enumerate() {
        println!(
            "SEG {} start={} end={} u={} a={} active={} tlen={} last={}",
            i,
            s.segment_started_at,
            s.ended_at,
            s.user_turns,
            s.assistant_turns,
            s.active_seconds,
            s.transcript.chars().count(),
            s.is_last,
        );
    }
}
