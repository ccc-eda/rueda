// node boot.mjs SOC.wasm IMAGE [--script FILE] [--trace FILE] [--max N]
// Boots an image (OpenSBI + Linux: fw_payload.bin) on the Fluxa Rv64Soc compiled by fluxa-emu
// (reda emu -o SOC.wasm --top Soc --param RAMW=33554432 --param TRACE=false soc.fx). The UART goes
// to stdout; the script's lines are typed at each shell prompt (as reda rvemu -s does). --trace
// writes riscv-emu's -t commit log (the SoC compiled with TRACE=true), --max stops after N
// instructions (traced) or N state transitions.
import fs from 'node:fs';
import { loadFiles } from '../../emu/js/emu.mjs';

const args = process.argv.slice(2);
const opt = (k) => { const i = args.indexOf(k); return i >= 0 ? args.splice(i, 2)[1] : null; };
const script = opt('--script'), traceFile = opt('--trace'), max = Number(opt('--max') ?? 0);
const [wasm, image] = args;
const m = await loadFiles(wasm);
const ram = m.meta.insts.find((i) => i.mems.ram).mems.ram;
const img = fs.readFileSync(image);
new Uint8Array(m.ex.mem.buffer, ram.addr, img.length).set(img);
m.close(m.chan('load'));

const [envReq, env, tx, gpio, fin, trace] = ['env_req', 'env', 'tx', 'gpio', 'fin', 'trace'].map((n) => m.chan(n));
const lines = script ? fs.readFileSync(script, 'utf8').split('\n').filter((l) => l.length) : [];
let typing = [];                     // bytes of the line being typed
let shown = '';                      // the UART output's last bytes (to see the prompt)
const out = fs.openSync(traceFile ?? '/dev/null', 'w');
let tbuf = [];
let traced = 0, transitions = 0;
const hex = (v, n) => BigInt.asUintN(64, v).toString(16).padStart(n, '0');
const t0 = performance.now();
let done = null;
while (!done) {
  const n = m.run(1 << 22);
  transitions += n;
  for (let t; (t = m.pop(tx)); ) {
    const c = String.fromCharCode(Number(t[0]));
    process.stdout.write(c);
    shown = (shown + c).slice(-16);
    if (shown.endsWith('rv64:~$ ') && typing.length === 0) {
      if (lines.length) typing = [...Buffer.from(lines.shift() + '\n')];
      else done = done ?? 'the script ended';
    }
  }
  for (let t; (t = m.pop(gpio)); ) process.stdout.write(`[gpio] out=0x${Number(t[0]).toString(16).padStart(4, '0')}\n`);
  for (let t; (t = m.pop(fin)); ) done = `finisher 0x${Number(t[0]).toString(16)}`;
  for (let t; (t = m.pop(trace)); ) {
    const [prv, pc, insn, wrote, rd, val] = t;
    let l = `core   0: ${prv} 0x${hex(pc, 16)} (0x${(insn & 3n) !== 3n ? hex(insn, 4) : hex(insn, 8)})`;
    if (wrote) l += ` x${String(rd).padEnd(2)} 0x${hex(val, 16)}`;
    tbuf.push(l);
    if (tbuf.length >= 4096) { fs.writeSync(out, tbuf.join('\n') + '\n'); tbuf = []; }
    traced++;
  }
  // the devices ask the outside: a byte of the line being typed, no pins
  for (let t; (t = m.pop(envReq)); ) {
    const b = typing.length ? typing.shift() : null;
    if (b === null && t[0] && !lines.length && !typing.length) { done = done ?? 'idle'; }
    m.push(env, [b === null ? 0n : 1n, BigInt(b ?? 0), 0n]);
  }
  if (max && (traceFile ? traced : transitions) >= max) done = done ?? `${max} reached`;
  if (n === 0 && !m.count(envReq)) done = done ?? 'stuck';
}
if (tbuf.length) fs.writeSync(out, tbuf.join('\n') + '\n');
const s = (performance.now() - t0) / 1000;
console.log(`\n[${done}: ${s.toFixed(1)} s, ${transitions} transitions${traceFile ? `, ${traced} instructions` : ''}]`);
