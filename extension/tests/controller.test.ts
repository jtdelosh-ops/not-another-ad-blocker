import { test } from 'node:test';
import assert from 'node:assert/strict';
import { Controller, desiredRules, type Backend, type BrowserRule } from '../src/background/controller';
import { createRouter } from '../src/background/routes';
import { NativeClient } from '../src/shared/native-client';
import { emptyConfig, OVERRIDE_ID, RESOURCE_TYPES, type Compilation } from '../src/shared/types';
import { fixtureClient, subscriptionFixture, networkRule } from './subscription-fixtures';

const compiled = (): Compilation => ({ networkRules: [{ id: 7, priority: 1, action: { type: 'block' }, condition: { urlFilter: '||ads.example.test^', resourceTypes: [...RESOURCE_TYPES] } }], cosmeticRules: [{ domains: ['example.test'], selector: '.advertisement', raw: 'example.test##.advertisement', source: 'Test' }], diagnostics: [{ line: 1, raw: '||ads.example.test^', target: 'MV3_NETWORK', message: 'Compiled successfully' }], stats: { network: 1, cosmetic: 1, unsupported: 0, ignored: 0 } });
class FakeBackend implements Backend {
  stored: Record<string, unknown> = {};
  active: BrowserRule[] = [];
  updates = 0;
  notifications = 0;
  failRuleUpdate = false;
  failConfigWrite = false;
  maxDynamicRules = 30_000;
  async read() { return structuredClone(this.stored); }
  async write(values: Record<string, unknown>) {
    if (this.failConfigWrite && 'config' in values) throw new Error('Storage full');
    Object.assign(this.stored, structuredClone(values));
  }
  async remove(key: string) { delete this.stored[key]; }
  async rules() { return structuredClone(this.active); }
  async replace(removeRuleIds: number[], addRules: BrowserRule[]) {
    this.updates++;
    if (this.failRuleUpdate) throw new Error('DNR rejected update');
    this.active = [...this.active.filter(rule => !removeRuleIds.includes(rule.id)), ...structuredClone(addRules)];
  }
  async notify() { this.notifications++; }
}
function setup(backend = new FakeBackend()) {
  const client = new NativeClient(async (_host, request) => {
    const message = request as any;
    return { version: 1, id: message.id, ok: true, payload: compiled() };
  });
  return { backend, controller: new Controller(backend, client) };
}
test('import atomically replaces rules and persists successful compilation', async () => {
  const { controller, backend } = setup();
  const result = await controller.import('||ads.example.test^', 'Test');
  assert.equal(result.compiled.stats.network, 1);
  assert.deepEqual(backend.active, desiredRules(result));
  assert.deepEqual((backend.stored.config as any).compiled, compiled());
  assert.equal(backend.stored.pending, undefined);
  assert.equal(backend.notifications, 1);
});
test('browser rejection leaves saved and active rules intact', async () => {
  const { controller, backend } = setup();
  await controller.import('old', 'Test');
  const old = await controller.snapshot();
  backend.failRuleUpdate = true;
  await assert.rejects(controller.import('new', 'Other'), /DNR rejected/);
  assert.deepEqual(await controller.snapshot(), old);
  assert.deepEqual(backend.active, desiredRules(old));
  assert.equal(backend.stored.pending, undefined);
});
test('storage failure rolls DNR back to previous rules', async () => {
  const { controller, backend } = setup();
  await controller.import('old', 'Test');
  const old = await controller.snapshot();
  backend.failConfigWrite = true;
  await assert.rejects(controller.setEnabled(false), /previous rules restored/);
  assert.deepEqual(backend.active, desiredRules(old));
  assert.deepEqual(await controller.snapshot(), old);
  assert.equal(backend.stored.pending, undefined);
});
test('worker restart discards interrupted pending update and restores committed rules', async () => {
  const backend = new FakeBackend();
  const old = { ...emptyConfig(), compiled: compiled() };
  backend.stored = { config: old, pending: { ...old, enabled: false } };
  backend.active = [];
  const { controller } = setup(backend);
  assert.deepEqual(await controller.snapshot(), old);
  assert.deepEqual(backend.active, desiredRules(old));
  assert.equal(backend.stored.pending, undefined);
});
test('failed rollback stops reporting a protection state until extension restart recovers it', async () => {
  const { controller, backend } = setup();
  await controller.import('old', 'Test');
  const write = backend.write.bind(backend);
  backend.write = async values => {
    if ('config' in values) { backend.failRuleUpdate = true; throw new Error('Storage failure'); }
    await write(values);
  };
  await assert.rejects(controller.setEnabled(false), /Protection state is unknown/);
  await assert.rejects(controller.snapshot(), /Protection state is unknown/);
  await assert.rejects(controller.setEnabled(true), /reload the extension/);
  backend.failRuleUpdate = false;
  backend.write = write;
  const recovered = setup(backend).controller;
  assert.equal((await recovered.snapshot()).enabled, true);
  assert.equal(backend.active.length, 1);
});
test('global off removes DNR and cosmetics; import while off stays off; enabling restores rules', async () => {
  const { controller, backend } = setup();
  await controller.setEnabled(false);
  await controller.import('test', 'Test');
  assert.deepEqual(backend.active, []);
  assert.deepEqual(await controller.cosmetics('example.test'), { selectors: [] });
  await controller.setEnabled(true);
  assert.equal(backend.active.length, 1);
  assert.deepEqual(await controller.cosmetics('example.test'), { selectors: ['.advertisement'] });
});
test('site exception overrides block and exception priorities, scopes cosmetics, and persists', async () => {
  const { controller, backend } = setup();
  await controller.import('test', 'Test');
  await controller.setSite('example.test', false);
  const overrides = backend.active.filter(rule => rule.id >= OVERRIDE_ID);
  assert.equal(overrides.length, 1);
  assert.equal(overrides[0].priority, 100);
  assert.equal(overrides[0].action.type, 'allowAllRequests');
  assert.deepEqual(overrides[0].condition.requestDomains, ['example.test']);
  assert.deepEqual(overrides[0].condition.resourceTypes, ['main_frame']);
  assert.equal(overrides[0].condition.initiatorDomains, undefined, 'Site exceptions must not allow paused-site iframe traffic under another top-level site');
  assert.deepEqual(await controller.cosmetics('sub.example.test'), { selectors: [] });
  assert.deepEqual((backend.stored.config as any).disabledSites, ['example.test']);
  await controller.setSite('example.test', true);
  assert.equal(backend.active.length, 1);
  assert.deepEqual(await controller.cosmetics('sub.example.test'), { selectors: ['.advertisement'] });
  assert.deepEqual(await controller.cosmetics('notexample.test'), { selectors: [] });
});
test('limit failure keeps active and stored configuration unchanged', async () => {
  const { controller, backend } = setup();
  await controller.import('test', 'Test');
  backend.maxDynamicRules = 1;
  await assert.rejects(controller.setSite('example.test', false), /browser allows 1/);
  assert.equal(backend.active.length, 1);
  assert.deepEqual((await controller.snapshot()).disabledSites, []);
});
test('concurrent mutations are serialized without losing settings', async () => {
  const { controller } = setup();
  await Promise.all([controller.import('test', 'Test'), controller.setSite('example.test', false), controller.setEnabled(false)]);
  const saved = await controller.snapshot();
  assert.equal(saved.compiled.stats.network, 1);
  assert.equal(saved.enabled, false);
  assert.deepEqual(saved.disabledSites, ['example.test']);
});
test('web content cannot call privileged routes or request cosmetics for another URL', async () => {
  const { controller } = setup();
  await controller.import('test', 'Test');
  const route = createRouter(controller, 'extension-id', 'chrome-extension://extension-id/');
  const web = { id: 'extension-id', url: 'https://other.test/', tab: { id: 1 }, frameId: 0 };
  await assert.rejects(route({ type: 'config.enabled', enabled: false }, web), /extension interface/);
  await assert.rejects(route({ type: 'config.get' }, web), /extension interface/);
  await assert.rejects(route({ type: 'status.get' }, web), /extension interface/);
  assert.deepEqual(await route({ type: 'cosmetics.get', host: 'example.test' }, web), { selectors: [] });
  await assert.rejects(route({ type: 'cosmetics.get' }, { ...web, frameId: 1 }), /top frame/);
  await assert.rejects(route({ type: 'config.get' }, { id: 'other', url: 'chrome-extension://extension-id/options.html' }), /Unauthorized/);
  const popup = { id: 'extension-id', url: 'chrome-extension://extension-id/popup.html' };
  assert.equal((await route({ type: 'config.get' }, popup) as any).enabled, true);
  const options = { id: 'extension-id', url: 'chrome-extension://extension-id/options.html', tab: { id: 2 }, frameId: 0 };
  assert.equal((await route({ type: 'config.get' }, options) as any).enabled, true);
});

test('subscriptions preserve a legacy local config and settings, reserve capacity, and merge IDs safely', async () => {
  const backend = new FakeBackend();
  const local = { ...emptyConfig(), enabled: false, disabledSites: ['paused.test'], source: 'Original', text: 'original rules', compiled: compiled(), updatedAt: '2026-09-21T12:00:00Z' };
  backend.stored.config = local;
  const requests: any[] = [];
  const controller = new Controller(backend, fixtureClient(undefined, value => value, request => requests.push(request)));
  assert.deepEqual(await controller.snapshot(), local);
  await controller.refreshSubscriptions(['easylist']);
  const saved = await controller.snapshot();
  const { subscriptions, ...rest } = saved;
  assert.deepEqual(rest, local);
  assert.equal(subscriptions?.compiled.stats.network, 1);
  assert.deepEqual(backend.active, []);
  assert.equal(requests.find(request => request.type === 'lists.refresh').payload.networkBudget, 27_800);
  await controller.setEnabled(true);
  const rules = await backend.rules();
  assert.equal(new Set(rules.map(rule => rule.id)).size, rules.length);
  assert.deepEqual(rules.slice(0, 2).map(rule => rule.priority), [1, 3]);
  assert.equal(rules[2].priority, 100);
  assert.deepEqual(await controller.cosmetics('example.test'), { selectors: ['.advertisement', '.subscription-ad'] });
  assert.deepEqual(await controller.cosmetics('sub.excluded.test'), { selectors: [] });
  assert.deepEqual(await controller.cosmetics('sub.paused.test'), { selectors: [] });
  const view = await controller.view();
  assert.equal((view.compiled as any).networkRules, undefined);
  assert.equal((view.subscriptions as any).compiled, undefined);
  assert.equal(view.subscriptions?.diagnostics[0].line, 17);
});
test('local import preserves subscriptions; removing subscriptions preserves local import and settings', async () => {
  const backend = new FakeBackend();
  const controller = new Controller(backend, fixtureClient());
  await controller.refreshSubscriptions(['easylist']);
  await controller.setSite('paused.test', false);
  const before = (await controller.snapshot()).subscriptions;
  await controller.import('replacement local list', 'New local');
  assert.deepEqual((await controller.snapshot()).subscriptions, before);
  const { subscriptions: _, ...expected } = await controller.snapshot();
  await controller.removeSubscriptions();
  assert.deepEqual(await controller.snapshot(), expected);
  assert.deepEqual(backend.active, desiredRules(expected));
});
test('download and paging failures preserve active and committed rules without a pending journal', async () => {
  for (const failAt of ['lists.refresh', 'lists.page']) {
    const backend = new FakeBackend();
    const old = { ...emptyConfig(), compiled: compiled() }; backend.stored.config = old;
    const client = fixtureClient(undefined, (payload, message) => { if (message.type === failAt) throw new Error(`Failed ${failAt}`); return payload; });
    const controller = new Controller(backend, client);
    await assert.rejects(controller.refreshSubscriptions(['easylist']), /Failed lists/);
    assert.deepEqual(await controller.snapshot(), old);
    assert.deepEqual(backend.active, desiredRules(old));
    assert.equal(backend.stored.pending, undefined);
    assert.equal(controller.subscriptionProgress().phase, 'failed');
  }
});
test('subscription commit failures retain previous subscriptions through browser and storage rollback', async () => {
  for (const failure of ['browser', 'storage']) {
    const backend = new FakeBackend();
    const controller = new Controller(backend, fixtureClient());
    await controller.refreshSubscriptions(['easylist']);
    await controller.import('original', 'Local');
    const before = await controller.snapshot();
    if (failure === 'browser') backend.failRuleUpdate = true;
    else backend.failConfigWrite = true;
    await assert.rejects(controller.removeSubscriptions(), /failed|restored/);
    assert.deepEqual(await controller.snapshot(), before);
    assert.deepEqual(backend.active, desiredRules(before));
    assert.deepEqual(backend.stored.config, before);
    assert.equal(backend.stored.pending, undefined);
  }
});
test('subscriptions use the reduced browser budget, retain every supplied allow exception and never truncate at merge', async () => {
  const backend = new FakeBackend(); backend.maxDynamicRules = 2202;
  const state = subscriptionFixture();
  state.compiled.networkRules = [networkRule(7), networkRule(8, 'allow')];
  state.compiled.stats.network = state.manifest.stats.network = state.manifest.counts.network = state.manifest.coverage.networkSupported = 2;
  const controller = new Controller(backend, fixtureClient(state, (value, request) => {
    if (request.type === 'lists.refresh') assert.equal(request.payload.networkBudget, 2);
    return value;
  }));
  await controller.refreshSubscriptions(['easylist']);
  assert.equal(backend.active.length, 2);
  assert.equal(backend.active.filter(rule => rule.action.type === 'allow').length, 1);
  const config = await controller.snapshot(); config.compiled = compiled(); config.compiled.networkRules.push(networkRule(8, 'allow')); config.compiled.stats.network = 2;
  const rules = desiredRules(config);
  assert.deepEqual(rules.map(rule => rule.priority), [1, 2, 3, 4]);
  assert.deepEqual(rules.map(rule => rule.id), [1, 2, 3, 4]);
  assert.equal(rules.every(rule => rule.id < OVERRIDE_ID), true);
});
test('in-flight refresh keeps cosmetics available, rejects duplicate refresh, and merges current protection settings', async () => {
  const backend = new FakeBackend(); backend.stored.config = { ...emptyConfig(), compiled: compiled() };
  const client = fixtureClient();
  let finish!: (value: ReturnType<typeof subscriptionFixture>) => void;
  client.refresh = async () => new Promise(resolve => { finish = resolve; });
  const controller = new Controller(backend, client);
  await controller.snapshot();
  const pending = controller.refreshSubscriptions(['easylist']);
  await Promise.resolve();
  assert.deepEqual(await controller.cosmetics('example.test'), { selectors: ['.advertisement'] });
  await assert.rejects(controller.refreshSubscriptions(['easylist']), /already running/);
  await controller.setEnabled(false);
  await controller.setSite('paused.test', false);
  finish(subscriptionFixture());
  await pending;
  assert.equal((await controller.snapshot()).enabled, false);
  assert.deepEqual((await controller.snapshot()).disabledSites, ['paused.test']);
  assert.deepEqual(backend.active, []);
});
test('removing subscriptions cancels a pending refresh before it can reactivate old choices', async () => {
  const backend = new FakeBackend(); const client = fixtureClient();
  let finish!: (value: ReturnType<typeof subscriptionFixture>) => void;
  client.refresh = async () => new Promise(resolve => { finish = resolve; });
  const controller = new Controller(backend, client); await controller.snapshot();
  const pending = controller.refreshSubscriptions(['easylist']); await Promise.resolve();
  await controller.removeSubscriptions(); finish(subscriptionFixture());
  await assert.rejects(pending, /cancelled/);
  assert.equal((await controller.snapshot()).subscriptions, undefined);
  assert.deepEqual(backend.active, []);
});
test('web pages cannot read download progress, initiate subscriptions, remove them or receive network arrays', async () => {
  const backend = new FakeBackend(); const controller = new Controller(backend, fixtureClient());
  await controller.refreshSubscriptions(['easylist']);
  const route = createRouter(controller, 'extension-id', 'chrome-extension://extension-id/');
  const web = { id: 'extension-id', url: 'https://example.test/', tab: { id: 1 }, frameId: 0 };
  for (const message of [{ type: 'subscriptions.progress' }, { type: 'subscriptions.refresh', ids: ['easylist'] }, { type: 'subscriptions.remove' }]) await assert.rejects(route(message, web), /extension interface/);
  assert.deepEqual(Object.keys(await route({ type: 'cosmetics.get' }, web) as object), ['selectors']);
  const view = await route({ type: 'config.get' }, { id: 'extension-id', url: 'chrome-extension://extension-id/options.html', tab: { id: 2 }, frameId: 0 }) as any;
  assert.equal(view.compiled.networkRules, undefined);
  assert.equal(view.subscriptions.compiled, undefined);
});
