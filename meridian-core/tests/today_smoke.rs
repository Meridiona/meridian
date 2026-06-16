//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Smoke / rough golden-compare for `get_today` against the REAL
//! `~/.meridian/meridian.db`. `#[ignore]`d so it never runs in CI (it's
//! machine-specific and reads live data). Run it explicitly:
//!
//!   cargo test -p meridian-core --test today_smoke -- --ignored --nocapture
//!
//! Then compare the printed scalars against the live Node route:
//!   curl -s localhost:3939/api/today | jq '{focus_s, idle_s, agent_s, switch_count}'

#[tokio::test]
#[ignore]
async fn today_smoke() {
    let home = std::env::var("HOME").unwrap();
    let db = format!("{home}/.meridian/meridian.db");
    let pool = meridian_core::open_existing(&db).await.expect("open db");

    let date = meridian_core::date::today_string();
    let now = chrono::Utc::now().to_rfc3339();
    let r = meridian_core::today::get_today(&pool, &date, &now)
        .await
        .expect("get_today");

    println!(
        "RUST_TODAY date={} sessions={} focus_s={} idle_s={} agent_s={} supervised_s={} \
         autonomous_s={} engaged_s={} switch_count={} session_count={} tasks={} \
         presence_segments={} agent_segments={} active={}",
        r.date,
        r.sessions.len(),
        r.focus_s,
        r.idle_s,
        r.agent_s,
        r.supervised_s,
        r.autonomous_s,
        r.engaged_s,
        r.switch_count,
        r.session_count,
        r.task_totals.len(),
        r.presence_segments.len(),
        r.agent_segments.len(),
        r.active.is_some(),
    );

    // Sanity invariants (true regardless of the live data):
    assert!(r.focus_s >= 0 && r.idle_s >= 0 && r.agent_s >= 0);
    assert!(
        r.supervised_s <= r.agent_s,
        "supervised is a subset of agent"
    );
    assert_eq!(r.autonomous_s, (r.agent_s - r.supervised_s).max(0));
    assert_eq!(r.engaged_s, r.focus_s + r.autonomous_s);
    assert_eq!(
        r.session_count,
        r.sessions.len() as i64 + r.active.is_some() as i64
    );
}
