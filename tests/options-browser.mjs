// Loaded-extension regression with a controlled native-compilation fixture.
// The browser profile is disposable; no user's host registration is changed.
import assert from 'node:assert/strict';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { createRequire } from 'node:module';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const require = createRequire(import.meta.url);
const { chromium } = require(process.env.NAAB_PLAYWRIGHT || 'playwright');
const root = fileURLToPath(new URL('..', import.meta.url));
const extension = path.join(root, 'extension', 'dist');
const profile = await mkdtemp(path.join(tmpdir(), 'naab-options-profile-'));
const errors = [];
let context;
try {
  context = await chromium.launchPersistentContext(profile, {
    headless: true, channel: 'chromium',
    ...(process.env.NAAB_CHROMIUM ? { executablePath: process.env.NAAB_CHROMIUM } : {}),
    args: [`--disable-extensions-except=${extension}`, `--load-extension=${extension}`, '--no-proxy-server'],
  });
  const worker = context.serviceWorkers()[0] || await context.waitForEvent('serviceworker', { timeout: 15_000 });
  const id = new URL(worker.url()).host;
  await worker.evaluate(() => {
    chrome.runtime.sendNativeMessage = async (_host, message) => {
      if (message.method === 'status.get') return { version: 1, id: message.id, ok: true, payload: { healthy: true, companionVersion: '0.2.0', protocolVersion: 1, mode: 'local-import', capabilities: [] } };
      if (globalThis.rejectCompile) throw new Error('Controlled compiler failure');
      return { version: 1, id: message.id, ok: true, payload: {
        networkRules: [], cosmeticRules: [{ domains: ['example.test'], selector: '.advertisement', raw: 'example.test##.advertisement', source: 'Browser fixture' }],
        diagnostics: [], stats: { network: 0, cosmetic: 1, unsupported: 0, ignored: 0 },
      } };
    };
    const query = chrome.tabs.query.bind(chrome.tabs);
    chrome.tabs.query = options => options?.url ? new Promise(() => {}) : query(options);
  });
  const page = await context.newPage();
  page.on('pageerror', error => errors.push(error.message));
  await page.goto(`chrome-extension://${id}/options.html`);
  await page.waitForFunction(() => !document.querySelector('#import').disabled);
  const draft = 'example.test##.advertisement';
  await page.locator('#filters').fill(draft);
  await page.locator('#import').click();
  await page.waitForFunction(() => document.querySelector('#message').textContent.startsWith('Saved.'), null, { timeout: 5000 });
  assert.equal((await page.evaluate(() => chrome.runtime.sendMessage({ type: 'config.get' }))).payload.text, draft);
  assert.equal(await page.locator('#import').isDisabled(), false, 'Stalled tab query must not hold the committed save');

  await worker.evaluate(() => { globalThis.rejectCompile = true; });
  await page.locator('#filters').fill('draft to keep on known failure');
  await page.locator('#import').click();
  await page.waitForFunction(() => document.querySelector('#error').textContent.includes('Controlled compiler failure'));
  assert.equal(await page.locator('#import').isDisabled(), false);
  assert.equal(await page.locator('#filters').inputValue(), 'draft to keep on known failure');
  await worker.evaluate(() => { globalThis.rejectCompile = false; });

  // Deliver the save to the real worker, but lose its confirmation at the UI.
  // The save may have committed, so the UI cannot claim the old state or retry.
  await page.evaluate(() => {
    const send = chrome.runtime.sendMessage.bind(chrome.runtime);
    chrome.runtime.sendMessage = message => message.type === 'config.import'
      ? send(message).then(() => { throw new Error('A listener indicated an asynchronous response by returning true, but the message channel closed before a response was received'); })
      : send(message);
  });
  await page.locator('#filters').fill('saved despite lost reply');
  await page.locator('#import').click();
  await page.waitForFunction(() => document.querySelector('#summary').textContent.includes('state is unknown'));
  assert.equal(await page.locator('#import').isDisabled(), true);
  assert.equal(await page.locator('#filters').isDisabled(), false);
  assert.equal(await page.locator('#filters').getAttribute('readonly'), '');
  assert.equal(await page.locator('#filters').inputValue(), 'saved despite lost reply');
  assert.equal((await worker.evaluate(() => chrome.storage.local.get('config'))).config.text, 'saved despite lost reply');
  await page.reload();
  await page.waitForFunction(() => !document.querySelector('#import').disabled);
  assert.equal(await page.locator('#filters').inputValue(), 'saved despite lost reply');
  assert.deepEqual(errors, []);
  console.log('Options browser regression passed: save with stalled tab query, known compiler failure, lost confirmation after real commit, copyable draft, and reopened saved state. Native compilation was a controlled fixture.');
} finally {
  await context?.close();
  // profile is the exact mkdtemp child of the system temp directory.
  if (path.dirname(profile) !== path.resolve(tmpdir())) throw new Error('Unexpected browser profile path');
  await rm(profile, { recursive: true, force: true });
}
