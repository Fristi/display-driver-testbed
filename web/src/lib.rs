//! Browser host for the breathing UI.
//!
//! Built with `wasm-pack build web --target web`. The winit backend installs
//! itself and renders into the `<canvas id="canvas">` from `web/index.html`
//! through FemtoVG/WebGL, so there is no platform to set up here.

#[cfg_attr(target_family = "wasm", wasm_bindgen::prelude::wasm_bindgen(start))]
pub fn main() -> Result<(), String> {
    ui::App::new(ui::NoopSink)
        .and_then(|app| app.run())
        .map_err(|e| format!("{e}"))
}