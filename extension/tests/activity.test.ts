import test from 'node:test';
import assert from 'node:assert/strict';
import { ActivityLog, blockedCountText, privateRequestURL, supportsBlockedCount, type ActivityMatch } from '../src/background/activity';
import { ACTIVITY_LIMIT, type ActivityAccess, type ActivityRuleInfo } from '../src/shared/activity';
import { createRouter } from '../src/background/routes';
import type { Controller } from '../src/background/controller';

const rule: ActivityRuleInfo = { action: 'block', source: 'Local list: Test', condition: 'Compiled rule: /adimage.' };
const match = (id: number, tabId = 7): ActivityMatch => ({ rule: { ruleId: 1, rulesetId: '_dynamic' }, request: { tabId, requestId: String(id), type: 'image', url: 'https://user:secret@ads.test/banner.svg?token=private#private' } });
function setup(saved?: unknown) {
  let state = saved;
  const log = new ActivityLog({ read: async () => state, write: async value => { state = structuredClone(value); } }, () => 1234);
  return { log, stored: () => state as any };
}

test('activity redacts request secrets before memory and storage, retaining bounded metadata', async () => {
  const { log, stored } = setup();
  await log.ready;
  log.record(match(1), { ...rule, source: 'x'.repeat(400), condition: 'c'.repeat(2000) }, 'page.test');
  await log.flush();
  const [entry] = await log.view(7);
  assert.equal(entry.request, 'https://ads.test/banner.svg');
  assert.equal(entry.site, 'page.test');
  assert.equal(entry.source.length, 200);
  assert.equal(entry.condition.length, 1024);
  assert.doesNotMatch(JSON.stringify(stored()), /secret|token|private|user/);
  assert.equal(privateRequestURL('file:///private.txt'), null);
  assert.equal(privateRequestURL('not a URL'), null);
  assert.ok(privateRequestURL('https://a.test/' + 'x'.repeat(4000))!.length <= 320);
});

test('activity caps pre-initialization buffer and retained log; duplicate events do not add rows', async () => {
  let release!: (value: unknown) => void;
  const log = new ActivityLog({ read: () => new Promise(resolve => { release = resolve; }), write: async () => {} });
  for (let i = 0; i < ACTIVITY_LIMIT + 30; ++i) log.record(match(i), rule);
  release(undefined);
  await log.ready;
  log.record(match(ACTIVITY_LIMIT + 29), rule);
  assert.equal((await log.view(null)).length, ACTIVITY_LIMIT);
  const newest = (await log.view(null))[0];
  assert.equal(newest.id, ACTIVITY_LIMIT);
  for (let i = 400; i < 800; ++i) log.record(match(i), rule);
  assert.equal((await log.view(null)).length, ACTIVITY_LIMIT);
  await log.flush();
});

test('activity captures provenance at observation, marks transition unknown, and separates tab history', async () => {
  const { log } = setup();
  await log.ready;
  const info = { ...rule };
  log.record(match(1), info);
  info.source = 'New list';
  log.record(match(2, 8), null);
  log.record(match(3, 8), { ...rule, action: 'allow' });
  assert.equal((await log.view(7))[0].source, rule.source);
  assert.deepEqual((await log.view(8)).map(entry => entry.action), ['allow', 'unknown']);
  const copy = await log.view(7); copy[0].request = 'tampered';
  assert.notEqual((await log.view(7))[0].request, 'tampered');
  await log.removeTab(8);
  assert.equal((await log.view(null)).length, 1);
  await log.retainTabs(new Set());
  assert.equal((await log.view(null)).length, 0);
  await log.flush();
});

test('session restoration validates, redacts, bounds entries and continues identifiers', async () => {
  const entry = { id: 4, tabId: 7, ruleId: 1, timestamp: 100, action: 'block', request: 'https://ads.test/a?private=1', resourceType: 'image', source: 'Local', condition: 'rule' };
  const { log, stored } = setup({ version: 1, entries: [null, { ...entry, tabId: -1 }, entry] });
  await log.ready;
  assert.equal((await log.view(null)).length, 1);
  assert.equal((await log.view(null))[0].request, 'https://ads.test/a');
  log.record(match(2), rule);
  await log.flush();
  assert.equal((await log.view(null))[0].id, 5);
  const restored = setup(stored()).log;
  assert.equal((await restored.view(7)).length, 2);
});

test('clear wins over an in-flight session write and does not replay delayed duplicates', async () => {
  let state: unknown;
  let release!: () => void;
  let started!: () => void;
  let writes = 0;
  const began = new Promise<void>(resolve => { started = resolve; });
  const log = new ActivityLog({ read: async () => undefined, write: async value => {
    if (++writes === 1) { started(); await new Promise<void>(resolve => { release = resolve; }); }
    state = structuredClone(value);
  } });
  await log.ready;
  log.record(match(1), rule);
  const saving = log.flush();
  await began;
  const clearing = log.clear();
  release();
  await Promise.all([saving, clearing]);
  assert.deepEqual((state as any).entries, []);
  log.record(match(1), rule);
  assert.deepEqual(await log.view(null), []);
});

test('session failures leave a bounded memory view and report failed clearing', async () => {
  const log = new ActivityLog({ read: async () => { throw new Error('unavailable'); }, write: async () => { throw new Error('quota'); } });
  await log.ready;
  log.record(match(1), rule);
  await log.flush();
  assert.equal(log.storageError, true);
  assert.equal((await log.view(null)).length, 1);
  await assert.rejects(log.clear(), /session storage could not be cleared/);
  assert.deepEqual(await log.view(null), []);
});

test('page counter rejects placeholders and non-blocking action types that would inflate it', () => {
  assert.equal(blockedCountText(''), '0');
  assert.equal(blockedCountText('28'), '28');
  assert.equal(blockedCountText('1000+'), '1000+');
  assert.equal(blockedCountText('?'), null);
  assert.equal(blockedCountText('BLOCK'), null);
  assert.equal(supportsBlockedCount(['block', 'allow', 'allowAllRequests'].map(type => ({ action: { type } }))), true);
  for (const type of ['redirect', 'modifyHeaders', 'upgradeScheme']) assert.equal(supportsBlockedCount([{ action: { type } }]), false);
});

test('activity ignores non-dynamic, non-page and invalid request events', async () => {
  const { log } = setup();
  await log.ready;
  log.record({ ...match(1), rule: { ruleId: 1, rulesetId: '_session' } }, rule);
  log.record(match(2, -1), rule);
  log.record({ ...match(3), rule: { ruleId: NaN, rulesetId: '_dynamic' } }, rule);
  log.record({ ...match(4), request: { ...match(4).request, url: 'data:text/plain,test' } }, rule);
  assert.deepEqual(await log.view(null), []);
});

test('only trusted packaged interfaces may read or clear activity, with bounded numeric tab IDs', async () => {
  const seen: unknown[] = [];
  const access: ActivityAccess = {
    async get(tabId) { seen.push(tabId); return { available: true, entries: [], tabId, site: null, blockedCount: null, limit: ACTIVITY_LIMIT }; },
    async clear() { seen.push('clear'); }
  };
  const route = createRouter({} as Controller, 'test', 'chrome-extension://test/', access);
  const viewer = { id: 'test', url: 'chrome-extension://test/activity.html#tab=7', tab: { id: 10 }, frameId: 0 };
  await route({ type: 'activity.get', tabId: 7 }, viewer);
  await route({ type: 'activity.get', tabId: null }, viewer);
  await route({ type: 'activity.clear' }, viewer);
  assert.deepEqual(seen, [7, null, 'clear']);
  for (const tabId of [-1, 1.5, '7', undefined, Infinity, 2147483648]) await assert.rejects(route({ type: 'activity.get', tabId }, viewer), /Invalid activity tab/);
  for (const sender of [
    { ...viewer, url: 'https://example.test/activity.html' },
    { ...viewer, url: 'chrome-extension://other/activity.html' },
    { ...viewer, url: 'chrome-extension://test/activity.html?untrusted=1' },
    { ...viewer, frameId: 1 },
    { ...viewer, id: 'other' }
  ]) {
    await assert.rejects(route({ type: 'activity.get', tabId: 7 }, sender));
    await assert.rejects(route({ type: 'activity.clear' }, sender));
  }
});
