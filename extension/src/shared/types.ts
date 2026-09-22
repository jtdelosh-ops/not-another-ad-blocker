export const OVERRIDE_ID = 1_000_000_000;
export const MAX_RULES = 2_000;
export const MAX_TEXT_BYTES = 128 * 1024;
export const MAX_SUBSCRIPTION_NETWORK = 29_800;
export const MAX_SUBSCRIPTION_COSMETIC = 10_000;
export const MAX_SELECTORS = 12_000;
export const MAX_DIAGNOSTICS = 200;
export const SUBSCRIPTION_URLS = { easylist: 'https://easylist.to/easylist/easylist.txt', easyprivacy: 'https://easylist.to/easylist/easyprivacy.txt' } as const;
export type SubscriptionId = keyof typeof SUBSCRIPTION_URLS;
export const RESOURCE_TYPES = ['sub_frame', 'stylesheet', 'script', 'image', 'font', 'object', 'xmlhttprequest', 'ping', 'media', 'websocket', 'other'] as const;
export interface NetworkRule {
  id: number;
  priority: number;
  action: { type: 'block' | 'allow' | 'allowAllRequests' };
  condition: { urlFilter?: string; requestDomains?: string[]; resourceTypes: string[]; domainType?: 'firstParty' | 'thirdParty'; initiatorDomains?: string[]; excludedInitiatorDomains?: string[]; isUrlFilterCaseSensitive?: boolean };
}
export interface CosmeticRule { domains: string[]; excludedDomains?: string[]; selector: string; raw: string; source: string }
export interface Diagnostic { line: number; raw: string; target: string; message: string; source?: string }
export interface Compilation {
  networkRules: NetworkRule[];
  cosmeticRules: CosmeticRule[];
  diagnostics: Diagnostic[];
  stats: { network: number; cosmetic: number; unsupported: number; ignored: number };
}
export interface CompanionStatus { companionVersion: string; protocolVersion: 1; healthy: boolean; capabilities: string[]; mode: 'local-import' }
export interface SubscriptionManifest {
  snapshotId: string;
  fetchedAt: string;
  lists: { id: SubscriptionId; title: string; url: string; fetchedAt: string; sha256: string; bytes: number; version: string }[];
  counts: { network: number; cosmetic: number; diagnostics: number };
  stats: Compilation['stats'];
  coverage: { networkSupported: number; networkDropped: number; cosmeticSupported: number; cosmeticDropped: number; exceptionSafetySuppressed: number; diagnosticsTotal: number; diagnosticsTruncated: boolean };
  listStats: (Compilation['stats'] & { id: SubscriptionId })[];
  unsupportedReasons: { reason: string; count: number }[];
}
export interface SubscriptionState { manifest: SubscriptionManifest; compiled: Compilation; appliedAt: string }
export interface SubscriptionProgress { phase: 'idle' | 'checking' | 'downloading' | 'network' | 'cosmetic' | 'diagnostics' | 'applying' | 'complete' | 'failed'; completed?: number; total?: number; message?: string }
export interface Config { version: 1; enabled: boolean; disabledSites: string[]; source: string; text: string; updatedAt: string | null; compiled: Compilation; subscriptions?: SubscriptionState }
// Interfaces receive bounded summaries instead of full network rule arrays.
export interface ConfigView extends Omit<Config, 'compiled' | 'subscriptions'> {
  compiled: Pick<Compilation, 'stats' | 'diagnostics'>;
  subscriptions?: { manifest: SubscriptionManifest; appliedAt: string; diagnostics: Diagnostic[] };
}
export function ruleCounts(config: Pick<ConfigView, 'compiled' | 'subscriptions'>): { network: number; cosmetic: number } {
  return { network: config.compiled.stats.network + (config.subscriptions?.manifest.stats.network ?? 0), cosmetic: config.compiled.stats.cosmetic + (config.subscriptions?.manifest.stats.cosmetic ?? 0) };
}
export function emptyConfig(): Config {
  return { version: 1, enabled: true, disabledSites: [], source: 'No list imported', text: '', updatedAt: null, compiled: { networkRules: [], cosmeticRules: [], diagnostics: [], stats: { network: 0, cosmetic: 0, unsupported: 0, ignored: 0 } } };
}
export function isDomain(value: unknown): value is string {
  return typeof value === 'string' && value.length <= 253 && value === value.toLowerCase() && value.split('.').every(label => /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/.test(label));
}
export function hostname(url: string | undefined): string | null {
  try { const parsed = new URL(url ?? ''); return /^https?:$/.test(parsed.protocol) && isDomain(parsed.hostname) ? parsed.hostname : null; } catch { return null; }
}
export function matchesDomain(host: string, domain: string): boolean { return host === domain || host.endsWith(`.${domain}`); }
export function disabledBy(host: string, sites: string[]): string | undefined { return sites.find(site => matchesDomain(host, site)); }
export function isSafeSelector(value: unknown): value is string {
  if (typeof value !== 'string' || !value.length || value.length > 512) return false;
  const compound = (text: string): boolean => text.length > 0 && /^(?:[A-Za-z][A-Za-z0-9-]*)?(?:[.#](?:-?[A-Za-z_]|--)[A-Za-z0-9_-]*)*$/.exec(text)?.[0] === text;
  const marker = ':has(>';
  const index = value.indexOf(marker);
  if (index < 0) return compound(value);
  if (!value.endsWith(')')) return false;
  const child = value.slice(index + marker.length, -1).replace(/^ +| +$/g, '');
  return compound(value.slice(0, index)) && /[.#]/.test(child) && compound(child);
}
export function errorMessage(error: unknown): string { return error instanceof Error ? error.message : String(error); }
export class RecoveryRequiredError extends Error {
  readonly code = 'RECOVERY_REQUIRED';
}
