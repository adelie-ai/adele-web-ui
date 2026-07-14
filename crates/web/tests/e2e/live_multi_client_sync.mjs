// Headless end-to-end check for live multi-client sync (issue #15). Explicitly
// invoked — NOT part of `just check` (that stays browser-free). See README.md.
//
// It serves the REAL built SPA (`../../dist`, produced by `trunk build`) from a
// STATEFUL fake BFF that speaks the real WS protocol and, crucially, PUSHES
// server-initiated `WsFrame::Event` frames to simulate activity by OTHER clients
// (gtk/tui/kde/voice) — turns the browser did NOT initiate, plus conversation
// list/title changes. It asserts, in the DOM and with NO manual refresh, that:
//
//   1. On connect the SPA subscribes the open conversation (a
//      `subscribe_conversations` command carrying its id is observed).
//   2. An external turn on the open conversation renders live: the pushed
//      `user_message_added` draws the user bubble, `assistant_delta` streams,
//      and `assistant_completed` finalises the reply — all wire→reducer→signals.
//   3. The switcher sidebar updates live while OPEN: a pushed
//      `conversation_title_changed` renames a row in place, and a
//      `conversation_list_changed` (with the fake BFF's list now holding a new
//      conversation) makes a new row appear — the reducer's refetch path.
//   4. After a simulated socket drop the SPA reconnects and RE-subscribes (a
//      fresh `subscribe_conversations` for the open conversation is observed),
//      and a live event pushed AFTER the reconnect still renders — proving the
//      live path survives a phone sleeping / changing networks.
//
// A stateful fake BFF (rather than the shared local daemon) keeps this
// deterministic and isolated — two other agents build against that daemon
// concurrently, and this test must never race them or touch data it didn't
// create. Everything here carries a LIVESYNC marker for the same reason.
//
// NOTE ON PRODUCTION SCOPE: this exercises the CLIENT (crates/web) handling of
// pushed live events. The real BFF's `ForwardingHandler` currently relays only a
// browser-initiated send-turn's own events; relaying the daemon's fanned-out
// cross-client events to the browser is a separate crates/server follow-up (see
// the PR's scope findings). The client is correct the moment those frames arrive
// — which is exactly what this fake BFF proves.
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

// --- Wire helpers ------------------------------------------------------------
const reply = (id, result) => JSON.stringify({ result: { id, result } });
// `WsFrame::Event { event: Event }` nests twice: outer key = the frame's variant
// tag, inner `event` = its single field, then the externally-tagged snake_case
// `Event` variant. (Pinned by wire.rs's `event_frame_wire_shape` golden test.)
const eventFrame = (variant, fields) =>
  JSON.stringify({ event: { event: { [variant]: fields } } });

const MODELS = [
  { connection_id: 'local', connection_label: 'local (test)', model: { id: 'llama3.2:latest', display_name: 'Llama 3.2', context_limit: 131072, capabilities: { reasoning: false, vision: false, tools: true, embedding: false } } },
];
const PURPOSES = { interactive: { connection: 'local', model: 'llama3.2:latest' }, dreaming: { connection: 'local', model: 'llama3.2:latest' }, consolidation: { connection: 'local', model: 'llama3.2:latest' }, embedding: { connection: 'local', model: 'llama3.2:latest' } };

// --- Stateful conversation list (mutated to simulate other-client changes) ---
// Seeded with two distinct titles; the first is the most-recent, so the SPA
// opens it on connect. The LIVESYNC marker keeps this unmistakable for real data.
let convs = [
  { id: 'ls-main', title: 'Main LIVESYNCTEST', message_count: 3, updated_at: '2026-07-14 02:00:00', archived: false },
  { id: 'ls-side', title: 'Side LIVESYNCTEST', message_count: 1, updated_at: '2026-07-14 01:00:00', archived: false },
];

// Every `subscribe_conversations` command the client sends, in order. Length
// grows on connect and again on reconnect — the re-subscribe evidence.
const subscribeLog = [];
let connCount = 0;
let activeSock = null;

function handle(reqId, key, args) {
  switch (key) {
    case 'list_available_models':
      return reply(reqId, { models: MODELS });
    case 'get_purposes':
      return reply(reqId, { purposes: PURPOSES });
    case 'list_conversations':
      return reply(reqId, { conversations: convs });
    case 'get_conversation': {
      const c = convs.find((x) => x.id === args.id);
      if (!c) return JSON.stringify({ error: { id: reqId, error: 'conversation not found' } });
      // Empty transcript: the fake BFF does not persist turns, so a reload after
      // a live render starts clean — live events are asserted BEFORE any reload.
      return reply(reqId, { conversation: { id: c.id, title: c.title, messages: [] } });
    }
    case 'subscribe_conversations':
      subscribeLog.push(args.conversation_ids || []);
      return reply(reqId, 'ack');
    default:
      // Anything else in this flow: ack so the transport never hangs.
      return reply(reqId, 'ack');
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
  connCount += 1;
  activeSock = sock;
  sock.on('message', (data) => {
    const o = JSON.parse(data.toString());
    const key = typeof o.command === 'string' ? o.command : Object.keys(o.command)[0];
    const args = typeof o.command === 'string' ? {} : o.command[key] || {};
    const out = handle(o.id, key, args);
    if (out) sock.send(out);
  });
});

// Push a server-initiated event to the browser (simulating another client).
const push = (variant, fields) => activeSock.send(eventFrame(variant, fields));

// --- Assertions --------------------------------------------------------------
const problems = [];
const check = (cond, msg) => { if (!cond) problems.push(msg); };

// Poll `fn()` until truthy or timeout (for out-of-band conditions like the
// subscribe log growing — Playwright's waiters only observe the DOM).
async function until(fn, timeout, label) {
  const t0 = Date.now();
  while (Date.now() - t0 < timeout) {
    if (await fn()) return true;
    await sleep(50);
  }
  problems.push(`timeout waiting for: ${label}`);
  return false;
}

async function header(page) {
  return (await page.locator('.chat-header .title').innerText()).trim();
}
async function rowTitles(page) {
  return page.locator('.conv-row .conv-title').allInnerTexts();
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

    // 1. On connect the SPA opens the most-recent conversation and subscribes it.
    check((await header(page)) === 'Main LIVESYNCTEST', `header on load = ${JSON.stringify(await header(page))}`);
    await until(() => subscribeLog.some((ids) => ids.includes('ls-main')), 10000, 'subscribe-on-connect for ls-main');

    // 2. External turn on the OPEN conversation renders live (message list).
    //    A turn this client did NOT initiate: a distinct request_id, pushed
    //    unprompted. No composer send, no refresh.
    push('user_message_added', { conversation_id: 'ls-main', request_id: 'ext-1', content: 'ping from another client LIVESYNC' });
    await page.waitForFunction(
      (want) => [...document.querySelectorAll('.msg.user p')].some((p) => p.textContent.includes(want)),
      'ping from another client LIVESYNC',
      { timeout: 10000 },
    ).catch(() => problems.push('external user bubble did not appear live'));

    push('assistant_delta', { conversation_id: 'ls-main', request_id: 'ext-1', chunk: 'streaming ' });
    await page.waitForFunction(
      () => { const el = document.querySelector('.msg.assistant.streaming p'); return el && el.textContent.includes('streaming'); },
      undefined,
      { timeout: 10000 },
    ).catch(() => problems.push('external streaming chunk did not render live'));

    push('assistant_completed', { conversation_id: 'ls-main', request_id: 'ext-1', full_response: 'streamed reply LIVESYNC' });
    await page.waitForFunction(
      (want) => [...document.querySelectorAll('.msg.assistant:not(.streaming) p')].some((p) => p.textContent.includes(want)),
      'streamed reply LIVESYNC',
      { timeout: 10000 },
    ).catch(() => problems.push('external assistant reply did not finalise live'));

    // 3. Sidebar updates live while the drawer is OPEN (no re-open in between).
    await page.click('button[aria-label="Open conversations"]');
    await page.waitForSelector('.sidebar-drawer', { timeout: 5000 });
    await sleep(500); // let the load-on-open refetch settle (2 rows, original titles)
    const t0 = await rowTitles(page);
    check(t0.length === 2 && t0.includes('Side LIVESYNCTEST'), `rows on open = ${JSON.stringify(t0)}`);

    // 3a. A live rename of an existing row updates it IN PLACE (reducer patches
    //     its conversation list directly). The drawer is not re-opened, so the
    //     only path to the new title is the pushed event.
    convs = convs.map((c) => (c.id === 'ls-side' ? { ...c, title: 'Side RENAMED LIVESYNC' } : c));
    push('conversation_title_changed', { conversation_id: 'ls-side', title: 'Side RENAMED LIVESYNC' });
    await page.waitForFunction(
      (want) => [...document.querySelectorAll('.conv-row .conv-title')].some((el) => el.textContent === want),
      'Side RENAMED LIVESYNC',
      { timeout: 10000 },
    ).catch(() => problems.push('live rename did not update the sidebar row'));

    // 3b. A live list change makes a NEW conversation appear (reducer refetches;
    //     the fake BFF's list now holds it). Still no drawer re-open.
    convs = [{ id: 'ls-new', title: 'New LIVESYNC', message_count: 0, updated_at: '2026-07-14 03:00:00', archived: false }, ...convs];
    push('conversation_list_changed', { conversation_id: 'ls-new' });
    await page.waitForFunction(
      (want) => [...document.querySelectorAll('.conv-row .conv-title')].some((el) => el.textContent === want),
      'New LIVESYNC',
      { timeout: 10000 },
    ).catch(() => problems.push('live list change did not add the new sidebar row'));
    const t1 = await rowTitles(page);
    check(t1.length === 3, `rows after live add = ${JSON.stringify(t1)}`);

    // Close the drawer before the reconnect leg.
    await page.click('button[aria-label="Close conversations"]');
    await page.waitForSelector('.sidebar-drawer', { state: 'detached', timeout: 5000 });

    // 4. Simulated socket drop → reconnect → RE-subscribe. Phones sleep / change
    //    networks; the daemon subscription is per-connection, so it must be
    //    re-sent on the new socket.
    const subsBefore = subscribeLog.length;
    const connsBefore = connCount;
    activeSock.close();
    await until(() => connCount > connsBefore, 10000, 'socket reconnect');
    await until(
      () => subscribeLog.length > subsBefore && subscribeLog[subscribeLog.length - 1].includes('ls-main'),
      10000,
      're-subscribe for ls-main after reconnect',
    );
    await page.waitForSelector('span.dot.online', { timeout: 15000 });

    // 4a. A live event pushed AFTER the reconnect still renders — the live path
    //     survived the drop (the reload started the transcript clean).
    await sleep(300);
    push('user_message_added', { conversation_id: 'ls-main', request_id: 'ext-2', content: 'post-reconnect ping LIVESYNC' });
    await page.waitForFunction(
      (want) => [...document.querySelectorAll('.msg.user p')].some((p) => p.textContent.includes(want)),
      'post-reconnect ping LIVESYNC',
      { timeout: 10000 },
    ).catch(() => problems.push('live event after reconnect did not render'));

    if (!failure && problems.length) failure = `assertions failed:\n  - ${problems.join('\n  - ')}`;
  } catch (e) {
    failure = `exception: ${e.message}`;
  } finally {
    await browser.close();
    server.close();
  }
  if (failure) { console.error(`FAIL: ${failure}`); process.exit(1); }
  console.log('PASS: live multi-client sync — external turns render, the sidebar updates live, and the client re-subscribes after a reconnect.');
}
main();
