//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

pub fn format_duration(secs: u64) -> String {
    if secs < 60 {
        return "Just getting started".to_string();
    }
    let minutes = secs / 60;
    let hours = minutes / 60;
    let rem_minutes = minutes % 60;

    if hours == 0 {
        if minutes == 1 {
            "1 minute".to_string()
        } else {
            format!("{} minutes", minutes)
        }
    } else if rem_minutes == 0 {
        if hours == 1 {
            "1 hour".to_string()
        } else {
            format!("{} hours", hours)
        }
    } else if hours == 1 {
        if rem_minutes == 1 {
            "1 hour 1 minute".to_string()
        } else {
            format!("1 hour {} minutes", rem_minutes)
        }
    } else if rem_minutes == 1 {
        format!("{} hours 1 minute", hours)
    } else {
        format!("{} hours {} minutes", hours, rem_minutes)
    }
}

/// Compact running-timer readout for the menu-bar title — `H:MM:SS`
/// (e.g. `2:05:09`), matching the design's live tray pill.
pub fn format_timer(secs: u64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    format!("{h}:{m:02}:{s:02}")
}

pub fn format_elapsed(secs: u64) -> String {
    if secs < 60 {
        return "moments ago".to_string();
    }
    format!("for {}", format_duration(secs))
}

pub fn describe_active(app_name: &str, elapsed_s: u64) -> String {
    let verb = app_verb(app_name);
    let time = format_elapsed(elapsed_s);
    format!("{} {}", verb, time)
}

fn app_verb(app: &str) -> String {
    let lower = app.to_lowercase();
    if lower.contains("claude") || lower.contains("cursor") || lower.contains("codex") {
        format!("In a session with {}", app)
    } else if lower.contains("code")
        || lower.contains("xcode")
        || lower.contains("vim")
        || lower.contains("neovim")
        || lower.contains("emacs")
    {
        format!("Deep in {}", app)
    } else if lower.contains("slack") || lower.contains("teams") || lower.contains("discord") {
        format!("In {}", app)
    } else if lower.contains("safari") || lower.contains("chrome") || lower.contains("firefox") {
        format!("Browsing in {}", app)
    } else if lower.contains("terminal")
        || lower.contains("iterm")
        || lower.contains("warp")
        || lower.contains("kitty")
    {
        format!("In {} — in the zone", app)
    } else if lower.contains("mail") || lower.contains("outlook") {
        format!("In {} — catching up on email", app)
    } else if lower.contains("figma") || lower.contains("sketch") {
        format!("Designing in {}", app)
    } else if lower.contains("notion") || lower.contains("obsidian") || lower.contains("notes") {
        format!("Writing in {}", app)
    } else {
        format!("In {}", app)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn under_a_minute() {
        assert_eq!(format_duration(0), "Just getting started");
        assert_eq!(format_duration(45), "Just getting started");
        assert_eq!(format_duration(59), "Just getting started");
    }

    #[test]
    fn minutes_only() {
        assert_eq!(format_duration(60), "1 minute");
        assert_eq!(format_duration(120), "2 minutes");
        assert_eq!(format_duration(1740), "29 minutes");
    }

    #[test]
    fn exact_hours() {
        assert_eq!(format_duration(3600), "1 hour");
        assert_eq!(format_duration(7200), "2 hours");
    }

    #[test]
    fn hours_and_minutes() {
        assert_eq!(format_duration(3660), "1 hour 1 minute");
        assert_eq!(format_duration(5040), "1 hour 24 minutes");
        assert_eq!(format_duration(9000), "2 hours 30 minutes");
    }

    #[test]
    fn describe_active_short() {
        let desc = describe_active("VS Code", 30);
        assert_eq!(desc, "Deep in VS Code moments ago");
    }

    #[test]
    fn describe_active_long() {
        let desc = describe_active("Claude", 5040);
        assert!(desc.contains("session"));
        assert!(desc.contains("Claude"));
        assert!(desc.contains("1 hour 24 minutes"));
    }
}
