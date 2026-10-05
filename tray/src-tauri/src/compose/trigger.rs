//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Telling a deliberate tap of the trigger key from every other thing a modifier key does.
//!
//! The trigger is a bare modifier key (left Option by default), which is also how people
//! type accented characters, hold shortcuts and drag with Option. A tap therefore has to
//! be strict: the key goes down and up within [`TAP_MAX_MS`], with no other key, click or
//! modifier in between. Anything else is the user doing something else and must never
//! start a draft.
//!
//! This is a pure state machine fed timestamps by the caller, so every case (a slow press,
//! Option+click, Option+Shift, a typed accent) is unit-tested without a keyboard.
//!
//! # Who calls this
//! [`super::tap`]'s event-tap callback feeds it; it returns [`Tap`] when a press completes.
//!
//! # Related
//! - [`super::tap`] - turns raw macOS events into [`InputEvent`]s.

/// Longest a press may last and still count as a tap. Longer is a hold (edit mode, later).
pub const TAP_MAX_MS: u64 = 400;

/// Which Option key triggers a draft. Left is the default so right Option stays free for
/// other tools while this is being compared against them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerKey {
    LeftOption,
    RightOption,
}

impl TriggerKey {
    /// Parse the settings value. Unknown or missing values fall back to left Option.
    pub fn from_setting(value: Option<&str>) -> TriggerKey {
        match value.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
            Some("right_option") => TriggerKey::RightOption,
            _ => TriggerKey::LeftOption,
        }
    }
}

// Device-dependent modifier bits in a CGEvent's flags (IOLLEvent.h). The generic
// `Alternate` bit cannot say which Option key is down; these can.
const DEVICE_LEFT_ALT: u64 = 0x0000_0020;
const DEVICE_RIGHT_ALT: u64 = 0x0000_0040;
const FLAG_SHIFT: u64 = 0x0002_0000;
const FLAG_CONTROL: u64 = 0x0004_0000;
const FLAG_COMMAND: u64 = 0x0010_0000;
const FLAG_FN: u64 = 0x0080_0000;

/// The modifier keys currently down.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifiers {
    pub left_option: bool,
    pub right_option: bool,
    pub shift: bool,
    pub control: bool,
    pub command: bool,
    pub function: bool,
}

impl Modifiers {
    /// Decode the raw flags word of a `flagsChanged` event.
    pub fn from_flags(flags: u64) -> Modifiers {
        Modifiers {
            left_option: flags & DEVICE_LEFT_ALT != 0,
            right_option: flags & DEVICE_RIGHT_ALT != 0,
            shift: flags & FLAG_SHIFT != 0,
            control: flags & FLAG_CONTROL != 0,
            command: flags & FLAG_COMMAND != 0,
            function: flags & FLAG_FN != 0,
        }
    }

    fn trigger_down(self, key: TriggerKey) -> bool {
        match key {
            TriggerKey::LeftOption => self.left_option,
            TriggerKey::RightOption => self.right_option,
        }
    }

    /// True when anything other than the trigger key is held.
    fn others_down(self, key: TriggerKey) -> bool {
        let other_option = match key {
            TriggerKey::LeftOption => self.right_option,
            TriggerKey::RightOption => self.left_option,
        };
        other_option || self.shift || self.control || self.command || self.function
    }
}

/// One thing the user did at the keyboard or mouse, with a millisecond timestamp.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputEvent {
    /// The set of held modifier keys changed.
    Modifiers { mods: Modifiers, at_ms: u64 },
    /// An ordinary key went down.
    KeyDown { at_ms: u64 },
    /// A mouse button went down.
    MouseDown { at_ms: u64 },
}

/// A completed, deliberate tap of the trigger key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tap {
    pub held_ms: u64,
}

/// Detects taps of one trigger key.
#[derive(Debug, Clone)]
pub struct TapDetector {
    key: TriggerKey,
    /// When the trigger went down, while it is down.
    down_at_ms: Option<u64>,
    /// Something else happened while the trigger was down, so this press is not a tap.
    spoiled: bool,
}

impl TapDetector {
    pub fn new(key: TriggerKey) -> Self {
        Self {
            key,
            down_at_ms: None,
            spoiled: false,
        }
    }

    /// Feed one event; returns a [`Tap`] when this event completes one.
    pub fn feed(&mut self, event: InputEvent) -> Option<Tap> {
        match event {
            InputEvent::KeyDown { .. } | InputEvent::MouseDown { .. } => {
                if self.down_at_ms.is_some() {
                    self.spoiled = true;
                }
                None
            }
            InputEvent::Modifiers { mods, at_ms } => self.on_modifiers(mods, at_ms),
        }
    }

    fn on_modifiers(&mut self, mods: Modifiers, at_ms: u64) -> Option<Tap> {
        let trigger = mods.trigger_down(self.key);
        let others = mods.others_down(self.key);
        match (self.down_at_ms, trigger) {
            // The trigger just went down.
            (None, true) => {
                self.down_at_ms = Some(at_ms);
                // Pressing it while another modifier is already held is a chord.
                self.spoiled = others;
                None
            }
            // Still down, and the modifier set changed again: a chord, not a tap.
            (Some(_), true) => {
                if others {
                    self.spoiled = true;
                }
                None
            }
            // The trigger just came up.
            (Some(down_at), false) => {
                let held_ms = at_ms.saturating_sub(down_at);
                let ok = !self.spoiled && !others && held_ms <= TAP_MAX_MS;
                self.down_at_ms = None;
                self.spoiled = false;
                ok.then_some(Tap { held_ms })
            }
            // Some other modifier changed while the trigger is up: nothing to track.
            (None, false) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEFT: u64 = DEVICE_LEFT_ALT | 0x0008_0000;
    const RIGHT: u64 = DEVICE_RIGHT_ALT | 0x0008_0000;

    fn mods(flags: u64, at_ms: u64) -> InputEvent {
        InputEvent::Modifiers {
            mods: Modifiers::from_flags(flags),
            at_ms,
        }
    }

    fn detector() -> TapDetector {
        TapDetector::new(TriggerKey::LeftOption)
    }

    #[test]
    fn a_quick_press_and_release_is_a_tap() {
        let mut d = detector();
        assert_eq!(d.feed(mods(LEFT, 1000)), None);
        assert_eq!(d.feed(mods(0, 1120)), Some(Tap { held_ms: 120 }));
    }

    #[test]
    fn a_slow_press_is_a_hold_not_a_tap() {
        let mut d = detector();
        d.feed(mods(LEFT, 0));
        assert_eq!(d.feed(mods(0, TAP_MAX_MS + 1)), None);
    }

    #[test]
    fn a_press_at_exactly_the_limit_is_still_a_tap() {
        let mut d = detector();
        d.feed(mods(LEFT, 0));
        assert!(d.feed(mods(0, TAP_MAX_MS)).is_some());
    }

    #[test]
    fn typing_an_accent_with_option_is_not_a_tap() {
        let mut d = detector();
        d.feed(mods(LEFT, 0));
        d.feed(InputEvent::KeyDown { at_ms: 50 });
        assert_eq!(d.feed(mods(0, 100)), None);
    }

    #[test]
    fn option_click_is_not_a_tap() {
        let mut d = detector();
        d.feed(mods(LEFT, 0));
        d.feed(InputEvent::MouseDown { at_ms: 40 });
        assert_eq!(d.feed(mods(0, 90)), None);
    }

    #[test]
    fn option_with_another_modifier_is_not_a_tap() {
        let mut d = detector();
        d.feed(mods(LEFT, 0));
        d.feed(mods(LEFT | FLAG_SHIFT, 30));
        d.feed(mods(LEFT, 60));
        assert_eq!(d.feed(mods(0, 90)), None);
    }

    #[test]
    fn pressing_option_while_another_modifier_is_held_is_not_a_tap() {
        let mut d = detector();
        d.feed(mods(FLAG_COMMAND, 0));
        d.feed(mods(FLAG_COMMAND | LEFT, 10));
        d.feed(mods(FLAG_COMMAND, 60));
        assert_eq!(d.feed(mods(0, 90)), None);
    }

    #[test]
    fn the_other_option_key_does_not_trigger_it() {
        let mut d = detector();
        d.feed(mods(RIGHT, 0));
        assert_eq!(d.feed(mods(0, 80)), None);
    }

    #[test]
    fn the_right_option_key_can_be_chosen() {
        let mut d = TapDetector::new(TriggerKey::RightOption);
        d.feed(mods(RIGHT, 0));
        assert!(d.feed(mods(0, 80)).is_some());
    }

    #[test]
    fn a_spoiled_press_does_not_poison_the_next_one() {
        let mut d = detector();
        d.feed(mods(LEFT, 0));
        d.feed(InputEvent::KeyDown { at_ms: 10 });
        d.feed(mods(0, 50));
        d.feed(mods(LEFT, 1000));
        assert!(d.feed(mods(0, 1080)).is_some());
    }

    #[test]
    fn keys_pressed_while_the_trigger_is_up_are_ignored() {
        let mut d = detector();
        d.feed(InputEvent::KeyDown { at_ms: 0 });
        d.feed(mods(LEFT, 10));
        assert!(d.feed(mods(0, 90)).is_some());
    }

    #[test]
    fn two_quick_taps_are_two_taps() {
        let mut d = detector();
        d.feed(mods(LEFT, 0));
        assert!(d.feed(mods(0, 80)).is_some());
        d.feed(mods(LEFT, 200));
        assert!(d.feed(mods(0, 280)).is_some());
    }

    #[test]
    fn setting_parser_defaults_to_left_option() {
        assert_eq!(TriggerKey::from_setting(None), TriggerKey::LeftOption);
        assert_eq!(
            TriggerKey::from_setting(Some("nonsense")),
            TriggerKey::LeftOption
        );
        assert_eq!(
            TriggerKey::from_setting(Some(" Right_Option ")),
            TriggerKey::RightOption
        );
    }

    #[test]
    fn device_bits_tell_the_two_option_keys_apart() {
        let m = Modifiers::from_flags(LEFT);
        assert!(m.left_option && !m.right_option);
        let m = Modifiers::from_flags(RIGHT);
        assert!(m.right_option && !m.left_option);
    }
}
