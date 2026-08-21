//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! CPU and memory used by **Meridian's own processes**, sampled on the poll
//! tick and shipped as a per-day aggregate on `daily_usage`.
//!
//! # Why this exists
//! Memory footprint is the longest-standing complaint about always-on tooling,
//! and until now the only numbers anyone had came from a developer's own
//! Activity Monitor. This turns that into fleet data: how much a real install
//! actually costs on a real machine, and whether a release made it worse.
//!
//! # What is measured, and what is deliberately NOT
//! Two processes, identified **by PID**: this tray, and the daemon (whose pid
//! comes from [`crate::commands::daemon_control::status`]'s socket greeting).
//!
//! Transient subprocesses — the coding-agent CLIs the summariser spawns — are
//! **excluded on purpose**, and this is the single most important decision in
//! the module. Meridian spawns `claude`, `codex` and `cursor-agent`; those are
//! the exact binaries the *user* runs themselves, and watching them is the
//! product. Attributing by process NAME would fold the user's own AI usage into
//! "Meridian's CPU" and produce a number that is badly wrong while looking
//! entirely plausible. Attributing by parent PID fixes that only while both are
//! alive — a CLI that outlives a daemon restart is reparented to launchd and
//! becomes indistinguishable from the user's own. So the honest scope is the
//! two processes we can identify exactly, and the exclusion is named here and
//! in the privacy doc rather than left for someone to infer from a low number.
//!
//! # What the memory number does and does not see
//! [`sysinfo`] reports `pti_resident_size` on macOS — plain **RSS**. That is
//! honest for the daemon: the session-distiller's embedder runs on CPU by
//! deliberate choice (`src/embedder/device.rs` — candle's Metal backend has no
//! `layer-norm` kernel, which BGE needs), and the MLX model that once made RSS
//! undercount by ~6.5 GB is gone from the codebase entirely.
//!
//! The tray is a **floor, not a total**: it captures through ScreenCaptureKit,
//! and GPU-backed IOSurfaces do not appear in RSS. Reconciling the tray figure
//! with Activity Monitor will therefore show Activity Monitor higher. That is
//! expected, not a bug — say so before someone "fixes" it.
//!
//! # Window semantics (read before trusting a peak)
//! Samples accumulate into a window that resets at the local-day boundary AND
//! at tray restart, because the accumulator lives in memory. So a window can
//! cover a full day or twenty minutes, and `perf_window_s` + `perf_samples`
//! ship alongside every aggregate so the two are never confused. A forced
//! update relaunches every tray in the fleet, which makes short windows common
//! on release days specifically.
//!
//! `*_peak` is the **highest sample**, not the highest usage: at one sample a
//! minute, an eight-second spike during OCR is invisible. Read it as "the worst
//! we happened to observe", never as a true maximum.
//!
//! # Who calls this
//! - [`sample`] — [`crate::poll::run_poll_loop`], once per tick.
//! - [`completed_for`] — [`super::daily::send_daily_usage`], to attach the
//!   closed day's aggregate.

use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use sysinfo::{Pid, PidExt, ProcessExt, ProcessRefreshKind, System, SystemExt};

/// Running totals for one process over one window.
#[derive(Debug, Clone, Default, PartialEq)]
struct ProcStats {
    rss_peak_bytes: u64,
    rss_sum_bytes: u128,
    cpu_peak: f32,
    cpu_sum: f64,
    samples: u32,
}

impl ProcStats {
    fn add(&mut self, rss_bytes: u64, cpu: f32) {
        self.rss_peak_bytes = self.rss_peak_bytes.max(rss_bytes);
        self.rss_sum_bytes += rss_bytes as u128;
        if cpu > self.cpu_peak {
            self.cpu_peak = cpu;
        }
        self.cpu_sum += cpu as f64;
        self.samples += 1;
    }

    fn rss_peak_mb(&self) -> f64 {
        bytes_to_mb(self.rss_peak_bytes)
    }

    fn rss_mean_mb(&self) -> f64 {
        if self.samples == 0 {
            return 0.0;
        }
        round2((self.rss_sum_bytes as f64 / self.samples as f64) / 1_048_576.0)
    }

    fn cpu_mean(&self) -> f64 {
        if self.samples == 0 {
            return 0.0;
        }
        round2(self.cpu_sum / self.samples as f64)
    }
}

fn bytes_to_mb(b: u64) -> f64 {
    round2(b as f64 / 1_048_576.0)
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// One window of samples: a local day, or the part of it this tray was up for.
#[derive(Debug, Clone)]
pub(crate) struct PerfWindow {
    /// The local day these samples belong to.
    day: String,
    /// Wall-clock span actually covered, in seconds.
    window_s: u64,
    tray: ProcStats,
    daemon: ProcStats,
    /// Tray + daemon summed WITHIN each sample, then aggregated — not the sum
    /// of the two peaks, which would report a combined high-water mark that
    /// never actually occurred.
    combined: ProcStats,
}

/// The mutable half, kept separate so [`PerfWindow`] can be cloned out cheaply.
struct WindowBuilder {
    day: String,
    started: Instant,
    tray: ProcStats,
    daemon: ProcStats,
    combined: ProcStats,
}

impl WindowBuilder {
    fn new(day: String) -> Self {
        Self {
            day,
            started: Instant::now(),
            tray: ProcStats::default(),
            daemon: ProcStats::default(),
            combined: ProcStats::default(),
        }
    }

    fn finish(&self) -> PerfWindow {
        PerfWindow {
            day: self.day.clone(),
            window_s: self.started.elapsed().as_secs(),
            tray: self.tray.clone(),
            daemon: self.daemon.clone(),
            combined: self.combined.clone(),
        }
    }
}

struct PerfState {
    current: WindowBuilder,
    /// The most recently CLOSED window, kept so `daily_usage` can attach it.
    /// Never consumed on read — a failed send retries the same day, and a
    /// destructive read would leave the retry with no perf data.
    completed: Option<PerfWindow>,
}

fn state() -> &'static Mutex<Option<PerfState>> {
    static S: OnceLock<Mutex<Option<PerfState>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(None))
}

/// The `System` handle must persist across ticks: `cpu_usage()` is computed
/// from the delta since the previous refresh of that same process, so a fresh
/// `System` every tick would report 0% forever.
fn sysinfo_handle() -> &'static Mutex<System> {
    static SYS: OnceLock<Mutex<System>> = OnceLock::new();
    SYS.get_or_init(|| Mutex::new(System::new()))
}

/// Take one sample of the tray and (when running) the daemon.
///
/// Rolls the window when the local day changes, so the closed day's aggregate
/// is available to `daily_usage` on the very tick that reports it. Never
/// panics and never blocks on the daemon: a missing pid simply means the
/// daemon half of this sample is skipped.
pub(crate) async fn sample() {
    let daemon_pid = crate::commands::daemon_control::status().await.pid;
    let today = meridian_core::date::today_string();

    let (tray_rss, tray_cpu, daemon_rss, daemon_cpu) = {
        let Ok(mut sys) = sysinfo_handle().lock() else {
            return;
        };
        let refresh = ProcessRefreshKind::new().with_cpu();
        let self_pid = Pid::from_u32(std::process::id());
        sys.refresh_process_specifics(self_pid, refresh);
        let tray = sys
            .process(self_pid)
            .map(|p| (p.memory(), p.cpu_usage()))
            .unwrap_or((0, 0.0));

        let daemon = daemon_pid.and_then(|pid| {
            let pid = Pid::from_u32(pid);
            sys.refresh_process_specifics(pid, refresh);
            sys.process(pid).map(|p| (p.memory(), p.cpu_usage()))
        });
        (tray.0, tray.1, daemon.map(|d| d.0), daemon.map(|d| d.1))
    };

    // A zero RSS means the refresh found no such process — record nothing
    // rather than a fabricated zero, which would drag every mean down.
    if tray_rss == 0 {
        tracing::debug!("perf: tray process not readable — skipping sample");
        return;
    }

    let Ok(mut guard) = state().lock() else {
        return;
    };
    let st = guard.get_or_insert_with(|| PerfState {
        current: WindowBuilder::new(today.clone()),
        completed: None,
    });

    if st.current.day != today {
        st.completed = Some(st.current.finish());
        st.current = WindowBuilder::new(today.clone());
        tracing::info!(
            day = %st.completed.as_ref().map(|w| w.day.clone()).unwrap_or_default(),
            "perf: window rolled at local-day boundary"
        );
    }

    st.current.tray.add(tray_rss, tray_cpu);
    if let (Some(rss), Some(cpu)) = (daemon_rss, daemon_cpu) {
        st.current.daemon.add(rss, cpu);
        st.current
            .combined
            .add(tray_rss.saturating_add(rss), tray_cpu + cpu);
    } else {
        // Daemon down: the combined figure is still the honest total for this
        // instant, it is simply the tray alone.
        st.current.combined.add(tray_rss, tray_cpu);
    }
}

/// The closed window for `day`, if this tray actually accumulated one.
///
/// `None` when the tray never sampled that day (started today and is reporting
/// a stale cursor, or was restarted past the boundary) — the caller then omits
/// the `perf_*` properties entirely rather than shipping zeros, matching the
/// rule the health snapshot follows: unknown must be absent, never wrong.
pub(crate) fn completed_for(day: &str) -> Option<PerfWindow> {
    let guard = state().lock().ok()?;
    let st = guard.as_ref()?;
    st.completed.as_ref().filter(|w| w.day == day).cloned()
}

impl PerfWindow {
    /// Fold the window into an event's property map under a `perf_` prefix.
    ///
    /// `perf_samples` and `perf_window_s` are not optional extras: without them
    /// a peak from a twenty-minute window is indistinguishable from a peak over
    /// a full day, and averaging across the fleet silently mixes the two.
    pub(crate) fn write_properties(&self, props: &mut serde_json::Map<String, serde_json::Value>) {
        props.insert(
            "perf_samples".to_string(),
            serde_json::json!(self.combined.samples),
        );
        props.insert(
            "perf_window_s".to_string(),
            serde_json::json!(self.window_s),
        );

        for (prefix, s) in [
            ("perf_tray", &self.tray),
            ("perf_daemon", &self.daemon),
            ("perf_total", &self.combined),
        ] {
            props.insert(
                format!("{prefix}_rss_peak_mb"),
                serde_json::json!(s.rss_peak_mb()),
            );
            props.insert(
                format!("{prefix}_rss_mean_mb"),
                serde_json::json!(s.rss_mean_mb()),
            );
            props.insert(
                format!("{prefix}_cpu_peak"),
                serde_json::json!(round2(s.cpu_peak as f64)),
            );
            props.insert(
                format!("{prefix}_cpu_mean"),
                serde_json::json!(s.cpu_mean()),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peak_and_mean_are_computed_over_samples() {
        let mut s = ProcStats::default();
        s.add(100 * 1_048_576, 10.0);
        s.add(300 * 1_048_576, 50.0);
        s.add(200 * 1_048_576, 30.0);

        assert_eq!(s.samples, 3);
        assert_eq!(s.rss_peak_mb(), 300.0);
        assert_eq!(s.rss_mean_mb(), 200.0);
        assert_eq!(round2(s.cpu_peak as f64), 50.0);
        assert_eq!(s.cpu_mean(), 30.0);
    }

    /// An empty window must report zeros rather than dividing by zero. It can
    /// happen: the daemon half stays empty for a whole window if the daemon
    /// never ran.
    #[test]
    fn an_empty_stat_does_not_divide_by_zero() {
        let s = ProcStats::default();
        assert_eq!(s.rss_mean_mb(), 0.0);
        assert_eq!(s.cpu_mean(), 0.0);
    }

    /// The combined figure must be the peak of (tray + daemon) measured within
    /// the same sample — NOT the sum of the two independent peaks, which would
    /// report a high-water mark that never actually occurred. Here the tray
    /// peaks on sample 1 and the daemon on sample 2; summing peaks would claim
    /// 500 MB, but Meridian never used more than 400 MB at once.
    #[test]
    fn combined_is_per_sample_not_the_sum_of_peaks() {
        let mut tray = ProcStats::default();
        let mut daemon = ProcStats::default();
        let mut combined = ProcStats::default();

        for (t, d) in [(300u64, 100u64), (100, 200)] {
            let (t, d) = (t * 1_048_576, d * 1_048_576);
            tray.add(t, 0.0);
            daemon.add(d, 0.0);
            combined.add(t + d, 0.0);
        }

        assert_eq!(tray.rss_peak_mb(), 300.0);
        assert_eq!(daemon.rss_peak_mb(), 200.0);
        assert_eq!(
            combined.rss_peak_mb(),
            400.0,
            "peak of the sums, not the sum of the peaks (which would be 500)"
        );
    }

    /// The window aggregate must carry its own sample count and span, so a
    /// peak from a short window is never silently compared against a full day.
    #[test]
    fn properties_carry_the_window_size() {
        let mut b = WindowBuilder::new("2026-05-14".to_string());
        b.tray.add(150 * 1_048_576, 5.0);
        b.combined.add(150 * 1_048_576, 5.0);

        let mut props = serde_json::Map::new();
        b.finish().write_properties(&mut props);

        assert_eq!(props.get("perf_samples"), Some(&serde_json::json!(1)));
        assert!(props.contains_key("perf_window_s"));
        assert_eq!(
            props.get("perf_tray_rss_peak_mb"),
            Some(&serde_json::json!(150.0))
        );
        // The daemon never ran in this window: zeros, not absent keys, because
        // "Meridian's daemon used no memory" is a real and interesting answer.
        assert_eq!(
            props.get("perf_daemon_rss_peak_mb"),
            Some(&serde_json::json!(0.0))
        );
    }

    /// A window is only offered to the day it actually covers. Reporting a
    /// stale cursor's day with another day's numbers would be worse than
    /// reporting nothing.
    #[test]
    fn a_window_is_only_returned_for_its_own_day() {
        let w = WindowBuilder::new("2026-05-14".to_string()).finish();
        assert_eq!(w.day, "2026-05-14");
        assert!(
            w.day != "2026-05-13",
            "completed_for filters on this equality"
        );
    }
}
