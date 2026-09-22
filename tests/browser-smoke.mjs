// Optional integration check: install Playwright + Chromium before running.
// Native transport is substituted with results from the real Rust process.
// No native-host registration or existing browser profile is changed.
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { mkdir, mkdtemp, readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import { createRequire } from 'node:module';
import { NativeClient } from '../extension/lib/native-client.mjs';
import { nativeTransport } from './native-transport.mjs';

const require = createRequire(import.meta.url);
const { chromium } = require(process.env.NAAB_PLAYWRIGHT || 'playwright');
const root = fileURLToPath(new URL('..', import.meta.url));
const binary = process.env.NAAB_BINARY || path.join(root, 'companion', 'target', 'debug', `naab-companion${process.platform === 'win32' ? '.exe' : ''}`);
const client = new NativeClient(nativeTransport(binary));
const initialText = await readFile(new URL('./fixtures/local-filters.txt', import.meta.url), 'utf8');
const exceptionText = `${initialText}\n@@||ads.example.test^\n`;
const payloads = { status: await client.status(), initial: await client.compile(initialText, 'Browser test'), exception: await client.compile(exceptionText, 'Browser test') };
const resultDir = path.join(root, 'test-results'); await mkdir(resultDir, { recursive: true });
const profile = await mkdtemp(path.join(resultDir, 'chromium-profile-'));
let origin;
const server = createServer((request, response) => {
  response.setHeader('Cache-Control', 'no-store');
  if (request.url === '/banner.js') { response.setHeader('Content-Type', 'application/javascript'); response.end('window.bannerLoaded=true;'); }
  else {
    response.setHeader('Content-Type', 'text/html');
    const embedded = request.url === '/parent' ? `<iframe src="http://paused.example.test:${server.address().port}/child"></iframe>` : '';
    response.end(`<!doctype html><html><head><title>NAAB browser regression</title></head><body><h1>Visible content</h1><div class="naab-demo-ad">Ad fixture</div><script>window.bannerLoaded=false</script><script src="http://ads.example.test:${server.address().port}/banner.js"></script>${embedded}</body></html>`);
  }
});
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
origin = `http://127.0.0.1:${server.address().port}`;
let context;
try {
  const extension = path.join(root, 'extension', 'dist');
  context = await chromium.launchPersistentContext(profile, {
    headless: true, channel: 'chromium',
    ...(process.env.NAAB_CHROMIUM ? { executablePath: process.env.NAAB_CHROMIUM } : {}),
    args: [`--disable-extensions-except=${extension}`, `--load-extension=${extension}`, '--host-resolver-rules=MAP *.example.test 127.0.0.1', '--no-proxy-server'],
  });
  const worker = context.serviceWorkers()[0] || await context.waitForEvent('serviceworker', { timeout: 15_000 });
  const id = new URL(worker.url()).host;
  await worker.evaluate(({ payloads, initialText, exceptionText }) => {
    chrome.runtime.sendNativeMessage = async (host, request) => {
      if (host !== 'com.naab.companion') throw new Error('Unexpected host');
      let payload;
      if (request.type === 'status.get') payload = payloads.status;
      else if (request.payload.text === initialText) payload = payloads.initial;
      else if (request.payload.text === exceptionText) payload = payloads.exception;
      else throw new Error('Simulated unavailable native companion');
      return { version: 1, id: request.id, ok: true, payload };
    };
  }, { payloads, initialText, exceptionText });
  const options = await context.newPage();
  const pageErrors = []; options.on('pageerror', error => pageErrors.push(error.message));
  await options.goto(`chrome-extension://${id}/options.html`);
  await options.waitForFunction(() => document.querySelector('#summary').textContent.includes('No rules loaded'));
  await options.waitForFunction(() => document.querySelector('#companion').textContent.includes('is ready'));
  await options.locator('#source-name').fill('Browser test');
  await options.locator('#filters').fill(initialText);
  await options.locator('#import').click();
  await options.waitForFunction(() => document.querySelector('#summary').textContent.includes('3 network rules and 2 cosmetic'));
  assert.equal(await options.locator('#error').textContent(), '');
  assert.ok((await options.locator('#diagnostic-summary').textContent()).includes('1 unsupported'));
  const page = await context.newPage();
  page.on('pageerror', error => pageErrors.push(error.message));
  await page.goto(origin);
  await page.waitForFunction(() => getComputedStyle(document.querySelector('.naab-demo-ad')).display === 'none');
  assert.equal(await page.evaluate(() => window.bannerLoaded), false, 'DNR blocked actual subresource');

  // Per-site bypass removes both network and cosmetic effects after navigation.
  const setSite = async enabled => {
    const response = await options.evaluate(enabled => chrome.runtime.sendMessage({ type: 'config.site', host: '127.0.0.1', enabled }), enabled);
    assert.equal(response.ok, true, JSON.stringify(response));
  };
  await setSite(false); await page.reload();
  await page.waitForFunction(() => getComputedStyle(document.querySelector('.naab-demo-ad')).display !== 'none');
  assert.equal(await page.evaluate(() => window.bannerLoaded), true);
  await setSite(true); await page.reload();
  await page.waitForFunction(() => getComputedStyle(document.querySelector('.naab-demo-ad')).display === 'none');
  assert.equal(await page.evaluate(() => window.bannerLoaded), false);

  // A paused domain embedded under another protected top-level page must not
  // bypass that page's network filtering merely by being the initiator.
  const pauseEmbedded = await options.evaluate(() => chrome.runtime.sendMessage({ type: 'config.site', host: 'paused.example.test', enabled: false }));
  assert.equal(pauseEmbedded.ok, true);
  await page.goto(`${origin}/parent`);
  const embedded = page.frames().find(frame => frame.url().includes('paused.example.test'));
  assert.ok(embedded);
  assert.equal(await embedded.evaluate(() => window.bannerLoaded), false, 'Paused origin cannot bypass filtering when embedded under a protected site');
  await page.goto(`http://paused.example.test:${server.address().port}/child`);
  assert.equal(await page.evaluate(() => window.bannerLoaded), true, 'The paused domain is bypassed when it is the actual top-level site');
  const resumeEmbedded = await options.evaluate(() => chrome.runtime.sendMessage({ type: 'config.site', host: 'paused.example.test', enabled: true }));
  assert.equal(resumeEmbedded.ok, true);
  await page.goto(origin);

  await options.locator('#global').uncheck(); await options.waitForFunction(() => document.querySelector('#summary').textContent.includes('paused globally'));
  await page.reload(); assert.equal(await page.evaluate(() => window.bannerLoaded), true);
  await page.waitForFunction(() => getComputedStyle(document.querySelector('.naab-demo-ad')).display !== 'none');
  await options.locator('#global').check(); await options.waitForFunction(() => document.querySelector('#summary').textContent.includes('enabled'));

  // A failed native import retains the active rule set.
  await options.locator('#filters').fill('force native failure'); await options.locator('#import').click();
  await options.waitForFunction(() => document.querySelector('#error').textContent.includes('Simulated unavailable'));
  await page.reload(); assert.equal(await page.evaluate(() => window.bannerLoaded), false);

  // A higher priority exception permits an otherwise blocked request.
  await options.locator('#filters').fill(exceptionText); await options.locator('#import').click();
  await options.waitForFunction(() => document.querySelector('#summary').textContent.includes('4 network rules'));
  await page.reload(); assert.equal(await page.evaluate(() => window.bannerLoaded), true);
  await page.waitForFunction(() => getComputedStyle(document.querySelector('.naab-demo-ad')).display === 'none');
  await options.screenshot({ path: path.join(resultDir, 'options.png'), fullPage: true });

  // If persisting a mutation and rolling its DNR change back both fail, the UI
  // must stop presenting the old configuration as verified protection.
  await worker.evaluate(() => {
    const replace = chrome.declarativeNetRequest.updateDynamicRules.bind(chrome.declarativeNetRequest);
    const save = chrome.storage.local.set.bind(chrome.storage.local);
    let updates = 0;
    chrome.declarativeNetRequest.updateDynamicRules = async change => {
      if (++updates > 1) throw new Error('Simulated rollback failure');
      return replace(change);
    };
    chrome.storage.local.set = async values => {
      if ('config' in values) throw new Error('Simulated storage failure');
      return save(values);
    };
  });
  await options.locator('#global').uncheck();
  await options.waitForFunction(() => /unknown/i.test(document.querySelector('#summary').textContent));
  assert.equal(await options.locator('#global').isDisabled(), true);
  assert.equal(await options.locator('#import').isDisabled(), true);
  assert.ok((await options.locator('#error').textContent()).includes('rollback'));
  assert.deepEqual(pageErrors, []);
  console.log('PASS: real Chromium extension load, local import UI, native status display, DNR block/allow, cosmetic hide, site/global toggles, and failed-import retention.');
  console.log('LIMIT: browser Native Messaging was substituted with real Rust results; OS registration/launch requires a separate installed-browser check.');
} finally {
  await context?.close();
  await new Promise(resolve => server.close(resolve));
}
