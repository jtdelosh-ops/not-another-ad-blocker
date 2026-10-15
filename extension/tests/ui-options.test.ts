import { test } from 'node:test';
import assert from 'node:assert/strict';
import { setImmediate } from 'node:timers/promises';
import { mountOptionsPage } from '../src/options/index';
import { configView } from '../src/background/controller';
import { emptyConfig } from '../src/shared/types';

class Element {
  textContent = '';
  value = '';
  disabled = false;
  readOnly = false;
  checked = false;
  children: Element[] = [];
  private listeners = new Map<string, (() => void)[]>();
  constructor(readonly id = '', readonly tagName = 'div') {}
  addEventListener(type: string, callback: () => void): void { this.listeners.set(type, [...this.listeners.get(type) ?? [], callback]); }
  dispatch(type: string): void { for (const callback of this.listeners.get(type) ?? []) callback(); }
  append(...children: Element[]): void { this.children.push(...children.flatMap(child => child.tagName === '#fragment' ? child.children : [child])); }
  replaceChildren(...children: Element[]): void { this.children = []; this.append(...children); }
}
async function withPage(send: (message: any) => Promise<unknown>, run: (element: (id: string) => Element) => Promise<void>): Promise<void> {
  const names = ['document', 'chrome'] as const;
  const saved = names.map(name => [name, Object.getOwnPropertyDescriptor(globalThis, name)] as const);
  const elements = new Map<string, Element>();
  const buttons = ['check', 'import', 'refresh-lists', 'remove-lists', 'demo'];
  const inputs = ['source-name', 'global', 'easylist', 'easyprivacy', 'file'];
  const element = (id: string): Element => {
    if (!elements.has(id)) elements.set(id, new Element(id, buttons.includes(id) ? 'button' : inputs.includes(id) ? 'input' : id === 'filters' ? 'textarea' : 'div'));
    return elements.get(id)!;
  };
  [...buttons, ...inputs, 'filters'].forEach(element);
  Object.assign(globalThis, {
    document: {
      getElementById: element,
      createElement: (tag: string) => new Element('', tag),
      createDocumentFragment: () => new Element('', '#fragment'),
      querySelectorAll: (selector: string) => [...elements.values()].filter(value => ['button', 'input', 'textarea'].includes(value.tagName) && (!selector.includes(':not(#check)') || value.id !== 'check')),
    },
    chrome: { runtime: { sendMessage: send, id: 'a'.repeat(32), getManifest: () => ({ version: '0.4.2' }) } },
  });
  try { mountOptionsPage(); await setImmediate(); await run(element); }
  finally { for (const [name, descriptor] of saved) { if (descriptor) Object.defineProperty(globalThis, name, descriptor); else Reflect.deleteProperty(globalThis, name); } }
}
const config = configView(emptyConfig());
const initialReply = (message: any) => message.type === 'config.get'
  ? { ok: true, payload: config }
  : { ok: true, payload: { healthy: true, companionVersion: '0.2.0', capabilities: [] } };

test('options shows compilation progress, prevents duplicate saves, and restores controls after confirmation', async () => {
  let finish!: (value: unknown) => void;
  let imports = 0;
  await withPage(async message => {
    if (message.type !== 'config.import') return initialReply(message);
    imports++;
    return new Promise(resolve => { finish = resolve; });
  }, async element => {
    element('filters').value = 'example.test##.ad';
    element('import').dispatch('click');
    assert.match(element('message').textContent, /Compiling and saving/);
    assert.equal(element('import').disabled, true);
    element('import').dispatch('click');
    assert.equal(imports, 1);
    finish({ ok: true, payload: { ...config, text: 'example.test##.ad' } });
    await setImmediate();
    assert.match(element('message').textContent, /^Saved/);
    assert.equal(element('import').disabled, false);
    assert.equal(element('filters').value, 'example.test##.ad');
  });
});

test('options lost save reply hides stale protection claims and keeps the draft copyable', async () => {
  await withPage(async message => {
    if (message.type !== 'config.import') return initialReply(message);
    throw new Error('message channel closed before a response was received');
  }, async element => {
    element('filters').value = 'example.test##.ad';
    element('import').dispatch('click');
    await setImmediate();
    assert.match(element('summary').textContent, /state is unknown/);
    assert.match(element('error').textContent, /may already be saved/);
    assert.equal(element('message').textContent, '');
    assert.equal(element('import').disabled, true);
    assert.equal(element('global').disabled, true);
    assert.equal(element('check').disabled, false);
    assert.equal(element('filters').value, 'example.test##.ad');
    assert.equal(element('filters').disabled, false);
    assert.equal(element('filters').readOnly, true);
  });
});

test('options stalled save ends in recovery and late confirmation cannot reenable changes', async context => {
  context.mock.timers.enable({ apis: ['setTimeout'] });
  let finish!: (value: unknown) => void;
  try {
    await withPage(async message => {
      if (message.type !== 'config.import') return initialReply(message);
      return new Promise(resolve => { finish = resolve; });
    }, async element => {
      element('filters').value = 'example.test##.ad';
      element('import').dispatch('click');
      context.mock.timers.tick(30_000);
      await setImmediate();
      assert.match(element('summary').textContent, /state is unknown/);
      assert.equal(element('import').disabled, true);
      finish({ ok: true, payload: config });
      await setImmediate();
      assert.equal(element('import').disabled, true);
      assert.equal(element('filters').value, 'example.test##.ad');
      assert.equal(element('message').textContent, '');
    });
  } finally { context.mock.timers.reset(); }
});

test('known compiler failure preserves the draft and permits correction and retry', async () => {
  await withPage(async message => message.type === 'config.import' ? { ok: false, error: 'Invalid filter syntax' } : initialReply(message), async element => {
    element('filters').value = 'bad rule';
    element('import').dispatch('click');
    await setImmediate();
    assert.equal(element('error').textContent, 'Invalid filter syntax');
    assert.doesNotMatch(element('summary').textContent, /state is unknown/);
    assert.equal(element('filters').value, 'bad rule');
    assert.equal(element('filters').readOnly, false);
    assert.equal(element('import').disabled, false);
  });
});
