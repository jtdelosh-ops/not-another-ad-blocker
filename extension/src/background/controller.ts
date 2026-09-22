import { NativeClient, validateCompilation, validateSubscriptionState, validateSubscriptionIds } from '../shared/native-client';
import { diagnosticSamples } from '../shared/diagnostics';
import type { ActivityRuleInfo } from '../shared/activity';
import { MAX_SELECTORS, MAX_SUBSCRIPTION_NETWORK, MAX_TEXT_BYTES, OVERRIDE_ID, RecoveryRequiredError, disabledBy, emptyConfig, errorMessage, isDomain, isSafeSelector, matchesDomain, type Config, type ConfigView, type SubscriptionId, type SubscriptionProgress } from '../shared/types';

export type BrowserRule = chrome.declarativeNetRequest.Rule;
export interface Backend {
  read(): Promise<{ config?: unknown; pending?: unknown }>;
  write(values: Record<string, unknown>): Promise<void>;
  remove(key: string): Promise<void>;
  rules(): Promise<BrowserRule[]>;
  replace(removeRuleIds: number[], addRules: BrowserRule[]): Promise<void>;
  notify(): Promise<void>;
  maxDynamicRules: number;
}
export function validateConfig(value: unknown): Config {
  const config = value as Config;
  if (!config || config.version !== 1 || typeof config.enabled !== 'boolean' || !Array.isArray(config.disabledSites) || config.disabledSites.length > 200 || !config.disabledSites.every(isDomain) || typeof config.source !== 'string' || typeof config.text !== 'string' || (config.updatedAt !== null && typeof config.updatedAt !== 'string')) throw new Error('Stored configuration is invalid.');
  validateCompilation(config.compiled);
  if (new TextEncoder().encode(config.text).length > MAX_TEXT_BYTES || new TextEncoder().encode(config.source).length > 128) throw new Error('Stored local list exceeds its limits.');
  if (config.subscriptions !== undefined) validateSubscriptionState(config.subscriptions);
  return config;
}
export function configView(config: Config): ConfigView {
  const { compiled, subscriptions, ...settings } = config;
  return structuredClone({ ...settings, compiled: { stats: compiled.stats, diagnostics: diagnosticSamples(compiled.diagnostics) }, ...(subscriptions ? { subscriptions: { manifest: subscriptions.manifest, appliedAt: subscriptions.appliedAt, diagnostics: diagnosticSamples(subscriptions.compiled.diagnostics) } } : {}) });
}
export function desiredRules(config: Config): BrowserRule[] {
  if (!config.enabled) return [];
  // Stable array order gives globally unique IDs even when each compiler starts at 1.
  const rules = [...(config.subscriptions?.compiled.networkRules ?? []).map(rule => structuredClone(rule)), ...config.compiled.networkRules.map(rule => ({ ...structuredClone(rule), priority: rule.action.type === 'block' ? 3 : 4 }))].map((rule, index) => ({ ...rule, id: index + 1 })) as BrowserRule[];
  config.disabledSites.forEach((domain, index) => {
    // Main-frame allowance propagates to descendant frames after navigation.
    rules.push({ id: OVERRIDE_ID + index, priority: 100, action: { type: 'allowAllRequests' as chrome.declarativeNetRequest.RuleActionType }, condition: { requestDomains: [domain], resourceTypes: ['main_frame' as chrome.declarativeNetRequest.ResourceType] } });
  });
  return rules;
}
function boundedText(value: string, limit: number): string {
  return value.length > limit ? `${value.slice(0, limit - 1)}…` : value;
}
function describeCondition(condition: BrowserRule['condition'], priority: number): string {
  // Packed rules can contain 1,000 domains. Describe only a bounded sample;
  // this is the compiled condition, not a claim about an original filter line.
  const summary: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(condition)) {
    summary[key] = Array.isArray(value) && value.length > 3
      ? { sample: value.slice(0, 3), total: value.length }
      : value;
  }
  return boundedText(`Compiled rule (priority ${priority}): ${JSON.stringify(summary)}`, 1024);
}
export class Controller {
  private config: Config = emptyConfig();
  private queue: Promise<unknown> = Promise.resolve();
  private readonly ready: Promise<void>;
  private recoveryError: Error | undefined;
  private activityRulesReady = false;
  private refreshing = false;
  private subscriptionGeneration = 0;
  private progress: SubscriptionProgress = { phase: 'idle' };
  constructor(private backend: Backend, readonly native: NativeClient) {
    this.ready = this.initialize();
    // A startup failure is also returned to every subsequent caller.
    void this.ready.catch(() => {});
  }
  private async initialize(): Promise<void> {
    const stored = await this.backend.read();
    this.config = stored.config === undefined ? emptyConfig() : validateConfig(stored.config);
    // Always reconcile committed storage with DNR after service-worker restart.
    // If termination interrupted a write, uncommitted pending state is discarded.
    await this.applyRules(desiredRules(this.config));
    if (stored.pending !== undefined) await this.backend.remove('pending');
    this.activityRulesReady = true;
  }
  private serial<T>(action: () => Promise<T>): Promise<T> {
    const job = this.queue.then(async () => { await this.ready; if (this.recoveryError) throw this.recoveryError; return action(); });
    this.queue = job.catch(() => {});
    return job;
  }
  private async applyRules(rules: BrowserRule[]): Promise<void> {
    if (rules.length > this.backend.maxDynamicRules) throw new Error(`This configuration needs ${rules.length} network rules; this browser allows ${this.backend.maxDynamicRules}. Existing rules were kept.`);
    const old = await this.backend.rules();
    // Chrome validates the whole update atomically, preserving old rules on failure.
    await this.backend.replace(old.map(rule => rule.id), rules);
  }
  private async commit(next: Config): Promise<Config> {
    const previous = structuredClone(this.config);
    // IDs are reused when lists change. Never attribute an event against the
    // old config while Chrome and committed storage are being reconciled.
    this.activityRulesReady = false;
    try {
      await this.backend.write({ pending: next });
      try { await this.applyRules(desiredRules(next)); }
      catch (error) {
        await this.backend.remove('pending').catch(() => {});
        throw new Error(`Browser rule update failed: ${errorMessage(error)}`);
      }
      try { await this.backend.write({ config: next }); }
      catch (error) {
        try {
          await this.applyRules(desiredRules(previous));
          await this.backend.remove('pending');
        } catch (rollback) {
          this.recoveryError = new RecoveryRequiredError(`Settings could not be saved and rule rollback failed. Protection state is unknown; reload the extension to recover. ${errorMessage(rollback)}`);
          throw this.recoveryError;
        }
        throw new Error(`Settings could not be saved; previous rules restored. ${errorMessage(error)}`);
      }
      this.config = next;
      await this.backend.remove('pending').catch(() => {});
      await this.backend.notify().catch(() => {});
      return structuredClone(this.config);
    } finally {
      // Ordinary failures leave/restore the previous rules; failed rollback
      // latches recoveryError and keeps provenance unavailable until restart.
      this.activityRulesReady = !this.recoveryError;
    }
  }
  describeActivityRule(ruleId: number): ActivityRuleInfo | null {
    if (!this.activityRulesReady || this.recoveryError || !this.config.enabled || !Number.isSafeInteger(ruleId) || ruleId < 1) return null;
    if (ruleId >= OVERRIDE_ID) {
      const domain = this.config.disabledSites[ruleId - OVERRIDE_ID];
      return domain === undefined ? null : {
        action: 'allowAllRequests', source: 'Site exception',
        condition: describeCondition({ requestDomains: [domain], resourceTypes: ['main_frame' as chrome.declarativeNetRequest.ResourceType] }, 100)
      };
    }
    const subscriptions = this.config.subscriptions;
    const subscriptionCount = subscriptions?.compiled.networkRules.length ?? 0;
    const subscription = ruleId <= subscriptionCount;
    const rule = subscription
      ? subscriptions!.compiled.networkRules[ruleId - 1]
      : this.config.compiled.networkRules[ruleId - subscriptionCount - 1];
    if (!rule) return null;
    const source = subscription
      ? `${subscriptions!.manifest.lists.map(list => list.title).join(' + ')} subscriptions`
      : `Local list: ${this.config.source}`;
    return {
      action: rule.action.type,
      source: boundedText(source, 200),
      condition: describeCondition(rule.condition as BrowserRule['condition'], subscription ? rule.priority : rule.action.type === 'block' ? 3 : 4)
    };
  }
  snapshot(): Promise<Config> { return this.serial(async () => structuredClone(this.config)); }
  view(): Promise<ConfigView> { return this.serial(async () => configView(this.config)); }
  subscriptionProgress(): SubscriptionProgress { return structuredClone(this.progress); }
  async refreshSubscriptions(ids: SubscriptionId[]): Promise<Config> {
    ids = validateSubscriptionIds(ids);
    if (this.refreshing) throw new Error('A subscription refresh is already running.');
    this.refreshing = true;
    const generation = ++this.subscriptionGeneration;
    try {
      await this.ready;
      if (this.recoveryError) throw this.recoveryError;
      // Reserve capacity for all 2,000 local rules and 200 per-site allowances.
      const budget = Math.min(MAX_SUBSCRIPTION_NETWORK, Math.max(0, this.backend.maxDynamicRules - 2_200));
      const subscriptions = await this.native.refresh(ids, budget, progress => { this.progress = progress; });
      return await this.serial(async () => {
        if (generation !== this.subscriptionGeneration) throw new Error('Subscription refresh was cancelled by a newer subscription change.');
        this.progress = { phase: 'applying' };
        const next = await this.commit({ ...this.config, subscriptions: { ...subscriptions, appliedAt: new Date().toISOString() } });
        this.progress = { phase: 'complete' };
        return next;
      });
    } catch (error) { this.progress = { phase: 'failed', message: errorMessage(error) }; throw error; }
    finally { this.refreshing = false; }
  }
  removeSubscriptions(): Promise<Config> {
    return this.serial(async () => {
      ++this.subscriptionGeneration;
      const { subscriptions: _subscriptions, ...next } = this.config;
      return this.commit(next);
    });
  }
  import(text: string, source: string): Promise<Config> {
    return this.serial(async () => {
      const compiled = await this.native.compile(text, source);
      return this.commit({ ...this.config, text, source, compiled, updatedAt: new Date().toISOString() });
    });
  }
  addPickerRule(host: string, selector: string, token: string): Promise<{ rule: string; added: boolean }> {
    return this.serial(async () => {
      const { rule, block } = pickerBlock(host, selector, token);
      if (!this.config.enabled || disabledBy(host, this.config.disabledSites)) throw new Error('Protection is paused for this site. Enable it before saving.');
      if (this.config.text.includes(block)) return { rule, added: true };
      if (this.config.compiled.cosmeticRules.some(item => item.selector === selector && (!item.domains.length || item.domains.some(domain => matchesDomain(host, domain))))) return { rule, added: false };
      const text = this.config.text + block;
      const source = this.config.updatedAt ? this.config.source : 'Local filters';
      const compiled = await this.native.compile(text, source);
      if (!compiled.cosmeticRules.some(item => item.domains.includes(host) && item.selector === selector)) throw new Error('The companion does not support this rule. Update it before saving.');
      await this.commit({ ...this.config, text, source, compiled, updatedAt: new Date().toISOString() });
      return { rule, added: true };
    });
  }
  removePickerRule(host: string, selector: string, token: string): Promise<void> {
    return this.serial(async () => {
      const { block } = pickerBlock(host, selector, token);
      if (!this.config.text.includes(block)) throw new Error('This rule was edited or removed. Manage it in Lists & diagnostics.');
      // Remove only this picker's exact marked addition, preserving concurrent
      // imports, other picker rules, subscriptions and protection settings.
      const text = this.config.text.replace(block, '');
      const compiled = await this.native.compile(text, this.config.source);
      await this.commit({ ...this.config, text, compiled, updatedAt: new Date().toISOString() });
    });
  }
  setEnabled(enabled: boolean): Promise<Config> {
    return this.serial(async () => this.commit({ ...this.config, enabled }));
  }
  setSite(host: string, enabled: boolean): Promise<Config> {
    return this.serial(async () => {
      if (!isDomain(host)) throw new Error('This page does not have a supported HTTP(S) hostname.');
      let sites = [...this.config.disabledSites];
      if (enabled) sites = sites.filter(site => site !== host);
      else if (!sites.includes(host)) sites.push(host);
      if (sites.length > 200) throw new Error('The preview supports at most 200 site exceptions.');
      return this.commit({ ...this.config, disabledSites: sites.sort() });
    });
  }
  cosmetics(host: string): Promise<{ selectors: string[] }> {
    return this.serial(async () => ({ selectors: !isDomain(host) || !this.config.enabled || disabledBy(host, this.config.disabledSites) ? [] : [...new Set([...this.config.compiled.cosmeticRules, ...(this.config.subscriptions?.compiled.cosmeticRules ?? [])].filter(rule => (rule.domains.length === 0 || rule.domains.some(domain => matchesDomain(host, domain))) && !rule.excludedDomains?.some(domain => matchesDomain(host, domain))).map(rule => rule.selector))].slice(0, MAX_SELECTORS) }));
  }
}

function pickerBlock(host: string, selector: string, token: string): { rule: string; block: string } {
  if (!isDomain(host) || !isSafeSelector(selector) || !/[.#]/.test(selector) || !/^[a-f0-9-]{36}$/.test(token)) throw new Error('Invalid picker rule.');
  const rule = `${host}##${selector}`;
  return { rule, block: `\n! NAAB picker ${token}\n${rule}\n` };
}
