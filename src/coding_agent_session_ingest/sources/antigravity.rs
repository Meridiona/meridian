// meridian — normalises screenpipe activity into structured app sessions
//
// Antigravity (Google's agentic IDE) source — DETECTION-ONLY STUB. The IDE is
// a VS Code fork, but its agent-conversation store could not be pinned: the
// only Antigravity install available during development had no User data at
// all (no workspaceStorage, no chat dirs), so any parser written now would be
// guesswork. The stub keeps the seam honest: Antigravity presence is detected
// and logged once (so a user wondering why their Antigravity sessions don't
// appear gets a clear answer in the daemon log), and `collect_changed`
// returns nothing. When a real store sample exists, pin the format the same
// way cursor.rs / copilot_vscode.rs document theirs, and implement here —
// db.rs already maps agent "antigravity" → app_name "Antigravity Agent".

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Once;

use chrono::{DateTime, Utc};

use crate::coding_agent_session_ingest::jsonl::NormRecord;

pub const AGENT: &str = "antigravity";

static DORMANT_NOTICE: Once = Once::new();

#[derive(Clone)]
pub struct AntigravitySource {
    pub app_dir: PathBuf,
}

impl AntigravitySource {
    pub fn from_env() -> Self {
        let raw = std::env::var("ANTIGRAVITY_APP_DIR")
            .unwrap_or_else(|_| "~/Library/Application Support/Antigravity".to_string());
        Self {
            app_dir: PathBuf::from(shellexpand::tilde(&raw).into_owned()),
        }
    }

    /// Antigravity is installed on this machine. Presence only gates the
    /// detection notice — ingest stays dormant either way.
    pub fn present(&self) -> bool {
        self.app_dir.is_dir()
    }

    /// Dormant: log the detection once per daemon lifetime, ingest nothing.
    pub fn collect_changed(
        &self,
        _endpoints: &HashMap<String, String>,
        _now: DateTime<Utc>,
    ) -> Vec<(String, Vec<NormRecord>, Option<String>)> {
        DORMANT_NOTICE.call_once(|| {
            tracing::info!(
                app_dir = %self.app_dir.display(),
                "Antigravity detected — conversation ingest not yet supported \
                 (store format unpinned); sessions will not appear in app_sessions"
            );
        });
        Vec::new()
    }
}
