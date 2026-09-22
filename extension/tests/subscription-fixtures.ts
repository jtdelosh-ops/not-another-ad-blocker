import type { Compilation, NetworkRule, SubscriptionState } from '../src/shared/types';
import { RESOURCE_TYPES, SUBSCRIPTION_URLS } from '../src/shared/types';
import { NativeClient, type NativeTransport } from '../src/shared/native-client';
export const networkRule = (id = 1, action: 'block' | 'allow' = 'block'): NetworkRule => ({ id, priority: action === 'block' ? 1 : 2, action: { type: action }, condition: { urlFilter: `||ad${id}.example.test^`, resourceTypes: [...RESOURCE_TYPES] } });
export function subscriptionFixture(): SubscriptionState {
  const timestamp = '2026-09-21T12:00:00Z';
  const stats = { network: 1, cosmetic: 1, unsupported: 1, ignored: 2 };
  const compiled: Compilation = {
    networkRules: [networkRule()],
    cosmeticRules: [{ selector: '.subscription-ad', domains: [], excludedDomains: ['excluded.test'], raw: '##.subscription-ad', source: 'easylist' }],
    diagnostics: [{ line: 17, raw: '##div:has(.ad)', target: 'UNSUPPORTED', message: 'Unsupported selector', source: 'easylist' }], stats
  };
  return { manifest: {
    snapshotId: 'a'.repeat(64), fetchedAt: timestamp,
    lists: [{ id: 'easylist', title: 'EasyList', url: SUBSCRIPTION_URLS.easylist, fetchedAt: timestamp, sha256: 'b'.repeat(64), bytes: 1000, version: '202609211200' }],
    counts: { network: 1, cosmetic: 1, diagnostics: 1 }, stats: { ...stats },
    coverage: { networkSupported: 1, networkDropped: 0, cosmeticSupported: 1, cosmeticDropped: 0, exceptionSafetySuppressed: 0, diagnosticsTotal: 1, diagnosticsTruncated: false },
    listStats: [{ id: 'easylist', ...stats }], unsupportedReasons: [{ reason: 'Unsupported selector', count: 1 }]
  }, compiled, appliedAt: timestamp };
}
export function fixtureClient(state = subscriptionFixture(), mutate: (payload: any, message: any) => any = value => value, capture: (message: any) => void = () => {}): NativeClient {
  const transport: NativeTransport = async (_host, request) => {
    const message = request as any; capture(message);
    let payload: any;
    if (message.type === 'status.get') payload = { companionVersion: '0.2.0', protocolVersion: 1, healthy: true, capabilities: ['rules.compile', 'lists.refresh', 'lists.page'], mode: 'local-import' };
    else if (message.type === 'lists.refresh') payload = state.manifest;
    else if (message.type === 'rules.compile') payload = { ...state.compiled, cosmeticRules: state.compiled.cosmeticRules.map(({ excludedDomains: _, ...rule }) => rule) };
    else {
      const { kind, offset } = message.payload;
      const source = kind === 'network' ? state.compiled.networkRules : kind === 'cosmetic' ? state.compiled.cosmeticRules : state.compiled.diagnostics;
      const items = source.slice(offset, offset + 128);
      payload = { snapshotId: state.manifest.snapshotId, kind, offset, total: source.length, items, nextOffset: offset + items.length < source.length ? offset + items.length : null };
    }
    return { version: 1, id: message.id, ok: true, payload: mutate(structuredClone(payload), message) };
  };
  return new NativeClient(transport);
}
