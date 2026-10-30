#!/usr/bin/env bash
# Test the whole project; every step runs even after a failure, with a summary at the end.
#
#   ./test.sh          what CI runs: cargo test of the whole workspace (eda, riscv, veryl, fluxa, reda),
#                      reda sim on an example, the tutorial's flow, reda emu's check, the Fluxa processors,
#                      the RV64 emulator's JIT in WebAssembly, npm test, the IDE end to end in Chrome (npm run e2e)
#   ./test.sh --full   + veryl/synth/test.sh: yosys formal equivalence with `veryl build`, the FPGA flows
#                        and the own place & route's co-simulation (needs OSS CAD Suite; tens of minutes)
#
# npm test runs on web/dist as built: run ./build.sh first after changing Rust or TypeScript.
# Place & route needs IceStorm's chipdb-5k.txt; without one, it is taken from web/dist/chipdb.js.
set -uo pipefail
cd "$(dirname "$0")"
ROOT=$(pwd)

FULL=false
case "${1:-}" in
    --full) FULL=true ;;
    "") ;;
    *) echo "usage: $0 [--full]"; exit 2 ;;
esac

LOG=$(mktemp -d)
results=()
fail=0
# run NAME COMMAND...: output to the terminal and $LOG/NAME.log
run() {
    local name=$1; shift
    echo
    echo "== $name"
    local t0=$SECONDS
    # set -e inside the step: its first failing command fails it (not inside an `if`, which would disable that)
    ( set -e; "$@" ) 2>&1 | tee "$LOG/$name.log"
    if [ "${PIPESTATUS[0]}" = 0 ]; then
        results+=("PASS  $name ($((SECONDS - t0)) s)")
    else
        results+=("FAIL  $name ($((SECONDS - t0)) s, log: $LOG/$name.log)")
        fail=1
    fi
}

# the chip database for place and route
if [ -z "${VERYL_EDA_CHIPDB:-}" ]; then
    for c in /usr/local/share/icebox/chipdb-5k.txt /usr/share/icebox/chipdb-5k.txt /opt/homebrew/share/icebox/chipdb-5k.txt \
             /Users/Shared/ccc/oss-cad-suite/share/icebox/chipdb-5k.txt "$HOME/oss-cad-suite/share/icebox/chipdb-5k.txt"; do
        [ -f "$c" ] && export VERYL_EDA_CHIPDB=$c && break
    done
fi
if [ -z "${VERYL_EDA_CHIPDB:-}" ] && [ -f web/dist/chipdb.js ]; then
    node web/test/chipdb.mjs "$LOG/chipdb-5k.txt" && export VERYL_EDA_CHIPDB=$LOG/chipdb-5k.txt
fi

t0=$SECONDS

workspace_tests() {
    cd "$ROOT"
    cargo test --release
    cargo build --release -p rueda
}
run cargo-test workspace_tests

run reda-sim "$ROOT/veryl/sim/test.sh"

run tutorial "$ROOT/veryl/tutorial/check.sh"

if command -v node > /dev/null; then
    run veryl-emu "$ROOT/veryl/emu/tests/check.sh"
else
    results+=("SKIP  veryl-emu (Node.js not installed)")
fi

fluxa_cpus() {
    "$ROOT/fluxa/rv32i.sh"
    "$ROOT/fluxa/rv64.sh"
}
run fluxa fluxa_cpus

# the translation to WebAssembly runs only in a WebAssembly host: Node
rvemu_jit() {
    cd "$ROOT"
    cargo build --release -p rueda-rvemu --lib --target wasm32-unknown-unknown
    node riscv/rvemu/tests/jit.mjs
}
if command -v node > /dev/null; then
    run rvemu-jit rvemu_jit
else
    results+=("SKIP  rvemu-jit (Node.js not installed)")
fi

web_tests() {
    cd "$ROOT/web"
    [ -d node_modules ] || npm ci
    npm test
}
if command -v node > /dev/null; then
    run web web_tests
else
    results+=("SKIP  web (Node.js not installed)")
fi

# the IDE in a real browser (Playwright with the installed Chrome)
web_e2e() {
    cd "$ROOT/web"
    [ -d node_modules ] || npm ci
    npm run e2e
}
if command -v node > /dev/null && { [ -n "${CHROME:-}" ] || [ -d "/Applications/Google Chrome.app" ] || command -v google-chrome > /dev/null; }; then
    run web-e2e web_e2e
else
    results+=("SKIP  web-e2e (no Chrome; set CHROME=/path/to/chrome)")
fi

if $FULL; then
    run synth-full "$ROOT/veryl/synth/test.sh"
fi

echo
echo "== summary ($((SECONDS - t0)) s)"
printf '   %s\n' "${results[@]}"
[ $fail = 0 ] && echo "== all passed" || echo "== FAILED"
exit $fail
