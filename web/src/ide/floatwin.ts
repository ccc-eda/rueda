// Floating windows: a block of the IDE (the bottom panel, the synthesis page's details, the
// emulator's board, the virtual board's peripherals) that can leave its place in the layout and
// float over the page instead of squeezing what is beside it. Its title bar (`handle`) drags it,
// its corner resizes it, ▁ folds it to the title bar, ⧉ / ⇲ floats or docks it. Each window
// remembers its choice, place and size (localStorage, when there is one); without a choice it
// floats when `auto()` says the screen is too small for it docked (folded, with `folded`).

import { tr } from '../i18n';

interface Saved { float?: boolean; min?: boolean; x?: number; y?: number; w?: number; h?: number }

export interface FloatWin {
  readonly floating: boolean;
  set(float: boolean): void;
  /** keep it inside its container (after the window or the layout changed size) */
  clamp(): void;
  /** show it whole if it is folded to its title bar */
  unfold(): void;
}

let top = 20;
const all: FloatWin[] = [];
window.addEventListener('resize', () => { for (const w of all) w.clamp(); });

function load(key: string): Saved {
  try { return JSON.parse(localStorage.getItem('rueda.fw.' + key) ?? '{}') as Saved; } catch { return {}; }
}
function save(key: string, s: Saved): void {
  try { localStorage.setItem('rueda.fw.' + key, JSON.stringify(s)); } catch { /* no storage */ }
}

/** make `el` a floating window dragged by `handle`; `onChange` hears float / dock */
export function floatWin(el: HTMLElement, handle: HTMLElement, o: { key: string; auto: () => boolean; folded?: boolean; onChange?: (floating: boolean) => void }): FloatWin {
  const s = load(o.key);
  // without a choice of its own, it starts folded when `folded` says so
  if (s.min === undefined && o.folded) s.min = true;
  el.classList.add('fw');
  handle.classList.add('fw-h');
  const btns = document.createElement('span');
  btns.className = 'fw-btns';
  btns.innerHTML = `<button class="icon fw-min" data-fw="min"></button><button class="icon" data-fw="float"></button>`;
  handle.appendChild(btns);
  const minB = btns.querySelector<HTMLButtonElement>('[data-fw=min]')!, floatB = btns.querySelector<HTMLButtonElement>('[data-fw=float]')!;
  const box = () => (el.offsetParent as HTMLElement | null) ?? el.parentElement!;

  const place = () => {
    if (!el.classList.contains('fw-float')) return;
    if (s.x !== undefined && s.y !== undefined) { el.style.left = s.x + 'px'; el.style.top = s.y + 'px'; el.style.right = el.style.bottom = 'auto'; }
    if (s.w) el.style.width = s.w + 'px';
    if (s.h && !s.min) el.style.height = s.h + 'px';
  };
  const label = () => {
    const f = el.classList.contains('fw-float'), m = el.classList.contains('fw-folded');
    floatB.textContent = f ? '⇲' : '⧉';
    floatB.title = f ? tr('放回原位', 'Dock it back') : tr('變成浮動視窗（拖曳標題列移動、拖曳右下角縮放）', 'Float it (drag the title bar to move it, the corner to resize it)');
    minB.textContent = m ? '▢' : '▁';
    minB.title = m ? tr('展開', 'Unfold') : tr('收成標題列', 'Fold to the title bar');
    minB.classList.toggle('hidden', !f);
  };
  const w: FloatWin = {
    get floating() { return el.classList.contains('fw-float'); },
    set(f: boolean) {
      el.classList.toggle('fw-float', f);
      el.classList.toggle('fw-folded', f && !!s.min);
      for (const k of ['left', 'top', 'right', 'bottom', 'width', 'height'] as const) el.style[k] = '';
      place();
      label();
      o.onChange?.(f);
      if (f) requestAnimationFrame(() => w.clamp());
    },
    clamp() {
      if (!w.floating || !el.offsetParent) return;
      const p = box().getBoundingClientRect(), r = el.getBoundingClientRect();
      if (r.width > p.width) el.style.width = Math.max(200, p.width - 8) + 'px';
      if (!el.classList.contains('fw-folded') && r.height > p.height) el.style.height = Math.max(80, p.height - 8) + 'px';
      const r2 = el.getBoundingClientRect();
      const x = Math.max(0, Math.min(r2.left - p.left, p.width - r2.width)), y = Math.max(0, Math.min(r2.top - p.top, p.height - 32));
      if (Math.abs(x - (r2.left - p.left)) > 0.5 || Math.abs(y - (r2.top - p.top)) > 0.5) {
        el.style.left = x + 'px'; el.style.top = y + 'px'; el.style.right = el.style.bottom = 'auto';
      }
    },
    unfold() { /* set below */ },
  };
  floatB.onclick = () => { s.float = !w.floating; save(o.key, s); w.set(s.float); };
  const fold = (m: boolean) => { s.min = m; save(o.key, s); el.classList.toggle('fw-folded', m); el.style.height = m ? '' : (s.h ? s.h + 'px' : ''); label(); };
  minB.onclick = () => fold(!el.classList.contains('fw-folded'));
  w.unfold = () => { if (el.classList.contains('fw-folded')) fold(false); };
  // drag by the title bar (not by its buttons or menus)
  handle.addEventListener('pointerdown', (e) => {
    if (!w.floating || (e.target as HTMLElement).closest('button, select, input, a')) return;
    e.preventDefault();
    const p = box().getBoundingClientRect(), r = el.getBoundingClientRect();
    const dx = e.clientX - r.left, dy = e.clientY - r.top;
    const move = (m: PointerEvent) => {
      s.x = Math.round(Math.max(0, Math.min(p.width - r.width, m.clientX - p.left - dx)));
      s.y = Math.round(Math.max(0, Math.min(p.height - 32, m.clientY - p.top - dy)));
      el.style.left = s.x + 'px'; el.style.top = s.y + 'px'; el.style.right = el.style.bottom = 'auto';
    };
    const up = () => { window.removeEventListener('pointermove', move); window.removeEventListener('pointerup', up); save(o.key, s); };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
  });
  // in front when touched
  el.addEventListener('pointerdown', () => { if (w.floating) el.style.zIndex = String(++top); }, true);
  // its size, after a resize at the corner
  new ResizeObserver(() => {
    if (!w.floating || el.classList.contains('fw-folded') || !el.style.width) return;
    const r = el.getBoundingClientRect();
    if (r.width && r.height && (Math.round(r.width) !== s.w || Math.round(r.height) !== s.h)) { s.w = Math.round(r.width); s.h = Math.round(r.height); save(o.key, s); }
  }).observe(el);
  all.push(w);
  w.set(s.float ?? o.auto());
  return w;
}
