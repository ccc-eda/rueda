#!/usr/bin/env bash
# One entry point for the project's everyday commands: ./run.sh COMMAND [ARGS...]
# (./run.sh help lists them). It calls the scripts that do the work (build.sh, test.sh,
# fluxa/*.sh, web's npm scripts), so they stay the reference.
set -euo pipefail
cd "$(dirname "$0")"
ROOT=$(pwd)
REDA=$ROOT/target/release/reda

usage() {
    cat <<'EOF'
usage: ./run.sh COMMAND [ARGS...]

build
  build [--no-web]      reda and web/dist (build.sh; --no-web: reda only)
  web                   web/dist: the Rust core to wasm32 and the TypeScript (npm run build)
  web-fast              web/dist, TypeScript only (no wasm rebuild)
  typecheck             tsc --noEmit on web/src

run
  serve [PORT]          serve web/dist on http://localhost:PORT (default 8000) and open the IDE
  open                  open web/dist/index.html directly (file://, no server)
  reda ARGS...          target/release/reda (built first if missing), e.g. ./run.sh reda help

test
  test [--full]         the whole project (test.sh; --full adds yosys / nextpnr, tens of minutes)
  test-web              npm test (web/dist as built)
  e2e                   the IDE end to end in Chrome (npm run e2e)
  fluxa                 the Fluxa processors: fluxa/rv32i.sh, fluxa/rv64.sh
  fluxa-linux           Linux on the Fluxa Rv64Soc (fluxa-emu vs riscv-emu)
  rv64 [ARGS...]        the Veryl Rv64Core regression (veryl/examples/rv64/test.sh [--xv6] [--fpga])

other
  linux                 build the OpenSBI + Linux image (veryl/examples/rv64/linux/build.sh)
  size                  disk used by target/, web/dist, web/node_modules
  clean                 cargo clean (web/dist is in git: git checkout web/dist restores it)
  help                  this list
EOF
}

need() { command -v "$1" > /dev/null || { echo "error: $1 not found ($2)" >&2; exit 1; }; }
npm_run() {
    need npm "install Node.js: https://nodejs.org"
    cd "$ROOT/web"
    [ -d node_modules ] || npm ci
    npm run "$@"
}
open_url() {
    if command -v open > /dev/null; then open "$1"
    elif command -v xdg-open > /dev/null; then xdg-open "$1"
    else echo "open $1 in a browser"; fi
}

cmd=${1:-help}
[ $# -gt 0 ] && shift
case "$cmd" in
    build)       exec ./build.sh "$@" ;;
    web)         npm_run build ;;
    web-fast)    npm_run build:web ;;
    typecheck)   npm_run typecheck ;;
    serve)
        need python3 "install Python 3"
        PORT=${1:-8000}
        [ -f web/dist/index.html ] || { echo "web/dist is not built: ./run.sh web (or git checkout web/dist)" >&2; exit 1; }
        echo "serving web/dist on http://localhost:$PORT (Ctrl+C stops)"
        (sleep 1; open_url "http://localhost:$PORT/index.html") &
        exec python3 -m http.server "$PORT" --bind 127.0.0.1 --directory web/dist ;;
    open)        open_url "file://$ROOT/web/dist/index.html" ;;
    reda)
        [ -x "$REDA" ] || cargo build --release -p rueda
        exec "$REDA" "$@" ;;
    test)        exec ./test.sh "$@" ;;
    test-web)    npm_run test ;;
    e2e)         npm_run e2e ;;
    fluxa)       fluxa/rv32i.sh && fluxa/rv64.sh ;;
    fluxa-linux) exec fluxa/rv64/linux/test.sh "$@" ;;
    rv64)        exec veryl/examples/rv64/test.sh "$@" ;;
    linux)       exec veryl/examples/rv64/linux/build.sh "$@" ;;
    size)        du -sh target web/dist web/node_modules 2>/dev/null || true ;;
    clean)       cargo clean ;;
    help|-h|--help) usage ;;
    *)           echo "unknown command: $cmd" >&2; echo >&2; usage >&2; exit 2 ;;
esac
