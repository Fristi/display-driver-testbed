//! Drives the real component headlessly and asserts the sound port fires at the
//! right moments in the breathing state machine.
//!
//! Slint's published `i-slint-backend-testing` crate does not compile from crates.io
//! (it `include_bytes!`s test fonts that are not shipped), so this file brings its
//! own minimal platform: a `MinimalSoftwareWindow` plus a mock clock, mirroring
//! what `firmware/src/platform.rs` does on the ESP32.
//!
//! Everything runs in one `#[test]` because Slint's global platform can only be
//! installed once per process.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use ui::slint::platform::software_renderer::MinimalSoftwareWindow;
use ui::slint::platform::{Platform, WindowAdapter};
use ui::slint::{ComponentHandle as _, PhysicalSize};
use ui::{App, Screen, SoundSink};

// Mock clock: `mock_elapsed_time` moves it forward, so timers and animations
// advance deterministically.
thread_local! {
    static MOCK_NOW_MS: Cell<u64> = const { Cell::new(0) };
}

fn mock_elapsed_time(ms: u64) {
    MOCK_NOW_MS.with(|now| now.set(now.get() + ms));
}

struct HeadlessPlatform {
    window: Rc<MinimalSoftwareWindow>,
}

impl Platform for HeadlessPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, ui::slint::PlatformError> {
        Ok(self.window.clone())
    }

    fn duration_since_start(&self) -> core::time::Duration {
        core::time::Duration::from_millis(MOCK_NOW_MS.with(Cell::get))
    }

    fn debug_log(&self, _arguments: core::fmt::Arguments) {}
}

fn install_platform() {
    let window = MinimalSoftwareWindow::new(Default::default());
    window.set_size(PhysicalSize::new(480, 480));
    ui::slint::platform::set_platform(Box::new(HeadlessPlatform { window })).unwrap();
}

#[derive(Default, Clone)]
struct Recorder(Rc<RefCell<Vec<ui::Sound>>>);

impl SoundSink for Recorder {
    fn play(&mut self, sound: ui::Sound) {
        self.0.borrow_mut().push(sound);
    }
}

impl Recorder {
    fn take(&self) -> Vec<ui::Sound> {
        core::mem::take(&mut self.0.borrow_mut())
    }
}

/// Ticks Slint's timers and animations, one second per step.
fn run_for(seconds: u32) {
    for _ in 0..seconds {
        mock_elapsed_time(1000);
        ui::slint::platform::update_timers_and_animations();
    }
}

/// Starts a session and lets the 120 ms arming timer fire, which is what
/// actually begins the first inhale.
fn start_session(pattern: i32, minutes: i32, recorder: &Recorder) -> App {
    let app = App::new(recorder.clone()).unwrap();
    app.component().set_selected_pattern(pattern);
    app.component().set_selected_duration(minutes);
    app.show().unwrap();
    app.component().invoke_start_session_now();
    mock_elapsed_time(200);
    app
}

#[test]
fn sound_port_follows_the_breathing_state_machine() {
    install_platform();

    // 6-0-2-0 for one minute: holds are skipped, so only inhale/exhale fire.
    let recorder = Recorder::default();
    let app = start_session(0, 1, &recorder);
    assert_eq!(app.component().get_screen(), Screen::Session);

    // The first 1 s tick lands one second after arming, so a 60 s session needs
    // a little over 60 s of clock. Run a few extra seconds; the timer stops
    // itself once the session finishes.
    run_for(65);
    let cues = recorder.take();

    assert_eq!(cues.first(), Some(&ui::Sound::BreathIn), "{cues:?}");
    assert_eq!(cues.last(), Some(&ui::Sound::Ding), "{cues:?}");
    assert!(
        !cues.contains(&ui::Sound::Hold),
        "zero-second phases must stay silent: {cues:?}"
    );

    // 6 s inhale + 2 s exhale = 8 s per cycle: one inhale at t=0 and each cycle.
    let breaths = cues.iter().filter(|c| **c == ui::Sound::BreathIn).count();
    let exhales = cues.iter().filter(|c| **c == ui::Sound::BreathOut).count();
    assert_eq!(breaths, 8, "{cues:?}");
    assert_eq!(exhales, 7, "{cues:?}");

    // 4-4-4-4 ("Box") does have holds, and they must be audible.
    let recorder = Recorder::default();
    start_session(2, 1, &recorder);
    // Inhale 4 s, so the hold begins on the 4th one-second tick after arming.
    run_for(5);
    assert_eq!(recorder.take(), [ui::Sound::BreathIn, ui::Sound::Hold]);

    // Leaving the session screen stops the clock and must not ding.
    let recorder = Recorder::default();
    let app = start_session(0, 1, &recorder);
    recorder.take();
    app.component().set_screen(Screen::Start);
    run_for(5);
    let leftover = recorder.take();
    assert!(leftover.is_empty(), "closing must stay silent, got {leftover:?}");
}
