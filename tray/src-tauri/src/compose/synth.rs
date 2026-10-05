//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The "typing" sound that plays while a draft is being written: what each tick sounds like
//! and when it plays.
//!
//! # Why synthesised
//! No system sound is a keystroke, and shipping audio files means assets to license and
//! bundle. A key tick is simple physics - a very short bright click on top of a soft low
//! "thock" that fades fast - so it is generated here, deterministically, into small WAV
//! files the first time it is needed. Several variants differ slightly in pitch and noise so
//! the sequence sounds like fingers, not a metronome.
//!
//! # Rhythm
//! People type in bursts: a few quick keys, a breath, a few more. [`Rhythm`] produces that
//! shape - bursts of 3 to 7 ticks 70 to 150 ms apart, separated by 280 to 600 ms pauses - with
//! a little random variation in loudness, from a seeded generator so it is reproducible.
//!
//! Everything in this file is pure and unit-tested; playing the files is [`super::typing`].
//!
//! # Who calls this
//! [`super::typing`], which writes the WAVs and schedules the ticks.
//!
//! # Related
//! - [`super::feedback`] - the one-shot sounds for done and stopped.

use std::time::Duration;

pub const SAMPLE_RATE: u32 = 44_100;
/// Number of distinct tick sounds.
pub const VARIANTS: usize = 4;
/// Length of one tick.
const TICK_MS: usize = 70;
/// Loudest sample as a fraction of full scale. Kept low: this plays for seconds at a time.
const PEAK: f32 = 0.5;
/// The last few milliseconds fade to zero so a tick never ends on a pop.
const FADE_MS: usize = 6;

/// Small deterministic generator (a 64-bit linear congruential generator). Quality is plenty
/// for noise and timing jitter, and it keeps the sound reproducible.
#[derive(Debug, Clone)]
struct Lcg(u64);

impl Lcg {
    fn next_u32(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) as u32
    }

    /// Uniform in [0, 1).
    fn unit(&mut self) -> f32 {
        self.next_u32() as f32 / (u32::MAX as f32 + 1.0)
    }

    /// Uniform in [lo, hi).
    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }
}

/// The samples of one key tick, mono, 16-bit.
pub fn key_samples(variant: usize) -> Vec<i16> {
    let variant = variant % VARIANTS;
    let n = SAMPLE_RATE as usize * TICK_MS / 1000;
    let mut rng =
        Lcg(0x9E37_79B9_7F4A_7C15 ^ (variant as u64 + 1).wrapping_mul(0xD1B5_4A32_D192_ED03));

    // Each variant is a slightly different key: pitch of the body and brightness of the click.
    let body_hz = [150.0, 172.0, 196.0, 224.0][variant];
    let click_ms = [1.6, 1.9, 2.2, 1.7][variant];
    let rate = SAMPLE_RATE as f32;

    let mut prev_noise = 0.0f32;
    let mut raw = Vec::with_capacity(n);
    for i in 0..n {
        let t = i as f32 / rate;
        // Click: white noise, high-passed by differencing, with a very fast decay.
        let noise = rng.range(-1.0, 1.0);
        let bright = noise - prev_noise;
        prev_noise = noise;
        let click = bright * 0.5 * (-t / (click_ms / 1000.0)).exp();
        // Body: a low sine whose pitch droops a little as it decays, like a key bottoming out.
        let droop = 1.0 - 0.18 * (1.0 - (-t / 0.012).exp());
        let body =
            (2.0 * std::f32::consts::PI * body_hz * droop * t).sin() * 0.7 * (-t / 0.014).exp();
        raw.push(click + body);
    }

    // Fade the tail, then scale so the loudest sample sits at PEAK.
    let fade = (SAMPLE_RATE as usize * FADE_MS / 1000).min(n);
    for k in 0..fade {
        raw[n - 1 - k] *= k as f32 / fade as f32;
    }
    let loudest = raw
        .iter()
        .fold(0.0f32, |m, s| m.max(s.abs()))
        .max(f32::EPSILON);
    raw.iter()
        .map(|s| (s / loudest * PEAK * i16::MAX as f32) as i16)
        .collect()
}

/// Wrap mono 16-bit samples in a WAV container.
pub fn wav_bytes(samples: &[i16]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    out.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes()); // byte rate
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// One scheduled tick: wait `delay`, then play `variant` at `volume`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tick {
    pub delay: Duration,
    pub variant: usize,
    pub volume: f32,
}

/// Produces the endless rhythm of a person typing.
#[derive(Debug, Clone)]
pub struct Rhythm {
    rng: Lcg,
    /// Ticks left in the current burst; zero means the next tick starts a new burst.
    left_in_burst: u32,
    last_variant: Option<usize>,
    started: bool,
}

impl Rhythm {
    pub fn new(seed: u64) -> Rhythm {
        Rhythm {
            rng: Lcg(seed ^ 0xA076_1D64_78BD_642F),
            left_in_burst: 0,
            last_variant: None,
            started: false,
        }
    }

    /// The next tick. The first one has no delay, so the user hears the key was heard.
    pub fn next_tick(&mut self) -> Tick {
        let delay_ms = if !self.started {
            self.started = true;
            self.left_in_burst = self.burst_len().saturating_sub(1);
            0.0
        } else if self.left_in_burst == 0 {
            self.left_in_burst = self.burst_len().saturating_sub(1);
            self.rng.range(280.0, 600.0)
        } else {
            self.left_in_burst -= 1;
            // Mostly quick, now and then a slightly slower key.
            let slow = self.rng.unit() < 0.15;
            if slow {
                self.rng.range(120.0, 170.0)
            } else {
                self.rng.range(70.0, 130.0)
            }
        };

        // Never repeat the same variant twice in a row.
        let mut variant = self.rng.next_u32() as usize % VARIANTS;
        if Some(variant) == self.last_variant {
            variant = (variant + 1) % VARIANTS;
        }
        self.last_variant = Some(variant);

        Tick {
            delay: Duration::from_millis(delay_ms as u64),
            variant,
            volume: self.rng.range(0.45, 0.7),
        }
    }

    fn burst_len(&mut self) -> u32 {
        3 + self.rng.next_u32() % 5
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tick_is_the_expected_length() {
        assert_eq!(key_samples(0).len(), SAMPLE_RATE as usize * TICK_MS / 1000);
    }

    #[test]
    fn ticks_are_deterministic() {
        assert_eq!(key_samples(2), key_samples(2));
    }

    #[test]
    fn variants_differ_from_each_other() {
        for a in 0..VARIANTS {
            for b in (a + 1)..VARIANTS {
                assert_ne!(key_samples(a), key_samples(b), "{a} vs {b}");
            }
        }
    }

    #[test]
    fn ticks_are_quiet_enough_for_continuous_play() {
        for v in 0..VARIANTS {
            let peak = key_samples(v)
                .iter()
                .map(|s| s.unsigned_abs())
                .max()
                .unwrap();
            assert!(
                peak as f32 <= PEAK * i16::MAX as f32 + 1.0,
                "variant {v} peak {peak}"
            );
            assert!(peak > 5_000, "variant {v} is too faint to hear: {peak}");
        }
    }

    #[test]
    fn ticks_end_at_silence_so_they_never_pop() {
        for v in 0..VARIANTS {
            let s = key_samples(v);
            assert!(s.last().unwrap().unsigned_abs() < 50, "variant {v}");
        }
    }

    #[test]
    fn a_tick_decays_most_of_its_energy_early() {
        let s = key_samples(1);
        let energy = |part: &[i16]| part.iter().map(|x| (*x as f64).powi(2)).sum::<f64>();
        let half = s.len() / 2;
        assert!(energy(&s[..half]) > 10.0 * energy(&s[half..]));
    }

    #[test]
    fn the_wav_header_is_valid() {
        let samples = key_samples(0);
        let wav = wav_bytes(&samples);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        assert_eq!(&wav[36..40], b"data");
        let riff_len = u32::from_le_bytes(wav[4..8].try_into().unwrap()) as usize;
        assert_eq!(riff_len, wav.len() - 8);
        let data_len = u32::from_le_bytes(wav[40..44].try_into().unwrap()) as usize;
        assert_eq!(data_len, samples.len() * 2);
        assert_eq!(wav.len(), 44 + data_len);
        assert_eq!(
            u32::from_le_bytes(wav[24..28].try_into().unwrap()),
            SAMPLE_RATE
        );
    }

    #[test]
    fn the_first_tick_is_immediate() {
        assert_eq!(Rhythm::new(7).next_tick().delay, Duration::ZERO);
    }

    #[test]
    fn rhythm_has_bursts_and_pauses_within_human_bounds() {
        let mut r = Rhythm::new(42);
        r.next_tick();
        let mut quick = 0;
        let mut pauses = 0;
        for _ in 0..400 {
            let t = r.next_tick();
            let ms = t.delay.as_millis();
            assert!((70..=600).contains(&ms), "delay {ms} ms is out of range");
            if ms >= 280 {
                pauses += 1;
            } else {
                quick += 1;
            }
            assert!((0.45..0.7).contains(&t.volume));
            assert!(t.variant < VARIANTS);
        }
        assert!(
            quick > pauses * 2,
            "mostly quick keys: {quick} vs {pauses} pauses"
        );
        assert!(
            pauses > 20,
            "there must be breaths between bursts: {pauses}"
        );
    }

    #[test]
    fn the_same_variant_never_plays_twice_in_a_row() {
        let mut r = Rhythm::new(3);
        let mut prev = r.next_tick().variant;
        for _ in 0..500 {
            let v = r.next_tick().variant;
            assert_ne!(v, prev);
            prev = v;
        }
    }

    #[test]
    fn different_seeds_give_different_rhythms_and_the_same_seed_repeats() {
        let run = |seed| {
            let mut r = Rhythm::new(seed);
            (0..30).map(|_| r.next_tick().delay).collect::<Vec<_>>()
        };
        assert_eq!(run(1), run(1));
        assert_ne!(run(1), run(2));
    }
}
