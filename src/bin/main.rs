#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use core::cell::RefCell;

use defmt::info;
use display_co5300_test::axp2101::{self, Axp2101Power};
use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use esp_backtrace as _;
use esp_hal::{clock::CpuClock, i2c::master::I2c};
use esp_hal::timer::timg::TimerGroup;
use esp_println as _;

use embedded_graphics::{
    framebuffer::{buffer_size, Framebuffer},
    geometry::{Point, Size},
    mono_font::{ascii::FONT_9X18_BOLD, MonoTextStyle},
    pixelcolor::{
        raw::{BigEndian, RawU16},
        Rgb565,
    },
    prelude::*,
    primitives::{Circle, PrimitiveStyle, Rectangle},
    text::{Alignment, Text},
};

// Display driver
use display_driver_qspi::QspiDisplayBus;
use display_driver_co5300::{spec::GenericCo5300, Co5300};


extern crate alloc;

// ---------------------------------------------------------------------------
// Display geometry
// ---------------------------------------------------------------------------

const WIDTH: usize = 240;
const HEIGHT: usize = 240;

type FbType = Framebuffer<
    Rgb565,
    RawU16,
    BigEndian,
    WIDTH,
    HEIGHT,
    { buffer_size::<Rgb565>(WIDTH, HEIGHT) },
>;

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

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 65536);

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

    // ── Framebuffer (static, 240×240 RGB565 = 115 200 bytes) ──────────────
    static mut FB_DATA: core::mem::MaybeUninit<FbType> = core::mem::MaybeUninit::uninit();
    let fb = unsafe {
        let ptr = core::ptr::addr_of_mut!(FB_DATA) as *mut FbType;
        // Zero-initialize directly in memory to avoid stack copy
        core::ptr::write_bytes(ptr, 0, 1);
        &mut *ptr
    };

    // ── Run async display init & drawing ───────────────────────────────────
    // Initialise display driver
    let disp = display_driver::DisplayDriver::builder(bus, panel)
        .with_color_format(display_driver::ColorFormat::RGB565)
        .init(&mut embassy_time::Delay)
        .await
        .unwrap();    
    

 // Wrap with framebuffer for embedded-graphics drawing
    let mut fb_disp = display_co5300_test::framebuffered::FrameBufferedDisplayDriver::new(disp, fb);


    fb_disp.set_brightness(0xff).await.unwrap();
    info!("Brightness set to 255.");

    // ── Draw demo scene ──────────────────────────────────────────
    fb_disp.clear(Rgb565::WHITE).unwrap();

    // Three overlapping coloured shapes
    Rectangle::new(Point::new(10, 40), Size::new(100, 100))
        .into_styled(PrimitiveStyle::with_fill(Rgb565::RED))
        .draw(&mut fb_disp)
        .unwrap();

    Rectangle::new(Point::new(70, 70), Size::new(100, 100))
        .into_styled(PrimitiveStyle::with_fill(Rgb565::GREEN))
        .draw(&mut fb_disp)
        .unwrap();

    Circle::new(Point::new(110, 110), 110)
        .into_styled(PrimitiveStyle::with_fill(Rgb565::BLUE))
        .draw(&mut fb_disp)
        .unwrap();

    // Title text
    let title = MonoTextStyle::new(&FONT_9X18_BOLD, Rgb565::WHITE);
    Text::with_alignment("CO5300 QSPI", Point::new(120, 22), title, Alignment::Center)
        .draw(&mut fb_disp)
        .unwrap();

    // Subtitle
    let sub = MonoTextStyle::new(&FONT_9X18_BOLD, Rgb565::CYAN);
    Text::with_alignment("ESP32-S3 display-driver", Point::new(120, 232), sub, Alignment::Center)
        .draw(&mut fb_disp)
        .unwrap();

    // Flush framebuffer → display
    info!("Flushing framebuffer to display (in chunks)...");

    // fb_disp.flush().await.unwrap();
    
    fb_disp.flush_lines_with_frame_control(0, 59, display_driver::FrameControl::new_first()).await.unwrap();
    fb_disp.flush_lines_with_frame_control(60, 119, display_driver::FrameControl { first: false, last: false }).await.unwrap();
    fb_disp.flush_lines_with_frame_control(120, 179, display_driver::FrameControl { first: false, last: false }).await.unwrap();
    fb_disp.flush_lines_with_frame_control(180, 239, display_driver::FrameControl::new_last()).await.unwrap();
    
    info!("Done! Display should show the demo scene.");

    info!("Entering idle loop.");

    loop {
        let s = axp2101.read_chip_id().unwrap();
        let v = axp2101.is_vbus_in().unwrap();

        info!("Status : and {} and {}", s, v);

        Timer::after(Duration::from_secs(1)).await;
    }

}
