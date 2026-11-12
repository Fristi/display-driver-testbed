//! Scent-guided breathing UI.
//!
//! This crate owns the Slint frontend plus the ports the UI needs from its
//! host. It is `no_std` + `alloc` so the same code runs on the ESP32 (software
//! renderer) and in a browser (winit + FemtoVG). Hosts supply the platform and
//! a [`SoundSink`].

#![no_std]
extern crate alloc;

slint::include_modules!();

pub use slint;

mod app;
mod sound;

pub use app::App;
pub use sound::{NoopSink, SoundSink};