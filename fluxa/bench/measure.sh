#!/bin/sh
# 量測一個實作（iCE40 HX8K ct256：腳位夠多，寬的串流介面也放得下）：./measure.sh FILE TOP → 一行 TSV：檔案、LC、LUT4、CARRY、DFF、BRAM、Fmax
set -u
cd "$(dirname "$0")"
F=$1; TOP=$2
R=../../target/release/reda
OUT=$(mktemp -d)
LOG=$("$R" fpga --device hx8k --package ct256 --top "$TOP" --bin "$OUT/x.bin" "$F" 2>&1)
lc=$(printf '%s\n' "$LOG" | sed -n 's/.*ICESTORM_LC: *\([0-9]*\)\/.*/\1/p' | head -1)
fmax=$(printf '%s\n' "$LOG" | sed -n "s/.*Max frequency for clock.*: \([0-9.]*\) MHz.*/\1/p" | head -1)
cell() { printf '%s\n' "$LOG" | awk -v t="$1" '$1 ~ t { s += $2 } END { print s + 0 }'; }
printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$F" "${lc:-?}" "$(cell '^SB_LUT4$')" "$(cell '^SB_CARRY$')" "$(cell '^SB_DFF')" "$(cell '^SB_RAM40_4K$')" "${fmax:-?}"
rm -rf "$OUT"
