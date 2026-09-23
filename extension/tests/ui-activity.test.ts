import { test } from 'node:test';
import assert from 'node:assert/strict';
import { setImmediate } from 'node:timers/promises';
import { mountActivityPage } from '../src/activity/index';
import type { ActivityEntry, ActivityView } from '../src/shared/activity';

class TextElement {
  textContent = '';
  className = '';
  dateTime = '';
  disabled = false;
  hidden = false;
  value = '';
  children: TextElement[] = [];
  readonly attributes = new Map<string, string>();
  private readonly listeners = new Map<string, (() => void)[]>();
  constructor(readonly tagName = 'div') {}
  set innerHTML(_value: string) { throw new Error('Activity data must never be rendered as HTML'); }
  addEventListener(type: string, callback: () => void): void { this.listeners.set(type, [...this.listeners.get(type) ?? [], callback]); }
  dispatch(type: string): void { for (const callback of this.listeners.get(type) ?? []) callback(); }
  setAttribute(key: string, value: string): void { this.attributes.set(key, value); }
  append(...children: TextElement[]): void { for (const child of children) this.children.push(...(child.tagName === '#fragment' ? child.children : [child])); }
  replaceChildren(...children: TextElement[]): void { this.children = []; this.append(...children); }
  descendants(): TextElement[] { return [this, ...this.children.flatMap(child => child.descendants())]; }
}

const view = (changes: Partial<ActivityView> = {}): ActivityView => ({ available: true, limit: 300, entries: [], tabId: 42, site: 'example.test', blockedCount: '7', ...changes });
const entry = (changes: Partial<ActivityEntry> = {}): ActivityEntry => ({ id: 1, timestamp: 1_700_000_000_000, tabId: 42, site: 'example.test', request: 'https://ads.test/banner', resourceType: 'image', ruleId: 12, action: 'block', source: 'Local import', condition: 'urlFilter: ||ads.test^', ...changes });
function deferred<T>(): { promise: Promise<T>; resolve: (value: T) => void } {
  let resolve!: (value: T) => void;
  return { promise: new Promise<T>(done => { resolve = done; }), resolve: value => resolve(value) };
}

async function withPage(send: (message: any) => Promise<unknown>, run: (page: { element(id: string): TextElement; window: TextElement; location: { hash: string } }) => Promise<void>): Promise<void> {
  const names = ['document', 'window', 'location', 'chrome'] as const;
  const saved = names.map(name => [name, Object.getOwnPropertyDescriptor(globalThis, name)] as const);
  const elements = new Map<string, TextElement>();
  const element = (id: string): TextElement => { if (!elements.has(id)) elements.set(id, new TextElement()); return elements.get(id)!; };
  const window = new TextElement();
  const location = { hash: '#tab=42' };
  Object.assign(globalThis, {
    document: { getElementById: element, createElement: (tag: string) => new TextElement(tag), createDocumentFragment: () => new TextElement('#fragment') },
    window, location,
    chrome: { runtime: { sendMessage: send, openOptionsPage: async () => {} } },
  });
  try { mountActivityPage(); await setImmediate(); await run({ element, window, location }); }
  finally { for (const [name, descriptor] of saved) { if (descriptor) Object.defineProperty(globalThis, name, descriptor); else Reflect.deleteProperty(globalThis, name); } }
}

test('activity distinguishes an unavailable page counter from a literal zero', async () => {
  let count: string | null = null;
  await withPage(async () => ({ ok: true, payload: view({ blockedCount: count }) }), async page => {
    assert.equal(page.element('page-count').textContent, 'Unavailable');
    count = '0'; page.element('refresh').dispatch('click'); await setImmediate();
    assert.equal(page.element('page-count').textContent, '0');
    count = '1000+'; page.element('refresh').dispatch('click'); await setImmediate();
    assert.equal(page.element('page-count').textContent, '1000+');
  });
});

test('activity treats hostile request, site, source, and condition values as text and labels exceptions honestly', async () => {
  const hostile = '<img src=x onerror="globalThis.compromised=true">';
  const records = [entry({ request: `https://ads.test/${hostile}`, site: hostile, source: hostile, condition: hostile }), entry({ id: 2, site: null, action: 'allow' }), entry({ id: 3, action: 'allowAllRequests' }), entry({ id: 4, action: 'unknown' })];
  await withPage(async () => ({ ok: true, payload: view({ entries: records }) }), async page => {
    const rows = page.element('activity-rows').children;
    assert.equal(rows.length, 4);
    const rendered = rows[0].descendants();
    assert.equal(rendered.find(node => node.className === 'request-url')?.textContent, `https://ads.test/${hostile}`);
    assert.equal(rendered.find(node => node.className === 'rule-source')?.textContent, hostile);
    assert.equal(rendered.find(node => node.className === 'rule-condition')?.textContent, hostile);
    assert.equal(rows[0].children[1].children[1].textContent, `image · Tab site: ${hostile} · Tab 42`);
    assert.equal(rows[1].children[1].children[1].textContent, 'image · Site unavailable · Tab 42');
    assert.equal(rows[2].children[1].children[1].textContent, 'image · Tab site: example.test · Tab 42');
    assert.ok(rendered.every(node => node.tagName !== 'img'));
    assert.deepEqual(rows.map(row => row.children[2].children[0].textContent), ['Blocked', 'Exception matched', 'Exception matched', 'Unknown match']);
  });
});

test('clear requests every tab, refreshes the log, and preserves the browser page counter', async () => {
  const messages: unknown[] = [];
  let records = [entry()];
  await withPage(async message => {
    messages.push(message);
    if (message.type === 'activity.clear') { records = []; return { ok: true, payload: null }; }
    return { ok: true, payload: view({ entries: records, blockedCount: '7' }) };
  }, async page => {
    assert.equal(page.element('activity-rows').children.length, 1);
    page.element('clear').dispatch('click'); await setImmediate();
    assert.deepEqual(messages, [{ type: 'activity.get', tabId: 42 }, { type: 'activity.clear' }, { type: 'activity.get', tabId: 42 }]);
    assert.equal(page.element('activity-rows').children.length, 0);
    assert.equal(page.element('page-count').textContent, '7');
    assert.equal(page.element('activity-empty').hidden, false);
  });
});

test('an older unavailable response cannot replace a newer tab view or page counter', async () => {
  const older = deferred<unknown>();
  let calls = 0;
  await withPage(async message => {
    if (++calls === 1) return older.promise;
    assert.equal(message.tabId, 84);
    return { ok: true, payload: view({ tabId: 84, site: 'new.example.test', blockedCount: '9', entries: [entry({ tabId: 84 })] }) };
  }, async page => {
    page.location.hash = '#tab=84'; page.window.dispatch('hashchange'); await setImmediate();
    assert.equal(page.element('page-count').textContent, '9');
    older.resolve({ ok: true, payload: view({ available: false, blockedCount: null }) }); await setImmediate();
    assert.equal(page.element('page-count').textContent, '9');
    assert.equal(page.element('activity-site').textContent, 'new.example.test');
    assert.equal(page.element('activity-rows').children.length, 1);
    assert.equal(page.element('log-unavailable').hidden, true);
  });
});
