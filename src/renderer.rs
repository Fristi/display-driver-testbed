//! Slint software renderer -> CO5300 line bridge.
//!
//! Slint hands out one line at a time. The CO5300 wants DMA transfers that
//! cover at least two lines, so [`DisplayWrapper`] buffers an even line until
//! its odd partner arrives and then flushes both as a single 2-line window.

use display_driver::{Area, DisplayBus, DisplayDriver, FrameControl, Panel};
use slint::platform::software_renderer::{LineBufferProvider, MinimalSoftwareWindow, Rgb565Pixel};

/// Panel width in pixels, and the stride of one line in [`LINE_BUFFER`].
const PANEL_W: usize = 480;

/// Two lines of scratch space, shared with Slint through `&mut`.
static mut LINE_BUFFER: [Rgb565Pixel; PANEL_W * 2] = [Rgb565Pixel(0); PANEL_W * 2];

/// Redraw the window into `display` if Slint marked it dirty.
///
/// Returns whether anything was drawn.
pub fn draw<B, P>(display: &mut DisplayDriver<B, P>, window: &MinimalSoftwareWindow) -> bool
where
    B: DisplayBus,
    P: Panel<B>,
{
    // SAFETY: the renderer is only driven from the single main loop, and the
    // buffer is not aliased for the duration of the borrow.
    let line_buffer = unsafe { &mut *core::ptr::addr_of_mut!(LINE_BUFFER) };

    window.draw_if_needed(|renderer| {
        renderer.render_by_line(DisplayWrapper {
            display,
            line_buffer,
            last_even_line: None,
        });
    })
}

/// Forward a touch edge to Slint as a mouse event.
///
/// Slint's software renderer only reacts to press/release for `TouchArea`
/// clicks, so a contact maps naturally onto the left mouse button. Coordinates
/// are physical pixels, converted to Slint's logical space (scale factor 1).
pub fn dispatch_touch(window: &MinimalSoftwareWindow, event: crate::touch::TouchEvent) {
    use crate::touch::TouchEvent;
    use slint::platform::{PointerEventButton, WindowEvent};

    let kind = match event {
        TouchEvent::Press { x, y } => WindowEvent::PointerPressed {
            position: slint::LogicalPosition::new(x as f32, y as f32),
            button: PointerEventButton::Left,
        },
        TouchEvent::Move { x, y } => WindowEvent::PointerMoved {
            position: slint::LogicalPosition::new(x as f32, y as f32),
        },
        TouchEvent::Release { x, y } => WindowEvent::PointerReleased {
            position: slint::LogicalPosition::new(x as f32, y as f32),
            button: PointerEventButton::Left,
        },
    };

    let _ = window.dispatch_event_with_result(kind);

    // A press/move/release sequence dirties the items under the pointer.
    window.request_redraw();
}

/// Per-frame state for [`DisplayWrapper`].
pub struct DisplayWrapper<'a, D> {
    display: &'a mut D,
    /// Buffer capable of holding exactly 2 lines of pixel data.
    line_buffer: &'a mut [Rgb565Pixel],
    /// Tracks the line index of the previously written even line.
    last_even_line: Option<usize>,
}

impl<B: DisplayBus, P: Panel<B>> LineBufferProvider for DisplayWrapper<'_, DisplayDriver<B, P>> {
    type TargetPixel = Rgb565Pixel;

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
            if let Some(prev) = self.last_even_line {
                defmt::warn!(
                    "Discarding data: New Even line {} arrived while Even line {} was still pending",
                    line,
                    prev
                );
            }

            // Store state indicating this even line is buffered
            self.last_even_line = Some(line);

            // Write to the FIRST half of the buffer (Offset 0), placed at the
            // span's x offset so partial spans land in the right columns.
            // Memory Layout: [ <Even Line Data> | ... empty ... ]
            render_fn(&mut self.line_buffer[range.start..range.end]);
        } else {
            // --- Handling Odd Line ---

            match self.last_even_line {
                Some(prev_line) if prev_line == line - 1 => {
                    // Valid pair sequence (e.g., Line 0 followed by Line 1)

                    // Write to the SECOND half of the buffer, one full panel
                    // stride after the even line.
                    // Memory Layout: [ <Even Line Data> | <Odd Line Data> ]
                    render_fn(&mut self.line_buffer[PANEL_W + range.start..PANEL_W + range.end]);

                    // Construct Area covering exactly 2 lines
                    let area = Area {
                        x: range.start as u16,
                        y: prev_line as u16, // Start from the Even line's Y
                        w: width as u16,
                        h: 2, // Strictly 2 lines
                    };

                    // Slice the buffer to include exactly both lines (2 * PANEL_W)
                    let buffer_slice = &mut self.line_buffer[0..PANEL_W * 2];

                    // CO5300 wants big-endian RGB565; swap in SRAM so the DMA
                    // stays zero-copy and aligned.
                    for p in buffer_slice.iter_mut() {
                        p.0 = p.0.swap_bytes();
                    }

                    let byte_buffer: &[u8] = bytemuck::cast_slice(buffer_slice);

                    // Perform the transfer
                    if embassy_futures::block_on(self.display.write_pixels(
                        area,
                        FrameControl::new_standalone(),
                        byte_buffer,
                    ))
                    .is_err()
                    {
                        defmt::warn!("write_pixels FAILED at y={}", area.y);
                    }
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
            self.last_even_line = None;
        }
    }
}
