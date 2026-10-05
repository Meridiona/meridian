//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Plays the typing sound while a draft is being written, and stops the moment it ends.
//!
//! [`TypingSound::start`] returns a guard. The sound plays from a background thread for as
//! long as the guard lives; dropping it (when the draft arrives, fails or is refused) stops
//! the next tick, so the sound can never outlast the work or get stuck on. If the sound
//! cannot be set up for any reason - the files cannot be written, the system will not load
//! them - the thread logs once and exits; the feature works the same without sound.
//!
//! # How it plays
//! The tick files are written once to `~/.meridian/compose-sounds/` (the sound itself is
//! made in [`super::synth`]). Each variant is loaded several times into a small pool, because
//! an `NSSound` that is still sounding cannot be started again and ticks overlap slightly.
//! `NSSound` follows the normal output volume, unlike the alert volume the system beeps use.
//!
//! # Who calls this
//! [`super::controller`], at the start of every press.
//!
//! # Related
//! - [`super::synth`] - the sound and the typing rhythm.
//! - [`super::feedback`] - the one-shot done and stopped sounds.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use objc2::rc::Retained;
use objc2::AnyThread;
use objc2_app_kit::NSSound;
use objc2_foundation::NSString;

use super::synth::{key_samples, wav_bytes, Rhythm, VARIANTS};

/// Bump when the sound changes, so old files on disk are replaced.
const SOUND_VERSION: &str = "v1";
/// Copies of each variant, so overlapping ticks do not cut each other off.
const POOL_PER_VARIANT: usize = 3;
/// How often the thread checks whether it has been told to stop.
const POLL: Duration = Duration::from_millis(10);

/// Keeps the typing sound playing; dropping it stops it.
pub struct TypingSound {
    stop: Arc<AtomicBool>,
}

impl Drop for TypingSound {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

impl TypingSound {
    /// Start the sound. Returns immediately; the first tick plays within a few milliseconds.
    pub fn start() -> TypingSound {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(1);
        let spawned = std::thread::Builder::new()
            .name("compose-typing-sound".into())
            .spawn(move || run(flag, seed));
        if let Err(e) = spawned {
            tracing::warn!(error = %e, "compose: could not start the typing sound");
        }
        TypingSound { stop }
    }
}

fn sounds_dir() -> Option<PathBuf> {
    meridian_core::paths::meridian_dir().map(|d| d.join("compose-sounds"))
}

/// Write the tick files if they are missing or the wrong size. Returns their paths by variant.
fn ensure_files(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    std::fs::create_dir_all(dir)?;
    (0..VARIANTS)
        .map(|v| {
            let path = dir.join(format!("key-{SOUND_VERSION}-{v}.wav"));
            let wav = wav_bytes(&key_samples(v));
            let current = std::fs::metadata(&path).map(|m| m.len() as usize).ok();
            if current != Some(wav.len()) {
                std::fs::write(&path, &wav)?;
            }
            Ok(path)
        })
        .collect()
}

fn load(path: &Path) -> Option<Retained<NSSound>> {
    let path = NSString::from_str(&path.to_string_lossy());
    NSSound::initWithContentsOfFile_byReference(NSSound::alloc(), &path, false)
}

fn run(stop: Arc<AtomicBool>, seed: u64) {
    let Some(dir) = sounds_dir() else {
        tracing::warn!("compose: no home directory, so no typing sound");
        return;
    };
    let files = match ensure_files(&dir) {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!(error = %e, "compose: could not write the typing-sound files");
            return;
        }
    };
    // pool[variant][copy]
    let pool: Vec<Vec<Retained<NSSound>>> = files
        .iter()
        .map(|f| (0..POOL_PER_VARIANT).filter_map(|_| load(f)).collect())
        .collect();
    if pool.iter().any(Vec::is_empty) {
        tracing::warn!("compose: the system would not load the typing-sound files");
        return;
    }

    let mut rhythm = Rhythm::new(seed);
    let mut next_copy = [0usize; VARIANTS];
    while !stop.load(Ordering::SeqCst) {
        let tick = rhythm.next_tick();
        let due = Instant::now() + tick.delay;
        while Instant::now() < due {
            if stop.load(Ordering::SeqCst) {
                return;
            }
            std::thread::sleep(POLL);
        }
        if stop.load(Ordering::SeqCst) {
            return;
        }
        let copies = &pool[tick.variant];
        let sound = &copies[next_copy[tick.variant] % copies.len()];
        next_copy[tick.variant] += 1;
        sound.stop();
        sound.setVolume(tick.volume);
        sound.play();
    }
}
