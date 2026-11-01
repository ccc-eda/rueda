"use strict";
var __defProp = Object.defineProperty;
var __getOwnPropDesc = Object.getOwnPropertyDescriptor;
var __getOwnPropNames = Object.getOwnPropertyNames;
var __hasOwnProp = Object.prototype.hasOwnProperty;
var __export = (target, all) => {
  for (var name in all)
    __defProp(target, name, { get: all[name], enumerable: true });
};
var __copyProps = (to, from, except, desc) => {
  if (from && typeof from === "object" || typeof from === "function") {
    for (let key of __getOwnPropNames(from))
      if (!__hasOwnProp.call(to, key) && key !== except)
        __defProp(to, key, { get: () => from[key], enumerable: !(desc = __getOwnPropDesc(from, key)) || desc.enumerable });
  }
  return to;
};
var __toCommonJS = (mod) => __copyProps(__defProp({}, "__esModule", { value: true }), mod);

// src/fx_machine.ts
var fx_machine_exports = {};
__export(fx_machine_exports, {
  FluxaMachine: () => FluxaMachine,
  fxFingerprint: () => fxFingerprint
});
module.exports = __toCommonJS(fx_machine_exports);
var RAM_BASE = 0x80000000n;
function fxFingerprint(meta) {
  const s = JSON.stringify(meta);
  let h = 2166136261;
  for (let i = 0; i < s.length; i++) {
    h ^= s.charCodeAt(i);
    h = Math.imul(h, 16777619) >>> 0;
  }
  return "fluxa:" + h.toString(16).padStart(8, "0") + ":" + s.length;
}
var MAX64 = 0xffffffffffffffffn;
var FluxaMachine = class _FluxaMachine {
  constructor(mod, meta, cfg) {
    this.mod = mod;
    this.meta = meta;
    this.cfg = cfg;
  }
  mod;
  meta;
  cfg;
  /** state transitions run since the reset (the machine's measure of work) */
  cycle = 0;
  finished = null;
  x;
  dv;
  ram = { addr: 0, size: 0 };
  started = false;
  t0 = 0;
  realtime = false;
  rx = [];
  pins = 0;
  /** a wait the devices asked for, not answered yet: until (10 MHz ticks) */
  waiting = null;
  /** the latest time the machine was told (mtime ticks): a restored machine's clock goes on from it */
  told = 0n;
  /** the GPIO outputs and enables now */
  gout = 0;
  goe = 0;
  static async create(wasm, meta, cfg, opts = {}) {
    const mod = wasm instanceof WebAssembly.Module ? wasm : await WebAssembly.compile(wasm);
    const m = new _FluxaMachine(mod, meta, cfg);
    m.realtime = !!opts.realtime;
    m.clear();
    return m;
  }
  chan(name) {
    const e = [...this.meta.ins, ...this.meta.outs].find((x) => x[0] === name);
    if (!e) throw new Error(`the Fluxa machine has no channel ${name}`);
    return e[1];
  }
  /** a new instance: everything zero, the hart not started */
  clear() {
    this.x = new WebAssembly.Instance(this.mod, {}).exports;
    this.dv = new DataView(this.x.mem.buffer);
    const r = this.meta.insts.find((i) => i.mems.ram)?.mems.ram;
    if (!r) throw new Error("the Fluxa machine has no memory `ram`");
    this.ram = { addr: r.addr, size: r.words * r.word };
    this.started = false;
    this.waiting = null;
    this.rx = [];
    this.cycle = 0;
    this.finished = null;
  }
  bytes(addr, n) {
    const off = Number(addr - RAM_BASE);
    if (off < 0 || off + n > this.ram.size) throw new Error(`0x${addr.toString(16)} (${n} bytes) is outside the RAM`);
    return new Uint8Array(this.x.mem.buffer, this.ram.addr + off, n);
  }
  load(bytes, addr) {
    this.bytes(addr ?? RAM_BASE, bytes.length).set(bytes);
  }
  /** start the hart (the RAM keeps what was loaded) */
  reset() {
    if (this.started) throw new Error("the Fluxa machine resets by clear() and loading again");
    this.close(this.chan("load"));
    this.started = true;
    this.t0 = performance.now();
  }
  writeArea(bytes) {
    if (!this.cfg.files) return;
    if (bytes.length > this.cfg.files.size) throw new Error(`the project's files (${bytes.length} bytes) do not fit the exchange area`);
    this.load(bytes, BigInt(this.cfg.files.addr));
  }
  readArea() {
    const f = this.cfg.files;
    return f ? this.bytes(BigInt(f.addr), f.size).slice() : null;
  }
  screen() {
    const f = this.cfg.fb;
    return f ? this.bytes(BigInt(f.addr), f.width * f.height * 2) : null;
  }
  input(bytes) {
    this.rx.push(...bytes);
  }
  setGpio(bits) {
    this.pins = bits & 65535;
  }
  /** the whole memory (its non-zero 4 KB pages) and the time last told */
  snapshot() {
    const mem = new Uint8Array(this.x.mem.buffer);
    const pages = [];
    for (let p = 0; p < mem.length; p += 4096) {
      for (let i = p; i < p + 4096; i += 8) if (this.dv.getBigUint64(i, true)) {
        pages.push(p);
        break;
      }
    }
    const H = 16;
    const out = new Uint8Array(H + pages.length * (4 + 4096));
    const v = new DataView(out.buffer);
    v.setUint32(0, 1180190029, true);
    v.setUint32(4, pages.length, true);
    v.setBigUint64(8, this.told, true);
    pages.forEach((p, k) => {
      v.setUint32(H + k * 4100, p, true);
      out.set(mem.subarray(p, p + 4096), H + 4 + k * 4100);
    });
    return out;
  }
  restore(data, cycle = 0) {
    const v = new DataView(data.buffer, data.byteOffset, data.byteLength);
    if (v.getUint32(0, true) !== 1180190029) throw new Error("not a Fluxa machine state");
    this.clear();
    const H = 16;
    const mem = new Uint8Array(this.x.mem.buffer);
    const n = v.getUint32(4, true);
    for (let k = 0; k < n; k++) {
      const p = v.getUint32(H + k * 4100, true);
      mem.set(data.subarray(H + 4 + k * 4100, H + 4 + k * 4100 + 4096), p);
    }
    this.started = true;
    this.cycle = cycle;
    this.told = v.getBigUint64(8, true);
    this.t0 = performance.now() - Number(this.told) / 1e4;
  }
  // ---- channels to the outside (fluxa/emu/js/emu.mjs, the same layout)
  ch(c) {
    return this.meta.chans[c];
  }
  count(c) {
    return this.dv.getUint32(this.ch(c).addr, true);
  }
  close(c) {
    this.dv.setUint32(this.ch(c).addr + 8, 1, true);
  }
  push(c, vals) {
    const ch = this.ch(c), a = ch.addr, dv = this.dv;
    const cnt = dv.getUint32(a, true), head = dv.getUint32(a + 4, true);
    let p = a + 16 + (head + cnt) % ch.depth * ch.tok;
    ch.leaves.forEach((n, k) => {
      const x = vals[k] ?? 0n;
      dv.setBigUint64(p, BigInt.asUintN(64, x), true);
      if (n === 16) dv.setBigUint64(p + 8, BigInt.asUintN(64, x >> 64n), true);
      p += n;
    });
    dv.setUint32(a, cnt + 1, true);
  }
  pop(c) {
    const ch = this.ch(c), a = ch.addr, dv = this.dv;
    const cnt = dv.getUint32(a, true), head = dv.getUint32(a + 4, true);
    if (!cnt) return null;
    let p = a + 16 + head * ch.tok;
    const out = ch.leaves.map((n) => {
      const v = dv.getBigUint64(p, true);
      p += n;
      return v;
    });
    dv.setUint32(a + 4, (head + 1) % ch.depth, true);
    dv.setUint32(a, cnt - 1, true);
    return out;
  }
  error() {
    const d = this.dv.getUint32(0, true);
    const inst = this.meta.insts.find((i) => i.desc === d);
    const k = this.dv.getUint32(d + 12, true);
    return inst ? `${inst.name}: ${this.meta.traps[inst.code][k - 1] ?? "stopped"}` : "the Fluxa machine stopped";
  }
  /** the clock as mtime ticks (10 MHz) */
  now() {
    return BigInt(Math.floor((performance.now() - this.t0) * 1e4));
  }
  /** answer the devices: a byte if one waits, the pins, the time */
  answer(until) {
    const env = this.chan("env");
    let t = 0n;
    if (this.realtime) {
      t = this.now();
      if (until !== null && !this.rx.length && t < until) return false;
    } else if (until !== null && until !== MAX64) t = until;
    const b = this.rx.length ? this.rx.shift() : null;
    if (t > this.told) this.told = t;
    this.push(env, [b === null ? 0n : 1n, BigInt(b ?? 0), BigInt(this.pins), t]);
    return true;
  }
  /** run up to `max` state transitions; stops at the finisher, or (real time) when the hart waits */
  step(max) {
    if (!this.started) this.reset();
    const out = [];
    const n = this.cfg.gpio?.n ?? 16;
    const high = new Array(n).fill(0);
    let oeSeen = this.goe, done = 0, weight = 0, idle = false;
    const [envReq, tx, gpio, fin] = ["env_req", "tx", "gpio", "fin"].map((c) => this.chan(c));
    const SAMPLE = 4e4;
    while (done < max && !this.finished) {
      if (this.waiting !== null) {
        if (!this.answer(this.waiting)) {
          idle = true;
          break;
        }
        this.waiting = null;
      }
      const r = this.x.run(Math.min(SAMPLE, max - done));
      if (r < 0) throw new Error(this.error());
      done += r;
      const w = Math.max(r, 1);
      weight += w;
      for (let k = 0; k < n; k++) if ((this.gout & this.goe) >> k & 1) high[k] += w;
      for (let t; t = this.pop(tx); ) out.push(Number(t[0]));
      for (let t; t = this.pop(gpio); ) {
        this.gout = Number(t[0]);
        this.goe = Number(t[1]);
        oeSeen |= this.goe;
      }
      for (let t; t = this.pop(fin); ) {
        const code = Number(t[0]);
        this.finished = { code, pass: (code & 65535) === 21845 };
      }
      for (let t; t = this.pop(envReq); ) {
        const [wait, until] = t;
        if (wait) {
          if (!this.answer(until)) {
            this.waiting = until;
          }
        } else this.answer(null);
      }
      if (r === 0 && this.waiting === null && !this.count(envReq)) {
        idle = true;
        break;
      }
    }
    this.cycle += done;
    return { cycles: done, out, finished: this.finished, duty: high.map((h) => weight ? h / weight : 0), oe: oeSeen, idle };
  }
};
// Annotate the CommonJS export names for ESM import in node:
0 && (module.exports = {
  FluxaMachine,
  fxFingerprint
});
