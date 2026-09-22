import { test } from 'node:test';
import assert from 'node:assert/strict';
import { setImmediate } from 'node:timers/promises';
import { diagnosticSamples } from '../src/shared/diagnostics';
import { pollSubscriptionProgress } from '../src/shared/ui';
import { configView } from '../src/background/controller';
import { emptyConfig, MAX_DIAGNOSTICS, type Diagnostic, type SubscriptionProgress } from '../src/shared/types';

const diagnostics = (count: number, source: string): Diagnostic[] => Array.from({ length: count }, (_, index) => ({ line: index + 1, raw: `rule-${index}`, target: 'UNSUPPORTED', message: 'Unsupported rule', source }));

test('the bounded diagnostic table keeps explanations from both local rules and full subscriptions', () => {
  const local = diagnostics(200, 'Local import');
  const subscriptions = diagnostics(200, 'easylist');
  const samples = diagnosticSamples(local, subscriptions);
  assert.equal(samples.length, MAX_DIAGNOSTICS);
  assert.equal(samples.filter(row => row.source === 'Local import').length, 100);
  assert.equal(samples.filter(row => row.source === 'easylist').length, 100);
  assert.ok(samples.includes(local[0]));
  assert.ok(samples.includes(subscriptions[0]));
});
test('diagnostic allocation uses spare capacity and prioritizes late unsupported local lines before transport truncation', () => {
  assert.equal(diagnosticSamples(diagnostics(3, 'local'), diagnostics(200, 'easylist')).filter(row => row.source === 'easylist').length, 197);
  assert.equal(diagnosticSamples(diagnostics(200, 'local'), diagnostics(2, 'easylist')).filter(row => row.source === 'local').length, 198);
  assert.equal(diagnosticSamples([], diagnostics(200, 'easylist')).length, 200);
  assert.equal(diagnosticSamples(diagnostics(200, 'local')).length, 200);
  const config = emptyConfig();
  config.compiled.diagnostics = diagnostics(250, 'Local import').map(row => ({ ...row, target: 'MV3_NETWORK' }));
  const unsupported = { line: 251, raw: '##div:has(.ad)', target: 'UNSUPPORTED', message: 'Unsupported selector', source: 'Local import' };
  config.compiled.diagnostics.push(unsupported); config.compiled.stats.unsupported = 1;
  const view = configView(config);
  assert.equal(view.compiled.diagnostics.length, 200);
  const table = diagnosticSamples(view.compiled.diagnostics, diagnostics(200, 'easylist'));
  assert.ok(table.some(row => row.line === 251 && row.message === 'Unsupported selector'));
});
test('late progress responses cannot replace completed or failed refresh messages', async () => {
  for (const phase of ['complete', 'failed'] as const) {
    let started!: () => void;
    const requestStarted = new Promise<void>(resolve => { started = resolve; });
    let finish!: (value: SubscriptionProgress) => void;
    const pending = new Promise<SubscriptionProgress>(resolve => { finish = resolve; });
    const shown: SubscriptionProgress[] = [];
    const stop = pollSubscriptionProgress(() => { started(); return pending; }, progress => shown.push(progress), 1);
    try {
      await requestStarted;
      stop();
      shown.push({ phase });
      finish({ phase: 'network', completed: 128, total: 1000 });
      await setImmediate();
      assert.deepEqual(shown, [{ phase }]);
    } finally { stop(); }
  }
});
