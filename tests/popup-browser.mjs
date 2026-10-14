// Focused loaded-extension regression. Uses an isolated Chromium profile and
// never changes the user's installed extension or browser profile.
import assert from 'node:assert/strict';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { createRequire } from 'node:module';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const require = createRequire(import.meta.url);
const { chromium } = require(process.env.NAAB_PLAYWRIGHT || 'playwright');
const root = fileURLToPath(new URL('..', import.meta.url));
const extension = process.env.NAAB_EXTENSION_DIR || path.join(root, 'extension', 'dist');
const profile = await mkdtemp(path.join(tmpdir(), 'naab-popup-profile-'));
const pageErrors = [];
let context;
let options;

async function openPopup(id, mode = 'normal') {
  const page = await context.newPage();
  page.on('pageerror', error => pageErrors.push(error.message));
  // The router correctly rejects popup.html opened as a normal browser tab.
  // Relay through the trusted packaged options page while exercising the real
  // popup bundle and real background controller in an isolated browser.
  await page.exposeBinding('naabSend', (_, message) => options.evaluate(input => chrome.runtime.sendMessage(input), message));
  await page.addInitScript(selectedMode => {
    const send = message => window.naabSend(message);
    if (selectedMode === 'tab-stall') chrome.tabs.query = () => new Promise(() => {});
    if (selectedMode === 'delayed-tab') chrome.tabs.query = () => new Promise(resolve => { window.releaseTabQuery = resolve; });
    let first = true;
    chrome.runtime.sendMessage = message => {
      if (selectedMode === 'stall-first-config' && message?.type === 'config.get' && first) {
        first = false;
        return new Promise(resolve => { window.releaseStaleConfig = resolve; });
      }
      if (selectedMode === 'recovery' && message?.type === 'config.get') {
        return Promise.resolve({ ok: false, code: 'RECOVERY_REQUIRED', error: 'Reload required' });
      }
      return send(message);
    };
  }, mode);
  await page.goto(`chrome-extension://${id}/popup.html`);
  return page;
}

async function savedConfig(page) {
  const response = await page.evaluate(() => chrome.runtime.sendMessage({ type: 'config.get' }));
  assert.equal(response.ok, true, JSON.stringify(response));
  return response.payload;
}

async function checkToggle(page, enabled) {
  try {
    await page.waitForFunction(expected => {
      const toggle = document.querySelector('#global');
      return !toggle.disabled && toggle.checked === expected;
    }, enabled, { timeout: 12_000 });
  } catch (error) {
    const state = await page.evaluate(() => ({ text: document.body.innerText, disabled: document.querySelector('#global').disabled, checked: document.querySelector('#global').checked }));
    throw new Error(`Popup did not show protection=${enabled}: ${JSON.stringify(state)}; page errors: ${JSON.stringify(pageErrors)}`, { cause: error });
  }
  assert.equal((await savedConfig(page)).enabled, enabled, 'Popup switch must match the saved protection state');
}

try {
  context = await chromium.launchPersistentContext(profile, {
    headless: process.env.NAAB_HEADFUL !== '1',
    channel: 'chromium',
    ...(process.env.NAAB_CHROMIUM ? { executablePath: process.env.NAAB_CHROMIUM } : {}),
    args: [`--disable-extensions-except=${extension}`, `--load-extension=${extension}`, '--no-proxy-server'],
  });
  const worker = context.serviceWorkers()[0] || await context.waitForEvent('serviceworker', { timeout: 15_000 });
  const id = new URL(worker.url()).host;
  options = await context.newPage();
  options.on('pageerror', error => pageErrors.push(error.message));
  await options.goto(`chrome-extension://${id}/options.html`);
  assert.equal((await options.evaluate(() => chrome.runtime.sendMessage({ type: 'config.get' }))).ok, true);

  const popup = await openPopup(id);
  await checkToggle(popup, true);
  for (const enabled of [false, true, false, true]) {
    if (enabled) await popup.locator('#global').check();
    else await popup.locator('#global').uncheck();
    await checkToggle(popup, enabled);
  }
  const reopened = await openPopup(id);
  await checkToggle(reopened, true);

  // A tab query that never resolves must not hold up the global switch.
  const tabStall = await openPopup(id, 'tab-stall');
  await checkToggle(tabStall, true);
  assert.match(await tabStall.locator('#site').textContent(), /Browser or unsupported page/);
  assert.equal(await tabStall.locator('#site-toggle').isDisabled(), true);

  // Tab discovery can finish while a protection change is still awaiting its
  // reply. That unrelated render must not reenable mutation controls early.
  const delayedTab = await openPopup(id, 'delayed-tab');
  await checkToggle(delayedTab, true);
  await delayedTab.evaluate(() => {
    const send = chrome.runtime.sendMessage.bind(chrome.runtime);
    let first = true;
    chrome.runtime.sendMessage = message => {
      if (message?.type !== 'config.enabled' || !first) return send(message);
      first = false;
      return send(message).then(response => new Promise(resolve => { window.releaseChange = () => resolve(response); }));
    };
  });
  await delayedTab.locator('#global').uncheck();
  await delayedTab.evaluate(() => window.releaseTabQuery([{ id: 1, url: 'https://example.test/' }]));
  await delayedTab.waitForFunction(() => document.querySelector('#site').textContent === 'example.test');
  assert.equal(await delayedTab.locator('#global').isDisabled(), true);
  assert.equal(await delayedTab.locator('#site-toggle').isDisabled(), true);
  await delayedTab.waitForFunction(() => typeof window.releaseChange === 'function');
  await delayedTab.evaluate(() => window.releaseChange());
  await checkToggle(delayedTab, false);
  await delayedTab.locator('#global').check();
  await checkToggle(delayedTab, true);

  // A stalled first settings read must show retry. A late reply from that read
  // cannot overwrite the fresh settings returned by the retry.
  const retry = await openPopup(id, 'stall-first-config');
  await retry.locator('#retry-config').waitFor({ state: 'visible', timeout: 15_000 });
  assert.equal(await retry.locator('#global').isDisabled(), true);
  await retry.locator('#retry-config').click();
  await checkToggle(retry, true);
  await retry.evaluate(async () => {
    const current = await chrome.runtime.sendMessage({ type: 'config.get' });
    window.releaseStaleConfig({ ...current, payload: { ...current.payload, enabled: false } });
  });
  await retry.waitForTimeout(100);
  await checkToggle(retry, true);

  // A mutation that never receives confirmation must not claim the requested
  // state took effect or offer another change against an unknown state.
  const stalledChange = await openPopup(id);
  await checkToggle(stalledChange, true);
  await stalledChange.evaluate(() => {
    const send = chrome.runtime.sendMessage.bind(chrome.runtime);
    chrome.runtime.sendMessage = (message, ...rest) => message?.type === 'config.enabled'
      ? new Promise(() => {}) : send(message, ...rest);
  });
  await stalledChange.locator('#global').uncheck();
  await stalledChange.waitForFunction(() => document.querySelector('#protection').textContent.includes('state is unknown'), null, { timeout: 15_000 });
  assert.equal(await stalledChange.locator('#global').isDisabled(), true);
  assert.equal((await savedConfig(stalledChange)).enabled, true);

  // The background can explicitly require recovery even on the first read.
  const recovery = await openPopup(id, 'recovery');
  await recovery.waitForFunction(() => document.querySelector('#protection').textContent.includes('state is unknown'));
  assert.equal(await recovery.locator('#global').isDisabled(), true);
  assert.equal(await recovery.locator('#retry-config').isHidden(), true);

  assert.deepEqual(pageErrors, []);
  console.log('Popup browser regression passed: saved off/on state, repeated toggles, stalled tab lookup, retry, late reply, mutation timeout, recovery error.');
} finally {
  await context?.close();
  await rm(profile, { recursive: true, force: true });
}
