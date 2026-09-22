// Optional real-Chromium regression for the visual local test page.
// Pass a saved real snapshot produced by tests/live-subscriptions.mjs.
// Only an isolated temporary profile is changed; native messaging is unused.
import assert from 'node:assert/strict';
import { mkdir, mkdtemp, readFile, rm } from 'node:fs/promises';
import { createRequire } from 'node:module';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { validateSubscriptionState } from '../extension/lib/native-client.mjs';
import { startTestPage } from '../scripts/test-page.mjs';

if (!process.argv[2]) throw new Error('Pass the snapshot JSON produced by tests/live-subscriptions.mjs.');
const state = validateSubscriptionState(JSON.parse(await readFile(process.argv[2], 'utf8')));
assert.ok(state.compiled.networkRules.some(rule => rule.action.type === 'block'
  && rule.condition.urlFilter === '/adimage.'
  && rule.condition.resourceTypes.includes('image')
  && !rule.condition.resourceTypes.includes('xmlhttprequest')
  && !rule.condition.domainType && !rule.condition.initiatorDomains
  && !rule.condition.excludedInitiatorDomains), 'Snapshot must contain the unscoped EasyList /adimage. image rule');

const require = createRequire(import.meta.url);
const { chromium } = require(process.env.NAAB_PLAYWRIGHT || 'playwright');
const root = fileURLToPath(new URL('..', import.meta.url));
const extension = path.join(root, 'extension', 'dist');
const resultDir = path.join(root, 'test-results');
await mkdir(resultDir, { recursive: true });
const profile = await mkdtemp(path.join(resultDir, 'local-check-profile-'));
const fixture = await startTestPage();
const launch = () => chromium.launchPersistentContext(profile, {
  headless: true, channel: 'chromium', viewport: { width: 1200, height: 1000 },
  ...(process.env.NAAB_CHROMIUM ? { executablePath: process.env.NAAB_CHROMIUM } : {}),
  args: [`--disable-extensions-except=${extension}`, `--load-extension=${extension}`, '--no-proxy-server'],
});
const errors = [];
let context;
try {
  // Both URLs really serve identical content, independently of the browser.
  const [reference, advert] = await Promise.all(['/reference.svg', '/adimage.svg'].map(async pathname => {
    const response = await fetch(new URL(pathname, fixture.url));
    assert.equal(response.status, 200);
    assert.match(response.headers.get('cache-control'), /no-store/);
    return response.text();
  }));
  assert.equal(reference, advert);

  context = await launch();
  let worker = context.serviceWorkers()[0] || await context.waitForEvent('serviceworker', { timeout: 15_000 });
  const id = new URL(worker.url()).host;
  let options = await context.newPage();
  await options.goto(`chrome-extension://${id}/options.html`);
  const initial = await options.evaluate(() => chrome.runtime.sendMessage({ type: 'config.get' }));
  assert.equal(initial.ok, true, JSON.stringify(initial));
  assert.equal(await worker.evaluate(async () => (await chrome.declarativeNetRequest.getDynamicRules()).length), 0);
  let page = await context.newPage();
  page.on('pageerror', error => errors.push(error.message));
  const check = async (overall, advertState, referenceState = 'loaded') => {
    await page.waitForFunction(expected => document.querySelector('#overall')?.dataset.state === expected, overall, { timeout: 15_000 });
    assert.equal(await page.locator('#reference-result').getAttribute('data-state'), referenceState);
    assert.equal(await page.locator('#advert-result').getAttribute('data-state'), advertState);
  };
  await page.goto(fixture.url);
  await check('unblocked', 'loaded');

  // Seed committed storage and restart, exercising the real startup DNR path.
  await worker.evaluate(async subscriptions => {
    await chrome.storage.local.set({ config: {
      version: 1, enabled: true, disabledSites: [], source: 'No list imported', text: '', updatedAt: null,
      compiled: { networkRules: [], cosmeticRules: [], diagnostics: [], stats: { network: 0, cosmetic: 0, unsupported: 0, ignored: 0 } },
      subscriptions,
    } });
  }, state);
  await context.close();
  context = await launch();
  worker = context.serviceWorkers()[0] || await context.waitForEvent('serviceworker', { timeout: 15_000 });
  options = await context.newPage();
  await options.goto(`chrome-extension://${id}/options.html`);
  const mutate = async message => {
    const result = await options.evaluate(input => chrome.runtime.sendMessage(input), message);
    assert.equal(result.ok, true, JSON.stringify(result));
    return result.payload;
  };
  const saved = await mutate({ type: 'config.get' });
  assert.equal(saved.subscriptions.manifest.snapshotId, state.manifest.snapshotId);
  assert.equal(await worker.evaluate(async () => (await chrome.declarativeNetRequest.getDynamicRules()).length), state.manifest.counts.network);
  page = await context.newPage();
  page.on('pageerror', error => errors.push(error.message));
  await page.goto(fixture.url);
  await check('blocked', 'blocked');
  await page.screenshot({ path: path.join(resultDir, 'local-check-on.png'), fullPage: true });

  await mutate({ type: 'config.enabled', enabled: false });
  await page.locator('#rerun').click();
  await check('unblocked', 'loaded');
  await page.screenshot({ path: path.join(resultDir, 'local-check-off.png'), fullPage: true });
  await mutate({ type: 'config.enabled', enabled: true });
  await page.locator('#rerun').click();
  await check('blocked', 'blocked');

  await mutate({ type: 'config.site', host: '127.0.0.1', enabled: false });
  await page.reload();
  await check('unblocked', 'loaded');
  await mutate({ type: 'config.site', host: '127.0.0.1', enabled: true });
  await page.reload();
  await check('blocked', 'blocked');

  // A missing reference must never be mistaken for successful blocking.
  await page.route('**/reference.svg*', route => route.abort('failed'));
  await page.locator('#rerun').click();
  await page.waitForFunction(() => document.querySelector('#overall')?.dataset.state === 'inconclusive', undefined, { timeout: 15_000 });
  assert.notEqual(await page.locator('#reference-result').getAttribute('data-state'), 'loaded');
  await page.unroute('**/reference.svg*');
  await page.locator('#rerun').click();
  await check('blocked', 'blocked');
  assert.deepEqual(errors, []);
  console.log(`PASS: local visual check with ${state.manifest.counts.network} real subscription rules; empty baseline, global toggle/rerun, site pause/resume, and reference failure classification.`);
  console.log('LIMIT: Saved subscriptions were seeded into an isolated profile; OS native-host discovery and list downloads were not tested here.');
} finally {
  await context?.close();
  await fixture.close();
  // Delete only the unique profile created inside this test's result directory.
  const relative = path.relative(path.resolve(resultDir), path.resolve(profile));
  assert.ok(relative.startsWith('local-check-profile-') && !relative.includes(path.sep) && !path.isAbsolute(relative), 'Temporary profile must remain directly within test-results');
  await rm(profile, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 });
}
