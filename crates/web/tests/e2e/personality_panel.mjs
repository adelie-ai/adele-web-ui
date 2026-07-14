// Headless end-to-end check for the per-conversation personality panel (#13).
// Explicitly invoked — NOT part of `just check` (that stays browser-free).
//
// It serves the REAL built SPA (`../../dist`, produced by `trunk build`) from a
// minimal but STATEFUL fake BFF that speaks the real WS protocol. The fake
// persists the last `set_conversation_personality` per conversation and returns
// it from `get_conversation` (as `conversation_personality`), so this drives the
// genuine client round-trip in a real browser: open the panel, pin two traits,
// Save (→ SetConversationPersonality), then prove it persists across a full page
// reload (fresh wasm + fresh ViewSignals → GetConversation → the panel pre-fills
// from the stored override). The stateful fake keeps this deterministic and
// isolated from the shared local daemon (which two other agents are using).
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
const CONV = 'c1';
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

if (!fs.existsSync(path.join(DIST, 'index.html'))) {
  console.error(`No built SPA at ${DIST}. Run \`trunk build\` in crates/web first.`);
  process.exit(2);
}

// Server-side state: the stored personality override, keyed by conversation id.
// `null` = no override (inherits global). This is what a real daemon persists.
const stored = { [CONV]: null };

const reply = (id, result) => JSON.stringify({ result: { id, result } });
const conversationResult = (id) => {
  const conv = { id: CONV, title: 'Test', messages: [] };
  if (stored[CONV]) conv.conversation_personality = stored[CONV];
  return reply(id, { conversation: conv });
};
const RESULTS = (id, cmd, body) => {
  switch (cmd) {
    case 'list_available_models':
      return reply(id, {
        models: [
          { connection_id: 'local', connection_label: 'local (test)', model: { id: 'llama3.2:latest', display_name: 'Llama 3.2', context_limit: 131072, capabilities: { reasoning: false, vision: false, tools: true, embedding: false } } },
        ],
      });
    case 'get_purposes':
      return reply(id, { purposes: { interactive: { connection: 'local', model: 'llama3.2:latest' } } });
    case 'list_conversations':
      return reply(id, { conversations: [{ id: CONV, title: 'Test', message_count: 0, updated_at: '2026-07-14 00:00:00', archived: false }] });
    case 'get_conversation':
      return conversationResult(id);
    case 'set_conversation_personality': {
      // Persist the partial override (empty object = cleared → null), then echo
      // it back as CommandResult::ConversationPersonality — exactly the daemon's
      // contract the client's `set_personality` reads.
      const p = body && body.personality ? body.personality : {};
      stored[body.conversation_id] = Object.keys(p).length ? p : null;
      return reply(id, { conversation_personality: p });
    }
    default:
      return reply(id, 'ack'); // subscribe_conversations, etc.
  }
};

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
    const cmd = typeof o.command === 'string' ? o.command : Object.keys(o.command)[0];
    const body = typeof o.command === 'string' ? undefined : o.command[cmd];
    sock.send(RESULTS(o.id, cmd, body));
  });
});

// Open the settings sheet and navigate to the Personality tab.
async function openPersonality(page) {
  await page.click('button[aria-label="Open settings"]');
  await page.waitForSelector('.settings-sheet');
  await page.click('button.sheet-tab:has-text("Personality")');
  await page.waitForSelector('.personality-card', { timeout: 10000 });
}

async function selectValues(page) {
  return {
    professionalism: await page.locator('select[aria-label="Professionalism"]').inputValue(),
    warmth: await page.locator('select[aria-label="Warmth"]').inputValue(),
    directness: await page.locator('select[aria-label="Directness"]').inputValue(),
    humor: await page.locator('select[aria-label="Humor"]').inputValue(),
  };
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
    await sleep(1500);

    // 1. Open the panel; every trait starts on Global (empty value), no override.
    await openPersonality(page);
    const rows = await page.locator('.personality-row select').count();
    const before = await selectValues(page);
    const summaryBefore = (await page.locator('.personality-panel .panel-summary').innerText()).trim();
    if (rows !== 7) failure = `expected 7 trait selects, got ${rows}`;
    else if (before.humor !== '' || before.directness !== '') failure = `traits should start on Global, got ${JSON.stringify(before)}`;
    else if (!/No overrides/i.test(summaryBefore)) failure = `expected a no-overrides summary, got ${JSON.stringify(summaryBefore)}`;

    // 2. Pin humor=Never and directness=Always, then Save.
    if (!failure) {
      await page.selectOption('select[aria-label="Humor"]', 'never');
      await page.selectOption('select[aria-label="Directness"]', 'always');
      await page.waitForSelector('.personality-card .save-purpose', { timeout: 5000 });
      await page.click('.personality-card .save-purpose');
      await sleep(800);
      // After the daemon echo re-seeds the form the Save button (dirty → clean)
      // disappears and the summary reflects two pinned traits.
      const saveGone = await page.locator('.personality-card .save-purpose').count();
      const summaryAfter = (await page.locator('.personality-panel .panel-summary').innerText()).trim();
      if (saveGone !== 0) failure = 'Save button should disappear after a successful save';
      else if (!/2 of 7/.test(summaryAfter)) failure = `expected "2 of 7" summary after save, got ${JSON.stringify(summaryAfter)}`;
    }

    // 3. Prove persistence: full page reload (fresh wasm + fresh signals). The
    //    panel must pre-fill from the stored override via GetConversation.
    if (!failure) {
      await page.reload({ waitUntil: 'domcontentloaded' });
      await page.waitForSelector('form.composer', { timeout: 15000 });
      await sleep(1500);
      await openPersonality(page);
      await sleep(400);
      const after = await selectValues(page);
      const summaryReload = (await page.locator('.personality-panel .panel-summary').innerText()).trim();
      console.log(`reload selects=${JSON.stringify(after)} summary=${JSON.stringify(summaryReload)} storedServer=${JSON.stringify(stored[CONV])}`);
      if (after.humor !== 'never') failure = `humor did not persist across reload: ${JSON.stringify(after)}`;
      else if (after.directness !== 'always') failure = `directness did not persist across reload: ${JSON.stringify(after)}`;
      else if (after.professionalism !== '' || after.warmth !== '') failure = `unpinned traits should still inherit (Global): ${JSON.stringify(after)}`;
      else if (!/2 of 7/.test(summaryReload)) failure = `expected "2 of 7" after reload, got ${JSON.stringify(summaryReload)}`;
    }
  } finally {
    await browser.close();
    server.close();
  }
  if (failure) { console.error(`FAIL: ${failure}`); process.exit(1); }
  console.log('PASS: personality overrides save via SetConversationPersonality and persist across a reload (GetConversation).');
}
main();
