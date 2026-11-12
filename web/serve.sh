#!/usr/bin/env bash
# Build the browser host and serve it for local UI work.
#
#   ./web/serve.sh          # debug build, rebuild on each run
#   ./web/serve.sh --release
#
# Then open http://127.0.0.1:8000/web/
#
# Notes:
# - wasm-pack and a static file server must be on PATH (`devbox shell` provides
#   wasm-pack and python3; otherwise `nix profile install nixpkgs#wasm-pack`).
# - SLINT_DEFAULT_FONT is only needed on headless machines where fontconfig
#   finds no system font; the Slint compiler resolves `sans-serif` through it.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
port="${PORT:-8000}"

profile=--dev
if [[ "${1:-}" == "--release" ]]; then
  profile=--release
fi

cd "$root"
wasm-pack build web --target web "$profile"

echo "Serving $root on http://127.0.0.1:$port/web/"
exec python3 -I -m http.server "$port" --bind 127.0.0.1 --directory "$root"