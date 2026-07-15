// Headless end-to-end regression for graceful recovery from a rejected/expired
// session token (issue #42). Explicitly invoked — NOT part of `just check`.
//
// Before the fix the SPA held an invalid token and retry-spammed the `/ws`
// upgrade forever ("WebSocket is already in CLOSING or CLOSED state", "transport
// closed before reply"), never recovering. This drives the REAL built SPA
// (`../../dist`, produced by `trunk build`) against three fake BFFs and asserts
// the SPA now recovers to the login screen (or reconnects) instead:
//
//   1. Expired token  → app opens straight on the LOGIN screen (pre-emptive
//      `exp` check), the dead token is cleared from storage, and logging in
//      connects and works. No CLOSING/CLOSED console spam.
//   2. Rejected (non-expired) token → the BFF refuses every `/ws` upgrade; after
//      a few fast failures the app drops to LOGIN and STOPS retrying (bounded
//      upgrade attempts), clearing the token. No CLOSING/CLOSED console spam.
//   3. Healthy mid-session drop → the BFF accepts, serves the initial load, then
//      drops the socket once; the app RECONNECTS and stays in chat (never drops
//      to login) — the phone-sleep/network-change path is unregressed.
import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { WebSocketServer } from 'ws';
import { chromium } from 'playwright';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const DIST = path.resolve(HERE, '../../dist');
const BEARER = 'adele.bearer';
const TOKEN_KEY = 'adele.token';
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

if (!fs.existsSync(path.join(DIST, 'index.html'))) {
  console.error(`No built SPA at ${DIST}. Run \`trunk build\` in crates/web first.`);
  process.exit(2);
}

// --- helpers -----------------------------------------------------------------

const nowSecs = () => Math.floor(Date.now() / 1000);

// A JWT-shaped `header.payload.sig` (base64url, no padding — matching the BFF's
// `jsonwebtoken` output) whose payload carries `exp`. Nothing verifies the
// signature; the SPA only reads `exp` from the payload.
function makeJwt(expSecs) {
  const b64 = (obj) => Buffer.from(JSON.stringify(obj)).toString('base64url');
  const header = b64({ alg: 'HS256', typ: 'JWT' });
  const payload = b64({ sub: 'adele', exp: expSecs, iat: nowSecs() });
  return `${header}.${payload}.c2ln`;
}

// Minimal valid protocol replies for the connect-time initial load (real shapes;
// two models, one chat-capable), delivered as normal TEXT frames.
const reply = (id, result) => JSON.stringify({ result: { id, result } });
const RESULTS = (id) => ({
  list_available_models: reply(id, {
    models: [
      { connection_id: 'local', connection_label: 'local (test)', model: { id: 'llama3.2:latest', display_name: 'Llama 3.2', context_limit: 131072, capabilities: { reasoning: false, vision: false, tools: true, embedding: false } } },
    ],
  }),
  get_purposes: reply(id, { purposes: { interactive: { connection: 'local', model: 'llama3.2:latest' }, dreaming: { connection: 'local', model: 'llama3.2:latest' }, consolidation: { connection: 'local', model: 'llama3.2:latest' }, embedding: { connection: 'local', model: 'llama3.2:latest' } } }),
  list_conversations: reply(id, { conversations: [{ id: 'c1', title: 'Test', message_count: 0, updated_at: '2026-07-14 00:00:00', archived: false }] }),
  get_conversation: reply(id, { conversation: { id: 'c1', title: 'Test', messages: [] } }),
  subscribe_conversations: reply(id, 'ack'),
});

const MIME = { '.html': 'text/html', '.js': 'text/javascript', '.wasm': 'application/wasm', '.css': 'text/css' };
// Build an HTTP server that serves the SPA + `/login` (returns `loginToken`) +
// `/auth/config`. The caller wires up `/ws` behaviour separately.
function makeHttpServer(loginToken) {
  return http.createServer((req, res) => {
    const url = new URL(req.url, `http://${req.headers.host}`);
    if (url.pathname === '/login' && req.method === 'POST') { res.writeHead(200, { 'content-type': 'application/json' }); res.end(JSON.stringify({ token: loginToken })); return; }
    if (url.pathname === '/auth/config') { res.writeHead(200, { 'content-type': 'application/json' }); res.end(JSON.stringify({ methods: ['password'] })); return; }
    let fp = path.join(DIST, url.pathname === '/' ? 'index.html' : url.pathname);
    if (!fs.existsSync(fp) || fs.statSync(fp).isDirectory()) fp = path.join(DIST, 'index.html');
    res.writeHead(200, { 'content-type': MIME[path.extname(fp)] || 'application/octet-stream' });
    res.end(fs.readFileSync(fp));
  });
}

// Answer the SPA's initial-load commands so it goes online. `onConn` lets a
// scenario intervene (e.g. drop the socket after a delay).
function attachAcceptingWs(server, onConn) {
  const wss = new WebSocketServer({ server, path: '/ws', handleProtocols: (p) => (p.has(BEARER) ? BEARER : false) });
  wss.on('connection', (sock) => {
    sock.on('message', (data) => {
      const o = JSON.parse(data.toString());
      const key = typeof o.command === 'string' ? o.command : Object.keys(o.command)[0];
      const out = RESULTS(o.id)[key];
      if (out) sock.send(out);
    });
    if (onConn) onConn(sock);
  });
  return wss;
}

// A browser page wired to record CLOSING/CLOSED spam and wasm panics.
async function newTrackedPage(browser) {
  const ctx = await browser.newContext();
  const page = await ctx.newPage();
  const state = { spam: 0, pageError: null };
  page.on('console', (msg) => { if (/already in (closing|closed) state/i.test(msg.text())) state.spam += 1; });
  page.on('pageerror', (e) => { state.pageError = e.message; });
  return { page, state };
}

const seedToken = (page, token) =>
  page.addInitScript(([k, t]) => { window.localStorage.setItem(k, JSON.stringify(t)); }, [TOKEN_KEY, token]);
const readStoredToken = (page) => page.evaluate((k) => window.localStorage.getItem(k), TOKEN_KEY);

// --- scenario 1: expired token -> login, then log in and it works ------------

async function scenarioExpired() {
  const freshToken = makeJwt(nowSecs() + 3600);
  const server = makeHttpServer(freshToken);
  attachAcceptingWs(server);
  const port = await listen(server);
  const browser = await chromium.launch({ headless: true });
  let fail = null;
  try {
    const { page, state } = await newTrackedPage(browser);
    await seedToken(page, makeJwt(nowSecs() - 60)); // already expired
    await page.goto(`http://127.0.0.1:${port}`, { waitUntil: 'domcontentloaded' });

    // Pre-emptive check: opens on the login screen, not a chat with a dead socket.
    await page.waitForSelector('form.login-form', { timeout: 15000 });
    const chatShown = await page.locator('form.composer').count();
    const cleared = await readStoredToken(page);
    await sleep(1500); // give any (buggy) reconnect loop time to spam
    console.log(`[expired] login=${await page.locator('form.login-form').count()} chat=${chatShown} storedToken=${cleared} spam=${state.spam}`);
    if (chatShown) fail = 'expired token opened the chat instead of the login screen';
    else if (cleared !== null) fail = `expired token was not cleared from storage (got ${cleared})`;
    else if (state.spam) fail = `expired token produced ${state.spam} CLOSING/CLOSED warning(s)`;

    // Now log in: it should connect and come online.
    if (!fail) {
      await page.fill('input[autocomplete="username"]', 'adele');
      await page.fill('input[autocomplete="current-password"]', 'testpass123');
      await page.click('button[type="submit"]');
      await page.waitForSelector('form.composer', { timeout: 15000 });
      await page.waitForSelector('span.dot.online', { timeout: 15000 });
      console.log('[expired] logged in -> composer + online');
    }
    if (!fail && state.pageError) fail = `uncaught wasm error: ${state.pageError}`;
  } finally {
    await browser.close();
    server.close();
  }
  return fail;
}

// --- scenario 2: rejected (non-expired) token -> login, retries bounded -------

async function scenarioRejected() {
  const freshToken = makeJwt(nowSecs() + 3600);
  const server = makeHttpServer(freshToken);
  // Reject EVERY /ws upgrade with a 401 (never opens) and count attempts.
  let upgrades = 0;
  server.on('upgrade', (req, socket) => {
    upgrades += 1;
    socket.write('HTTP/1.1 401 Unauthorized\r\nConnection: close\r\n\r\n');
    socket.destroy();
  });
  const port = await listen(server);
  const browser = await chromium.launch({ headless: true });
  let fail = null;
  try {
    const { page, state } = await newTrackedPage(browser);
    await seedToken(page, makeJwt(nowSecs() + 3600)); // valid exp, but BFF refuses it
    await page.goto(`http://127.0.0.1:${port}`, { waitUntil: 'domcontentloaded' });

    // A few fast failures, then drop to login and STOP retrying.
    await page.waitForSelector('form.login-form', { timeout: 15000 });
    const chatShown = await page.locator('form.composer').count();
    const attemptsAtLogin = upgrades;
    const cleared = await readStoredToken(page);
    await sleep(2500); // if it were still looping, `upgrades` would keep climbing
    const attemptsAfter = upgrades;
    console.log(`[rejected] login=${await page.locator('form.login-form').count()} chat=${chatShown} upgradesAtLogin=${attemptsAtLogin} upgradesAfter=${attemptsAfter} storedToken=${cleared} spam=${state.spam}`);
    if (chatShown) fail = 'rejected token stayed in chat instead of dropping to login';
    else if (cleared !== null) fail = `rejected token was not cleared from storage (got ${cleared})`;
    else if (attemptsAfter > attemptsAtLogin) fail = `kept retrying after login (${attemptsAtLogin} -> ${attemptsAfter} upgrades)`;
    else if (attemptsAfter < 2 || attemptsAfter > 5) fail = `expected a few bounded upgrade attempts, got ${attemptsAfter}`;
    else if (state.spam) fail = `rejected token produced ${state.spam} CLOSING/CLOSED warning(s)`;
    if (!fail && state.pageError) fail = `uncaught wasm error: ${state.pageError}`;
  } finally {
    await browser.close();
    server.close();
  }
  return fail;
}

// --- scenario 3: healthy mid-session drop -> reconnect, no regression ---------

async function scenarioHealthyDrop() {
  const server = makeHttpServer(makeJwt(nowSecs() + 3600));
  let conns = 0;
  attachAcceptingWs(server, (sock) => {
    conns += 1;
    // Drop only the first connection, shortly after it has come online.
    if (conns === 1) setTimeout(() => sock.close(), 1200);
  });
  const port = await listen(server);
  const browser = await chromium.launch({ headless: true });
  let fail = null;
  try {
    const { page, state } = await newTrackedPage(browser);
    await seedToken(page, makeJwt(nowSecs() + 3600));
    await page.goto(`http://127.0.0.1:${port}`, { waitUntil: 'domcontentloaded' });

    await page.waitForSelector('form.composer', { timeout: 15000 });
    await page.waitForSelector('span.dot.online', { timeout: 15000 }); // online on conn 1
    // Wait for the server to drop conn 1 and the SPA to reconnect (conn 2).
    await page.waitForFunction(() => document.querySelectorAll('span.dot.offline').length > 0, { timeout: 8000 }).catch(() => {});
    await page.waitForSelector('span.dot.online', { timeout: 15000 }); // online again after reconnect

    const loginShown = await page.locator('form.login-form').count();
    const chatShown = await page.locator('form.composer').count();
    console.log(`[healthy-drop] connections=${conns} login=${loginShown} chat=${chatShown}`);
    if (loginShown) fail = 'a healthy mid-session drop wrongly dropped to login';
    else if (!chatShown) fail = 'chat screen disappeared after a healthy drop';
    else if (conns < 2) fail = `expected a reconnect (>=2 connections), got ${conns}`;
    if (!fail && state.pageError) fail = `uncaught wasm error: ${state.pageError}`;
  } finally {
    await browser.close();
    server.close();
  }
  return fail;
}

function listen(server) {
  return new Promise((resolve) => {
    server.listen(0, '127.0.0.1', () => resolve(server.address().port));
  });
}

async function main() {
  const scenarios = [
    ['expired-token-drops-to-login', scenarioExpired],
    ['rejected-token-drops-to-login-bounded', scenarioRejected],
    ['healthy-drop-reconnects', scenarioHealthyDrop],
  ];
  let failed = false;
  for (const [name, run] of scenarios) {
    const fail = await run();
    if (fail) { console.error(`FAIL [${name}]: ${fail}`); failed = true; break; }
    console.log(`PASS [${name}]`);
  }
  if (failed) process.exit(1);
  console.log('PASS: SPA recovers gracefully from expired/rejected tokens and still reconnects on healthy drops.');
}
main();
