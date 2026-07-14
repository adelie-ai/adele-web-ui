// Headless end-to-end check for the context-window usage indicator (issue #14).
// Explicitly invoked — NOT part of `just check` (that stays browser-free). See
// README.md.
//
// It serves the REAL built SPA (`../../dist`, produced by `trunk build`) from a
// minimal fake BFF that speaks the real WS protocol. After the user sends a
// message the BFF acks it and streams the turn's events back — crucially a
// `context_usage` event (the daemon emits one per turn, DA#341) — as real,
// correctly-nested `WsFrame::Event` frames. This asserts the indicator, hidden
// before any turn, (1) appears with the shared `used / budget (pct%)` readout
// and the green colour bucket after the first turn, then (2) UPDATES to the
// amber bucket + new readout after a second, heavier turn crosses the 0.85
// compaction line — i.e. the whole wire→reducer→engine→DOM path works in a real
// browser, not just in the host unit tests.
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

// --- Wire helpers ------------------------------------------------------------
const reply = (id, result) => JSON.stringify({ result: { id, result } });
// `WsFrame::Event { event: Event }` nests twice: outer key = the frame's
// variant tag, inner `event` = its single field, then the externally-tagged
// `Event` variant. (Pinned by wire.rs's `event_frame_wire_shape` golden test.)
const eventFrame = (variant, fields) =>
  JSON.stringify({ event: { event: { [variant]: fields } } });

// Non-send RPC replies for the initial load (one chat-capable model, one convo).
const RESULTS = (id) => ({
  list_available_models: reply(id, {
    models: [
      { connection_id: 'local', connection_label: 'local (test)', model: { id: 'llama3.2:latest', display_name: 'Llama 3.2', context_limit: 131072, capabilities: { reasoning: false, vision: false, tools: true, embedding: false } } },
    ],
  }),
  get_purposes: reply(id, { purposes: { interactive: { connection: 'local', model: 'llama3.2:latest' }, dreaming: { connection: 'local', model: 'llama3.2:latest' }, consolidation: { connection: 'local', model: 'llama3.2:latest' }, embedding: { connection: 'local', model: 'llama3.2:latest' } } }),
  list_conversations: reply(id, { conversations: [{ id: 'c1', title: 'Ctx Probe', message_count: 0, updated_at: '2026-07-14 00:00:00', archived: false }] }),
  get_conversation: reply(id, { conversation: { id: 'c1', title: 'Ctx Probe', messages: [] } }),
  subscribe_conversations: reply(id, 'ack'),
});

// Per-turn context_usage figures: turn 1 stays green (38%), turn 2 crosses the
// 0.85 line into amber (91%). request_id per turn so the SPA's pending stream
// claims each turn's events cleanly.
const TURNS = [
  { rid: 'r1', used: 12000, budget: 32000, readout: '12k / 32k (38%)', level: 'context-fill-green' },
  { rid: 'r2', used: 29000, budget: 32000, readout: '29k / 32k (91%)', level: 'context-fill-amber' },
];

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
  let turn = 0;
  sock.on('message', (data) => {
    const o = JSON.parse(data.toString());
    const key = typeof o.command === 'string' ? o.command : Object.keys(o.command)[0];
    if (key === 'send_message') {
      const t = TURNS[Math.min(turn, TURNS.length - 1)];
      turn += 1;
      // Ack the send, then stream the turn: a reply chunk, completion, and the
      // per-turn context_usage the indicator is built to surface.
      sock.send(reply(o.id, { send_message_ack: { request_id: t.rid, task_id: `task-${t.rid}` } }));
      setTimeout(() => sock.send(eventFrame('assistant_delta', { conversation_id: 'c1', request_id: t.rid, chunk: 'ok' })), 40);
      setTimeout(() => sock.send(eventFrame('assistant_completed', { conversation_id: 'c1', request_id: t.rid, full_response: 'ok' })), 80);
      setTimeout(() => sock.send(eventFrame('context_usage', { conversation_id: 'c1', request_id: t.rid, used_tokens: t.used, budget_tokens: t.budget, compaction_active: false })), 120);
      return;
    }
    const out = RESULTS(o.id)[key];
    if (out) sock.send(out);
  });
});

async function sendTurn(page, text) {
  await page.fill('form.composer input', text);
  await page.waitForSelector('form.composer button:not([disabled])', { timeout: 5000 });
  await page.click('form.composer button');
}

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

    // The indicator is hidden until a turn reports usage (a fresh conversation
    // load clears any stale reading, #341).
    const before = await page.locator('.context-usage').count();
    if (before !== 0) { failure = `indicator should be hidden before any turn, found ${before}`; return; }

    // --- Turn 1: appears, green -------------------------------------------
    await sendTurn(page, 'ctx probe A #14');
    await page.waitForSelector('.context-usage', { timeout: 15000 });
    const readout1 = (await page.locator('.context-usage .context-readout').innerText()).trim();
    const class1 = await page.locator('.context-usage').getAttribute('class');
    const aria1 = await page.locator('.context-usage').getAttribute('aria-label');
    const fillW1 = await page.locator('.context-usage .context-bar-fill').evaluate((el) => el.style.width);
    console.log(`turn1 readout=${JSON.stringify(readout1)} class=${JSON.stringify(class1)} aria=${JSON.stringify(aria1)} fill=${fillW1}`);
    if (readout1 !== TURNS[0].readout) { failure = `turn1 readout ${JSON.stringify(readout1)} != ${JSON.stringify(TURNS[0].readout)}`; return; }
    if (!class1.includes(TURNS[0].level)) { failure = `turn1 class ${JSON.stringify(class1)} missing ${TURNS[0].level}`; return; }
    if (aria1 !== 'Context window 38% full, 12000 of 32000 tokens') { failure = `turn1 aria unexpected: ${JSON.stringify(aria1)}`; return; }
    if (fillW1 !== '38%') { failure = `turn1 bar fill ${fillW1} != 38%`; return; }

    // --- Turn 2: updates in place, amber ----------------------------------
    await sendTurn(page, 'ctx probe B #14');
    await page.waitForFunction(
      (want) => {
        const el = document.querySelector('.context-usage .context-readout');
        return el && el.textContent.trim() === want;
      },
      TURNS[1].readout,
      { timeout: 15000 },
    );
    const class2 = await page.locator('.context-usage').getAttribute('class');
    const fillW2 = await page.locator('.context-usage .context-bar-fill').evaluate((el) => el.style.width);
    console.log(`turn2 readout=${JSON.stringify(TURNS[1].readout)} class=${JSON.stringify(class2)} fill=${fillW2}`);
    if (!class2.includes(TURNS[1].level)) { failure = `turn2 class ${JSON.stringify(class2)} missing ${TURNS[1].level}`; return; }
    if (class2.includes('context-fill-green')) { failure = `turn2 still shows green: ${JSON.stringify(class2)}`; return; }
    if (fillW2 !== '91%') { failure = `turn2 bar fill ${fillW2} != 91%`; return; }

    // Exactly one indicator (updated in place, not duplicated).
    const count = await page.locator('.context-usage').count();
    if (count !== 1) { failure = `expected exactly one indicator, found ${count}`; return; }
  } finally {
    await browser.close();
    server.close();
  }
  if (failure) { console.error(`FAIL: ${failure}`); process.exit(1); }
  console.log('PASS: context-usage indicator appears (green), updates per turn, and crosses to amber.');
}
main();
