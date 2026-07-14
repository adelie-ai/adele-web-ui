// Headless end-to-end check for browser read-aloud (issue #18). Explicitly
// invoked — NOT part of `just check` (that stays browser-free). See README.md.
//
// It serves the REAL built SPA (`../../dist`, produced by `trunk build`) from a
// minimal fake BFF that speaks the real WS protocol. The browser's Web Speech
// API (`window.speechSynthesis`) is STUBBED via `page.addInitScript` before the
// SPA loads, recording every `.speak(utterance.text)` and `.cancel()` on
// `window.__ra` — a browser API can't be observed any other way. It asserts, in
// a real headless Chromium:
//
//   1. With `speechSynthesis` present the read-aloud toggle is shown, and while
//      it is ON a COMPLETED assistant reply (fake BFF acks the send, then streams
//      assistant_delta + assistant_completed) is spoken with the reply's text.
//   2. Toggling read-aloud OFF mid-reply calls `speechSynthesis.cancel()`, and a
//      reply that completes while OFF is NOT spoken (the toggle genuinely gates
//      output, not just the mic).
//   3. With `speechSynthesis` ABSENT (stubbed undefined) the toggle is hidden and
//      the app runs without error — capability detection degrades gracefully.
//
// A fake BFF (not the shared local daemon) keeps this deterministic and isolated
// — concurrent agents build against that daemon and this must never race them.
// Every reply carries a RAOK marker for the same reason.
//
// The pure decision core (enable/dedup/blank/cancel) is unit-tested under `just
// check` in `src/read_aloud.rs`; this covers only the browser SpeechSynthesis +
// reactive-DOM layer those host tests can't reach.
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
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

if (!fs.existsSync(path.join(DIST, 'index.html'))) {
  console.error(`No built SPA at ${DIST}. Run \`trunk build\` in crates/web first.`);
  process.exit(2);
}

// --- Wire helpers ------------------------------------------------------------
const reply = (id, result) => JSON.stringify({ result: { id, result } });
// `WsFrame::Event { event: Event }` nests twice (pinned by wire.rs's
// `event_frame_wire_shape` golden test).
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
  list_conversations: reply(id, { conversations: [{ id: 'c1', title: 'Read Aloud RAOK', message_count: 0, updated_at: '2026-07-14 00:00:00', archived: false }] }),
  get_conversation: reply(id, { conversation: { id: 'c1', title: 'Read Aloud RAOK', messages: [] } }),
  subscribe_conversations: reply(id, 'ack'),
});

// Per-turn scripted replies. Turn 1 completes fast (the "speak it" case); turn 2
// delays completion so the test can toggle OFF mid-reply before it lands.
const TURNS = [
  { rid: 'ra1', text: 'First reply RAOK one', completeDelay: 60 },
  { rid: 'ra2', text: 'Second reply RAOK two', completeDelay: 900 },
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
      sock.send(reply(o.id, { send_message_ack: { request_id: t.rid, task_id: `task-${t.rid}` } }));
      setTimeout(() => sock.send(eventFrame('assistant_delta', { conversation_id: 'c1', request_id: t.rid, chunk: t.text })), 40);
      setTimeout(() => sock.send(eventFrame('assistant_completed', { conversation_id: 'c1', request_id: t.rid, full_response: t.text })), t.completeDelay);
      return;
    }
    const out = RESULTS(o.id)[key];
    if (out) sock.send(out);
  });
});

// --- Browser-API stubs (installed before the SPA loads) ----------------------
// Present: record speak()/cancel() on window.__ra so we can assert them.
const STUB_PRESENT = () => {
  window.__ra = { spoken: [], cancelled: 0 };
  const synth = {
    speak: (u) => { window.__ra.spoken.push(u && typeof u.text === 'string' ? u.text : String(u)); },
    cancel: () => { window.__ra.cancelled += 1; },
    pause() {}, resume() {}, getVoices: () => [],
    speaking: false, pending: false, paused: false,
    addEventListener() {}, removeEventListener() {},
  };
  // `speechSynthesis` is a prototype accessor; defineProperty is the reliable way
  // to override it (plain assignment silently no-ops against a getter-only prop).
  Object.defineProperty(window, 'speechSynthesis', { value: synth, configurable: true });
};
// Absent: the API is simply not there — the capability-detection path.
const STUB_ABSENT = () => {
  Object.defineProperty(window, 'speechSynthesis', { value: undefined, configurable: true });
};

// --- Assertions --------------------------------------------------------------
const problems = [];
const check = (cond, msg) => { if (!cond) problems.push(msg); };

async function until(fn, timeout, label) {
  const t0 = Date.now();
  while (Date.now() - t0 < timeout) {
    if (await fn()) return true;
    await sleep(50);
  }
  problems.push(`timeout waiting for: ${label}`);
  return false;
}

async function login(page) {
  await page.goto(`http://127.0.0.1:${PORT}`, { waitUntil: 'domcontentloaded' });
  await page.fill('input[placeholder="Username"]', 'dave');
  await page.fill('input[type="password"]', 'testpass123');
  await page.click('button[type="submit"]');
  await page.waitForSelector('form.composer', { timeout: 15000 });
  await page.waitForSelector('span.dot.online', { timeout: 15000 });
}

async function sendTurn(page, text) {
  await page.fill('form.composer input', text);
  await page.waitForSelector('form.composer button:not([disabled])', { timeout: 5000 });
  await page.click('form.composer button');
}

const TOGGLE = 'button[aria-label="Read replies aloud"]';

async function main() {
  await new Promise((r) => server.listen(PORT, '127.0.0.1', r));
  const browser = await chromium.launch({ headless: true });
  let failure = null;
  try {
    // === Part 1 & 2: speechSynthesis PRESENT ================================
    const ctx = await browser.newContext();
    await ctx.addInitScript(STUB_PRESENT);
    const page = await ctx.newPage();
    page.on('pageerror', (e) => { failure = `uncaught wasm error: ${e.message}`; });
    await login(page);

    // The toggle is shown (API present) and starts OFF.
    await page.waitForSelector(TOGGLE, { timeout: 10000 });
    check((await page.getAttribute(TOGGLE, 'aria-pressed')) === 'false', 'toggle should start OFF (aria-pressed=false)');

    // Turn OFF (default) speaks nothing: send a warm-up-free check by turning ON first.
    await page.click(TOGGLE);
    check((await page.getAttribute(TOGGLE, 'aria-pressed')) === 'true', 'toggle did not switch ON');

    // 1. Completed reply is spoken with the reply's text while ON.
    await sendTurn(page, 'read this aloud RAOK');
    await until(
      () => page.evaluate((t) => window.__ra.spoken.includes(t), TURNS[0].text),
      10000,
      'first reply spoken via speechSynthesis.speak()',
    );
    const spokenAfter1 = await page.evaluate(() => window.__ra.spoken.slice());
    const cancelledAfter1 = await page.evaluate(() => window.__ra.cancelled);
    check(spokenAfter1.length === 1, `exactly one reply spoken so far, got ${JSON.stringify(spokenAfter1)}`);
    check(cancelledAfter1 === 0, `no cancel before toggling off, got ${cancelledAfter1}`);

    // 2. Toggle OFF mid-reply cancels, and a reply completing while OFF is silent.
    await sendTurn(page, 'this one gets cut RAOK');
    // Wait for the streaming bubble (turn 2's delta) so we're genuinely mid-reply.
    await page.waitForFunction(
      (want) => { const el = document.querySelector('.msg.assistant.streaming p'); return el && el.textContent.includes(want); },
      TURNS[1].text,
      { timeout: 10000 },
    ).catch(() => problems.push('turn-2 streaming bubble did not appear'));
    await page.click(TOGGLE); // toggle OFF, mid-reply
    check((await page.getAttribute(TOGGLE, 'aria-pressed')) === 'false', 'toggle did not switch OFF');
    await until(() => page.evaluate(() => window.__ra.cancelled >= 1), 5000, 'cancel() called on toggle-off');

    // Let turn 2 finish (completion arrives while read-aloud is OFF).
    await sleep(TURNS[1].completeDelay);
    await until(
      () => page.evaluate((t) => [...document.querySelectorAll('.msg.assistant:not(.streaming) p')].some((p) => p.textContent.includes(t)), TURNS[1].text),
      10000,
      'turn-2 reply finalised in the transcript',
    );
    const spokenFinal = await page.evaluate(() => window.__ra.spoken.slice());
    check(
      !spokenFinal.includes(TURNS[1].text),
      `reply completed while OFF must NOT be spoken, but spoken = ${JSON.stringify(spokenFinal)}`,
    );
    check(spokenFinal.length === 1, `still exactly one reply ever spoken, got ${JSON.stringify(spokenFinal)}`);
    await ctx.close();

    // === Part 3: speechSynthesis ABSENT (capability detection) =============
    const ctx2 = await browser.newContext();
    await ctx2.addInitScript(STUB_ABSENT);
    const page2 = await ctx2.newPage();
    let absentError = null;
    page2.on('pageerror', (e) => { absentError = `uncaught wasm error (absent API): ${e.message}`; });
    await login(page2);
    // The toggle is hidden — degrade gracefully rather than render a dead control.
    check((await page2.locator(TOGGLE).count()) === 0, 'toggle must be hidden when speechSynthesis is absent');
    // And the app is otherwise healthy: it can still send/receive a turn.
    await sendTurn(page2, 'no speech here RAOK');
    await until(
      () => page2.evaluate((t) => [...document.querySelectorAll('.msg.assistant:not(.streaming) p')].some((p) => p.textContent.includes(t)), TURNS[0].text),
      10000,
      'turn still completes with the API absent',
    );
    if (absentError) problems.push(absentError);
    await ctx2.close();

    if (!failure && problems.length) failure = `assertions failed:\n  - ${problems.join('\n  - ')}`;
  } catch (e) {
    failure = `exception: ${e.message}`;
  } finally {
    await browser.close();
    server.close();
  }
  if (failure) { console.error(`FAIL: ${failure}`); process.exit(1); }
  console.log('PASS: read-aloud speaks completed replies when ON, cancels on toggle-off (and stays silent while OFF), and hides when speechSynthesis is absent.');
}
main();
