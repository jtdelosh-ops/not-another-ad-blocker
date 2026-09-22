import { test } from 'node:test';
import assert from 'node:assert/strict';
import { Controller, desiredRules, type Backend, type BrowserRule } from '../src/background/controller';
import { createRouter } from '../src/background/routes';
import { NativeClient } from '../src/shared/native-client';
import { emptyConfig, OVERRIDE_ID, RESOURCE_TYPES, SUBSCRIPTION_URLS, type Compilation } from '../src/shared/types';
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
  assert.equal(controller.describeActivityRule(1)?.source, 'Local list: Test');
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
  assert.equal(controller.describeActivityRule(1)?.source, 'Local list: Test');
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
  assert.equal(controller.describeActivityRule(1), null);
  await assert.rejects(controller.snapshot(), /Protection state is unknown/);
  await assert.rejects(controller.setEnabled(true), /reload the extension/);
  backend.failRuleUpdate = false;
  backend.write = write;
  const recovered = setup(backend).controller;
  assert.equal((await recovered.snapshot()).enabled, true);
  assert.equal(backend.active.length, 1);
  assert.equal(recovered.describeActivityRule(1)?.action, 'block');
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

function deferred() {
  let resolve!: () => void;
  const promise = new Promise<void>(done => { resolve = done; });
  return { promise, resolve };
}
test('activity provenance follows installed IDs and preserves allow actions without claiming exact subscription lines', async () => {
  const backend = new FakeBackend();
  const subscriptions = subscriptionFixture();
  subscriptions.manifest.lists.push({ ...subscriptions.manifest.lists[0], id: 'easyprivacy', title: 'EasyPrivacy', url: SUBSCRIPTION_URLS.easyprivacy });
  subscriptions.manifest.listStats.push({ ...subscriptions.manifest.listStats[0], id: 'easyprivacy' });
  subscriptions.compiled.networkRules.push(networkRule(51, 'allow'), { id: 80, priority: 2, action: { type: 'allowAllRequests' }, condition: { urlFilter: '||exempt.test^', resourceTypes: ['main_frame', 'sub_frame'] } });
  subscriptions.compiled.stats.network = subscriptions.manifest.stats.network = subscriptions.manifest.counts.network = subscriptions.manifest.coverage.networkSupported = 3;
  const local = compiled(); local.networkRules.push(networkRule(99, 'allow')); local.stats.network = 2;
  backend.stored.config = { ...emptyConfig(), compiled: local, source: 'My rules', subscriptions, disabledSites: ['paused.test'] };
  const { controller } = setup(backend);
  await controller.snapshot();
  assert.deepEqual([1, 2, 3, 4, 5, OVERRIDE_ID].map(id => controller.describeActivityRule(id)?.action), ['block', 'allow', 'allowAllRequests', 'block', 'allow', 'allowAllRequests']);
  assert.equal(controller.describeActivityRule(1)?.source, 'EasyList + EasyPrivacy subscriptions');
  assert.equal(controller.describeActivityRule(4)?.source, 'Local list: My rules');
  assert.match(controller.describeActivityRule(4)!.condition, /Compiled rule \(priority 3\)/);
  assert.match(controller.describeActivityRule(5)!.condition, /Compiled rule \(priority 4\)/);
  assert.equal(controller.describeActivityRule(OVERRIDE_ID)?.source, 'Site exception');
  assert.match(controller.describeActivityRule(OVERRIDE_ID)!.condition, /paused\.test/);
  for (const invalid of [0, -1, 1.5, Number.NaN, Number.MAX_SAFE_INTEGER + 1, 6, 7, 51, OVERRIDE_ID + 1]) assert.equal(controller.describeActivityRule(invalid), null);
  const captured = controller.describeActivityRule(4)!; captured.source = 'Tampered';
  assert.equal(controller.describeActivityRule(4)?.source, 'Local list: My rules');
  await controller.removeSubscriptions();
  assert.equal(controller.describeActivityRule(1)?.source, 'Local list: My rules');
  assert.equal(controller.describeActivityRule(4), null, 'Reassigned IDs cannot retain old descriptions');
  await controller.setEnabled(false);
  assert.equal(controller.describeActivityRule(1), null);
  assert.equal(controller.describeActivityRule(OVERRIDE_ID), null);
});
test('activity descriptions sample packed domains and cap source and condition text', async () => {
  const backend = new FakeBackend();
  const subscriptions = subscriptionFixture();
  subscriptions.manifest.lists[0].title = 'Long title '.repeat(23);
  subscriptions.compiled.networkRules[0].condition = { requestDomains: Array.from({ length: 1000 }, (_, i) => `ad${i}.example.test`), resourceTypes: [...RESOURCE_TYPES] };
  backend.stored.config = { ...emptyConfig(), subscriptions };
  const { controller } = setup(backend); await controller.snapshot();
  const description = controller.describeActivityRule(1)!;
  assert.ok(description.source.length <= 200);
  assert.ok(description.condition.length <= 1024);
  assert.match(description.condition, /"total":1000/);
  assert.match(description.condition, /ad0\.example\.test/);
  assert.doesNotMatch(description.condition, /ad999/);
});
test('activity provenance is unavailable until startup succeeds, including startup failure', async () => {
  const backend = new FakeBackend(); backend.stored.config = { ...emptyConfig(), compiled: compiled() };
  const gate = deferred(); const read = backend.read.bind(backend);
  backend.read = async () => { await gate.promise; return read(); };
  const { controller } = setup(backend);
  assert.equal(controller.describeActivityRule(1), null);
  gate.resolve(); await controller.snapshot();
  assert.equal(controller.describeActivityRule(1)?.action, 'block');
  const failed = new FakeBackend(); failed.failRuleUpdate = true;
  failed.stored.config = backend.stored.config;
  const broken = setup(failed).controller;
  await assert.rejects(broken.snapshot(), /DNR rejected/);
  assert.equal(broken.describeActivityRule(1), null);
});
test('activity provenance is unavailable throughout deferred browser replacement and config persistence', async () => {
  for (const phase of ['replace', 'write'] as const) {
    const { controller, backend } = setup(); await controller.import('old', 'Original');
    const entered = deferred(); const release = deferred();
    if (phase === 'replace') {
      const replace = backend.replace.bind(backend);
      backend.replace = async (remove, add) => { entered.resolve(); await release.promise; await replace(remove, add); };
    } else {
      const write = backend.write.bind(backend);
      backend.write = async values => { if ('config' in values) { entered.resolve(); await release.promise; } await write(values); };
    }
    const update = controller.import('new', 'Replacement');
    await entered.promise;
    assert.equal(controller.describeActivityRule(1), null, phase);
    release.resolve(); await update;
    assert.equal(controller.describeActivityRule(1)?.source, 'Local list: Replacement');
  }
});
test('activity provenance remains unavailable during rollback and returns only after restoration', async () => {
  const { controller, backend } = setup(); await controller.import('old', 'Original');
  const entered = deferred(); const release = deferred();
  const replace = backend.replace.bind(backend);
  backend.replace = async (remove, add) => {
    if (add.length) { entered.resolve(); await release.promise; }
    await replace(remove, add);
  };
  backend.failConfigWrite = true;
  const update = controller.setEnabled(false);
  const rejected = assert.rejects(update, /previous rules restored/);
  await entered.promise;
  assert.equal(controller.describeActivityRule(1), null);
  release.resolve(); await rejected;
  assert.equal(controller.describeActivityRule(1)?.source, 'Local list: Original');
});
test('a failed pending write keeps previous activity provenance available', async () => {
  const { controller, backend } = setup(); await controller.import('old', 'Original');
  backend.write = async () => { throw new Error('Pending write failed'); };
  await assert.rejects(controller.setEnabled(false), /Pending write failed/);
  assert.equal(controller.describeActivityRule(1)?.source, 'Local list: Original');
});
