// Optional Chromium regression using a real snapshot from live-subscriptions.mjs.
// Native host responses are substituted; browser DNR/storage/UI are real.
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { mkdir, mkdtemp, readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import { createRequire } from 'node:module';
import { NativeClient, validateSubscriptionState } from '../extension/lib/native-client.mjs';
import { nativeTransport } from './native-transport.mjs';

if (!process.argv[2]) throw new Error('Pass the snapshot JSON produced by tests/live-subscriptions.mjs.');
const state = validateSubscriptionState(JSON.parse(await readFile(process.argv[2], 'utf8')));
const require = createRequire(import.meta.url);
const { chromium } = require(process.env.NAAB_PLAYWRIGHT || 'playwright');
const root = fileURLToPath(new URL('..', import.meta.url));
const binary = process.env.NAAB_BINARY || path.join(root, 'companion', 'target', 'debug', `naab-companion${process.platform === 'win32' ? '.exe' : ''}`);
const client = new NativeClient(nativeTransport(binary));
const blockedHost = '2mdn.net';
assert.ok(state.compiled.networkRules.some(rule => rule.action.type === 'block' && rule.condition.requestDomains?.includes(blockedHost) && rule.condition.resourceTypes.includes('script')), 'Snapshot must contain the network fixture rule');
const cosmetic = state.compiled.cosmeticRules.find(rule => rule.domains.includes('1000logos.net') && /^#[A-Za-z_][A-Za-z0-9_-]*$/.test(rule.selector));
assert.ok(cosmetic, 'Snapshot must contain a site-scoped ID selector for the fixture');
const localText = '||local-ads.example.test^\n##.naab-local-ad';
const allowText = `${localText}\n@@||${blockedHost}^`;
const payloads = { status: await client.status(), local: await client.compile(localText, 'Browser local'), allow: await client.compile(allowText, 'Browser local') };
const resultDir = path.join(root, 'test-results'); await mkdir(resultDir, { recursive: true });
const profile = await mkdtemp(path.join(resultDir, 'subscriptions-profile-'));
const server = createServer((request, response) => {
  response.setHeader('Cache-Control', 'no-store');
  if (request.url === '/subscription.js' || request.url === '/local.js') {
    response.setHeader('Content-Type', 'application/javascript');
    response.end(request.url === '/subscription.js' ? 'window.subscriptionLoaded=true;' : 'window.localLoaded=true;');
  } else {
    response.setHeader('Content-Type', 'text/html');
    const port = server.address().port;
    response.end(`<!doctype html><html><head><title>NAAB subscription regression</title></head><body><h1>Visible fixture</h1><div id="${cosmetic.selector.slice(1)}">Subscription ad fixture</div><div class="naab-local-ad">Local ad fixture</div><script>window.subscriptionLoaded=false;window.localLoaded=false;</script><script src="http://${blockedHost}:${port}/subscription.js"></script><script src="http://local-ads.example.test:${port}/local.js"></script></body></html>`);
  }
});
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const origin = `http://1000logos.net:${server.address().port}`;
const extension = path.join(root, 'extension', 'dist');
const launch = () => chromium.launchPersistentContext(profile, {
  headless: true, channel: 'chromium', viewport: { width: 1280, height: 1100 },
  ...(process.env.NAAB_CHROMIUM ? { executablePath: process.env.NAAB_CHROMIUM } : {}),
  args: [`--disable-extensions-except=${extension}`, `--load-extension=${extension}`, '--host-resolver-rules=MAP * 127.0.0.1', '--no-proxy-server'],
});
const pageErrors = [];
let context;
try {
  context = await launch();
  let worker = context.serviceWorkers()[0] || await context.waitForEvent('serviceworker');
  const id = new URL(worker.url()).host;
  await worker.evaluate(({ state, payloads, localText, allowText }) => {
    globalThis.failSubscriptionPage = false;
    chrome.runtime.sendNativeMessage = async (host, request) => {
      if (host !== 'com.naab.companion') throw new Error('Unexpected native host');
      let payload;
      if (request.type === 'status.get') payload = payloads.status;
      else if (request.type === 'rules.compile') {
        if (request.payload.text === localText) payload = payloads.local;
        else if (request.payload.text === allowText) payload = payloads.allow;
        else throw new Error('Unexpected local fixture');
      } else if (request.type === 'lists.refresh') payload = state.manifest;
      else if (request.type === 'lists.page') {
        if (globalThis.failSubscriptionPage) throw new Error('Simulated subscription page failure');
        const { snapshotId, kind, offset } = request.payload;
        if (snapshotId !== state.manifest.snapshotId) throw new Error('Wrong snapshot');
        const all = kind === 'network' ? state.compiled.networkRules : kind === 'cosmetic' ? state.compiled.cosmeticRules : state.compiled.diagnostics;
        let end = Math.min(offset + 128, all.length);
        do {
          payload = { snapshotId, kind, offset, total: all.length, items: all.slice(offset, end), nextOffset: end < all.length ? end : null };
          if (new TextEncoder().encode(JSON.stringify(payload)).length <= 512 * 1024) break;
          --end;
        } while (end > offset);
      } else throw new Error('Unexpected native request');
      return structuredClone({ version: 1, id: request.id, ok: true, payload });
    };
  }, { state, payloads, localText, allowText });
  let options = await context.newPage(); options.on('pageerror', error => pageErrors.push(error.message));
  await options.goto(`chrome-extension://${id}/options.html`);
  await options.waitForFunction(() => document.querySelector('#companion').textContent.includes('0.2.0 is ready'));
  const send = message => options.evaluate(message => chrome.runtime.sendMessage(message), message);
  const config = async () => { const result = await send({ type: 'config.get' }); assert.equal(result.ok, true); return result.payload; };
  const mutate = async message => { const result = await send(message); assert.equal(result.ok, true, JSON.stringify(result)); return result.payload; };
  await mutate({ type: 'config.import', source: 'Browser local', text: localText });
  await mutate({ type: 'config.site', host: 'paused.example.test', enabled: false });
  await mutate({ type: 'config.enabled', enabled: false });
  await options.reload();
  await options.waitForFunction(() => !document.querySelector('#refresh-lists').disabled);
  await options.locator('#refresh-lists').click();
  await options.waitForFunction(() => document.querySelector('#list-progress').textContent.includes('Subscriptions saved'), undefined, { timeout: 90000 });
  assert.equal(await options.locator('#error').textContent(), '');
  let saved = await config();
  assert.equal(saved.enabled, false); assert.equal(saved.text, localText);
  assert.deepEqual(saved.disabledSites, ['paused.example.test']);
  assert.equal(saved.subscriptions.manifest.snapshotId, state.manifest.snapshotId);
  assert.equal(await worker.evaluate(async () => (await chrome.declarativeNetRequest.getDynamicRules()).length), 0);
  // Enabling submits every real subscription rule to Chrome's own validator.
  await mutate({ type: 'config.enabled', enabled: true });
  assert.equal(await worker.evaluate(async () => (await chrome.declarativeNetRequest.getDynamicRules()).length), state.manifest.counts.network + 2);
  let page = await context.newPage(); page.on('pageerror', error => pageErrors.push(error.message));
  const checkFixture = async (subscriptionBlocked, localBlocked, cosmeticHidden) => {
    await page.goto(origin);
    assert.equal(await page.evaluate(() => window.subscriptionLoaded), !subscriptionBlocked, 'Real subscription DNR enforcement');
    assert.equal(await page.evaluate(() => window.localLoaded), !localBlocked, 'Local DNR enforcement');
    await page.waitForFunction(({ selector, hidden }) => (getComputedStyle(document.querySelector(selector)).display === 'none') === hidden, { selector: cosmetic.selector, hidden: cosmeticHidden });
  };
  await checkFixture(true, true, true);
  await mutate({ type: 'config.site', host: '1000logos.net', enabled: false });
  await checkFixture(false, false, false);
  await mutate({ type: 'config.site', host: '1000logos.net', enabled: true });
  await checkFixture(true, true, true);
  await options.reload();
  await options.waitForFunction(() => !document.querySelector('#refresh-lists').disabled);
  await options.screenshot({ path: path.join(resultDir, 'subscriptions-options.png'), fullPage: false });
  assert.equal(await options.locator('#list-metadata .list-metadata').count(), 2);
  assert.ok(await options.locator('#diagnostics tr').count() <= 200);
  // Refresh failure occurs after a valid manifest, exercising partial transfers.
  await worker.evaluate(() => { globalThis.failSubscriptionPage = true; });
  await options.locator('#refresh-lists').click();
  await options.waitForFunction(() => document.querySelector('#error').textContent.includes('Simulated subscription page failure'));
  assert.equal((await config()).subscriptions.manifest.snapshotId, state.manifest.snapshotId);
  await checkFixture(true, true, true);
  // Local allowances override subscription blocking without removing cosmetics.
  await mutate({ type: 'config.import', source: 'Browser local', text: allowText });
  await checkFixture(false, true, true);
  await mutate({ type: 'config.import', source: 'Browser local', text: localText });
  await context.close(); context = await launch();
  worker = context.serviceWorkers()[0] || await context.waitForEvent('serviceworker');
  // No native substitution in this fresh process: saved state must work offline.
  options = await context.newPage(); options.on('pageerror', error => pageErrors.push(error.message));
  await options.goto(`chrome-extension://${id}/options.html`);
  await options.waitForFunction(() => document.querySelector('#summary').textContent.includes('network rules'));
  page = await context.newPage(); page.on('pageerror', error => pageErrors.push(error.message));
  await checkFixture(true, true, true);
  saved = await config(); assert.equal(saved.text, localText); assert.deepEqual(saved.disabledSites, ['paused.example.test']);
  await options.locator('#remove-lists').click();
  await options.waitForFunction(() => document.querySelector('#list-progress').textContent.includes('Subscriptions removed'));
  saved = await config(); assert.equal(saved.subscriptions, undefined); assert.equal(saved.text, localText); assert.equal(saved.enabled, true); assert.deepEqual(saved.disabledSites, ['paused.example.test']);
  await checkFixture(false, true, false);
  await page.waitForFunction(() => getComputedStyle(document.querySelector('.naab-local-ad')).display === 'none');
  assert.deepEqual(pageErrors, []);
  console.log(`PASS: Chromium accepted ${state.manifest.counts.network} real subscription network rules; UI paging, real network/cosmetic effects, preservation of local rules/settings, local override, site bypass, failed-refresh retention, offline browser restart, and removal passed.`);
  console.log('LIMIT: Native Messaging transport was substituted with a separately verified real snapshot; OS host discovery/launch was not tested here.');
} finally {
  await context?.close();
  await new Promise(resolve => server.close(resolve));
}
