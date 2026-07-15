// Headless end-to-end check for LIVE knowledge-base refresh (issue #39).
// Explicitly invoked — NOT part of `just check` (that stays browser-free).
// See README.md.
//
// Builds on the browse/search panel (#19). Serves the REAL built SPA
// (`../../dist`, produced by `trunk build`) from a fake BFF that speaks the real
// WS protocol. The BFF answers `list_knowledge_entries` from a swappable data
// set and COUNTS how many such reads it serves, then — while the Knowledge panel
// is open — pushes an unsolicited `KnowledgeChanged` event frame. It asserts, in
// a real browser, that:
//
//   1. Live refresh WHILE OPEN: after the panel is open (browsing V1's 2
//      entries), a pushed `KnowledgeChanged` makes the panel re-fetch on its own
//      — the list swaps to V2 (a new entry appears, 2 → 3 rows) with NO manual
//      Refresh/Clear click.
//   2. No-op WHILE CLOSED: with the panel closed, a pushed `KnowledgeChanged`
//      triggers NO `list_knowledge_entries` read (the closed panel has no live
//      effect watching). Re-opening then reads once (load-on-open) and shows the
//      latest data.
//
// This exercises the whole relay→wire→engine→panel live path in wasm. Fails on
// any uncaught wasm panic.
import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { WebSocketServer } from 'ws';
import { chromium } from 'playwright';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const DIST = path.resolve(HERE, '../../dist');
const PORT = 9396;
const BEARER = 'adele.bearer';

if (!fs.existsSync(path.join(DIST, 'index.html'))) {
  console.error(`No built SPA at ${DIST}. Run \`trunk build\` in crates/web first.`);
  process.exit(2);
}

// --- Wire helpers ------------------------------------------------------------
const reply = (id, result) => JSON.stringify({ result: { id, result } });

// `WsFrame::Event { event: Event }` nests twice; `KnowledgeChanged` is a UNIT
// variant, so the inner value is the bare snake_case string (pinned by wire.rs's
// `event_frame_wire_shape_is_doubly_tagged_and_round_trips` golden test).
const KNOWLEDGE_CHANGED_FRAME = JSON.stringify({ event: { event: 'knowledge_changed' } });

const entry = (id, content, tags, created_at, updated_at) => ({
  id, content, tags, metadata: {}, created_at, updated_at,
});

// Two KB snapshots. V2 adds a distinctive NEW entry, so a successful live
// re-fetch is visible as a 2 → 3 row change with a recognisable snippet.
const V1 = [
  entry('kb1', 'User prefers Rust over Go for backend services.', ['preferences'], '2026-07-10 09:00:00', '2026-07-10 09:00:00'),
  entry('kb2', 'Adele stores long-term memory in Postgres.', ['infra'], '2026-07-09 09:00:00', '2026-07-09 09:00:00'),
];
const V2 = [
  entry('kb3', 'LIVEFACT: the dream cycle just consolidated a new fact.', ['dreaming'], '2026-07-15 03:00:00', '2026-07-15 03:00:00'),
  ...V1,
];

// Mutable server state: which snapshot to serve, and how many browse reads
// (`list_knowledge_entries`) we've answered — the no-op-when-closed assertion
// watches this counter.
let kbData = V1;
let kbListCount = 0;

const RESULTS = (id) => ({
  list_available_models: reply(id, {
    models: [
      { connection_id: 'local', connection_label: 'local (test)', model: { id: 'llama3.2:latest', display_name: 'Llama 3.2', context_limit: 131072, capabilities: { reasoning: false, vision: false, tools: true, embedding: false } } },
    ],
  }),
  get_purposes: reply(id, { purposes: { interactive: { connection: 'local', model: 'llama3.2:latest' }, dreaming: { connection: 'local', model: 'llama3.2:latest' }, consolidation: { connection: 'local', model: 'llama3.2:latest' }, embedding: { connection: 'local', model: 'llama3.2:latest' } } }),
  list_conversations: reply(id, { conversations: [{ id: 'c1', title: 'KB Live Probe', message_count: 0, updated_at: '2026-07-14 00:00:00', archived: false }] }),
  get_conversation: reply(id, { conversation: { id: 'c1', title: 'KB Live Probe', messages: [] } }),
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

let activeSock = null;
const wss = new WebSocketServer({ server, path: '/ws', handleProtocols: (p) => (p.has(BEARER) ? BEARER : false) });
wss.on('connection', (sock) => {
  activeSock = sock;
  sock.on('message', (data) => {
    const o = JSON.parse(data.toString());
    const key = typeof o.command === 'string' ? o.command : Object.keys(o.command)[0];
    if (key === 'list_knowledge_entries') { kbListCount += 1; sock.send(reply(o.id, { knowledge_entries: kbData })); return; }
    if (key === 'search_knowledge_entries') { sock.send(reply(o.id, { knowledge_entries: kbData })); return; }
    const out = RESULTS(o.id)[key];
    if (out) sock.send(out);
  });
});

async function openKnowledge(page) {
  await page.click('button[aria-label="Open settings"]');
  await page.locator('.sheet-tab', { hasText: 'Knowledge' }).click();
  await page.waitForSelector('.knowledge-panel', { timeout: 5000 });
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function main() {
  await new Promise((r) => server.listen(PORT, '127.0.0.1', r));
  const browser = await chromium.launch({ headless: true });
  let failure = null;
  try {
    const page = await (await browser.newContext()).newPage();
    page.on('pageerror', (e) => { failure = `uncaught wasm error: ${e.message}`; });
    await page.goto(`http://127.0.0.1:${PORT}`, { waitUntil: 'domcontentloaded' });
    await page.fill('input[placeholder="Username"]', 'dave');
    await page.fill('input[type="password"]', 'testpass123');
    await page.click('button[type="submit"]');
    await page.waitForSelector('form.composer', { timeout: 15000 });
    await page.waitForSelector('span.dot.online', { timeout: 15000 });

    // --- Open: browse V1 (2 entries) ---------------------------------------
    await openKnowledge(page);
    await page.waitForFunction(() => document.querySelectorAll('.kb-entry').length === 2, { timeout: 15000 });
    if (await page.locator('.kb-snippet', { hasText: 'LIVEFACT' }).count() !== 0) { failure = 'V2 entry present before the change'; return; }
    console.log(`opened: 2 entries (kbListCount=${kbListCount})`);

    // --- (1) Live refresh WHILE OPEN ---------------------------------------
    // Flip the data set, then push a KnowledgeChanged. The open panel must
    // re-fetch on its own and swap in V2 (the new LIVEFACT row) — no clicks.
    kbData = V2;
    activeSock.send(KNOWLEDGE_CHANGED_FRAME);
    await page.waitForFunction(
      () => document.querySelectorAll('.kb-entry').length === 3
        && [...document.querySelectorAll('.kb-snippet')].some((s) => s.textContent.includes('LIVEFACT')),
      { timeout: 15000 },
    ).catch(() => { failure = 'panel did not live-refresh to V2 after KnowledgeChanged'; });
    if (failure) return;
    console.log(`live-refreshed while open: 3 entries incl. LIVEFACT (kbListCount=${kbListCount})`);

    // --- (2) No-op WHILE CLOSED --------------------------------------------
    // Close the sheet, push another change, and assert NO browse read fires
    // (the closed panel has no live effect). Give the wasm loop time to run.
    await page.click('button[aria-label="Close settings"]');
    await page.waitForSelector('.knowledge-panel', { state: 'detached', timeout: 5000 });
    const countBeforeClosedPush = kbListCount;
    activeSock.send(KNOWLEDGE_CHANGED_FRAME);
    await sleep(750);
    if (kbListCount !== countBeforeClosedPush) {
      failure = `closed panel re-fetched on KnowledgeChanged (kbListCount ${countBeforeClosedPush} -> ${kbListCount})`;
      return;
    }
    console.log(`no-op while closed: kbListCount unchanged at ${kbListCount}`);

    // Re-opening reads once (load-on-open) and shows the latest data.
    await openKnowledge(page);
    await page.waitForFunction(() => document.querySelectorAll('.kb-entry').length === 3, { timeout: 15000 });
    if (kbListCount !== countBeforeClosedPush + 1) {
      failure = `re-open should read exactly once (kbListCount ${countBeforeClosedPush} -> ${kbListCount})`;
      return;
    }
    console.log(`re-open read once: 3 entries (kbListCount=${kbListCount})`);
  } finally {
    await browser.close();
    server.close();
  }
  if (failure) { console.error(`FAIL: ${failure}`); process.exit(1); }
  console.log('PASS: KB panel live-refreshes on KnowledgeChanged when open, and stays quiet when closed.');
}
main();
