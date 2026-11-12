//! The app object hosts construct: the compiled window plus the port wiring.

use alloc::rc::Rc;
use core::cell::RefCell;

use crate::slint::ComponentHandle as _;
use crate::sound::SoundSink;
use crate::MainWindow;

/// Owns the generated component and routes its callbacks to the host ports.
pub struct App {
    component: MainWindow,
    /// Shared with the callback handler, which holds the only other `Rc`.
    /// Nothing else reads it, so it is never borrowed here.
    _sink: Rc<RefCell<dyn SoundSink>>,
}

impl App {
    /// Builds the window. The Slint platform must already be installed; on the
    /// MCU that is [`crate`]'s host's job, on the web winit installs itself.
    pub fn new(sink: impl SoundSink + 'static) -> Result<Self, crate::slint::PlatformError> {
        let component = MainWindow::new()?;

        // `set_handler` requires `'static`, so the sink lives behind an `Rc`.
        // Slint's single-threaded contract (and the `unsafe-single-threaded`
        // feature on the MCU) makes the `RefCell` sound.
        let sink: Rc<RefCell<dyn SoundSink>> = Rc::new(RefCell::new(sink));
        let handler_sink = sink.clone();
        component.on_play_sound(move |sound| handler_sink.borrow_mut().play(sound));

        Ok(Self { component, _sink: sink })
    }

    pub fn component(&self) -> &MainWindow {
        &self.component
    }

    pub fn show(&self) -> Result<(), crate::slint::PlatformError> {
        self.component.show()
    }

    /// Runs Slint's own event loop. Only the web host uses this; the firmware
    /// drives the loop by hand so it can poll touch and push pixels over QSPI.
    pub fn run(self) -> Result<(), crate::slint::PlatformError> {
        self.component.run()
    }
}