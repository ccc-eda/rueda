#!/usr/bin/env bash
# fluxa-emu's WebAssembly against its executor: every example, the RV32I and RV64 suites run with
# their tasks compiled (reda emu), each machine recorded (its module, the tokens in and out), then
# the modules replayed in Node with the same inputs must send the same outputs.
set -euo pipefail
cd "$(dirname "$0")/../../.."
REDA=${REDA:-target/release/reda}
D=$(mktemp -d)
for f in fluxa/examples/*.fx; do FLUXA_EMU_RECORD=$D $REDA emu "$f" > /dev/null || true; done
for t in smoke tests_basic; do FLUXA_EMU_RECORD=$D $REDA emu fluxa/rv32i/rv32i.fx fluxa/rv32i/$t.fx > /dev/null; done
for t in smoke tests_basic tests_mmu tests_amo tests_fp tests_irq tests_system; do
    FLUXA_EMU_RECORD=$D $REDA emu fluxa/rv64/rv64.fx fluxa/rv64/fpu.fx fluxa/rv64/$t.fx > /dev/null
done
node fluxa/emu/js/replay.mjs "$D"
rm -rf "$D"
