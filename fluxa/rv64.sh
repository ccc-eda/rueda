#!/bin/sh
# RV64G 處理器範例：任一測試檔失敗則以非零結束碼結束（但仍跑完全部）
cd "$(dirname "$0")"
F="../target/release/reda sim"
status=0
$F rv64/rv64.fx rv64/fpu.fx rv64/smoke.fx --stats || status=1
for t in basic mmu amo fpu fp irq system; do
    $F rv64/rv64.fx rv64/fpu.fx rv64/tests_$t.fx || status=1
done
exit $status
