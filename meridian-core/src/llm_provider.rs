//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Which AI runs the user's pipeline — the single, centralised provider choice.
//!
//! The user picks this once during setup (and can change it in Settings): their own
//! coding-agent CLI subscription (Claude / Codex / Cursor / Copilot), the on-device
//! MLX model, or a [`LlmProvider::Custom`] cloud endpoint they configured themselves.
//! One enum, one setting, one factory — every prose LLM call obeys it.
//!
//! # Why `Custom` carries no data
//!
//! A custom provider has an identity (vendor, base URL, API key, model) — but putting it
//! in the variant would cost this enum its `Copy`, its `as_str() -> &'static str`, and its
//! fixed-size [`LlmProvider::all`], across every use site. Worse, there can be SEVERAL
//! configured at once, which one variant cannot express.
//!
//! So the variant is a unit marker meaning "a custom endpoint", and the instances live in
//! a registry in `crate::settings` keyed by id. The stored setting names which one is
//! active; the wire form `custom:<id>` addresses one directly (the LLM Lab runs several
//! side by side). This enum stays the *kind* of provider, never the instance.
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
    /// A user-configured cloud endpoint (OpenAI-compatible), identified by a registry row
    /// rather than by this variant — see the module docs. The first provider that is
    /// neither on-device nor a CLI: it is a direct HTTP call on the user's own API key,
    /// and the only one that spends metered money rather than a flat subscription.
    Custom,
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
            LlmProvider::Custom => "custom",
        }
    }

    /// Parse a stored settings value. `None` for anything unrecognised — never panics,
    /// never errors the whole settings load. See the module docs for why that matters.
    ///
    /// `"custom"` parses to [`LlmProvider::Custom`] — the *kind*. WHICH custom endpoint is
    /// a separate stored id, because this type cannot carry it.
    pub fn from_wire(s: &str) -> Option<Self> {
        match s.trim() {
            "claude" => Some(LlmProvider::Claude),
            "codex" => Some(LlmProvider::Codex),
            "cursor" => Some(LlmProvider::Cursor),
            "copilot" => Some(LlmProvider::Copilot),
            "local" => Some(LlmProvider::Local),
            "custom" => Some(LlmProvider::Custom),
            _ => None,
        }
    }

    /// Every variant — the set a stored `llm_provider` is validated against.
    ///
    /// NOT the set of cards to render: [`LlmProvider::Custom`] is a kind, not an instance,
    /// so it has no card of its own (its cards come one-per-registry-row). Use
    /// [`LlmProvider::builtins`] to enumerate the providers that ARE a single fixed thing.
    pub fn all() -> [LlmProvider; 6] {
        [
            LlmProvider::Claude,
            LlmProvider::Codex,
            LlmProvider::Cursor,
            LlmProvider::Copilot,
            LlmProvider::Local,
            LlmProvider::Custom,
        ]
    }

    /// The providers that are one fixed, self-identifying thing — everything except
    /// [`LlmProvider::Custom`], whose instances live in the settings registry.
    ///
    /// This is what enumerates install probes and picker cards. `detect` reads "no CLI
    /// name" as "the on-device model, always installed", so handing it `Custom` would
    /// claim an unconfigured endpoint is ready — hence the split.
    pub fn builtins() -> [LlmProvider; 5] {
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
    ///
    /// [`LlmProvider::Custom`] is remote — it takes no gate, and (like the CLIs) it does
    /// back off when rate-limited.
    pub fn is_local(self) -> bool {
        matches!(self, LlmProvider::Local)
    }

    /// The executable to invoke, or `None` for a provider that is an HTTP call rather than
    /// a subprocess. The single place the binary names live.
    ///
    /// `None` means "not a subprocess" — it does NOT mean on-device: both
    /// [`LlmProvider::Local`] (localhost MLX) and [`LlmProvider::Custom`] (a cloud
    /// endpoint) answer `None`. Pair it with [`LlmProvider::is_local`] to tell them apart;
    /// reading `None` alone as "local, therefore always available" is wrong for `Custom`.
    pub fn cli_name(self) -> Option<&'static str> {
        match self {
            LlmProvider::Claude => Some("claude"),
            LlmProvider::Codex => Some("codex"),
            LlmProvider::Cursor => Some("cursor-agent"),
            LlmProvider::Copilot => Some("copilot"),
            LlmProvider::Local | LlmProvider::Custom => None,
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

    /// `custom` names the KIND. `custom:<id>` addresses one instance and is deliberately
    /// NOT parsed here — the id has no meaning to this type, and silently accepting the
    /// prefixed form would resolve every custom endpoint to the same thing.
    #[test]
    fn custom_parses_as_the_kind_and_never_as_an_instance() {
        assert_eq!(LlmProvider::from_wire("custom"), Some(LlmProvider::Custom));
        assert_eq!(LlmProvider::from_wire("custom:openrouter-1"), None);
    }

    /// `builtins()` is `all()` minus `Custom` — the split exists so an install probe never
    /// enumerates a kind that has no single binary, endpoint, or identity.
    #[test]
    fn builtins_is_all_without_custom() {
        assert!(!LlmProvider::builtins().contains(&LlmProvider::Custom));
        assert!(LlmProvider::all().contains(&LlmProvider::Custom));
        assert_eq!(LlmProvider::builtins().len() + 1, LlmProvider::all().len());
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

    /// Once "not local" meant "has a CLI". `Custom` broke that: it is remote AND has no
    /// binary, so the two questions are now genuinely independent and `cli_name().is_none()`
    /// can no longer stand in for "runs on this machine".
    #[test]
    fn only_local_is_local_but_two_providers_have_no_cli() {
        for p in LlmProvider::builtins() {
            assert_eq!(p.is_local(), p.cli_name().is_none(), "{p:?}");
        }
        assert!(!LlmProvider::Custom.is_local());
        assert!(LlmProvider::Custom.cli_name().is_none());
    }
}
