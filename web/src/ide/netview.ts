// The plugged board as a drawing (an SVG; no DOM other than its host), in two views:
// - nets (here): each part a box with its pins; a signal net drawn as curves along its shortest
//   connecting tree;
// - schematic (schematic.ts): symbols, orthogonal wires, net labels, power symbols.
// Both place the parts the same way (`layout`): connectors on the left, ICs in the middle as
// schematic symbols (inputs and power on the left, outputs on the right), two-pin parts to the
// right, each column ordered by the pins it connects to. 3V3 and GND are labels / symbols at each
// pin, not wires. While the board runs, each net takes its level's color (1, 0, floating);
// hovering a net lights all its pins.

import { tr } from '../i18n';
import { drawSchematic } from './schematic';

export interface NvPart { id: string; kind: string; value: string | null; pins: [number, string, string][] }
export interface NvNet { name: string; rail: boolean | null; pins: [number, number][] }
export interface NvBoard { parts: NvPart[]; nets: NvNet[]; erc: { error: boolean; msg: string }[] }

/** a pin's place: its end on the box, and which way its stub goes (-1 left, 1 right) */
export interface Pt { x: number; y: number; dir: number }
/** a placed part; `col`: its column (0 connectors, 1 ICs, 2.. two-pin parts) */
export interface Box { x: number; y: number; w: number; h: number; pins: Map<number, Pt>; role: 'conn' | 'ic' | 'two'; col: number }
/** a drawing: its size and its SVG elements (parts, then nets) */
export interface Drawing { w: number; h: number; body: string }

export const PITCH = 16, STUB = 14;
export const esc = (s: string) => s.replace(/&/g, '&amp;').replace(/</g, '&lt;');
const role = (p: NvPart): Box['role'] => (p.kind === 'pmod' ? 'conn' : p.pins.length > 2 ? 'ic' : 'two');
const defaultX = (c: number) => (c === 0 ? 40 : c === 1 ? 250 : 520 + 170 * (c - 2));

/** place every part: (column, then the order of what it connects to); `colX` gives each column's x */
export function layout(b: NvBoard, colX: (col: number) => number = defaultX): Box[] {
  const boxes: Box[] = new Array(b.parts.length);
  const netsOf = (pi: number) => b.nets.map((n, i) => (n.pins.some((p) => p[0] === pi) ? i : -1)).filter((i) => i >= 0 && b.nets[i].rail === null);
  const shape = (pi: number, col: number, y: number): Box => {
    const p = b.parts[pi], r = role(p), pins = new Map<number, Pt>(), x = colX(col);
    const nums = p.pins.map((q) => q[0]).sort((a, c) => a - c);
    if (r === 'conn') {
      nums.forEach((n, k) => pins.set(n, { x: x + 56, y: y + 18 + k * PITCH, dir: 1 }));
      return { x, y, w: 56, h: 18 + nums.length * PITCH, pins, role: r, col };
    }
    if (r === 'ic') {
      // as a schematic symbol: outputs on the right (by name: Q0..Q7), the rest on the left (power last)
      const nat = (a: string, c: string) => a.localeCompare(c, undefined, { numeric: true });
      const right = p.pins.filter((q) => q[2] === 'out' || q[2] === 'od').sort((a, c) => nat(a[1], c[1]));
      const left = p.pins.filter((q) => !right.includes(q)).sort((a, c) => (+(a[2] === 'power') - +(c[2] === 'power')) || a[0] - c[0]);
      left.forEach((q, k) => pins.set(q[0], { x, y: y + 18 + k * PITCH, dir: -1 }));
      right.forEach((q, k) => pins.set(q[0], { x: x + 120, y: y + 18 + k * PITCH, dir: 1 }));
      return { x, y, w: 120, h: 18 + Math.max(left.length, right.length) * PITCH, pins, role: r, col };
    }
    pins.set(nums[0], { x, y: y + 12, dir: -1 });
    if (nums[1] !== undefined) pins.set(nums[1], { x: x + 64, y: y + 12, dir: 1 });
    return { x, y, w: 64, h: 24, pins, role: r, col };
  };
  // connectors, then ICs: one column each
  for (const [ci, want] of (['conn', 'ic'] as const).entries()) {
    let y = 30;
    b.parts.forEach((p, i) => { if (role(p) === want) { boxes[i] = shape(i, ci, y); y += boxes[i].h + 34; } });
  }
  // two-pin parts: a column per step away from the connectors and ICs, near the pins they reach
  let left = b.parts.map((p, i) => (role(p) === 'two' ? i : -1)).filter((i) => i >= 0);
  let col = 2;
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
      boxes[e.pi] = shape(e.pi, col, y);
      bottom = y + 24;
    }
    const done = new Set(now.map((e) => e.pi));
    left = left.filter((pi) => !done.has(pi));
    col++;
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

/** the pins of a net as "U1.SDA, R2.1" */
export const netPins = (b: NvBoard, n: NvNet, sep = ', ') => n.pins.map(([pi, num]) => `${b.parts[pi].id}.${b.parts[pi].pins.find((q) => q[0] === num)?.[1] ?? num}`).join(sep);
/** a net's group: classes for its kind and ERC, its index, its tooltip */
export const netGroup = (b: NvBoard, ni: number, bad: Set<number>, inner: string) => {
  const n = b.nets[ni];
  return `<g class="nv-net ${n.rail === true ? 'nv-vcc' : n.rail === false ? 'nv-gnd' : 'nv-sig'}${bad.has(ni) ? ' nv-bad' : ''}" data-n="${ni}"><title>${esc(n.name)}: ${esc(netPins(b, n))}</title>${inner}</g>`;
};
/** a part's title above it: id, kind, value */
export const partTitle = (p: NvPart, x: number, y: number) => `<text class="nv-id" x="${x}" y="${y}">${esc(p.id)} <tspan class="nv-kind">${esc(p.kind)}${p.value ? ' ' + esc(p.value) : ''}</tspan></text>`;

/** the nets view: boxes, and curves along each net's shortest tree */
function drawNets(b: NvBoard, bad: Set<number>): Drawing {
  const boxes = layout(b);
  const W = Math.max(...boxes.map((x) => x.x + x.w)) + 90, H = Math.max(...boxes.map((x) => x.y + x.h)) + 30;
  const pin = (pi: number, num: number) => boxes[pi]?.pins.get(num);
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
    nets += netGroup(b, ni, bad, g);
  });
  let parts = '';
  b.parts.forEach((p, pi) => {
    const x = boxes[pi];
    if (!x) return;
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
    parts += `<g class="nv-part" data-p="${esc(p.id)}"><rect class="nv-body" x="${x.x}" y="${x.y}" width="${x.w}" height="${x.h}" rx="4"/>${partTitle(p, x.x, x.y - 4)}${names}</g>`;
  });
  return { w: W, h: H, body: parts + nets };
}

export class NetView {
  private b: NvBoard | null = null;
  private svg: SVGSVGElement | null = null;
  private info!: HTMLElement;
  private levels: (number | null)[] | null = null;
  private leds = new Map<string, number>();
  private hover = -1;
  /** the view: nets or schematic (remembered) */
  private mode: 'nets' | 'sch' = 'nets';

  constructor(private host: HTMLElement) {
    try { if (localStorage.getItem('rueda.nv.mode') === 'sch') this.mode = 'sch'; } catch { /* no storage */ }
  }

  set(b: NvBoard): void {
    this.b = b;
    this.levels = null;
    this.leds.clear();
    this.render();
  }

  private render(): void {
    const b = this.b;
    if (!b) return;
    const bad = new Set(b.nets.map((n, i) => (b.erc.some((d) => d.msg.startsWith(`net ${n.name}:`)) ? i : -1)).filter((i) => i >= 0));
    const d = this.mode === 'sch' ? drawSchematic(b, bad) : drawNets(b, bad);
    const tab = (m: 'nets' | 'sch', label: string, title: string) => `<button class="nv-tab${this.mode === m ? ' cur' : ''}" data-m="${m}" title="${title}">${label}</button>`;
    this.host.innerHTML = `<div class="nv-bar">${tab('nets', tr('線路圖', 'Nets'), tr('零件方塊和連線（網表）', 'Part boxes and their connections (the netlist)'))}`
      + `${tab('sch', tr('電路圖', 'Schematic'), tr('符號、直角導線、網路標籤、電源符號', 'Symbols, right-angle wires, net labels, power symbols'))}<div class="nv-info"></div></div>`
      + `<div class="nv-scroll"><svg class="nv${this.mode === 'sch' ? ' sch' : ''}" viewBox="0 0 ${d.w} ${d.h}" width="${d.w}" height="${d.h}" xmlns="http://www.w3.org/2000/svg">${d.body}</svg></div>`;
    this.svg = this.host.querySelector('svg')!;
    this.info = this.host.querySelector('.nv-info')!;
    for (const t of this.host.querySelectorAll<HTMLButtonElement>('.nv-tab')) {
      t.onclick = () => {
        this.mode = t.dataset.m as 'nets' | 'sch';
        try { localStorage.setItem('rueda.nv.mode', this.mode); } catch { /* no storage */ }
        this.hover = -1;
        this.render();
      };
    }
    this.legend();
    const svg = this.svg;
    for (const g of svg.querySelectorAll<SVGGElement>('.nv-net')) {
      const ni = +g.dataset.n!;
      g.addEventListener('pointerenter', () => { this.hover = ni; svg.classList.add('hl'); for (const x of svg.querySelectorAll(`.nv-net[data-n="${ni}"]`)) x.classList.add('on'); this.showInfo(); });
      g.addEventListener('pointerleave', () => { this.hover = -1; svg.classList.remove('hl'); for (const x of svg.querySelectorAll('.nv-net.on')) x.classList.remove('on'); this.legend(); });
    }
    this.paint();
  }

  private legend(): void {
    this.info.innerHTML = `${tr('滑過一條線看它接到哪些腳；顏色：', 'Hover a net to see its pins; colors: ')}`
      + `<span class="nv-k1">1</span> <span class="nv-k0">0</span> <span class="nv-kz">${tr('浮接', 'floating')}</span> <span class="nv-kv">3V3</span> <span class="nv-kg">GND</span>`;
  }

  private showInfo(): void {
    if (!this.b || this.hover < 0) return;
    const n = this.b.nets[this.hover];
    const lv = this.levels?.[this.hover];
    const level = n.rail !== null ? (n.rail ? '3V3' : 'GND') : lv === undefined ? '' : lv === null ? tr('浮接', 'floating') : String(lv);
    const erc = this.b.erc.filter((d) => d.msg.startsWith(`net ${n.name}:`)).map((d) => ` ⚠ ${esc(d.msg)}`).join('');
    this.info.innerHTML = `<b>${esc(n.name)}</b>${level ? ` = ${level}` : ''}　${esc(netPins(this.b, n, '  '))}${erc}`;
  }

  /** the nets' levels (1, 0, null floating) and the LEDs' brightness while the board runs */
  update(levels: (number | null)[] | null, leds: Map<string, number>): void {
    if (!this.b) return;
    if (levels) this.levels = levels;
    this.leds = leds;
    this.paint();
    if (this.hover >= 0) this.showInfo();
  }

  private paint(): void {
    const svg = this.svg;
    if (!svg) return;
    if (this.levels) {
      for (const g of svg.querySelectorAll<SVGGElement>('.nv-sig')) {
        const l = this.levels[+g.dataset.n!];
        g.classList.toggle('lv1', l === 1);
        g.classList.toggle('lv0', l === 0);
        g.classList.toggle('lvz', l === null);
      }
    }
    for (const [id, d] of this.leds) {
      const r = svg.querySelector<SVGElement>(`.nv-part[data-p="${CSS.escape(id)}"] .nv-body`);
      if (r) r.style.fill = d > 0.004 ? `rgba(255, 70, 50, ${(0.25 + 0.75 * Math.sqrt(d)).toFixed(3)})` : '';
    }
  }
}
