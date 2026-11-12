# display-co5300-test

480x480 Slint breathing UI for the CO5300 QSPI panel with CST9220 touch. The
same UI runs on the ESP32-C6 board and in a browser, so layout work does not
need a flash cycle.

## Layout

| Crate      | Target        | Purpose                                              |
| ---------- | ------------- | ---------------------------------------------------- |
| `ui`       | host + both   | Slint window, breathing state machine, `SoundSink`   |
| `firmware` | ESP32-C6      | QSPI display, touch polling, software renderer       |
| `web`      | wasm32        | winit + FemtoVG host                                 |

`ui` re-exports Slint as `ui::slint`; hosts must not depend on `slint` directly,
because the feature set differs per target.

## Run in the browser

```sh
./web/serve.sh           # debug
./web/serve.sh --release
```

Then open <http://127.0.0.1:8000/web/>. `PORT=9000 ./web/serve.sh` changes the
port. Needs `wasm-pack` and `python3` on `PATH`.

The script runs `wasm-pack build web --target web` and serves the repo root so
`web/index.html` can load `web/pkg/`. wasm-opt is disabled in
`web/Cargo.toml`, so it works offline.

## Run on hardware

```sh
cd firmware
cargo run --release
```

The `espflash` runner from `firmware/.cargo/config.toml` flashes and opens a
defmt monitor (`DEFMT_LOG=info`). Build from `firmware/` so Cargo picks up that
config: it holds the `riscv32imac-unknown-none-elf` target and
`build-std = ["alloc", "core"]`. The `cargo esp` alias at the repo root sets the
target but not `build-std`, so it fails to find `core`; use the directory.

Needs `espflash` and an ESP32-C6 in bootloader mode.

## Tests

```sh
cargo test -p ui
```

Host tests live in `ui/tests/`; they drive the real component through a headless
`MinimalSoftwareWindow` plus a mock clock. `cargo test` for `firmware` cannot run
at all, since `build-std` has no `test` crate.

## Running in this sandbox

The nix toolchain has no `rustlib/src`, `lld`, or a system font, so build steps
need these environment overrides:

```sh
export PATH="/nix/store/31igim4lq41cf1zlrlphfhv807z9iw2b-lld-21.1.8/bin:$PATH"
export RUSTC_BOOTSTRAP=1
export __CARGO_TESTS_ONLY_SRC_ROOT=/nix/store/vksm9ykgwcm0c6ai06f8bb8c5ka675zn-rust-lib-src
export SLINT_DEFAULT_FONT=$HOME/.fonts/Inter-VariableFont.ttf
```

- `PATH`: both the riscv ELF and the wasm cdylib need `lld` to link.
- `RUSTC_BOOTSTRAP` + `__CARGO_TESTS_ONLY_SRC_ROOT`: give `build-std` a source
  root. Refresh the path with
  `nix build --extra-experimental-features 'nix-command flakes' --print-out-paths nixpkgs#rustPlatform.rustLibSrc`.
- `SLINT_DEFAULT_FONT`: the Slint compiler otherwise fails with "could not
  determine a default font for sans-serif".

`wasm-pack` is not on `PATH` by default either; prepend
`/nix/store/h2dc9xl0ixdk2mkb01rchr9kvfqjldfr-wasm-pack-0.15.1/bin`. Store
paths are machine-specific; the listings above are what this container resolves
to.

`cargo test -p ui`, `cargo build` in `firmware/`, and `wasm-pack build` were all
verified working with these set. `cargo run` in `firmware/` still needs
`espflash`, which is not installed here.
