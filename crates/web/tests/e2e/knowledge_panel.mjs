// Headless end-to-end check for the knowledge-base browse/search panel (issue
// #19). Explicitly invoked — NOT part of `just check` (that stays browser-free).
// See README.md.
//
// Serves the REAL built SPA (`../../dist`, produced by `trunk build`) from a fake
// BFF that speaks the real WS protocol and answers the client-facing KB commands
// the panel issues: `list_knowledge_entries` (browse, on panel open) and
// `search_knowledge_entries` (on search submit), each with
// `CommandResult::KnowledgeEntries` — `{ knowledge_entries: [...] }`. It asserts,
// in a real browser, that the Knowledge settings panel: (1) browses the most
// recent entries on open — rows with a collapsed single-line snippet (multi-line
// content collapsed + truncated with an ellipsis), tag chips, and an Added/Updated
// meta line — with a "2 entries" summary; (2) opens an entry in place to reveal
// its full content; (3) runs a server-side search that swaps in the hits with a
// "1 result" summary + a Clear affordance; and (4) clears back to browse. This
// exercises the whole wire→engine→DOM path (and the pure snippet/summary/meta
// helpers) in wasm. Fails on any uncaught wasm panic.
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

if (!fs.existsSync(path.join(DIST, 'index.html'))) {
  console.error(`No built SPA at ${DIST}. Run \`trunk build\` in crates/web first.`);
  process.exit(2);
}

// --- Wire helpers ------------------------------------------------------------
const reply = (id, result) => JSON.stringify({ result: { id, result } });

// A KB entry: `CommandResult::KnowledgeEntries(Vec<KnowledgeEntryView>)`
// serializes as `{ knowledge_entries: [ {id,content,tags,metadata,created_at,
// updated_at}, ... ] }` (CommandResult + KnowledgeEntryView are snake_case).
const entry = (id, content, tags, created_at, updated_at) => ({
  id, content, tags, metadata: {}, created_at, updated_at,
});

// Browse: two entries, newest first. The second has multi-line content well over
// the 140-char snippet cap (to prove collapse + truncation) and a later update
// date (to prove the "Added … · Updated …" meta line).
const LONG = `This is a deliberately long knowledge entry.\nIt spans multiple lines and has    irregular   whitespace, so the collapsed snippet must join it into a single line and then truncate it once it runs past the preview budget of one hundred and forty characters.`;
const BROWSE = [
  entry('kb1', 'User prefers Rust over Go for backend services.', ['preferences', 'languages'], '2026-07-10 09:00:00', '2026-07-10 09:00:00'),
  entry('kb2', LONG, ['projects'], '2026-07-01 12:00:00', '2026-07-12 08:00:00'),
];
// Search hits for the query: a single, distinct entry.
const SEARCH = [
  entry('kb1', 'Rust is the user’s preferred backend language for its type safety.', ['languages'], '2026-07-10 09:00:00', '2026-07-10 09:00:00'),
];

// Non-KB replies for the initial load (one chat-capable model, one convo). The
// reducer also fetches the conversation scratchpad on load — answer it empty so
// that path doesn't time out and toast over our panel.
const RESULTS = (id) => ({
  list_available_models: reply(id, {
    models: [
      { connection_id: 'local', connection_label: 'local (test)', model: { id: 'llama3.2:latest', display_name: 'Llama 3.2', context_limit: 131072, capabilities: { reasoning: false, vision: false, tools: true, embedding: false } } },
    ],
  }),
  get_purposes: reply(id, { purposes: { interactive: { connection: 'local', model: 'llama3.2:latest' }, dreaming: { connection: 'local', model: 'llama3.2:latest' }, consolidation: { connection: 'local', model: 'llama3.2:latest' }, embedding: { connection: 'local', model: 'llama3.2:latest' } } }),
  list_conversations: reply(id, { conversations: [{ id: 'c1', title: 'KB Probe', message_count: 0, updated_at: '2026-07-14 00:00:00', archived: false }] }),
  get_conversation: reply(id, { conversation: { id: 'c1', title: 'KB Probe', messages: [] } }),
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
    if (key === 'list_knowledge_entries') { sock.send(reply(o.id, { knowledge_entries: BROWSE })); return; }
    if (key === 'search_knowledge_entries') { sock.send(reply(o.id, { knowledge_entries: SEARCH })); return; }
    const out = RESULTS(o.id)[key];
    if (out) sock.send(out);
  });
});

// Open Settings → Knowledge tab.
async function openKnowledge(page) {
  await page.click('button[aria-label="Open settings"]');
  await page.locator('.sheet-tab', { hasText: 'Knowledge' }).click();
  await page.waitForSelector('.knowledge-panel', { timeout: 5000 });
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

    // --- Browse: two entries render on open --------------------------------
    await openKnowledge(page);
    await page.waitForFunction(
      () => document.querySelectorAll('.kb-entry').length === 2,
      { timeout: 15000 },
    );
    const summary1 = (await page.locator('.knowledge-panel .panel-summary').innerText()).trim();
    // `.field-label` is CSS-uppercased; `innerText` reflects the transform, so
    // compare against the DOM `textContent` (the untransformed "Recent"/"Results").
    const label1 = (await page.locator('.knowledge-panel .field-label').textContent()).trim();
    const snippets = await page.locator('.kb-snippet').allTextContents();
    const tags = await page.locator('.kb-tag').allTextContents();
    const metas = await page.locator('.kb-meta').allTextContents();
    console.log(`browse summary=${JSON.stringify(summary1)} label=${JSON.stringify(label1)} tags=${JSON.stringify(tags)} metas=${JSON.stringify(metas)}`);
    if (summary1 !== '2 entries') { failure = `browse summary ${JSON.stringify(summary1)} != "2 entries"`; return; }
    if (label1 !== 'Recent') { failure = `browse label ${JSON.stringify(label1)} != "Recent"`; return; }
    if (!snippets.some((s) => s.includes('User prefers Rust'))) { failure = `browse missing the first snippet: ${JSON.stringify(snippets)}`; return; }
    // The long, multi-line entry must be collapsed to one line and ellipsized.
    const long = snippets.find((s) => s.startsWith('This is a deliberately long'));
    if (!long) { failure = `browse missing the long entry snippet: ${JSON.stringify(snippets)}`; return; }
    if (long.includes('\n')) { failure = `long snippet was not collapsed to one line: ${JSON.stringify(long)}`; return; }
    if (!long.endsWith('…')) { failure = `long snippet was not truncated with an ellipsis: ${JSON.stringify(long)}`; return; }
    if (!tags.includes('preferences') || !tags.includes('languages') || !tags.includes('projects')) { failure = `browse tag chips missing: ${JSON.stringify(tags)}`; return; }
    if (!metas.some((m) => m.includes('Added 2026-07-10'))) { failure = `browse meta missing Added date: ${JSON.stringify(metas)}`; return; }
    if (!metas.some((m) => m.includes('Updated 2026-07-12'))) { failure = `browse meta missing Updated date: ${JSON.stringify(metas)}`; return; }

    // --- Open an entry: full content reveals in place ----------------------
    if (await page.locator('.kb-full-content').count() !== 0) { failure = 'entry detail should be collapsed before opening'; return; }
    await page.locator('.kb-entry-head').first().click();
    await page.waitForSelector('.kb-full-content', { timeout: 5000 });
    const detail = (await page.locator('.kb-full-content').first().innerText()).trim();
    if (!detail.includes('User prefers Rust over Go for backend services.')) { failure = `opened entry detail wrong: ${JSON.stringify(detail)}`; return; }

    // --- Search: swap in the hits, summary + Clear affordance --------------
    await page.fill('.kb-search input', 'rust');
    await page.click('.kb-search-btn');
    await page.waitForFunction(
      () => {
        const s = document.querySelector('.knowledge-panel .panel-summary');
        return s && s.textContent.trim() === '1 result'
          && document.querySelectorAll('.kb-entry').length === 1;
      },
      { timeout: 15000 },
    );
    const label2 = (await page.locator('.knowledge-panel .field-label').textContent()).trim();
    if (label2 !== 'Results') { failure = `search label ${JSON.stringify(label2)} != "Results"`; return; }
    const clear = page.locator('.knowledge-panel .field-head .link', { hasText: 'Clear search' });
    if (await clear.count() !== 1) { failure = 'Clear-search affordance not shown during a search'; return; }
    const hit = (await page.locator('.kb-snippet').first().innerText()).trim();
    if (!hit.includes('preferred backend language')) { failure = `search hit snippet wrong: ${JSON.stringify(hit)}`; return; }

    // --- Clear: back to the browse list ------------------------------------
    await clear.click();
    await page.waitForFunction(
      () => {
        const s = document.querySelector('.knowledge-panel .panel-summary');
        return s && s.textContent.trim() === '2 entries'
          && document.querySelectorAll('.kb-entry').length === 2;
      },
      { timeout: 15000 },
    );
    const label3 = (await page.locator('.knowledge-panel .field-label').textContent()).trim();
    if (label3 !== 'Recent') { failure = `after-clear label ${JSON.stringify(label3)} != "Recent"`; return; }
    console.log('cleared back to browse: 2 entries, label Recent');
  } finally {
    await browser.close();
    server.close();
  }
  if (failure) { console.error(`FAIL: ${failure}`); process.exit(1); }
  console.log('PASS: knowledge panel browses, opens an entry, searches, and clears.');
}
main();
