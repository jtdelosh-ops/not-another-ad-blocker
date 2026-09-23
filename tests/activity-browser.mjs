// Optional real-Chromium activity regression. Build the extension and companion
// first; set NAAB_PLAYWRIGHT / NAAB_CHROMIUM for a non-default local runtime.
// Only fixture-native replies are substituted, using results from the real Rust
// process. DNR, debug feedback, badge counts, storage and routes are all real.
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { mkdir, mkdtemp, rm } from 'node:fs/promises';
import { createRequire } from 'node:module';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { NativeClient } from '../extension/lib/native-client.mjs';
import { nativeTransport } from './native-transport.mjs';

const require = createRequire(import.meta.url);
const { chromium } = require(process.env.NAAB_PLAYWRIGHT || 'playwright');
const root = fileURLToPath(new URL('..', import.meta.url));
const resultDir = path.join(root, 'test-results');
const binary = process.env.NAAB_BINARY || path.join(root, 'companion', 'target', 'debug', `naab-companion${process.platform === 'win32' ? '.exe' : ''}`);
const client = new NativeClient(nativeTransport(binary));
const text = '! Deterministic activity fixture\n||ads.example.test^\n@@||allowed.ads.example.test^\n##.naab-demo-ad\n';
const payloads = { status: await client.status(), compiled: await client.compile(text, 'Activity regression') };
assert.equal(payloads.compiled.stats.network, 2);
assert.equal(payloads.compiled.stats.cosmetic, 1);
assert.equal(payloads.compiled.stats.unsupported, 0);
await mkdir(resultDir, { recursive: true });
const profile = await mkdtemp(path.join(resultDir, 'activity-profile-'));
const errors = [];
const server = createServer((request, response) => {
  response.setHeader('Cache-Control', 'no-store');
  const pathname = new URL(request.url, 'http://fixture.test').pathname;
  if (pathname.endsWith('.js')) {
    response.setHeader('Content-Type', 'application/javascript');
    response.end(`window.${pathname === '/allowed.js' ? 'allowedLoaded' : pathname === '/reference.js' ? 'referenceLoaded' : 'blockedLoaded'} = true;`);
  } else if (pathname === '/favicon.ico') { response.statusCode = 204; response.end(); }
  else {
    response.setHeader('Content-Type', 'text/html');
    const port = server.address().port;
    const fixture = pathname === '/fixture' ? `<div class="naab-demo-ad">Cosmetic fixture</div>
      <script>window.blockedLoaded = window.allowedLoaded = window.referenceLoaded = false;</script>
      <script src="http://ads.example.test:${port}/blocked.js?private_token=secret#private-fragment"></script>
      <script src="http://allowed.ads.example.test:${port}/allowed.js?private_token=secret#private-fragment"></script>
      <script src="/reference.js"></script>` : '';
    response.end(`<!doctype html><html><head><title>NAAB activity regression</title><link rel="icon" href="data:,"></head><body><h1>Activity test page</h1>${fixture}</body></html>`);
  }
});
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const origin = `http://page.example.test:${server.address().port}`;
const extension = path.join(root, 'extension', 'dist');
const launch = () => chromium.launchPersistentContext(profile, {
  headless: true, channel: 'chromium', viewport: { width: 1280, height: 1000 },
  ...(process.env.NAAB_CHROMIUM ? { executablePath: process.env.NAAB_CHROMIUM } : {}),
  args: [`--disable-extensions-except=${extension}`, `--load-extension=${extension}`, '--host-resolver-rules=MAP *.example.test 127.0.0.1', '--no-proxy-server'],
});
async function eventually(read, predicate, description, timeout = 15_000) {
  const deadline = Date.now() + timeout;
  let value;
  while (Date.now() < deadline) {
    value = await read();
    if (predicate(value)) return value;
    await new Promise(resolve => setTimeout(resolve, 100));
  }
  assert.fail(`${description}: ${JSON.stringify(value)}`);
}
let context;
try {
  context = await launch();
  let worker = context.serviceWorkers()[0] || await context.waitForEvent('serviceworker', { timeout: 15_000 });
  const id = new URL(worker.url()).host;
  await worker.evaluate(({ payloads, text }) => {
    chrome.runtime.sendNativeMessage = async (host, request) => {
      if (host !== 'com.naab.companion') throw new Error('Unexpected native host');
      const payload = request.type === 'status.get' ? payloads.status
        : request.type === 'rules.compile' && request.payload.text === text ? payloads.compiled : null;
      if (!payload) throw new Error(`Unexpected native request: ${request.type}`);
      return { version: 1, id: request.id, ok: true, payload };
    };
  }, { payloads, text });
  let options = await context.newPage();
  options.on('pageerror', error => errors.push(error.message));
  await options.goto(`chrome-extension://${id}/options.html`);
  const send = async message => {
    const result = await options.evaluate(input => chrome.runtime.sendMessage(input), message);
    assert.equal(result.ok, true, JSON.stringify(result));
    return result.payload;
  };
  await send({ type: 'config.get' });
  await send({ type: 'config.import', text, source: 'Activity regression' });

  let page = await context.newPage();
  page.on('pageerror', error => errors.push(error.message));
  const pageCDP = await context.newCDPSession(page);
  let worlds = new Map();
  pageCDP.on('Runtime.executionContextCreated', ({ context: world }) => worlds.set(world.id, world));
  pageCDP.on('Runtime.executionContextDestroyed', ({ executionContextId }) => worlds.delete(executionContextId));
  pageCDP.on('Runtime.executionContextsCleared', () => { worlds = new Map(); });
  await pageCDP.send('Runtime.enable');
  await page.goto(`${origin}/fixture`);
  const tabId = await options.evaluate(async url => (await chrome.tabs.query({})).find(tab => tab.url === url).id, page.url());
  const activity = () => send({ type: 'activity.get', tabId });
  const checkFixture = async blocked => {
    assert.deepEqual(await page.evaluate(() => ({ allowed: window.allowedLoaded, reference: window.referenceLoaded })), { allowed: true, reference: true }, 'Allowance and reference scripts must load');
    assert.equal(await page.evaluate(() => window.blockedLoaded), !blocked, 'Actual network subresource block agrees with count');
    await page.waitForFunction(hidden => (getComputedStyle(document.querySelector('.naab-demo-ad')).display === 'none') === hidden, blocked);
    await eventually(activity, view => view.blockedCount === (blocked ? '1' : '0'), 'Exact per-page count excludes allowance and cosmetic hide');
  };
  await checkFixture(true);
  const initial = await eventually(activity, view => view.entries.some(entry => entry.action === 'block') && view.entries.some(entry => entry.action === 'allow'), 'Block and exception appear in real feedback');
  assert.equal(initial.available, true);
  assert.equal(initial.entries.filter(entry => entry.action === 'block').length, 1);
  assert.ok(initial.entries.every(entry => entry.source.includes('Activity regression')));
  assert.ok(initial.entries.every(entry => entry.condition.includes('Compiled rule')));
  const blockedURL = `http://ads.example.test:${server.address().port}/blocked.js`;
  assert.equal(initial.entries.find(entry => entry.action === 'block').request, blockedURL);
  const saved = await eventually(() => worker.evaluate(async () => (await chrome.storage.session.get('activity')).activity), value => value?.entries?.length >= 2, 'Feedback persisted to session storage');
  assert.doesNotMatch(JSON.stringify(saved), /private_token|private-fragment|secret/);

  // Send through the real extension content-script world: browser-provided
  // sender metadata must prevent a web page from reading or clearing the log.
  const world = await eventually(async () => {
    for (const candidate of worlds.values()) {
      if (candidate.auxData?.isDefault) continue;
      const result = await pageCDP.send('Runtime.evaluate', { contextId: candidate.id, expression: 'typeof chrome !== "undefined" && chrome.runtime && chrome.runtime.id', returnByValue: true });
      if (result.result.value === id) return candidate.id;
    }
    return null;
  }, value => value !== null, 'Extension content-script world found');
  for (const message of [{ type: 'activity.get', tabId }, { type: 'activity.clear' }]) {
    const result = await pageCDP.send('Runtime.evaluate', { contextId: world, expression: `chrome.runtime.sendMessage(${JSON.stringify(message)})`, awaitPromise: true, returnByValue: true });
    assert.equal(result.result.value?.ok, false, 'Content script cannot invoke privileged activity routes');
    assert.match(result.result.value.error, /extension interface/);
  }
  const storageAccess = await pageCDP.send('Runtime.evaluate', {
    contextId: world, expression: 'chrome.storage.session.get("activity").then(() => "unexpected access", () => "denied")', awaitPromise: true, returnByValue: true,
  });
  assert.equal(storageAccess.result.value, 'denied', 'Session activity is not exposed to content scripts');

  const viewer = await context.newPage();
  viewer.on('pageerror', error => errors.push(error.message));
  await viewer.goto(`chrome-extension://${id}/activity.html#tab=${tabId}`);
  await viewer.waitForFunction(() => document.querySelector('#page-count').textContent === '1');
  await viewer.waitForFunction(() => document.querySelectorAll('#activity-rows tr').length >= 2);
  assert.match(await viewer.locator('#activity-rows').textContent(), /Blocked/);
  assert.match(await viewer.locator('#activity-rows').textContent(), /Exception matched/);
  await viewer.locator('#activity-rows details').first().evaluate(element => { element.open = true; });
  await viewer.screenshot({ path: path.join(resultDir, 'activity-viewer.png'), fullPage: true });
  await viewer.locator('#clear').click();
  await viewer.waitForFunction(() => document.querySelector('#activity-empty').hidden === false);
  assert.equal(await viewer.locator('#page-count').textContent(), '1', 'Clearing diagnostic rows preserves native page count');
  assert.deepEqual((await activity()).entries, []);

  // Navigation replaces the page counter, even though tab-scoped diagnostic
  // rows intentionally retain earlier pages until clear, close, or eviction.
  await page.goto(`${origin}/blank`);
  await eventually(activity, view => view.blockedCount === '0', 'Blank navigation resets count');
  await page.reload();
  assert.equal((await activity()).blockedCount, '0');
  await page.goto(`${origin}/fixture`);
  await checkFixture(true);
  await page.reload();
  await checkFixture(true);
  await send({ type: 'config.enabled', enabled: false });
  await page.reload();
  await checkFixture(false);
  await send({ type: 'config.enabled', enabled: true });
  await page.reload();
  await checkFixture(true);
  await send({ type: 'config.site', host: 'page.example.test', enabled: false });
  await page.reload();
  await checkFixture(false);
  await send({ type: 'config.site', host: 'page.example.test', enabled: true });
  await page.reload();
  await checkFixture(true);

  // A same-origin credential-bearing script is issued from a credential-bearing
  // top-level URL. Assert the stored representation is clean whether Chromium
  // itself has already removed userinfo from its feedback URL or not.
  const credentialPage = await context.newPage();
  await credentialPage.goto(`http://fixture-user:fixture-pass@ads.example.test:${server.address().port}/blank`);
  await credentialPage.evaluate(url => new Promise(resolve => {
    const script = document.createElement('script'); script.src = url; script.onload = script.onerror = resolve; document.body.append(script);
  }), `http://fixture-user:fixture-pass@ads.example.test:${server.address().port}/credential.js?private_token=secret#private-fragment`);
  const credentialView = await eventually(() => send({ type: 'activity.get', tabId: null }), view => view.entries.some(entry => entry.request.endsWith('/credential.js')), 'Credential-bearing fixture generates actual feedback');
  assert.doesNotMatch(JSON.stringify(credentialView.entries), /fixture-user|fixture-pass|private_token|private-fragment|secret/);
  const credentialTabId = credentialView.entries.find(entry => entry.request.endsWith('/credential.js')).tabId;
  await credentialPage.close();
  await eventually(() => send({ type: 'activity.get', tabId: null }), view => !view.entries.some(entry => entry.tabId === credentialTabId), 'Closing a tab removes its diagnostic rows');

  // Stop the worker without closing/reloading the extension or browser. A real
  // message wakes a new worker; real session storage and native count survive.
  const beforeSleep = await activity();
  await eventually(() => worker.evaluate(async () => (await chrome.storage.session.get('activity')).activity), value => value?.entries?.length === beforeSleep.entries.length, 'Latest tab entries persisted before worker termination');
  const browserCDP = await context.newCDPSession(options);
  const versions = new Map();
  browserCDP.on('ServiceWorker.workerVersionUpdated', ({ versions: updates }) => updates.forEach(version => versions.set(version.versionId, version)));
  await browserCDP.send('ServiceWorker.enable');
  const version = await eventually(async () => [...versions.values()].find(item => item.scriptURL === worker.url() && item.runningStatus === 'running'), Boolean, 'Running extension worker located');
  await browserCDP.send('ServiceWorker.stopWorker', { versionId: version.versionId });
  await eventually(async () => versions.get(version.versionId)?.runningStatus, value => value === 'stopped', 'Worker really stopped');
  // Configuration read waits for the ordinary startup reconciliation.
  await send({ type: 'config.get' });
  const afterSleep = await activity();
  assert.equal(afterSleep.blockedCount, beforeSleep.blockedCount, 'Worker startup rule reconciliation preserves native count');
  assert.deepEqual(afterSleep.entries, beforeSleep.entries, 'Session-only log survives worker termination');
  await page.reload();
  await checkFixture(true);
  assert.ok((await activity()).entries.length > afterSleep.entries.length, 'Restarted worker captures new real feedback');
  await browserCDP.detach();

  assert.deepEqual(errors, []);
  await context.close();
  context = await launch();
  worker = context.serviceWorkers()[0] || await context.waitForEvent('serviceworker', { timeout: 15_000 });
  options = await context.newPage();
  await options.goto(`chrome-extension://${id}/options.html`);
  await send({ type: 'config.get' });
  assert.deepEqual((await send({ type: 'activity.get', tabId: null })).entries, [], 'Full browser restart clears session log');
  assert.equal(await worker.evaluate(async () => (await chrome.declarativeNetRequest.getDynamicRules()).length), 2, 'Committed local rules survive browser restart');
  console.log('PASS: real activity feedback, exact block count, allowance/cosmetic exclusion, redaction before storage, trusted routes, viewer/clear, navigation reset, global/site controls, tab cleanup, worker restart persistence and browser restart clearing.');
  console.log(`SCREENSHOT: ${path.join(resultDir, 'activity-viewer.png')}`);
  console.log('LIMIT: Native-host transport was substituted only for fixture import/status using real Rust results; OS native-host registration is not exercised.');
} finally {
  await context?.close();
  await new Promise(resolve => server.close(resolve));
  const relative = path.relative(path.resolve(resultDir), path.resolve(profile));
  assert.ok(relative.startsWith('activity-profile-') && !relative.includes(path.sep) && !path.isAbsolute(relative), 'Temporary profile must remain directly within test-results');
  await rm(profile, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 });
}
