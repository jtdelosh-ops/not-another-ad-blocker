import test from 'node:test';
import assert from 'node:assert/strict';
import path from 'node:path';
import { mkdtempSync, readFileSync, existsSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { makePlan, applyPlan, HOST } from '../scripts/native-host.mjs';

const base = { platform: 'darwin', extensionId: 'a'.repeat(32), binary: path.resolve('companion'), home: path.resolve('fake-home') };
test('manifest scopes access to exactly one extension and uses current-user paths', () => {
  const plan = makePlan(base);
  assert.deepEqual(plan.manifest.allowed_origins, [`chrome-extension://${'a'.repeat(32)}/`]);
  assert.equal(plan.manifest.name, HOST);
  assert.ok(plan.manifestPath.startsWith(base.home));
  assert.equal(plan.registryKey, null);
  const windows = makePlan({ ...base, platform: 'win32', browser: 'edge', localAppData: path.resolve('local-data') });
  assert.equal(windows.registryKey, `HKCU\\Software\\Microsoft\\Edge\\NativeMessagingHosts\\${HOST}`);
});
test('rejects invalid IDs, relative paths, unknown browsers and unsupported systems', () => {
  for (const changes of [{ extensionId: '*' }, { extensionId: 'z'.repeat(32) }, { binary: './relative' }, { browser: 'unknown' }, { platform: 'linux' }]) {
    assert.throws(() => makePlan({ ...base, ...changes }));
  }
});
test('macOS file registration is reversible and refuses conflicting existing manifests', () => {
  const home = mkdtempSync(path.join(tmpdir(), 'naab-installer-'));
  try {
    const binary = path.join(home, 'host'); writeFileSync(binary, 'fixture');
    const plan = makePlan({ ...base, home, binary });
    applyPlan(plan);
    assert.deepEqual(JSON.parse(readFileSync(plan.manifestPath, 'utf8')), plan.manifest);
    assert.throws(() => applyPlan(makePlan({ ...base, home, binary, extensionId: 'b'.repeat(32) })), /differs/);
    applyPlan({ ...plan, operation: 'unregister' });
    assert.equal(existsSync(plan.manifestPath), false);
    assert.equal(existsSync(binary), true);
  } finally { rmSync(home, { recursive: true }); }
});
