//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Which AI runs the user's pipeline — the single, centralised provider choice.
//!
//! The user picks this once during setup (and can change it in Settings): their own
//! coding-agent CLI subscription (Claude / Codex / Cursor / Copilot), or the on-device
//! MLX model. One enum, one setting, one factory — every prose LLM call obeys it.
//!
//! # Why the setting is a `String`, not this enum
//!
//! [`crate::settings::load_runtime_settings`] falls back to `RuntimeSettings::default()`
//! on *any* deserialise error, so one unparseable field silently resets **every** setting
//! the daemon reads — log level, poll interval, work hours. If `llm_provider` were typed
//! as this enum and a newer build wrote a variant an older daemon didn't know, that
//! daemon wouldn't merely lose the provider: it would lose the user's work hours too.
//!
//! So the field is stored as a `String` and parsed here with [`LlmProvider::from_wire`],
//! which returns `None` for anything unrecognised. The caller falls back to the default.
//! An unknown provider costs you the provider, and nothing else.
//!
//! # Who calls this
//! `crate::settings` (the stored field), the daemon's `llm::resolver` (the one place that
//! turns the choice into a live backend), and the tray's `commands::settings` (validation
//! on write). The UI mirrors the wire forms in `ui/lib/llm-providers.ts`.
//!
//! # Related
//! [`crate::canonical_task::Provider`] — the same shape for PM trackers, and the pattern
//! this follows.

use serde::{Deserialize, Serialize};

/// The AI backend that runs the user's prose LLM calls.
///
/// Wire forms deliberately match the coding-agent summariser's own `Source::as_str()`
/// (`"claude"`, `"codex"`, `"cursor"`, `"copilot"`), so the two can be mapped without a
/// translation table. `Local` is the on-device MLX model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LlmProvider {
    Claude,
    Codex,
    Cursor,
    Copilot,
    /// The on-device MLX model. Nothing leaves the machine.
    Local,
}

impl Default for LlmProvider {
    /// On-device. Privacy by default is the product's pitch, and it is the only backend
    /// guaranteed to be present — every other one needs a CLI the user may not have.
    fn default() -> Self {
        Self::Local
    }
}

impl LlmProvider {
    /// The snake_case string form — equals the serde wire form and the settings value.
    pub fn as_str(self) -> &'static str {
        match self {
            LlmProvider::Claude => "claude",
            LlmProvider::Codex => "codex",
            LlmProvider::Cursor => "cursor",
            LlmProvider::Copilot => "copilot",
            LlmProvider::Local => "local",
        }
    }

    /// Parse a stored settings value. `None` for anything unrecognised — never panics,
    /// never errors the whole settings load. See the module docs for why that matters.
    pub fn from_wire(s: &str) -> Option<Self> {
        match s.trim() {
            "claude" => Some(LlmProvider::Claude),
            "codex" => Some(LlmProvider::Codex),
            "cursor" => Some(LlmProvider::Cursor),
            "copilot" => Some(LlmProvider::Copilot),
            "local" => Some(LlmProvider::Local),
            _ => None,
        }
    }

    /// Every valid wire form — for validating a write and rendering the picker.
    pub fn all() -> [LlmProvider; 5] {
        [
            LlmProvider::Claude,
            LlmProvider::Codex,
            LlmProvider::Cursor,
            LlmProvider::Copilot,
            LlmProvider::Local,
        ]
    }

    /// Does this provider run the model on this machine?
    ///
    /// The one place the "gate the GPU, don't gate the subscription" rule is expressed:
    /// local calls contend for a single Metal device and must hold the LLM gate, while a
    /// CLI call spends the user's own cloud quota and must not.
    pub fn is_local(self) -> bool {
        matches!(self, LlmProvider::Local)
    }

    /// The executable to invoke, or `None` for the on-device model (which is an HTTP call
    /// to the local MLX server, not a subprocess). The single place the binary names live.
    pub fn cli_name(self) -> Option<&'static str> {
        match self {
            LlmProvider::Claude => Some("claude"),
            LlmProvider::Codex => Some("codex"),
            LlmProvider::Cursor => Some("cursor-agent"),
            LlmProvider::Copilot => Some("copilot"),
            LlmProvider::Local => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_form_round_trips() {
        for p in LlmProvider::all() {
            assert_eq!(LlmProvider::from_wire(p.as_str()), Some(p));
        }
    }

    #[test]
    fn serde_matches_as_str() {
        for p in LlmProvider::all() {
            let json = serde_json::to_string(&p).expect("serialise");
            assert_eq!(json, format!("\"{}\"", p.as_str()));
            let back: LlmProvider = serde_json::from_str(&json).expect("deserialise");
            assert_eq!(back, p);
        }
    }

    #[test]
    fn unknown_wire_form_is_none_not_a_panic() {
        // The whole point of `from_wire`: a provider from a newer build degrades to the
        // default instead of taking every other setting down with it.
        assert_eq!(LlmProvider::from_wire("gemini"), None);
        assert_eq!(LlmProvider::from_wire(""), None);
        assert_eq!(LlmProvider::from_wire("Claude"), None); // case-sensitive by design
    }

    #[test]
    fn whitespace_is_tolerated() {
        assert_eq!(
            LlmProvider::from_wire("  local  "),
            Some(LlmProvider::Local)
        );
    }

    #[test]
    fn default_is_local() {
        assert_eq!(LlmProvider::default(), LlmProvider::Local);
        assert!(LlmProvider::default().is_local());
    }

    #[test]
    fn only_local_is_local_and_only_local_has_no_cli() {
        for p in LlmProvider::all() {
            assert_eq!(p.is_local(), p.cli_name().is_none(), "{p:?}");
        }
    }
}
