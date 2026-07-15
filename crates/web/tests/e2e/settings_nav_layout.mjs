// Headless layout check for the settings-sheet nav strip (UX fix item 1).
// Explicitly invoked — NOT part of `just check` (that stays browser-free).
//
// Serves the REAL built SPA (`../../dist`, produced by `trunk build`) from a fake
// BFF that answers the initial-load commands plus `list_connections` with MANY
// connections, so the Connections panel body is long enough to scroll on a
// phone-sized viewport. It asserts, in a real browser, that: (1) the section nav
// strip (`.sheet-tabs`) stays at (near) full height — it is NOT squished away —
// even though the panel below overflows; (2) the panel body (`.sheet-body`), not
// the whole sheet, is the vertical scroll container; and (3) after scrolling the
// body down, the nav strip stays fixed (same top, same height) and fully visible.
// This is the layout regression reported in live QA. Fails on any uncaught wasm
// panic.
import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { WebSocketServer } from 'ws';
import { chromium } from 'playwright';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const DIST = path.resolve(HERE, '../../dist');
const PORT = 9395;
const BEARER = 'adele.bearer';

if (!fs.existsSync(path.join(DIST, 'index.html'))) {
  console.error(`No built SPA at ${DIST}. Run \`trunk build\` in crates/web first.`);
  process.exit(2);
}

const reply = (id, result) => JSON.stringify({ result: { id, result } });

// A long list of connections so the Connections panel body overflows the sheet.
const CONNECTIONS = Array.from({ length: 16 }, (_, i) => ({
  id: `conn-${i}`,
  connector_type: 'openai',
  display_label: `conn-${i} (openai)`,
  availability: { status: 'ok' },
  has_credentials: true,
  config: null,
}));

// Non-connections replies for the initial load (one chat-capable model, one
// conversation). The reducer also fetches the scratchpad on load — answer empty.
const RESULTS = (id) => ({
  list_available_models: reply(id, {
    models: [
      { connection_id: 'conn-0', connection_label: 'conn-0 (openai)', model: { id: 'gpt-4o', display_name: 'GPT-4o', context_limit: 128000, capabilities: { reasoning: false, vision: true, tools: true, embedding: false } } },
    ],
  }),
  get_purposes: reply(id, { purposes: { interactive: { connection: 'conn-0', model: 'gpt-4o' }, dreaming: { connection: 'conn-0', model: 'gpt-4o' }, consolidation: { connection: 'conn-0', model: 'gpt-4o' }, embedding: { connection: 'conn-0', model: 'gpt-4o' } } }),
  list_conversations: reply(id, { conversations: [{ id: 'c1', title: 'Layout Probe', message_count: 0, updated_at: '2026-07-14 00:00:00', archived: false }] }),
  get_conversation: reply(id, { conversation: { id: 'c1', title: 'Layout Probe', messages: [] } }),
  subscribe_conversations: reply(id, 'ack'),
  get_conversation_scratchpad: reply(id, { scratchpad: [] }),
});

const MIME = { '.html': 'text/html', '.js': 'text/javascript', '.wasm': 'application/wasm', '.css': 'text/css' };
const server = http.createServer((req, res) => {
  const url = new URL(req.url, `http://${req.headers.host}`);
  if (url.pathname === '/login' && req.method === 'POST') { res.writeHead(200, { 'content-type': 'application/json' }); res.end(JSON.stringify({ token: 'faketoken' })); return; }
  if (url.pathname === '/auth/config') { res.writeHead(200, { 'content-type': 'application/json' }); res.end(JSON.stringify({ methods: ['password'] })); return; }
  let fp = path.join(DIST, url.pathname === '/' ? 'index.html' : url.pathname);
  if (!fs.existsSync(fp) || fs.statSync(fp).isDirectory()) fp = path.join(DIST, 'index.html');
  res.writeHead(200, { 'content-type': MIME[path.extname(fp)] || 'application/octet-stream' });
  res.end(fs.readFileSync(fp));
});

const wss = new WebSocketServer({ server, path: '/ws', handleProtocols: (p) => (p.has(BEARER) ? BEARER : false) });
wss.on('connection', (sock) => {
  sock.on('message', (data) => {
    const o = JSON.parse(data.toString());
    const key = typeof o.command === 'string' ? o.command : Object.keys(o.command)[0];
    if (key === 'list_connections') { sock.send(reply(o.id, { connections: CONNECTIONS })); return; }
    const out = RESULTS(o.id)[key];
    if (out) sock.send(out);
  });
});

async function main() {
  await new Promise((r) => server.listen(PORT, '127.0.0.1', r));
  const browser = await chromium.launch({ headless: true });
  let failure = null;
  try {
    // A phone-sized viewport with a short height, so a long panel overflows.
    const context = await browser.newContext({ viewport: { width: 390, height: 667 } });
    const page = await context.newPage();
    page.on('pageerror', (e) => { failure = `uncaught wasm error: ${e.message}`; });
    await page.goto(`http://127.0.0.1:${PORT}`, { waitUntil: 'domcontentloaded' });
    await page.fill('input[placeholder="Username"]', 'dave');
    await page.fill('input[type="password"]', 'testpass123');
    await page.click('button[type="submit"]');
    await page.waitForSelector('form.composer', { timeout: 15000 });
    await page.waitForSelector('span.dot.online', { timeout: 15000 });

    // Open Settings → Connections (the long panel).
    await page.click('button[aria-label="Open settings"]');
    await page.locator('.sheet-tab', { hasText: 'Connections' }).click();
    await page.waitForFunction(
      () => document.querySelectorAll('.conn-card').length >= 16,
      { timeout: 15000 },
    );

    const measure = () => page.evaluate(() => {
      const tabs = document.querySelector('.sheet-tabs');
      const body = document.querySelector('.sheet-body');
      const sheet = document.querySelector('.settings-sheet');
      const r = (el) => el.getBoundingClientRect();
      return {
        tabsTop: r(tabs).top,
        tabsHeight: r(tabs).height,
        bodyScrollH: body.scrollHeight,
        bodyClientH: body.clientHeight,
        bodyScrollTop: body.scrollTop,
        sheetHeight: r(sheet).height,
        viewportH: window.innerHeight,
      };
    });

    const before = await measure();
    console.log('before scroll:', JSON.stringify(before));

    // (1) The nav strip is NOT squished — it keeps (near) full height. Before the
    // fix it collapsed toward zero as the body overflowed.
    if (before.tabsHeight < 44) { failure = `nav strip squished: tabsHeight=${before.tabsHeight} (< 44)`; return; }

    // (2) The panel body — not the whole sheet — is the scroll container: it
    // overflows its own client box, and the sheet stays within the viewport.
    if (before.bodyScrollH <= before.bodyClientH + 20) { failure = `panel body is not scrollable: scrollH=${before.bodyScrollH} clientH=${before.bodyClientH}`; return; }
    if (before.sheetHeight > before.viewportH + 1) { failure = `sheet overflows the viewport: sheetHeight=${before.sheetHeight} viewportH=${before.viewportH}`; return; }

    // (3) Scroll the body down; the nav strip stays fixed and fully visible.
    await page.locator('.sheet-body').evaluate((el) => { el.scrollTop = el.scrollHeight; });
    await page.waitForFunction(() => document.querySelector('.sheet-body').scrollTop > 50, { timeout: 5000 });
    const after = await measure();
    console.log('after scroll:', JSON.stringify(after));

    if (after.bodyScrollTop <= before.bodyScrollTop) { failure = `body did not scroll: before=${before.bodyScrollTop} after=${after.bodyScrollTop}`; return; }
    if (Math.abs(after.tabsTop - before.tabsTop) > 1) { failure = `nav strip moved on scroll: top ${before.tabsTop} -> ${after.tabsTop}`; return; }
    if (Math.abs(after.tabsHeight - before.tabsHeight) > 1) { failure = `nav strip height changed on scroll: ${before.tabsHeight} -> ${after.tabsHeight}`; return; }
    if (after.tabsTop < 0) { failure = `nav strip scrolled off the top: tabsTop=${after.tabsTop}`; return; }
    // The tabs must still sit above the body's scrolled content (i.e. it is a
    // sibling header, not part of the scrolled area).
    const connTab = await page.locator('.sheet-tab', { hasText: 'Connections' }).boundingBox();
    if (!connTab || connTab.height < 44) { failure = `Connections tab not fully visible after scroll: ${JSON.stringify(connTab)}`; return; }
    console.log('nav strip stayed fixed and fully visible while the panel body scrolled.');
  } finally {
    await browser.close();
    server.close();
  }
  if (failure) { console.error(`FAIL: ${failure}`); process.exit(1); }
  console.log('PASS: settings nav strip stays fixed; only the panel body scrolls.');
}
main();
