// Records the diagram features in dark mode: live preview, inline errors, the viewer toolbar
// and the zoom dialog. Adds a section to architecture/hashing, so run ../seed.sh first.
// Env: PW (demo user password); BASE and EMAIL override the defaults below.
const { chromium } = require('playwright');

const BASE = process.env.BASE || 'http://localhost:3000';
const EMAIL = process.env.EMAIL || 'demo@solidate.dev';
const P = BASE + '/t/heidtmare/p/solidate';
const W = 1280, H = 880;

// Overlay: cursor, click ripple and caption bar.
const overlay = () => {
  const css = `
    #__cur{position:fixed;z-index:2147483647;width:22px;height:22px;margin:-3px 0 0 -3px;pointer-events:none;
      background:url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24'%3E%3Cpath d='M3 2l7 19 2.6-7.4L20 11z' fill='%23fff' stroke='%23111' stroke-width='1.6' stroke-linejoin='round'/%3E%3C/svg%3E") no-repeat;}
    .__rip{position:fixed;z-index:2147483646;width:34px;height:34px;margin:-17px 0 0 -17px;border-radius:50%;pointer-events:none;
      background:rgba(124,156,245,.45);animation:__r .5s ease-out forwards}
    @keyframes __r{from{transform:scale(.3);opacity:1}to{transform:scale(1.6);opacity:0}}
    #__cap{position:fixed;z-index:2147483645;left:50%;bottom:20px;pointer-events:none;transform:translateX(-50%);max-width:1100px;
      background:rgba(124,156,245,.95);color:#0d1117;font:600 22px/1.4 -apple-system,system-ui,sans-serif;padding:10px 22px;border-radius:10px;
      box-shadow:0 6px 24px rgba(0,0,0,.4);text-align:center;transition:opacity .25s}
    #__cap.top{bottom:auto;top:56px}
    #__cap:empty{opacity:0}`;
  const init = () => {
    if (document.getElementById('__cur')) return;
    const st = document.createElement('style'); st.textContent = css; document.head.appendChild(st);
    const cur = document.createElement('div'); cur.id = '__cur'; document.body.appendChild(cur);
    const [x, y] = (sessionStorage.getItem('__pos') || '640,400').split(',');
    cur.style.left = x + 'px'; cur.style.top = y + 'px';
    const cap = document.createElement('div'); cap.id = '__cap'; document.body.appendChild(cap);
    // A modal dialog renders in the top layer, above any z-index; move the overlay into it while open.
    new MutationObserver(() => {
      const d = document.querySelector('dialog[open]');
      if (d && !d.contains(cur)) d.append(cur, cap);
    }).observe(document.body, { childList: true, subtree: true, attributes: true, attributeFilter: ['open'] });
    document.addEventListener('close', () => document.body.append(cur, cap), true);
  };
  window.__setCap = (t, top) => { const c = document.getElementById('__cap'); if (c) { c.textContent = t; c.classList.toggle('top', !!top); } };
  addEventListener('mousemove', e => {
    sessionStorage.setItem('__pos', e.clientX + ',' + e.clientY);
    const c = document.getElementById('__cur'); if (c) { c.style.left = e.clientX + 'px'; c.style.top = e.clientY + 'px'; }
  }, true);
  addEventListener('mousedown', e => {
    const r = document.createElement('div'); r.className = '__rip'; r.style.left = e.clientX + 'px'; r.style.top = e.clientY + 'px';
    document.body.appendChild(r); setTimeout(() => r.remove(), 600);
  }, true);
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', init); else init();
};

const sleep = ms => new Promise(r => setTimeout(r, ms));

const FENCE = '```mermaid\ngraph LR\n  M[Markdown] --> N[Normalize]\n  N --> S[Sync hash]\n  M --> C[Content hash]\n  C --> R[Merkle root]\n```';

(async () => {
  const browser = await chromium.launch();

  // Sign in outside the recording.
  const login = await browser.newContext();
  const lp = await login.newPage();
  await lp.goto(BASE + '/login');
  await lp.fill('input[name=email]', EMAIL);
  await lp.fill('input[name=password]', process.env.PW);
  await lp.click('button[type=submit]');
  await lp.waitForLoadState('networkidle');
  const storageState = await login.storageState();
  await login.close();

  const ctx = await browser.newContext({
    viewport: { width: W, height: H }, deviceScaleFactor: 1, colorScheme: 'dark', storageState,
    permissions: ['clipboard-read', 'clipboard-write'],
    recordVideo: { dir: 'video-diagrams', size: { width: W, height: H } },
  });
  await ctx.addInitScript(overlay);
  const page = await ctx.newPage();

  // `top` places the caption under the header, clear of content at the bottom of the viewport.
  const cap = async (t, hold = 0, top = false) => { await page.evaluate(([t, top]) => window.__setCap(t, top), [t, top]); if (hold) await sleep(hold); };
  const move = (x, y, steps = 25) => page.mouse.move(x, y, { steps });
  const point = async (loc, pause = 500) => {
    const b = await loc.boundingBox();
    await move(b.x + b.width / 2, b.y + b.height / 2); await sleep(pause);
  };
  const click = async (loc, after = 900) => {
    await point(loc, 350); await page.mouse.down(); await page.mouse.up(); await sleep(after);
  };
  const preview = sel => page.waitForSelector('#preview ' + sel, { timeout: 10000 });
  // Keeps the preview scrolled to its end, where the new section is.
  const previewEnd = () => page.evaluate(() => { const p = document.getElementById('preview'); p.scrollTop = p.scrollHeight; });

  // 1. Editor: write a diagram and watch it render.
  await page.goto(P + '/edit/architecture/hashing');
  await preview('h2');
  await page.evaluate(() => {
    const ta = document.querySelector('textarea[name=content]');
    ta.focus(); ta.setSelectionRange(ta.value.length, ta.value.length); ta.scrollTop = ta.scrollHeight;
  });
  await previewEnd();
  await cap('Diagrams are Mermaid fences, previewed as you type', 600);
  await page.keyboard.type('\n## Pipeline {#pipeline}\n\n', { delay: 40 });
  await page.keyboard.insertText(FENCE);
  await preview('figure.diagram'); await sleep(300); await previewEnd();
  await sleep(1800);

  // 2. A mistake shows inline, with the line and the last version that rendered.
  await page.evaluate(() => {
    const ta = document.querySelector('textarea[name=content]');
    const i = ta.value.lastIndexOf('C --> R[Merkle root]') + 'C --> R[Merkle root]'.length;
    ta.setSelectionRange(i, i);
  });
  await cap('Errors show inline, with the failing line and the last good render');
  await page.keyboard.type('\n  R -->', { delay: 90 });
  await preview('.diagram-error'); await sleep(300);
  await page.evaluate(() => {
    const p = document.getElementById('preview'), e = p.querySelector('.diagram-error');
    p.scrollTop += e.getBoundingClientRect().top - p.getBoundingClientRect().top - 8;
  });
  await sleep(2600);

  // 3. The note next to Save selects the failing line.
  await cap('The note next to Save selects the line in the editor');
  await click(page.locator('.diagram-status button'), 1200);
  await page.keyboard.type('  R --> P[Project root]', { delay: 45 });
  await page.waitForFunction(() => !document.querySelector('#preview .diagram-error'), null, { timeout: 10000 });
  await sleep(300); await previewEnd();
  await cap('Fixed', 1500);

  // 4. Save and view.
  await cap('');
  await click(page.locator('form.editor button[type=submit]'), 0);
  await page.waitForURL(u => !u.pathname.includes('/edit/'));
  await page.waitForSelector('#pipeline ~ figure.diagram, figure.diagram');
  await page.evaluate(() => document.getElementById('pipeline').scrollIntoView({ behavior: 'smooth', block: 'start' }));
  await sleep(1200);
  const fig = page.locator('figure.diagram').last();
  await cap('Every diagram has tools: expand, source, copy, SVG', 0, true);
  await point(fig.locator('.diagram-view'), 1400);
  await click(fig.getByRole('button', { name: 'Source' }), 1600);
  await click(fig.getByRole('button', { name: 'Diagram' }), 900);
  await click(fig.getByRole('button', { name: 'Copy' }), 1200);

  // 5. Full size with zoom.
  await cap('Expand opens it full size with zoom');
  await click(fig.getByRole('button', { name: 'Expand' }), 1200);
  const dlg = page.locator('dialog.diagram-dialog');
  await click(dlg.getByRole('button', { name: '+' }), 700);
  await click(dlg.getByRole('button', { name: '+' }), 700);
  await click(dlg.getByRole('button', { name: '+' }), 1000);
  await click(dlg.getByRole('button', { name: 'Fit' }), 1400);
  await click(dlg.getByRole('button', { name: 'Close' }), 800);
  await cap('', 600);

  await ctx.close();
  await browser.close();
})().catch(e => { console.error(e); process.exit(1); });
