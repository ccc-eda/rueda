// End-to-end test of the IDE in a real browser: the built web/dist/ served over HTTP, driven by
// Playwright (playwright-core with the installed Chrome: no browser download).
//   node test/e2e.mjs                 (npm run e2e; CHROME=/path/to/chrome for another browser)
//   HEADED=1 node test/e2e.mjs        watch it
// Each check runs in a fresh browser context (empty storage), at two window sizes.
import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { chromium } from 'playwright-core';

const dist = path.join(path.dirname(fileURLToPath(import.meta.url)), '..', 'dist');
const TYPES = { '.html': 'text/html', '.js': 'text/javascript', '.cjs': 'text/javascript', '.css': 'text/css' };
const server = http.createServer((req, res) => {
  const p = path.join(dist, decodeURIComponent(new URL(req.url, 'http://x').pathname));
  if (!p.startsWith(dist) || !fs.existsSync(p) || fs.statSync(p).isDirectory()) { res.writeHead(404); res.end(); return; }
  res.writeHead(200, { 'content-type': TYPES[path.extname(p)] ?? 'application/octet-stream' });
  fs.createReadStream(p).pipe(res);
});
await new Promise((r) => server.listen(0, '127.0.0.1', r));
const base = `http://127.0.0.1:${server.address().port}/index.html`;

const browser = await chromium.launch(process.env.CHROME ? { executablePath: process.env.CHROME, headless: !process.env.HEADED } : { channel: 'chrome', headless: !process.env.HEADED });
let bad = 0;
const SIZES = [{ width: 1280, height: 800 }, { width: 1680, height: 1050 }];

/** a fresh page at `hash`, the WebAssembly core loaded */
async function open(size, hash = '') {
  const ctx = await browser.newContext({ viewport: size });
  const page = await ctx.newPage();
  const errors = [];
  page.on('pageerror', (e) => errors.push(e.message));
  await page.goto(base + hash);
  await page.waitForFunction(() => document.getElementById('sbCore')?.textContent.startsWith('✓'), null, { timeout: 30000 });
  return { ctx, page, errors };
}

/** pixels of the synthesis canvas that are not its background (#171c24): something was drawn */
const drawn = (page) => page.evaluate(() => {
  const c = document.getElementById('synCanvas');
  if (!c.width || !c.height) return 0;
  const d = c.getContext('2d').getImageData(0, 0, c.width, c.height).data;
  let n = 0;
  for (let i = 0; i < d.length; i += 4) if (Math.abs(d[i] - 0x17) + Math.abs(d[i + 1] - 0x1c) + Math.abs(d[i + 2] - 0x24) > 24) n++;
  return n;
});

/** paint the canvas over with its background, so a stage that never repaints shows up as empty
 * (instead of still showing the previous stage's drawing) */
const blank = (page) => page.evaluate(() => {
  const c = document.getElementById('synCanvas');
  const x = c.getContext('2d');
  x.setTransform(1, 0, 0, 1, 0, 0);
  x.fillStyle = '#171c24';
  x.fillRect(0, 0, c.width, c.height);
});

/** wait (without touching the page) until the canvas shows a drawing; its pixel count, or 0 */
async function waitDrawn(page, ms = 4000) {
  const t0 = Date.now();
  for (;;) {
    const n = await drawn(page);
    if (n > 200 || Date.now() - t0 > ms) return n;
    await page.waitForTimeout(100);
  }
}

async function check(name, size, fn) {
  const t0 = Date.now();
  try {
    const why = await fn();
    if (why) { bad++; console.log(`FAIL ${name} ${size.width}x${size.height}: ${why}`); }
    else console.log(`ok   ${name} ${size.width}x${size.height}  (${Date.now() - t0} ms)`);
  } catch (e) {
    bad++;
    console.log(`FAIL ${name} ${size.width}x${size.height}: ${e.message.split('\n')[0]}`);
  }
}

for (const size of SIZES) {
  // the stage menu: every netlist stage is drawn as soon as it is chosen, without a click on the canvas
  await check('flow stages drawn without a click', size, async () => {
    const { ctx, page, errors } = await open(size, '#ex=seq101');
    await page.click('#guide [data-step="synth"]');
    await page.waitForSelector('#flowStage', { state: 'visible' });
    const missing = [];
    for (const stage of ['raw', 'opt', 'map', 'rtl', 'raw', 'opt']) {
      await blank(page);
      await page.selectOption('#flowStage', stage);
      const n = await waitDrawn(page);
      if (n <= 200) missing.push(stage);
    }
    await ctx.close();
    return missing.length ? `nothing drawn at ${missing.join(', ')}` : errors.length ? `page errors: ${errors[0]}` : null;
  });

  // the same, the flow opened from the suggested step ③ first
  await check('flow from the suggested step, then Logic Netlist', size, async () => {
    const { ctx, page } = await open(size, '#ex=seq101');
    await page.click('#guide [data-step="synth"]');
    if (await waitDrawn(page) <= 200) { await ctx.close(); return 'RTL view not drawn'; }
    await blank(page);
    await page.selectOption('#flowStage', 'raw');
    const n = await waitDrawn(page);
    await ctx.close();
    return n <= 200 ? 'Logic Netlist not drawn until clicked' : null;
  });

  // a design too big to draw at ② / ③ (RvBoard, ~290k cells): the canvas says so at once, a click draws
  // nothing (it used to show the ports alone, unfitted), and ④ is drawn
  await check('RvBoard: too big to draw, said on the canvas; a click draws nothing', size, async () => {
    const { ctx, page } = await open(size, '#ex=c_sevenseg');
    await page.click('#guide [data-step="synth"]');
    await page.waitForFunction(() => /RTL→Logic/.test(document.querySelector('.flowpane')?.innerText ?? ''), null, { timeout: 60000 });
    const why = [];
    for (const stage of ['raw', 'opt']) {
      await page.selectOption('#flowStage', stage);
      const msg = await page.waitForSelector('#synEmpty:not(.hidden)', { timeout: 5000 }).then((e) => e.textContent()).catch(() => '');
      if (!/\d{5,}/.test(msg)) why.push(`${stage}: no "too many to draw" message`);
      await blank(page);
      const box = await page.$eval('#synCanvas', (c) => { const r = c.getBoundingClientRect(); return [r.x + r.width / 2, r.y + r.height / 2]; });
      await page.mouse.click(box[0], box[1]);
      await page.waitForTimeout(500);
      if (await drawn(page) > 200) why.push(`${stage}: a click drew something`);
    }
    await page.selectOption('#flowStage', 'map');
    if (!(await page.$('#synEmpty.hidden'))) why.push('the message stays at ④');
    if (await waitDrawn(page, 15000) <= 200) why.push('④ not drawn');
    await ctx.close();
    return why.join('; ') || null;
  });

  // the suggested steps: one line; each step's explanation is its tooltip
  await check('suggested steps on one line, hints as tooltips', size, async () => {
    const { ctx, page } = await open(size, '#ex=seq101');
    const g = await page.evaluate(() => {
      const el = document.getElementById('guide');
      const steps = [...el.querySelectorAll('.step')];
      return { visible: !el.classList.contains('hidden'), h: el.offsetHeight, n: steps.length, spans: el.querySelectorAll('.step span').length, titled: steps.every((s) => s.title.length > 5) };
    });
    await ctx.close();
    if (!g.visible || !g.n) return 'no suggested steps';
    if (g.spans) return `${g.spans} hint line(s) still under the steps`;
    if (!g.titled) return 'a step has no tooltip';
    if (g.h > 44) return `the bar is ${g.h} px high (wrapped)`;
    return null;
  });

  // the welcome page: a Fluxa starting point that opens a Fluxa example whose tests pass
  await check('welcome page: Fluxa starting point, tests pass', size, async () => {
    const { ctx, page, errors } = await open(size);
    await page.waitForSelector('#welcome:not(.hidden) [data-goal="fluxa"]', { timeout: 5000 });
    await page.click('#welcome [data-goal="fluxa"]');
    await page.waitForFunction(() => [...document.querySelectorAll('#fileList .nm')].some((e) => e.textContent.endsWith('.fx')), null, { timeout: 5000 });
    await page.click('#mainBtn');
    await page.waitForSelector('#p-results .banner', { timeout: 20000 });
    const banner = await page.textContent('#p-results .banner');
    await ctx.close();
    if (!/✓/.test(banner)) return `tests: ${banner}`;
    return errors.length ? `page errors: ${errors[0]}` : null;
  });

  // the largest Fluxa example: the RV64G processor's 90 tests run in the browser (the cooperative scheduler)
  await check('Fluxa RV64 example: all tests pass in the browser', size, async () => {
    const { ctx, page } = await open(size, '#ex=fx_rv64');
    await page.click('#mainBtn');
    await page.waitForSelector('#p-results .banner', { timeout: 120000 });
    const banner = await page.textContent('#p-results .banner');
    await ctx.close();
    return /✓ 90 /.test(banner) ? null : `tests: ${banner}`;
  });

  // a Fluxa example from the list: the deadlock is reported, synthesis is off
  await check('Fluxa deadlock example reported, synthesis off', size, async () => {
    const { ctx, page } = await open(size, '#ex=fx_deadlock');
    // the suggested first step of a Fluxa program is its tests; synthesis is not offered
    const steps = await page.$$eval('#guide [data-step]', (b) => b.map((x) => x.dataset.step));
    await page.click('#mainBtn');
    await page.waitForFunction(() => /deadlock/.test(document.getElementById('p-output').textContent), null, { timeout: 20000 });
    await ctx.close();
    return steps[0] !== 'test' ? `first step ${steps[0]}` : steps.includes('synth') ? 'synthesis is suggested' : null;
  });
}

await browser.close();
server.close();
console.log(bad ? `${bad} failure(s)` : 'all passed');
process.exit(bad ? 1 : 0);
