import { Controller, type Backend } from './controller';
import { createRouter } from './routes';
import { NativeClient } from '../shared/native-client';
import { RecoveryRequiredError, errorMessage, hostname } from '../shared/types';
import { ActivityLog, blockedCountText, supportsBlockedCount } from './activity';
import { ACTIVITY_LIMIT, type ActivityAccess } from '../shared/activity';
import { Picker, type PickerGrant } from './picker';

const storageReady = chrome.storage.local.setAccessLevel({ accessLevel: 'TRUSTED_CONTEXTS' });
const sessionReady = chrome.storage.session.setAccessLevel({ accessLevel: 'TRUSTED_CONTEXTS' });
const activityLog = new ActivityLog({
  async read() { await sessionReady; return (await chrome.storage.session.get('activity')).activity; },
  async write(value) { await sessionReady; await chrome.storage.session.set({ activity: value }); }
});
const knownTabs = new Map<number, chrome.tabs.Tab>();
let initializingTabs = true;
const tabsTouchedDuringStartup = new Set<number>();
const touchedTab = (id: number): void => { if (initializingTabs) tabsTouchedDuringStartup.add(id); };
const rememberTab = (tab: chrome.tabs.Tab): void => {
  if (tab.id === undefined) return;
  touchedTab(tab.id);
  if (tab.incognito) { knownTabs.delete(tab.id); void activityLog.removeTab(tab.id); }
  else knownTabs.set(tab.id, tab);
};
chrome.tabs.onCreated.addListener(rememberTab);
chrome.tabs.onUpdated.addListener((_id, _change, tab) => rememberTab(tab));
chrome.tabs.onRemoved.addListener(id => { touchedTab(id); knownTabs.delete(id); void activityLog.removeTab(id); });
chrome.tabs.onReplaced.addListener((added, removed) => {
  touchedTab(removed);
  knownTabs.delete(removed);
  void activityLog.removeTab(removed);
  void chrome.tabs.get(added).then(rememberTab, () => {});
});
const tabsReady = chrome.tabs.query({}).then(async tabs => {
  // Do not resurrect a closed/private tab or overwrite a newer navigation with
  // the asynchronous startup query's earlier snapshot.
  tabs.filter(tab => tab.id !== undefined && !tabsTouchedDuringStartup.has(tab.id)).forEach(rememberTab);
  await activityLog.retainTabs(new Set(knownTabs.keys()));
}).finally(() => { initializingTabs = false; tabsTouchedDuringStartup.clear(); });
void tabsReady.catch(() => {});
let blockCounterSafe = false;
const counterReady = chrome.declarativeNetRequest.setExtensionActionOptions({ displayActionCountAsBadgeText: true }).then(() => true, () => false);
const backend: Backend = {
  async read() { await storageReady; return chrome.storage.local.get(['config', 'pending']); },
  async write(values) { await chrome.storage.local.set(values); },
  async remove(key) { await chrome.storage.local.remove(key); },
  rules: () => chrome.declarativeNetRequest.getDynamicRules(),
  async replace(removeRuleIds, addRules) {
    await chrome.declarativeNetRequest.updateDynamicRules({ removeRuleIds, addRules });
    blockCounterSafe = supportsBlockedCount(addRules);
  },
  maxDynamicRules: (chrome.declarativeNetRequest as typeof chrome.declarativeNetRequest & { MAX_NUMBER_OF_DYNAMIC_RULES?: number }).MAX_NUMBER_OF_DYNAMIC_RULES ?? 5_000,
  async notify() {
    const tabs = await chrome.tabs.query({ url: ['http://*/*', 'https://*/*'] });
    await Promise.allSettled(tabs.filter(tab => tab.id !== undefined).map(tab => chrome.tabs.sendMessage(tab.id!, { type: 'cosmetics.refresh' }, { frameId: 0 })));
  }
};
const native = new NativeClient((host, message) => chrome.runtime.sendNativeMessage(host, message as object));
const controller = new Controller(backend, native);
let detailedActivityAvailable = false;
try {
  if (chrome.declarativeNetRequest.onRuleMatchedDebug) {
    chrome.declarativeNetRequest.onRuleMatchedDebug.addListener(match => {
      const tab = knownTabs.get(match.request.tabId);
      if (!tab || tab.incognito || !hostname(tab.url)) return;
      // Capture metadata now: queued lookups could misattribute a reused rule ID.
      activityLog.record(match, controller.describeActivityRule(match.rule.ruleId), hostname(tab.url));
    });
    detailedActivityAvailable = true;
  }
} catch { /* Packed builds do not expose the unpacked debug event. */ }
const activity: ActivityAccess = {
  async get(tabId) {
    await tabsReady.catch(() => {});
    let site: string | null = null;
    let blockedCount: string | null = null;
    if (tabId !== null) {
      try {
        const tab = await chrome.tabs.get(tabId);
        rememberTab(tab);
        if (!tab.incognito) site = hostname(tab.url);
        if (site && blockCounterSafe && await counterReady) blockedCount = blockedCountText(await chrome.action.getBadgeText({ tabId }));
      } catch { knownTabs.delete(tabId); await activityLog.removeTab(tabId); }
    }
    return {
      available: detailedActivityAvailable, limit: ACTIVITY_LIMIT,
      entries: await activityLog.view(tabId), tabId, site, blockedCount,
      ...(activityLog.storageError ? { error: 'Session storage is unavailable. Recent activity may be lost when the extension worker sleeps.' } : {})
    };
  },
  clear: () => activityLog.clear()
};
const picker = new Picker(controller, {
  tab: id => chrome.tabs.get(id),
  async read(id) { await sessionReady; return (await chrome.storage.session.get(`picker:${id}`))[`picker:${id}`] as PickerGrant | undefined; },
  async write(id, grant) { await sessionReady; await chrome.storage.session.set({ [`picker:${id}`]: grant }); },
  async remove(id) { await sessionReady; await chrome.storage.session.remove(`picker:${id}`); },
  start: (id, token) => chrome.tabs.sendMessage(id, { type: 'picker.start', token }, { frameId: 0 })
});
chrome.tabs.onRemoved.addListener(id => { void sessionReady.then(() => chrome.storage.session.remove(`picker:${id}`)).catch(() => {}); });
const route = createRouter(controller, chrome.runtime.id, chrome.runtime.getURL('/'), activity, picker);
chrome.runtime.onMessage.addListener((message: unknown, sender, respond) => {
  void route(message, sender).then(payload => respond({ ok: true, payload }), error => respond({ ok: false, error: errorMessage(error), code: error instanceof RecoveryRequiredError ? error.code : undefined }));
  return true;
});
