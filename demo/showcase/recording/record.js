// Records a walkthrough of the showcase project. Env: SOLIDATE_TOKEN (agent token), PW (demo user password).
const { chromium } = require('playwright');

const BASE = 'http://localhost:3000';
const P = BASE + '/t/heidtmare/p/solidate';
const API = BASE + '/api/v1/projects/solidate';
const TOKEN = process.env.SOLIDATE_TOKEN;
const W = 1440, H = 900;

// Overlay: fake cursor, click ripple, caption bar, code card. State survives navigation via sessionStorage.
const overlay = () => {
  const ss = (k, v) => { try { if (v === undefined) return sessionStorage.getItem(k); if (v === null) sessionStorage.removeItem(k); else sessionStorage.setItem(k, v); } catch (_) {} };
  const css = `
    #__cur{position:fixed;z-index:2147483647;width:22px;height:22px;margin:-3px 0 0 -3px;pointer-events:none;
      background:url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24'%3E%3Cpath d='M3 2l7 19 2.6-7.4L20 11z' fill='%23111' stroke='white' stroke-width='1.6' stroke-linejoin='round'/%3E%3C/svg%3E") no-repeat;}
    .__rip{position:fixed;z-index:2147483646;width:34px;height:34px;margin:-17px 0 0 -17px;border-radius:50%;pointer-events:none;
      background:rgba(37,99,235,.35);animation:__r .5s ease-out forwards}
    @keyframes __r{from{transform:scale(.3);opacity:1}to{transform:scale(1.6);opacity:0}}
    #__cap{position:fixed;z-index:2147483645;left:50%;bottom:28px;transform:translateX(-50%);max-width:1100px;
      background:rgba(17,24,39,.92);color:#fff;font:500 21px/1.4 -apple-system,system-ui,sans-serif;padding:12px 22px;border-radius:10px;
      box-shadow:0 6px 24px rgba(0,0,0,.25);text-align:center;transition:opacity .25s}
    #__cap:empty{opacity:0}
    #__card{position:fixed;z-index:2147483644;right:28px;top:70px;width:620px;background:#0f172a;color:#e2e8f0;border-radius:10px;
      font:13.5px/1.5 ui-monospace,Menlo,monospace;padding:14px 18px;white-space:pre-wrap;box-shadow:0 10px 30px rgba(0,0,0,.35)}
    #__card b{color:#93c5fd;font-weight:600}`;
  const init = () => {
    if (document.getElementById('__cur')) return;
    const st = document.createElement('style'); st.textContent = css; document.head.appendChild(st);
    const cur = document.createElement('div'); cur.id = '__cur'; document.body.appendChild(cur);
    const [x, y] = (ss('__pos') || '720,450').split(',');
    cur.style.left = x + 'px'; cur.style.top = y + 'px';
    const cap = document.createElement('div'); cap.id = '__cap'; cap.textContent = ss('__cap') || ''; document.body.appendChild(cap);
    const card = ss('__card'); if (card) window.__setCard(card);
  };
  window.__setCap = t => { ss('__cap', t); const c = document.getElementById('__cap'); if (c) c.textContent = t; };
  window.__setCard = h => {
    ss('__card', h); let c = document.getElementById('__card');
    if (!h) { if (c) c.remove(); return; }
    if (!c) { c = document.createElement('div'); c.id = '__card'; document.body.appendChild(c); }
    c.innerHTML = h;
  };
  addEventListener('mousemove', e => {
    ss('__pos', e.clientX + ',' + e.clientY);
    const c = document.getElementById('__cur'); if (c) { c.style.left = e.clientX + 'px'; c.style.top = e.clientY + 'px'; }
  }, true);
  addEventListener('mousedown', e => {
    const r = document.createElement('div'); r.className = '__rip'; r.style.left = e.clientX + 'px'; r.style.top = e.clientY + 'px';
    document.body.appendChild(r); setTimeout(() => r.remove(), 600);
  }, true);
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', init); else init();
};

const sleep = ms => new Promise(r => setTimeout(r, ms));

async function api(method, path, body) {
  const r = await fetch(API + path, {
    method,
    headers: { Authorization: `Bearer ${TOKEN}`, ...(body ? { 'Content-Type': 'application/json' } : {}) },
    body: body ? JSON.stringify(body) : undefined,
  });
  if (!r.ok) throw new Error(`${method} ${path}: ${r.status} ${await r.text()}`);
  return r.status === 204 ? null : r.json();
}

// Proposes `variant` of `path` with one text substitution, as the agent.
async function propose(path, variant, from, to, message, resolves) {
  const d = await api('GET', `/docs/${path}?variant=${variant}&format=json`);
  if (!d.content.includes(from)) throw new Error(`${path} (${variant}): anchor text not found`);
  await api('POST', `/propose/${path}`, {
    variant, content: d.content.replace(from, to), base_hash: d.content_hash, message, resolves,
  });
}

(async () => {
  const browser = await chromium.launch();
  const ctx = await browser.newContext({
    viewport: { width: W, height: H }, deviceScaleFactor: 1,
    recordVideo: { dir: 'video', size: { width: W, height: H } },
  });
  await ctx.addInitScript(overlay);
  const page = await ctx.newPage();
  let mx = 720, my = 450;

  const cap = async (t, hold = 0) => { await page.evaluate(t => window.__setCap(t), t); if (hold) await sleep(hold); };
  const card = h => page.evaluate(h => window.__setCard(h), h);
  const go = async url => { await page.goto(url); await page.waitForLoadState('networkidle'); await sleep(400); };
  const move = async (x, y, steps = 25) => { await page.mouse.move(x, y, { steps }); mx = x; my = y; };
  const box = async loc => {
    await loc.scrollIntoViewIfNeeded(); await sleep(250);
    const b = await loc.boundingBox(); return [b.x + b.width / 2, b.y + b.height / 2];
  };
  const point = async (loc, pause = 500) => { const [x, y] = await box(loc); await move(x, y); await sleep(pause); };
  const click = async (loc, after = 900) => {
    await point(loc, 350); await page.mouse.down(); await page.mouse.up();
    await page.waitForLoadState('networkidle').catch(() => {}); await sleep(after);
  };
  const scroll = async (dy, steps = 12) => {
    for (let i = 0; i < steps; i++) { await page.mouse.wheel(0, dy / steps); await sleep(35); }
    await sleep(500);
  };
  const scrollToEl = async (sel, block = 'start') => {
    await page.evaluate(([s, b]) => document.querySelector(s).scrollIntoView({ behavior: 'smooth', block: b }), [sel, block]);
    await sleep(1100);
  };
  const scrollTop = async () => { await page.evaluate(() => scrollTo({ top: 0, behavior: 'smooth' })); await sleep(900); };
  // Places the textarea caret right after `after` (or at the end) and scrolls it into view.
  const caret = async after => {
    await page.evaluate(after => {
      const ta = document.querySelector('textarea[name=content]');
      const i = after == null ? ta.value.length : ta.value.indexOf(after) + after.length;
      ta.focus(); ta.setSelectionRange(i, i);
      const lh = parseFloat(getComputedStyle(ta).lineHeight) || 22;
      const line = ta.value.slice(0, i).split('\n').length;
      ta.scrollTop = Math.max(0, line * lh - ta.clientHeight / 2);
    }, after);
    await sleep(400);
  };
  const type = async (text, delay = 28) => { await page.keyboard.type(text, { delay }); await sleep(900); };
  const syncPreview = () => page.evaluate(() => {
    const ta = document.querySelector('textarea[name=content]'), pv = document.getElementById('preview');
    const f = ta.scrollTop / Math.max(1, ta.scrollHeight - ta.clientHeight);
    pv.scrollTop = f * (pv.scrollHeight - pv.clientHeight);
  });

  // 1. Sign in
  await go(BASE + '/login');
  await cap('Solidate: one source of truth, written for people and for AI agents', 2200);
  await click(page.locator('input[name=email]'), 200);
  await type('demo@solidate.dev', 35);
  await click(page.locator('input[name=password]'), 200);
  await type(process.env.PW, 15);
  await click(page.locator('button[type=submit]'), 800);

  // 2. Project home
  await go(P);
  await cap('The “solidate” project: Solidate’s own docs, stored in Solidate', 2000);
  await point(page.getByText('Inherits from handbook'), 400);
  await cap('It inherits the glossary, style rules and translation guide from “handbook”', 2200);
  await point(page.locator('a', { hasText: 'glossary' }).first(), 1200);
  await scroll(500); await sleep(900); await scroll(-500);

  // 3. Readme: human variant, include, AI variant
  await click(page.locator('a', { hasText: /^readme$/ }).first());
  await cap('Every document has a human variant: narrative prose', 2400);
  await scrollToEl('#vocabulary');
  await cap('Vocabulary is transcluded from handbook:glossary with {{include}}', 1600);
  await point(page.locator('aside').getByText('handbook:glossary#core-terms'), 1800);
  await scrollTop();
  await cap('…and an AI variant: the same facts as dense reference', 600);
  await click(page.locator('a', { hasText: /^AI$/ }).first(), 1800);
  await scroll(700); await sleep(1200); await scrollTop();

  // 4. Sync queue
  await cap('Sections are paired by anchor and hashed; the sync queue lists every section that drifted', 600);
  await go(P + '/sync');
  await sleep(2600);
  await point(page.locator('span.state', { hasText: 'human changed' }).first(), 700);
  await point(page.locator('span.state', { hasText: 'AI changed' }).first(), 700);
  await point(page.locator('span.state', { hasText: 'both changed' }).first(), 1200);

  // 5. Conflict details
  await cap('A conflict: both variants changed the same section since the last sync', 400);
  await click(page.locator('td a', { hasText: 'architecture/hashing' }));
  await click(page.locator('tr', { hasText: '#semantic' }).getByRole('button', { name: 'Details' }), 1000);
  await scrollToEl('.sync-item', 'center');
  await cap('Each side shows its diff since the last sync; the base text is one click away', 3000);

  // 6. Accept an agent's proposal
  await go(P + '/sync');
  await cap('An agent translated the quickstart change and submitted a proposal', 600);
  await click(page.locator('td a', { hasText: 'guide/quickstart' }));
  await point(page.getByText('By an agent (API token)'), 600);
  await scrollToEl('section.proposal pre.diff', 'center');
  await cap('Agents never write the other variant directly: a person reviews the diff', 2600);
  await scrollTop();
  await cap('Accept', 300);
  await click(page.getByRole('button', { name: 'Accept' }), 1400);
  await cap('Accepted: the AI variant is updated and the section is back in sync', 2400);

  // 7. Human translates by hand
  await go(P + '/sync');
  await cap('A person can also translate by hand. The Diagrams section exists only in the human variant', 800);
  await click(page.locator('td a', { hasText: 'guide/writing-documents' }));
  await click(page.locator('tr', { hasText: '#diagrams' }).getByRole('link', { name: /Translate into ai by hand/ }), 1000);
  await cap('The editor shows the human text as a reference', 2200);
  await scrollToEl('form.editor', 'start');
  await caret(null);
  await cap('Write the AI version, with a live preview', 300);
  await type('\n## Diagrams {#diagrams}\n\n', 45);
  await syncPreview();
  await type('- box diagrams: fenced `text` blocks keep layout; example: [[architecture/overview#layers]].\n', 22);
  await syncPreview();
  await type('- Mermaid: not rendered; shown as code.\n', 22);
  await sleep(700); await syncPreview(); await sleep(1500);
  await click(page.locator('input[name=message]'), 200);
  await type('Translate #diagrams', 35);
  await click(page.getByRole('button', { name: 'Save' }), 1200);
  await cap('Saving marks #diagrams in sync', 2400);

  // 8. Human edit, then an agent translates it
  await go(P + '/d/readme');
  await cap('Now a person edits the human readme', 600);
  await click(page.getByRole('link', { name: 'Edit', exact: true }), 900);
  const anchor = '- How it is built: [[architecture/overview]].';
  await caret(anchor);
  await sleep(600);
  await type('\n- Why it is built this way: the design decisions, starting with\n  [[decisions/0002-dual-variants]].', 26);
  await syncPreview(); await sleep(900);
  await click(page.locator('input[name=message]'), 200);
  await type('Link the design decisions', 35);
  await click(page.getByRole('button', { name: 'Save' }), 1200);
  await cap('The AI variant is now behind: the Sync badge counts stale sections', 400);
  await point(page.getByRole('link', { name: /Sync/ }).first(), 2000);
  await go(P + '/sync');
  await cap('The queue gains a “human changed” entry for the readme', 2600);

  await cap('An agent works the queue through the REST API (or MCP), with a scoped token', 400);
  await card(
    '<b>GET</b>  /api/v1/projects/solidate/sync\n' +
    '<b>GET</b>  /api/v1/projects/solidate/translation-guide\n' +
    '<b>POST</b> /api/v1/projects/solidate/propose/readme\n' +
    '     {"variant":"ai", "resolves":["next"], "base_hash":"…", "content":"…"}\n' +
    '<b>POST</b> /api/v1/projects/solidate/propose/reference/configuration\n' +
    '     {"variant":"human", "resolves":["limits"], "base_hash":"…", "content":"…"}');
  await sleep(3200);
  await propose('readme', 'ai',
    '- internals: [[architecture/overview]]',
    '- internals: [[architecture/overview]]\n- rationale: [[decisions/0002-dual-variants]] (ADRs 0001-0004)',
    'Translate the new design-decisions link.', ['next']);
  await propose('reference/configuration', 'human',
    'levels deep. These are not configurable at runtime.',
    'levels deep. Request bodies for document writes and proposals share the 1 MiB\nlimit. These are not configurable at runtime.',
    'Carry over the request body limit from the AI variant.', ['limits']);
  await card(null);
  await go(P + '/sync');
  await cap('Two proposals await review: human → AI and AI → human', 2400);

  await click(page.locator('ul.plain a', { hasText: 'readme' }));
  await scrollToEl('section.proposal pre.diff', 'center');
  await cap('Review the agent’s translation of the human edit', 2400);
  await scrollTop();
  await click(page.getByRole('button', { name: 'Accept' }), 1200);

  await go(P + '/sync');
  await click(page.locator('ul.plain a', { hasText: 'reference/configuration' }));
  await scrollToEl('section.proposal pre.diff', 'center');
  await cap('And the reverse direction: an AI-side change carried into the human prose', 2400);
  await scrollTop();
  await click(page.getByRole('button', { name: 'Accept' }), 1200);

  await go(P + '/sync');
  await cap('The queue is down to the one conflict, which needs a person to reconcile', 3000);

  // 9. History
  await go(P + '/history/readme');
  await cap('History records every revision, by person or agent, with its change note', 3200);

  // 10. Search and llms.txt
  await go(P);
  await cap('Full-text search across both variants', 300);
  await click(page.locator('input[type=search], input[name=q]').first(), 200);
  await type('merkle', 90);
  await page.keyboard.press('Enter'); await page.waitForLoadState('networkidle'); await sleep(2800);
  await cap('llms.txt points language models at the AI variants', 400);
  await go(P + '/llms.txt');
  await sleep(3000);
  await cap('Solidate', 1500);

  await page.close();
  await ctx.close();
  await browser.close();
})().catch(e => { console.error(e); process.exit(1); });
