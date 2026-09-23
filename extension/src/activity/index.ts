import type { ActivityAction, ActivityEntry, ActivityView } from '../shared/activity';
import { errorMessage } from '../shared/types';
import { element, request } from '../shared/ui';

export function mountActivityPage(): void {
  const scope = element<HTMLSelectElement>('activity-scope');
  const refreshButton = element<HTMLButtonElement>('refresh');
  const clearButton = element<HTMLButtonElement>('clear');
  const currentTabOption = element<HTMLOptionElement>('current-tab-option');
  let sourceTabId: number | null = null;
  let requestSequence = 0;
  let canLog = false;

  function readScope(): number | null {
    const parameters = new URLSearchParams(location.hash.slice(1));
    const raw = parameters.get('tab');
    const candidate = raw !== null && /^\d+$/.test(raw) ? Number(raw) : NaN;
    sourceTabId = Number.isSafeInteger(candidate) && candidate >= 0 && candidate <= 2_147_483_647 ? candidate : null;
    currentTabOption.disabled = sourceTabId === null;
    scope.value = sourceTabId !== null && parameters.get('scope') !== 'all' ? 'tab' : 'all';
    return scope.value === 'tab' ? sourceTabId : null;
  }

  function busy(value: boolean): void {
    refreshButton.disabled = scope.disabled = value;
    clearButton.disabled = value || !canLog;
    element('activity-table-wrap').setAttribute('aria-busy', String(value));
  }

  function actionLabel(action: ActivityAction): string {
    return action === 'block' ? 'Blocked' : action === 'allow' || action === 'allowAllRequests' ? 'Exception matched' : 'Unknown match';
  }

  function makeRow(entry: ActivityEntry): HTMLTableRowElement {
    const row = document.createElement('tr');
    const when = document.createElement('td');
    const timestamp = document.createElement('time');
    const date = new Date(entry.timestamp);
    timestamp.textContent = Number.isFinite(date.getTime()) ? date.toLocaleString() : 'Time unavailable';
    if (Number.isFinite(date.getTime())) timestamp.dateTime = date.toISOString();
    when.append(timestamp);

    const requestCell = document.createElement('td');
    const requestUrl = document.createElement('code'); requestUrl.className = 'request-url'; requestUrl.textContent = entry.request;
    const resource = document.createElement('p'); resource.className = 'small muted'; resource.textContent = `${entry.resourceType} · ${entry.site === null ? 'Site unavailable' : `Tab site: ${entry.site}`} · Tab ${entry.tabId}`;
    requestCell.append(requestUrl, resource);

    const match = document.createElement('td');
    const action = document.createElement('span'); action.className = entry.action === 'block' ? 'match-label match-block' : 'match-label'; action.textContent = actionLabel(entry.action);
    match.append(action);

    const rule = document.createElement('td');
    const source = document.createElement('p'); source.className = 'rule-source'; source.textContent = entry.source;
    const details = document.createElement('details');
    const summary = document.createElement('summary'); summary.textContent = `Rule ${entry.ruleId} · compiled condition`;
    const condition = document.createElement('code'); condition.className = 'rule-condition'; condition.textContent = entry.condition;
    details.append(summary, condition); rule.append(source, details);
    row.append(when, requestCell, match, rule);
    return row;
  }

  function render(view: ActivityView): void {
    canLog = view.available;
    element('page-count').textContent = view.blockedCount ?? 'Unavailable';
    element('activity-site').textContent = view.tabId === null ? 'All tabs — select Current tab for a page counter' : view.site ?? 'Selected tab is unavailable or is not an HTTP(S) page';
    element('error').textContent = view.error ?? '';
    element('log-unavailable').hidden = view.available;
    element('activity-empty').hidden = !view.available || view.entries.length > 0;
    element('activity-table-wrap').hidden = !view.available || view.entries.length === 0;
    element('activity-status').textContent = view.available ? `${view.entries.length.toLocaleString()} recent ${view.entries.length === 1 ? 'match' : 'matches'} · ${view.tabId === null ? 'all tabs' : 'current tab'} · refreshed ${new Date().toLocaleTimeString()}` : 'Detailed activity is unavailable in this installation';
    element('retention-note').textContent = `This session-only log keeps the newest ${view.limit.toLocaleString()} matches across tabs and removes a tab's records when it closes. It includes earlier pages visited in the same tab and is not a complete page history.`;
    const rows = document.createDocumentFragment();
    for (const entry of view.entries) rows.append(makeRow(entry));
    element('activity-rows').replaceChildren(rows);
  }

  async function load(): Promise<void> {
    const sequence = ++requestSequence;
    const tabId = readScope();
    busy(true);
    element('error').textContent = '';
    element('activity-status').textContent = 'Refreshing recent activity…';
    try {
      const view = await request<ActivityView>({ type: 'activity.get', tabId });
      if (sequence === requestSequence) render(view);
    } catch (error) {
      if (sequence !== requestSequence) return;
      element('page-count').textContent = 'Unavailable';
      element('activity-status').textContent = 'Activity could not be refreshed. Any visible rows are from the previous refresh.';
      element('error').textContent = errorMessage(error);
    } finally { if (sequence === requestSequence) busy(false); }
  }

  refreshButton.addEventListener('click', () => void load());
  scope.addEventListener('change', () => {
    location.hash = sourceTabId === null ? 'scope=all' : `tab=${sourceTabId}${scope.value === 'all' ? '&scope=all' : ''}`;
  });
  clearButton.addEventListener('click', () => void (async () => {
    busy(true);
    element('error').textContent = '';
    try { await request({ type: 'activity.clear' }); await load(); }
    catch (error) { element('error').textContent = errorMessage(error); }
    finally { busy(false); }
  })());
  element('options').addEventListener('click', () => void chrome.runtime.openOptionsPage());
  window.addEventListener('hashchange', () => void load());
  void load();
}

if (typeof document !== 'undefined') mountActivityPage();
