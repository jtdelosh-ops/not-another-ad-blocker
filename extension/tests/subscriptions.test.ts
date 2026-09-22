import { test } from 'node:test';
import assert from 'node:assert/strict';
import { NativeClient, validateCompilation, validateSubscriptionManifest, validateSubscriptionState } from '../src/shared/native-client';
import { networkRule, subscriptionFixture, fixtureClient } from './subscription-fixtures';

test('subscription client validates capability, metadata and paged results with honest progress', async () => {
  const state = subscriptionFixture();
  const requests: any[] = [], progress: any[] = [];
  const result = await fixtureClient(state, value => value, request => requests.push(request)).refresh(['easylist'], 27_800, value => progress.push(value));
  assert.deepEqual(result.compiled, state.compiled);
  assert.deepEqual(requests[1].payload, { ids: ['easylist'], networkBudget: 27_800 });
  assert.deepEqual(requests.slice(2).map(request => request.payload.kind), ['network', 'cosmetic', 'diagnostics']);
  assert.deepEqual(progress.at(-1), { phase: 'diagnostics', completed: 1, total: 1 });
  assert.equal(result.compiled.diagnostics[0].line, 17);
  assert.equal(result.compiled.diagnostics[0].source, 'easylist');
});
test('old companion still supports status/import and explains required subscription update', async () => {
  const client = fixtureClient(undefined, (value, request) => request.type === 'status.get' ? { ...value, capabilities: ['rules.compile'], companionVersion: '0.1.0' } : value);
  assert.equal((await client.status()).healthy, true);
  assert.equal((await client.compile('test')).stats.network, 1);
  await assert.rejects(client.refresh(['easylist'], 100), /Update the local companion to version 0.2.0/);
});
test('subscription requests reject unknown URLs, duplicate IDs, empty selection and bad budgets before transport', async () => {
  let called = false;
  const client = new NativeClient(async () => { called = true; return {}; });
  for (const ids of [[], ['easylist', 'easylist'], ['https://evil.test'], ['easylist', 'easyprivacy', 'easylist']]) await assert.rejects(client.refresh(ids as any, 100), /Select EasyList/);
  for (const budget of [-1, 29_801, 1.5, NaN]) await assert.rejects(client.refresh(['easylist'], budget), /budget/);
  assert.equal(called, false);
});
test('subscription downloads have their own bounded timeout', async () => {
  const client = new NativeClient(async (_host, request) => {
    const message = request as any;
    if (message.type === 'lists.refresh') return new Promise(() => {});
    return { version: 1, id: message.id, ok: true, payload: { companionVersion: '0.2.0', protocolVersion: 1, healthy: true, capabilities: ['rules.compile', 'lists.refresh', 'lists.page'], mode: 'local-import' } };
  }, 1000, 10);
  await assert.rejects(client.refresh(['easylist'], 100), /timed out/);
});
test('multi-page arrays require matching identity, offset, totals, forward progress and terminal offset', async () => {
  const state = subscriptionFixture();
  state.compiled.networkRules = Array.from({ length: 129 }, (_, index) => networkRule(index + 1));
  state.compiled.stats.network = state.manifest.stats.network = state.manifest.counts.network = state.manifest.coverage.networkSupported = 129;
  assert.equal((await fixtureClient(state).refresh(['easylist'], 129)).compiled.networkRules.length, 129);
  for (const mutation of [
    (page: any) => ({ ...page, snapshotId: 'c'.repeat(64) }),
    (page: any) => ({ ...page, kind: 'cosmetic' }),
    (page: any) => ({ ...page, offset: page.offset + 1 }),
    (page: any) => ({ ...page, total: page.total - 1 }),
    (page: any) => ({ ...page, nextOffset: page.offset }),
    (page: any) => ({ ...page, items: [], nextOffset: 0 }),
    (page: any) => ({ ...page, nextOffset: null }),
    (page: any) => ({ ...page, items: state.compiled.networkRules })
  ]) {
    const client = fixtureClient(state, (payload, message) => message.type === 'lists.page' && message.payload.kind === 'network' ? mutation(payload) : payload);
    await assert.rejects(client.refresh(['easylist'], 129), /invalid response/);
  }
  const duplicate = fixtureClient(state, (page, message) => {
    if (message.type === 'lists.page' && message.payload.kind === 'network' && page.offset === 128) page.items[0].id = 1;
    return page;
  });
  await assert.rejects(duplicate.refresh(['easylist'], 129), /invalid response/);
});
test('zero-length kinds use a final empty page; extra items or nonterminal empty pages fail', async () => {
  const state = subscriptionFixture();
  state.compiled.networkRules = []; state.compiled.stats.network = state.manifest.stats.network = state.manifest.counts.network = state.manifest.coverage.networkSupported = 0;
  assert.equal((await fixtureClient(state).refresh(['easylist'], 0)).compiled.networkRules.length, 0);
  const client = fixtureClient(state, (page, message) => message.type === 'lists.page' && page.kind === 'network' ? { ...page, nextOffset: 0 } : page);
  await assert.rejects(client.refresh(['easylist'], 0), /invalid response/);
});
test('individual page serialized byte limit is enforced independently of item count', async () => {
  const state = subscriptionFixture();
  state.compiled.diagnostics = Array.from({ length: 128 }, (_, index) => ({ line: index + 1, raw: 'a'.repeat(2048), target: 'UNSUPPORTED', message: 'b'.repeat(2048), source: 'easylist' }));
  state.manifest.counts.diagnostics = state.manifest.coverage.diagnosticsTotal = 128;
  await assert.rejects(fixtureClient(state).refresh(['easylist'], 100), /invalid response/);
});
test('subscription network conditions accept packed hosts, context, resource types and frame allowances', () => {
  const state = subscriptionFixture();
  state.compiled.networkRules[0].condition = { requestDomains: ['ad.example.test'], resourceTypes: ['script'], domainType: 'firstParty', initiatorDomains: ['news.test'], excludedInitiatorDomains: ['sub.news.test'], isUrlFilterCaseSensitive: false };
  assert.equal(validateCompilation(state.compiled, true).stats.network, 1);
  state.compiled.networkRules[0] = { id: 1, priority: 2, action: { type: 'allowAllRequests' }, condition: { urlFilter: '||allowed.test^', resourceTypes: ['main_frame', 'sub_frame'] } };
  assert.equal(validateCompilation(state.compiled, true).networkRules[0].action.type, 'allowAllRequests');
});
test('subscription schema rejects unknown conditions, main-frame blocking, broad privileges and unsafe cosmetics', () => {
  const mutations: ((state: any) => void)[] = [
    state => state.compiled.networkRules[0].condition.resourceTypes = ['main_frame'],
    state => state.compiled.networkRules[0].condition.requestDomains = ['ad.example.test'],
    state => state.compiled.networkRules[0].condition.initiatorDomains = ['https://news.test'],
    state => state.compiled.networkRules[0].condition.excludedInitiatorDomains = Array(201).fill('news.test'),
    state => state.compiled.networkRules[0].condition.domainType = 'all',
    state => state.compiled.networkRules[0].condition.isUrlFilterCaseSensitive = 'false',
    state => state.compiled.networkRules[0].condition.regexFilter = '.*',
    state => state.compiled.networkRules[0].condition.urlFilter = '||ad.é.test^',
    state => state.compiled.networkRules[0].action.type = 'redirect',
    state => { state.compiled.networkRules[0].action.type = 'allowAllRequests'; state.compiled.networkRules[0].priority = 2; },
    state => state.compiled.cosmeticRules[0].selector = '.ad}body{display:none}',
    state => state.compiled.cosmeticRules[0].excludedDomains = ['UPPER.test'],
    state => state.compiled.diagnostics[0].raw = 'a'.repeat(2049),
    state => state.compiled.diagnostics[0].line = 0
  ];
  for (const mutation of mutations) { const state = subscriptionFixture(); mutation(state); assert.throws(() => validateSubscriptionState(state), /invalid response/); }
});
test('manifest requires fixed source URLs, requested identities, consistent counts and bounded diagnostics', () => {
  for (const mutate of [
    (state: any) => state.manifest.lists[0].url = 'https://evil.test/list.txt',
    (state: any) => state.manifest.counts.network = 2,
    (state: any) => state.manifest.counts.diagnostics = 201,
    (state: any) => state.manifest.coverage.networkDropped = 1,
    (state: any) => state.manifest.coverage.diagnosticsTruncated = true,
    (state: any) => state.manifest.lists[0].sha256 = 'not a hash',
    (state: any) => state.manifest.fetchedAt = 'yesterday'
  ]) { const state = subscriptionFixture(); mutate(state); assert.throws(() => validateSubscriptionManifest(state.manifest, ['easylist'], 100), /invalid response/); }
  assert.throws(() => validateSubscriptionManifest(subscriptionFixture().manifest, ['easyprivacy']), /invalid response/);
  const state = subscriptionFixture(); state.manifest.coverage.cosmeticSupported = 20;
  assert.equal(validateSubscriptionState(state).manifest.coverage.cosmeticSupported, 20, 'deduplicated and exempted cosmetics may exceed emitted rules');
});
