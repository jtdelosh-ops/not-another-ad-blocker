import { MAX_DIAGNOSTICS, type Diagnostic } from './types';

// Give each source a share of the bounded table and use spare capacity from
// either share. Unsupported local lines stay visible even after long imports.
export function diagnosticSamples(local: readonly Diagnostic[], subscriptions: readonly Diagnostic[] = [], limit = MAX_DIAGNOSTICS): Diagnostic[] {
  const prioritize = (rows: readonly Diagnostic[]) => [...rows.filter(row => row.target === 'UNSUPPORTED'), ...rows.filter(row => row.target !== 'UNSUPPORTED')];
  const localRows = prioritize(local);
  const subscriptionRows = prioritize(subscriptions);
  let localCount = Math.min(localRows.length, Math.ceil(limit / 2));
  const subscriptionCount = Math.min(subscriptionRows.length, limit - localCount);
  localCount += Math.min(localRows.length - localCount, limit - localCount - subscriptionCount);
  return [...localRows.slice(0, localCount), ...subscriptionRows.slice(0, subscriptionCount)];
}
