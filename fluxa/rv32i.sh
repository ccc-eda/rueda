#!/bin/sh
# RV32I 處理器範例：任一測試檔失敗則以非零結束碼結束（但仍跑完全部）
cd "$(dirname "$0")"
F="../target/release/reda sim"
status=0
$F rv32i/rv32i.fx rv32i/smoke.fx --stats || status=1
$F rv32i/rv32i.fx rv32i/tests_basic.fx || status=1
exit $status
