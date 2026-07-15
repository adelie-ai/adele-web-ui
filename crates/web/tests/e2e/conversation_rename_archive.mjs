// Headless end-to-end check for conversation rename + archive (issue #49).
// Explicitly invoked — NOT part of `just check` (that stays browser-free). See
// README.md.
//
// It serves the REAL built SPA (`../../dist`, produced by `trunk build`) from a
// STATEFUL mock BFF that speaks the real WS protocol and keeps an in-memory
// conversation list with per-conversation `title` + `archived`, so the full
// rename/archive/unarchive flow can be driven in a real headless browser and
// asserted in the DOM:
//   1. rename the OPEN conversation → its sidebar row AND the header title
//      update (the header via a re-fetch, proving persistence);
//   2. archive another conversation → it leaves the default list;
//   3. expand "Archived" → the archived one is listed there;
//   4. unarchive it → it returns to the default list and leaves the section.
//
// A stateful mock (rather than the shared local daemon) keeps this deterministic
// and isolated — concurrent agents build against that daemon, and this test must
// never race them or touch data it didn't create. The pure decision logic
// (`src/conversation_manage.rs`) runs under `just check`.
import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { WebSocketServer } from 'ws';
import { chromium } from 'playwright';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const DIST = path.resolve(HERE, '../../dist');
const PORT = 9394;
const BEARER = 'adele.bearer';
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

if (!fs.existsSync(path.join(DIST, 'index.html'))) {
  console.error(`No built SPA at ${DIST}. Run \`trunk build\` in crates/web first.`);
  process.exit(2);
}

// --- In-memory conversation state (mutated by rename/archive/unarchive) -------
// Two distinct active conversations; the first is most-recent so the SPA opens
// it on connect. Unique markers so nothing here is confusable with real data.
let convs = [
  { id: 'ra-alpha', title: 'Alpha RENAMETEST', message_count: 2, updated_at: '2026-07-14 02:00:00', archived: false },
  { id: 'ra-beta', title: 'Beta RENAMETEST', message_count: 5, updated_at: '2026-07-14 01:00:00', archived: false },
];

const wrap = (id, result) => JSON.stringify({ result: { id, result } });
const errFrame = (id, error) => JSON.stringify({ error: { id, error } });

const MODELS = [
  { connection_id: 'local', connection_label: 'local (test)', model: { id: 'llama3.2:latest', display_name: 'Llama 3.2', context_limit: 131072, capabilities: { reasoning: false, vision: false, tools: true, embedding: false } } },
];
const PURPOSES = { interactive: { connection: 'local', model: 'llama3.2:latest' }, dreaming: { connection: 'local', model: 'llama3.2:latest' }, consolidation: { connection: 'local', model: 'llama3.2:latest' }, embedding: { connection: 'local', model: 'llama3.2:latest' } };

// Compute the reply for one request, mutating conversation state as needed.
function handle(reqId, key, args) {
  switch (key) {
    case 'list_available_models':
      return wrap(reqId, { models: MODELS });
    case 'get_purposes':
      return wrap(reqId, { purposes: PURPOSES });
    case 'list_conversations':
      // Honour include_archived exactly like the daemon: false = active only.
      return wrap(reqId, { conversations: args.include_archived ? convs : convs.filter((c) => !c.archived) });
    case 'get_conversation': {
      const c = convs.find((x) => x.id === args.id);
      if (!c) return errFrame(reqId, 'conversation not found');
      // Return the CURRENT stored title so a post-rename re-fetch reflects it.
      return wrap(reqId, { conversation: { id: c.id, title: c.title, messages: [] } });
    }
    case 'rename_conversation': {
      const c = convs.find((x) => x.id === args.id);
      if (!c) return errFrame(reqId, 'conversation not found');
      c.title = args.title;
      return wrap(reqId, 'ack');
    }
    case 'archive_conversation': {
      const c = convs.find((x) => x.id === args.id);
      if (!c) return errFrame(reqId, 'conversation not found');
      c.archived = true;
      return wrap(reqId, 'ack');
    }
    case 'unarchive_conversation': {
      const c = convs.find((x) => x.id === args.id);
      if (!c) return errFrame(reqId, 'conversation not found');
      c.archived = false;
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

async function activeTitles(page) {
  // Rows in the default list only (exclude the archived section).
  return page.locator('.conv-list .conv-row .conv-title').allInnerTexts();
}
async function archivedTitles(page) {
  return page.locator('.conv-archived-list .conv-title').allInnerTexts();
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
    check((await header(page)) === 'Alpha RENAMETEST', `header on load = ${JSON.stringify(await header(page))}`);

    // 2. Open the drawer: both active conversations listed.
    await page.click('button[aria-label="Open conversations"]');
    await page.waitForSelector('.sidebar-drawer');
    await sleep(400);
    const t1 = await activeTitles(page);
    check(t1.length === 2 && t1.includes('Alpha RENAMETEST') && t1.includes('Beta RENAMETEST'), `active list on open = ${JSON.stringify(t1)}`);

    // 3. Rename the OPEN conversation (Alpha) inline: the row AND the header
    //    (via a re-fetch that reads the persisted title) both update.
    const alpha = page.locator('.conv-list .conv-item', { hasText: 'Alpha RENAMETEST' });
    await alpha.locator('button[aria-label="Rename conversation"]').click();
    await page.waitForSelector('.conv-rename-input', { timeout: 5000 });
    await page.fill('.conv-rename-input', 'Alpha RENAMED');
    await page.locator('.conv-rename button[type="submit"]').click();
    await sleep(800);
    const t2 = await activeTitles(page);
    check(t2.includes('Alpha RENAMED') && !t2.includes('Alpha RENAMETEST'), `active list after rename = ${JSON.stringify(t2)}`);
    check((await header(page)) === 'Alpha RENAMED', `header after rename = ${JSON.stringify(await header(page))}`);

    // 4. Archive the other conversation (Beta): it leaves the default list.
    const beta = page.locator('.conv-list .conv-item', { hasText: 'Beta RENAMETEST' });
    await beta.locator('button[aria-label="Archive conversation"]').click();
    await sleep(800);
    const t3 = await activeTitles(page);
    check(t3.length === 1 && t3.includes('Alpha RENAMED') && !t3.includes('Beta RENAMETEST'), `active list after archive = ${JSON.stringify(t3)}`);

    // 5. Expand the Archived section: the archived conversation is listed there.
    await page.click('button[aria-label="Show archived conversations"]');
    await page.waitForSelector('.conv-archived-list', { timeout: 5000 });
    await sleep(600);
    const a1 = await archivedTitles(page);
    check(a1.length === 1 && a1.includes('Beta RENAMETEST'), `archived list = ${JSON.stringify(a1)}`);

    // 6. Unarchive it: it returns to the default list and leaves the section.
    await page.locator('.conv-archived-list button[aria-label="Unarchive conversation"]').click();
    await sleep(800);
    const t4 = await activeTitles(page);
    check(t4.length === 2 && t4.includes('Beta RENAMETEST') && t4.includes('Alpha RENAMED'), `active list after unarchive = ${JSON.stringify(t4)}`);
    const a2 = await archivedTitles(page);
    check(a2.length === 0, `archived list after unarchive = ${JSON.stringify(a2)}`);

    if (!failure && problems.length) failure = `assertions failed:\n  - ${problems.join('\n  - ')}`;
  } catch (e) {
    failure = `exception: ${e.message}`;
  } finally {
    await browser.close();
    server.close();
  }
  if (failure) { console.error(`FAIL: ${failure}`); process.exit(1); }
  console.log('PASS: switcher renames (row + header), archives, lists archived, and unarchives in a real browser.');
}
main();
