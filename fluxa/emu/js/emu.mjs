// fluxa-emu's harness: load a compiled network (OUT.wasm + OUT.json from `reda emu -o OUT.wasm`)
// and talk to it through its channels to the outside. Works in Node and in a browser (pass the
// bytes and the parsed JSON to `instantiate`).
//
//   const m = await loadFiles('core.wasm');
//   const c = m.chan('io');            // a channel by name (index)
//   m.push(c, [5n]); m.run(1 << 20); m.pop(c)
//
// Token values are BigInt; a token is the list of its scalars.

export async function instantiate(bytes, meta) {
  const { instance } = await WebAssembly.instantiate(bytes, {});
  return new FluxaEmu(instance.exports, meta);
}

export async function loadFiles(wasmPath) {
  const fs = await import('node:fs');
  const bytes = fs.readFileSync(wasmPath);
  const meta = JSON.parse(fs.readFileSync(wasmPath.replace(/\.wasm$/, '.json'), 'utf8'));
  return instantiate(bytes, meta);
}

export class FluxaEmu {
  constructor(ex, meta) {
    this.ex = ex;
    this.meta = meta;
    this.dv = new DataView(ex.mem.buffer);
  }
  /** the index of the channel to the outside named `name` */
  chan(name) {
    const e = [...this.meta.ins, ...this.meta.outs].find((x) => x[0] === name);
    if (!e) throw new Error(`no channel ${name}`);
    return e[1];
  }
  get ins() { return this.meta.ins; }
  get outs() { return this.meta.outs; }
  count(c) { return this.dv.getUint32(this.meta.chans[c].addr, true); }
  closed(c) { return this.dv.getUint32(this.meta.chans[c].addr + 8, true) !== 0; }
  room(c) { return this.count(c) < this.meta.chans[c].depth && !this.closed(c); }
  close(c) { this.dv.setUint32(this.meta.chans[c].addr + 8, 1, true); }
  /** put a token into channel `c` (there must be room) */
  push(c, vals) {
    const ch = this.meta.chans[c], a = ch.addr, dv = this.dv;
    const cnt = dv.getUint32(a, true), head = dv.getUint32(a + 4, true);
    let p = a + 16 + ((head + cnt) % ch.depth) * ch.tok;
    ch.leaves.forEach((n, k) => {
      const v = BigInt(vals[k] ?? 0);
      dv.setBigUint64(p, BigInt.asUintN(64, v), true);
      if (n === 16) dv.setBigUint64(p + 8, BigInt.asUintN(64, v >> 64n), true);
      p += n;
    });
    dv.setUint32(a, cnt + 1, true);
  }
  /** take a token from channel `c` (null: empty) */
  pop(c) {
    const ch = this.meta.chans[c], a = ch.addr, dv = this.dv;
    const cnt = dv.getUint32(a, true), head = dv.getUint32(a + 4, true);
    if (cnt === 0) return null;
    let p = a + 16 + head * ch.tok;
    const out = ch.leaves.map((n, k) => {
      const lo = dv.getBigUint64(p, true);
      let v = n === 16 ? BigInt.asIntN(128, (dv.getBigUint64(p + 8, true) << 64n) | lo) : (ch.signed[k] ? BigInt.asIntN(64, lo) : lo);
      p += n;
      return v;
    });
    dv.setUint32(a + 4, (head + 1) % ch.depth, true);
    dv.setUint32(a, cnt - 1, true);
    return out;
  }
  /** run up to `budget` state transitions (until no instance moves); returns the transitions made,
   *  throws the program's error */
  run(budget) {
    const n = this.ex.run(budget);
    if (n < 0) throw new Error(this.error());
    return n;
  }
  error() {
    const d = this.dv.getUint32(0, true);
    const inst = this.meta.insts.find((i) => i.desc === d);
    const k = this.dv.getUint32(d + 12, true);
    if (!inst) return 'the program stopped';
    if (k === 0x7fffffff) return `${inst.name}: recv after the end of a stream`;
    return `${inst.name}: ${this.meta.traps[inst.code][k - 1] ?? 'trap'}`;
  }
}
