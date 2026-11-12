//! Compiles `src/app-window.slint` for both hosts.
//!
//! The MCU runs Slint's software renderer, which wants glyphs pre-rasterised at
//! build time (`EmbedForSoftwareRenderer`). The browser runs FemtoVG, which
//! cannot consume those textures and needs the font files instead.

fn main() {
    let is_software_renderer = std::env::var("CARGO_CFG_TARGET_ARCH")
        .map(|arch| arch == "riscv32" || arch == "arm")
        .unwrap_or(false);

    let embed = if is_software_renderer {
        slint_build::EmbedResourcesKind::EmbedForSoftwareRenderer
    } else {
        slint_build::EmbedResourcesKind::EmbedFiles
    };

    slint_build::compile_with_config(
        "src/app-window.slint",
        slint_build::CompilerConfiguration::new().embed_resources(embed),
    )
    .unwrap();
}