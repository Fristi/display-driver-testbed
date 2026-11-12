//! The one hardware-facing port the UI needs today.
//!
//! The voices are modelled here as cues rather than samples: the firmware will
//! later turn them into PCM on I2S, the browser can stay silent. `Sound` is the
//! enum generated from `enum Sound` in `app-window.slint`, so there is no
//! conversion layer between the UI and the host.

pub use crate::Sound;

/// Plays the cues the breathing session emits.
pub trait SoundSink {
    fn play(&mut self, sound: Sound);
}

/// Discards every cue. Used by hosts that have no audio yet.
pub struct NoopSink;

impl SoundSink for NoopSink {
    fn play(&mut self, _sound: Sound) {}
}

impl SoundSink for () {
    fn play(&mut self, _sound: Sound) {}
}