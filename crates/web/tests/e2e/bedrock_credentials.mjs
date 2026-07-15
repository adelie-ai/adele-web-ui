// Headless end-to-end check for Bedrock's separate credential fields (UX fix
// item 2). Explicitly invoked — NOT part of `just check` (that stays
// browser-free).
//
// Serves the REAL built SPA (`../../dist`, produced by `trunk build`) from a fake
// BFF that lists one Bedrock connection and records every `set_connection_secret`
// it receives. It asserts, in a real browser, that: (1) configuring the Bedrock
// connection shows THREE separate write-only inputs — Access Key ID (text),
// Secret Access Key (password), Session Token (password) — not one glued field;
// (2) saving with all three joins them on the wire into
// `ACCESS_KEY_ID:SECRET_ACCESS_KEY:SESSION_TOKEN`; (3) saving with no session
// token yields `ACCESS_KEY_ID:SECRET_ACCESS_KEY` with NO trailing colon; and (4)
// the fields are write-only — blank on every reopen, never pre-filled. Fails on
// any uncaught wasm panic.
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

if (!fs.existsSync(path.join(DIST, 'index.html'))) {
  console.error(`No built SPA at ${DIST}. Run \`trunk build\` in crates/web first.`);
  process.exit(2);
}

const reply = (id, result) => JSON.stringify({ result: { id, result } });

// One Bedrock connection with a stored non-secret config (so Configure pre-fills
// profile/region) but no credential yet.
const BEDROCK = {
  id: 'aws',
  connector_type: 'bedrock',
  display_label: 'aws (bedrock)',
  availability: { status: 'ok' },
  has_credentials: false,
  config: { type: 'bedrock', aws_profile: 'adele', region: 'us-east-1' },
};

// Clearly-fake AWS credential material.
const ACCESS = 'AKIAIOSFODNN7EXAMPLE';
const SECRET = 'wJalrXUtnFEMI/K7MDENG/EXAMPLEKEY';
const SESSION = 'FQoGZXIvYXdzEXAMPLESESSIONTOKEN';

const RESULTS = (id) => ({
  list_available_models: reply(id, {
    models: [
      { connection_id: 'aws', connection_label: 'aws (bedrock)', model: { id: 'anthropic.claude', display_name: 'Claude', context_limit: 200000, capabilities: { reasoning: true, vision: true, tools: true, embedding: false } } },
    ],
  }),
  get_purposes: reply(id, { purposes: { interactive: { connection: 'aws', model: 'anthropic.claude' }, dreaming: { connection: 'aws', model: 'anthropic.claude' }, consolidation: { connection: 'aws', model: 'anthropic.claude' }, embedding: { connection: 'aws', model: 'anthropic.claude' } } }),
  list_conversations: reply(id, { conversations: [{ id: 'c1', title: 'Bedrock Probe', message_count: 0, updated_at: '2026-07-14 00:00:00', archived: false }] }),
  get_conversation: reply(id, { conversation: { id: 'c1', title: 'Bedrock Probe', messages: [] } }),
  subscribe_conversations: reply(id, 'ack'),
  get_conversation_scratchpad: reply(id, { scratchpad: [] }),
});

// Records every credential the client writes, in order.
const secretsSet = [];

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
    if (key === 'list_connections') { sock.send(reply(o.id, { connections: [BEDROCK] })); return; }
    if (key === 'update_connection' || key === 'create_connection') { sock.send(reply(o.id, 'ack')); return; }
    if (key === 'set_connection_secret') {
      const p = o.command.set_connection_secret;
      secretsSet.push({ id: p.id, credential: p.credential });
      sock.send(reply(o.id, 'ack'));
      return;
    }
    const out = RESULTS(o.id)[key];
    if (out) sock.send(out);
  });
});

const waitFor = async (predicate, ms = 15000) => {
  const start = Date.now();
  while (Date.now() - start < ms) {
    if (predicate()) return true;
    await new Promise((r) => setTimeout(r, 50));
  }
  return false;
};

async function openConfigureForm(page) {
  await page.locator('.conn-card', { hasText: 'aws' }).locator('button', { hasText: 'Configure' }).click();
  await page.waitForSelector('.conn-cred', { timeout: 5000 });
}

async function main() {
  await new Promise((r) => server.listen(PORT, '127.0.0.1', r));
  const browser = await chromium.launch({ headless: true });
  let failure = null;
  try {
    const page = await (await browser.newContext({ viewport: { width: 390, height: 800 } })).newPage();
    page.on('pageerror', (e) => { failure = `uncaught wasm error: ${e.message}`; });
    await page.goto(`http://127.0.0.1:${PORT}`, { waitUntil: 'domcontentloaded' });
    await page.fill('input[placeholder="Username"]', 'dave');
    await page.fill('input[type="password"]', 'testpass123');
    await page.click('button[type="submit"]');
    await page.waitForSelector('form.composer', { timeout: 15000 });
    await page.waitForSelector('span.dot.online', { timeout: 15000 });

    // Open Settings → Connections → Configure the Bedrock connection.
    await page.click('button[aria-label="Open settings"]');
    await page.locator('.sheet-tab', { hasText: 'Connections' }).click();
    await page.waitForSelector('.conn-card', { timeout: 15000 });
    await openConfigureForm(page);

    // (1) THREE separate credential inputs, correctly typed. A single glued
    // field would give exactly one input in the credential card.
    const credFields = page.locator('.conn-cred .conn-cred-field');
    const count = await credFields.count();
    if (count !== 3) { failure = `expected 3 Bedrock credential fields, found ${count}`; return; }
    const akInput = page.locator('.conn-cred-field', { hasText: 'Access Key ID' }).locator('input');
    const skInput = page.locator('.conn-cred-field', { hasText: 'Secret Access Key' }).locator('input');
    const stInput = page.locator('.conn-cred-field', { hasText: 'Session Token' }).locator('input');
    if (await akInput.getAttribute('type') !== 'text') { failure = 'Access Key ID should be a text input'; return; }
    if (await skInput.getAttribute('type') !== 'password') { failure = 'Secret Access Key should be a password input'; return; }
    if (await stInput.getAttribute('type') !== 'password') { failure = 'Session Token should be a password input'; return; }
    // Write-only: nothing pre-filled.
    if (await akInput.inputValue() !== '' || await skInput.inputValue() !== '' || await stInput.inputValue() !== '') { failure = 'credential fields must start blank (write-only)'; return; }

    // (2) Save with all three → joined `ACCESS:SECRET:SESSION` on the wire.
    await akInput.fill(ACCESS);
    await skInput.fill(SECRET);
    await stInput.fill(SESSION);
    await page.locator('.conn-form-actions .conn-btn.primary', { hasText: 'Save' }).click();
    if (!await waitFor(() => secretsSet.length === 1)) { failure = `no credential written (secretsSet=${JSON.stringify(secretsSet)})`; return; }
    console.log('first save credential:', JSON.stringify(secretsSet[0]));
    if (secretsSet[0].id !== 'aws') { failure = `credential written for wrong id: ${secretsSet[0].id}`; return; }
    if (secretsSet[0].credential !== `${ACCESS}:${SECRET}:${SESSION}`) { failure = `three-part join wrong: ${JSON.stringify(secretsSet[0].credential)}`; return; }

    // Form closed back to the list on success.
    await page.waitForSelector('.conn-add', { timeout: 5000 });

    // (4) Reopen: fields are blank again (write-only, never pre-filled).
    await openConfigureForm(page);
    const ak2 = page.locator('.conn-cred-field', { hasText: 'Access Key ID' }).locator('input');
    const sk2 = page.locator('.conn-cred-field', { hasText: 'Secret Access Key' }).locator('input');
    const st2 = page.locator('.conn-cred-field', { hasText: 'Session Token' }).locator('input');
    if (await ak2.inputValue() !== '' || await sk2.inputValue() !== '' || await st2.inputValue() !== '') { failure = 'credential fields must be blank on reopen (write-only)'; return; }

    // (3) Save with NO session token → two-part join, no trailing colon.
    await ak2.fill(ACCESS);
    await sk2.fill(SECRET);
    await page.locator('.conn-form-actions .conn-btn.primary', { hasText: 'Save' }).click();
    if (!await waitFor(() => secretsSet.length === 2)) { failure = `second credential not written (secretsSet=${JSON.stringify(secretsSet)})`; return; }
    console.log('second save credential:', JSON.stringify(secretsSet[1]));
    if (secretsSet[1].credential !== `${ACCESS}:${SECRET}`) { failure = `two-part join wrong: ${JSON.stringify(secretsSet[1].credential)}`; return; }
    if (secretsSet[1].credential.endsWith(':')) { failure = `two-part credential has a trailing colon: ${JSON.stringify(secretsSet[1].credential)}`; return; }
  } finally {
    await browser.close();
    server.close();
  }
  if (failure) { console.error(`FAIL: ${failure}`); process.exit(1); }
  console.log('PASS: Bedrock shows 3 separate fields and joins them into the credential string on the wire.');
}
main();
