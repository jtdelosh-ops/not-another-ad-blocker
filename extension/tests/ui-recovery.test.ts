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

test('a closed runtime channel makes mutation results unknown without retrying the save', async () => {
  const original = globalThis.chrome;
  let sends = 0;
  try {
    globalThis.chrome = { runtime: { sendMessage: async () => {
      sends++;
      throw new Error('A listener indicated an asynchronous response by returning true, but the message channel closed before a response was received');
    } } } as any;
    await assert.rejects(request({ type: 'config.import', text: 'example.test##.ad' }), error => {
      assert.ok(error instanceof RecoveryRequiredError);
      assert.match(error.message, /may already be saved/);
      assert.match(error.message, /Copy any unsaved filter text/);
      return true;
    });
    assert.equal(sends, 1);
    await assert.rejects(request({ type: 'status.get' }), error => {
      assert.ok(error instanceof Error);
      assert.equal(error instanceof RecoveryRequiredError, false);
      return true;
    });
  } finally { globalThis.chrome = original; }
});

test('missing mutation reply cannot be mistaken for a confirmed failed transaction', async () => {
  const original = globalThis.chrome;
  try {
    globalThis.chrome = { runtime: { sendMessage: async () => undefined } } as any;
    await assert.rejects(request({ type: 'config.enabled', enabled: false }), RecoveryRequiredError);
  } finally { globalThis.chrome = original; }
});

test('stalled requests expire, preserve the longer list deadline, and ignore late save replies', async context => {
  const original = globalThis.chrome;
  context.mock.timers.enable({ apis: ['setTimeout'] });
  let finish!: (response: unknown) => void;
  let sends = 0;
  try {
    globalThis.chrome = { runtime: { sendMessage: () => {
      sends++;
      return new Promise(resolve => { finish = resolve; });
    } } } as any;
    const local = request({ type: 'config.import' });
    const rejected = assert.rejects(local, RecoveryRequiredError);
    context.mock.timers.tick(30_000);
    await rejected;
    finish({ ok: true, payload: 'late save' });
    await assert.rejects(local, RecoveryRequiredError);
    assert.equal(sends, 1);

    let settled = false;
    const list = request({ type: 'subscriptions.refresh' });
    void list.then(() => { settled = true; }, () => { settled = true; });
    const listRejected = assert.rejects(list, RecoveryRequiredError);
    context.mock.timers.tick(30_000);
    await Promise.resolve();
    assert.equal(settled, false);
    context.mock.timers.tick(150_000);
    await listRejected;
  } finally { globalThis.chrome = original; context.mock.timers.reset(); }
});
