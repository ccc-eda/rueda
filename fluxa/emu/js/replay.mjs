// node replay.mjs DIR: the machines recorded by `FLUXA_EMU_RECORD=DIR reda emu x.fx` (the Rust
// executor), their WebAssembly modules given the same input tokens, must send the same outputs.
import fs from 'node:fs';
import path from 'node:path';
import { loadFiles } from './emu.mjs';

const dir = process.argv[2];
let bad = 0, n = 0;
const files = fs.readdirSync(dir).filter((f) => /^m[\d_]+\.streams\.json$/.test(f)).sort();
for (const f of files) {
  const k = f.match(/^m([\d_]+)/)[1];
  const rec = JSON.parse(fs.readFileSync(path.join(dir, f), 'utf8'));
  const m = await loadFiles(path.join(dir, `m${k}.wasm`));
  const pos = rec.ins.map(() => 0);
  const got = rec.outs.map(() => []);
  let err = null;
  for (let round = 0; round < 1e7; round++) {
    let moved = false;
    m.ins.forEach(([, c], i) => {
      while (pos[i] < rec.ins[i].length) {
        const t = rec.ins[i][pos[i]];
        if (t === 'close') { m.close(c); pos[i]++; moved = true; break; }
        if (!m.room(c)) break;
        m.push(c, t.map(BigInt)); pos[i]++; moved = true;
      }
    });
    try { if (m.run(1 << 22) > 0) moved = true; } catch (e) { err = e.message; break; }
    m.outs.forEach(([, c], i) => { for (let t; (t = m.pop(c)); ) { got[i].push(t.map(String)); moved = true; } });
    if (!moved) break;
  }
  n++;
  // the recorded outputs: what the test took, so a prefix of what the module sends
  const same = rec.outs.every((o, i) => o.every((t, j) => got[i][j] && JSON.stringify(got[i][j]) === JSON.stringify(t)));
  if (!same) {
    bad++;
    const i = rec.outs.findIndex((o, i) => o.some((t, j) => !got[i][j] || JSON.stringify(got[i][j]) !== JSON.stringify(t)));
    const j = rec.outs[i].findIndex((t, j) => !got[i][j] || JSON.stringify(got[i][j]) !== JSON.stringify(t));
    console.log(`m${k}: output ${m.outs[i][0]} token ${j}: executor ${JSON.stringify(rec.outs[i][j])}, wasm ${JSON.stringify(got[i][j])}${err ? ' (' + err + ')' : ''}`);
  }
}
console.log(`${n} machines replayed, ${bad} differ`);
process.exit(bad ? 1 : 0);
