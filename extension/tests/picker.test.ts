import test from 'node:test';
import assert from 'node:assert/strict';
import { Picker, type PickerBackend, type PickerGrant } from '../src/background/picker';
import { Controller, type Backend, type BrowserRule } from '../src/background/controller';
import { createRouter } from '../src/background/routes';
import { NativeClient } from '../src/shared/native-client';
import { emptyConfig, type Compilation } from '../src/shared/types';
import { subscriptionFixture } from './subscription-fixtures';

const token = 'aaaaaaaa-aaaa-4aaa-aaaa-aaaaaaaaaaaa';
const sender = { id: 'test', url: 'https://example.test/page', tab: { id: 7 }, frameId: 0 };
function permissionFixture() {
  let now = 100;
  const grants = new Map<number, PickerGrant>();
  const calls: unknown[] = [];
  const config = emptyConfig();
  const controller = {
    snapshot: async () => config,
    addPickerRule: async (...args: unknown[]) => { calls.push(args); return { rule: 'example.test##.ad', added: true }; },
    removePickerRule: async (...args: unknown[]) => { calls.push(['undo', ...args]); }
  } as unknown as Controller;
  const backend: PickerBackend = {
    tab: async () => ({ url: sender.url, active: true }), read: async id => grants.get(id),
    write: async (id, grant) => { grants.set(id, structuredClone(grant)); }, remove: async id => { grants.delete(id); }, start: async () => ({ ok: true })
  };
  const create = () => new Picker(controller, backend, () => now, () => token);
  return { backend, controller, config, calls, grants, create, expire: () => { now += 600_001; } };
}
test('only an extension interface can start a picker; only the authorized tab can save', async () => {
  const f = permissionFixture(); const picker = f.create(); const route = createRouter(f.controller, 'test', 'chrome-extension://test/', undefined, picker);
  await assert.rejects(route({ type: 'picker.start', tabId: 7 }, sender), /extension interface/);
  await assert.rejects(route({ type: 'picker.save', token, selector: '.ad' }, sender), /expired/);
  await route({ type: 'picker.start', tabId: 7 }, { id: 'test', url: 'chrome-extension://test/popup.html' });
  const save = { type: 'picker.save', token, selector: '.ad' };
  for (const invalid of [{ ...sender, frameId: 1 }, { ...sender, tab: { id: 8 } }, { ...sender, url: 'https://other.test' }, { ...sender, tab: { id: 7, incognito: true } }, { ...sender, id: 'other' }]) await assert.rejects(route(save, invalid));
  await assert.rejects(route({ ...save, token: 'forged' }, sender));
  await assert.rejects(route({ ...save, selector: 'body' }, sender));
  await assert.rejects(route({ ...save, selector: '.ad,body' }, sender));
  assert.equal(f.calls.length, 0);
  await route(save, sender); assert.equal(f.calls.length, 1);
});
test('picker grants survive worker recreation, expire, and are revoked on cancel', async () => {
  const f = permissionFixture(); await f.create().start(7);
  await f.create().handle({ type: 'picker.save', token, selector: '.ad' }, sender);
  await f.create().handle({ type: 'picker.undo', token }, sender);
  assert.equal(f.grants.size, 0);
  await f.create().start(7); f.expire();
  await assert.rejects(f.create().handle({ type: 'picker.save', token, selector: '.ad' }, sender), /expired/);
  await f.create().start(7); await f.create().handle({ type: 'picker.cancel', token }, sender);
  await assert.rejects(f.create().handle({ type: 'picker.save', token, selector: '.ad' }, sender), /expired/);
});
test('picker refuses paused/private/browser pages and removes failed start grants', async () => {
  const f = permissionFixture(); f.config.enabled = false;
  await assert.rejects(f.create().start(7), /Enable protection/);
  f.config.enabled = true; f.config.disabledSites = ['example.test'];
  await assert.rejects(f.create().start(7), /Enable protection/);
  f.config.disabledSites = [];
  for (const tab of [{ url: 'chrome://extensions', active: true }, { url: sender.url, active: true, incognito: true }, { url: sender.url, active: false }]) {
    f.backend.tab = async () => tab; await assert.rejects(f.create().start(7), /normal HTTP/);
  }
  f.backend.tab = async () => ({ url: sender.url, active: true });
  f.backend.start = async () => { throw new Error('No receiver'); };
  await assert.rejects(f.create().start(7), /Reload/); assert.equal(f.grants.size, 0);
});
test('failed compilation permits picking a different selector without reopening', async () => {
  const f = permissionFixture(); await f.create().start(7);
  f.controller.addPickerRule = async () => { throw new Error('Offline'); };
  await assert.rejects(f.create().handle({ type: 'picker.save', token, selector: '.ad' }, sender), /Offline/);
  assert.equal(f.grants.get(7)?.selector, undefined);
});

function controllerFixture() {
  let stored: Record<string, unknown> = {}; let active: BrowserRule[] = []; let fail = false;
  const backend: Backend = {
    read: async () => structuredClone(stored), write: async data => { Object.assign(stored, structuredClone(data)); }, remove: async key => { delete stored[key]; },
    rules: async () => active, replace: async (_ids, rules) => { if (fail) throw new Error('Rejected'); active = structuredClone(rules); }, notify: async () => {}, maxDynamicRules: 30_000
  };
  // The compiler seam supplies known supported fixture rules; real Rust/browser
  // integration is exercised separately in tests/picker-browser.mjs.
  const native = new NativeClient(async () => { throw new Error('Unexpected transport'); });
  native.compile = async (text, source = 'Local import') => {
    const cosmeticRules = ['example.test##.keep', 'example.test##.ad', 'example.test##.later'].filter(rule => text.split('\n').includes(rule)).map(raw => ({ raw, domains: ['example.test'], selector: raw.split('##')[1], source }));
    return { networkRules: [], cosmeticRules, diagnostics: [], stats: { network: 0, cosmetic: cosmeticRules.length, ignored: 0, unsupported: 0 } } as Compilation;
  };
  return { controller: new Controller(backend, native), native, backend, fail: () => { fail = true; } };
}
test('picker appends and undo removes only its own rule after another local edit', async () => {
  const f = controllerFixture(); await f.controller.import('example.test##.keep', 'My filters');
  await f.controller.setSite('other.test', false);
  await f.controller.addPickerRule('example.test', '.ad', token);
  const saved = await f.controller.snapshot();
  assert.equal(saved.source, 'My filters'); assert.equal(saved.compiled.stats.cosmetic, 2);
  assert.equal((await f.controller.addPickerRule('example.test', '.ad', token)).added, true);
  await f.controller.import(saved.text + '\nexample.test##.later', saved.source);
  await f.controller.removePickerRule('example.test', '.ad', token);
  const after = await f.controller.snapshot();
  assert.equal(after.text, 'example.test##.keep\nexample.test##.later');
  assert.deepEqual(after.disabledSites, ['other.test']); assert.equal(after.compiled.stats.cosmetic, 2);
});
test('picker duplicates have no undo ownership; edited rules cannot be silently removed', async () => {
  const f = controllerFixture(); await f.controller.import('example.test##.ad', 'Existing');
  assert.equal((await f.controller.addPickerRule('example.test', '.ad', token)).added, false);
  await assert.rejects(f.controller.removePickerRule('example.test', '.ad', token), /edited or removed/);
  assert.equal((await f.controller.snapshot()).text, 'example.test##.ad');
});
test('unsupported compiler result and failed browser update leave existing state intact', async () => {
  const f = controllerFixture(); await f.controller.import('example.test##.keep', 'Keep');
  const before = await f.controller.snapshot();
  f.fail(); await assert.rejects(f.controller.addPickerRule('example.test', '.ad', token), /Rejected/);
  assert.deepEqual(await f.controller.snapshot(), before);
  const g = controllerFixture(); g.native.compile = async () => emptyConfig().compiled;
  await assert.rejects(g.controller.addPickerRule('example.test', '.ad', token), /does not support/);
  assert.equal((await g.controller.snapshot()).text, '');
});
test('picker save/undo preserves subscriptions and refuses saving after protection is paused', async () => {
  const f = controllerFixture(); const subscriptions = subscriptionFixture();
  await f.backend.write({ config: { ...emptyConfig(), subscriptions } });
  const controller = new Controller(f.backend, f.native);
  await controller.addPickerRule('example.test', '.ad', token);
  assert.deepEqual((await controller.snapshot()).subscriptions, subscriptions);
  await controller.removePickerRule('example.test', '.ad', token);
  assert.deepEqual((await controller.snapshot()).subscriptions, subscriptions);
  await controller.setEnabled(false);
  await assert.rejects(controller.addPickerRule('example.test', '.ad', token), /paused/);
  assert.equal((await controller.snapshot()).text, '');
});
