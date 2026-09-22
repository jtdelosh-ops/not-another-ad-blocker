// Optional real Chromium check. All site content is a synthetic loopback fixture.
// Only native-host delivery is substituted, using actual Rust compiler results.
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { mkdir, mkdtemp, rm } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import { createRequire } from 'node:module';
import { NativeClient } from '../extension/lib/native-client.mjs';
import { nativeTransport } from './native-transport.mjs';
const require = createRequire(import.meta.url);
const { chromium } = require(process.env.NAAB_PLAYWRIGHT || 'playwright');
const root = fileURLToPath(new URL('..', import.meta.url));
const client = new NativeClient(nativeTransport(process.env.NAAB_BINARY || path.join(root, 'companion/target/debug', `naab-companion${process.platform === 'win32' ? '.exe' : ''}`)));
const text = 'ads-fixture.test##div:has(> .t-j-inbanlabel-container)';
const payloads = { status: await client.status(), compiled: await client.compile(text) };
assert.equal(payloads.compiled.stats.cosmetic, 1);
let serial = 0;
const server = createServer((_req, res) => {
  res.setHeader('Content-Type', 'text/html'); res.setHeader('Cache-Control', 'no-store');
  res.end(`<!doctype html><title>NAAB rotating wrapper fixture</title><main id="normal-page">
    <div id="ad" class="rotating-${++serial}"><div class="inner-${serial}"><video loop muted playsinline></video></div><div class="t-j-inbanlabel-container">Advertisement</div></div>
    <div id="ordinary"><video loop muted playsinline></video><span>Normal video</span></div>
    <div id="similar"><span class="t-j-inbanlabel-container-other">Ordinary label</span></div>
    </main><script>for (const v of document.querySelectorAll('video')) v.src = URL.createObjectURL(new Blob([], {type:'video/webm'}));</script>`);
});
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const port = server.address().port;
const results = path.join(root, 'test-results'); await mkdir(results, { recursive: true });
const profile = await mkdtemp(path.join(results, 'child-cosmetic-profile-'));
const extension = path.join(root, 'extension/dist');
let context;
try {
  context = await chromium.launchPersistentContext(profile, { headless: true, channel: 'chromium',
    ...(process.env.NAAB_CHROMIUM ? { executablePath: process.env.NAAB_CHROMIUM } : {}),
    args: [`--disable-extensions-except=${extension}`, `--load-extension=${extension}`, '--host-resolver-rules=MAP * 127.0.0.1', '--no-proxy-server'] });
  const worker = context.serviceWorkers()[0] || await context.waitForEvent('serviceworker');
  await worker.evaluate(({ payloads, text }) => {
    chrome.runtime.sendNativeMessage = async (_host, request) => {
      let payload;
      if (request.type === 'status.get') payload = payloads.status;
      else if (request.type === 'rules.compile' && request.payload.text === text) payload = payloads.compiled;
      else throw new Error('Unexpected fixture native request');
      return structuredClone({ version: 1, id: request.id, ok: true, payload });
    };
  }, { payloads, text });
  const options = await context.newPage();
  await options.goto(`chrome-extension://${new URL(worker.url()).host}/options.html`);
  const mutate = async message => { const reply = await options.evaluate(message => chrome.runtime.sendMessage(message), message); assert.equal(reply.ok, true, JSON.stringify(reply)); };
  await mutate({ type: 'config.import', source: 'Rotating wrapper regression', text });
  const page = await context.newPage();
  const check = async (host, hidden) => {
    await page.goto(`http://${host}:${port}`);
    await page.waitForFunction(hidden => (getComputedStyle(document.querySelector('#ad')).display === 'none') === hidden, hidden);
    for (const selector of ['#normal-page', '#ordinary', '#similar']) assert.notEqual(await page.locator(selector).evaluate(el => getComputedStyle(el).display), 'none', selector);
  };
  await check('ads-fixture.test', true);
  const oldClass = await page.locator('#ad').getAttribute('class');
  await check('ads-fixture.test', true);
  assert.notEqual(await page.locator('#ad').getAttribute('class'), oldClass);
  await page.evaluate(() => {
    const ad = document.querySelector('#ad'); ad.className = 'changed-after-load';
    const late = document.createElement('div'); late.id = 'late'; late.innerHTML = '<video loop muted playsinline></video>';
    document.querySelector('main').append(late);
  });
  assert.notEqual(await page.locator('#late').evaluate(el => getComputedStyle(el).display), 'none');
  await page.evaluate(() => { const label = document.createElement('div'); label.className = 't-j-inbanlabel-container'; document.querySelector('#late').append(label); });
  await page.waitForFunction(() => getComputedStyle(document.querySelector('#late')).display === 'none');
  assert.equal(await page.locator('#ad').evaluate(el => getComputedStyle(el).display), 'none');
  await check('other-fixture.test', false);
  await mutate({ type: 'config.site', host: 'ads-fixture.test', enabled: false }); await check('ads-fixture.test', false);
  await mutate({ type: 'config.site', host: 'ads-fixture.test', enabled: true }); await check('ads-fixture.test', true);
  await mutate({ type: 'config.enabled', enabled: false }); await check('ads-fixture.test', false);
  await mutate({ type: 'config.enabled', enabled: true }); await check('ads-fixture.test', true);
  console.log('PASS: rotating wrappers, reloads, late ad labels, ordinary blob videos, domain scope, site pause and global pause in real Chromium. Synthetic page only; native delivery substituted with real Rust results.');
} finally {
  await context?.close();
  await new Promise(resolve => server.close(resolve));
  const target = path.resolve(profile);
  if (path.dirname(target) !== path.resolve(results) || !path.basename(target).startsWith('child-cosmetic-profile-')) throw new Error('Unexpected fixture cleanup target');
  await rm(target, { recursive: true, force: true });
}
