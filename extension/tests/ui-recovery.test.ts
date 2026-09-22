import { test } from 'node:test';
import assert from 'node:assert/strict';
import { renderRecoveryRequired, request } from '../src/shared/ui';
import { RecoveryRequiredError } from '../src/shared/types';

test('structured recovery error survives the runtime boundary and replaces stale protection claims', async () => {
  const original = globalThis.chrome;
  try {
    globalThis.chrome = { runtime: { sendMessage: async () => ({ ok: false, code: 'RECOVERY_REQUIRED', error: 'Storage and rollback both failed.' }) } } as any;
    const summary = { textContent: 'Protection enabled with imported rules' };
    const globalToggle = { disabled: false };
    const siteToggle = { disabled: false };
    const importButton = { disabled: false };
    const companionCheck = { disabled: false };
    const openOptions = { disabled: false };
    await assert.rejects(request({ type: 'config.enabled', enabled: false }), error => {
      assert.ok(error instanceof RecoveryRequiredError);
      renderRecoveryRequired(summary, [globalToggle, siteToggle, importButton]);
      return true;
    });
    assert.match(summary.textContent, /Protection state is unknown/);
    assert.match(summary.textContent, /reload the extension/);
    assert.equal(globalToggle.disabled, true);
    assert.equal(siteToggle.disabled, true);
    assert.equal(importButton.disabled, true);
    assert.equal(companionCheck.disabled, false);
    assert.equal(openOptions.disabled, false);
    // An ordinary completion/unbusy cycle cannot leave mutation controls active
    // when the latched recovery render is applied again.
    globalToggle.disabled = importButton.disabled = false;
    renderRecoveryRequired(summary, [globalToggle, siteToggle, importButton]);
    assert.equal(globalToggle.disabled, true);
    assert.equal(importButton.disabled, true);
  } finally { globalThis.chrome = original; }
});

test('ordinary failures do not become fatal based on message wording', async () => {
  const original = globalThis.chrome;
  try {
    globalThis.chrome = { runtime: { sendMessage: async () => ({ ok: false, error: 'Protection state is unknown' }) } } as any;
    await assert.rejects(request({ type: 'config.import' }), error => {
      assert.ok(error instanceof Error);
      assert.equal(error instanceof RecoveryRequiredError, false);
      return true;
    });
  } finally { globalThis.chrome = original; }
});
