// The plugged board as a schematic (netview.ts shows it; it also has the nets view): the parts
// placed as in the nets view (`layout`), drawn as symbols (a resistor's zigzag, an LED's triangle
// and bar, a button's contacts, ICs and connectors as boxes with their pins), wires at right angles.
//
// The space between two columns is a channel. The pins of a net that face the same channel are
// joined there: a horizontal wire from each pin to the net's vertical trunk, a dot where a wire
// joins it in the middle; each trunk gets its own track, so trunks do not overlap. A net that
// reaches several channels is not drawn across the parts: each of its pins or trunks carries the
// net's name (a flag on a single pin), as net labels do on paper. 3V3 and GND are power symbols at
// each pin. Columns are spaced after the tracks and labels their channels need.

import { Box, Drawing, esc, layout, netGroup, NvBoard, partTitle, STUB } from './netview';

/** a pin of a net, with its channel (k: between column k and k + 1; -1 left of the first) */
interface Ref { pi: number; num: number; y: number; k: number }

const textW = (s: string) => s.length * 6 + 4;

export function drawSchematic(b: NvBoard, bad: Set<number>): Drawing {
  // the rows come from the layout; the columns' x are set below
  let boxes = layout(b);
  const cols = [...new Set(boxes.filter(Boolean).map((x) => x.col))].sort((a, c) => a - c);
  const ci = new Map(cols.map((c, i) => [c, i]));
  const chan = (x: Box, dir: number) => ci.get(x.col)! - (dir > 0 ? 0 : 1);
  const n = cols.length;

  // per channel: the trunks (net, its pins, its track) and the room the labels and symbols need on each side
  const trunks: { ni: number; k: number; refs: Ref[]; lo: number; hi: number; track: number }[] = [];
  const flags: { ni: number; ref: Ref }[] = [];
  const side = Array.from({ length: n + 1 }, () => ({ left: 0, right: 0 }));
  const room = (k: number, dir: number, w: number) => { const s = side[k + 1]; if (dir > 0) s.left = Math.max(s.left, w); else s.right = Math.max(s.right, w); };
  b.nets.forEach((net, ni) => {
    const groups = new Map<number, Ref[]>();
    for (const [pi, num] of net.pins) {
      const x = boxes[pi], p = x?.pins.get(num);
      if (!x || !p) continue;
      const r = { pi, num, y: p.y, k: chan(x, p.dir) };
      if (net.rail !== null) { room(r.k, p.dir, 40); continue; }
      groups.set(r.k, [...(groups.get(r.k) ?? []), r]);
    }
    for (const [k, refs] of groups) {
      if (refs.length >= 2) trunks.push({ ni, k, refs, lo: Math.min(...refs.map((r) => r.y)), hi: Math.max(...refs.map((r) => r.y)), track: 0 });
      else {
        flags.push({ ni, ref: refs[0] });
        const p = boxes[refs[0].pi].pins.get(refs[0].num)!;
        room(k, p.dir, textW(net.name) + 22);
      }
    }
  });
  // tracks: the first free one in its channel (trunks overlapping in y take different tracks)
  const tracks: number[] = new Array(n + 1).fill(0);
  for (let k = -1; k < n; k++) {
    const ends: number[] = [];
    for (const t of trunks.filter((t) => t.k === k).sort((a, c) => a.lo - c.lo)) {
      let i = ends.findIndex((e) => e < t.lo - 8);
      if (i < 0) { i = ends.length; ends.push(0); }
      ends[i] = t.hi;
      t.track = i;
    }
    tracks[k + 1] = ends.length;
  }
  const chW = side.map((s, i) => Math.max(50, s.left + s.right + tracks[i] * 12 + (tracks[i] ? 30 : 10)));
  const colW = cols.map((c) => Math.max(...boxes.filter((x) => x && x.col === c).map((x) => x.w)));
  const xs: number[] = [];
  let x = 20 + chW[0];
  for (let i = 0; i < n; i++) { xs.push(x); x += colW[i] + chW[i + 1]; }
  boxes = layout(b, (c) => xs[ci.get(c) ?? 0]);
  const W = x + 20, H = Math.max(...boxes.filter(Boolean).map((x) => x.y + x.h)) + 30;
  // a channel's left edge, and its tracks' x (after the room for labels on the left)
  const chLeft = (k: number) => (k < 0 ? 20 : xs[k] + colW[k]);
  const trackX = (k: number, t: number) => chLeft(k) + side[k + 1].left + 15 + t * 12;
  const at = (r: Ref) => boxes[r.pi].pins.get(r.num)!;

  // the nets: trunks with their wires, flags, power symbols
  let nets = '';
  for (const t of trunks) {
    const tx = trackX(t.k, t.track);
    let w = t.refs.map((r) => `<line x1="${at(r).x}" y1="${r.y}" x2="${tx}" y2="${r.y}"/>`).join('') + `<line x1="${tx}" y1="${t.lo}" x2="${tx}" y2="${t.hi}"/>`;
    const dots = t.refs.filter((r) => r.y > t.lo && r.y < t.hi).map((r) => `<circle cx="${tx}" cy="${r.y}" r="2.6"/>`).join('');
    w += `<g class="nv-hit">${w}</g>`;
    // the name above the top pin's wire, past the pin's number (trunk tops are too close together)
    const name = b.nets[t.ni].name, top = t.refs.find((r) => r.y === t.lo)!, tp = at(top);
    const lx = tp.x + tp.dir * 12;
    nets += netGroup(b, t.ni, bad, w + dots + `<text class="nv-lbl" x="${lx}" y="${t.lo - 3}" text-anchor="${tp.dir > 0 ? 'start' : 'end'}">${esc(name)}${bad.has(t.ni) ? ' ⚠' : ''}</text>`);
  }
  for (const f of flags) {
    const p = at(f.ref), d = p.dir, ex = p.x + d * STUB, name = b.nets[f.ni].name, w = textW(name) + 8, y = p.y;
    const tip = ex + d * 5, end = ex + d * w;
    const g = `<line x1="${p.x}" y1="${y}" x2="${ex}" y2="${y}"/><polygon class="nv-flag" points="${ex},${y} ${tip},${y - 6} ${end},${y - 6} ${end},${y + 6} ${tip},${y + 6}"/>`
      + `<g class="nv-hit"><line x1="${p.x}" y1="${y}" x2="${end}" y2="${y}"/></g>`
      + `<text class="nv-lbl" x="${tip + d * 2}" y="${y + 3}" text-anchor="${d > 0 ? 'start' : 'end'}">${esc(name)}${bad.has(f.ni) ? ' ⚠' : ''}</text>`;
    nets += netGroup(b, f.ni, bad, g);
  }
  b.nets.forEach((net, ni) => {
    if (net.rail === null) return;
    let g = '';
    for (const [pi, num] of net.pins) {
      const p = boxes[pi]?.pins.get(num);
      if (!p) continue;
      const d = p.dir, ex = p.x + d * STUB, y = p.y;
      g += `<line x1="${p.x}" y1="${y}" x2="${ex}" y2="${y}"/>`;
      // 3V3: a bar across the wire and its name; GND: three bars getting shorter
      g += net.rail
        ? `<line x1="${ex}" y1="${y - 6}" x2="${ex}" y2="${y + 6}"/><text class="nv-rail" x="${ex + d * 3}" y="${y + 3}" text-anchor="${d > 0 ? 'start' : 'end'}">${esc(net.name)}</text>`
        : `<line x1="${ex}" y1="${y - 6}" x2="${ex}" y2="${y + 6}"/><line x1="${ex + d * 3}" y1="${y - 4}" x2="${ex + d * 3}" y2="${y + 4}"/><line x1="${ex + d * 6}" y1="${y - 2}" x2="${ex + d * 6}" y2="${y + 2}"/>`;
    }
    nets += netGroup(b, ni, bad, g + `<g class="nv-hit">${g}</g>`);
  });

  // the parts as symbols
  let parts = '';
  b.parts.forEach((p, pi) => {
    const x = boxes[pi];
    if (!x) return;
    let s = '';
    if (x.role !== 'two') {
      s = `<rect class="nv-body" x="${x.x}" y="${x.y}" width="${x.w}" height="${x.h}" rx="2"/>`;
      for (const [num, name] of p.pins) {
        const q = x.pins.get(num)!;
        s += `<text class="nv-pin" x="${q.x - q.dir * 5}" y="${q.y + 3}" text-anchor="${q.dir > 0 ? 'end' : 'start'}">${esc(name)}</text>`
          + `<text class="nv-num" x="${q.x + q.dir * 3}" y="${q.y - 3}" text-anchor="${q.dir > 0 ? 'start' : 'end'}">${num}</text>`;
      }
      parts += `<g class="nv-part" data-p="${esc(p.id)}">${s}${partTitle(p, x.x, x.y - 4)}</g>`;
      return;
    }
    const X = x.x, cy = x.y + 12;
    if (p.kind === 'res') {
      s = `<path class="nv-sym" d="M${X} ${cy} H${X + 12} l2.5 -5 l5 10 l5 -10 l5 10 l5 -10 l5 10 l5 -10 l5 10 l2.5 -5 H${X + 64}"/>`;
    } else if (p.kind === 'led') {
      s = `<path class="nv-sym" d="M${X} ${cy} H${X + 22} M${X + 38} ${cy} H${X + 64} M${X + 38} ${cy - 8} V${cy + 8}"/>`
        + `<polygon class="nv-sym nv-body" points="${X + 22},${cy - 8} ${X + 22},${cy + 8} ${X + 38},${cy}"/>`
        + `<path class="nv-sym nv-thin" d="M${X + 30} ${cy - 10} l6 -6 m-3 0 h3 v3 M${X + 37} ${cy - 9} l6 -6 m-3 0 h3 v3"/>`;
    } else if (p.kind === 'btn') {
      s = `<path class="nv-sym" d="M${X} ${cy} H${X + 20} M${X + 44} ${cy} H${X + 64} M${X + 18} ${cy - 7} H${X + 46} M${X + 32} ${cy - 7} V${cy - 13}"/>`
        + `<circle class="nv-sym" cx="${X + 22}" cy="${cy}" r="2"/><circle class="nv-sym" cx="${X + 42}" cy="${cy}" r="2"/>`;
    } else {
      s = `<rect class="nv-body" x="${X + 12}" y="${cy - 6}" width="40" height="12"/><path class="nv-sym" d="M${X} ${cy} H${X + 12} M${X + 52} ${cy} H${X + 64}"/>`;
    }
    parts += `<g class="nv-part" data-p="${esc(p.id)}">${s}${partTitle(p, X + 4, x.y - 2)}</g>`;
  });
  return { w: W, h: H, body: parts + nets };
}
