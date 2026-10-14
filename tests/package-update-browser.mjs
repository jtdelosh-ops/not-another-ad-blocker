// Simulate a package update in a disposable installed folder/profile. Rust
// compiles the fixture; no real browser registration or user's settings change.
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { cp, mkdtemp, readFile, writeFile, rm } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { NativeClient, validateSubscriptionState } from '../extension/lib/native-client.mjs';
import { nativeTransport } from './native-transport.mjs';

const require = createRequire(import.meta.url);
const { chromium } = require(process.env.NAAB_PLAYWRIGHT || 'playwright');
const root = fileURLToPath(new URL('..', import.meta.url));
const binary = process.env.NAAB_BINARY || path.join(root, 'companion/target/release', process.platform === 'win32' ? 'naab-companion.exe' : 'naab-companion');
const source = process.env.NAAB_EXTENSION_DIR || path.join(root, 'extension/dist');
const temporary = await mkdtemp(path.join(tmpdir(), 'naab-package-update-'));
const extension = path.join(temporary, 'installed-extension');
const profile = path.join(temporary, 'browser-profile');
const text = '||ads.example.test^\n127.0.0.1##.advertisement';
const compiled = await new NativeClient(nativeTransport(binary)).compile(text, 'My custom rules');
const timestamp = new Date().toISOString();
const stats = { network: 1, cosmetic: 0, unsupported: 0, ignored: 0 };
const subscriptions = validateSubscriptionState({
  manifest: {
    snapshotId: 'a'.repeat(64), fetchedAt: timestamp,
    lists: [{ id: 'easyprivacy', title: 'EasyPrivacy', url: 'https://easylist.to/easylist/easyprivacy.txt', fetchedAt: timestamp, sha256: 'b'.repeat(64), bytes: 1000, version: 'fixture' }],
    counts: { network: 1, cosmetic: 0, diagnostics: 0 }, stats,
    coverage: { networkSupported: 1, networkDropped: 0, cosmeticSupported: 0, cosmeticDropped: 0, exceptionSafetySuppressed: 0, diagnosticsTotal: 0, diagnosticsTruncated: false },
    listStats: [{ id: 'easyprivacy', ...stats }], unsupportedReasons: [],
  },
  compiled: { networkRules: [{ id: 1, priority: 1, action: { type: 'block' }, condition: { urlFilter: '||tracker.example.test^', resourceTypes: ['image'] } }], cosmeticRules: [], diagnostics: [], stats },
  appliedAt: timestamp,
});
const config = { version: 1, enabled: false, disabledSites: ['paused.example.test'], source: 'My custom rules', text, updatedAt: timestamp, compiled, subscriptions };
const server = createServer((request, response) => {
  response.setHeader('Cache-Control', 'no-store');
  if (request.url === '/probe.js') { response.setHeader('Content-Type', 'application/javascript'); response.end('window.adLoaded=true;'); }
  else { response.setHeader('Content-Type', 'text/html'); response.end(`<div class="advertisement">Harmless banner</div><script>window.adLoaded=false</script><script src="http://ads.example.test:${server.address().port}/probe.js"></script>`); }
});
let context;
const launch = () => chromium.launchPersistentContext(profile, {
  headless: true, channel: 'chromium',
  ...(process.env.NAAB_CHROMIUM ? { executablePath: process.env.NAAB_CHROMIUM } : {}),
  args: [`--disable-extensions-except=${extension}`, `--load-extension=${extension}`, '--host-resolver-rules=MAP *.example.test 127.0.0.1', '--no-proxy-server'],
});
const workerFor = async () => context.serviceWorkers()[0] || await context.waitForEvent('serviceworker', { timeout: 15_000 });
try {
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  await cp(source, extension, { recursive: true });
  context = await launch();
  let worker = await workerFor();
  const id = new URL(worker.url()).host;
  await worker.evaluate(config => chrome.storage.local.set({ config }), config);
  await context.close(); context = undefined;
  // Replace package files at the exact installed path; use a changed manifest
  // version so this exercises an extension update rather than only a restart.
  await cp(source, extension, { recursive: true, force: true });
  const manifestPath = path.join(extension, 'manifest.json');
  const manifest = JSON.parse(await readFile(manifestPath, 'utf8'));
  const parts = manifest.version.split('.').map(Number); parts[parts.length - 1]++;
  manifest.version = parts.join('.');
  await writeFile(manifestPath, JSON.stringify(manifest));
  context = await launch(); worker = await workerFor();
  assert.equal(new URL(worker.url()).host, id, 'Installed path must preserve extension identity');
  await worker.evaluate(() => { chrome.runtime.sendNativeMessage = async () => { throw new Error('Simulated unavailable companion'); }; });
  const options = await context.newPage();
  await options.goto(`chrome-extension://${id}/options.html`);
  await options.waitForFunction(() => document.querySelector('#summary').textContent.includes('paused globally'));
  assert.equal(await options.locator('#filters').inputValue(), text);
  assert.equal(await options.locator('#global').isChecked(), false);
  assert.equal(await options.locator('#easylist').isChecked(), false);
  assert.equal(await options.locator('#easyprivacy').isChecked(), true);
  assert.match(await options.locator('#sites').textContent(), /paused.example.test/);
  assert.deepEqual((await worker.evaluate(() => chrome.storage.local.get('config'))).config, config);
  assert.equal(await worker.evaluate(() => chrome.runtime.getManifest().version), manifest.version);
  await options.waitForFunction(() => document.querySelector('#companion').textContent.includes('Companion unavailable'));
  const page = await context.newPage();
  await page.goto(`http://127.0.0.1:${server.address().port}`);
  assert.equal(await page.evaluate(() => window.adLoaded), true);
  await options.locator('#global').check();
  await options.waitForFunction(() => document.querySelector('#message').textContent.startsWith('Saved.'));
  await page.reload();
  assert.equal(await page.evaluate(() => window.adLoaded), false);
  await page.waitForFunction(() => getComputedStyle(document.querySelector('.advertisement')).display === 'none');
  await options.locator('#filters').fill('an unsaved draft while companion is unavailable');
  await options.locator('#import').click();
  await options.waitForFunction(() => document.querySelector('#error').textContent.includes('Simulated unavailable companion'));
  assert.equal((await worker.evaluate(() => chrome.storage.local.get('config'))).config.text, text);
  console.log('PASS: update preserves extension ID, custom rules, subscription selection, site exceptions, and paused protection; saved network/cosmetic rules work without the companion; failed import keeps saved rules.');
} finally {
  await context?.close();
  server.closeAllConnections();
  if (server.listening) await new Promise(resolve => server.close(resolve));
  if (path.dirname(temporary) !== path.resolve(tmpdir()) || !path.basename(temporary).startsWith('naab-package-update-')) throw new Error('Unsafe update-test cleanup target');
  await rm(temporary, { recursive: true, force: true });
}
