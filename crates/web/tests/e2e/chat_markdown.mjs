// Headless end-to-end check for chat markdown rendering (issue #48). Explicitly
// invoked — NOT part of `just check` (that stays browser-free). See README.md.
//
// It serves the REAL built SPA (`../../dist`, produced by `trunk build`) from a
// minimal fake BFF that speaks the real WS protocol. A single assistant turn
// streams a rich, and partly HOSTILE, markdown reply: a heading, bold + inline
// code, a bulleted list, a link, a very long fenced code line, plus a
// `<script>` and an `<img onerror>` XSS attempt. It asserts, in a real headless
// Chromium:
//
//   1. The settled reply renders as FORMATTED HTML — a heading, <strong>, a
//      <ul>/<li> list, a safe <a> (href + target=_blank + rel noopener), and a
//      <pre> code block — not the old escaped `<p>{content}</p>` plain text.
//   2. The <script>/<img onerror> attempts DO NOT execute (no alert, no flag
//      set) and leave no `<script>`/`onerror` token in the rendered bubble —
//      ammonia stripped them before they reached the DOM via inner_html.
//   3. The code block scrolls HORIZONTALLY inside its own container
//      (pre.scrollWidth > pre.clientWidth) while the PAGE BODY does not scroll
//      sideways — wide content never blows out the mobile layout.
//   4. STREAMING is graceful: mid-stream, after a delta that ends on an
//      UNTERMINATED code fence, the partial buffer still renders (a <pre>
//      appears) without breaking the page, and settles to the final render on
//      completion.
//
// A fake BFF (not the shared local daemon) keeps this deterministic and isolated
// — concurrent agents build against that daemon and this must never race them.
// Every reply carries an MDOK marker for the same reason.
//
// The pure md→sanitized-HTML core (formatting + the sanitizer: script/img/js:
// stripping, unterminated-fence well-formedness) is unit-tested under `just
// check` in `src/markdown.rs`; this covers only the browser render + horizontal-
// scroll + no-execution layer those host tests can't reach.
import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { WebSocketServer } from 'ws';
import { chromium } from 'playwright';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const DIST = path.resolve(HERE, '../../dist');
const PORT = 9398;
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

// --- The rich, partly-hostile markdown reply --------------------------------
// A long single code line forces the <pre> to overflow horizontally. The
// <script>/<img onerror> are the XSS attempts. Built without JS template
// backticks (they clash with markdown fences) by joining lines.
const FENCE = '```';
const LONG = 'fn ' + 'x'.repeat(240) + '() { /* one very long line MDOK */ }';
const MD = [
  '# Heading MDOK',
  '',
  'Here is **bold MDOK** text, `inline_code`, and a [the docs](https://example.com/docs).',
  '',
  '- first item MDOK',
  '- second item MDOK',
  '',
  FENCE + 'rust',
  LONG,
  FENCE,
  '',
  '<script>window.__pwned = true; window.alert("xss MDOK")</script>',
  '<img src=x onerror="window.__pwned = true">',
  '',
  '> a quoted line MDOK',
  '',
  'Done MDOK',
].join('\n');

// Split mid-code-block so DELTA1 ends on an UNTERMINATED fence (opening fence +
// the long line, no closing fence yet) — the streaming-robustness case.
const splitAt = MD.indexOf(LONG) + LONG.length;
const DELTA1 = MD.slice(0, splitAt);
const DELTA2 = MD.slice(splitAt);
const RID = 'md1';

// Non-send RPC replies for the initial load (one chat-capable model, one convo).
const RESULTS = (id) => ({
  list_available_models: reply(id, {
    models: [
      { connection_id: 'local', connection_label: 'local (test)', model: { id: 'llama3.2:latest', display_name: 'Llama 3.2', context_limit: 131072, capabilities: { reasoning: false, vision: false, tools: true, embedding: false } } },
    ],
  }),
  get_purposes: reply(id, { purposes: { interactive: { connection: 'local', model: 'llama3.2:latest' }, dreaming: { connection: 'local', model: 'llama3.2:latest' }, consolidation: { connection: 'local', model: 'llama3.2:latest' }, embedding: { connection: 'local', model: 'llama3.2:latest' } } }),
  list_conversations: reply(id, { conversations: [{ id: 'c1', title: 'Markdown MDOK', message_count: 0, updated_at: '2026-07-15 00:00:00', archived: false }] }),
  get_conversation: reply(id, { conversation: { id: 'c1', title: 'Markdown MDOK', messages: [] } }),
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
  sock.on('message', (data) => {
    const o = JSON.parse(data.toString());
    const key = typeof o.command === 'string' ? o.command : Object.keys(o.command)[0];
    if (key === 'send_message') {
      sock.send(reply(o.id, { send_message_ack: { request_id: RID, task_id: `task-${RID}` } }));
      // Stream: delta 1 ends on an unterminated fence; delta 2 closes it and
      // adds the hostile tail; then completion finalises with the full text.
      setTimeout(() => sock.send(eventFrame('assistant_delta', { conversation_id: 'c1', request_id: RID, chunk: DELTA1 })), 40);
      setTimeout(() => sock.send(eventFrame('assistant_delta', { conversation_id: 'c1', request_id: RID, chunk: DELTA2 })), 260);
      setTimeout(() => sock.send(eventFrame('assistant_completed', { conversation_id: 'c1', request_id: RID, full_response: MD })), 500);
      return;
    }
    const out = RESULTS(o.id)[key];
    if (out) sock.send(out);
  });
});

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

// Record any attempt to execute injected JS. `__pwned` is set by the hostile
// payload's own script/onerror if either ever runs; `alert` is stubbed to count
// calls. Neither must fire.
const XSS_PROBE = () => {
  window.__pwned = undefined;
  window.__alerts = 0;
  window.alert = () => { window.__alerts += 1; };
};

async function main() {
  await new Promise((r) => server.listen(PORT, '127.0.0.1', r));
  const browser = await chromium.launch({ headless: true });
  let failure = null;
  try {
    const ctx = await browser.newContext();
    await ctx.addInitScript(XSS_PROBE);
    const page = await ctx.newPage();
    page.on('pageerror', (e) => { failure = `uncaught wasm error: ${e.message}`; });
    // A dialog would mean alert() actually fired natively — fail loudly.
    page.on('dialog', async (d) => { problems.push(`native dialog fired: ${d.message()}`); await d.dismiss(); });
    await login(page);

    // Send the rich turn.
    await page.fill('form.composer input', 'show me markdown MDOK');
    await page.waitForSelector('form.composer button:not([disabled])', { timeout: 5000 });
    await page.click('form.composer button');

    // 4a. Mid-stream: after delta 1 (unterminated fence) a <pre> renders in the
    // streaming bubble and the page body does not scroll horizontally.
    await until(
      () => page.evaluate(() => !!document.querySelector('.msg.assistant.streaming .msg-body pre')),
      10000,
      'streaming bubble renders a <pre> from the unterminated-fence partial',
    );
    const bodyScrollMid = await page.evaluate(() => {
      const el = document.scrollingElement || document.documentElement;
      return { sw: el.scrollWidth, cw: el.clientWidth };
    });
    check(bodyScrollMid.sw <= bodyScrollMid.cw + 2, `page must not scroll horizontally mid-stream (scrollWidth ${bodyScrollMid.sw} > clientWidth ${bodyScrollMid.cw})`);

    // Settle: the finalised (non-streaming) assistant bubble carries the reply.
    await until(
      () => page.evaluate(() => {
        const b = document.querySelector('.msg.assistant:not(.streaming) .msg-body');
        return b && b.querySelector('h1') && b.textContent.includes('Done MDOK');
      }),
      10000,
      'assistant reply finalises with formatted content',
    );

    // 1. Formatted HTML (not escaped plain text).
    const fmt = await page.evaluate(() => {
      const b = document.querySelector('.msg.assistant:not(.streaming) .msg-body');
      const a = b.querySelector('a');
      return {
        html: b.innerHTML,
        h1: b.querySelector('h1') ? b.querySelector('h1').textContent : null,
        strong: b.querySelector('strong') ? b.querySelector('strong').textContent : null,
        inlineCode: b.querySelector('code') ? b.querySelector('code').textContent : null,
        listItems: [...b.querySelectorAll('ul li')].map((li) => li.textContent),
        blockquote: !!b.querySelector('blockquote'),
        preCount: b.querySelectorAll('pre').length,
        link: a ? { href: a.getAttribute('href'), target: a.getAttribute('target'), rel: a.getAttribute('rel'), text: a.textContent } : null,
      };
    });
    check(fmt.h1 === 'Heading MDOK', `heading renders as <h1>, got ${JSON.stringify(fmt.h1)}`);
    check(fmt.strong === 'bold MDOK', `bold renders as <strong>, got ${JSON.stringify(fmt.strong)}`);
    check(fmt.inlineCode === 'inline_code', `inline code renders as <code>, got ${JSON.stringify(fmt.inlineCode)}`);
    check(fmt.listItems.length >= 2 && fmt.listItems[0].includes('first item'), `list renders as <ul><li>, got ${JSON.stringify(fmt.listItems)}`);
    check(fmt.blockquote, 'blockquote renders as <blockquote>');
    check(fmt.preCount >= 1, `code block renders as <pre>, got ${fmt.preCount}`);
    check(fmt.link && fmt.link.href === 'https://example.com/docs', `link href renders, got ${JSON.stringify(fmt.link)}`);
    check(fmt.link && fmt.link.target === '_blank', `link opens in a new tab (target=_blank), got ${JSON.stringify(fmt.link && fmt.link.target)}`);
    check(fmt.link && /noopener/.test(fmt.link.rel || '') && /noreferrer/.test(fmt.link.rel || ''), `link carries safe rel, got ${JSON.stringify(fmt.link && fmt.link.rel)}`);
    check(fmt.link && fmt.link.text === 'the docs', `link text renders, got ${JSON.stringify(fmt.link && fmt.link.text)}`);

    // 2. XSS attempts did not execute and left no dangerous token in the bubble.
    const pwned = await page.evaluate(() => window.__pwned);
    const alerts = await page.evaluate(() => window.__alerts);
    check(pwned === undefined, `injected script/onerror must NOT run (window.__pwned = ${JSON.stringify(pwned)})`);
    check(alerts === 0, `alert() must never fire, got ${alerts}`);
    const lower = fmt.html.toLowerCase();
    for (const bad of ['<script', 'onerror', 'javascript:', 'window.__pwned']) {
      check(!lower.includes(bad), `hostile token ${JSON.stringify(bad)} must not appear in rendered bubble HTML`);
    }

    // 3. Code block scrolls horizontally in its OWN container; page body does not.
    const scroll = await page.evaluate(() => {
      const pre = [...document.querySelectorAll('.msg.assistant:not(.streaming) .msg-body pre')].pop();
      const el = document.scrollingElement || document.documentElement;
      return { preSW: pre.scrollWidth, preCW: pre.clientWidth, bodySW: el.scrollWidth, bodyCW: el.clientWidth };
    });
    check(scroll.preSW > scroll.preCW + 4, `code block must overflow its own container (scrollWidth ${scroll.preSW} <= clientWidth ${scroll.preCW})`);
    check(scroll.bodySW <= scroll.bodyCW + 2, `page body must NOT scroll horizontally (scrollWidth ${scroll.bodySW} > clientWidth ${scroll.bodyCW})`);

    await ctx.close();
    if (!failure && problems.length) failure = `assertions failed:\n  - ${problems.join('\n  - ')}`;
  } catch (e) {
    failure = `exception: ${e.message}`;
  } finally {
    await browser.close();
    server.close();
  }
  if (failure) { console.error(`FAIL: ${failure}`); process.exit(1); }
  console.log('PASS: chat renders sanitized markdown (heading/bold/list/link/code), strips the XSS attempts, scrolls the code block in its own container without scrolling the page, and streams partial markdown gracefully.');
}
main();
