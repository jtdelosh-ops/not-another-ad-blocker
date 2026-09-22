import { RecoveryRequiredError, disabledBy, errorMessage, hostname, ruleCounts, type CompanionStatus, type ConfigView } from '../shared/types';
import { element, renderRecoveryRequired, request } from '../shared/ui';
let config: ConfigView;
let recoveryRequired = false;
let host: string | null = null;
const globalToggle = element<HTMLInputElement>('global');
const siteToggle = element<HTMLInputElement>('site-toggle');
function render(): void {
  if (recoveryRequired) {
    renderRecoveryRequired(element('protection'), [globalToggle, siteToggle]);
    element('counts').textContent = '';
    return;
  }
  const exception = host ? disabledBy(host, config.disabledSites) : undefined;
  const counts = ruleCounts(config);
  const total = counts.network + counts.cosmetic;
  element('site').textContent = host ?? 'Browser or unsupported page';
  element('protection').textContent = !config.enabled ? 'Protection paused globally' : exception ? 'Protection paused for this site' : total === 0 ? 'No rules loaded — nothing is blocked' : 'Protection enabled';
  element('counts').textContent = `${counts.network.toLocaleString()} network · ${counts.cosmetic.toLocaleString()} cosmetic rules loaded`;
  globalToggle.checked = config.enabled;
  globalToggle.disabled = false;
  siteToggle.checked = !exception;
  siteToggle.disabled = !host || !config.enabled;
  element('site-label').textContent = exception ? `Resume ${exception}` : 'Protection for this site';
  element('site-note').textContent = host ? 'Site settings include subdomains. Reload the page after a change.' : 'Site controls and cosmetic filtering apply to HTTP(S) pages. Browser pages cannot be filtered.';
}
async function change(message: unknown): Promise<void> {
  if (recoveryRequired) return;
  globalToggle.disabled = siteToggle.disabled = true;
  element('error').textContent = '';
  try { config = await request<ConfigView>(message); } catch (error) { recoveryRequired ||= error instanceof RecoveryRequiredError; element('error').textContent = errorMessage(error); }
  finally { if (config || recoveryRequired) render(); }
}
globalToggle.addEventListener('change', () => void change({ type: 'config.enabled', enabled: globalToggle.checked }));
siteToggle.addEventListener('change', () => { if (host) void change({ type: 'config.site', host: disabledBy(host, config.disabledSites) ?? host, enabled: siteToggle.checked }); });
element('options').addEventListener('click', () => void chrome.runtime.openOptionsPage());
void (async () => {
  try {
    const [saved, tabs] = await Promise.all([request<ConfigView>({ type: 'config.get' }), chrome.tabs.query({ active: true, currentWindow: true })]);
    config = saved;
    host = hostname(tabs[0]?.url);
    render();
  } catch (error) { recoveryRequired ||= error instanceof RecoveryRequiredError; element('error').textContent = errorMessage(error); if (recoveryRequired) render(); else element('protection').textContent = 'Protection state could not be verified'; }
})();
void request<CompanionStatus>({ type: 'status.get' }).then(status => { element('companion').textContent = status.healthy ? `Companion ${status.companionVersion} · ${status.capabilities.includes('lists.refresh') && status.capabilities.includes('lists.page') ? 'ready for subscriptions' : 'update companion for subscriptions'}` : 'Companion reported unhealthy'; }, () => { element('companion').textContent = 'Companion unavailable. Saved rules still work; open settings for setup.'; });
