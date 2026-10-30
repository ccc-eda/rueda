#!/usr/bin/env bash
# reda emu differential test: tests/check.veryl on the simulator (reda sim, its testbench) and compiled
# by reda emu (driven by tests/check.mjs in Node); every printed line must match.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --release -q -p rueda
REDA=../../target/release/reda
OUT=$(mktemp -d)
$REDA emu --top ExprCheck -o "$OUT/check.wasm" tests/check.veryl 2> "$OUT/emu.log" || { cat "$OUT/emu.log"; exit 1; }
$REDA sim tests/check.veryl | grep -E '^[0-9a-f]+ ' > "$OUT/sim.txt"
n=$(wc -l < "$OUT/sim.txt" | tr -d " ")
node tests/check.mjs "$OUT/check.wasm" "$n" > "$OUT/emu.txt"
if cmp -s "$OUT/sim.txt" "$OUT/emu.txt"; then
    echo "reda emu check: $n cycles x 21 outputs identical to reda sim"
    rm -rf "$OUT"
else
    echo "reda emu check: differs (files in $OUT)"
    diff <(tr ' ' '\n' < "$OUT/sim.txt" | head -2000) <(tr ' ' '\n' < "$OUT/emu.txt" | head -2000) | head -10
    exit 1
fi
