// Headless end-to-end check for the per-connection "Refresh models" action (UX
// fix item 3). Explicitly invoked — NOT part of `just check` (browser-free).
//
// Serves the REAL built SPA (`../../dist`, produced by `trunk build`) from a fake
// BFF that lists one Bedrock connection and records the scoped
// `list_available_models` the refresh action issues. It asserts, in a real
// browser, that: (1) configuring a connection shows a "Refresh models" button;
// (2) clicking it sends `list_available_models { connection_id: <id>, refresh:
// true }` on the wire (the cache-bypassing scoped form the KCM uses for Bedrock);
// and (3) the resulting model count surfaces inline ("2 models available").
// Fails on any uncaught wasm panic.
import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { WebSocketServer } from 'ws';
import { chromium } from 'playwright';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const DIST = path.resolve(HERE, '../../dist');
const PORT = 9397;
const BEARER = 'adele.bearer';

if (!fs.existsSync(path.join(DIST, 'index.html'))) {
  console.error(`No built SPA at ${DIST}. Run \`trunk build\` in crates/web first.`);
  process.exit(2);
}

const reply = (id, result) => JSON.stringify({ result: { id, result } });

const BEDROCK = {
  id: 'aws',
  connector_type: 'bedrock',
  display_label: 'aws (bedrock)',
  availability: { status: 'ok' },
  has_credentials: true,
  config: { type: 'bedrock', aws_profile: 'adele', region: 'us-east-1' },
};

// The two models a scoped refresh returns for the `aws` connection.
const model = (id, name) => ({ connection_id: 'aws', connection_label: 'aws (bedrock)', model: { id, display_name: name, context_limit: 200000, capabilities: { reasoning: true, vision: true, tools: true, embedding: false } } });
const REFRESHED = [model('anthropic.claude-3-5-sonnet', 'Claude 3.5 Sonnet'), model('anthropic.claude-3-haiku', 'Claude 3 Haiku')];

// Records the scoped (per-connection) list_available_models calls.
const scopedRefreshes = [];

const RESULTS = (id) => ({
  get_purposes: reply(id, { purposes: { interactive: { connection: 'aws', model: 'anthropic.claude-3-5-sonnet' }, dreaming: { connection: 'aws', model: 'anthropic.claude-3-5-sonnet' }, consolidation: { connection: 'aws', model: 'anthropic.claude-3-5-sonnet' }, embedding: { connection: 'aws', model: 'anthropic.claude-3-5-sonnet' } } }),
  list_conversations: reply(id, { conversations: [{ id: 'c1', title: 'Refresh Probe', message_count: 0, updated_at: '2026-07-14 00:00:00', archived: false }] }),
  get_conversation: reply(id, { conversation: { id: 'c1', title: 'Refresh Probe', messages: [] } }),
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
    if (key === 'list_connections') { sock.send(reply(o.id, { connections: [BEDROCK] })); return; }
    if (key === 'list_available_models') {
      const p = o.command.list_available_models;
      // The initial load is unscoped (connection_id: null, refresh: false); the
      // per-connection refresh is scoped + refresh:true. Only the latter returns
      // the refreshed model set (and is recorded).
      if (p.connection_id) {
        scopedRefreshes.push({ connection_id: p.connection_id, refresh: p.refresh });
        sock.send(reply(o.id, { models: REFRESHED }));
      } else {
        sock.send(reply(o.id, { models: [REFRESHED[0]] }));
      }
      return;
    }
    const out = RESULTS(o.id)[key];
    if (out) sock.send(out);
  });
});

const waitFor = async (predicate, ms = 15000) => {
  const start = Date.now();
  while (Date.now() - start < ms) {
    if (predicate()) return true;
    await new Promise((r) => setTimeout(r, 50));
  }
  return false;
};

async function main() {
  await new Promise((r) => server.listen(PORT, '127.0.0.1', r));
  const browser = await chromium.launch({ headless: true });
  let failure = null;
  try {
    const page = await (await browser.newContext({ viewport: { width: 390, height: 800 } })).newPage();
    page.on('pageerror', (e) => { failure = `uncaught wasm error: ${e.message}`; });
    await page.goto(`http://127.0.0.1:${PORT}`, { waitUntil: 'domcontentloaded' });
    await page.fill('input[placeholder="Username"]', 'dave');
    await page.fill('input[type="password"]', 'testpass123');
    await page.click('button[type="submit"]');
    await page.waitForSelector('form.composer', { timeout: 15000 });
    await page.waitForSelector('span.dot.online', { timeout: 15000 });

    // Open Settings → Connections → Configure the connection.
    await page.click('button[aria-label="Open settings"]');
    await page.locator('.sheet-tab', { hasText: 'Connections' }).click();
    await page.waitForSelector('.conn-card', { timeout: 15000 });
    await page.locator('.conn-card', { hasText: 'aws' }).locator('button', { hasText: 'Configure' }).click();

    // (1) The "Refresh models" action is present in the edit form.
    const refreshBtn = page.locator('.conn-refresh-btn', { hasText: 'Refresh models' });
    await refreshBtn.waitFor({ timeout: 5000 });

    // (2) Clicking it sends the scoped, cache-bypassing list_available_models.
    await refreshBtn.click();
    if (!await waitFor(() => scopedRefreshes.length === 1)) { failure = `scoped refresh not sent (scopedRefreshes=${JSON.stringify(scopedRefreshes)})`; return; }
    console.log('scoped refresh on the wire:', JSON.stringify(scopedRefreshes[0]));
    if (scopedRefreshes[0].connection_id !== 'aws') { failure = `refresh scoped to wrong connection: ${scopedRefreshes[0].connection_id}`; return; }
    if (scopedRefreshes[0].refresh !== true) { failure = `refresh flag not true: ${JSON.stringify(scopedRefreshes[0])}`; return; }

    // (3) The model count surfaces inline.
    await page.waitForFunction(
      () => {
        const s = document.querySelector('.conn-refresh-status');
        return s && s.textContent.trim() === '2 models available';
      },
      { timeout: 15000 },
    );
    const status = (await page.locator('.conn-refresh-status').innerText()).trim();
    console.log('inline refresh status:', JSON.stringify(status));
  } finally {
    await browser.close();
    server.close();
  }
  if (failure) { console.error(`FAIL: ${failure}`); process.exit(1); }
  console.log('PASS: per-connection Refresh models sends the scoped refresh and shows the model count.');
}
main();
