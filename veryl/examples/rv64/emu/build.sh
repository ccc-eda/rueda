#!/usr/bin/env bash
# Rv64Soc for reda emu: build reda (Rust) and compile the design to
# WebAssembly: build/emu/rv64soc.wasm + .json (signal offsets). Run it with emu/rv64emu.
set -euo pipefail
cd "$(dirname "$0")/.."
(cd ../../.. && cargo build --release -q -p rueda)
mkdir -p build/emu
../../../target/release/reda emu --top Rv64Soc --stop o_tx_valid,o_finish,o_ret \
    -o build/emu/rv64soc.wasm src/rv64core.veryl src/rv64soc.veryl
