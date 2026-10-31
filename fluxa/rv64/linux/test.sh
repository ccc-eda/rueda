#!/usr/bin/env bash
# The Fluxa Rv64Soc (soc.fx) compiled by fluxa-emu boots the RV64 board's Linux image
# (web/dist/img_linux.js: OpenSBI + Linux + busybox) to the shell and runs a command; its first
# million instructions are riscv-emu's, commit-log line for line.
set -euo pipefail
cd "$(dirname "$0")/../../.."
REDA=${REDA:-target/release/reda}
D=$(mktemp -d)
node -e "
const fs = require('fs'), zlib = require('zlib');
const t = fs.readFileSync('web/dist/img_linux.js', 'utf8');
fs.writeFileSync('$D/fw_payload.bin', zlib.gunzipSync(Buffer.from(t.match(/VERYL_IMAGES.linux = \"([^\"]+)\"/)[1], 'base64')));"
# the commit log: the first million instructions
$REDA emu -o $D/trace.wasm --top Soc --param RAMW=33554432 --param TRACE=true fluxa/rv64/linux/soc.fx
node fluxa/rv64/linux/boot.mjs $D/trace.wasm $D/fw_payload.bin --trace $D/fluxa.log --max 1000000 > /dev/null
($REDA rvemu -t -m 1000000 $D/fw_payload.bin 2> /dev/null || true) | grep '^core' > $D/rvemu.log
if ! cmp -s <(head -1000000 $D/fluxa.log) $D/rvemu.log; then
    echo "the commit logs differ:"; diff <(head -1000000 $D/fluxa.log) $D/rvemu.log | head -5; exit 1
fi
echo "commit log: 1000000 instructions, the same as riscv-emu"
# the boot to the shell
$REDA emu -o $D/soc.wasm --top Soc --param RAMW=33554432 --param TRACE=false fluxa/rv64/linux/soc.fx
printf 'echo hello from fluxa\nuname -m\n' > $D/script
node fluxa/rv64/linux/boot.mjs $D/soc.wasm $D/fw_payload.bin --script $D/script | tee $D/out | tail -6
grep -q '^hello from fluxa' $D/out && grep -q '^riscv64' $D/out
rm -rf "$D"
