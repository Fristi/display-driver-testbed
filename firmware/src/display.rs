//! Panel bring-up for the Waveshare ESP32-C6-Touch-AMOLED-2.16.
//!
//! Owns the QSPI wiring, the CO5300 panel spec and the static DMA buffers
//! that `EspHalQspiDevice` borrows for the lifetime of the program.

use core::cell::RefCell;

use anyhow::anyhow;
use display_driver::{
    panel::reset::NoResetPin, ColorFormat, DisplayDriver, LCDResetOption,
};
use display_driver_co5300::{
    spec::{Co5300Spec, PanelSpec},
    Co5300,
};
use display_driver_qspi::{QspiConfig, QspiDisplayBus};
use embassy_time::{Delay, Duration, Timer};
use embedded_hal_async::delay::DelayNs;
use embedded_hal_bus::i2c::RefCellDevice;
use esp_hal::{
    dma::{DmaDescriptor, DmaRxBuf},
    gpio::{Level, Output, OutputConfig},
    i2c::master::I2c,
    peripherals::{DMA_CH0, GPIO0, GPIO1, GPIO2, GPIO3, GPIO4, GPIO15, SPI2},
    spi::master::{Config as SpiConfig, Spi},
    time::Rate,
    Blocking,
};
use static_cell::StaticCell;

use crate::axp2101::Axp2101Power;
use crate::qspi::EspHalQspiDevice;

/// Waveshare ESP32-C6-Touch-AMOLED-2.16 panel: 480x480 CO5300 AMOLED.
///
/// `GenericCo5300` in the driver crate is a 240x240 placeholder with a
/// different init page, so it cannot be used for this panel.
pub struct WaveshareAmoled480x480;

impl PanelSpec for WaveshareAmoled480x480 {
    const PHYSICAL_WIDTH: u16 = 480;
    const PHYSICAL_HEIGHT: u16 = 480;
    const PHYSICAL_X_OFFSET: u16 = 0;
    const PHYSICAL_Y_OFFSET: u16 = 0;
}

impl Co5300Spec for WaveshareAmoled480x480 {
    const INIT_PAGE_PARAM: u8 = 0x00;
    const IGNORE_ID_CHECK: bool = true;
}

/// QSPI pins on the 2.16 board: CS=15, PCLK=0, D0..D3=1..4.
pub struct QspiPins {
    pub cs: GPIO15<'static>,
    pub sck: GPIO0<'static>,
    pub sio0: GPIO1<'static>,
    pub sio1: GPIO2<'static>,
    pub sio2: GPIO3<'static>,
    pub sio3: GPIO4<'static>,
}

/// Fully initialized display for the on-board panel.
pub type BoardDisplay = DisplayDriver<
    QspiDisplayBus<EspHalQspiDevice<'static>>,
    Co5300<WaveshareAmoled480x480, NoResetPin, QspiDisplayBus<EspHalQspiDevice<'static>>>,
>;

/// Bounce buffer for init commands, which live in flash and must be copied
/// to DRAM before the DMA engine can read them.
const BOUNCE_BUF_SIZE: usize = 57600;
const DMA_TX_DESCRIPTORS: usize = 8;

static TX_DESCRIPTORS: StaticCell<[DmaDescriptor; DMA_TX_DESCRIPTORS]> = StaticCell::new();
static BOUNCE_BUF: StaticCell<[u8; BOUNCE_BUF_SIZE]> = StaticCell::new();

/// Power-cycle the panel rail (ALDO3).
///
/// The 2.16 board has no LCD reset pin: Waveshare resets the panel by
/// power-cycling the ALDO3 rail. Without this the panel ignores init.
pub async fn power_cycle_panel(i2c: &RefCell<I2c<'static, Blocking>>) -> anyhow::Result<()> {
    let mut axp2101 = Axp2101Power::new(RefCellDevice::new(i2c));
    axp2101
        .init()
        .map_err(|e| anyhow!("AXP2101 init failed: {e:?}"))?;

    defmt::info!("Power-cycling panel rail (ALDO3)");
    axp2101
        .enable_aldo3()
        .map_err(|e| anyhow!("ALDO3 on failed: {e:?}"))?;
    Timer::after(Duration::from_millis(100)).await;
    axp2101
        .disable_aldo3()
        .map_err(|e| anyhow!("ALDO3 off failed: {e:?}"))?;
    Timer::after(Duration::from_millis(100)).await;
    axp2101
        .enable_aldo3()
        .map_err(|e| anyhow!("ALDO3 on failed: {e:?}"))?;
    Timer::after(Duration::from_millis(100)).await;

    Ok(())
}

/// Bring up the QSPI bus and the CO5300 panel, leaving the backlight on full.
pub async fn init(
    spi: SPI2<'static>,
    pins: QspiPins,
    dma: DMA_CH0<'static>,
    delay: &mut impl DelayNs,
) -> anyhow::Result<BoardDisplay> {
    let (rx_buffer, rx_descriptors, _, _) = esp_hal::dma_buffers!(4096, 0);
    let dma_rx_buf = DmaRxBuf::new(rx_descriptors, rx_buffer)
        .map_err(|e| anyhow!("DMA rx buffer setup failed: {e:?}"))?;

    let tx_descriptors = TX_DESCRIPTORS.init([DmaDescriptor::EMPTY; DMA_TX_DESCRIPTORS]);
    let bounce_buf = BOUNCE_BUF.init_with(|| [0; BOUNCE_BUF_SIZE]);

    let cs = Output::new(pins.cs, Level::High, OutputConfig::default());
    let spi = Spi::new(
        spi,
        SpiConfig::default()
            .with_frequency(Rate::from_mhz(80))
            .with_mode(esp_hal::spi::Mode::_0),
    )
    .map_err(|e| anyhow!("SPI init failed: {e}"))?
    .with_sck(pins.sck)
    .with_sio0(pins.sio0)
    .with_sio1(pins.sio1)
    .with_sio2(pins.sio2)
    .with_sio3(pins.sio3)
    .with_cs(cs)
    .with_dma(dma)
    .into_async();

    let device = EspHalQspiDevice {
        spi: Some(spi),
        rx_buf: Some(dma_rx_buf),
        tx_descriptors: Some(tx_descriptors),
        bounce_buf: Some(bounce_buf),
    };

    // Default QspiConfig: cmd_data_mode=Single, ram_data_mode=Quad (1-1-4)
    let bus = QspiDisplayBus::new(device, QspiConfig::default());
    let panel = Co5300::<WaveshareAmoled480x480, NoResetPin, _>::new(LCDResetOption::none());

    let mut disp = DisplayDriver::builder(bus, panel)
        .with_color_format(ColorFormat::RGB565)
        .init(delay)
        .await
        .map_err(|e| anyhow!("Panel init failed: {e:?}"))?;

    disp.set_brightness(0xff)
        .await
        .map_err(|e| anyhow!("Set brightness failed: {e:?}"))?;
    defmt::info!("Brightness set to 255.");
    Ok(disp)
}

/// Delay provider used for panel initialization.
pub fn delay() -> Delay {
    Delay
}
