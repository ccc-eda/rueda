#!/bin/sh
# the simulator on its example (reda sim)
set -e
cd "$(dirname "$0")"
cargo build --release -q -p rueda
../../target/release/reda sim examples/counter.veryl
