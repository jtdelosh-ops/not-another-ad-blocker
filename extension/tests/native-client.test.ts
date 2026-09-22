import { test } from 'node:test';
import assert from 'node:assert/strict';
import { NATIVE_HOST, NativeClient, validateCompilation } from '../src/shared/native-client';
import { emptyConfig, isSafeSelector, MAX_TEXT_BYTES, OVERRIDE_ID, RESOURCE_TYPES } from '../src/shared/types';

const valid = () => ({ ...emptyConfig().compiled, networkRules: [{ id: 1, priority: 1, action: { type: 'block' }, condition: { urlFilter: '||ads.example.test^', resourceTypes: [...RESOURCE_TYPES] } }], stats: { network: 1, cosmetic: 0, unsupported: 0, ignored: 0 } });
test('native client sends versioned requests to the declared host and validates status', async () => {
  const client = new NativeClient(async (host, request) => {
    const message = request as any;
    assert.equal(host, NATIVE_HOST); assert.equal(message.version, 1); assert.equal(message.type, 'status.get'); assert.deepEqual(message.payload, {});
    return { version: 1, id: message.id, ok: true, payload: { companionVersion: '0.1.0', protocolVersion: 1, healthy: true, capabilities: ['rules.compile'], mode: 'local-import' } };
  });
  assert.equal((await client.status()).healthy, true);
});
test('native client rejects response ID mismatch and surfaces structured host errors', async () => {
  const bad = new NativeClient(async () => ({ version: 1, id: 'wrong', ok: true, payload: valid() }));
  await assert.rejects(bad.compile('test'), /invalid response/);
  const failure = new NativeClient(async (_host, request) => ({ version: 1, id: (request as any).id, ok: false, error: { code: 'INVALID_REQUEST', message: 'Rejected' } }));
  await assert.rejects(failure.compile('test'), /INVALID_REQUEST: Rejected/);
});
test('native timeout is bounded, and oversized UTF-8 input never reaches host', async () => {
  const hung = new NativeClient(async () => new Promise(() => {}), 10);
  await assert.rejects(hung.status(), /timed out/);
  let called = false;
  const client = new NativeClient(async () => { called = true; return {}; });
  await assert.rejects(client.compile('é'.repeat(MAX_TEXT_BYTES / 2 + 1)), /128 KiB/);
  assert.equal(called, false);
});
test('network validation rejects main-frame blocking, redirects, duplicate and reserved IDs', () => {
  const mainFrame = valid(); (mainFrame.networkRules[0].condition as any).resourceTypes = ['main_frame'];
  assert.throws(() => validateCompilation(mainFrame), /invalid response/);
  const redirect = valid(); (redirect.networkRules[0].action as any).type = 'redirect';
  assert.throws(() => validateCompilation(redirect), /invalid response/);
  const duplicate = valid(); duplicate.networkRules.push(duplicate.networkRules[0]); duplicate.stats.network = 2;
  assert.throws(() => validateCompilation(duplicate), /invalid response/);
  const reserved = valid(); reserved.networkRules[0].id = OVERRIDE_ID;
  assert.throws(() => validateCompilation(reserved), /invalid response/);
  const extra = valid(); (extra.networkRules[0].condition as any).regexFilter = '.*';
  assert.throws(() => validateCompilation(extra), /invalid response/);
});
test('selectors cannot inject CSS, URLs, attributes or procedural expressions', () => {
  for (const selector of ['.ad', 'div.ad.banner', '#sponsor', 'aside', '.-ad', '.--ad', '.--', '.--1']) assert.equal(isSafeSelector(selector), true, selector);
  for (const selector of ['', '.ad}body{color:red', '.ad,body', '[data-ad]', 'div:has(.ad)', '.ad .child', '@import', '.ad\\7b', '*', '.-9ad', '.-', '.' + 'a'.repeat(512)]) assert.equal(isSafeSelector(selector), false, selector);
  assert.equal(isSafeSelector('.' + 'a'.repeat(511)), true);
  const injected = { ...valid(), cosmeticRules: [{ domains: [], selector: '.ad}body{display:none}', raw: '', source: 'Test' }], stats: { network: 1, cosmetic: 1, ignored: 0, unsupported: 0 } };
  assert.throws(() => validateCompilation(injected), /invalid response/);
});
test('blank lines may exceed nonempty line quota without invalidating compilation', () => {
  const result = valid(); result.stats.ignored = 2_001;
  assert.equal(validateCompilation(result).stats.ignored, 2_001);
  result.stats.ignored = MAX_TEXT_BYTES + 2;
  assert.throws(() => validateCompilation(result), /invalid response/);
});

test('direct-child hiding permits one class/ID check without CSS injection or broad video matching', () => {
  for (const selector of ['div:has(> .t-j-inbanlabel-container)', '.slot:has(>#ad-label)']) assert.equal(isSafeSelector(selector), true, selector);
  for (const selector of ['div:has(> video)', 'div:has(> .ad:has(> .nested))', 'div:has(> .ad),body', 'div:has(> .ad) .child', 'div:has(> [data-ad])', 'div:has(> .ad\n)', '.ad\n', 'div:has(> .ad){display:none}', 'div:has(> .ad)\n']) assert.equal(isSafeSelector(selector), false, selector);
  const base = 'div:has(> .';
  assert.equal(isSafeSelector(base + 'a'.repeat(512 - base.length - 1) + ')'), true);
  assert.equal(isSafeSelector(base + 'a'.repeat(513 - base.length - 1) + ')'), false);
});
