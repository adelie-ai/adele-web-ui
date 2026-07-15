// Headless end-to-end check for the BACKGROUND-TASKS panel (issue #50).
// Explicitly invoked — NOT part of `just check` (that stays browser-free).
// See README.md.
//
// Serves the REAL built SPA (`../../dist`, produced by `trunk build`) from a
// fake BFF that speaks the real WS protocol. The BFF answers
// `list_background_tasks` from a swappable snapshot (counting the reads) and —
// while the Tasks panel is open — pushes unsolicited `task_*` event frames, the
// same user-scoped broadcast the real relay does. It asserts, in a real browser,
// the whole relay→wire→reducer→engine→panel path in wasm:
//
//   1. Snapshot on open: opening Tasks reads `list_background_tasks` once and
//      shows the running task with its title + "Running".
//   2. Live progress: a pushed `task_progress` updates the row's hint in place,
//      no refetch, no manual poke.
//   3. Live start: a pushed `task_started` for a NEW task adds a second row at
//      the top, live.
//   4. Completion → authoritative refetch: a pushed `task_completed` (whose
//      reducer effect carries no status) makes the engine re-fetch the snapshot;
//      the finished task then shows its real terminal status ("Completed") and
//      stays visible as "recent" (it is NOT dropped).
//
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

// --- Wire helpers ------------------------------------------------------------
const reply = (id, result) => JSON.stringify({ result: { id, result } });

// `WsFrame::Event { event: Event }` nests the `event` key TWICE — the outer key
// is the variant tag, the inner is the struct's single `event` field (pinned by
// wire.rs's `event_frame_wire_shape_is_doubly_tagged_and_round_trips`). The
// innermost object is the externally-tagged Event (snake_case), from api-model's
// `event_variants_match_documented_snake_case` golden test.
const eventFrame = (variant, body) => JSON.stringify({ event: { event: { [variant]: body } } });

const NOW = Date.now();
// A TaskView: id is a bare string (newtype), kind is externally tagged, status
// is snake_case. Optional fields (ended_at/last_error/parent/children/
// progress_hint) may be omitted — serde defaults them on the SPA side.
const task = (id, title, kind, status, extra = {}) => ({
  id,
  kind,
  status,
  started_at: NOW,
  title,
  ...extra,
});

const AGENT = { standalone: { name: 'Researcher', conversation_id: 'c1' } };
const MAINT = { maintenance: { name: 'Dream cycle' } };

// The authoritative snapshot the BFF serves for `list_background_tasks`. Starts
// with one running agent; step 4 swaps in the post-completion snapshot.
let taskSnapshot = [task('t-agent', 'Researcher: pricing data', AGENT, 'running')];
let taskListCount = 0;

const RESULTS = (id) => ({
  list_available_models: reply(id, {
    models: [
      { connection_id: 'local', connection_label: 'local (test)', model: { id: 'llama3.2:latest', display_name: 'Llama 3.2', context_limit: 131072, capabilities: { reasoning: false, vision: false, tools: true, embedding: false } } },
    ],
  }),
  get_purposes: reply(id, { purposes: { interactive: { connection: 'local', model: 'llama3.2:latest' }, dreaming: { connection: 'local', model: 'llama3.2:latest' }, consolidation: { connection: 'local', model: 'llama3.2:latest' }, embedding: { connection: 'local', model: 'llama3.2:latest' } } }),
  list_conversations: reply(id, { conversations: [{ id: 'c1', title: 'Tasks Probe', message_count: 0, updated_at: '2026-07-14 00:00:00', archived: false }] }),
  get_conversation: reply(id, { conversation: { id: 'c1', title: 'Tasks Probe', messages: [] } }),
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
    if (key === 'list_background_tasks') { taskListCount += 1; sock.send(reply(o.id, { background_tasks: taskSnapshot })); return; }
    const out = RESULTS(o.id)[key];
    if (out) sock.send(out);
  });
});

async function openTasks(page) {
  await page.click('button[aria-label="Open settings"]');
  await page.locator('.sheet-tab', { hasText: 'Tasks' }).click();
  await page.waitForSelector('.tasks-panel', { timeout: 5000 });
}

// Run in the browser: the status text of the row whose title contains `title`.
const statusFor = (title) => {
  const row = [...document.querySelectorAll('.task-row')].find(
    (r) => r.querySelector('.task-title')?.textContent?.includes(title),
  );
  return row?.querySelector('.task-status')?.textContent ?? null;
};

async function main() {
  await new Promise((r) => server.listen(PORT, '127.0.0.1', r));
  const browser = await chromium.launch({ headless: true });
  let failure = null;
  try {
    const page = await (await browser.newContext()).newPage();
    // A wasm panic surfaces as a page error — treat it as a hard failure.
    page.on('pageerror', (e) => { failure = failure || `uncaught wasm error: ${e.message}`; });
    await page.goto(`http://127.0.0.1:${PORT}`, { waitUntil: 'domcontentloaded' });
    await page.fill('input[placeholder="Username"]', 'dave');
    await page.fill('input[type="password"]', 'testpass123');
    await page.click('button[type="submit"]');
    await page.waitForSelector('form.composer', { timeout: 15000 });
    await page.waitForSelector('span.dot.online', { timeout: 15000 });

    // --- (1) Snapshot on open ----------------------------------------------
    await openTasks(page);
    await page.waitForFunction(
      () => document.querySelectorAll('.task-row').length === 1
        && [...document.querySelectorAll('.task-title')].some((t) => t.textContent.includes('Researcher: pricing data')),
      { timeout: 15000 },
    ).catch(() => { failure = 'tasks panel did not show the running task on open'; });
    if (failure) return;
    if (taskListCount !== 1) { failure = `open should read the snapshot once (taskListCount=${taskListCount})`; return; }
    if (await page.evaluate(statusFor, 'Researcher: pricing data') !== 'Running') { failure = 'running task did not show "Running"'; return; }
    console.log(`opened: 1 running task (taskListCount=${taskListCount})`);

    // --- (2) Live progress (no refetch) ------------------------------------
    activeSock.send(eventFrame('task_progress', { id: 't-agent', progress_hint: 'searching page 2/4' }));
    await page.waitForFunction(
      () => [...document.querySelectorAll('.task-hint')].some((h) => h.textContent.includes('searching page 2/4')),
      { timeout: 15000 },
    ).catch(() => { failure = 'progress hint did not appear live after task_progress'; });
    if (failure) return;
    if (taskListCount !== 1) { failure = `task_progress must not refetch (taskListCount=${taskListCount})`; return; }
    console.log('live progress: hint updated in place, no refetch');

    // --- (3) Live start (new row at top) -----------------------------------
    activeSock.send(eventFrame('task_started', { task: task('t-dream', 'Dream: consolidate memory', MAINT, 'running') }));
    await page.waitForFunction(
      () => document.querySelectorAll('.task-row').length === 2
        && document.querySelectorAll('.task-row')[0].querySelector('.task-title')?.textContent?.includes('Dream: consolidate memory'),
      { timeout: 15000 },
    ).catch(() => { failure = 'a pushed task_started did not add a live row at the top'; });
    if (failure) return;
    console.log('live start: new task appeared at the top, live');

    // --- (4) Completion → authoritative refetch ----------------------------
    // Swap the snapshot so the refetch reflects t-agent as Completed (and keeps
    // both rows — "active + recent"). Then push task_completed (no status): the
    // engine must re-fetch and show the real terminal status.
    taskSnapshot = [
      task('t-dream', 'Dream: consolidate memory', MAINT, 'running'),
      task('t-agent', 'Researcher: pricing data', AGENT, 'completed', { ended_at: NOW + 4000 }),
    ];
    activeSock.send(eventFrame('task_completed', { id: 't-agent', status: 'completed' }));
    await page.waitForFunction(
      () => {
        const rows = [...document.querySelectorAll('.task-row')];
        const agent = rows.find((r) => r.querySelector('.task-title')?.textContent?.includes('Researcher: pricing data'));
        return rows.length === 2 && agent?.querySelector('.task-status')?.textContent === 'Completed';
      },
      { timeout: 15000 },
    ).catch(() => { failure = 'completed task did not reflect its terminal status after the refetch'; });
    if (failure) return;
    if (taskListCount !== 2) { failure = `completion should trigger exactly one refetch (taskListCount=${taskListCount})`; return; }
    // The header summary should now read the active/recent split.
    const summary = await page.locator('.tasks-panel .panel-summary').textContent();
    if (!summary.includes('active') || !summary.includes('recent')) {
      failure = `summary should report the active/recent split, got: ${JSON.stringify(summary)}`;
      return;
    }
    console.log(`completion: refetched once, terminal status shown, summary="${summary}" (taskListCount=${taskListCount})`);
  } catch (e) {
    failure = failure || `unexpected error: ${e && e.stack ? e.stack : e}`;
  } finally {
    // Report inside `finally` so an early `return` on a failed step still
    // surfaces the failure (a bare `return` would otherwise skip a post-block
    // check and exit 0).
    await browser.close();
    server.close();
    if (failure) { console.error(`FAIL: ${failure}`); process.exitCode = 1; } else {
      console.log('PASS: tasks panel surfaces active + recent tasks and updates live (progress / start / completion).');
    }
  }
}
main().catch((e) => { console.error(`ERROR: ${e && e.stack ? e.stack : e}`); process.exit(1); });
