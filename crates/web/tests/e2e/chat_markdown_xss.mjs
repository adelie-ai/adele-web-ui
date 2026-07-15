// Headless adversarial XSS gauntlet for chat markdown rendering (issue #48).
// Explicitly invoked — NOT part of `just check`. Companion to chat_markdown.mjs:
// that one proves rich formatting + a representative XSS attempt; THIS one
// hammers the sanitizer with a broad battery of hostile constructs, EACH IN ITS
// OWN assistant turn (a fresh top-level parse — the strongest adversarial
// context, and it prevents one payload's unclosed tag from swallowing the next),
// and asserts — in a real headless Chromium — that after every turn NOTHING
// executed and NO dangerous token / on* handler / dangerous tag / script-scheme
// href survived in the rendered bubble.
//
// The battery spans how sanitizers usually get bypassed:
//   - mutation-XSS via foreign-content namespace confusion (`<svg>/<math>` +
//     `<style>` breakout) — the classic ammonia/DOMPurify-era mXSS shape;
//   - `javascript:` obfuscated with entities / leading whitespace / `&colon;`,
//     in markdown links and raw anchors;
//   - `data:text/html` in href and img src;
//   - SVG `onload` + inline `<svg><script>`;
//   - `<iframe>`, `<form action=js>` + `formaction`, `<base href=js>`,
//     `<style>@import js`;
//   - tag-splitting (`<scr<script>ipt>`);
//   - event handlers on an otherwise-allowed tag (`<p onclick=…>`);
//   - an attempt to override the forced `target="_blank"` with `target="_self"`.
//
// A payload that "fires" sets `window.__pwned`; `alert` is stubbed to count. The
// test fails if any turn sets __pwned, fires an alert/dialog, throws a wasm
// error, or leaves a dangerous token in that turn's bubble. A stripped-to-empty
// bubble is a PASS (the payload was correctly removed).
import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { WebSocketServer } from 'ws';
import { chromium } from 'playwright';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const DIST = path.resolve(HERE, '../../dist');
const PORT = 9399;
const BEARER = 'adele.bearer';
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

if (!fs.existsSync(path.join(DIST, 'index.html'))) {
  console.error(`No built SPA at ${DIST}. Run \`trunk build\` in crates/web first.`);
  process.exit(2);
}

const reply = (id, result) => JSON.stringify({ result: { id, result } });
const eventFrame = (variant, fields) => JSON.stringify({ event: { event: { [variant]: fields } } });

const B64_SCRIPT = Buffer.from('<script>window.__pwned=1</script>').toString('base64');

// One hostile construct per turn. No trailing sentinel inside a payload — a
// correctly-stripped payload can render an EMPTY bubble, which is a pass.
const PAYLOADS = [
  '<script>window.__pwned=1;window.alert("x")</script>',
  '<img src=x onerror="window.__pwned=1">',
  '<svg onload="window.__pwned=1"></svg>',
  '<svg><script>window.__pwned=1</script></svg>',
  '<svg><style><a title="</style><img src=x onerror=window.__pwned=1>">',
  '<math><mtext><table><mglyph><style><img src=x onerror=window.__pwned=1></style></mtext></math>',
  '[md js link](javascript:window.__pwned=1)',
  '<a href="javascript:window.__pwned=1">raw js</a>',
  '<a href="java&#115;cript:window.__pwned=1">entity js</a>',
  '<a href="  javascript:window.__pwned=1">ws js</a>',
  '<a href="javascript&colon;window.__pwned=1">colon-entity js</a>',
  '[data link](data:text/html;base64,' + B64_SCRIPT + ')',
  '<img src="data:text/html,<script>window.__pwned=1</script>">',
  '<scr<script>ipt>window.__pwned=1</script>',
  '<iframe src="javascript:window.__pwned=1"></iframe>',
  '<form action="javascript:window.__pwned=1"><button formaction="javascript:window.__pwned=1">go</button></form>',
  '<base href="javascript:window.__pwned=1">',
  '<style>@import "javascript:window.__pwned=1";</style>',
  '<p onclick="window.__pwned=1" onmouseover="window.__pwned=1">hover</p>',
  '<a href="https://ok.example" target="_self" onfocus="window.__pwned=1">override</a>',
];

const RESULTS = (id) => ({
  list_available_models: reply(id, { models: [{ connection_id: 'local', connection_label: 'local (test)', model: { id: 'llama3.2:latest', display_name: 'Llama 3.2', context_limit: 131072, capabilities: { reasoning: false, vision: false, tools: true, embedding: false } } }] }),
  get_purposes: reply(id, { purposes: { interactive: { connection: 'local', model: 'llama3.2:latest' }, dreaming: { connection: 'local', model: 'llama3.2:latest' }, consolidation: { connection: 'local', model: 'llama3.2:latest' }, embedding: { connection: 'local', model: 'llama3.2:latest' } } }),
  list_conversations: reply(id, { conversations: [{ id: 'c1', title: 'XSS MDOK', message_count: 0, updated_at: '2026-07-15 00:00:00', archived: false }] }),
  get_conversation: reply(id, { conversation: { id: 'c1', title: 'XSS MDOK', messages: [] } }),
  subscribe_conversations: reply(id, 'ack'),
});

// The BFF hands out the next payload on each send_message, so the Nth turn
// renders PAYLOADS[N-1].
let turn = 0;
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
    if (key === 'send_message') {
      const rid = `xss-${turn}`;
      const payload = PAYLOADS[turn] ?? '';
      turn += 1;
      sock.send(reply(o.id, { send_message_ack: { request_id: rid, task_id: `task-${rid}` } }));
      setTimeout(() => sock.send(eventFrame('assistant_completed', { conversation_id: 'c1', request_id: rid, full_response: payload })), 40);
      return;
    }
    const out = RESULTS(o.id)[key];
    if (out) sock.send(out);
  });
});

const problems = [];
const check = (cond, msg) => { if (!cond) problems.push(msg); };
async function until(fn, timeout, label) {
  const t0 = Date.now();
  while (Date.now() - t0 < timeout) { if (await fn()) return true; await sleep(50); }
  problems.push(`timeout waiting for: ${label}`);
  return false;
}

const XSS_PROBE = () => { window.__pwned = undefined; window.__alerts = 0; window.alert = () => { window.__alerts += 1; }; };

const FORBIDDEN = ['<script', '<iframe', '<svg', '<math', '<style', '<base', '<form', '<object', '<embed', 'onerror', 'onload', 'onclick', 'onmouseover', 'onfocus', 'formaction', 'javascript:', 'data:text/html', 'target="_self"', 'target=_self'];

async function main() {
  await new Promise((r) => server.listen(PORT, '127.0.0.1', r));
  const browser = await chromium.launch({ headless: true });
  let failure = null;
  try {
    const ctx = await browser.newContext();
    await ctx.addInitScript(XSS_PROBE);
    const page = await ctx.newPage();
    page.on('pageerror', (e) => { failure = `uncaught wasm error: ${e.message}`; });
    page.on('dialog', async (d) => { problems.push(`native dialog fired: ${d.message()}`); await d.dismiss(); });

    await page.goto(`http://127.0.0.1:${PORT}`, { waitUntil: 'domcontentloaded' });
    await page.fill('input[placeholder="Username"]', 'dave');
    await page.fill('input[type="password"]', 'testpass123');
    await page.click('button[type="submit"]');
    await page.waitForSelector('form.composer', { timeout: 15000 });
    await page.waitForSelector('span.dot.online', { timeout: 15000 });

    for (let i = 0; i < PAYLOADS.length; i += 1) {
      const before = await page.evaluate(() => document.querySelectorAll('.msg.assistant:not(.streaming) .msg-body').length);
      await page.fill('form.composer input', `hostile turn ${i}`);
      await page.waitForSelector('form.composer button:not([disabled])', { timeout: 5000 });
      await page.click('form.composer button');
      // Wait for this turn's assistant bubble to land.
      const arrived = await until(
        () => page.evaluate((n) => document.querySelectorAll('.msg.assistant:not(.streaming) .msg-body').length > n, before),
        8000,
        `turn ${i} (${JSON.stringify(PAYLOADS[i]).slice(0, 60)}) renders an assistant bubble`,
      );
      if (!arrived) break;
      // Let any async handler (img onerror, svg onload) fire.
      await sleep(150);

      const state = await page.evaluate(() => {
        const bubbles = [...document.querySelectorAll('.msg.assistant:not(.streaming) .msg-body')];
        const b = bubbles[bubbles.length - 1];
        return {
          pwned: window.__pwned,
          alerts: window.__alerts,
          html: b.innerHTML,
          anchors: [...b.querySelectorAll('a')].map((a) => ({ target: a.getAttribute('target'), href: a.getAttribute('href') })),
          withHandlers: [...b.querySelectorAll('*')].filter((el) => [...el.attributes].some((at) => /^on/i.test(at.name))).map((el) => el.tagName),
          dangerousTags: [...b.querySelectorAll('script,iframe,svg,math,style,base,form,object,embed')].map((el) => el.tagName),
        };
      });
      const tag = `turn ${i} ${JSON.stringify(PAYLOADS[i]).slice(0, 48)}`;
      check(state.pwned === undefined, `${tag}: payload EXECUTED (window.__pwned = ${JSON.stringify(state.pwned)})`);
      check(state.alerts === 0, `${tag}: alert() fired (${state.alerts})`);
      const lower = state.html.toLowerCase();
      for (const bad of FORBIDDEN) check(!lower.includes(bad), `${tag}: forbidden token ${JSON.stringify(bad)} survived: ${JSON.stringify(state.html).slice(0, 120)}`);
      check(state.withHandlers.length === 0, `${tag}: on* handler survived on ${JSON.stringify(state.withHandlers)}`);
      check(state.dangerousTags.length === 0, `${tag}: dangerous tag survived ${JSON.stringify(state.dangerousTags)}`);
      for (const a of state.anchors) {
        check(a.target === '_blank' || a.target === null, `${tag}: anchor target must be _blank, got ${JSON.stringify(a.target)}`);
        const h = (a.href || '').trim().toLowerCase();
        check(!h.startsWith('javascript:') && !h.startsWith('data:text/html'), `${tag}: script-scheme href survived: ${JSON.stringify(a.href)}`);
      }
    }

    await ctx.close();
    if (!failure && problems.length) failure = `assertions failed:\n  - ${problems.join('\n  - ')}`;
  } catch (e) {
    failure = `exception: ${e.message}`;
  } finally {
    await browser.close();
    server.close();
  }
  if (failure) { console.error(`FAIL: ${failure}`); process.exit(1); }
  console.log(`PASS: ${PAYLOADS.length} hostile constructs, each in its own turn, all neutralised — none executed, no on* handler / dangerous tag / script-scheme href survived, forced target=_blank held.`);
}
main();
