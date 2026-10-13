import { RecoveryRequiredError, disabledBy, errorMessage, hostname, ruleCounts, type CompanionStatus, type ConfigView } from '../shared/types';
import { element, renderRecoveryRequired, request } from '../shared/ui';
import type { ActivityView } from '../shared/activity';
let config: ConfigView | null = null;
let recoveryRequired = false;
let mutationPending = false;
let host: string | null = null;
let activeTabId: number | null = null;
const globalToggle = element<HTMLInputElement>('global');
const siteToggle = element<HTMLInputElement>('site-toggle');
const pickerButton = element<HTMLButtonElement>('picker');
const retryConfigButton = element<HTMLButtonElement>('retry-config');
let configLoadGeneration = 0;
const REQUEST_TIMEOUT_MS = 10_000;
class RequestTimedOutError extends Error {}
async function requestWithTimeout<T>(message: unknown, timeoutMessage: string): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([
      request<T>(message),
      new Promise<never>((_, reject) => {
        timer = setTimeout(() => reject(new RequestTimedOutError(timeoutMessage)), REQUEST_TIMEOUT_MS);
      })
    ]);
  } finally {
    if (timer !== undefined) clearTimeout(timer);
  }
}
function render(): void {
  if (recoveryRequired) {
    pickerButton.disabled = true;
    renderRecoveryRequired(element('protection'), [globalToggle, siteToggle]);
    element('counts').textContent = '';
    return;
  }
  if (!config) return;
  const exception = host ? disabledBy(host, config.disabledSites) : undefined;
  const counts = ruleCounts(config);
  const total = counts.network + counts.cosmetic;
  element('site').textContent = host ?? 'Browser or unsupported page';
  element('protection').textContent = !config.enabled ? 'Protection paused globally' : exception ? 'Protection paused for this site' : total === 0 ? 'No rules loaded — nothing is blocked' : 'Protection enabled';
  element('counts').textContent = `${counts.network.toLocaleString()} network · ${counts.cosmetic.toLocaleString()} cosmetic rules loaded`;
  globalToggle.checked = config.enabled;
  globalToggle.disabled = mutationPending;
  siteToggle.checked = !exception;
  siteToggle.disabled = mutationPending || !host || !config.enabled;
  pickerButton.disabled = mutationPending || !host || !config.enabled || !!exception || activeTabId === null;
  element('site-label').textContent = exception ? `Resume ${exception}` : 'Protection for this site';
  element('site-note').textContent = host ? 'Site settings include subdomains. Reload the page after a change.' : 'Site controls and cosmetic filtering apply to HTTP(S) pages. Browser pages cannot be filtered.';
}
async function change(message: unknown): Promise<void> {
  if (recoveryRequired || mutationPending || !config) { render(); return; }
  mutationPending = true;
  globalToggle.disabled = siteToggle.disabled = pickerButton.disabled = true;
  element('error').textContent = '';
  try {
    config = await requestWithTimeout<ConfigView>(message, 'The extension did not confirm the protection change in time. Reload NAAB to verify its state.');
  } catch (error) {
    recoveryRequired ||= error instanceof RecoveryRequiredError || error instanceof RequestTimedOutError;
    element('error').textContent = errorMessage(error);
  } finally {
    mutationPending = false;
    render();
  }
}
globalToggle.addEventListener('change', () => void change({ type: 'config.enabled', enabled: globalToggle.checked }));
siteToggle.addEventListener('change', () => { if (host && config) void change({ type: 'config.site', host: disabledBy(host, config.disabledSites) ?? host, enabled: siteToggle.checked }); });
retryConfigButton.addEventListener('click', () => void loadConfig());
element('options').addEventListener('click', () => void chrome.runtime.openOptionsPage());
pickerButton.addEventListener('click', () => {
  pickerButton.disabled = true;
  void request({ type: 'picker.start', tabId: activeTabId }).then(() => window.close(), error => {
    element('error').textContent = errorMessage(error); render();
  });
});
element('activity').addEventListener('click', () => {
  void chrome.tabs.create({ url: chrome.runtime.getURL('activity.html') + (activeTabId === null ? '' : `#tab=${activeTabId}`) }).catch(error => { element('error').textContent = errorMessage(error); });
});
async function loadPageCount(tabId: number | null): Promise<void> {
  if (tabId === null) { element('page-count').textContent = 'Unavailable'; return; }
  try {
    const activity = await request<ActivityView>({ type: 'activity.get', tabId });
    element('page-count').textContent = activity.blockedCount ?? 'Unavailable';
    element('page-count-note').textContent = activity.error ?? 'Current page only. Cosmetically hidden elements are not counted.';
  } catch {
    element('page-count').textContent = 'Unavailable';
    element('page-count-note').textContent = 'The browser could not provide a page counter. Protection controls remain available.';
  }
}
async function loadConfig(): Promise<void> {
  const generation = ++configLoadGeneration;
  retryConfigButton.disabled = true;
  retryConfigButton.hidden = true;
  globalToggle.disabled = siteToggle.disabled = pickerButton.disabled = true;
  element('protection').textContent = 'Checking protection.';
  element('error').textContent = '';
  try {
    const saved = await requestWithTimeout<ConfigView>({ type: 'config.get' }, 'The extension did not load protection settings in time. Try again or reload NAAB from the browser extensions page.');
    if (generation !== configLoadGeneration) return;
    config = saved;
    render();
  } catch (error) {
    if (generation !== configLoadGeneration) return;
    recoveryRequired ||= error instanceof RecoveryRequiredError;
    element('error').textContent = errorMessage(error);
    if (recoveryRequired) render();
    else {
      element('protection').textContent = 'Protection settings could not be loaded';
      globalToggle.disabled = siteToggle.disabled = pickerButton.disabled = true;
      retryConfigButton.hidden = false;
    }
  } finally {
    if (generation === configLoadGeneration) retryConfigButton.disabled = false;
  }
}

void loadConfig();
// The global switch only needs the saved config. Do not couple it to active-tab
// discovery: a stuck browser tab query must not leave all popup controls loading.
void chrome.tabs.query({ active: true, currentWindow: true }).then(tabs => {
  activeTabId = Number.isInteger(tabs[0]?.id) && tabs[0].id! >= 0 ? tabs[0].id! : null;
  host = hostname(tabs[0]?.url);
  render();
  void loadPageCount(activeTabId);
}, () => {
  activeTabId = null;
  host = null;
  if (config) render();
});
void request<CompanionStatus>({ type: 'status.get' }).then(status => { element('companion').textContent = status.healthy ? `Companion ${status.companionVersion} · ${status.capabilities.includes('lists.refresh') && status.capabilities.includes('lists.page') ? 'ready for subscriptions' : 'update companion for subscriptions'}` : 'Companion reported unhealthy'; }, () => { element('companion').textContent = 'Companion unavailable. Saved rules still work; open settings for setup.'; });
