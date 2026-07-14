// Headless end-to-end check for the global personality panel (#17).
// Explicitly invoked — NOT part of `just check` (that stays browser-free).
//
// It serves the REAL built SPA (`../../dist`, produced by `trunk build`) from a
// minimal but STATEFUL fake BFF that speaks the real WS protocol. The fake holds
// a single global `Config` and mutates its `personality` block on `set_config`
// (applying the `ConfigChanges`), returning the current config from both
// `get_config` and `set_config` as `CommandResult::Config`. This drives the
// genuine client round-trip in a real browser: open Settings → Global
// Personality, confirm the seven traits pre-fill from the daemon's config
// (Expressive-7 defaults, every trait a concrete level — no "Global (inherit)"
// sentinel), change two, Save (→ SetConfig), then prove the change persists
// across a full page reload (fresh wasm + fresh ViewSignals → GetConfig re-seeds
// the panel). The stateful fake keeps this deterministic and isolated from the
// shared local daemon (which two other agents are using).
import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { WebSocketServer } from 'ws';
import { chromium } from 'playwright';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const DIST = path.resolve(HERE, '../../dist');
const PORT = 9392;
const BEARER = 'adele.bearer';
const CONV = 'c1';
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

if (!fs.existsSync(path.join(DIST, 'index.html'))) {
  console.error(`No built SPA at ${DIST}. Run \`trunk build\` in crates/web first.`);
  process.exit(2);
}

// The seven traits, in the wire order, and their `personality_<trait>` change
// keys. Server-side state: the global personality, seeded to the daemon's
// Expressive-7 defaults — this is what a real daemon persists in its config.
const TRAITS = ['professionalism', 'warmth', 'directness', 'enthusiasm', 'humor', 'sarcasm', 'pretentiousness'];
const personality = {
  professionalism: 'always',
  warmth: 'often',
  directness: 'often',
  enthusiasm: 'sometimes',
  humor: 'sometimes',
  sarcasm: 'rarely',
  pretentiousness: 'rarely',
};

const reply = (id, result) => JSON.stringify({ result: { id, result } });
// A complete `Config` view: embeddings + persistence are required (no serde
// defaults), so the client can only deserialize `CommandResult::Config` if they
// are present — mirror the daemon's real shape.
const configResult = (id) =>
  reply(id, {
    config: {
      embeddings: { connector: '', model: '', base_url: '', has_api_key: false, available: false, is_default: false },
      persistence: { enabled: false, remote_url: '', remote_name: '', push_on_update: false },
      personality: { ...personality },
    },
  });

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
      return reply(id, { conversation: { id: CONV, title: 'Test', messages: [] } });
    case 'get_config':
      return configResult(id);
    case 'set_config': {
      // Apply the ConfigChanges' personality_<trait> fields to the stored
      // config, then echo it back as CommandResult::Config — exactly the
      // daemon's contract the client's `save_global_personality` reads.
      const ch = (body && body.changes) || {};
      for (const t of TRAITS) {
        const v = ch[`personality_${t}`];
        if (v != null) personality[t] = v;
      }
      return configResult(id);
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

// Open the settings sheet and navigate to the Global Personality tab. The tab
// text "Global Personality" is unique (the per-conversation tab is just
// "Personality"), so :has-text targets it unambiguously.
async function openGlobalPersonality(page) {
  await page.click('button[aria-label="Open settings"]');
  await page.waitForSelector('.settings-sheet');
  await page.click('button.sheet-tab:has-text("Global Personality")');
  await page.waitForSelector('.global-personality-card', { timeout: 10000 });
}

const SEL = (t) => `.global-personality-card select[aria-label="${t[0].toUpperCase()}${t.slice(1)}"]`;
async function selectValues(page) {
  const out = {};
  for (const t of TRAITS) out[t] = await page.locator(SEL(t)).inputValue();
  return out;
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

    // 1. Open the panel; the seven traits pre-fill from GetConfig, every one a
    //    concrete level (no empty "Global (inherit)" value), and each <select>
    //    offers exactly the five levels.
    await openGlobalPersonality(page);
    const rows = await page.locator('.global-personality-card select').count();
    const before = await selectValues(page);
    const humorOptions = await page.locator(`${SEL('humor')} option`).count();
    const humorEmptyOption = await page.locator(`${SEL('humor')} option[value=""]`).count();
    if (rows !== 7) failure = `expected 7 trait selects, got ${rows}`;
    else if (humorOptions !== 5) failure = `expected 5 level options per trait, got ${humorOptions}`;
    else if (humorEmptyOption !== 0) failure = 'global traits must not offer a "Global (inherit)" option';
    else if (Object.values(before).some((v) => v === '')) failure = `every global trait must pre-fill a concrete level, got ${JSON.stringify(before)}`;
    else if (before.professionalism !== 'always' || before.humor !== 'sometimes') failure = `traits should seed from the config defaults, got ${JSON.stringify(before)}`;

    // 2. Change professionalism=Never and humor=Always, then Save (→ SetConfig).
    if (!failure) {
      await page.selectOption(SEL('professionalism'), 'never');
      await page.selectOption(SEL('humor'), 'always');
      await page.waitForSelector('.global-personality-card .save-purpose', { timeout: 5000 });
      await page.click('.global-personality-card .save-purpose');
      await sleep(800);
      // After the daemon echo re-seeds the form the Save button (dirty → clean)
      // disappears.
      const saveGone = await page.locator('.global-personality-card .save-purpose').count();
      if (saveGone !== 0) failure = 'Save button should disappear after a successful save';
    }

    // 3. Prove persistence: full page reload (fresh wasm + fresh signals). The
    //    panel must re-fill from the stored config via GetConfig.
    if (!failure) {
      await page.reload({ waitUntil: 'domcontentloaded' });
      await page.waitForSelector('form.composer', { timeout: 15000 });
      await sleep(1500);
      await openGlobalPersonality(page);
      await sleep(400);
      const after = await selectValues(page);
      console.log(`reload selects=${JSON.stringify(after)} storedServer=${JSON.stringify(personality)}`);
      if (after.professionalism !== 'never') failure = `professionalism did not persist across reload: ${JSON.stringify(after)}`;
      else if (after.humor !== 'always') failure = `humor did not persist across reload: ${JSON.stringify(after)}`;
      else if (after.warmth !== 'often' || after.sarcasm !== 'rarely') failure = `untouched traits should be unchanged: ${JSON.stringify(after)}`;
      // The server actually stored the two edits (not just a client echo).
      else if (personality.professionalism !== 'never' || personality.humor !== 'always') failure = `server did not persist the edits: ${JSON.stringify(personality)}`;
    }
  } finally {
    await browser.close();
    server.close();
  }
  if (failure) { console.error(`FAIL: ${failure}`); process.exit(1); }
  console.log('PASS: global personality saves via SetConfig and persists across a reload (GetConfig).');
}
main();
