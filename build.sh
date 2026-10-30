#!/usr/bin/env bash
# Build the whole project:
#   reda                   the command line, every tool (target/release/reda)
#   web/dist               the web pages (veryl/wasm and riscv/rvemu compiled to wasm32 + TypeScript)
#
#   ./build.sh            everything
#   ./build.sh --no-web   only the Rust command-line tools
#
# Needs Rust (cargo) and, for the web, the wasm32-unknown-unknown target and Node.js.
set -euo pipefail
cd "$(dirname "$0")"

WEB=true
case "${1:-}" in
    --no-web) WEB=false ;;
    "") ;;
    *) echo "usage: $0 [--no-web]"; exit 2 ;;
esac

step() { echo; echo "== $*"; }
need() { command -v "$1" > /dev/null || { echo "error: $1 not found ($2)" >&2; exit 1; }; }
need cargo "install Rust: https://rustup.rs"
t0=$SECONDS

step "reda"
cargo build --release -p rueda

if $WEB; then
    need node "install Node.js: https://nodejs.org"
    need npm "install Node.js: https://nodejs.org"
    if command -v rustup > /dev/null && ! rustup target list --installed | grep -q wasm32-unknown-unknown; then
        step "adding the wasm32-unknown-unknown target"
        rustup target add wasm32-unknown-unknown
    fi
    step "web (veryl/wasm + riscv/rvemu → web/dist)"
    cd web
    [ -d node_modules ] || npm ci
    npm run build
    cd ..
fi

echo
echo "== built in $((SECONDS - t0)) s:"
echo "   target/release/reda   (reda help: every subcommand)"
$WEB && echo "   web/dist/index.html (IDE), web/dist/demo.html"
exit 0
