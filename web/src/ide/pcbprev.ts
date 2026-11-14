// The .board preview: while a .board file is open in the editor, 🔌 PCB on the action bar draws it
// (the nets view and the schematic, netview.ts) beside the text or in a floating window, and draws it
// again as it is edited. No flashing is needed: the core only parses it (pcb_info) and runs the ERC; the
// levels come only on the virtual board. A parse error shows in the window's bar, with its line.

import { tr } from '../i18n';
import { FloatWin, floatWin } from './floatwin';
import { esc, NetView, NvBoard } from './netview';

type Info = (NvBoard & { name: string }) | { error: string };

export class PcbPreview {
  private win: HTMLElement;
  private fw: FloatWin;
  private nets: NetView;
  private msg: HTMLElement;
  private file = '';
  private timer = 0;
  private gen = 0;
  /** the preview is wanted (it opens again with the next .board file) */
  private open = false;

  constructor(private area: HTMLElement, private parse: (src: string) => Promise<Info>) {
    try { this.open = localStorage.getItem('rueda.pcbprev') === '1'; } catch { /* no storage */ }
    this.win = document.createElement('div');
    this.win.className = 'pcb-prev hidden';
    this.win.innerHTML = `<div class="pcb-prev-h"><span class="fw-t">⠿ 🔌 <span class="pcb-prev-name"></span></span><span class="pcb-prev-msg"></span></div><div class="pcb-prev-body"></div>`;
    area.appendChild(this.win);
    this.msg = this.win.querySelector('.pcb-prev-msg')!;
    this.nets = new NetView(this.win.querySelector('.pcb-prev-body')!);
    // docked: beside the text (the editor narrows); floating: over it
    this.fw = floatWin(this.win, this.win.querySelector('.pcb-prev-h')!, { key: 'pcb.preview', auto: () => window.innerWidth < 1000, onChange: () => this.layout() });
  }

  get isOpen(): boolean { return this.open; }

  /** the active file changed: the preview of a .board, if it is wanted */
  show(name: string | undefined, src: string): void {
    const on = !!name?.endsWith('.board');
    this.file = on ? name! : '';
    this.win.classList.toggle('hidden', !(on && this.open));
    this.layout();
    if (on && this.open) this.update(src, 0);
  }

  /** the .board's text changed: draw it again a moment after the typing stops */
  update(src: string, wait = 300): void {
    if (!this.file || !this.open) return;
    clearTimeout(this.timer);
    this.timer = window.setTimeout(async () => {
      const g = ++this.gen;
      let r: Info;
      try { r = await this.parse(src); } catch (e) { r = { error: String((e as Error).message ?? e) }; }
      if (g !== this.gen) return;
      this.win.querySelector('.pcb-prev-name')!.textContent = tr('擴充板', 'Expansion board') + ' ' + ('name' in r && r.name ? r.name : this.file);
      if ('error' in r) {
        // keep the last drawing: the error is usually a line being typed
        this.msg.innerHTML = `<span class="vb-err">✗ ${esc(r.error)}</span>`;
        return;
      }
      const errs = r.erc.filter((d) => d.error).length, warns = r.erc.length - errs;
      this.msg.innerHTML = !r.erc.length ? '<span class="vb-dim">ERC ✓</span>'
        : `<span class="${errs ? 'vb-err' : 'vb-dim'}" title="${esc(r.erc.map((d) => (d.error ? '✗ ' : '⚠ ') + d.msg).join('\n'))}">ERC ${errs ? `✗ ${errs}` : ''} ${warns ? `⚠ ${warns}` : ''}</span>`;
      this.nets.set(r);
    }, wait);
  }

  /** 🔌 PCB: show or hide it (`src`: the open .board's text) */
  toggle(src: string): void {
    this.open = !this.open;
    try { localStorage.setItem('rueda.pcbprev', this.open ? '1' : '0'); } catch { /* no storage */ }
    this.win.classList.toggle('hidden', !(this.open && this.file));
    this.layout();
    if (this.open && this.file) {
      this.fw.unfold();
      this.fw.clamp();
      this.update(src, 0);
    }
  }

  /** docked and shown: the editor gives it the right part of the area */
  private layout(): void {
    this.area.classList.toggle('pcb-split', !this.win.classList.contains('hidden') && !this.fw.floating);
  }
}
