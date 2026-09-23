// Synthetic pages in isolated Chromium. Native delivery is bridged to the real
// Rust process; no advertising sites or existing browser profiles are used.
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { mkdir, mkdtemp, rm } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import { createRequire } from 'node:module';
import { nativeTransport } from './native-transport.mjs';
const require = createRequire(import.meta.url);
const { chromium } = require(process.env.NAAB_PLAYWRIGHT || 'playwright');
const root = fileURLToPath(new URL('..', import.meta.url));
const transport = nativeTransport(process.env.NAAB_BINARY || path.join(root, 'companion/target/debug', `naab-companion${process.platform === 'win32' ? '.exe' : ''}`));
let failNative = false;
const server = createServer(async (req, res) => {
  res.setHeader('Cache-Control', 'no-store');
  if (req.method === 'POST' && req.url === '/native') {
    try {
      let body = ''; for await (const chunk of req) body += chunk;
      const message = JSON.parse(body);
      if (failNative) throw new Error('Fixture companion unavailable');
      res.setHeader('Content-Type', 'application/json'); res.end(JSON.stringify(await transport('com.naab.companion', message)));
    } catch (error) { res.statusCode = 500; res.end(error.message); }
    return;
  }
  res.setHeader('Content-Type', 'text/html');
  res.end(`<!doctype html><title>NAAB picker fixture</title><style>body{font:18px system-ui;padding:24px}a,.tile,.sibling{display:block;background:#dcebf0;padding:22px;margin:15px 0;width:360px}.tile{background:#ffe6b9}</style>
    <main><h1>Picker test page</h1><p id="keep">Ordinary page content stays visible.</p>
    <a id="ad" href="/navigated">Sponsored banner</a><div class="tile">Repeated card one</div><div class="tile">Repeated card two</div>
    <div class="sibling">Normal card</div><div id="parent"><span>Element without a selector</span></div>
    <iframe srcdoc="<button onclick='window.frameActions=(window.frameActions||0)+1'>Frame action</button>"></iframe></main><script>
    window.pageActions=0;
    for(const node of [window,document]) for(const type of ['pointerdown','mousedown','click','keydown']) node.addEventListener(type,()=>window.pageActions++,true);
    </script>`);
});
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const port = server.address().port;
const results = path.join(root, 'test-results'); await mkdir(results, { recursive: true });
const profile = await mkdtemp(path.join(results, 'picker-profile-'));
let context;
try {
  context = await chromium.launchPersistentContext(profile, { headless: true, channel: 'chromium', viewport: { width: 1100, height: 850 },
    ...(process.env.NAAB_CHROMIUM ? { executablePath: process.env.NAAB_CHROMIUM } : {}),
    args: [`--disable-extensions-except=${path.join(root, 'extension/dist')}`, `--load-extension=${path.join(root, 'extension/dist')}`, '--host-resolver-rules=MAP *.example.test 127.0.0.1', '--no-proxy-server'] });
  const worker = context.serviceWorkers()[0] || await context.waitForEvent('serviceworker');
  await worker.evaluate(port => {
    chrome.runtime.sendNativeMessage = async (_host, request) => {
      const reply = await fetch(`http://127.0.0.1:${port}/native`, { method: 'POST', body: JSON.stringify(request) });
      if (!reply.ok) throw new Error(await reply.text()); return reply.json();
    };
  }, port);
  const id = new URL(worker.url()).host;
  const options = await context.newPage(); await options.goto(`chrome-extension://${id}/options.html`);
  const send = message => options.evaluate(message => chrome.runtime.sendMessage(message), message);
  const mutate = async message => { const reply = await send(message); assert.equal(reply.ok, true, JSON.stringify(reply)); return reply.payload; };
  const original = '! Keep my manual filters\n||tracker.example.test^\nother.example.test##.keep';
  await mutate({ type: 'config.import', text: original, source: 'My filters' });
  await mutate({ type: 'config.site', host: 'paused.example.test', enabled: false });
  const page = await context.newPage(); const origin = `http://page.example.test:${port}`; await page.goto(origin);
  const cdp = await context.newCDPSession(page);
  const tabId = await worker.evaluate(url => chrome.tabs.query({}).then(tabs => tabs.find(tab => tab.url === url)?.id), origin + '/');
  assert.equal(typeof tabId, 'number');
  // CDP can inspect the closed shadow tree for testing. UI actions still use
  // real mouse/keyboard input, so synthetic page events cannot fake consent.
  async function pickerNode(label) {
    const { root } = await cdp.send('DOM.getDocument', { depth: -1, pierce: true });
    const all = [];
    const walk = node => { all.push(node); for (const child of [...(node.children || []), ...(node.shadowRoots || [])]) walk(child); };
    walk(root);
    const text = node => (node.nodeValue || '') + (node.children || []).map(text).join('');
    return all.find(node => node.nodeName === 'BUTTON' && !node.attributes?.includes('hidden') && text(node) === label);
  }
  async function click(label) {
    const node = await pickerNode(label); assert.ok(node, `Missing button ${label}`);
    const { model } = await cdp.send('DOM.getBoxModel', { nodeId: node.nodeId });
    const q = model.content; await page.mouse.click((q[0] + q[4]) / 2, (q[1] + q[5]) / 2);
  }
  async function visible(selector, expected) {
    await page.waitForFunction(({ selector, expected }) => (getComputedStyle(document.querySelector(selector)).display !== 'none') === expected, { selector, expected });
  }
  async function pick(selector) {
    const box = await page.locator(selector).first().boundingBox(); assert.ok(box);
    await page.mouse.click(box.x + box.width / 2, box.y + box.height / 2);
  }
  async function start() { await page.bringToFront(); await mutate({ type: 'picker.start', tabId }); await page.locator('[data-naab-picker]').waitFor(); }
  async function savedRule(selector) {
    for (let i = 0; i < 100; i++) {
      const result = await send({ type: 'config.get' });
      if (result.payload.text.includes(`page.example.test##${selector}`)) return result.payload;
      await new Promise(resolve => setTimeout(resolve, 30));
    }
    throw new Error('Rule was not saved');
  }
  await start();
  await page.locator('#ad').evaluate(el => el.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true })));
  assert.equal(await pickerNode('Preview'), undefined, 'Synthetic click cannot select');
  await pick('#ad'); assert.equal(page.url(), origin + '/', 'Picker must prevent link navigation');
  await page.keyboard.press('Enter'); await visible('#ad', false); await visible('#keep', true);
  assert.equal(await page.evaluate(() => window.pageActions), 0, 'Page capture handlers must not activate during picking or preview');
  await page.screenshot({ path: path.join(results, 'picker-preview.png') });
  await page.keyboard.press('Escape'); await visible('#ad', true);
  assert.equal((await mutate({ type: 'config.get' })).text, original, 'Cancel must not change filters');
  await start(); await pick('#ad'); await click('Preview'); await click('Save rule');
  const saved = await savedRule('#ad'); assert.ok(saved.text.startsWith(original)); assert.equal(saved.source, 'My filters'); assert.deepEqual(saved.disabledSites, ['paused.example.test']); assert.equal(saved.compiled.stats.network, 1);
  assert.equal(await page.evaluate(() => window.pageActions), 0, 'Page capture handlers must not activate on Save');
  await page.screenshot({ path: path.join(results, 'picker-saved.png') });
  await mutate({ type: 'config.import', source: saved.source, text: saved.text + '\nother.example.test##.later' });
  await click('Undo saved rule'); await page.locator('[data-naab-picker]').waitFor({ state: 'detached' }); await visible('#ad', true);
  assert.equal((await mutate({ type: 'config.get' })).text, original + '\nother.example.test##.later');
  await start(); await pick('#ad'); await click('Preview'); await click('Save rule'); await savedRule('#ad'); await click('Done');
  await page.reload(); await visible('#ad', false); await visible('#keep', true);
  await page.goto(`http://other.example.test:${port}`); await visible('#ad', true);
  await page.goto(origin); await visible('#ad', false);
  await mutate({ type: 'config.site', host: 'page.example.test', enabled: false }); await visible('#ad', true);
  assert.equal((await send({ type: 'picker.start', tabId })).ok, false);
  await mutate({ type: 'config.site', host: 'page.example.test', enabled: true }); await visible('#ad', false);
  // A shared class previews every match, while ordinary siblings stay visible.
  await start(); await pick('.tile'); await click('Preview'); await visible('.tile', false); await visible('.sibling', true);
  await click('Restore preview'); await visible('.tile', true); await click('Cancel');
  // No supported ID/class: explain it and allow choosing a usable parent.
  await start(); await pick('#parent span'); assert.equal(await pickerNode('Preview'), undefined);
  await click('Select parent'); await click('Preview'); await visible('#parent', false); await click('Cancel'); await visible('#parent', true);
  // Native failure must restore the temporary preview and keep saved state.
  const before = await mutate({ type: 'config.get' });
  await start(); await pick('.tile'); await click('Preview'); failNative = true; await click('Save rule');
  await visible('.tile', true); assert.deepEqual(await mutate({ type: 'config.get' }), before); failNative = false;
  await page.keyboard.press('Escape');
  await start();
  const frameButton = page.frameLocator('iframe').getByRole('button');
  const frameBox = await frameButton.boundingBox(); assert.ok(frameBox);
  await page.mouse.click(frameBox.x + 10, frameBox.y + 10);
  const frame = page.frames().find(item => item !== page.mainFrame());
  assert.equal(await frame.evaluate(() => window.frameActions || 0), 0, 'Picker must catch clicks over embedded frames');
  await click('Cancel'); await frameButton.click();
  assert.equal(await frame.evaluate(() => window.frameActions), 1, 'Cleanup must restore ordinary frame interaction');
  await start(); await pick('.tile');
  await page.locator('.tile').first().evaluate(el => el.remove());
  await click('Preview');
  assert.ok(JSON.stringify(await cdp.send('DOM.getDocument', { depth: -1, pierce: true })).includes('The page changed. Pick the element again.'), 'Changed-page error must remain visible');
  await click('Cancel');
  await mutate({ type: 'config.enabled', enabled: false }); await visible('#ad', true);
  await mutate({ type: 'config.enabled', enabled: true }); await visible('#ad', false);
  console.log('PASS: trusted selection, capture-handler and iframe interception, keyboard preview, link suppression, preview/cancel, save, scoped reload, undo preserving later edits, existing filters/settings, multiple matches, parent selection, failed-save retention, site/global controls. Real Rust compiler; browser native delivery bridged locally.');
} finally {
  await context?.close(); await new Promise(resolve => server.close(resolve));
  const target = path.resolve(profile);
  if (path.dirname(target) !== path.resolve(results) || !path.basename(target).startsWith('picker-profile-')) throw new Error('Unexpected fixture cleanup target');
  await rm(target, { recursive: true, force: true });
}
