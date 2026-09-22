import { MAX_RULES, MAX_TEXT_BYTES, MAX_SUBSCRIPTION_NETWORK, MAX_SUBSCRIPTION_COSMETIC, MAX_DIAGNOSTICS, SUBSCRIPTION_URLS, OVERRIDE_ID, RESOURCE_TYPES, isDomain, isSafeSelector, type Compilation, type CompanionStatus, type SubscriptionId, type SubscriptionManifest, type SubscriptionState, type SubscriptionProgress } from './types';
export const NATIVE_HOST = 'com.naab.companion';
export type NativeTransport = (host: string, message: unknown) => Promise<unknown>;
const bytes = (value: string) => new TextEncoder().encode(value).byteLength;
const record = (value: unknown): value is Record<string, any> => !!value && typeof value === 'object' && !Array.isArray(value);
const string = (value: unknown, limit: number): value is string => typeof value === 'string' && bytes(value) <= limit;
const keys = (value: Record<string, any>, allowed: string[]) => Object.keys(value).every(key => allowed.includes(key));
const integer = (value: unknown, max: number) => Number.isInteger(value) && (value as number) >= 0 && (value as number) <= max;
const domains = (value: unknown, max = 200) => Array.isArray(value) && value.length <= max && value.every(isDomain) && new Set(value).size === value.length;
const digest = (value: unknown): value is string => typeof value === 'string' && /^[a-f0-9]{64}$/.test(value);
const timestamp = (value: unknown): value is string => typeof value === 'string' && /^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d+)?(?:Z|\+00:00)$/.test(value) && Number.isFinite(Date.parse(value));
function invalid(): never { throw new Error('The companion returned an invalid response; existing rules were kept.'); }
export function validateSubscriptionIds(value: unknown): SubscriptionId[] {
  if (!Array.isArray(value) || !value.length || value.length > 2 || value.some(id => id !== 'easylist' && id !== 'easyprivacy') || new Set(value).size !== value.length) throw new Error('Select EasyList, EasyPrivacy, or both.');
  return [...value] as SubscriptionId[];
}
function validateNetwork(rule: unknown, subscriptions: boolean): void {
  if (!record(rule) || !keys(rule, ['id', 'priority', 'action', 'condition']) || !Number.isInteger(rule.id) || rule.id < 1 || rule.id >= OVERRIDE_ID) invalid();
  if (!record(rule.action) || !keys(rule.action, ['type']) || !(subscriptions ? ['block', 'allow', 'allowAllRequests'] : ['block', 'allow']).includes(rule.action.type) || rule.priority !== (rule.action.type === 'block' ? 1 : 2)) invalid();
  const condition = rule.condition;
  const fields = subscriptions ? ['urlFilter', 'requestDomains', 'resourceTypes', 'domainType', 'initiatorDomains', 'excludedInitiatorDomains', 'isUrlFilterCaseSensitive'] : ['urlFilter', 'resourceTypes', 'domainType'];
  if (!record(condition) || !keys(condition, fields) || !Array.isArray(condition.resourceTypes) || !condition.resourceTypes.length || new Set(condition.resourceTypes).size !== condition.resourceTypes.length) invalid();
  if (condition.requestDomains !== undefined) {
    if (!subscriptions || condition.urlFilter !== undefined || !domains(condition.requestDomains, 1000) || !condition.requestDomains.length) invalid();
  } else if (!string(condition.urlFilter, 2048) || !condition.urlFilter.length || (!subscriptions && !condition.urlFilter.startsWith('||')) || !/^[\x20-\x7e]+$/.test(condition.urlFilter)) invalid();
  if (rule.action.type === 'allowAllRequests') {
    if (condition.resourceTypes.some((type: unknown) => type !== 'main_frame' && type !== 'sub_frame')) invalid();
  } else if (condition.resourceTypes.some((type: unknown) => !RESOURCE_TYPES.includes(type as any))) invalid();
  if (condition.domainType !== undefined && !(subscriptions ? ['firstParty', 'thirdParty'] : ['thirdParty']).includes(condition.domainType)) invalid();
  for (const field of ['initiatorDomains', 'excludedInitiatorDomains']) if (condition[field] !== undefined && (!domains(condition[field]) || !condition[field].length)) invalid();
  if (condition.isUrlFilterCaseSensitive !== undefined && typeof condition.isUrlFilterCaseSensitive !== 'boolean') invalid();
}
function validateCosmetic(rule: unknown, subscriptions: boolean): void {
  if (!record(rule) || !keys(rule, ['domains', 'excludedDomains', 'selector', 'raw', 'source']) || !domains(rule.domains) || !isSafeSelector(rule.selector) || !string(rule.raw, 2048) || !string(rule.source, 128) || (rule.excludedDomains !== undefined && (!subscriptions || !domains(rule.excludedDomains)))) invalid();
}
function validateDiagnostic(value: unknown): void {
  if (!record(value) || !keys(value, ['line', 'raw', 'target', 'message', 'source']) || !integer(value.line, 600_000) || value.line < 1 || !string(value.raw, 2048) || !['MV3_NETWORK', 'COSMETIC', 'UNSUPPORTED', 'IGNORED', 'LOCAL_ONLY'].includes(value.target) || !string(value.message, 2048) || (value.source !== undefined && !string(value.source, 128))) invalid();
}
function validateStats(value: unknown, subscriptions: boolean): asserts value is Compilation['stats'] {
  if (!record(value) || !keys(value, ['network', 'cosmetic', 'unsupported', 'ignored'])) invalid();
  for (const field of ['network', 'cosmetic', 'unsupported', 'ignored']) if (!integer(value[field], subscriptions ? 600_002 : field === 'ignored' ? MAX_TEXT_BYTES + 1 : MAX_RULES)) invalid();
}
export function validateCompilation(value: unknown, subscriptions = false): Compilation {
  if (!record(value) || !Array.isArray(value.networkRules) || !Array.isArray(value.cosmeticRules) || !Array.isArray(value.diagnostics)) invalid();
  if (subscriptions ? value.networkRules.length > MAX_SUBSCRIPTION_NETWORK || value.cosmeticRules.length > MAX_SUBSCRIPTION_COSMETIC || value.diagnostics.length > MAX_DIAGNOSTICS : value.networkRules.length + value.cosmeticRules.length > MAX_RULES || value.diagnostics.length > MAX_RULES) invalid();
  const ids = new Set<number>();
  for (const rule of value.networkRules) { validateNetwork(rule, subscriptions); if (ids.has(rule.id)) invalid(); ids.add(rule.id); }
  for (const rule of value.cosmeticRules) validateCosmetic(rule, subscriptions);
  for (const diagnostic of value.diagnostics) validateDiagnostic(diagnostic);
  validateStats(value.stats, subscriptions);
  if (value.stats.network !== value.networkRules.length || value.stats.cosmetic !== value.cosmeticRules.length) invalid();
  return value as Compilation;
}
export function validateSubscriptionManifest(value: unknown, requested?: SubscriptionId[], budget = MAX_SUBSCRIPTION_NETWORK): SubscriptionManifest {
  if (!record(value) || !keys(value, ['snapshotId', 'fetchedAt', 'lists', 'counts', 'stats', 'coverage', 'listStats', 'unsupportedReasons']) || !digest(value.snapshotId) || !timestamp(value.fetchedAt) || !Array.isArray(value.lists) || !Array.isArray(value.listStats) || !record(value.counts) || !record(value.coverage) || !Array.isArray(value.unsupportedReasons)) invalid();
  const ids = validateSubscriptionIds(value.lists.map((list: any) => list?.id));
  if (requested && (ids.length !== requested.length || ids.some(id => !requested.includes(id)))) invalid();
  for (const list of value.lists) if (!record(list) || !keys(list, ['id', 'title', 'url', 'fetchedAt', 'sha256', 'bytes', 'version']) || !string(list.title, 256) || !list.title.length || list.url !== SUBSCRIPTION_URLS[list.id as SubscriptionId] || !timestamp(list.fetchedAt) || !digest(list.sha256) || !integer(list.bytes, 8 * 1024 * 1024) || !list.bytes || !string(list.version, 256)) invalid();
  if (!keys(value.counts, ['network', 'cosmetic', 'diagnostics']) || !integer(value.counts.network, budget) || !integer(value.counts.cosmetic, MAX_SUBSCRIPTION_COSMETIC) || !integer(value.counts.diagnostics, MAX_DIAGNOSTICS)) invalid();
  validateStats(value.stats, true);
  if (value.stats.network !== value.counts.network || value.stats.cosmetic !== value.counts.cosmetic) invalid();
  const coverage = value.coverage;
  const countFields = ['networkSupported', 'networkDropped', 'cosmeticSupported', 'cosmeticDropped', 'exceptionSafetySuppressed', 'diagnosticsTotal'];
  if (!keys(coverage, [...countFields, 'diagnosticsTruncated']) || countFields.some(field => !integer(coverage[field], 600_002)) || typeof coverage.diagnosticsTruncated !== 'boolean' || coverage.diagnosticsTotal < value.counts.diagnostics || coverage.diagnosticsTruncated !== (coverage.diagnosticsTotal > value.counts.diagnostics) || coverage.networkSupported !== value.counts.network + coverage.networkDropped || coverage.cosmeticSupported < value.counts.cosmetic + coverage.cosmeticDropped) invalid();
  if (value.listStats.length !== ids.length || new Set(value.listStats.map((stats: any) => stats?.id)).size !== ids.length) invalid();
  for (const stats of value.listStats) {
    if (!record(stats) || !keys(stats, ['id', 'network', 'cosmetic', 'unsupported', 'ignored']) || !ids.includes(stats.id)) invalid();
    const { id: _id, ...counts } = stats; validateStats(counts, true);
  }
  if (value.unsupportedReasons.length > 200) invalid();
  for (const item of value.unsupportedReasons) if (!record(item) || !keys(item, ['reason', 'count']) || !string(item.reason, 2048) || !item.reason.length || !integer(item.count, 600_002) || item.count < 1) invalid();
  return value as SubscriptionManifest;
}
export function validateSubscriptionState(value: unknown): SubscriptionState {
  if (!record(value) || !keys(value, ['manifest', 'compiled', 'appliedAt']) || !timestamp(value.appliedAt)) invalid();
  const manifest = validateSubscriptionManifest(value.manifest);
  const compiled = validateCompilation(value.compiled, true);
  if (compiled.diagnostics.length !== manifest.counts.diagnostics || ['network', 'cosmetic', 'unsupported', 'ignored'].some(field => compiled.stats[field as keyof Compilation['stats']] !== manifest.stats[field as keyof Compilation['stats']])) invalid();
  if (bytes(JSON.stringify(value)) > 25 * 1024 * 1024) invalid();
  return value as SubscriptionState;
}
export class NativeClient {
  private sequence = 0;
  constructor(private readonly transport: NativeTransport, private readonly timeoutMs = 15_000, private readonly refreshTimeoutMs = 120_000) {}
  private async request(type: string, payload: unknown, timeout = this.timeoutMs): Promise<unknown> {
    const id = `naab-${Date.now()}-${++this.sequence}`;
    let timer: ReturnType<typeof setTimeout> | undefined;
    try {
      const response = await Promise.race([
        this.transport(NATIVE_HOST, { version: 1, id, type, payload }),
        new Promise<never>((_, reject) => { timer = setTimeout(() => reject(new Error('The local companion timed out. Existing rules remain active.')), timeout); })
      ]);
      if (!record(response) || response.version !== 1 || response.id !== id || typeof response.ok !== 'boolean') invalid();
      if (!response.ok) {
        if (!record(response.error) || !string(response.error.message, 2048) || !string(response.error.code, 128)) invalid();
        throw new Error(`${response.error.code}: ${response.error.message}`);
      }
      return response.payload;
    } finally { if (timer) clearTimeout(timer); }
  }
  async status(): Promise<CompanionStatus> {
    const value = await this.request('status.get', {});
    if (!record(value) || !string(value.companionVersion, 64) || value.protocolVersion !== 1 || typeof value.healthy !== 'boolean' || !Array.isArray(value.capabilities) || value.capabilities.length > 32 || !value.capabilities.every((capability: unknown) => string(capability, 64)) || !value.capabilities.includes('rules.compile') || value.mode !== 'local-import') invalid();
    return value as CompanionStatus;
  }
  async compile(text: string, source = 'Local import'): Promise<Compilation> {
    if (typeof text !== 'string' || bytes(text) > MAX_TEXT_BYTES) throw new Error('Local lists are limited to 128 KiB in this preview.');
    if (!string(source, 128)) throw new Error('The list name is limited to 128 UTF-8 bytes.');
    return validateCompilation(await this.request('rules.compile', { text, source }));
  }
  async refresh(ids: SubscriptionId[], networkBudget: number, onProgress: (progress: SubscriptionProgress) => void = () => {}): Promise<SubscriptionState> {
    ids = validateSubscriptionIds(ids);
    if (!integer(networkBudget, MAX_SUBSCRIPTION_NETWORK)) throw new Error('Invalid subscription network rule budget.');
    onProgress({ phase: 'checking' });
    const status = await this.status();
    if (!status.healthy) throw new Error('The companion is unhealthy. Existing rules were kept.');
    if (!status.capabilities.includes('lists.refresh') || !status.capabilities.includes('lists.page')) throw new Error('Update the local companion to version 0.2.0 or later for EasyList/EasyPrivacy. Existing local rules remain active.');
    onProgress({ phase: 'downloading' });
    const manifest = validateSubscriptionManifest(await this.request('lists.refresh', { ids, networkBudget }, this.refreshTimeoutMs), ids, networkBudget);
    const compiled: Compilation = { networkRules: [], cosmeticRules: [], diagnostics: [], stats: manifest.stats };
    let receivedBytes = bytes(JSON.stringify(manifest));
    for (const kind of ['network', 'cosmetic', 'diagnostics'] as const) {
      const items = kind === 'network' ? compiled.networkRules : kind === 'cosmetic' ? compiled.cosmeticRules : compiled.diagnostics;
      const total = manifest.counts[kind];
      onProgress({ phase: kind, completed: 0, total });
      let offset = 0;
      do {
        const page = await this.request('lists.page', { snapshotId: manifest.snapshotId, kind, offset });
        const pageBytes = bytes(JSON.stringify(page));
        receivedBytes += pageBytes;
        if (!record(page) || !keys(page, ['snapshotId', 'kind', 'offset', 'total', 'items', 'nextOffset']) || page.snapshotId !== manifest.snapshotId || page.kind !== kind || page.offset !== offset || page.total !== total || !Array.isArray(page.items) || page.items.length > 128 || pageBytes > 512 * 1024 || receivedBytes > 25 * 1024 * 1024) invalid();
        const next = offset + page.items.length;
        if (next > total || (next < total && (!page.items.length || page.nextOffset !== next)) || (next === total && page.nextOffset !== null)) invalid();
        for (const item of page.items) {
          if (kind === 'network') validateNetwork(item, true);
          else if (kind === 'cosmetic') validateCosmetic(item, true);
          else validateDiagnostic(item);
        }
        items.push(...page.items);
        offset = next;
        onProgress({ phase: kind, completed: offset, total });
        if (page.nextOffset === null) break;
      } while (offset < total);
      if (items.length !== total) invalid();
    }
    return validateSubscriptionState({ manifest, compiled, appliedAt: new Date().toISOString() });
  }
}
