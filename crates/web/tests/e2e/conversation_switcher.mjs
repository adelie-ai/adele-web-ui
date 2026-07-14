// Headless end-to-end check for the conversation switcher (issue #12). Explicitly
// invoked — NOT part of `just check` (that stays browser-free). See README.md.
//
// It serves the REAL built SPA (`../../dist`, produced by `trunk build`) from a
// STATEFUL mock BFF that speaks the real WS protocol and keeps an in-memory
// conversation list, so the full switcher flow can be driven in a real headless
// browser and asserted in the DOM: the list loads with the open conversation
// marked, switching updates the header + marker, a new conversation is created
// and opened, and deleting one you made removes its row and re-homes the view.
//
// A stateful mock (rather than the shared local daemon) keeps this deterministic
// and isolated — two other agents build against that daemon concurrently, and
// this test must never race them or touch data it didn't create.
import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { WebSocketServer } from 'ws';
import { chromium } from 'playwright';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const DIST = path.resolve(HERE, '../../dist');
const PORT = 9391;
const BEARER = 'adele.bearer';
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

if (!fs.existsSync(path.join(DIST, 'index.html'))) {
  console.error(`No built SPA at ${DIST}. Run \`trunk build\` in crates/web first.`);
  process.exit(2);
}

// --- In-memory conversation state (mutated by create/delete) -----------------
// Seeded with two distinct titles; the first is the most-recent, so the SPA
// opens it on connect. Titles carry a unique marker so nothing here could ever
// be confused with real data.
let convs = [
  { id: 'sw-alpha', title: 'Alpha SWITCHERTEST', message_count: 2, updated_at: '2026-07-14 02:00:00', archived: false },
  { id: 'sw-beta', title: 'Beta SWITCHERTEST', message_count: 5, updated_at: '2026-07-14 01:00:00', archived: false },
];
let created = 0;

const wrap = (id, result) => JSON.stringify({ result: { id, result } });
const errFrame = (id, error) => JSON.stringify({ error: { id, error } });

const MODELS = [
  { connection_id: 'local', connection_label: 'local (test)', model: { id: 'llama3.2:latest', display_name: 'Llama 3.2', context_limit: 131072, capabilities: { reasoning: false, vision: false, tools: true, embedding: false } } },
];
const PURPOSES = { interactive: { connection: 'local', model: 'llama3.2:latest' }, dreaming: { connection: 'local', model: 'llama3.2:latest' }, consolidation: { connection: 'local', model: 'llama3.2:latest' }, embedding: { connection: 'local', model: 'llama3.2:latest' } };

// Compute the reply for one request, mutating conversation state as needed.
// Returns a serialized WsFrame string, or null to drop (never used here).
function handle(reqId, key, args) {
  switch (key) {
    case 'list_available_models':
      return wrap(reqId, { models: MODELS });
    case 'get_purposes':
      return wrap(reqId, { purposes: PURPOSES });
    case 'list_conversations':
      return wrap(reqId, { conversations: convs });
    case 'get_conversation': {
      const c = convs.find((x) => x.id === args.id);
      if (!c) return errFrame(reqId, 'conversation not found');
      return wrap(reqId, { conversation: { id: c.id, title: c.title, messages: [] } });
    }
    case 'create_conversation': {
      created += 1;
      const id = `sw-new-${created}`;
      // The SPA hard-codes the title "New Conversation" for a new chat; echo it.
      convs = [{ id, title: args.title, message_count: 0, updated_at: '2026-07-14 03:00:00', archived: false }, ...convs];
      return wrap(reqId, { conversation_id: { id } });
    }
    case 'delete_conversation': {
      convs = convs.filter((x) => x.id !== args.id);
      return wrap(reqId, 'ack');
    }
    case 'subscribe_conversations':
      return wrap(reqId, 'ack');
    default:
      // Anything unexpected in this flow: ack so the transport never hangs.
      return wrap(reqId, 'ack');
  }
}

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
    const args = typeof o.command === 'string' ? {} : o.command[key] || {};
    const out = handle(o.id, key, args);
    if (out) sock.send(out);
  });
});

// --- Assertions --------------------------------------------------------------
const problems = [];
const check = (cond, msg) => { if (!cond) problems.push(msg); };

async function titles(page) {
  return page.locator('.conv-row .conv-title').allInnerTexts();
}
async function activeTitle(page) {
  const n = await page.locator('.conv-item.active .conv-title').count();
  return n ? (await page.locator('.conv-item.active .conv-title').first().innerText()) : '(none)';
}
async function header(page) {
  return (await page.locator('.chat-header .title').innerText()).trim();
}

async function main() {
  await new Promise((r) => server.listen(PORT, '127.0.0.1', r));
  const browser = await chromium.launch({ headless: true });
  let failure = null;
  try {
    const page = await (await browser.newContext()).newPage();
    page.on('pageerror', (e) => { failure = `uncaught wasm error: ${e.message}`; });

    await page.goto(`http://127.0.0.1:${PORT}`, { waitUntil: 'domcontentloaded' });
    await page.fill('input[autocomplete="username"]', 'dave');
    await page.fill('input[autocomplete="current-password"]', 'testpass123');
    await page.click('button[type="submit"]');
    await page.waitForSelector('form.composer', { timeout: 15000 });
    await sleep(2000);

    // 1. On connect the SPA opens the most-recent conversation (Alpha).
    check((await header(page)) === 'Alpha SWITCHERTEST', `header on load = ${JSON.stringify(await header(page))}`);

    // 2. Drawer lists both conversations with the open one marked.
    await page.click('button[aria-label="Open conversations"]');
    await page.waitForSelector('.sidebar-drawer');
    await sleep(400);
    const t1 = await titles(page);
    check(t1.length === 2 && t1.includes('Alpha SWITCHERTEST') && t1.includes('Beta SWITCHERTEST'), `list on open = ${JSON.stringify(t1)}`);
    check((await activeTitle(page)) === 'Alpha SWITCHERTEST', `active on open = ${JSON.stringify(await activeTitle(page))}`);

    // 3. Switch to Beta: the row loads it, the drawer closes, the header updates.
    await page.locator('.conv-row', { hasText: 'Beta SWITCHERTEST' }).click();
    await page.waitForSelector('.sidebar-drawer', { state: 'detached', timeout: 5000 });
    await sleep(600);
    check((await header(page)) === 'Beta SWITCHERTEST', `header after switch = ${JSON.stringify(await header(page))}`);
    await page.click('button[aria-label="Open conversations"]');
    await sleep(400);
    check((await activeTitle(page)) === 'Beta SWITCHERTEST', `active after switch = ${JSON.stringify(await activeTitle(page))}`);

    // 4. New conversation: created, opened, and the header shows it.
    await page.click('button.conv-new');
    await page.waitForSelector('.sidebar-drawer', { state: 'detached', timeout: 5000 });
    await sleep(700);
    check((await header(page)) === 'New Conversation', `header after new = ${JSON.stringify(await header(page))}`);
    await page.click('button[aria-label="Open conversations"]');
    await sleep(400);
    const t2 = await titles(page);
    check(t2.length === 3 && t2.includes('New Conversation'), `list after new = ${JSON.stringify(t2)}`);
    check((await activeTitle(page)) === 'New Conversation', `active after new = ${JSON.stringify(await activeTitle(page))}`);

    // 5. Delete the one we created (with the inline confirm); it disappears and
    //    the view falls back to a remaining conversation.
    const newItem = page.locator('.conv-item', { hasText: 'New Conversation' });
    await newItem.locator('button[aria-label="Delete conversation"]').click();
    await page.waitForSelector('.conv-confirm', { timeout: 5000 });
    await page.locator('.conv-confirm-actions button.danger').click();
    await sleep(800);
    const t3 = await titles(page);
    check(t3.length === 2 && !t3.includes('New Conversation'), `list after delete = ${JSON.stringify(t3)}`);
    const h = await header(page);
    check(h === 'Alpha SWITCHERTEST' || h === 'Beta SWITCHERTEST', `header after delete = ${JSON.stringify(h)}`);

    if (!failure && problems.length) failure = `assertions failed:\n  - ${problems.join('\n  - ')}`;
  } catch (e) {
    failure = `exception: ${e.message}`;
  } finally {
    await browser.close();
    server.close();
  }
  if (failure) { console.error(`FAIL: ${failure}`); process.exit(1); }
  console.log('PASS: switcher lists, switches, creates, and deletes in a real browser.');
}
main();
