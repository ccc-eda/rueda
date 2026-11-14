// The plugged board's nets as a drawing (an SVG; no DOM other than its host): each part a box
// with its pins, the nets between them. Connectors on the left, ICs in the middle as schematic
// symbols (inputs and power on the left, outputs on the right), two-pin parts to the right,
// each column ordered by the pins it connects to. A signal net is drawn as curves along its
// shortest connecting tree; 3V3 and GND get a label at each pin instead of wires. While the board
// runs, each net takes its level's color (1, 0, floating); hovering a net lights all its pins.

import { tr } from '../i18n';

export interface NvPart { id: string; kind: string; value: string | null; pins: [number, string, string][] }
export interface NvNet { name: string; rail: boolean | null; pins: [number, number][] }
export interface NvBoard { parts: NvPart[]; nets: NvNet[]; erc: { error: boolean; msg: string }[] }

/** a pin's place: its end on the box, and which way its stub goes (-1 left, 1 right) */
interface Pt { x: number; y: number; dir: number }
interface Box { x: number; y: number; w: number; h: number; pins: Map<number, Pt>; role: 'conn' | 'ic' | 'two' }

const PITCH = 16, STUB = 14;
const esc = (s: string) => s.replace(/&/g, '&amp;').replace(/</g, '&lt;');
const role = (p: NvPart): Box['role'] => (p.kind === 'pmod' ? 'conn' : p.pins.length > 2 ? 'ic' : 'two');

/** place every part: (column, then the order of what it connects to) */
function layout(b: NvBoard): Box[] {
  const boxes: Box[] = new Array(b.parts.length);
  const netsOf = (pi: number) => b.nets.map((n, i) => (n.pins.some((p) => p[0] === pi) ? i : -1)).filter((i) => i >= 0 && b.nets[i].rail === null);
  const shape = (pi: number, x: number, y: number): Box => {
    const p = b.parts[pi], r = role(p), pins = new Map<number, Pt>();
    const nums = p.pins.map((q) => q[0]).sort((a, c) => a - c);
    if (r === 'conn') {
      nums.forEach((n, k) => pins.set(n, { x: x + 56, y: y + 18 + k * PITCH, dir: 1 }));
      return { x, y, w: 56, h: 18 + nums.length * PITCH, pins, role: r };
    }
    if (r === 'ic') {
      // as a schematic symbol: outputs on the right (by name: Q0..Q7), the rest on the left (power last)
      const nat = (a: string, c: string) => a.localeCompare(c, undefined, { numeric: true });
      const right = p.pins.filter((q) => q[2] === 'out' || q[2] === 'od').sort((a, c) => nat(a[1], c[1]));
      const left = p.pins.filter((q) => !right.includes(q)).sort((a, c) => (+(a[2] === 'power') - +(c[2] === 'power')) || a[0] - c[0]);
      left.forEach((q, k) => pins.set(q[0], { x, y: y + 18 + k * PITCH, dir: -1 }));
      right.forEach((q, k) => pins.set(q[0], { x: x + 120, y: y + 18 + k * PITCH, dir: 1 }));
      return { x, y, w: 120, h: 18 + Math.max(left.length, right.length) * PITCH, pins, role: r };
    }
    pins.set(nums[0], { x, y: y + 12, dir: -1 });
    if (nums[1] !== undefined) pins.set(nums[1], { x: x + 64, y: y + 12, dir: 1 });
    return { x, y, w: 64, h: 24, pins, role: r };
  };
  // connectors, then ICs: one column each
  const cols = [40, 250];
  for (const [ci, want] of (['conn', 'ic'] as const).entries()) {
    let y = 30;
    b.parts.forEach((p, i) => { if (role(p) === want) { boxes[i] = shape(i, cols[ci], y); y += boxes[i].h + 34; } });
  }
  // two-pin parts: a column per step away from the connectors and ICs, near the pins they reach
  let left = b.parts.map((p, i) => (role(p) === 'two' ? i : -1)).filter((i) => i >= 0);
  let x = 520;
  while (left.length) {
    const target = (pi: number) => {
      const ys: number[] = [];
      for (const ni of netsOf(pi)) for (const [q, num] of b.nets[ni].pins) { const pt = boxes[q]?.pins.get(num); if (q !== pi && pt) ys.push(pt.y); }
      return ys.length ? ys.reduce((a, c) => a + c, 0) / ys.length : null;
    };
    const here = left.map((pi) => ({ pi, t: target(pi) })).filter((e) => e.t !== null) as { pi: number; t: number }[];
    // nothing reaches the placed parts (only rails, or a separate group): place the rest here
    const now = here.length ? here : left.map((pi, k) => ({ pi, t: 30 + k * 30 }));
    now.sort((a, c) => a.t - c.t);
    // each box with its title above it (14 px) and a gap
    let bottom = 10;
    for (const e of now) {
      const y = Math.max(e.t - 12, bottom + 20);
      boxes[e.pi] = shape(e.pi, x, y);
      bottom = y + 24;
    }
    const done = new Set(now.map((e) => e.pi));
    left = left.filter((pi) => !done.has(pi));
    x += 170;
  }
  return boxes;
}

/** the shortest tree joining points (Prim), as pairs of indices */
function tree(pts: { x: number; y: number }[]): [number, number][] {
  const n = pts.length, inT = new Array(n).fill(false), best = new Array(n).fill(Infinity), from = new Array(n).fill(-1), out: [number, number][] = [];
  if (!n) return out;
  best[0] = 0;
  for (let k = 0; k < n; k++) {
    let u = -1;
    for (let i = 0; i < n; i++) if (!inT[i] && (u < 0 || best[i] < best[u])) u = i;
    inT[u] = true;
    if (from[u] >= 0) out.push([from[u], u]);
    for (let v = 0; v < n; v++) {
      const d = Math.abs(pts[u].x - pts[v].x) + Math.abs(pts[u].y - pts[v].y);
      if (!inT[v] && d < best[v]) { best[v] = d; from[v] = u; }
    }
  }
  return out;
}

export class NetView {
  private b: NvBoard | null = null;
  private svg!: SVGSVGElement;
  private info!: HTMLElement;
  private levels: (number | null)[] | null = null;
  private hover = -1;

  constructor(private host: HTMLElement) {}

  set(b: NvBoard): void {
    this.b = b;
    this.levels = null;
    const boxes = layout(b);
    const W = Math.max(...boxes.map((x) => x.x + x.w)) + 90, H = Math.max(...boxes.map((x) => x.y + x.h)) + 30;
    const pin = (pi: number, num: number) => boxes[pi]?.pins.get(num);
    const bad = new Set(b.nets.map((n, i) => (b.erc.some((d) => d.msg.startsWith(`net ${n.name}:`)) ? i : -1)).filter((i) => i >= 0));
    let nets = '';
    b.nets.forEach((n, ni) => {
      const pts = n.pins.map(([pi, num]) => pin(pi, num)).filter((p): p is Pt => !!p);
      const ends = pts.map((p) => ({ x: p.x + p.dir * STUB, y: p.y, dir: p.dir }));
      let wires = pts.map((p, k) => `<line x1="${p.x}" y1="${p.y}" x2="${ends[k].x}" y2="${p.y}"/>`).join('');
      if (n.rail === null) {
        for (const [a, c] of tree(ends)) {
          const A = ends[a], C = ends[c], k = Math.max(30, Math.abs(C.x - A.x) / 2);
          wires += `<path d="M${A.x} ${A.y} C${A.x + A.dir * k} ${A.y} ${C.x + C.dir * k} ${C.y} ${C.x} ${C.y}"/>`;
        }
      }
      // the wires, and a wide invisible copy that is easy to point at
      let g = wires + `<g class="nv-hit">${wires}</g>`;
      if (n.rail === null) {
        // the name once, at its first pin
        const e = ends[0];
        if (e) g += `<text class="nv-lbl" x="${e.x + e.dir * 2}" y="${e.y - 3}" text-anchor="${e.dir > 0 ? 'start' : 'end'}">${esc(n.name)}${bad.has(ni) ? ' ⚠' : ''}</text>`;
      } else {
        g += ends.map((e) => `<text class="nv-rail" x="${e.x + e.dir * 2}" y="${e.y + 3}" text-anchor="${e.dir > 0 ? 'start' : 'end'}">${n.rail ? '▲ ' : '⏚ '}${esc(n.name)}</text>`).join('');
      }
      g += pts.map((p) => `<circle cx="${p.x}" cy="${p.y}" r="2.6"/>`).join('');
      const who = n.pins.map(([pi, num]) => `${b.parts[pi].id}.${b.parts[pi].pins.find((q) => q[0] === num)?.[1] ?? num}`).join(', ');
      nets += `<g class="nv-net ${n.rail === true ? 'nv-vcc' : n.rail === false ? 'nv-gnd' : 'nv-sig'}${bad.has(ni) ? ' nv-bad' : ''}" data-n="${ni}"><title>${esc(n.name)}: ${esc(who)}</title>${g}</g>`;
    });
    let parts = '';
    b.parts.forEach((p, pi) => {
      const x = boxes[pi];
      if (!x) return;
      const title = `<text class="nv-id" x="${x.x}" y="${x.y - 4}">${esc(p.id)} <tspan class="nv-kind">${esc(p.kind)}${p.value ? ' ' + esc(p.value) : ''}</tspan></text>`;
      let names = '';
      if (x.role !== 'two') {
        for (const [num, name] of p.pins) {
          const q = x.pins.get(num)!;
          names += `<text class="nv-pin" x="${q.x - q.dir * 5}" y="${q.y + 3}" text-anchor="${q.dir > 0 ? 'end' : 'start'}">${esc(name)}</text>`
            + `<text class="nv-num" x="${q.x + q.dir * 3}" y="${q.y - 3}" text-anchor="${q.dir > 0 ? 'start' : 'end'}">${num}</text>`;
        }
      } else {
        const glyph = p.kind === 'led' ? '▶|' : p.kind === 'btn' ? '⏺' : p.kind === 'res' ? '⏛' : '';
        names = `<text class="nv-glyph" x="${x.x + x.w / 2}" y="${x.y + 16}" text-anchor="middle">${glyph}</text>`;
      }
      parts += `<g class="nv-part" data-p="${esc(p.id)}"><rect x="${x.x}" y="${x.y}" width="${x.w}" height="${x.h}" rx="4"/>${title}${names}</g>`;
    });
    this.host.innerHTML = `<div class="nv-info">${tr('滑過一條線看它接到哪些腳；顏色：', 'Hover a net to see its pins; colors: ')}`
      + `<span class="nv-k1">1</span> <span class="nv-k0">0</span> <span class="nv-kz">${tr('浮接', 'floating')}</span> <span class="nv-kv">3V3</span> <span class="nv-kg">GND</span></div>`
      + `<div class="nv-scroll"><svg class="nv" viewBox="0 0 ${W} ${H}" width="${W}" height="${H}" xmlns="http://www.w3.org/2000/svg">${parts}${nets}</svg></div>`;
    this.svg = this.host.querySelector('svg')!;
    this.info = this.host.querySelector('.nv-info')!;
    const legend = this.info.innerHTML;
    for (const g of this.svg.querySelectorAll<SVGGElement>('.nv-net')) {
      const ni = +g.dataset.n!;
      g.addEventListener('pointerenter', () => { this.hover = ni; this.svg.classList.add('hl'); g.classList.add('on'); this.showInfo(); });
      g.addEventListener('pointerleave', () => { this.hover = -1; this.svg.classList.remove('hl'); g.classList.remove('on'); this.info.innerHTML = legend; });
    }
  }

  private showInfo(): void {
    if (!this.b || this.hover < 0) return;
    const n = this.b.nets[this.hover];
    const lv = this.levels?.[this.hover];
    const level = n.rail !== null ? (n.rail ? '3V3' : 'GND') : lv === undefined ? '' : lv === null ? tr('浮接', 'floating') : String(lv);
    const who = n.pins.map(([pi, num]) => `${this.b!.parts[pi].id}.${this.b!.parts[pi].pins.find((q) => q[0] === num)?.[1] ?? num}`).join('  ');
    const erc = this.b.erc.filter((d) => d.msg.startsWith(`net ${n.name}:`)).map((d) => ` ⚠ ${esc(d.msg)}`).join('');
    this.info.innerHTML = `<b>${esc(n.name)}</b>${level ? ` = ${level}` : ''}　${esc(who)}${erc}`;
  }

  /** the nets' levels (1, 0, null floating) and the LEDs' brightness while the board runs */
  update(levels: (number | null)[] | null, leds: Map<string, number>): void {
    if (!this.b || !this.svg) return;
    this.levels = levels;
    if (levels) {
      for (const g of this.svg.querySelectorAll<SVGGElement>('.nv-sig')) {
        const l = levels[+g.dataset.n!];
        g.classList.toggle('lv1', l === 1);
        g.classList.toggle('lv0', l === 0);
        g.classList.toggle('lvz', l === null);
      }
    }
    for (const [id, d] of leds) {
      const r = this.svg.querySelector<SVGRectElement>(`.nv-part[data-p="${CSS.escape(id)}"] rect`);
      if (r) r.style.fill = d > 0.004 ? `rgba(255, 70, 50, ${(0.25 + 0.75 * Math.sqrt(d)).toFixed(3)})` : '';
    }
    if (this.hover >= 0) this.showInfo();
  }
}
