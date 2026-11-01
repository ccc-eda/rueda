// The Fluxa machine: fluxa/rv64/linux/soc.fx (the RV64 board written in Fluxa) compiled by
// fluxa-emu (core command `fluxa_emu_compile`), with the interface of Machine and RvMachine so the
// emulator's worker and the Node tests drive any of them. The module talks through its channels
// to the outside: `load` (closed when the images are in the RAM: the hart starts), `tx` (UART),
// `gpio` (outputs and enables), `fin` (the finisher), and the devices' questions `env_req` (a poll,
// or a wait until the hart's next timer) answered on `env` (a UART byte, the input pins, the time).
// No DOM.
//
// Time: with `realtime`, the time answered is the clock's (10 MHz, as the device tree says) and a
// wait is answered when the timer is due or a byte comes (`step` reports `idle` meanwhile); without
// it mtime counts instructions (reproducible, as on the command line).

import { MachineCfg, StepResult } from './emu_machine';

interface FxMeta {
  insts: { name: string; desc: number; code: number; mems: Record<string, { addr: number; word: number; words: number }> }[];
  chans: { name: string; addr: number; depth: number; tok: number; leaves: number[]; signed: boolean[] }[];
  ins: [string, number][];
  outs: [string, number][];
  traps: string[][];
}

const RAM_BASE = 0x8000_0000n;

/** the layout of a compiled machine (a snapshot only fits the same one): FNV over its JSON */
export function fxFingerprint(meta: object): string {
  const s = JSON.stringify(meta);
  let h = 0x811c9dc5;
  for (let i = 0; i < s.length; i++) { h ^= s.charCodeAt(i); h = Math.imul(h, 0x01000193) >>> 0; }
  return 'fluxa:' + h.toString(16).padStart(8, '0') + ':' + s.length;
}
const MAX64 = 0xffff_ffff_ffff_ffffn;

export class FluxaMachine {
  /** state transitions run since the reset (the machine's measure of work) */
  cycle = 0;
  finished: StepResult['finished'] = null;
  private x!: { mem: WebAssembly.Memory; run(n: number): number };
  private dv!: DataView;
  private ram = { addr: 0, size: 0 };
  private started = false;
  private t0 = 0;
  private realtime = false;
  private rx: number[] = [];
  private pins = 0;
  /** a wait the devices asked for, not answered yet: until (10 MHz ticks) */
  private waiting: bigint | null = null;
  /** the latest time the machine was told (mtime ticks): a restored machine's clock goes on from it */
  private told = 0n;
  /** the GPIO outputs and enables now */
  private gout = 0;
  private goe = 0;

  private constructor(private mod: WebAssembly.Module, private meta: FxMeta, readonly cfg: MachineCfg) {}

  static async create(wasm: Uint8Array | WebAssembly.Module, meta: object, cfg: MachineCfg, opts: { realtime?: boolean } = {}): Promise<FluxaMachine> {
    const mod = wasm instanceof WebAssembly.Module ? wasm : await WebAssembly.compile(wasm as BufferSource);
    const m = new FluxaMachine(mod, meta as FxMeta, cfg);
    m.realtime = !!opts.realtime;
    m.clear();
    return m;
  }

  private chan(name: string): number {
    const e = [...this.meta.ins, ...this.meta.outs].find((x) => x[0] === name);
    if (!e) throw new Error(`the Fluxa machine has no channel ${name}`);
    return e[1];
  }

  /** a new instance: everything zero, the hart not started */
  clear(): void {
    this.x = new WebAssembly.Instance(this.mod, {}).exports as unknown as FluxaMachine['x'];
    this.dv = new DataView(this.x.mem.buffer);
    const r = this.meta.insts.find((i) => i.mems.ram)?.mems.ram;
    if (!r) throw new Error('the Fluxa machine has no memory `ram`');
    this.ram = { addr: r.addr, size: r.words * r.word };
    this.started = false;
    this.waiting = null;
    this.rx = [];
    this.cycle = 0;
    this.finished = null;
  }

  private bytes(addr: bigint, n: number): Uint8Array {
    const off = Number(addr - RAM_BASE);
    if (off < 0 || off + n > this.ram.size) throw new Error(`0x${addr.toString(16)} (${n} bytes) is outside the RAM`);
    return new Uint8Array(this.x.mem.buffer, this.ram.addr + off, n);
  }

  load(bytes: Uint8Array, addr?: bigint): void { this.bytes(addr ?? RAM_BASE, bytes.length).set(bytes); }

  /** start the hart (the RAM keeps what was loaded) */
  reset(): void {
    if (this.started) throw new Error('the Fluxa machine resets by clear() and loading again');
    this.close(this.chan('load'));
    this.started = true;
    this.t0 = performance.now();
  }

  writeArea(bytes: Uint8Array): void {
    if (!this.cfg.files) return;
    if (bytes.length > this.cfg.files.size) throw new Error(`the project's files (${bytes.length} bytes) do not fit the exchange area`);
    this.load(bytes, BigInt(this.cfg.files.addr));
  }
  readArea(): Uint8Array | null { const f = this.cfg.files; return f ? this.bytes(BigInt(f.addr), f.size).slice() : null; }
  screen(): Uint8Array | null { const f = this.cfg.fb; return f ? this.bytes(BigInt(f.addr), f.width * f.height * 2) : null; }

  input(bytes: number[]): void { this.rx.push(...bytes); }
  setGpio(bits: number): void { this.pins = bits & 0xffff; }

  /** the whole memory (its non-zero 4 KB pages) and the time last told */
  snapshot(): Uint8Array {
    const mem = new Uint8Array(this.x.mem.buffer);
    const pages: number[] = [];
    for (let p = 0; p < mem.length; p += 4096) { for (let i = p; i < p + 4096; i += 8) if (this.dv.getBigUint64(i, true)) { pages.push(p); break; } }
    const H = 16;
    const out = new Uint8Array(H + pages.length * (4 + 4096));
    const v = new DataView(out.buffer);
    v.setUint32(0, 0x4658454d, true);
    v.setUint32(4, pages.length, true);
    v.setBigUint64(8, this.told, true);
    pages.forEach((p, k) => { v.setUint32(H + k * 4100, p, true); out.set(mem.subarray(p, p + 4096), H + 4 + k * 4100); });
    return out;
  }
  restore(data: Uint8Array, cycle = 0): void {
    const v = new DataView(data.buffer, data.byteOffset, data.byteLength);
    if (v.getUint32(0, true) !== 0x4658454d) throw new Error('not a Fluxa machine state');
    this.clear();
    const H = 16;
    const mem = new Uint8Array(this.x.mem.buffer);
    const n = v.getUint32(4, true);
    for (let k = 0; k < n; k++) { const p = v.getUint32(H + k * 4100, true); mem.set(data.subarray(H + 4 + k * 4100, H + 4 + k * 4100 + 4096), p); }
    this.started = true;
    this.cycle = cycle;
    // the clock goes on from the time the machine was last told
    this.told = v.getBigUint64(8, true);
    this.t0 = performance.now() - Number(this.told) / 1e4;
  }

  // ---- channels to the outside (fluxa/emu/js/emu.mjs, the same layout)
  private ch(c: number) { return this.meta.chans[c]; }
  private count(c: number): number { return this.dv.getUint32(this.ch(c).addr, true); }
  private close(c: number): void { this.dv.setUint32(this.ch(c).addr + 8, 1, true); }
  private push(c: number, vals: bigint[]): void {
    const ch = this.ch(c), a = ch.addr, dv = this.dv;
    const cnt = dv.getUint32(a, true), head = dv.getUint32(a + 4, true);
    let p = a + 16 + ((head + cnt) % ch.depth) * ch.tok;
    ch.leaves.forEach((n, k) => { const x = vals[k] ?? 0n; dv.setBigUint64(p, BigInt.asUintN(64, x), true); if (n === 16) dv.setBigUint64(p + 8, BigInt.asUintN(64, x >> 64n), true); p += n; });
    dv.setUint32(a, cnt + 1, true);
  }
  private pop(c: number): bigint[] | null {
    const ch = this.ch(c), a = ch.addr, dv = this.dv;
    const cnt = dv.getUint32(a, true), head = dv.getUint32(a + 4, true);
    if (!cnt) return null;
    let p = a + 16 + head * ch.tok;
    const out = ch.leaves.map((n) => { const v = dv.getBigUint64(p, true); p += n; return v; });
    dv.setUint32(a + 4, (head + 1) % ch.depth, true);
    dv.setUint32(a, cnt - 1, true);
    return out;
  }
  private error(): string {
    const d = this.dv.getUint32(0, true);
    const inst = this.meta.insts.find((i) => i.desc === d);
    const k = this.dv.getUint32(d + 12, true);
    return inst ? `${inst.name}: ${this.meta.traps[inst.code][k - 1] ?? 'stopped'}` : 'the Fluxa machine stopped';
  }

  /** the clock as mtime ticks (10 MHz) */
  private now(): bigint { return BigInt(Math.floor((performance.now() - this.t0) * 1e4)); }

  /** answer the devices: a byte if one waits, the pins, the time */
  private answer(until: bigint | null): boolean {
    const env = this.chan('env');
    let t = 0n;
    if (this.realtime) {
      t = this.now();
      if (until !== null && !this.rx.length && t < until) return false;
    } else if (until !== null && until !== MAX64) t = until;
    const b = this.rx.length ? this.rx.shift()! : null;
    if (t > this.told) this.told = t;
    this.push(env, [b === null ? 0n : 1n, BigInt(b ?? 0), BigInt(this.pins), t]);
    return true;
  }

  /** run up to `max` state transitions; stops at the finisher, or (real time) when the hart waits */
  step(max: number): StepResult & { idle?: boolean } {
    if (!this.started) this.reset();
    const out: number[] = [];
    const n = this.cfg.gpio?.n ?? 16;
    const high = new Array<number>(n).fill(0);
    let oeSeen = this.goe, done = 0, weight = 0, idle = false;
    const [envReq, tx, gpio, fin] = ['env_req', 'tx', 'gpio', 'fin'].map((c) => this.chan(c));
    const SAMPLE = 40_000;
    while (done < max && !this.finished) {
      // a wait not answered yet: answered when the timer is due or a byte comes
      if (this.waiting !== null) {
        if (!this.answer(this.waiting)) { idle = true; break; }
        this.waiting = null;
      }
      const r = this.x.run(Math.min(SAMPLE, max - done));
      if (r < 0) throw new Error(this.error());
      done += r;
      const w = Math.max(r, 1);
      weight += w;
      for (let k = 0; k < n; k++) if ((this.gout & this.goe) >> k & 1) high[k] += w;
      for (let t; (t = this.pop(tx)); ) out.push(Number(t[0]));
      for (let t; (t = this.pop(gpio)); ) { this.gout = Number(t[0]); this.goe = Number(t[1]); oeSeen |= this.goe; }
      for (let t; (t = this.pop(fin)); ) { const code = Number(t[0]); this.finished = { code, pass: (code & 0xffff) === 0x5555 }; }
      for (let t; (t = this.pop(envReq)); ) {
        const [wait, until] = t;
        if (wait) { if (!this.answer(until)) { this.waiting = until; } }
        else this.answer(null);
      }
      if (r === 0 && this.waiting === null && !this.count(envReq)) { idle = true; break; }
    }
    this.cycle += done;
    return { cycles: done, out, finished: this.finished, duty: high.map((h) => (weight ? h / weight : 0)), oe: oeSeen, idle };
  }
}
