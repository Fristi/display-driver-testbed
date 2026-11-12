#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

extern crate alloc;

use core::cell::RefCell;

use anyhow::anyhow;
use cst92xx::{CST92xx, TouchConfig};
use defmt::{error, info, warn};
use display_co5300_test::{display, platform, renderer};
use display_co5300_test::touch::TouchTracker;
use embassy_executor::Spawner;
use embassy_time::{Duration, Instant, Timer};
use esp_backtrace as _;
use esp_hal::{
    clock::CpuClock,
    gpio::{Level, Output, OutputConfig},
    i2c::master::{Config as I2cConfig, I2c},
    timer::timg::TimerGroup,
};
use esp_println as _;

slint::include_modules!();

// Allocator
const HEAP_SIZE: usize = 256 * 1024;
static mut HEAP: [u8; HEAP_SIZE] = [0; HEAP_SIZE];
#[global_allocator]
static ALLOCATOR: embedded_alloc::Heap = embedded_alloc::Heap::empty();

const PANEL_SIZE: u32 = 480;

/// How long the loop may sleep between iterations while no finger is down.
const TOUCH_POLL_IDLE: Duration = Duration::from_millis(20);
/// While a finger is down Slint animates the pressed state and gestures can
/// outrun a 20 ms sample; poll faster so drags feel responsive.
const TOUCH_POLL_ACTIVE: Duration = Duration::from_millis(10);
/// Animations redraw continuously; each frame is a 480x480 QSPI transfer, so
/// ~30 fps keeps the bus and the CPU from being saturated.
const MAX_FRAME_RATE: Duration = Duration::from_millis(33);

// This creates a default app-descriptor required by the esp-idf bootloader.
// For more information see: <https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/system/app_image_format.html#application-description>
esp_bootloader_esp_idf::esp_app_desc!();

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    unsafe { ALLOCATOR.init(core::ptr::addr_of_mut!(HEAP) as usize, HEAP_SIZE) }

    match run(spawner).await {
        Ok(()) => error!("Main loop exited unexpectedly"),
        // Debug prints the full anyhow cause chain; Display would only show the top message.
        Err(err) => error!("Fatal: {:?}", defmt::Debug2Format(&err)),
    }

    // Halt in place: resetting would only repeat the same failure.
    loop {
        Timer::after(Duration::from_secs(3600)).await;
    }
}

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
async fn run(_spawner: Spawner) -> anyhow::Result<()> {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let sw_interrupt =
        esp_hal::interrupt::software::SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_interrupt.software_interrupt0);

    info!("Embassy initialized!");

    // ── Shared I2C bus (AXP2101 + touch controller) ────────────────────────
    // Pins from the board schematic: SDA=GPIO8, SCL=GPIO7.
    let i2c = I2c::new(
        peripherals.I2C0,
        I2cConfig::default().with_frequency(esp_hal::time::Rate::from_khz(400)),
    )
    .map_err(|e| anyhow!("I2C init failed: {e}"))?
    .with_sda(peripherals.GPIO8)
    .with_scl(peripherals.GPIO7);
    let i2c_refcell = RefCell::new(i2c);

    // ── Display ────────────────────────────────────────────────────────────
    display::power_cycle_panel(&i2c_refcell).await?;

    let pins = display::QspiPins {
        cs: peripherals.GPIO15,
        sck: peripherals.GPIO0,
        sio0: peripherals.GPIO1,
        sio1: peripherals.GPIO2,
        sio2: peripherals.GPIO3,
        sio3: peripherals.GPIO4,
    };
    let mut disp = display::init(peripherals.SPI2, pins, peripherals.DMA_CH0, &mut display::delay())
        .await?;

    // ── Touch controller (CST9220) ─────────────────────────────────────────
    // Shares I2C0 with the AXP2101. `TP_INT` (GPIO5) is not used: the
    // controller is polled from the render loop instead, in the same thread,
    // which keeps the shared I2C bus free of cross-task locking.
    let touch_rst = Output::new(peripherals.GPIO11, Level::High, OutputConfig::default());
    let touch_dev = embedded_hal_bus::i2c::RefCellDevice::new(&i2c_refcell);
    let mut touch = CST92xx::new(touch_dev, esp_hal::delay::Delay::new())
        .with_reset(touch_rst)
        // Scale whatever the panel reports to the 480x480 Slint window.
        .with_config(TouchConfig::default().with_target_resolution(480, 480));

    match touch.init() {
        Ok(()) => info!("Touch controller ready: {}", touch.model_name()),
        Err(e) => error!("Touch init failed: {:?}", e),
    }

    let mut touch_tracker = TouchTracker::new();
    let mut last_touch_poll = Instant::now();

    // ── Slint UI ───────────────────────────────────────────────────────────
    let window = platform::init(PANEL_SIZE, PANEL_SIZE)?;

    let ui = MainWindow::new().map_err(|e| anyhow!("Window creation failed: {e}"))?;
    // MinimalSoftwareWindow only renders once it is visible and dirty.
    ui.show().map_err(|e| anyhow!("Window show failed: {e}"))?;
    window.request_redraw();

    info!("Entering idle loop.");

    loop {
        slint::platform::update_timers_and_animations();

        // --- Touch input ---
        let poll_interval = if touch_tracker.is_active() {
            TOUCH_POLL_ACTIVE
        } else {
            TOUCH_POLL_IDLE
        };
        if last_touch_poll.elapsed() >= poll_interval {
            last_touch_poll = Instant::now();
            match touch.touches() {
                Ok(points) => {
                    if let Some(event) = touch_tracker.update(&points) {
                        renderer::dispatch_touch(&window, event);
                    }
                }
                Err(e) => warn!("Touch read failed: {:?}", e),
            }
        }

        renderer::draw(&mut disp, &window);

        let mut wait = if window.has_active_animations() {
            MAX_FRAME_RATE
        } else if let Some(duration) = slint::platform::duration_until_next_timer_update() {
            // Sleep until the next Slint timer (e.g. the session countdown).
            Duration::from_micros(duration.as_micros() as u64)
        } else {
            // Nothing scheduled: poll occasionally so input still gets a chance.
            Duration::from_millis(50)
        };

        // Never sleep past the next touch sample.
        let until_poll = poll_interval
            .checked_sub(last_touch_poll.elapsed())
            .unwrap_or(Duration::MIN);
        if until_poll < wait {
            wait = until_poll;
        }
        if wait > Duration::MIN {
            Timer::after(wait).await;
        }
    }
}
