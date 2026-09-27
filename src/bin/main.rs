#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use core::cell::RefCell;

use alloc::rc::Rc;
use defmt::info;
use display_co5300_test::axp2101::{self, Axp2101Power};
use display_driver::{Area, DisplayDriver, FrameControl};
use embassy_executor::Spawner;
use embassy_time::{Duration, Timer, Instant};
use esp_backtrace as _;
use esp_hal::{clock::CpuClock, i2c::master::I2c};
use esp_hal::timer::timg::TimerGroup;
use esp_println as _;

// Display driver
use display_driver_qspi::QspiDisplayBus;
use display_driver_co5300::{spec::GenericCo5300, Co5300};

// Slint
slint::include_modules!();
use slint::platform::software_renderer::Rgb565Pixel;


extern crate alloc;

// Allocator
const HEAP_SIZE: usize = 256 * 1024;
static mut HEAP: [u8; HEAP_SIZE] = [0; HEAP_SIZE];
#[global_allocator]
static ALLOCATOR: embedded_alloc::Heap = embedded_alloc::Heap::empty();

// Double buffer allocation to satisfy the 2-line write requirement
static mut LINE_BUFFER: [Rgb565Pixel; 480 as usize * 2] =
    [Rgb565Pixel(0); 480 as usize * 2];

/// Slint platform implementation for our MCU
struct McuPlatform {
    window: Rc<slint::platform::software_renderer::MinimalSoftwareWindow>,
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
        // info!("{}", defmt::Debug2Format(&arguments));
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

// This creates a default app-descriptor required by the esp-idf bootloader.
// For more information see: <https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/system/app_image_format.html#application-description>
esp_bootloader_esp_idf::esp_app_desc!();

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[esp_rtos::main]
async fn main(_spawner: Spawner) -> ! {

    unsafe { ALLOCATOR.init(core::ptr::addr_of_mut!(HEAP) as usize, HEAP_SIZE) }


    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    // esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 65536);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let sw_interrupt =
        esp_hal::interrupt::software::SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_interrupt.software_interrupt0);

    info!("Embassy initialized!");

    // Pins: https://github.com/waveshareteam/ESP32-C6-Touch-AMOLED-2.16/blob/main/02_Example/XiaoZhi-v2.2.5/main/boards/waveshare/esp32-c6-touch-amoled-2.06/config.h#L32


    // peripherals.I2C0  →  I2c driver (implements embedded_hal::i2c::I2c)
    let i2c = I2c::new(peripherals.I2C0, esp_hal::i2c::master::Config::default().with_frequency(esp_hal::time::Rate::from_khz(400)))
        .expect("i2c failed")
        .with_sda(peripherals.GPIO8)
        .with_scl(peripherals.GPIO7);

    let i2c_refcell = RefCell::new(i2c);

    let disp_dev = embedded_hal_bus::i2c::RefCellDevice::new(&i2c_refcell);
    let mut axp2101 = Axp2101Power::new(disp_dev);

    axp2101.init().expect("Failed to init AXP2101");

    let (rx_buffer, rx_descriptors, _, _) = esp_hal::dma_buffers!(4096, 0);
    let dma_rx_buf = esp_hal::dma::DmaRxBuf::new(rx_descriptors, rx_buffer).unwrap();


    static TX_DESCRIPTORS: static_cell::StaticCell<[esp_hal::dma::DmaDescriptor; 8]> = static_cell::StaticCell::new();
    let tx_descriptors = TX_DESCRIPTORS.init([esp_hal::dma::DmaDescriptor::EMPTY; 8]);

   // Bounce buffer for Flash-based initialization commands
    static BOUNCE_BUF: static_cell::StaticCell<[u8; 57600]> = static_cell::StaticCell::new();
    let bounce_buf = BOUNCE_BUF.init([0; 57600]);
    
    let cs_pin = esp_hal::gpio::Output::new(peripherals.GPIO15, esp_hal::gpio::Level::High, esp_hal::gpio::OutputConfig::default());
    
    let spi = esp_hal::spi::master::Spi::new(
        peripherals.SPI2,
        esp_hal::spi::master::Config::default()
            .with_frequency(esp_hal::time::Rate::from_mhz(80)) // Using 10MHz for breadboard stability
            .with_mode(esp_hal::spi::Mode::_0),
    )
    .unwrap()
    .with_sck(peripherals.GPIO0)
    .with_sio0(peripherals.GPIO1)
    .with_sio1(peripherals.GPIO2)
    .with_sio2(peripherals.GPIO3)
    .with_sio3(peripherals.GPIO4)
    .with_cs(cs_pin)
    .with_dma(peripherals.DMA_CH0)
    .into_async();


  // ── Display bus & panel ────────────────────────────────────────────────
    let device = display_co5300_test::qspi::EspHalQspiDevice {
        spi: Some(spi),
        rx_buf: Some(dma_rx_buf),
        tx_descriptors: Some(tx_descriptors),
        bounce_buf: Some(bounce_buf)
    };

    // Default QspiConfig: cmd_data_mode=Single, ram_data_mode=Quad (1-1-4)
    let bus = QspiDisplayBus::new(device, display_driver_qspi::QspiConfig::default());
    let panel = Co5300::<GenericCo5300, _, _>::new(display_driver::LCDResetOption::none());


    // ── Run async display init & drawing ───────────────────────────────────
    // Initialise display driver
    let mut disp = display_driver::DisplayDriver::builder(bus, panel)
        .with_color_format(display_driver::ColorFormat::RGB565)
        .init(&mut embassy_time::Delay)
        .await
        .unwrap();    
    
    // disp.set_brightness(0xff).await.unwrap();

    // fb_disp.set_brightness(0xff).await.unwrap();
    // info!("Brightness set to 255.");

// --- Slint Platform Setup ---
    info!("Slint Platform Init");
    let window = slint::platform::software_renderer::MinimalSoftwareWindow::new(Default::default());
    window.set_size(slint::PhysicalSize::new(
        480,
        480
    ));

    slint::platform::set_platform(alloc::boxed::Box::new(McuPlatform {
        window: window.clone(),
        start_time: Instant::now(),
    }))
    .unwrap();

    // --- UI Creation ---
    let _ui = MainWindow::new().unwrap();

    info!("Entering idle loop.");

    loop {
        info!("Start a frame");
        slint::platform::update_timers_and_animations();

        // State tracker for the double buffering logic
        let mut last_even_line_index: Option<usize> = None;

        window.draw_if_needed(|renderer| {
            unsafe {
                renderer.render_by_line(DisplayWrapper {
                    display: &mut disp,
                    line_buffer: &mut LINE_BUFFER,
                    last_even_line: &mut last_even_line_index,
                })
            };
        });

        info!("Finished a frame");
        if !window.has_active_animations() {
            if let Some(duration) = slint::platform::duration_until_next_timer_update() {
                let embassy_duration = Duration::from_micros(duration.as_micros() as u64);
                Timer::after(embassy_duration).await;
            } else {
                Timer::after(Duration::from_millis(10)).await;
            }
        }
        Timer::after(Duration::from_secs(1)).await;
    }

}

struct DisplayWrapper<'a, D> {
    display: &'a mut D,
    /// Buffer capable of holding exactly 2 lines of pixel data.
    line_buffer: &'a mut [slint::platform::software_renderer::Rgb565Pixel],
    /// Tracks the line index of the previously written even line.
    last_even_line: &'a mut Option<usize>,
}

impl<'a, B: display_driver::DisplayBus, P: display_driver::Panel<B>>
    slint::platform::software_renderer::LineBufferProvider
    for DisplayWrapper<'a, DisplayDriver<B, P>>
{
    type TargetPixel = slint::platform::software_renderer::Rgb565Pixel;

    fn process_line(
        &mut self,
        line: usize,
        range: core::ops::Range<usize>,
        render_fn: impl FnOnce(&mut [Self::TargetPixel]),
    ) {
        let width = range.len();

        if line % 2 == 0 {
            // --- Handling Even Line ---

            // Check if we are overwriting a previous even line that never got its odd pair
            if let Some(prev) = *self.last_even_line {
                defmt::warn!("Discarding data: New Even line {} arrived while Even line {} was still pending", line, prev);
            }

            // Store state indicating this even line is buffered
            *self.last_even_line = Some(line);

            // Write to the FIRST half of the buffer (Offset 0)
            // Memory Layout: [ <Even Line Data> | ... empty ... ]
            render_fn(&mut self.line_buffer[0..range.end]);
        } else {
            // --- Handling Odd Line ---

            match *self.last_even_line {
                Some(prev_line) if prev_line == line - 1 => {
                    // Valid pair sequence (e.g., Line 0 followed by Line 1)

                    // Write to the SECOND half of the buffer, immediately following the even line.
                    // Memory Layout: [ <Even Line Data> | <Odd Line Data> ]
                    render_fn(&mut self.line_buffer[width..width + range.end]);

                    // Construct Area covering exactly 2 lines
                    let area = Area {
                        x: range.start as u16,
                        y: prev_line as u16, // Start from the Even line's Y
                        w: width as u16,
                        h: 2, // Strictly 2 lines
                    };

                    // Slice the buffer to include exactly both lines (2 * width)
                    let buffer_slice = &self.line_buffer[0..width * 2];
                    let byte_buffer: &[u8] = bytemuck::cast_slice(buffer_slice);

                    // unsafe {
                    //     cortex_m::Peripherals::steal()
                    //         .SCB
                    //         .clean_dcache_by_slice(byte_buffer);
                    // }

                    // Perform the transfer
                    embassy_futures::block_on(self.display.write_pixels(
                        area,
                        FrameControl::new_standalone(),
                        &byte_buffer[1..] //TODO: off by one? Which layer does this introduce? Suspect slint: https://github.com/jphein/esp32c6-watch/tree/03742a9db7cda4b60119bf6ab80fcf680be78594/crates/i-slint-renderer-software
                        // &byte_buffer
                    ))
                    .ok();
                }
                Some(prev_line) => {
                    defmt::warn!(
                        "Discarding data: Odd line {} follows unrelated Even line {}",
                        line,
                        prev_line
                    );
                }
                None => {
                    defmt::warn!(
                        "Discarding data: Odd line {} arrived without preceding Even line",
                        line
                    );
                }
            }

            // Whether successful or failed, the pair sequence is now concluded/broken.
            *self.last_even_line = None;
        }
    }
}