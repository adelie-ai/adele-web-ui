// Headless end-to-end check for the conversation scratchpad view (issue #16).
// Explicitly invoked — NOT part of `just check` (that stays browser-free). See
// README.md.
//
// Serves the REAL built SPA (`../../dist`, produced by `trunk build`) from a
// stateful fake BFF that speaks the real WS protocol. The reducer fetches the
// active conversation's scratchpad on load (and re-fetches after each completed
// turn), so the BFF answers `get_conversation_scratchpad` with a note set that
// CHANGES after a message is sent. This asserts, in the DOM, that the Scratchpad
// settings panel: (1) renders the conversation's notes grouped by type — a todo
// with an open checkbox and a plain note — with a "2 notes · 0 of 1 done"
// summary; then (2) after a turn, UPDATES in place to show the todo struck/done,
// a newly-added todo, and a "3 notes · 1 of 2 done" summary — proving the whole
// wire→reducer→engine→DOM refresh path in a real browser. Fails on any uncaught
// wasm panic.
import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { WebSocketServer } from 'ws';
import { chromium } from 'playwright';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const DIST = path.resolve(HERE, '../../dist');
const PORT = 9393;
const BEARER = 'adele.bearer';

if (!fs.existsSync(path.join(DIST, 'index.html'))) {
  console.error(`No built SPA at ${DIST}. Run \`trunk build\` in crates/web first.`);
  process.exit(2);
}

// --- Wire helpers ------------------------------------------------------------
const reply = (id, result) => JSON.stringify({ result: { id, result } });
const eventFrame = (variant, fields) =>
  JSON.stringify({ event: { event: { [variant]: fields } } });

// A scratchpad note: `CommandResult::Scratchpad(Vec<ScratchpadNoteView>)`
// serializes as `{ scratchpad: [ {id,key,content,note_type,sequence,done,
// updated_at}, ... ] }` (pinned by api-model's `scratchpad_result_...` test).
const note = (id, key, content, note_type, sequence, done) => ({
  id, key, content, note_type, sequence, done, updated_at: '2026-07-14 00:00:00',
});

// Before any turn: one open todo + one plain note (2 notes, 0 of 1 done).
const NOTES_BEFORE = [
  note('sp1', 't1', 'Draft the outline', 'todo', 1, false),
  note('sp2', 'n1', 'User prefers Rust', 'note', 1, false),
];
// After a turn: the todo is done, a second todo appears (3 notes, 1 of 2 done).
const NOTES_AFTER = [
  note('sp1', 't1', 'Draft the outline', 'todo', 1, true),
  note('sp3', 't2', 'Write the tests', 'todo', 2, false),
  note('sp2', 'n1', 'User prefers Rust', 'note', 1, false),
];

// Non-send RPC replies for the initial load (one chat-capable model, one convo).
const RESULTS = (id) => ({
  list_available_models: reply(id, {
    models: [
      { connection_id: 'local', connection_label: 'local (test)', model: { id: 'llama3.2:latest', display_name: 'Llama 3.2', context_limit: 131072, capabilities: { reasoning: false, vision: false, tools: true, embedding: false } } },
    ],
  }),
  get_purposes: reply(id, { purposes: { interactive: { connection: 'local', model: 'llama3.2:latest' }, dreaming: { connection: 'local', model: 'llama3.2:latest' }, consolidation: { connection: 'local', model: 'llama3.2:latest' }, embedding: { connection: 'local', model: 'llama3.2:latest' } } }),
  list_conversations: reply(id, { conversations: [{ id: 'c1', title: 'Pad Probe', message_count: 0, updated_at: '2026-07-14 00:00:00', archived: false }] }),
  get_conversation: reply(id, { conversation: { id: 'c1', title: 'Pad Probe', messages: [] } }),
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
  // A turn "changes" the scratchpad: before a send, the pad reads NOTES_BEFORE;
  // after, NOTES_AFTER. The reducer re-fetches on turn completion.
  let sent = false;
  sock.on('message', (data) => {
    const o = JSON.parse(data.toString());
    const key = typeof o.command === 'string' ? o.command : Object.keys(o.command)[0];
    if (key === 'get_conversation_scratchpad') {
      sock.send(reply(o.id, { scratchpad: sent ? NOTES_AFTER : NOTES_BEFORE }));
      return;
    }
    if (key === 'send_message') {
      sent = true;
      sock.send(reply(o.id, { send_message_ack: { request_id: 'r1', task_id: 'task-r1' } }));
      setTimeout(() => sock.send(eventFrame('assistant_delta', { conversation_id: 'c1', request_id: 'r1', chunk: 'done' })), 30);
      setTimeout(() => sock.send(eventFrame('assistant_completed', { conversation_id: 'c1', request_id: 'r1', full_response: 'done' })), 60);
      return;
    }
    const out = RESULTS(o.id)[key];
    if (out) sock.send(out);
  });
});

// Open Settings → Scratchpad tab.
async function openScratchpad(page) {
  await page.click('button[aria-label="Open settings"]');
  await page.locator('.sheet-tab', { hasText: 'Scratchpad' }).click();
  await page.waitForSelector('.scratchpad-panel', { timeout: 5000 });
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

    // --- Initial pad: two notes, the todo open -----------------------------
    await openScratchpad(page);
    // Wait for the notes fetched on conversation load to render.
    await page.waitForFunction(
      () => document.querySelectorAll('.scratchpad-note').length === 2,
      { timeout: 15000 },
    );
    const contents1 = await page.locator('.scratchpad-note .note-content').allTextContents();
    const summary1 = (await page.locator('.scratchpad-panel .panel-summary').innerText()).trim();
    // `textContent` (not `innerText`) so the group header's CSS uppercase
    // text-transform doesn't mangle the assertion — the DOM text is "Todo".
    const headers1 = await page.locator('.scratchpad-group .group-header').allTextContents();
    const doneCount1 = await page.locator('.scratchpad-note.done').count();
    console.log(`initial contents=${JSON.stringify(contents1)} summary=${JSON.stringify(summary1)} headers=${JSON.stringify(headers1)} done=${doneCount1}`);
    if (!contents1.some((t) => t.includes('Draft the outline'))) { failure = `initial pad missing the todo: ${JSON.stringify(contents1)}`; return; }
    if (!contents1.some((t) => t.includes('User prefers Rust'))) { failure = `initial pad missing the note: ${JSON.stringify(contents1)}`; return; }
    if (summary1 !== '2 notes \u{00b7} 0 of 1 done') { failure = `initial summary ${JSON.stringify(summary1)} != "2 notes · 0 of 1 done"`; return; }
    if (!headers1.map((h) => h.trim()).includes('Todo') || !headers1.map((h) => h.trim()).includes('Note')) { failure = `initial headers ${JSON.stringify(headers1)} missing Todo/Note`; return; }
    if (doneCount1 !== 0) { failure = `initial pad should have no done rows, found ${doneCount1}`; return; }

    // Close the sheet so the composer is reachable, then send a turn.
    await page.click('button[aria-label="Close settings"]');
    await page.fill('form.composer input', 'scratchpad probe #16');
    await page.waitForSelector('form.composer button:not([disabled])', { timeout: 5000 });
    await page.click('form.composer button');

    // --- After the turn: pad re-fetched, todo done, new todo added ---------
    await openScratchpad(page);
    await page.waitForFunction(
      () => document.querySelectorAll('.scratchpad-note').length === 3,
      { timeout: 15000 },
    );
    const contents2 = await page.locator('.scratchpad-note .note-content').allTextContents();
    const summary2 = (await page.locator('.scratchpad-panel .panel-summary').innerText()).trim();
    const doneCount2 = await page.locator('.scratchpad-note.done').count();
    console.log(`updated contents=${JSON.stringify(contents2)} summary=${JSON.stringify(summary2)} done=${doneCount2}`);
    if (!contents2.some((t) => t.includes('Write the tests'))) { failure = `updated pad missing the new todo: ${JSON.stringify(contents2)}`; return; }
    if (summary2 !== '3 notes \u{00b7} 1 of 2 done') { failure = `updated summary ${JSON.stringify(summary2)} != "3 notes · 1 of 2 done"`; return; }
    if (doneCount2 !== 1) { failure = `updated pad should have exactly one done row, found ${doneCount2}`; return; }
    // The done row is the one that was struck through.
    const struck = await page.locator('.scratchpad-note.done .note-content.struck').innerText();
    if (!struck.includes('Draft the outline')) { failure = `struck row ${JSON.stringify(struck)} != the completed todo`; return; }
  } finally {
    await browser.close();
    server.close();
  }
  if (failure) { console.error(`FAIL: ${failure}`); process.exit(1); }
  console.log('PASS: scratchpad panel renders grouped notes and refreshes after a turn.');
}
main();
