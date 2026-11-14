// A file's view beside its text: while a .board or a pin file is open in the editor, the action bar's
// 🔌 PCB / 📌 Pins draws it, docked beside the text or in a floating window, and draws it again as
// it is edited. No flashing is needed: the core only parses the text.
//   .board: the nets view and the schematic (netview.ts); pcb_info parses it and runs the ERC (the
//     levels come only on the virtual board).
//   .pcf: the iCEBreaker with the pins the file sets, and a table port → pin → what the pin is on the
//     board; pcf_info parses it. Hovering a row or a pin lights both.
// Parse errors and the checks' findings show in the window's bar, with their line.

import { tr } from '../i18n';
import { boardDrawing } from './board';
import { FloatWin, floatWin } from './floatwin';
import { esc, NetView, NvBoard } from './netview';

export type PcbInfo = (NvBoard & { name: string }) | { error: string };
export interface PcfPin { line: number; port: string; pin: number; name: string | null; kind: string | null }
export interface PcfInfo { pins: PcfPin[]; problems: { line: number; msg: string }[] }

/** the views: of a .board, of a pin file */
export type ViewKind = 'pcb' | 'pins';
export const viewOf = (name: string | undefined): ViewKind | null => (name?.endsWith('.board') ? 'pcb' : name?.endsWith('.pcf') ? 'pins' : null);

export class FilePreview {
  private win: HTMLElement;
  private fw: FloatWin;
  private nets: NetView;
  private pinBox: HTMLElement;
  private msg: HTMLElement;
  private name: HTMLElement;
  /** the open file and its view (none: the window is hidden) */
  private file = '';
  private kind: ViewKind | null = null;
  private timer = 0;
  private gen = 0;
  /** each view is wanted or not (it opens again with the next file of its kind) */
  private open: Record<ViewKind, boolean> = { pcb: false, pins: false };

  constructor(private area: HTMLElement, private calls: { pcb: (src: string) => Promise<PcbInfo>; pins: (src: string) => Promise<PcfInfo> }) {
    for (const k of ['pcb', 'pins'] as ViewKind[]) {
      try { this.open[k] = localStorage.getItem('rueda.view.' + k) === '1'; } catch { /* no storage */ }
    }
    this.win = document.createElement('div');
    this.win.className = 'fview hidden';
    this.win.innerHTML = `<div class="fview-h"><span class="fw-t">⠿ <span class="fview-name"></span></span><span class="fview-msg"></span></div>`
      + `<div class="fview-body"><div class="fview-nets"></div><div class="fview-pins"></div></div>`;
    area.appendChild(this.win);
    this.msg = this.win.querySelector('.fview-msg')!;
    this.name = this.win.querySelector('.fview-name')!;
    this.nets = new NetView(this.win.querySelector('.fview-nets')!);
    this.pinBox = this.win.querySelector('.fview-pins')!;
    // docked: beside the text (the editor narrows); floating: over it
    this.fw = floatWin(this.win, this.win.querySelector('.fview-h')!, { key: 'file.preview', auto: () => window.innerWidth < 1000, onChange: () => this.layout() });
  }

  isOpen(k: ViewKind): boolean { return this.open[k]; }

  /** the active file changed: its view, if it has one and it is wanted */
  show(name: string | undefined, src: string): void {
    this.kind = viewOf(name);
    if ((this.kind ? name! : '') !== this.file) { this.name.textContent = ''; this.msg.textContent = ''; }
    this.file = this.kind ? name! : '';
    const on = !!this.kind && this.open[this.kind];
    this.win.classList.toggle('hidden', !on);
    if (this.kind) this.win.dataset.kind = this.kind;
    this.layout();
    if (on) this.update(src, 0);
  }

  /** the file's text changed: draw it again a moment after the typing stops */
  update(src: string, wait = 300): void {
    const k = this.kind;
    if (!k || !this.open[k]) return;
    clearTimeout(this.timer);
    this.timer = window.setTimeout(() => (k === 'pcb' ? this.drawPcb(src) : this.drawPins(src)), wait);
  }

  /** the action bar's button: show or hide the open file's view (`src`: its text) */
  toggle(src: string): void {
    const k = this.kind;
    if (!k) return;
    this.open[k] = !this.open[k];
    try { localStorage.setItem('rueda.view.' + k, this.open[k] ? '1' : '0'); } catch { /* no storage */ }
    this.win.classList.toggle('hidden', !this.open[k]);
    this.layout();
    if (this.open[k]) {
      this.fw.unfold();
      this.fw.clamp();
      this.update(src, 0);
    }
  }

  private async drawPcb(src: string): Promise<void> {
    const g = ++this.gen;
    let r: PcbInfo;
    try { r = await this.calls.pcb(src); } catch (e) { r = { error: String((e as Error).message ?? e) }; }
    if (g !== this.gen) return;
    if ('error' in r) {
      // keep the last drawing: the error is usually a line being typed
      if (!this.name.textContent) this.name.textContent = '🔌 ' + this.file;
      this.msg.innerHTML = `<span class="vb-err">✗ ${esc(r.error)}</span>`;
      return;
    }
    this.name.textContent = `🔌 ${tr('擴充板', 'Expansion board')} ${r.name || this.file}`;
    const errs = r.erc.filter((d) => d.error).length, warns = r.erc.length - errs;
    this.msg.innerHTML = !r.erc.length ? '<span class="vb-dim">ERC ✓</span>'
      : `<span class="${errs ? 'vb-err' : 'vb-dim'}" title="${esc(r.erc.map((d) => (d.error ? '✗ ' : '⚠ ') + d.msg).join('\n'))}">ERC ${errs ? `✗ ${errs}` : ''} ${warns ? `⚠ ${warns}` : ''}</span>`;
    this.nets.set(r);
  }

  private async drawPins(src: string): Promise<void> {
    const g = ++this.gen;
    let r: PcfInfo;
    try { r = await this.calls.pins(src); } catch (e) { this.msg.innerHTML = `<span class="vb-err">✗ ${esc(String((e as Error).message ?? e))}</span>`; return; }
    if (g !== this.gen) return;
    this.name.textContent = `📌 ${tr('腳位', 'Pins')} ${this.file}`;
    this.msg.innerHTML = !r.problems.length ? `<span class="vb-dim">${tr(`${r.pins.length} 支腳`, `${r.pins.length} pins`)} ✓</span>`
      : `<span class="vb-err" title="${esc(r.problems.map((p) => `line ${p.line}: ${p.msg}`).join('\n'))}">⚠ ${esc(`line ${r.problems[0].line}: ${r.problems[0].msg}`)}${r.problems.length > 1 ? ` (+${r.problems.length - 1})` : ''}</span>`;
    const kind = (k: string | null) => (k ? ({ clock: tr('時脈', 'clock'), button: tr('按鈕', 'button'), led: 'LED', uart_rx: 'UART RX', uart_tx: 'UART TX', pmod: 'PMOD' } as Record<string, string>)[k] ?? k : '—');
    const bad = new Set(r.problems.map((p) => p.line));
    this.pinBox.innerHTML = `<svg class="vb-svg fview-board" viewBox="0 0 920 330" xmlns="http://www.w3.org/2000/svg">${boardDrawing()}</svg>`
      + `<table class="fview-tab"><tr><th>${tr('埠', 'Port')}</th><th>${tr('腳', 'Pin')}</th><th>${tr('板上', 'On the board')}</th></tr>`
      + r.pins.map((p) => `<tr data-pin="${p.pin}"${bad.has(p.line) ? ' class="bad"' : ''}><td>${esc(p.port)}</td><td>${p.pin}</td><td>${esc(p.name ?? '?')} <span class="vb-dim">${esc(kind(p.kind))}</span></td></tr>`).join('')
      + `</table>`;
    const svg = this.pinBox.querySelector('svg')!;
    const ports = new Map<number, string[]>();
    for (const p of r.pins) ports.set(p.pin, [...(ports.get(p.pin) ?? []), p.port]);
    for (const el of svg.querySelectorAll<SVGElement>('[data-pin]')) {
      const n = +el.dataset.pin!;
      const used = ports.get(n);
      el.classList.toggle('used', !!used);
      const t = el.querySelector('title') ?? el.appendChild(document.createElementNS('http://www.w3.org/2000/svg', 'title'));
      t.textContent = `${tr('腳', 'pin')} ${n}` + (used ? ` ↔ ${used.join(', ')}` : tr('：沒有用到', ': not used'));
    }
    // a row and its pin light together
    const light = (n: number, on: boolean) => {
      for (const el of this.pinBox.querySelectorAll(`[data-pin="${n}"]`)) el.classList.toggle('hl', on);
    };
    for (const el of this.pinBox.querySelectorAll<HTMLElement | SVGElement>('[data-pin]')) {
      const n = +el.dataset.pin!;
      el.addEventListener('pointerenter', () => light(n, true));
      el.addEventListener('pointerleave', () => light(n, false));
    }
  }

  /** docked and shown: the editor gives it the right part of the area */
  private layout(): void {
    this.area.classList.toggle('pcb-split', !this.win.classList.contains('hidden') && !this.fw.floating);
    const k = this.win.dataset.kind;
    this.win.querySelector<HTMLElement>('.fview-nets')!.classList.toggle('hidden', k !== 'pcb');
    this.pinBox.classList.toggle('hidden', k !== 'pins');
  }
}
