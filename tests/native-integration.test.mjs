import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { endianness } from 'node:os';
import { NativeClient } from '../extension/lib/native-client.mjs';
import { exchange, frame, nativeTransport } from './native-transport.mjs';

const binary = process.env.NAAB_BINARY || fileURLToPath(new URL(`../companion/target/debug/naab-companion${process.platform === 'win32' ? '.exe' : ''}`, import.meta.url));
assert.ok(existsSync(binary), `Build the companion first or set NAAB_BINARY: ${binary}`);
const client = new NativeClient(nativeTransport(binary));
const demo = readFileSync(new URL('./fixtures/local-filters.txt', import.meta.url), 'utf8');

test('real extension client exchanges status with the Rust executable', async () => {
  const status = await client.status();
  assert.equal(status.protocolVersion, 1);
  assert.equal(status.healthy, true);
  assert.ok(status.capabilities.includes('rules.compile'));
});
test('local fixture compiles through the real extension client and native wire', async () => {
  const result = await client.compile(demo, 'Integration fixture');
  assert.equal(result.stats.network, 3);
  assert.equal(result.stats.cosmetic, 2);
  assert.equal(result.stats.unsupported, 1);
  assert.equal(new Set(result.networkRules.map(rule => rule.id)).size, 3);
  assert.ok(result.networkRules.every(rule => !rule.condition.resourceTypes.includes('main_frame')));
  assert.equal(result.networkRules.find(rule => rule.action.type === 'allow').priority, 2);
  assert.ok(result.diagnostics.some(row => row.target === 'UNSUPPORTED' && row.raw.includes('redirect')));
  assert.deepEqual((await client.compile(demo, 'Integration fixture')).networkRules, result.networkRules);
});
test('multiple messages in one process, UTF-8, and structured version/type errors', async () => {
  const requests = [
    { version: 1, id: 'héllo', type: 'status.get', payload: {} },
    { version: 2, id: 'version', type: 'status.get', payload: {} },
    { version: 1, id: 'unknown', type: 'unsupported.call', payload: {} },
    { version: 1, id: 'last', type: 'status.get', payload: {} },
  ];
  const result = await exchange(binary, Buffer.concat(requests.map(frame)), { fragment: true });
  assert.equal(result.code, 0);
  assert.equal(result.messages.length, 4);
  assert.deepEqual(result.messages.map(message => message.ok), [true, false, false, true]);
  assert.deepEqual(result.messages.map(message => message.id), requests.map(request => request.id));
});
test('oversized and truncated incoming frames fail safely', async () => {
  const oversized = Buffer.alloc(4); oversized[endianness() === 'LE' ? 'writeUInt32LE' : 'writeUInt32BE'](256 * 1024 + 1);
  for (const bytes of [oversized, Buffer.from([3, 0]), Buffer.from([9, 0, 0, 0, 123])]) {
    const result = await exchange(binary, bytes);
    assert.notEqual(result.code, 0);
    assert.equal(result.messages.length, 0);
    assert.ok(result.stderr.length > 0);
  }
});

test('Rust and TypeScript agree on cosmetic grammar, boundaries, and blank lines', async () => {
  const hyphens = await client.compile('##.-ad\n##.--ad\n||ads.example.test^');
  assert.equal(hyphens.stats.cosmetic, 2);
  assert.equal(hyphens.stats.network, 1);
  const selectors = await client.compile(`##.${'a'.repeat(511)}\n##.${'a'.repeat(512)}\n||ads.example.test^`);
  assert.equal(selectors.stats.cosmetic, 1);
  assert.equal(selectors.stats.unsupported, 1);
  assert.equal(selectors.stats.network, 1);
  const blank = await client.compile('\n'.repeat(4000));
  assert.equal(blank.stats.network, 0);
  assert.equal(blank.stats.unsupported, 0);
  const domains = Array.from({ length: 201 }, (_, i) => `a${i}.test`);
  const scoped = await client.compile(`${domains.join(',')}##.ad\n||ads.example.test^`);
  assert.equal(scoped.stats.cosmetic, 0);
  assert.equal(scoped.stats.unsupported, 1);
  assert.equal(scoped.stats.network, 1);
});
