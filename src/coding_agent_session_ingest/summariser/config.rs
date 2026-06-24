//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Env-driven config for the summariser. Defaults match
// the former Python summariser/config.py; cadence is adapted to the
// in-daemon model (notify + short sweep instead of a 5-min standalone poll).

use std::path::PathBuf;

pub struct SummariserConfig {
    /// Catch-up sweep cadence (s). The indexer also notifies on its own seals
    /// (near-instant); this covers hook-sealed rows the daemon didn't seal.
    pub sweep_interval_secs: u64,
    /// Max rows summarised per drain pass (sequential — flat memory, no burst).
    pub batch_per_tick: i64,

    pub claude_model: String,
    pub skill_name: String,
    pub claude_timeout_s: u64,

    /// Empty → codex's configured default model.
    pub codex_model: String,
    pub codex_timeout_s: u64,

    pub copilot_timeout_s: u64,

    /// Empty → cursor-agent's configured default model.
    pub cursor_model: String,
    pub cursor_timeout_s: u64,

    /// How many times to attempt the primary engine before falling back to MLX
    /// (the user's "try 2 times, then MLX" rule). Rate-limit short-circuits.
    pub primary_attempts: u32,

    pub transcript_cap_chars: usize,

    pub mlx_host: String,
    pub mlx_port: u16,
    pub mlx_timeout_s: u64,
    pub mlx_max_tokens: u32,
    pub mlx_input_max_tokens: usize,
    pub mlx_chars_per_token: usize,

    /// Noise filter — a row must clear BOTH to be worth a summary.
    pub min_turns: i64,
    pub min_text_bytes: i64,

    /// When BOTH primary and MLX fail (rate-limited + down), back off this long.
    pub rate_limit_backoff_secs: u64,

    /// Neutral cwd for the agent subprocesses (no project CLAUDE.md to load).
    pub meridian_home: PathBuf,
}

impl SummariserConfig {
    pub fn from_env() -> Self {
        fn s(env: &str, default: &str) -> String {
            std::env::var(env).unwrap_or_else(|_| default.to_string())
        }
        fn n<T: std::str::FromStr>(env: &str, default: T) -> T {
            std::env::var(env)
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(default)
        }
        let home = std::env::var("MERIDIAN_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(shellexpand::tilde("~/.meridian").into_owned()));
        Self {
            sweep_interval_secs: n("SUMMARISER_SWEEP_S", 30),
            batch_per_tick: n("SUMMARISER_BATCH_PER_TICK", 8),
            claude_model: s("SUMMARISER_MODEL", "claude-haiku-4-5-20251001"),
            skill_name: s("SUMMARISER_SKILL", "session-summary"),
            claude_timeout_s: n("SUMMARISER_CLAUDE_TIMEOUT_S", 240),
            codex_model: s("SUMMARISER_CODEX_MODEL", ""),
            codex_timeout_s: n("SUMMARISER_CODEX_TIMEOUT_S", 240),
            copilot_timeout_s: n("SUMMARISER_COPILOT_TIMEOUT_S", 240),
            cursor_model: s("SUMMARISER_CURSOR_MODEL", ""),
            cursor_timeout_s: n("SUMMARISER_CURSOR_TIMEOUT_S", 240),
            primary_attempts: n("SUMMARISER_PRIMARY_ATTEMPTS", 2),
            transcript_cap_chars: n("SUMMARISER_TRANSCRIPT_CAP", 500_000),
            mlx_host: s("MLX_SERVER_HOST", "127.0.0.1"),
            mlx_port: n("MLX_SERVER_PORT", 7823),
            mlx_timeout_s: n("SUMMARISER_MLX_TIMEOUT_S", 180),
            mlx_max_tokens: n("SUMMARISER_MLX_MAX_TOKENS", 16384),
            mlx_input_max_tokens: n("SUMMARISER_MLX_INPUT_TOKENS", 25000),
            mlx_chars_per_token: n("SUMMARISER_MLX_CHARS_PER_TOKEN", 4),
            min_turns: n("SUMMARISER_MIN_TURNS", 2),
            min_text_bytes: n("SUMMARISER_MIN_TEXT_BYTES", 800),
            rate_limit_backoff_secs: n("SUMMARISER_BACKOFF_S", 1800),
            meridian_home: home,
        }
    }
}
