#!/bin/sh
# 一次跑完全部：建置、回歸測試（examples + rv32i + rv64），再跑兩個處理器腳本看輸出
set -e
cd "$(dirname "$0")"
cargo build --release -p rueda
cargo test --release -p rueda-fluxa
./rv32i.sh
./rv64.sh
