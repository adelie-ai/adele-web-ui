// Headless end-to-end check for the MCP-servers admin panel (issue #55).
// Explicitly invoked — NOT part of `just check` (that stays browser-free).
//
// Serves the REAL built SPA (`../../dist`, produced by `trunk build`) from a
// STATEFUL fake BFF that answers `list_mcp_servers` from a swappable snapshot
// and records `set_mcp_server_enabled` / `upsert_mcp_server` /
// `remove_mcp_server` / `set_mcp_secret`. It asserts, in a real browser, that:
//   (1) Settings → MCP Servers lists servers with status, tool count, and
//       transport chip, and renders the honest "sign in from the desktop" note
//       for an unauthorized OAuth server (no functional web sign-in button);
//   (2) toggling a server sends `set_mcp_server_enabled` and the row reflects
//       the new state after the re-list;
//   (3) removing a server (via the inline confirm) sends `remove_mcp_server`;
//   (4) Add (stdio) sends a well-formed `upsert_mcp_server { config_json }`;
//   (5) a Bearer save sends `set_mcp_secret` BEFORE `upsert_mcp_server`, under
//       the `{name}_token` ref.
// Fails on any uncaught wasm panic.
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

if (!fs.existsSync(path.join(DIST, 'index.html'))) {
  console.error(`No built SPA at ${DIST}. Run \`trunk build\` in crates/web first.`);
  process.exit(2);
}

const reply = (id, result) => JSON.stringify({ result: { id, result } });

// The swappable server snapshot the BFF answers `list_mcp_servers` from. Every
// required McpServerView field is present (missing required fields would fail
// deserialization in the SPA).
let servers = [
  {
    name: 'files',
    command: 'fileio-mcp',
    args: ['serve', '--root', '/data'],
    namespace: 'files',
    enabled: true,
    status: 'running',
    tool_count: 4,
    transport: 'stdio',
    target: 'fileio-mcp',
  },
  {
    name: 'gmail',
    command: '',
    args: [],
    enabled: true,
    status: 'needs_auth',
    tool_count: 0,
    transport: 'http',
    target: 'https://gmailmcp.googleapis.com/mcp/v1',
    auth_kind: 'oauth',
    oauth_authorized: false,
    oauth_account_ref: 'work-google',
    oauth_scopes: ['gmail.readonly'],
  },
];

const ACCOUNTS = [
  {
    id: 'work-google',
    display_name: 'Work Google',
    client_id: 'client-xyz',
    authorize_url: 'https://accounts.google.com/o/oauth2/v2/auth',
    token_url: 'https://oauth2.googleapis.com/token',
    refresh_token_ref: 'work-google_refresh',
    granted_scopes: ['gmail.readonly'],
    authorized: true,
  },
];

// Ordered wire log of the mutating calls (seq lets us assert secret-before-upsert).
let seq = 0;
const enabledCalls = [];
const upserts = [];
const removes = [];
const secrets = [];

// Build a McpServerView from an upserted McpServerConfig JSON string.
function viewFromConfig(configJson) {
  const cfg = JSON.parse(configJson);
  const http_ = cfg.http;
  const enabled = cfg.enabled !== false;
  let auth_kind;
  if (http_) auth_kind = http_.auth_bearer_secret ? 'bearer' : http_.oauth_account ? 'oauth' : 'none';
  return {
    name: cfg.name,
    command: cfg.command || '',
    args: cfg.args || [],
    namespace: cfg.namespace,
    enabled,
    status: enabled ? 'running' : 'disabled',
    tool_count: 0,
    transport: http_ ? 'http' : 'stdio',
    target: http_ ? http_.url : cfg.command || '',
    auth_kind,
  };
}

const RESULTS = (id) => ({
  list_available_models: reply(id, {
    models: [
      { connection_id: 'aws', connection_label: 'aws (bedrock)', model: { id: 'anthropic.claude', display_name: 'Claude', context_limit: 200000, capabilities: { reasoning: true, vision: true, tools: true, embedding: false } } },
    ],
  }),
  list_connections: reply(id, { connections: [{ id: 'aws', connector_type: 'bedrock', display_label: 'aws (bedrock)', availability: { status: 'ok' }, has_credentials: true, config: { type: 'bedrock', aws_profile: 'adele', region: 'us-east-1' } }] }),
  get_purposes: reply(id, { purposes: { interactive: { connection: 'aws', model: 'anthropic.claude' }, dreaming: { connection: 'aws', model: 'anthropic.claude' }, consolidation: { connection: 'aws', model: 'anthropic.claude' }, embedding: { connection: 'aws', model: 'anthropic.claude' } } }),
  list_conversations: reply(id, { conversations: [{ id: 'c1', title: 'MCP Probe', message_count: 0, updated_at: '2026-07-15 00:00:00', archived: false }] }),
  get_conversation: reply(id, { conversation: { id: 'c1', title: 'MCP Probe', messages: [] } }),
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
    if (key === 'list_mcp_servers') { sock.send(reply(o.id, { mcp_servers: servers })); return; }
    if (key === 'list_service_accounts') { sock.send(reply(o.id, { service_accounts: ACCOUNTS })); return; }
    if (key === 'set_mcp_server_enabled') {
      const p = o.command.set_mcp_server_enabled;
      enabledCalls.push({ seq: seq++, name: p.name, enabled: p.enabled });
      const s = servers.find((x) => x.name === p.name);
      if (s) { s.enabled = p.enabled; s.status = p.enabled ? 'running' : 'disabled'; }
      sock.send(reply(o.id, 'ack'));
      return;
    }
    if (key === 'upsert_mcp_server') {
      const p = o.command.upsert_mcp_server;
      upserts.push({ seq: seq++, config_json: p.config_json });
      const v = viewFromConfig(p.config_json);
      const idx = servers.findIndex((x) => x.name === v.name);
      if (idx >= 0) servers[idx] = v; else servers.push(v);
      sock.send(reply(o.id, 'ack'));
      return;
    }
    if (key === 'remove_mcp_server') {
      const p = o.command.remove_mcp_server;
      removes.push({ seq: seq++, name: p.name });
      servers = servers.filter((x) => x.name !== p.name);
      sock.send(reply(o.id, 'ack'));
      return;
    }
    if (key === 'set_mcp_secret') {
      const p = o.command.set_mcp_secret;
      secrets.push({ seq: seq++, id: p.id, value: p.value });
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

async function openMcpPanel(page) {
  await page.click('button[aria-label="Open settings"]');
  await page.locator('.sheet-tab', { hasText: 'MCP Servers' }).click();
  await page.waitForSelector('.mcp-card', { timeout: 15000 });
}

async function main() {
  await new Promise((r) => server.listen(PORT, '127.0.0.1', r));
  const browser = await chromium.launch({ headless: true });
  let failure = null;
  const fail = (m) => { if (!failure) failure = m; };
  try {
    const page = await (await browser.newContext({ viewport: { width: 390, height: 800 } })).newPage();
    page.on('pageerror', (e) => { fail(`uncaught wasm error: ${e.message}`); });
    await page.goto(`http://127.0.0.1:${PORT}`, { waitUntil: 'domcontentloaded' });
    await page.fill('input[placeholder="Username"]', 'dave');
    await page.fill('input[type="password"]', 'testpass123');
    await page.click('button[type="submit"]');
    await page.waitForSelector('form.composer', { timeout: 15000 });
    await page.waitForSelector('span.dot.online', { timeout: 15000 });

    // (1) The list renders both servers with status / tool count / transport.
    await openMcpPanel(page);
    const filesCard = page.locator('.mcp-card', { hasText: 'files' });
    const gmailCard = page.locator('.mcp-card', { hasText: 'gmail' });
    if (!await waitFor(async () => (await filesCard.locator('.mcp-status').innerText()).includes('Running'))) {
      fail(`files card missing Running status: ${await filesCard.innerText().catch(() => '<none>')}`); return;
    }
    const filesStatus = await filesCard.locator('.mcp-status').innerText();
    if (!filesStatus.includes('4 tools')) { fail(`files tool count missing: ${JSON.stringify(filesStatus)}`); return; }
    if ((await filesCard.locator('.mcp-chip').innerText()).toLowerCase() !== 'local') { fail('files chip should be "local"'); return; }
    if ((await gmailCard.locator('.mcp-chip').innerText()).toLowerCase() !== 'remote') { fail('gmail chip should be "remote"'); return; }
    // Honest OAuth degradation: needs-auth note, and NO functional sign-in button.
    if (!(await gmailCard.locator('.mcp-note.warn').count())) { fail('gmail should show the honest desktop sign-in note'); return; }
    if (await gmailCard.locator('button', { hasText: 'Sign in' }).count()) { fail('there must be no functional web Sign in button'); return; }
    console.log('list rendered: files (running/4 tools/local), gmail (needs-auth/remote + desktop note)');

    // (2) Toggle files → disable; the row reflects it after the re-list.
    await filesCard.locator('button', { hasText: 'Disable' }).click();
    if (!await waitFor(() => enabledCalls.length === 1)) { fail(`set_mcp_server_enabled not sent (${JSON.stringify(enabledCalls)})`); return; }
    if (enabledCalls[0].name !== 'files' || enabledCalls[0].enabled !== false) { fail(`wrong toggle payload: ${JSON.stringify(enabledCalls[0])}`); return; }
    if (!await waitFor(async () => (await filesCard.locator('.mcp-status').innerText()).includes('Disabled'))) { fail('files row did not reflect Disabled after re-list'); return; }
    console.log('toggle: set_mcp_server_enabled{files,false} sent; row now Disabled');

    // (3) Remove gmail via the inline confirm.
    await gmailCard.locator('button', { hasText: 'Remove' }).click();
    await page.waitForSelector('.mcp-confirm', { timeout: 5000 });
    await page.locator('.mcp-confirm .mcp-btn.danger', { hasText: 'Remove' }).click();
    if (!await waitFor(() => removes.length === 1)) { fail(`remove_mcp_server not sent (${JSON.stringify(removes)})`); return; }
    if (removes[0].name !== 'gmail') { fail(`wrong remove payload: ${JSON.stringify(removes[0])}`); return; }
    if (!await waitFor(async () => (await page.locator('.mcp-card', { hasText: 'gmail' }).count()) === 0)) { fail('gmail card still present after remove'); return; }
    console.log('remove: remove_mcp_server{gmail} sent; card gone');

    // (4) Add a stdio server → well-formed upsert_mcp_server{config_json}.
    await page.locator('.mcp-add', { hasText: 'Add server' }).click();
    await page.fill('input[placeholder="e.g. files, gmail, github"]', 'weather');
    await page.fill('input[placeholder="e.g. fileio-mcp"]', 'weather-mcp');
    await page.fill('input[placeholder="e.g. serve --root /data"]', 'serve --port 8080');
    await page.locator('.mcp-form-actions .mcp-btn.primary', { hasText: 'Save' }).click();
    if (!await waitFor(() => upserts.length === 1)) { fail(`upsert_mcp_server not sent (${JSON.stringify(upserts)})`); return; }
    const stdioCfg = JSON.parse(upserts[0].config_json);
    console.log('stdio upsert config_json:', upserts[0].config_json);
    if (stdioCfg.name !== 'weather' || stdioCfg.command !== 'weather-mcp') { fail(`stdio config wrong: ${upserts[0].config_json}`); return; }
    if (JSON.stringify(stdioCfg.args) !== JSON.stringify(['serve', '--port', '8080'])) { fail(`stdio args wrong: ${JSON.stringify(stdioCfg.args)}`); return; }
    if (stdioCfg.http) { fail('stdio config must not carry an http block'); return; }
    await page.waitForSelector('.mcp-add', { timeout: 5000 }); // back to the list

    // (5) Add an http bearer server → set_mcp_secret BEFORE upsert_mcp_server.
    await page.locator('.mcp-add', { hasText: 'Add server' }).click();
    await page.fill('input[placeholder="e.g. files, gmail, github"]', 'githubmcp');
    await page.locator('.segment', { hasText: 'Remote (HTTP)' }).click();
    await page.fill('input[placeholder="https://example.com/mcp/v1"]', 'https://api.github.com/mcp');
    await page.locator('.segment', { hasText: 'Bearer token' }).click();
    await page.fill('input[placeholder="Paste token (stored write-only)"]', 'ghp_exampletoken');
    await page.locator('.mcp-form-actions .mcp-btn.primary', { hasText: 'Save' }).click();
    if (!await waitFor(() => secrets.length === 1 && upserts.length === 2)) { fail(`bearer save incomplete (secrets=${JSON.stringify(secrets)}, upserts=${upserts.length})`); return; }
    console.log('bearer secret:', JSON.stringify({ id: secrets[0].id, value_redacted: true }));
    if (secrets[0].id !== 'githubmcp_token') { fail(`bearer secret ref wrong: ${secrets[0].id}`); return; }
    if (secrets[0].value !== 'ghp_exampletoken') { fail('bearer token value not delivered'); return; }
    if (!(secrets[0].seq < upserts[1].seq)) { fail(`set_mcp_secret must precede upsert (secret seq ${secrets[0].seq}, upsert seq ${upserts[1].seq})`); return; }
    const httpCfg = JSON.parse(upserts[1].config_json);
    if (!httpCfg.http || httpCfg.http.auth_bearer_secret !== 'githubmcp_token') { fail(`http bearer config wrong: ${upserts[1].config_json}`); return; }
    console.log('bearer: set_mcp_secret{githubmcp_token} sent BEFORE upsert_mcp_server; config references the ref');
  } finally {
    await browser.close();
    server.close();
  }
  if (failure) { console.error(`FAIL: ${failure}`); process.exit(1); }
  console.log('PASS: MCP panel lists/toggles/removes/adds servers and writes the bearer secret before the upsert.');
}
main();
