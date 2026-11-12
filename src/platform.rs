//! Minimal Slint platform implementation for the ESP32-C6 main loop.

use alloc::rc::Rc;

use anyhow::anyhow;
use embassy_time::Instant;
use slint::platform::software_renderer::MinimalSoftwareWindow;

/// Slint platform backed by a single [`MinimalSoftwareWindow`].
struct McuPlatform {
    window: Rc<MinimalSoftwareWindow>,
    start_time: Instant,
}

impl slint::platform::Platform for McuPlatform {
    fn create_window_adapter(
        &self,
    ) -> Result<Rc<dyn slint::platform::WindowAdapter>, slint::PlatformError> {
        Ok(self.window.clone())
    }

    fn duration_since_start(&self) -> core::time::Duration {
        self.start_time.elapsed().into()
    }

    fn debug_log(&self, _arguments: core::fmt::Arguments) {
        // Slint's log output is dropped to keep the binary small.
    }
}

/// Install the platform and return the window the UI renders into.
///
/// Must be called once, before any Slint component is created.
pub fn init(width: u32, height: u32) -> anyhow::Result<Rc<MinimalSoftwareWindow>> {
    let window = MinimalSoftwareWindow::new(Default::default());
    window.set_size(slint::PhysicalSize::new(width, height));

    slint::platform::set_platform(alloc::boxed::Box::new(McuPlatform {
        window: window.clone(),
        start_time: Instant::now(),
    }))
    .map_err(|e| anyhow!("Slint platform init failed: {e}"))?;

    Ok(window)
}
