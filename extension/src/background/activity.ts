import { ACTIVITY_LIMIT, type ActivityEntry, type ActivityRuleInfo } from '../shared/activity';
import { isDomain } from '../shared/types';

export interface ActivityStorage {
  read(): Promise<unknown>;
  write(value: unknown): Promise<void>;
}
export interface ActivityMatch {
  rule: { ruleId: number; rulesetId: string };
  request: { tabId: number; requestId: string; url: string; type: string };
}
const ACTIONS = new Set(['block', 'allow', 'allowAllRequests', 'unknown']);
const MAX_REQUEST = 320;
export function supportsBlockedCount(rules: { action: { type: string } }[]): boolean {
  return rules.every(rule => ['block', 'allow', 'allowAllRequests'].includes(rule.action.type));
}
export function blockedCountText(value: string): string | null {
  // Chrome may return a placeholder when feedback is unavailable. Never turn it
  // into zero. An empty badge is zero only after our native counter is enabled.
  return value === '' ? '0' : /^\d+\+?$/.test(value) ? value : null;
}
export function privateRequestURL(value: string): string | null {
  try {
    const url = new URL(value);
    if (url.protocol !== 'http:' && url.protocol !== 'https:') return null;
    // Never retain request credentials, query parameters, or fragments.
    const safe = url.origin + url.pathname;
    return safe.length > MAX_REQUEST ? safe.slice(0, MAX_REQUEST - 1) + '…' : safe;
  } catch { return null; }
}
function restoredEntry(value: unknown): ActivityEntry | null {
  if (!value || typeof value !== 'object') return null;
  const e = value as ActivityEntry;
  if (!Number.isSafeInteger(e.id) || e.id < 1 || !Number.isSafeInteger(e.tabId) || e.tabId < 0 || !Number.isSafeInteger(e.ruleId) || e.ruleId < 1 || !Number.isFinite(e.timestamp) || e.timestamp < 0 || !ACTIONS.has(e.action) || typeof e.request !== 'string' || typeof e.resourceType !== 'string' || typeof e.source !== 'string' || typeof e.condition !== 'string') return null;
  const request = privateRequestURL(e.request);
  if (!request) return null;
  return { id: e.id, tabId: e.tabId, site: isDomain(e.site) ? e.site : null, ruleId: e.ruleId, timestamp: e.timestamp, action: e.action, request, resourceType: e.resourceType.slice(0, 40), source: e.source.slice(0, 200), condition: e.condition.slice(0, 1024) };
}

// A bounded diagnostic sample, deliberately independent of Chrome's page count.
// Batching writes avoids one storage operation for every blocked request.
export class ActivityLog {
  readonly ready: Promise<void>;
  private entries: ActivityEntry[] = [];
  private pending: { entry: ActivityEntry; key: string }[] = [];
  private initialized = false;
  private sequence = 0;
  private seen = new Set<string>();
  private dirty = false;
  private timer: ReturnType<typeof setTimeout> | undefined;
  private saving: Promise<void> | undefined;
  storageError = false;

  constructor(private storage: ActivityStorage, private now = Date.now) {
    this.ready = this.initialize();
  }
  private async initialize(): Promise<void> {
    try {
      const saved = await this.storage.read() as { version?: unknown; entries?: unknown } | undefined;
      if (saved?.version === 1 && Array.isArray(saved.entries)) {
        this.entries = saved.entries.slice(-ACTIVITY_LIMIT).map(restoredEntry).filter((entry): entry is ActivityEntry => entry !== null);
        this.sequence = Math.max(0, ...this.entries.map(entry => entry.id));
      }
    } catch { this.storageError = true; }
    this.initialized = true;
    for (const item of this.pending) this.append(item.entry, item.key);
    this.pending = [];
  }
  record(match: ActivityMatch, info: ActivityRuleInfo | null, site: string | null = null): void {
    const { rule, request } = match;
    if (rule.rulesetId !== '_dynamic' || !Number.isSafeInteger(rule.ruleId) || rule.ruleId < 1 || !Number.isSafeInteger(request.tabId) || request.tabId < 0 || typeof request.requestId !== 'string' || request.requestId.length > 128 || typeof request.url !== 'string' || typeof request.type !== 'string') return;
    const url = privateRequestURL(request.url);
    if (!url) return;
    const key = `${request.tabId}:${request.requestId}:${rule.ruleId}`;
    const entry: ActivityEntry = {
      id: 0, timestamp: this.now(), tabId: request.tabId, site: isDomain(site) ? site : null, request: url,
      resourceType: request.type.slice(0, 40), ruleId: rule.ruleId,
      action: info?.action ?? 'unknown', source: (info?.source ?? 'Rule details unavailable').slice(0, 200),
      condition: (info?.condition ?? 'Rule configuration was changing when this match was observed.').slice(0, 1024)
    };
    if (!this.initialized) {
      this.pending.push({ entry, key });
      if (this.pending.length > ACTIVITY_LIMIT) this.pending.shift();
    } else this.append(entry, key);
  }
  private append(entry: ActivityEntry, key: string): void {
    if (this.seen.has(key)) return;
    this.seen.add(key);
    if (this.seen.size > ACTIVITY_LIMIT * 2) this.seen.delete(this.seen.values().next().value!);
    if (this.sequence >= Number.MAX_SAFE_INTEGER) this.sequence = 0;
    this.entries.push({ ...entry, id: ++this.sequence });
    if (this.entries.length > ACTIVITY_LIMIT) this.entries.shift();
    this.changed();
  }
  private changed(): void {
    this.dirty = true;
    if (this.timer === undefined) this.timer = setTimeout(() => { this.timer = undefined; void this.flush(); }, 250);
  }
  async view(tabId: number | null): Promise<ActivityEntry[]> {
    await this.ready;
    return this.entries.filter(entry => tabId === null || entry.tabId === tabId).map(entry => ({ ...entry })).reverse();
  }
  async retainTabs(tabIds: Set<number>): Promise<void> {
    await this.ready;
    const kept = this.entries.filter(entry => tabIds.has(entry.tabId));
    if (kept.length !== this.entries.length) { this.entries = kept; this.changed(); }
  }
  async removeTab(tabId: number): Promise<void> {
    await this.ready;
    const kept = this.entries.filter(entry => entry.tabId !== tabId);
    if (kept.length !== this.entries.length) { this.entries = kept; this.changed(); }
  }
  async clear(): Promise<void> {
    await this.ready;
    this.entries = [];
    // Retain deduplication keys so a delayed duplicate does not recreate a row.
    this.changed();
    await this.flush();
    if (this.storageError) throw new Error('Recent activity was cleared from memory, but session storage could not be cleared. Try again.');
  }
  async flush(): Promise<void> {
    await this.ready;
    if (this.timer !== undefined) { clearTimeout(this.timer); this.timer = undefined; }
    if (this.saving) return this.saving;
    this.saving = (async () => {
      while (this.dirty) {
        this.dirty = false;
        try {
          await this.storage.write({ version: 1, entries: this.entries.map(entry => ({ ...entry })) });
          this.storageError = false;
        } catch { this.storageError = true; break; }
      }
    })();
    try { await this.saving; } finally { this.saving = undefined; }
  }
}
