// Headless end-to-end regression for the transport-reliability fix (model picker
// empty / Refresh dead). Explicitly invoked — NOT part of `just check` (that
// stays browser-free). See README.md.
//
// It serves the REAL built SPA (`../../dist`, produced by `trunk build`) from a
// minimal fake BFF that speaks the real WS protocol but delivers every reply as
// a BINARY WebSocket frame — exactly what a proxy/ingress can do to a text
// payload. Before the fix the read pump matched only `Message::Text` and
// silently dropped `Message::Bytes`, so `list_available_models` never resolved,
// the sequential initial load stalled, the connection never came online, and the
// picker stayed empty with Refresh unable to recover. This asserts the opposite:
// the connection comes online and the picker lists the chat-capable model.
import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { WebSocketServer } from 'ws';
import { chromium } from 'playwright';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const DIST = path.resolve(HERE, '../../dist');
const PORT = 9390;
const BEARER = 'adele.bearer';
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

if (!fs.existsSync(path.join(DIST, 'index.html'))) {
  console.error(`No built SPA at ${DIST}. Run \`trunk build\` in crates/web first.`);
  process.exit(2);
}

// Minimal valid protocol replies (real shapes; two models, one chat-capable).
const reply = (id, result) => JSON.stringify({ result: { id, result } });
const RESULTS = (id) => ({
  list_available_models: reply(id, {
    models: [
      { connection_id: 'local', connection_label: 'local (test)', model: { id: 'llama3.2:latest', display_name: 'Llama 3.2', context_limit: 131072, capabilities: { reasoning: false, vision: false, tools: true, embedding: false } } },
      { connection_id: 'local', connection_label: 'local (test)', model: { id: 'nomic-embed', display_name: 'Nomic', capabilities: { reasoning: false, vision: false, tools: false, embedding: true } } },
    ],
  }),
  get_purposes: reply(id, { purposes: { interactive: { connection: 'local', model: 'llama3.2:latest' }, dreaming: { connection: 'local', model: 'llama3.2:latest' }, consolidation: { connection: 'local', model: 'llama3.2:latest' }, embedding: { connection: 'local', model: 'nomic-embed' } } }),
  list_conversations: reply(id, { conversations: [{ id: 'c1', title: 'Test', message_count: 0, updated_at: '2026-07-14 00:00:00', archived: false }] }),
  get_conversation: reply(id, { conversation: { id: 'c1', title: 'Test', messages: [] } }),
  subscribe_conversations: reply(id, 'ack'),
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
    const out = RESULTS(o.id)[key];
    if (out) sock.send(Buffer.from(out, 'utf8'), { binary: true }); // deliver as BINARY
  });
});

async function main() {
  await new Promise((r) => server.listen(PORT, '127.0.0.1', r));
  const browser = await chromium.launch({ headless: true });
  let failure = null;
  try {
    const page = await (await browser.newContext()).newPage();
    page.on('pageerror', (e) => { failure = `uncaught wasm error: ${e.message}`; });
    await page.goto(`http://127.0.0.1:${PORT}`, { waitUntil: 'domcontentloaded' });
    await page.fill('input[autocomplete="username"]', 'adele');
    await page.fill('input[autocomplete="current-password"]', 'testpass123');
    await page.click('button[type="submit"]');
    await page.waitForSelector('form.composer', { timeout: 15000 });
    await sleep(2500);

    const online = await page.locator('span.dot.online').count();
    await page.click('button[aria-label="Open settings"]');
    await page.waitForSelector('.settings-sheet');
    await sleep(500);
    const rows = await page.locator('button.model-row').count();
    const names = await page.locator('button.model-row .model-name').allInnerTexts();
    console.log(`online=${online} pickerRows=${rows} names=${JSON.stringify(names)}`);
    if (!online) failure = 'connection never went online (transport stalled)';
    else if (rows !== 1) failure = `expected 1 chat-capable model row, got ${rows}`;
    else if (names[0] !== 'Llama 3.2') failure = `unexpected model name ${JSON.stringify(names)}`;
  } finally {
    await browser.close();
    server.close();
  }
  if (failure) { console.error(`FAIL: ${failure}`); process.exit(1); }
  console.log('PASS: binary-framed replies load the picker and the connection is online.');
}
main();
