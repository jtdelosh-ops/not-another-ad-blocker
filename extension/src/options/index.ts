import { MAX_TEXT_BYTES, MAX_DIAGNOSTICS, RecoveryRequiredError, errorMessage, ruleCounts, type CompanionStatus, type ConfigView, type SubscriptionId, type SubscriptionProgress } from '../shared/types';
import { element, pollSubscriptionProgress, renderRecoveryRequired, request } from '../shared/ui';
import { diagnosticSamples } from '../shared/diagnostics';
let config: ConfigView;
let recoveryRequired = false;
let isBusy = false;
let subscriptionsAvailable = false;
const editor = element<HTMLTextAreaElement>('filters');
const source = element<HTMLInputElement>('source-name');
const globalToggle = element<HTMLInputElement>('global');
const format = (value: number) => value.toLocaleString();
function refreshControls(): void {
  element<HTMLButtonElement>('refresh-lists').disabled = isBusy || recoveryRequired || !config || !subscriptionsAvailable;
  element<HTMLButtonElement>('remove-lists').disabled = isBusy || recoveryRequired || !config?.subscriptions;
}
function render(): void {
  if (recoveryRequired) { renderRecovery(); return; }
  const counts = ruleCounts(config);
  const stats = config.compiled.stats;
  const total = counts.network + counts.cosmetic;
  globalToggle.checked = config.enabled;
  globalToggle.disabled = isBusy;
  element('summary').textContent = !config.enabled ? `Protection paused globally · ${format(total)} rules saved` : total === 0 ? 'No rules loaded — nothing is blocked' : `${format(counts.network)} network rules and ${format(counts.cosmetic)} cosmetic rules enabled`;
  element('source').textContent = config.updatedAt ? `Local list: ${config.source} · imported ${new Date(config.updatedAt).toLocaleString()}` : 'No local list imported. Optional subscriptions are managed below.';
  const sites = element('sites');
  sites.replaceChildren();
  if (!config.disabledSites.length) { const item = document.createElement('li'); item.textContent = 'No site exceptions.'; sites.append(item); }
  for (const host of config.disabledSites) {
    const item = document.createElement('li');
    const label = document.createElement('span'); label.textContent = host;
    const button = document.createElement('button'); button.textContent = 'Resume protection'; button.disabled = isBusy;
    button.addEventListener('click', () => void update({ type: 'config.site', host, enabled: true }));
    item.append(label, button); sites.append(item);
  }
  renderSubscriptions();
  const subscription = config.subscriptions;
  const samples = diagnosticSamples(config.compiled.diagnostics, subscription?.diagnostics ?? []);
  const unsupported = stats.unsupported + (subscription?.manifest.stats.unsupported ?? 0);
  element('diagnostic-summary').textContent = `${format(unsupported)} unsupported lines across saved lists. Showing ${samples.length} diagnostic samples (maximum ${MAX_DIAGNOSTICS}). Local import: ${format(stats.network)} network · ${format(stats.cosmetic)} cosmetic · ${format(stats.unsupported)} unsupported · ${format(stats.ignored)} ignored. These are compilation results, not browsing activity.`;
  const rows = document.createDocumentFragment();
  for (const diagnostic of samples) {
    const row = document.createElement('tr');
    for (const value of [diagnostic.source ?? config.source, diagnostic.line, diagnostic.target, diagnostic.raw, diagnostic.message]) { const cell = document.createElement('td'); cell.textContent = String(value); row.append(cell); }
    rows.append(row);
  }
  element('diagnostics').replaceChildren(rows);
  refreshControls();
}
function renderSubscriptions(): void {
  const state = config.subscriptions;
  const metadata = element('list-metadata'); metadata.replaceChildren();
  const reasons = element('unsupported-reasons'); reasons.replaceChildren();
  if (!state) { element('list-summary').textContent = 'No subscriptions installed. Select the lists you want and download them once to begin.'; return; }
  const { manifest } = state;
  const coverage = manifest.coverage;
  element('list-summary').textContent = `${format(manifest.stats.network)} network rules · ${format(manifest.stats.cosmetic)} cosmetic rules saved. ${config.enabled ? 'Applied' : 'Saved while protection is paused'} ${new Date(state.appliedAt).toLocaleString()}. ${format(manifest.stats.unsupported)} unsupported lines · ${format(coverage.networkDropped)} network and ${format(coverage.cosmeticDropped)} cosmetic rules deferred by limits · ${format(coverage.exceptionSafetySuppressed)} omitted for exception safety. Simple domain filters may share a network rule. Cosmetic duplicates and exceptions are resolved before applying.`;
  for (const list of manifest.lists) {
    const card = document.createElement('div'); card.className = 'list-metadata';
    const name = document.createElement('h3'); name.textContent = list.title;
    const summary = document.createElement('p'); summary.className = 'small muted';
    summary.textContent = `Fetched ${new Date(list.fetchedAt).toLocaleString()} · ${format(list.bytes)} bytes${list.version ? ` · source version ${list.version}` : ''}`;
    const link = document.createElement('a'); link.href = list.url; link.textContent = list.url; link.target = '_blank'; link.rel = 'noreferrer';
    const hash = document.createElement('p'); hash.className = 'small muted hash'; hash.textContent = `SHA-256: ${list.sha256}`;
    const stats = manifest.listStats.find(item => item.id === list.id)!;
    const sourceStats = document.createElement('p'); sourceStats.className = 'small muted'; sourceStats.textContent = `Source lines: ${format(stats.network)} supported network · ${format(stats.cosmetic)} supported cosmetic · ${format(stats.unsupported)} unsupported · ${format(stats.ignored)} ignored.`;
    card.append(name, summary, link, hash, sourceStats); metadata.append(card);
  }
  for (const group of manifest.unsupportedReasons) { const item = document.createElement('li'); item.textContent = `${format(group.count)} · ${group.reason}`; reasons.append(item); }
}
function busy(value: boolean): void { isBusy = value; document.querySelectorAll<HTMLButtonElement | HTMLInputElement | HTMLTextAreaElement>('button, input, textarea').forEach(control => { control.disabled = value; }); refreshControls(); }
function renderRecovery(): void {
  renderRecoveryRequired(element('summary'), Array.from(document.querySelectorAll<HTMLButtonElement | HTMLInputElement | HTMLTextAreaElement>('button:not(#check), input, textarea')));
  element('message').textContent = '';
}
function renderProgress(progress: SubscriptionProgress): void {
  const stages: Record<SubscriptionProgress['phase'], string> = {
    idle: '', checking: 'Checking subscription support in the local companion…', downloading: 'Downloading selected lists over HTTPS and compiling locally…', network: 'Validating network rules', cosmetic: 'Validating cosmetic rules', diagnostics: 'Validating diagnostics', applying: 'Saving and applying validated rules…', complete: 'Subscriptions saved. Reload open pages to apply all changes.', failed: progress.message ?? 'Refresh failed. Previously saved rules remain active.'
  };
  element('list-progress').textContent = stages[progress.phase] + (progress.total !== undefined ? ` · ${format(progress.completed ?? 0)} / ${format(progress.total)}` : '');
}
async function update(message: unknown): Promise<void> {
  if (recoveryRequired || isBusy || !config) return;
  busy(true); element('error').textContent = ''; element('message').textContent = '';
  const refreshing = (message as any)?.type === 'subscriptions.refresh';
  let stopPolling: (() => void) | undefined;
  if (refreshing) {
    renderProgress({ phase: 'checking' });
    // Poll actual completed work. No invented progress percentage or ETA.
    stopPolling = pollSubscriptionProgress(() => request<SubscriptionProgress>({ type: 'subscriptions.progress' }), renderProgress);
  }
  try {
    config = await request<ConfigView>(message); stopPolling?.(); render(); element('message').textContent = 'Saved. Reload open pages to fully apply the change.';
    if (refreshing) renderProgress({ phase: 'complete' });
    if ((message as any)?.type === 'subscriptions.remove') element('list-progress').textContent = 'Subscriptions removed. Local rules and protection settings were preserved.';
  } catch (error) {
    stopPolling?.();
    recoveryRequired ||= error instanceof RecoveryRequiredError;
    element('error').textContent = errorMessage(error);
    if (refreshing) renderProgress({ phase: 'failed', message: errorMessage(error) });
    if (config || recoveryRequired) render();
  } finally { stopPolling?.(); busy(false); if (recoveryRequired) renderRecovery(); }
}
async function checkCompanion(): Promise<void> {
  element<HTMLButtonElement>('check').disabled = true;
  element('companion').textContent = 'Checking local companion…';
  try {
    const status = await request<CompanionStatus>({ type: 'status.get' });
    subscriptionsAvailable = status.healthy && status.capabilities.includes('lists.refresh') && status.capabilities.includes('lists.page');
    element('companion').textContent = status.healthy ? `Local companion ${status.companionVersion} is ready. ${subscriptionsAvailable ? 'EasyList/EasyPrivacy downloads are available.' : 'Update the companion to 0.2.0 or later for subscriptions; local imports still work.'}` : 'The companion reported unhealthy.';
  } catch (error) { subscriptionsAvailable = false; element('companion').textContent = `Companion unavailable: ${errorMessage(error)} Install/register com.naab.companion using the README, then check again. Saved rules continue working.`; }
  finally { element<HTMLButtonElement>('check').disabled = isBusy; refreshControls(); }
}
element('check').addEventListener('click', () => void checkCompanion());
globalToggle.addEventListener('change', () => void update({ type: 'config.enabled', enabled: globalToggle.checked }));
element('import').addEventListener('click', () => void update({ type: 'config.import', text: editor.value, source: source.value.trim() || 'Local import' }));
element('refresh-lists').addEventListener('click', () => {
  const ids = (['easylist', 'easyprivacy'] as SubscriptionId[]).filter(id => element<HTMLInputElement>(id).checked);
  if (!ids.length) { element('error').textContent = 'Select EasyList, EasyPrivacy, or both.'; return; }
  void update({ type: 'subscriptions.refresh', ids });
});
element('remove-lists').addEventListener('click', () => void update({ type: 'subscriptions.remove' }));
element('demo').addEventListener('click', () => {
  void fetch(chrome.runtime.getURL('demo-filters.txt')).then(response => response.text()).then(text => { editor.value = text; source.value = 'Bundled demo'; element('message').textContent = 'Demo loaded into editor. Compile & replace local rules to activate it.'; }).catch(error => { element('error').textContent = errorMessage(error); });
});
element<HTMLInputElement>('file').addEventListener('change', event => {
  const file = (event.target as HTMLInputElement).files?.[0];
  if (!file) return;
  if (file.size > MAX_TEXT_BYTES) { element('error').textContent = 'Local imports accept files up to 128 KiB.'; return; }
  void file.text().then(text => { editor.value = text; source.value = file.name; element('error').textContent = ''; element('message').textContent = 'File loaded into editor. Compile & replace local rules to activate it.'; }).catch(error => { element('error').textContent = errorMessage(error); });
});
void request<ConfigView>({ type: 'config.get' }).then(saved => {
  config = saved; editor.value = saved.text; source.value = saved.updatedAt ? saved.source : 'Local import';
  for (const id of ['easylist', 'easyprivacy'] as const) element<HTMLInputElement>(id).checked = saved.subscriptions ? saved.subscriptions.manifest.lists.some(list => list.id === id) : true;
  render();
}, error => { recoveryRequired ||= error instanceof RecoveryRequiredError; element('error').textContent = errorMessage(error); if (recoveryRequired) renderRecovery(); else element('summary').textContent = 'Protection state could not be verified'; });
void checkCompanion();
